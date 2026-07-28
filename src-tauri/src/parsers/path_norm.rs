//! Path normalization for matching external agent session cwd to project roots.

/// Normalize a filesystem path for equality / prefix checks on Windows-ish paths.
pub fn normalize_path(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    // Extended-length prefix used by some agents (e.g. Grok).
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        s = rest.to_string();
    } else if let Some(rest) = s.strip_prefix("//?/") {
        s = rest.to_string();
    }
    // UNC keep leading `\\`; normalise separators.
    let is_unc = s.starts_with(r"\\");
    s = s.replace('/', "\\");
    if is_unc {
        // Collapse runs of `\` after the UNC prefix.
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
    // Trim trailing slash except drive root `D:\`
    while s.len() > 3 && s.ends_with('\\') {
        s.pop();
    }
    s
}

/// True if session_cwd belongs to project_root (equal or under it).
pub fn cwd_matches_project(session_cwd: &str, project_root: &str) -> bool {
    let a = normalize_path(session_cwd);
    let b = normalize_path(project_root);
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let al = a.to_ascii_lowercase();
    let bl = b.to_ascii_lowercase();
    if al == bl {
        return true;
    }
    // Under project: `D:\proj\sub` under `D:\proj`
    let prefix = if bl.ends_with('\\') {
        bl.clone()
    } else {
        format!("{bl}\\")
    };
    al.starts_with(&prefix)
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
        assert!(cwd_matches_project(
            r"D:\Myself\AgentsShell\fortest",
            root
        ));
        assert!(!cwd_matches_project(r"D:\Myself\Other", root));
    }
}
