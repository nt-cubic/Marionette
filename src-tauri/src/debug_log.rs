//! Local developer diary — not shown in the product UI.
//! Path: `%USERPROFILE%\.agentshell\logs\dev.log`

use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static LOG_LOCK: Mutex<()> = Mutex::new(());

fn log_path() -> Result<PathBuf, String> {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .ok_or_else(|| "Unable to determine home for debug log".to_string())?;
    let dir = PathBuf::from(home).join(".agentshell").join("logs");
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
    let _guard = LOG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
            let clipped = if d.len() > 4000 { &d[..4000] } else { &d };
            line.push_str(" | ");
            line.push_str(clipped);
        }
    }
    line.push('\n');

    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(line.as_bytes());
        let _ = file.flush();
    }
}

fn sanitize_one_line(text: &str) -> String {
    text.chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect()
}

pub fn log_path_display() -> String {
    log_path()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "%USERPROFILE%\\.agentshell\\logs\\dev.log".to_string())
}
