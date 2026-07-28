//! Local developer diary — not shown in the product UI.
//! Path: `%USERPROFILE%\.marionette\logs\dev.log`

use std::collections::HashMap;
use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

static LOG_LOCK: Mutex<()> = Mutex::new(());

fn log_path() -> Result<PathBuf, String> {
    let dir = crate::app_paths::global_dir()?.join("logs");
    create_dir_all(&dir).map_err(|error| format!("Create log dir failed: {error}"))?;
    Ok(dir.join("dev.log"))
}

fn now_iso() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    // Compact sortable stamp without pulling chrono
    format!("{millis}")
}

/// Append one line to the local dev diary.
pub fn append(
    source: &str,
    level: &str,
    session_id: &str,
    summary: &str,
    detail: Option<&str>,
) {
    append_inner(source, level, session_id, summary, detail, false);
}

/// Logging path for the panic hook.
///
/// The hook runs *before* unwinding, so the panicking thread may still hold
/// `LOG_LOCK` — if the panic came from inside `append`, it does. Blocking on it
/// there deadlocks the thread permanently and leaves the guard held, taking
/// every other logger down with it. A panic report is worth dropping; the
/// process is not worth wedging.
pub fn append_from_panic_hook(summary: &str) {
    append_inner("panic", "error", "", summary, None, true);
}

fn append_inner(
    source: &str,
    level: &str,
    session_id: &str,
    summary: &str,
    detail: Option<&str>,
    never_block: bool,
) {
    let _guard = if never_block {
        match LOG_LOCK.try_lock() {
            Ok(guard) => Some(guard),
            // Held elsewhere (or by us). Write unlocked: a possibly interleaved
            // panic line beats no panic line and a frozen app.
            Err(_) => None,
        }
    } else {
        Some(LOG_LOCK.lock().unwrap_or_else(|e| e.into_inner()))
    };
    let Ok(path) = log_path() else {
        return;
    };

    // Soft rotate when file grows too large (~4MB)
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > 4 * 1024 * 1024 {
            let bak = path.with_extension("log.1");
            let _ = std::fs::rename(&path, bak);
        }
    }

    let mut line = format!(
        "[{ts}] [{level}] [{source}] session={session} {summary}",
        ts = now_iso(),
        level = level,
        source = source,
        session = if session_id.is_empty() { "-" } else { session_id },
        summary = sanitize_one_line(summary),
    );
    if let Some(detail) = detail {
        let d = sanitize_one_line(detail);
        if !d.is_empty() {
            line.push_str(" | ");
            line.push_str(clip_to_char_boundary(&d, MAX_DETAIL_BYTES));
        }
    }
    line.push('\n');

    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(line.as_bytes());
        let _ = file.flush();
    }
}

// ── Main-thread stall diagnostics ──────────────────────────────────────────
//
// "Not Responding" only says the main thread stopped pumping messages; it never
// says what it is stuck on, and a release build has no symbols to attach a
// debugger to. So commands announce themselves into an in-memory registry and
// the watchdog prints whatever was still running when the window went silent.

static INFLIGHT: OnceLock<Mutex<HashMap<u64, (&'static str, Instant)>>> = OnceLock::new();
static INFLIGHT_SEQ: AtomicU64 = AtomicU64::new(1);

fn inflight() -> &'static Mutex<HashMap<u64, (&'static str, Instant)>> {
    INFLIGHT.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Marks a command as running for as long as this value is alive.
///
/// Deliberately writes nothing on the happy path — it has to be cheap enough to
/// leave on every command permanently. Only the watchdog ever reads it.
pub struct CmdTrace(u64);

impl CmdTrace {
    pub fn new(name: &'static str) -> Self {
        let id = INFLIGHT_SEQ.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut map) = inflight().lock() {
            map.insert(id, (name, Instant::now()));
        }
        Self(id)
    }
}

impl Drop for CmdTrace {
    fn drop(&mut self) {
        if let Ok(mut map) = inflight().lock() {
            map.remove(&self.0);
        }
    }
}

/// Commands still executing, longest-running first.
pub fn inflight_summary() -> String {
    let Ok(map) = inflight().lock() else {
        return "<registry poisoned>".to_string();
    };
    if map.is_empty() {
        return "<none — main thread is not stuck in a command>".to_string();
    }
    let mut rows: Vec<(&str, u128)> = map
        .values()
        .map(|(name, since)| (*name, since.elapsed().as_millis()))
        .collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1));
    rows.iter()
        .map(|(name, ms)| format!("{name}({ms}ms)"))
        .collect::<Vec<_>>()
        .join(", ")
}

const MAX_DETAIL_BYTES: usize = 4000;

/// Trim to at most `max` **bytes** without splitting a character.
///
/// `&text[..max]` panics when the cut lands inside a multi-byte character, and
/// a CJK payload makes that the common case rather than the edge case. It used
/// to panic here while `LOG_LOCK` was held, and because the panic hook logs —
/// re-entering this same non-reentrant lock on the same thread — the thread
/// deadlocked before it could unwind and release the guard. Every other thread
/// that logged then blocked forever, silently, with nothing written anywhere.
fn clip_to_char_boundary(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn sanitize_one_line(text: &str) -> String {
    text.chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `read` tool result for a Chinese document is >4000 bytes of 3-byte
    /// characters, so the clip offset lands mid-character. Byte-slicing there
    /// panics *while holding `LOG_LOCK`*, and the panic hook's own `append`
    /// then deadlocks on the same non-reentrant lock — which is why the panic
    /// never reached the log and every other thread went quiet at once.
    #[test]
    fn clipping_a_long_multibyte_detail_does_not_panic() {
        let detail = "战".repeat(1334); // 4002 bytes; byte 4000 is mid-character
        assert!(detail.len() > 4000);
        assert!(!detail.is_char_boundary(4000), "test must exercise the bad offset");

        append("test", "info", "test-session", "long multibyte detail", Some(&detail));
    }

    #[test]
    fn clip_stays_on_char_boundaries() {
        let cjk = "战".repeat(1334); // 4002 bytes
        let clipped = clip_to_char_boundary(&cjk, 4000);
        assert!(clipped.len() <= 4000);
        assert_eq!(clipped.len() % 3, 0, "must end on a whole character");
        assert!(cjk.starts_with(clipped), "must be a prefix, not a re-encode");

        assert_eq!(clip_to_char_boundary("short", 4000), "short");
        assert_eq!(clip_to_char_boundary("战战战", 1), "", "no room for one char");
    }

    // Asserts only on this test's own command name — the registry is global and
    // the suite runs in parallel.
    #[test]
    fn inflight_names_the_command_that_is_running() {
        let name = "unit_test_probe_command";
        assert!(!inflight_summary().contains(name));

        let trace = CmdTrace::new(name);
        let during = inflight_summary();
        assert!(
            during.contains(name),
            "a running command must be nameable while the UI is frozen, got: {during}"
        );

        drop(trace);
        assert!(
            !inflight_summary().contains(name),
            "a finished command must not linger and frame itself"
        );
    }
}

pub fn log_path_display() -> String {
    log_path()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "%USERPROFILE%\\.marionette\\logs\\dev.log".to_string())
}
