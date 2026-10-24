//! Marionette data directories.
//!
//! Preferred: `.marionette` (project) and `~/.marionette` (global).
//! Legacy AgentShell paths (`.agentshell`) are migrated once via rename when safe.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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
///
/// 双开竞态：两个实例同时首启时，一个在 rename、另一个可能已经
/// `create_dir_all` 出空的新目录，旧数据就永远留在 `.agentshell`（数据分叉）。
/// 所以「检查 + rename」用父目录里一把 `create_new` 迁移锁包成跨进程原子段，
/// 拿不到锁的一方有界轮询等迁移完成。
fn migrate_dir_once(parent: PathBuf) -> PathBuf {
    let neu = parent.join(DIR_NAME);
    let old = parent.join(LEGACY_DIR_NAME);
    if old.exists() && !neu.exists() {
        let lock_path = parent.join(format!("{DIR_NAME}.migrate.lock"));
        if try_acquire_migrate_lock(&lock_path).is_ok() {
            if !neu.exists() && old.exists() {
                // Best-effort one-shot migrate from AgentShell layout. Ignore
                // errors — callers create_dir_all on the new path as needed.
                let _ = fs::rename(&old, &neu);
            }
            let _ = fs::remove_file(&lock_path);
        } else {
            // 另一实例正在迁移：等它完成（有界，不无限自旋）。
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline {
                if !old.exists() || neu.exists() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
    neu
}

/// 创建即占用的迁移锁；已存在 = 别的实例持有（或持有者崩溃留下的——超龄回收）。
fn try_acquire_migrate_lock(lock_path: &Path) -> Result<fs::File, ()> {
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(lock_path)
    {
        Ok(file) => Ok(file),
        Err(_) => {
            let stale = fs::metadata(lock_path)
                .and_then(|meta| meta.modified())
                .map(|modified| modified.elapsed().unwrap_or_default() > Duration::from_secs(30))
                .unwrap_or(true);
            if stale {
                let _ = fs::remove_file(lock_path);
                return fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(lock_path)
                    .map_err(|_| ());
            }
            Err(())
        }
    }
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
