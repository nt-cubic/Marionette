//! Marionette data directories.
//!
//! Preferred: `.marionette` (project) and `~/.marionette` (global).
//! Legacy AgentShell paths (`.agentshell`) are migrated once via rename when safe.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub const DIR_NAME: &str = ".marionette";
pub const LEGACY_DIR_NAME: &str = ".agentshell";

/// Virtual project ID for chat sessions — not shown in the project list.
pub const CHAT_PROJECT_ID: &str = "project-chat";

/// Global app dir under the user profile (`%USERPROFILE%\.marionette`).
pub fn global_dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .ok_or_else(|| "Unable to determine the user home directory".to_string())?;
    Ok(migrate_dir_once(PathBuf::from(home)))
}

/// Per-project data root (`{project}/.marionette`).
pub fn project_dir(project_root: &Path) -> PathBuf {
    migrate_dir_once(project_root.to_path_buf())
}

/// If only the legacy folder exists, rename it to the new name. Prefer the new
/// path when both exist (do not delete legacy automatically).
fn migrate_dir_once(parent: PathBuf) -> PathBuf {
    let neu = parent.join(DIR_NAME);
    let old = parent.join(LEGACY_DIR_NAME);
    if !neu.exists() && old.exists() {
        // Best-effort one-shot migrate from AgentShell layout. Ignore errors —
        // callers create_dir_all on the new path as needed.
        let _ = fs::rename(&old, &neu);
    }
    neu
}

/// Ensure project data subdirs exist (sessions / transcripts / handoff).
pub fn ensure_project_layout(project_root: &Path) -> Result<PathBuf, String> {
    let root = project_dir(project_root);
    for sub in ["sessions", "transcripts", "handoff"] {
        fs::create_dir_all(root.join(sub))
            .map_err(|e| format!("Create {DIR_NAME}/{sub} failed: {e}"))?;
    }
    Ok(root)
}

/// Returns the current working directory of the process.
pub fn current_dir() -> Result<String, String> {
    env::current_dir()
        .map_err(|e| format!("Failed to get current directory: {e}"))
        .and_then(|p| {
            p.to_str()
                .map(|s| s.to_string())
                .ok_or_else(|| "Current directory path is not valid UTF-8".to_string())
        })
}
