//! Open files, folders and URLs that appear in agent output.
//!
//! Agent text is untrusted: every entry point here is a deliberate click in the
//! UI, nothing opens on its own, no argument is ever handed to a shell
//! interpreter (`explorer.exe` / `xdg-open` / `open` take the path as a real
//! argv entry), and launching an executable takes an explicit `force` from a
//! confirmation the user answered.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Extensions Windows would *run* rather than open in an editor.
const RISKY_EXTENSIONS: &[&str] = &[
    "exe", "com", "bat", "cmd", "msi", "msp", "scr", "pif", "cpl", "hta", "jar", "lnk", "reg",
    "ps1", "psm1", "vbs", "vbe", "js", "jse", "wsf", "wsh", "sh", "app",
];

fn is_url(target: &str) -> bool {
    let lower = target.trim().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// `file:///D:/x` / `file://localhost/C:/x` / `file://C:/x` → `D:/x` / `C:/x`.
///
/// Only the absolute forms an agent can point at are handled; UNC hosts
/// (`file://server/share`) stay unsupported — agent text should name local
/// files. Percent-encoding is deliberately not decoded: agents print raw
/// paths, not URL-escaped ones.
fn strip_file_url(target: &str) -> Option<String> {
    let rest = target.trim().strip_prefix("file://")?;
    let rest = if rest.len() >= 9 && rest.as_bytes()[0..9].eq_ignore_ascii_case(b"localhost") {
        &rest[9..]
    } else {
        rest
    };
    let rest = rest.strip_prefix('/').unwrap_or(rest);
    if rest.is_empty() {
        return None;
    }
    Some(rest.to_string())
}

/// Normalize a transcript string into a local target: `file://` URL → bare
/// path, anything else passes through untouched.
fn strip_scheme(target: String) -> String {
    strip_file_url(&target).unwrap_or(target)
}

/// `\\?\D:\x` → `D:\x`. The extended prefix leaks in from `fs::canonicalize`
/// and shells refuse to open it.
fn strip_extended_prefix(path: &str) -> String {
    path.strip_prefix(r"\\?\")
        .map(str::to_string)
        .unwrap_or_else(|| path.to_string())
}

/// The argument handed to the OS file opener.
///
/// On Windows, `explorer.exe` cannot open paths containing forward slashes —
/// it silently falls back to the Documents folder — so paths are normalized
/// to backslashes before they reach it. URLs never pass through here; they
/// keep their own slashes.
fn to_explorer_arg(path: &Path) -> String {
    #[cfg(target_os = "windows")]
    {
        path.to_string_lossy().replace('/', "\\")
    }
    #[cfg(not(target_os = "windows"))]
    {
        path.to_string_lossy().into_owned()
    }
}

fn is_risky(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| RISKY_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Resolve a raw string from a transcript into something openable.
///
/// Handles `file.ts:42` line suffixes and project-relative paths (`cwd`).
fn resolve_path(raw: &str, cwd: Option<&str>) -> Option<PathBuf> {
    let cleaned = strip_extended_prefix(raw.trim().trim_matches('"'));
    if cleaned.is_empty() {
        return None;
    }

    let mut candidates: Vec<String> = vec![cleaned.clone()];
    // `src/app/App.tsx:42` / `:42:7` — editors print these, the filesystem does not.
    if let Some(base) = cleaned.rsplit_once(':').map(|(head, tail)| {
        if tail.chars().all(|c| c.is_ascii_digit()) && !tail.is_empty() {
            head.to_string()
        } else {
            cleaned.clone()
        }
    }) {
        if base != cleaned {
            let base2 = base
                .rsplit_once(':')
                .filter(|(_, tail)| tail.chars().all(|c| c.is_ascii_digit()) && !tail.is_empty())
                .map(|(head, _)| head.to_string());
            candidates.push(base.clone());
            if let Some(b2) = base2 {
                candidates.push(b2);
            }
        }
    }

    for candidate in candidates {
        let as_path = PathBuf::from(&candidate);
        if as_path.is_absolute() && as_path.exists() {
            return Some(as_path);
        }
        if let Some(root) = cwd {
            let joined = Path::new(&strip_extended_prefix(root)).join(&candidate);
            if joined.exists() {
                return Some(joined);
            }
        }
    }
    None
}

/// What a link in agent text points at — drives click behaviour and the menu.
#[tauri::command(async)]
pub fn resolve_link_target(target: String, cwd: Option<String>) -> Value {
    if is_url(&target) {
        return json!({ "kind": "url", "target": target.trim(), "risky": false });
    }
    let local = strip_scheme(target);
    match resolve_path(&local, cwd.as_deref()) {
        Some(path) => json!({
            "kind": if path.is_dir() { "directory" } else { "file" },
            "target": path.to_string_lossy(),
            "risky": is_risky(&path),
        }),
        None => json!({ "kind": "missing", "target": strip_extended_prefix(&local), "risky": false }),
    }
}

fn spawn_detached(program: &str, args: &[&str]) -> Result<(), String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Launch {program} failed: {error}"))
}

/// Hand a file / folder / URL to the OS default handler.
///
/// Returns `{ opened: false, reason: "risky" }` for things Windows would
/// execute — the caller must confirm with the user and retry with `force`.
#[tauri::command(async)]
pub fn open_external(
    target: String,
    cwd: Option<String>,
    force: Option<bool>,
) -> Result<Value, String> {
    if is_url(&target) {
        let url = target.trim().to_string();
        open_with_os(&url)?;
        crate::debug_log::append("open", "info", "", "open url", Some(&url));
        return Ok(json!({ "opened": true, "kind": "url", "target": url }));
    }

    let local = strip_scheme(target);
    let path = resolve_path(&local, cwd.as_deref())
        .ok_or_else(|| format!("Not found on disk: {}", strip_extended_prefix(&local)))?;

    if is_risky(&path) && !force.unwrap_or(false) {
        return Ok(json!({
            "opened": false,
            "reason": "risky",
            "target": path.to_string_lossy(),
            "message": "This file type is executable — Marionette will not launch it from agent output without confirmation.",
        }));
    }

    let display = path.to_string_lossy().to_string();
    open_with_os(&to_explorer_arg(&path))?;
    crate::debug_log::append("open", "info", "", "open path", Some(&display));
    Ok(json!({
        "opened": true,
        "kind": if path.is_dir() { "directory" } else { "file" },
        "target": display,
    }))
}

/// Show the item in the OS file manager (selected, not opened).
#[tauri::command(async)]
pub fn reveal_in_file_manager(target: String, cwd: Option<String>) -> Result<Value, String> {
    let local = strip_scheme(target);
    let path = resolve_path(&local, cwd.as_deref())
        .ok_or_else(|| format!("Not found on disk: {}", strip_extended_prefix(&local)))?;
    let display = path.to_string_lossy().to_string();
    let arg = to_explorer_arg(&path);

    #[cfg(target_os = "windows")]
    {
        if path.is_dir() {
            spawn_detached("explorer.exe", &[&arg])?;
        } else {
            // `/select,<path>` must stay one argument — explorer parses it itself.
            spawn_detached("explorer.exe", &[&format!("/select,{arg}")])?;
        }
    }
    #[cfg(target_os = "macos")]
    {
        spawn_detached("open", &["-R", &display])?;
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        // No portable "reveal" on Linux — open the containing folder.
        let folder = if path.is_dir() {
            display.clone()
        } else {
            path.parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| display.clone())
        };
        spawn_detached("xdg-open", &[&folder])?;
    }

    crate::debug_log::append("open", "info", "", "reveal path", Some(&display));
    Ok(json!({ "revealed": true, "target": display }))
}

fn open_with_os(target: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        // explorer.exe hands the target to the registered handler. Unlike
        // `cmd /C start` it never re-parses the string, so `&` in a file name
        // cannot turn into a second command.
        spawn_detached("explorer.exe", &[target])
    }
    #[cfg(target_os = "macos")]
    {
        spawn_detached("open", &[target])
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        spawn_detached("xdg-open", &[target])
    }
}

