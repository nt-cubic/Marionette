use crate::models::ChangedFile;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

/// Current branch name for the project root.
/// `None` if not a git repo / no HEAD. Detached HEAD returns `HEAD (abcdef)`.
pub fn get_current_branch(project_root: &Path) -> Result<Option<String>, String> {
    if !project_root.is_dir() {
        return Err(format!("Not a directory: {}", project_root.display()));
    }

    let output = run_git(project_root, &["branch", "--show-current"])?;
    let code = output.status.code().unwrap_or(-1);
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if code == 128 || stderr.to_lowercase().contains("not a git repository") {
        return Ok(None);
    }
    if code != 0 && stdout.is_empty() {
        if stderr.to_lowercase().contains("not recognized")
            || stderr.to_lowercase().contains("not found")
            || stderr.to_lowercase().contains("enoent")
        {
            return Err("git is not available on PATH".to_string());
        }
        return Ok(None);
    }

    if !stdout.is_empty() {
        return Ok(Some(stdout));
    }

    // Detached HEAD: `branch --show-current` is empty — show a short SHA.
    let rev = run_git(project_root, &["rev-parse", "--short", "HEAD"])?;
    let sha = String::from_utf8_lossy(&rev.stdout).trim().to_string();
    if sha.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!("HEAD ({sha})")))
}

/// Run `git status --porcelain` in the project root. Empty vec if not a repo.
pub fn get_changed_files(project_root: &Path) -> Result<Vec<ChangedFile>, String> {
    if !project_root.is_dir() {
        return Err(format!("Not a directory: {}", project_root.display()));
    }

    let output = run_git(project_root, &["status", "--porcelain", "-uall"])?;
    let code = output.status.code().unwrap_or(-1);
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();

    if code == 128 || stderr.to_lowercase().contains("not a git repository") {
        return Ok(Vec::new());
    }
    if code != 0 && stdout.trim().is_empty() {
        if stderr.to_lowercase().contains("not recognized")
            || stderr.to_lowercase().contains("not found")
            || stderr.to_lowercase().contains("enoent")
        {
            return Err("git is not available on PATH".to_string());
        }
        return Ok(Vec::new());
    }

    Ok(parse_porcelain(&stdout))
}

/// Optional read-only single-file diff (truncated).
pub fn get_file_diff(project_root: &Path, path: &str) -> Result<String, String> {
    let path = project_relative_path(project_root, path)?;
    // Compare against HEAD so staged and unstaged edits are both visible.
    // Repositories without a HEAD fall back to the working-tree diff below.
    let output = run_git(project_root, &["diff", "--no-color", "HEAD", "--", &path])?;
    let mut text = String::from_utf8_lossy(&output.stdout).to_string();
    if text.trim().is_empty() {
        let working_tree = run_git(project_root, &["diff", "--no-color", "--", &path])?;
        text = String::from_utf8_lossy(&working_tree.stdout).to_string();
    }
    if text.trim().is_empty() {
        let untracked = run_git(project_root, &["status", "--porcelain", "--", &path])?;
        let st = String::from_utf8_lossy(&untracked.stdout);
        if st.trim().starts_with("??") {
            // `git diff` does not include untracked files. `--no-index` gives
            // the same unified format for a new file, including its contents.
            let added = run_git(
                project_root,
                &["diff", "--no-color", "--no-index", "--", "/dev/null", &path],
            )?;
            text = String::from_utf8_lossy(&added.stdout).to_string();
            if text.trim().is_empty() {
                return Ok(format!("(untracked) {path}\n"));
            }
        }
    }
    const MAX: usize = 80_000;
    if text.len() > MAX {
        // Byte-cap must land on a char boundary — CJK/emoji diffs make a raw
        // `truncate(80000)` panic mid-codepoint and take the whole app down.
        text.truncate(floor_char_boundary(&text, MAX));
        text.push_str("\n… (diff truncated)\n");
    }
    Ok(text)
}

