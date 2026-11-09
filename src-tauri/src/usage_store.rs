//! usage_store — the Token usage ledger on disk (`~/.marionette/usage/`).
//!
//! The ledger used to live in WebView localStorage (one key shared with every
//! other preference). A single failed `setItem` (quota) switched recording off
//! for the rest of the run and lost everything since the last good write, and a
//! large ledger crowded out other settings. On disk there is no 5 MB ceiling:
//!
//! - `ledger.json` — day aggregates + session summaries. The frontend owns the
//!   shape (`src/lib/usageLedger.ts`); this module stores it opaquely. Writes are
//!   optimistic: `save` carries the `rev` the caller loaded, a stale rev returns
//!   `{ ok: false }` (a return value, not an error — reload, merge, retry). Every
//!   window lives in this one process, so an in-memory rev is exact across them.
//! - `sessions/<id>.jsonl` — per-session turn rows (context reading, tokens,
//!   tools, compactions …) appended one line per turn; read back on demand by the
//!   session detail view. Files idle for 190 days are pruned on first load.
//!
//! Red lines: never take the storage lock (this is its own file), never block
//! the main thread (commands run in spawn_blocking).

use serde::Serialize;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};
use tauri::{AppHandle, Emitter};

const LEDGER_FILE: &str = "ledger.json";
const SESSIONS_DIR: &str = "sessions";
/// The detail view never needs more than this many rows of one session.
const SESSION_READ_CAP: usize = 4000;
const SESSION_FILE_MAX_AGE: Duration = Duration::from_secs(190 * 24 * 60 * 60);
pub const LEDGER_CHANGED_EVENT: &str = "usage-ledger-changed";

struct State {
    rev: u64,
    pruned: bool,
}

static STATE: Mutex<State> = Mutex::new(State { rev: 1, pruned: false });
static APPEND_LOCK: Mutex<()> = Mutex::new(());
static ROOT_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

#[cfg(test)]
pub fn set_root_override_for_tests(path: Option<PathBuf>) {
    *ROOT_OVERRIDE.lock().unwrap_or_else(|p| p.into_inner()) = path;
}

fn root() -> Result<PathBuf, String> {
    if let Some(path) = ROOT_OVERRIDE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
    {
        return Ok(path);
    }
    if cfg!(test) {
        // Tests must never read or write the real ~/.marionette.
        return Err("usage_store root not set for tests".to_string());
    }
    Ok(crate::app_paths::global_dir()?.join("usage"))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerFile {
    pub rev: u64,
    /// Raw ledger JSON; `None` when nothing was saved yet.
    pub json: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveOutcome {
    pub ok: bool,
    pub rev: u64,
}

fn read_ledger_text(path: &Path) -> Option<String> {
    match fs::read_to_string(path) {
        Ok(text) if !text.trim().is_empty() => Some(text),
        _ => {
            // write_atomic moves the old file to `.json.bak` before the rename;
            // a crash in between leaves only the backup.
            let bak = path.with_extension("json.bak");
            fs::read_to_string(bak).ok().filter(|t| !t.trim().is_empty())
        }
    }
}

pub fn load() -> Result<LedgerFile, String> {
    let dir = root()?;
    let mut state = STATE.lock().unwrap_or_else(|p| p.into_inner());
    if !state.pruned {
        state.pruned = true;
        prune_session_files(&dir.join(SESSIONS_DIR), SystemTime::now());
    }
    Ok(LedgerFile {
        rev: state.rev,
        json: read_ledger_text(&dir.join(LEDGER_FILE)),
    })
}

pub fn save(json: &str, base_rev: u64) -> Result<SaveOutcome, String> {
    if !json.trim_start().starts_with('{') {
        return Err("usage ledger must be a JSON object".to_string());
    }
    let path = root()?.join(LEDGER_FILE);
    // Check-and-write under one lock: two windows saving from the same rev must
    // not both succeed.
    let mut state = STATE.lock().unwrap_or_else(|p| p.into_inner());
    if base_rev != state.rev {
        return Ok(SaveOutcome {
            ok: false,
            rev: state.rev,
        });
    }
    crate::storage::write_atomic(&path, json)?;
    state.rev += 1;
    Ok(SaveOutcome {
        ok: true,
        rev: state.rev,
    })
}

/// Session ids become file names: keep `[A-Za-z0-9._-]`, cap the length.
fn session_file_name(session_id: &str) -> Option<String> {
    let cleaned: String = session_id
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(120)
        .collect();
    let cleaned = cleaned.trim_matches('.').to_string();
    if cleaned.is_empty() {
        None
    } else {
        Some(format!("{cleaned}.jsonl"))
    }
}

fn session_path(session_id: &str) -> Result<PathBuf, String> {
    let name = session_file_name(session_id).ok_or_else(|| "empty session id".to_string())?;
    Ok(root()?.join(SESSIONS_DIR).join(name))
}

pub fn append_session(session_id: &str, lines: &[String]) -> Result<(), String> {
    if lines.is_empty() {
        return Ok(());
    }
    let path = session_path(session_id)?;
    let mut body = String::new();
    for line in lines {
        let line = line.trim();
        // One JSON object per line; anything else would corrupt the file.
        if line.is_empty() || line.contains('\n') || !line.starts_with('{') {
            continue;
        }
        body.push_str(line);
        body.push('\n');
    }
    if body.is_empty() {
        return Ok(());
    }
    let _serial = APPEND_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Create usage dir failed: {e}"))?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("Open session usage file failed: {e}"))?;
    file.write_all(body.as_bytes())
        .map_err(|e| format!("Append session usage failed: {e}"))
}

pub fn read_session(session_id: &str) -> Result<Vec<String>, String> {
    let path = session_path(session_id)?;
    let file = match fs::File::open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("Open session usage file failed: {e}")),
    };
    let mut rows: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { continue };
        if line.trim().is_empty() {
            continue;
        }
        if rows.len() == SESSION_READ_CAP {
            rows.pop_front();
        }
        rows.push_back(line);
    }
    Ok(rows.into_iter().collect())
}

