//! Path normalization for matching external agent session cwd to project roots.

use std::path::Path;

/// Normalize a filesystem path for equality / prefix checks on Windows-ish paths.
pub fn normalize_path(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    // Extended-length prefix used by some agents (e.g. Grok).
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        s = rest.to_string();
    } else if let Some(rest) = s.strip_prefix("//?/") {
        s = rest.to_string();
    }
    let is_unc = s.starts_with(r"\\");
    s = s.replace('/', "\\");
    if is_unc {
        let rest = s.trim_start_matches('\\');
        let mut collapsed = String::from(r"\\");
        let mut prev_bs = false;
        for ch in rest.chars() {
            if ch == '\\' {
                if !prev_bs {
                    collapsed.push(ch);
                }
                prev_bs = true;
            } else {
                prev_bs = false;
                collapsed.push(ch);
            }
        }
        s = collapsed;
    } else {
        while s.contains(r"\\") {
            s = s.replace(r"\\", r"\");
        }
    }
    while s.len() > 3 && s.ends_with('\\') {
        s.pop();
    }
    s
}

/// Normalize, then canonicalize when the path exists (expands 8.3 short names
/// and strips a fresh `\\?\` that Windows canonicalize adds back).
pub fn canonicalize_for_match(raw: &str) -> String {
    let n = normalize_path(raw);
    if n.is_empty() {
        return n;
    }
    match Path::new(&n).canonicalize() {
        Ok(p) => normalize_path(&p.to_string_lossy()),
        Err(_) => n,
    }
}

/// True if session_cwd belongs to project_root (equal or under it).
pub fn cwd_matches_project(session_cwd: &str, project_root: &str) -> bool {
    if session_cwd.trim().is_empty() || project_root.trim().is_empty() {
        return false;
    }
    let a = canonicalize_for_match(session_cwd);
    let b = canonicalize_for_match(project_root);
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let al = a.to_ascii_lowercase();
    let bl = b.to_ascii_lowercase();
    if al == bl {
        return true;
    }
    let prefix = if bl.ends_with('\\') {
        bl.clone()
    } else {
        format!("{bl}\\")
    };
    al.starts_with(&prefix)
}

/// Claude-style encoding: `D:\Myself\Foo` → `D--Myself-Foo`.
pub fn encode_dashed_windows_path(project_root: &str) -> String {
    let n = normalize_path(project_root);
    if n.len() >= 2 && n.as_bytes()[1] == b':' {
        let drive = n.chars().next().unwrap();
        let rest = n[2..].trim_start_matches(['\\', '/']);
        let rest = rest.replace(['\\', '/'], "-");
        format!("{drive}--{rest}")
    } else {
        n.replace(['\\', '/'], "-")
    }
}

/// Pi-style cwd bucket: `D:\Projects\Jingzhe` → `--D--Projects-Jingzhe--`.
pub fn encode_pi_cwd_bucket(project_root: &str) -> String {
    format!("--{}--", encode_dashed_windows_path(project_root))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_verbatim_prefix() {
        assert_eq!(
            normalize_path(r"\\?\D:\Myself\AgentsShell"),
            r"D:\Myself\AgentsShell"
        );
    }

    #[test]
    fn matches_equal_and_child() {
        let root = r"D:\Myself\AgentsShell";
        assert!(cwd_matches_project(r"\\?\D:\Myself\AgentsShell", root));
        assert!(cwd_matches_project(r"D:/Myself/AgentsShell/", root));
        assert!(cwd_matches_project(r"D:\Myself\AgentsShell\fortest", root));
        assert!(!cwd_matches_project(r"D:\Myself\Other", root));
        assert!(!cwd_matches_project(r"D:\Myself\AgentsShellX", root));
    }

    #[test]
    fn encodes_claude_and_pi() {
        assert_eq!(
            encode_dashed_windows_path(r"D:\Projects\Marionette"),
            "D--Projects-Marionette"
        );
        assert_eq!(
            encode_pi_cwd_bucket(r"D:\Projects\Jingzhe"),
            "--D--Projects-Jingzhe--"
        );
    }
}