#[cfg(test)]
mod tests {
    use super::{is_risky, is_url, resolve_path, strip_extended_prefix, strip_file_url, strip_scheme, to_explorer_arg};
    use std::fs;
    use std::path::Path;

    #[cfg(target_os = "windows")]
    #[test]
    fn explorer_args_are_backslash_normalized() {
        assert_eq!(
            to_explorer_arg(Path::new("D:/Apps/Marionette/docs/CURRENT.md")),
            r"D:\Apps\Marionette\docs\CURRENT.md"
        );
        assert_eq!(
            to_explorer_arg(Path::new("D:/Apps")),
            r"D:\Apps"
        );
        assert_eq!(
            to_explorer_arg(Path::new(r"D:\Apps\a\b")),
            r"D:\Apps\a\b"
        );
    }

    #[test]
    fn strips_windows_extended_prefix() {
        assert_eq!(strip_extended_prefix(r"\\?\D:\a\b"), r"D:\a\b");
        assert_eq!(strip_extended_prefix(r"D:\a\b"), r"D:\a\b");
    }

    #[test]
    fn strips_file_urls() {
        assert_eq!(
            strip_file_url("file:///D:/Apps/a.png"),
            Some("D:/Apps/a.png".to_string())
        );
        assert_eq!(strip_file_url("file://localhost/D:/a"), Some("D:/a".to_string()));
        assert_eq!(strip_file_url("file://LOCALHOST/D:/a"), Some("D:/a".to_string()));
        assert_eq!(strip_file_url("file://D:/a"), Some("D:/a".to_string()));
        assert_eq!(strip_file_url("file:///C:/x"), Some("C:/x".to_string()));
        assert_eq!(strip_file_url("  file:///C:/x  "), Some("C:/x".to_string()));
        assert_eq!(strip_file_url("file://"), None);
        assert_eq!(strip_file_url("https://example.com"), None);
        assert_eq!(strip_file_url(r"D:\a\b"), None);
    }