/// Drop per-session files nobody appended to for SESSION_FILE_MAX_AGE.
fn prune_session_files(dir: &Path, now: SystemTime) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else { continue };
        if now.duration_since(modified).unwrap_or_default() > SESSION_FILE_MAX_AGE {
            let _ = fs::remove_file(&path);
        }
    }
}

fn log_warn(summary: &str, detail: &str) {
    crate::debug_log::append("usage", "warn", "-", summary, Some(detail));
}

#[tauri::command]
pub async fn usage_ledger_load() -> Result<LedgerFile, String> {
    tauri::async_runtime::spawn_blocking(load)
        .await
        .map_err(|e| format!("usage ledger load task failed: {e}"))?
        .inspect_err(|e| log_warn("usage ledger load failed", e))
}

#[tauri::command]
pub async fn usage_ledger_save(
    app: AppHandle,
    json: String,
    base_rev: u64,
) -> Result<SaveOutcome, String> {
    let outcome = tauri::async_runtime::spawn_blocking(move || save(&json, base_rev))
        .await
        .map_err(|e| format!("usage ledger save task failed: {e}"))?
        .inspect_err(|e| log_warn("usage ledger save failed", e))?;
    if outcome.ok {
        let _ = app.emit(LEDGER_CHANGED_EVENT, outcome.rev);
    }
    Ok(outcome)
}

#[tauri::command]
pub async fn usage_session_append(session_id: String, lines: Vec<String>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || append_session(&session_id, &lines))
        .await
        .map_err(|e| format!("usage session append task failed: {e}"))?
        .inspect_err(|e| log_warn("usage session append failed", e))
}

#[tauri::command]
pub async fn usage_session_read(session_id: String) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || read_session(&session_id))
        .await
        .map_err(|e| format!("usage session read task failed: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// usage_store keeps process-wide state (rev, root); tests share one lock.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn sandbox(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "marionette-usage-store-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        set_root_override_for_tests(Some(dir.clone()));
        dir
    }

    #[test]
    fn ledger_round_trip_and_stale_rev_is_a_return_value() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = sandbox("ledger");
        let first = load().unwrap();
        assert!(first.json.is_none());
        let saved = save(r#"{"version":1,"days":{}}"#, first.rev).unwrap();
        assert!(saved.ok);
        assert_eq!(saved.rev, first.rev + 1);
        // A second writer still holding the old rev is told to reload, not failed.
        let stale = save(r#"{"version":1,"days":{"x":1}}"#, first.rev).unwrap();
        assert!(!stale.ok);
        assert_eq!(stale.rev, saved.rev);
        let again = load().unwrap();
        assert_eq!(again.json.as_deref(), Some(r#"{"version":1,"days":{}}"#));
        assert!(save("not json", again.rev).is_err());
        set_root_override_for_tests(None);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn ledger_falls_back_to_the_backup_when_the_main_file_is_gone() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = sandbox("bak");
        fs::write(dir.join("ledger.json.bak"), r#"{"version":1}"#).unwrap();
        assert_eq!(load().unwrap().json.as_deref(), Some(r#"{"version":1}"#));
        set_root_override_for_tests(None);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn session_rows_append_and_read_back_in_order() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = sandbox("sessions");
        append_session("session-1", &[r#"{"t":1}"#.to_string(), r#"{"t":2}"#.to_string()]).unwrap();
        // Garbage and multi-line payloads are dropped rather than corrupting the file.
        append_session(
            "session-1",
            &["not json".to_string(), "{\"a\":1}\n{\"b\":2}".to_string(), r#"{"t":3}"#.to_string()],
        )
        .unwrap();
        assert_eq!(read_session("session-1").unwrap(), vec![r#"{"t":1}"#, r#"{"t":2}"#, r#"{"t":3}"#]);
        assert!(read_session("never-written").unwrap().is_empty());
        set_root_override_for_tests(None);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn session_ids_become_safe_file_names() {
        assert_eq!(session_file_name("session-1792813095629").as_deref(), Some("session-1792813095629.jsonl"));
        assert_eq!(session_file_name("../../etc/passwd").as_deref(), Some("_.._etc_passwd.jsonl"));
        assert_eq!(session_file_name("a\\b:c").as_deref(), Some("a_b_c.jsonl"));
        assert!(session_file_name("  ").is_none());
        assert!(session_file_name("...").is_none());
    }

    #[test]
    fn prune_keeps_recent_files() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = sandbox("prune").join(SESSIONS_DIR);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("fresh.jsonl"), "{}\n").unwrap();
        fs::write(dir.join("keep.txt"), "x").unwrap();
        // Pretend "now" is 200 days ahead: every jsonl looks stale.
        prune_session_files(&dir, SystemTime::now() + Duration::from_secs(200 * 24 * 3600));
        assert!(!dir.join("fresh.jsonl").exists());
        assert!(dir.join("keep.txt").exists());
        fs::write(dir.join("fresh.jsonl"), "{}\n").unwrap();
        prune_session_files(&dir, SystemTime::now());
        assert!(dir.join("fresh.jsonl").exists());
        set_root_override_for_tests(None);
    }
}
