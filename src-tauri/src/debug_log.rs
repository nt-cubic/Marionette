//! Local developer diary — not shown in the product UI.
//! Path: `%USERPROFILE%\.marionette\logs\dev.log`

use std::collections::HashMap;
use std::fs::{create_dir_all, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Backlog cap for the async info path. Bounded so a stuck disk cannot grow
/// the queue without limit; when full, info lines are dropped (best-effort by
/// contract — warn/error/panic never take this path).
const CHANNEL_BOUND: usize = 8192;
/// Soft-rotate threshold, tracked in bytes written by the writer thread
/// (no per-line metadata() syscall).
const ROTATE_BYTES: u64 = 4 * 1024 * 1024;

/// Info lines go to a resident writer thread; nothing on the caller side ever
/// touches the disk or takes a lock. Format happens here (timestamps stay
/// caller-side, so cross-level ordering is recoverable from the stamps).
static ASYNC_TX: OnceLock<SyncSender<String>> = OnceLock::new();

fn async_tx() -> &'static SyncSender<String> {
    ASYNC_TX.get_or_init(|| {
        let (tx, rx) = sync_channel::<String>(CHANNEL_BOUND);
        thread::Builder::new()
            .name("devlog-writer".to_string())
            .spawn(move || writer_loop(rx))
            .ok();
        tx
    })
}

/// Resident writer: keeps one open handle, appends each line, rotates by its
/// own byte counter, flushes periodically so a crash loses at most a moment
/// of info chatter (warn/error/panic are written synchronously elsewhere).
fn writer_loop(rx: Receiver<String>) {
    let mut file: Option<File> = None;
    let mut written: u64 = 0;
    while let Ok(line) = rx.recv() {
        if file.is_none() {
            match open_log() {
                Ok(f) => file = Some(f),
                Err(_) => continue, // retry on the next line; drop this one
            }
        }
        let Some(f) = file.as_mut() else { continue };
        let _ = f.write_all(line.as_bytes());
        let _ = f.flush();
        written += line.len() as u64;
        if written > ROTATE_BYTES {
            drop(file.take());
            if let Ok(path) = log_path() {
                // 三代轮转：崩溃现场往往在上一代日志里，只留一代会在连续轮转后
                // 把事故证据冲掉。最旧的 .log.3 先淘汰，再逐代上移。
                let _ = std::fs::remove_file(path.with_extension("log.3"));
                let _ = std::fs::rename(
                    path.with_extension("log.2"),
                    path.with_extension("log.3"),
                );
                let _ = std::fs::rename(
                    path.with_extension("log.1"),
                    path.with_extension("log.2"),
                );
                let _ = std::fs::rename(&path, path.with_extension("log.1"));
            }
            written = 0;
            file = open_log().ok();
        }
    }
}

fn open_log() -> Result<File, String> {
    let path = log_path()?;
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())
}

fn log_path() -> Result<PathBuf, String> {
    // Test binaries share this module via #[cfg(test)] and lazily hit the same
    // sink. Without this guard, `cargo test` writes its fixture chatter (moved-
    // aside engines, version=9.9.9, corrupt-index restores) straight into the
    // real diary, interleaving with — and rotating away — crash evidence
    // (2026-10-31). Formatting still runs in tests; only the file sink is cut.
    if cfg!(test) {
        return Err("test sink disabled".to_string());
    }
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
    append_inner(source, level, session_id, summary, detail);
}

/// Logging path for the panic hook.
///
/// With `panic = "abort"` the process dies as soon as the hook returns, so a
/// channel send would lose the report — the panic line is written inline.
/// There is no shared lock to deadlock on: the async writer owns its own
/// handle, and append-mode writes interleave at worst, which beats dropping
/// the one line that explains the crash.
pub fn append_from_panic_hook(summary: &str) {
    append_inner("panic", "error", "", summary, None);
}

fn append_inner(
    source: &str,
    level: &str,
    session_id: &str,
    summary: &str,
    detail: Option<&str>,
) {
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

    // Hot path: info chatter (every ACP token, ~93/s × sessions) must not
    // touch the disk on the caller's thread. Bounded channel, never blocks:
    // a full queue drops the line instead of stalling dispatch.
    if !matches!(level, "error" | "warn") && source != "panic" {
        match async_tx().try_send(line) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {} // disk stuck; drop info line
            Err(TrySendError::Disconnected(_)) => {}
        }
        return;
    }

    // warn/error/panic stay synchronous and flushed: a crash must leave the
    // stall/panic line on disk. These are rare, so per-call open is fine.
    if let Ok(mut file) = open_log() {
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

    /// Regression for the 2026-10-31 incident: `cargo test` fixture lines
    /// (engine move-aside, version=9.9.9, storage corrupt-index restore) showed
    /// up in the real `~/.marionette/logs/dev.log`, polluting the crash diary.
    /// The sink must refuse the real path in every test build.
    #[test]
    fn test_binaries_never_touch_the_real_dev_log() {
        assert!(log_path().is_err());
        assert!(open_log().is_err());
    }

    /// A `read` tool result for a Chinese document is >4000 bytes of 3-byte
    /// characters, so the clip offset lands mid-character. Byte-slicing there
    /// used to panic while holding the global log lock, and the panic hook's
    /// own `append` then deadlocked on the same non-reentrant lock — which is
    /// why the panic never reached the log and every other thread went quiet
    /// at once. Formatting (where the clip happens) must stay panic-free on
    /// both the async info path and the sync warn/error path.
    #[test]
    fn clipping_a_long_multibyte_detail_does_not_panic() {
        let detail = "战".repeat(1334); // 4002 bytes; byte 4000 is mid-character
        assert!(detail.len() > 4000);
        assert!(!detail.is_char_boundary(4000), "test must exercise the bad offset");

        append("test", "info", "test-session", "long multibyte detail", Some(&detail));
        append("test", "warn", "test-session", "long multibyte detail", Some(&detail));
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