    #[test]
    fn strips_schemes_or_passes_through() {
        assert_eq!(strip_scheme("file:///D:/Apps/a.png".to_string()), "D:/Apps/a.png");
        assert_eq!(strip_scheme(r"D:\a\b".to_string()), r"D:\a\b");
        assert_eq!(strip_scheme("".to_string()), "");
        assert_eq!(strip_scheme("https://example.com".to_string()), "https://example.com");
    }

    #[test]
    fn resolves_file_urls_as_paths() {
        let root = std::env::temp_dir().join(format!("marionette-fileurl-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let file = root.join("App.tsx");
        fs::write(&file, "x").unwrap();
        let file_url = format!("file:///{}", file.to_string_lossy().replace('\\', "/"));
        // Mirrors the command flow: strip the scheme first, then resolve.
        let stripped = strip_file_url(&file_url).unwrap_or_else(|| file_url.clone());
        assert_eq!(resolve_path(&stripped, None), Some(file.clone()));
        // Percent-encoding is a known limitation, not a silent surprise.
        assert_eq!(resolve_path("D:/not%20here", None), None);
        assert_eq!(strip_file_url("file:///D:/not%20here"), Some("D:/not%20here".to_string()));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn flags_executables_only() {
        assert!(is_risky(Path::new("C:/tmp/setup.exe")));
        assert!(is_risky(Path::new("/tmp/run.SH")));
        assert!(!is_risky(Path::new("/tmp/App.tsx")));
        assert!(!is_risky(Path::new("/tmp/notes.md")));
    }

    #[test]
    fn detects_urls() {
        assert!(is_url("https://example.com"));
        assert!(is_url("HTTP://example.com"));
        assert!(!is_url("ftp://example.com"));
        assert!(!is_url(r"D:\a\b"));
    }

    #[test]
    fn resolves_relative_paths_and_line_suffixes() {
        let root = std::env::temp_dir().join(format!("marionette-open-{}", std::process::id()));
        let nested = root.join("src");
        fs::create_dir_all(&nested).unwrap();
        let file = nested.join("App.tsx");
        fs::write(&file, "x").unwrap();

        let cwd = root.to_string_lossy().to_string();
        assert_eq!(resolve_path("src/App.tsx", Some(&cwd)), Some(file.clone()));
        // Editors print `file:line[:col]` — the filesystem does not.
        assert_eq!(resolve_path("src/App.tsx:42", Some(&cwd)), Some(file.clone()));
        assert_eq!(resolve_path("src/App.tsx:42:7", Some(&cwd)), Some(file));
        assert_eq!(resolve_path("src/Missing.tsx", Some(&cwd)), None);

        fs::remove_dir_all(root).unwrap();
    }
}
