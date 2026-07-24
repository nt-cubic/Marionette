use crate::models::ChangedFile;
use std::path::Path;
use std::process::Command;

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
    if path.trim().is_empty() || path.contains("..") {
        return Err("Invalid path".to_string());
    }
    let output = run_git(project_root, &["diff", "--no-color", "--", path])?;
    let mut text = String::from_utf8_lossy(&output.stdout).to_string();
    if text.trim().is_empty() {
        let untracked = run_git(project_root, &["status", "--porcelain", "--", path])?;
        let st = String::from_utf8_lossy(&untracked.stdout);
        if st.trim().starts_with("??") {
            return Ok(format!("(untracked) {path}\n"));
        }
    }
    const MAX: usize = 80_000;
    if text.len() > MAX {
        text.truncate(MAX);
        text.push_str("\n… (diff truncated)\n");
    }
    Ok(text)
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
}
