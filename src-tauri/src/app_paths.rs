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

/// Verify the global data dir accepts writes, and name what failed if not.
///
/// Existence proves nothing here: a process with restricted rights (a restricted
/// token, or an image whose mandatory integrity label is Low) still reads the
/// folder while every write is denied. Startup needs writes — instance lock, dev
/// diary, session state — so probe one and report the real OS error, which is the
/// only way a caller can tell "unusable folder" apart from "should be fine".
pub fn ensure_global_dir_writable() -> Result<(), String> {
    let dir = global_dir()?;
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;

    let probe = dir.join("write-probe.tmp");
    fs::write(&probe, b"marionette").map_err(|e| format!("{}: {e}", probe.display()))?;
    // Cleanup failure is not fatal — the write above is the part that matters.
    let _ = fs::remove_file(&probe);
    Ok(())
}

/// Drop the Windows verbatim (`\\?\`) prefix from a path string.
///
/// `fs::canonicalize` hands back verbatim paths, which our own file I/O accepts
/// but other programs often do not. A Node agent runtime that takes the root of
/// `\\?\D:\Projects\x` gets `D:` — a drive-relative path — and dies in
/// `realpath` before the first turn. Verbatim UNC keeps its `\\server\share`
/// spelling instead of turning into a relative path.
pub fn plain_path(path: &str) -> String {
    match path.strip_prefix(r"\\?\UNC\") {
        Some(rest) => format!(r"\\{rest}"),
        None => path.strip_prefix(r"\\?\").unwrap_or(path).to_string(),
    }
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

#[cfg(test)]
mod tests {
    use super::plain_path;

    #[test]
    fn plain_path_drops_the_verbatim_prefix() {
        assert_eq!(plain_path(r"\\?\D:\Projects\x"), r"D:\Projects\x");
        // Drive-relative spellings must keep their meaning: `D:` alone is not
        // `D:\`, and an agent that derives it from a verbatim path dies.
        assert_eq!(plain_path(r"\\?\D:"), r"D:");
        assert_eq!(plain_path(r"\\?\D:\"), r"D:\");
        assert_eq!(
            plain_path(r"\\?\UNC\server\share\x"),
            r"\\server\share\x"
        );
        // Already plain, or POSIX, is left alone.
        assert_eq!(plain_path(r"D:\Projects\x"), r"D:\Projects\x");
        assert_eq!(plain_path("/home/x"), "/home/x");
    }
}
