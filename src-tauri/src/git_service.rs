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
/// Per-file diff fingerprints from ONE whole-tree diff.
///
/// The pre-send snapshot used to request `get_file_diff` once per changed
/// file — one `git diff` process each (two for untracked paths, both empty).
/// A Unity-scale `-uall` status lists thousands of files, which stalled the
/// composer for minutes before the prompt ever reached the agent
/// (2026-10-18). This walks a single diff instead; files absent from the diff
/// (e.g. untracked) keep the JS-side empty-string fingerprint.
pub fn get_file_fingerprints(
    project_root: &Path,
) -> Result<std::collections::HashMap<String, String>, String> {
    if !project_root.is_dir() {
        return Err(format!("Not a directory: {}", project_root.display()));
    }
    let mut output = run_git(project_root, &["diff", "--no-color", "--no-ext-diff", "HEAD"])?;
    if output.status.code() == Some(128) {
        // No HEAD yet (repo without commits): fall back to the index diff,
        // mirroring `get_file_diff`.
        output = run_git(project_root, &["diff", "--no-color", "--no-ext-diff"])?;
    }
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    Ok(parse_diff_fingerprints(&stdout))
}

/// Split a unified diff into per-file sections, hashing each incrementally
/// (FNV-1a over UTF-8 bytes, `len:hash` like the JS `fingerprintDiff`) so the
/// full text is never retained. Keys mirror `parse_porcelain` output: the
/// `b/`-side path for modifications/additions, the `a/`-side for deletions,
/// `rename to`/`copy to` for pure renames, quotes stripped and C-escape
/// octets kept verbatim.
fn parse_diff_fingerprints(stdout: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let mut current: Option<String> = None;
    let mut a_side: Option<String> = None;
    let mut headers_done = false;
    let mut len: usize = 0;
    let mut hash: u32 = 216_613_6261;
    for line in stdout.split_inclusive('\n') {
        if line.starts_with("diff --git ") || line.starts_with("diff --cc ") {
            if let Some(path) = current.take() {
                map.insert(path, format!("{len}:{hash}"));
            }
            a_side = None;
            headers_done = false;
            len = 0;
            hash = 216_613_6261;
            continue;
        }
        if !headers_done {
            if let Some(rest) = line.strip_prefix("--- ") {
                if let Some(path) = diff_side_path(rest, 'a') {
                    a_side = Some(path);
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix("+++ ") {
                current = diff_side_path(rest, 'b').or_else(|| a_side.take());
                headers_done = true;
                continue;
            }
            if let Some(rest) = line.strip_prefix("rename to ") {
                current = Some(unquote_path(rest));
                continue;
            }
            if let Some(rest) = line.strip_prefix("copy to ") {
                current = Some(unquote_path(rest));
                continue;
            }
            if line.starts_with("@@") || line.starts_with("@@@") {
                headers_done = true;
            }
        }
        for byte in line.as_bytes() {
            hash ^= u32::from(*byte);
            hash = hash.wrapping_mul(16_777_619);
        }
        len += line.len();
    }
    if let Some(path) = current.take() {
        map.insert(path, format!("{len}:{hash}"));
    }
    map
}

/// Extract the path from a `--- a/…` / `+++ b/…` header value (rest of the
/// line). `/dev/null` yields `None`.
fn diff_side_path(rest: &str, side: char) -> Option<String> {
    let rest = rest.trim_end_matches(['\n', '\r']);
    if rest == "/dev/null" {
        return None;
    }
    let prefix = format!("{side}/");
    let body = rest.strip_prefix(&prefix).map(str::to_string).or_else(|| {
        unquote_path(rest).strip_prefix(&prefix).map(str::to_string)
    })?;
    Some(body)
}

/// Strip git's optional C-style quoting. Escape octets stay verbatim so the
/// result matches `parse_porcelain` keys.
fn unquote_path(raw: &str) -> String {
    let trimmed = raw.trim_end_matches(['\n', '\r']);
    trimmed
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(trimmed)
        .to_string()
}

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

    #[test]
    fn diff_fingerprints_split_by_file() {
        let raw = "diff --git a/src/one.rs b/src/one.rs\nindex 111..222 100644\n--- a/src/one.rs\n+++ b/src/one.rs\n@@ -1 +1 @@\n-old\n+new\ndiff --git a/src/two.rs b/src/two.rs\n--- a/src/two.rs\n+++ b/src/two.rs\n@@ -1 +1 @@\n-x\n+y\n";
        let map = parse_diff_fingerprints(raw);
        assert_eq!(map.len(), 2);
        assert!(map.contains_key("src/one.rs"));
        assert!(map.contains_key("src/two.rs"));
        // Different contents -> different fingerprints.
        assert_ne!(map["src/one.rs"], map["src/two.rs"]);
    }

    #[test]
    fn diff_fingerprints_key_like_porcelain() {
        // CJK path: git quotes it and keeps octal escapes; parse_porcelain
        // strips quotes but keeps the escapes verbatim — so must we.
        let quoted = "diff --git \"a/src/\\346\\224\\271.rs\" \"b/src/\\346\\224\\271.rs\"\n--- \"a/src/\\346\\224\\271.rs\"\n+++ \"b/src/\\346\\224\\271.rs\"\n@@ -1 +1 @@\n-a\n+b\n";
        let map = parse_diff_fingerprints(quoted);
        let key = "src/\\346\\224\\271.rs";
        assert_eq!(parse_porcelain(&format!("?? \"{key}\"\n"))[0].path, key);
        assert!(map.contains_key(key), "got keys: {map:?}");

        // Deleted file: keyed by the a/-side.
        let deleted = "diff --git a/gone.rs b/gone.rs\ndeleted file mode 100644\n--- a/gone.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\n";
        assert!(parse_diff_fingerprints(deleted).contains_key("gone.rs"));

        // Path with spaces: unquoted, rest-of-line is the path.
        let spaced = "diff --git a/foo bar.txt b/foo bar.txt\n--- a/foo bar.txt\n+++ b/foo bar.txt\n@@ -1 +1 @@\n-a\n+b\n";
        assert!(parse_diff_fingerprints(spaced).contains_key("foo bar.txt"));
    }

    #[test]
    fn diff_fingerprints_rename_and_ignores_content_headers() {
        // Pure rename has no ---/+++ pair; key comes from `rename to`.
        let rename = "diff --git a/old.rs b/new.rs\nsimilarity index 98%\nrename from old.rs\nrename to new.rs\n@@ -1 +1 @@\n-a\n+b\n";
        let map = parse_diff_fingerprints(rename);
        assert!(map.contains_key("new.rs"), "got keys: {map:?}");

        // A removed content line that looks like a header (`--- a/x`) after a
        // hunk must not create a phantom key.
        let tricky = "diff --git a/src/one.rs b/src/one.rs\n--- a/src/one.rs\n+++ b/src/one.rs\n@@ -1,2 +1,2 @@\n ctx\n--- a/trap.rs\n+++ b/trap.rs\n";
        let map = parse_diff_fingerprints(tricky);
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("src/one.rs"));
    }

    #[test]
    fn diff_fingerprints_empty() {
        assert!(parse_diff_fingerprints("").is_empty());
        assert!(parse_diff_fingerprints("nothing to see here\n").is_empty());
    }
}