/// Largest `<= max` that is a UTF-8 character boundary (or 0).
fn floor_char_boundary(text: &str, max: usize) -> usize {
    if max >= text.len() {
        return text.len();
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}

/// Git pathspecs must be relative to the project. ACP often sends an absolute
/// Windows path, while the Changed Files panel sends a relative one.
fn project_relative_path(project_root: &Path, raw: &str) -> Result<String, String> {
    if raw.trim().is_empty() {
        return Err("Invalid path".to_string());
    }

    let requested = Path::new(raw);
    let relative = if requested.is_absolute() {
        requested
            .strip_prefix(project_root)
            .map_err(|_| "Path is outside the project".to_string())?
    } else {
        requested
    };

    let mut clean = PathBuf::new();
    for component in relative.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => clean.push(part),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("Invalid path".to_string());
            }
        }
    }
    if clean.as_os_str().is_empty() {
        return Err("Invalid path".to_string());
    }
    Ok(clean.to_string_lossy().replace('\\', "/"))
}

fn run_git(cwd: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(cwd);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.output()
        .map_err(|e| format!("Failed to run git: {e}"))
}

fn parse_porcelain(stdout: &str) -> Vec<ChangedFile> {
    let mut files = Vec::new();
    for line in stdout.lines() {
        if line.len() < 4 {
            continue;
        }
        let status = &line[..2];
        let path_part = line[2..].trim_start();
        let path = if let Some((_, new_path)) = path_part.split_once(" -> ") {
            new_path.trim().to_string()
        } else {
            path_part.trim().trim_matches('"').to_string()
        };
        if path.is_empty() {
            continue;
        }
        files.push(ChangedFile {
            path,
            change_type: map_status(status),
        });
    }
    files
}

fn map_status(xy: &str) -> String {
    let bytes = xy.as_bytes();
    let x = bytes.first().copied().unwrap_or(b' ') as char;
    let y = bytes.get(1).copied().unwrap_or(b' ') as char;
    if x == '?' || y == '?' {
        return "untracked".to_string();
    }
    if x == 'D' || y == 'D' {
        return "deleted".to_string();
    }
    if x == 'A' || y == 'A' || x == 'C' || y == 'C' {
        return "added".to_string();
    }
    if x == 'M' || y == 'M' || x == 'R' || y == 'R' || x == 'U' || y == 'U' {
        return "modified".to_string();
    }
    "modified".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_lines() {
        let raw = " M src/a.rs\nA  src/b.rs\n?? notes.txt\n D gone.rs\n";
        let files = parse_porcelain(raw);
        assert_eq!(files.len(), 4);
        assert_eq!(files[0].change_type, "modified");
        assert_eq!(files[1].change_type, "added");
        assert_eq!(files[2].change_type, "untracked");
        assert_eq!(files[3].change_type, "deleted");
    }

    /// Diffs with CJK content are multi-byte; a fixed byte cap often lands
    /// mid-character. That used to panic the process on send/diff refresh.
    #[test]
    fn floor_char_boundary_never_splits_multibyte() {
        let cjk = "战".repeat(30_000); // 90_000 bytes
        assert!(cjk.len() > 80_000);
        assert!(!cjk.is_char_boundary(80_000));

        let end = floor_char_boundary(&cjk, 80_000);
        assert!(end <= 80_000);
        assert!(cjk.is_char_boundary(end));
        assert_eq!(end % 3, 0);

        let mut text = cjk;
        text.truncate(end);
        text.push_str("\n… (diff truncated)\n");
        assert!(text.ends_with("\n… (diff truncated)\n"));
    }

    #[test]
    fn floor_char_boundary_short_and_exact() {
        assert_eq!(floor_char_boundary("abc", 10), 3);
        assert_eq!(floor_char_boundary("abc", 3), 3);
        assert_eq!(floor_char_boundary("战", 1), 0);
        assert_eq!(floor_char_boundary("战", 3), 3);
    }
}
