//! Resolve agent CLIs on Windows (npm shims, missing PATH, .cmd wrappers).
//!
//! GUI apps often inherit a thinner PATH than an interactive shell. Prefer
//! absolute paths and `node + script.js` over `cmd /C *.cmd` so install/update
//! works the same on US, JP, CN, etc. Windows machines.

use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A spawnable program + optional leading args (e.g. `cmd.exe /C script.cmd`).
#[derive(Clone, Debug)]
pub struct ResolvedCommand {
    pub program: String,
    pub prefix_args: Vec<String>,
    /// Absolute path we resolved (for diagnostics).
    pub resolved_path: String,
}

impl ResolvedCommand {
    pub fn apply_to(&self, command: &mut Command) {
        command.args(&self.prefix_args);
    }
}

/// Resolve `command` to something `std::process::Command` can spawn on this OS.
pub fn resolve_spawn_command(command: &str) -> Result<ResolvedCommand, String> {
    let command = command.trim();
    if command.is_empty() {
        return Err("Agent command is empty".to_string());
    }

    // Absolute / relative path that already exists
    let as_path = PathBuf::from(command);
    if as_path.is_file() {
        return finalize_path(as_path);
    }

    // Search PATH + npm-ish locations
    let mut candidates: Vec<PathBuf> = Vec::new();

    #[cfg(target_os = "windows")]
    {
        if let Ok(output) = Command::new("where.exe").arg(command).output() {
            if output.status.success() {
                for line in String::from_utf8_lossy(&output.stdout).lines() {
                    let line = line.trim();
                    if !line.is_empty() {
                        candidates.push(PathBuf::from(line));
                    }
                }
            }
        }
        candidates.extend(windows_extra_candidates(command));
    }

    #[cfg(not(target_os = "windows"))]
    {
        if let Ok(output) = Command::new("which").arg(command).output() {
            if output.status.success() {
                if let Some(line) = String::from_utf8_lossy(&output.stdout).lines().next() {
                    let line = line.trim();
                    if !line.is_empty() {
                        candidates.push(PathBuf::from(line));
                    }
                }
            }
        }
    }

    // Dedup while preserving order
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|p| seen.insert(p.clone()));

    // Prefer real executables over extensionless npm shims
    candidates.sort_by_key(|p| rank_candidate(p));

    for candidate in candidates {
        if candidate.is_file() {
            if let Ok(resolved) = finalize_path(candidate) {
                return Ok(resolved);
            }
        }
    }

    Err(format!(
        "Command `{command}` was not found. Install it and ensure it is on PATH \
         (on Windows, npm global bin is usually %APPDATA%\\npm)."
    ))
}

/// PATH lookup used by the settings "test agent command" UI.
pub fn resolve_command_display_path(command: &str) -> Result<Option<String>, String> {
    match resolve_spawn_command(command) {
        Ok(resolved) => Ok(Some(resolved.resolved_path)),
        Err(_) => Ok(None),
    }
}

/// Resolve `npm` as `node path\to\npm-cli.js` (never `cmd /C npm.cmd`).
///
/// The GUI-update path must not depend on shell quoting or locale-specific
/// `cmd.exe` errors. Running the CLI entry through Node is stable worldwide.
pub fn resolve_npm_for_install() -> Result<ResolvedCommand, String> {
    if let Some(resolved) = resolve_npm_via_node_cli() {
        return Ok(resolved);
    }
    // Fall back to the general resolver (may still land on npm.cmd).
    resolve_spawn_command("npm").map_err(|error| {
        format!(
            "`npm` not found — install Node.js from https://nodejs.org and restart Marionette ({error})"
        )
    })
}

/// Prepend Node's directory (+ npm global bin) so child tools npm spawns can find `node`.
pub fn env_path_with_node_bins(node_program: &str) -> OsString {
    let mut prepend: Vec<PathBuf> = Vec::new();
    if let Some(dir) = Path::new(node_program).parent() {
        prepend.push(dir.to_path_buf());
    }
    if let Some(appdata) = env::var_os("APPDATA") {
        prepend.push(PathBuf::from(appdata).join("npm"));
    }
    if let Some(local) = env::var_os("LOCALAPPDATA") {
        prepend.push(PathBuf::from(local).join("npm"));
    }

    let current = env::var_os("PATH").unwrap_or_default();
    let mut parts: Vec<PathBuf> = prepend;
    parts.extend(env::split_paths(&current));

    // Dedup while preserving order
    let mut seen = std::collections::HashSet::new();
    parts.retain(|p| seen.insert(p.clone()));

    env::join_paths(parts).unwrap_or(current)
}

/// Decode subprocess stdout/stderr that may be UTF-8 or the Windows ANSI code page.
///
/// Japanese/Chinese Windows often emit Shift-JIS (CP932) from `cmd`/`npm` — treating
/// that as UTF-8 produces the classic mojibake users report as "乱码".
pub fn decode_process_bytes(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_string();
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(text) = decode_windows_ansi(bytes) {
            return text;
        }
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// Turn common localized Windows "command not found" lines into a clear English tip.
pub fn humanize_install_error(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    // EN: "is not recognized as an internal or external command"
    // JP: "は、内部コマンドまたは外部コマンド、…として認識されていません"
    // CN: "不是内部或外部命令"
    let not_recognized = lower.contains("is not recognized as an internal or external command")
        || raw.contains("認識されていません")
        || raw.contains("认识")
        || raw.contains("不是内部或外部命令")
        || raw.contains("не является внутренней или внешней")
        || raw.contains("не распознано");
    if not_recognized {
        return format!(
            "{raw}\n\n\
             Tip: Marionette could not run a Node/npm helper (often a PATH issue when the app is \
             started from the desktop). Install Node.js LTS, ensure `node` and `npm` work in a new \
             terminal, then fully quit and reopen Marionette."
        );
    }
    if lower.contains("eperm") || lower.contains("operation not permitted") {
        return format!(
            "{raw}\n\n\
             Tip: a running agent binary is locked. Stop that agent in Marionette (or end \
             `claude.exe` / the agent process in Task Manager), then update again."
        );
    }
    raw.to_string()
}

fn resolve_npm_via_node_cli() -> Option<ResolvedCommand> {
    let node = resolve_node_program()?;
    let node_dir = Path::new(&node).parent()?;
    let cli = node_dir
        .join("node_modules")
        .join("npm")
        .join("bin")
        .join("npm-cli.js");
    if !cli.is_file() {
        return None;
    }
    let cli_str = strip_extended_prefix(
        &cli.canonicalize().unwrap_or(cli).to_string_lossy(),
    );
    Some(ResolvedCommand {
        program: node,
        prefix_args: vec![cli_str.clone()],
        resolved_path: cli_str,
    })
}

fn resolve_node_program() -> Option<String> {
    if let Ok(resolved) = resolve_spawn_command("node") {
        // Prefer a real node.exe, not a weird wrapper with prefix args.
        if resolved.prefix_args.is_empty() {
            return Some(resolved.program);
        }
    }
    for candidate in node_install_candidates() {
        if candidate.is_file() {
            return Some(strip_extended_prefix(
                &candidate.canonicalize().unwrap_or(candidate).to_string_lossy(),
            ));
        }
    }
    None
}

fn node_install_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut push_node = |dir: PathBuf| {
        out.push(dir.join("node.exe"));
        out.push(dir.join("node"));
    };

    for key in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
        if let Ok(root) = env::var(key) {
            push_node(PathBuf::from(&root).join("nodejs"));
            push_node(PathBuf::from(&root).join("Programs").join("nodejs"));
            push_node(PathBuf::from(&root).join("Programs").join("node"));
        }
    }
    if let Ok(home) = env::var("USERPROFILE") {
        let home = PathBuf::from(home);
        push_node(home.join("scoop").join("apps").join("nodejs").join("current"));
        push_node(home.join("scoop").join("apps").join("nodejs-lts").join("current"));
        push_node(home.join("AppData").join("Roaming").join("fnm").join("aliases").join("default"));
    }
    if let Ok(volta) = env::var("VOLTA_HOME") {
        push_node(PathBuf::from(volta).join("bin"));
    }
    if let Ok(nvm) = env::var("NVM_SYMLINK") {
        push_node(PathBuf::from(nvm));
    }
    if let Ok(nvm_home) = env::var("NVM_HOME") {
        // nvm-windows: NVM_HOME holds versions; active one is NVM_SYMLINK — still scan latest.
        if let Ok(entries) = std::fs::read_dir(nvm_home) {
            let mut versions: Vec<PathBuf> = entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.is_dir())
                .collect();
            versions.sort();
            if let Some(last) = versions.pop() {
                push_node(last);
            }
        }
    }
    if let Ok(fnm) = env::var("FNM_MULTISHELL_PATH") {
        push_node(PathBuf::from(fnm));
    }
    out
}

fn strip_extended_prefix(path: &str) -> String {
    path.trim_start_matches(r"\\?\").to_string()
}

#[cfg(target_os = "windows")]
fn decode_windows_ansi(bytes: &[u8]) -> Option<String> {
    use windows_sys::Win32::Globalization::{
        MultiByteToWideChar, CP_ACP, CP_OEMCP, MB_ERR_INVALID_CHARS,
    };

    for cp in [CP_ACP, CP_OEMCP] {
        // First pass: measure
        let needed = unsafe {
            MultiByteToWideChar(
                cp,
                MB_ERR_INVALID_CHARS,
                bytes.as_ptr(),
                bytes.len() as i32,
                std::ptr::null_mut(),
                0,
            )
        };
        if needed <= 0 {
            continue;
        }
        let mut wide = vec![0u16; needed as usize];
        let written = unsafe {
            MultiByteToWideChar(
                cp,
                MB_ERR_INVALID_CHARS,
                bytes.as_ptr(),
                bytes.len() as i32,
                wide.as_mut_ptr(),
                needed,
            )
        };
        if written <= 0 {
            continue;
        }
        wide.truncate(written as usize);
        return Some(String::from_utf16_lossy(&wide));
    }
    None
}

/// Prefer a globally-installed ACP binary over slow `npx -y …` cold starts.
///
/// Example: `npx -y @agentclientprotocol/codex-acp` → `codex-acp` when on PATH.
pub fn prefer_fast_acp_launch(command: &str, args: &[String]) -> (String, Vec<String>) {
    let cmd = command.trim();
    let is_npx = cmd.eq_ignore_ascii_case("npx")
        || cmd.eq_ignore_ascii_case("npx.cmd")
        || cmd.to_ascii_lowercase().ends_with("\\npx.cmd")
        || cmd.to_ascii_lowercase().ends_with("/npx");

    if !is_npx {
        return (command.to_string(), args.to_vec());
    }

    let package = args
        .iter()
        .find(|a| a.contains("agentclientprotocol") || a.ends_with("-acp"))
        .map(|s| s.as_str());

    let candidates: &[&str] = match package {
        Some(p) if p.contains("codex") => &["codex-acp"],
        Some(p) if p.contains("claude") => &["claude-agent-acp", "claude-code-acp"],
        _ => &[],
    };

    for bin in candidates {
        if resolve_spawn_command(bin).is_ok() {
            return ((*bin).to_string(), Vec::new());
        }
    }

    // Keep npx, but drop redundant noise; ensure -y for non-interactive
    let mut next_args = args.to_vec();
    if !next_args.iter().any(|a| a == "-y" || a == "--yes") {
        next_args.insert(0, "-y".to_string());
    }
    (command.to_string(), next_args)
}

fn rank_candidate(path: &Path) -> i32 {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if name.ends_with(".exe") {
        return 0;
    }
    if name.ends_with(".cmd") || name.ends_with(".bat") {
        return 10;
    }
    if name.ends_with(".ps1") {
        return 50;
    }
    // Extensionless npm shim — often a Unix shell script; worst choice on Windows
    100
}

fn finalize_path(path: PathBuf) -> Result<ResolvedCommand, String> {
    let path = path
        .canonicalize()
        .unwrap_or(path);
    let path_str = path.to_string_lossy().to_string();
    // Strip Windows \\?\ prefix for friendlier command lines
    let path_str = path_str
        .strip_prefix(r"\\?\")
        .unwrap_or(&path_str)
        .to_string();

    #[cfg(target_os = "windows")]
    {
        let lower = path_str.to_ascii_lowercase();
        if lower.ends_with(".cmd") || lower.ends_with(".bat") {
            // Prefer node + script.js: cmd /C breaks piped stdin for ACP adapters on Windows.
            if let Some(resolved) = unwrap_npm_cmd_to_node(&path) {
                return Ok(resolved);
            }
            // Prefer unwrapping npm .cmd → real .exe when possible
            if let Some(exe) = unwrap_npm_cmd_to_exe(&path) {
                return Ok(ResolvedCommand {
                    program: exe.clone(),
                    prefix_args: Vec::new(),
                    resolved_path: exe,
                });
            }
            // Last resort: cmd /C (may break interactive stdio)
            return Ok(ResolvedCommand {
                program: "cmd.exe".to_string(),
                prefix_args: vec!["/D".into(), "/S".into(), "/C".into(), path_str.clone()],
                resolved_path: path_str,
            });
        }
        if lower.ends_with(".ps1") {
            return Ok(ResolvedCommand {
                program: "powershell.exe".to_string(),
                prefix_args: vec![
                    "-NoProfile".into(),
                    "-ExecutionPolicy".into(),
                    "Bypass".into(),
                    "-File".into(),
                    path_str.clone(),
                ],
                resolved_path: path_str,
            });
        }
        // Extensionless "opencode" shim next to opencode.cmd — try sibling .cmd / real exe
        if path.extension().is_none() {
            let cmd_sibling = path.with_extension("cmd");
            if cmd_sibling.is_file() {
                return finalize_path(cmd_sibling);
            }
            if let Some(exe) = unwrap_npm_shim_dir_to_exe(&path) {
                return Ok(ResolvedCommand {
                    program: exe.clone(),
                    prefix_args: Vec::new(),
                    resolved_path: exe,
                });
            }
        }
    }

    Ok(ResolvedCommand {
        program: path_str.clone(),
        prefix_args: Vec::new(),
        resolved_path: path_str,
    })
}

#[cfg(target_os = "windows")]
fn windows_extra_candidates(command: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let file_names = [
        format!("{command}.exe"),
        format!("{command}.cmd"),
        format!("{command}.bat"),
        command.to_string(),
    ];

    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(path) = env::var("PATH") {
        for part in env::split_paths(&path) {
            dirs.push(part);
        }
    }
    if let Some(appdata) = env::var_os("APPDATA") {
        dirs.push(PathBuf::from(&appdata).join("npm"));
        // Common npm package binary layouts
        dirs.push(
            PathBuf::from(&appdata)
                .join("npm")
                .join("node_modules")
                .join("opencode-ai")
                .join("bin"),
        );
        dirs.push(
            PathBuf::from(&appdata)
                .join("npm")
                .join("node_modules")
                .join("@anthropic-ai")
                .join("claude-code")
                .join("bin"),
        );
    }
    if let Some(local) = env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(&local).join("npm"));
    }
    // User profile npm
    if let Some(home) = env::var_os("USERPROFILE") {
        dirs.push(PathBuf::from(&home).join("AppData").join("Roaming").join("npm"));
    }
    // Node install dirs (GUI apps often miss these when User PATH is stale).
    for candidate in node_install_candidates() {
        if let Some(dir) = candidate.parent() {
            dirs.push(dir.to_path_buf());
        }
    }

    for dir in dirs {
        for name in &file_names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                out.push(candidate);
            }
        }
        // package bin folder may use command.exe directly
        let direct_exe = dir.join(format!("{command}.exe"));
        if direct_exe.is_file() {
            out.push(direct_exe);
        }
    }
    out
}

/// Parse npm `.cmd` shim → `node path\to\dist\index.js` so stdin/stdout pipes work for ACP.
#[cfg(target_os = "windows")]
fn unwrap_npm_cmd_to_node(cmd_path: &Path) -> Option<ResolvedCommand> {
    let content = std::fs::read_to_string(cmd_path).ok()?;

    // Typical npm shim after `%`/quote split:
    //   "%_prog%"  "%dp0%\node_modules\@scope\pkg\dist\index.js" %*
    // becomes tokens like `dp0` + `\node_modules\...\index.js`, or `~dp0\node_modules\...`.
    let mut js_hits: Vec<PathBuf> = Vec::new();
    for raw in content.split(|c: char| c == '"' || c == '%' || c.is_whitespace()) {
        let token = raw.trim().replace('/', "\\");
        if token.is_empty() {
            continue;
        }
        let lower = token.to_ascii_lowercase();
        if !(lower.contains("node_modules") && lower.ends_with(".js")) {
            continue;
        }
        // npm.cmd also references npm-prefix.js — that only prints a path.
        if lower.ends_with("npm-prefix.js") || lower.ends_with("-prefix.js") {
            continue;
        }
        if let Some(js_path) = resolve_batch_relative_path(cmd_path, &token) {
            js_hits.push(js_path);
        }
    }

    // Prefer the real CLI entry when several .js files appear in the shim.
    js_hits.sort_by_key(|p| {
        let name = p
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if name == "npm-cli.js" || name == "cli.js" || name == "index.js" {
            0
        } else {
            10
        }
    });

    let js_path = js_hits.into_iter().next()?;
    let js_str = strip_extended_prefix(
        &js_path.canonicalize().unwrap_or(js_path).to_string_lossy(),
    );

    let node = resolve_node_program()?;
    Some(ResolvedCommand {
        program: node,
        prefix_args: vec![js_str.clone()],
        resolved_path: js_str,
    })
}

/// Map a token from a `.cmd` shim to a real file, expanding `%~dp0` / `%dp0%`.
///
/// After we split on `%`, those become `~dp0\...`, `dp0` + `\...`, etc.
#[cfg(target_os = "windows")]
fn resolve_batch_relative_path(cmd_path: &Path, token: &str) -> Option<PathBuf> {
    let dir = cmd_path.parent()?;
    let token = token.trim().replace('/', "\\");
    if token.is_empty() {
        return None;
    }

    // Absolute path that already exists (skip bare `\foo` drive-root false friends).
    let as_path = PathBuf::from(&token);
    if as_path.is_file() {
        return Some(as_path);
    }
    // Windows treats `\node_modules\...` as absolute (current-drive root) — those
    // almost never exist; treat as relative to the shim directory instead.
    let looks_like_drive_root = token.starts_with('\\') && !token.starts_with("\\\\");

    let mut stripped = token.as_str();
    for prefix in ["~dp0", "dp0", ".\\", "./"] {
        if let Some(rest) = stripped
            .strip_prefix(prefix)
            .or_else(|| {
                let lower = stripped.to_ascii_lowercase();
                if lower.starts_with(&prefix.to_ascii_lowercase()) {
                    Some(&stripped[prefix.len()..])
                } else {
                    None
                }
            })
        {
            stripped = rest;
            break;
        }
    }
    stripped = stripped.trim_start_matches('\\');

    let candidates = [
        dir.join(stripped),
        dir.join(token.trim_start_matches('\\')),
        PathBuf::from(token),
    ];
    for candidate in candidates {
        if candidate.is_file() {
            return Some(candidate);
        }
        // Prefer shim-dir join over a non-existent drive-root absolute.
        if looks_like_drive_root {
            continue;
        }
    }
    None
}

/// Parse npm's .cmd shim and/or look beside it for the real .exe.
#[cfg(target_os = "windows")]
fn unwrap_npm_cmd_to_exe(cmd_path: &Path) -> Option<String> {
    if let Some(exe) = unwrap_npm_shim_dir_to_exe(cmd_path) {
        return Some(exe);
    }
    // Read the .cmd contents for `node_modules\...\bin\xxx.exe` or `node.exe ...`
    let content = std::fs::read_to_string(cmd_path).ok()?;
    // Typical: "%dp0%\node_modules\opencode-ai\bin\opencode.exe"
    for token in content.split(|c: char| c.is_whitespace() || c == '"' || c == '%') {
        let token = token.trim();
        if !token.to_ascii_lowercase().ends_with(".exe") {
            continue;
        }
        if let Some(path) = resolve_batch_relative_path(cmd_path, token) {
            return Some(strip_extended_prefix(&path.to_string_lossy()));
        }
        // Bare `node.exe` next to the shim / on PATH
        if token.eq_ignore_ascii_case("node.exe") {
            if let Some(node) = resolve_node_program() {
                return Some(node);
            }
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn unwrap_npm_shim_dir_to_exe(shim_path: &Path) -> Option<String> {
    let dir = shim_path.parent()?;
    let stem = shim_path.file_stem()?.to_str()?;

    // %APPDATA%\npm\node_modules\<pkg>\bin\<stem>.exe
    // Try a few known package folder names for popular agents
    let package_guesses = [
        format!("{stem}-ai"),     // opencode-ai
        stem.to_string(),         // opencode
        format!("{stem}-code"),   // claude-code style
        format!("@agentclientprotocol/{stem}-acp"),
    ];

    let node_modules = dir.join("node_modules");
    for pkg in package_guesses {
        // scoped packages: @scope/name
        let pkg_path = if pkg.starts_with('@') {
            let mut parts = pkg.splitn(2, '/');
            let scope = parts.next()?;
            let name = parts.next()?;
            node_modules.join(scope).join(name)
        } else {
            node_modules.join(&pkg)
        };
        let exe = pkg_path.join("bin").join(format!("{stem}.exe"));
        if exe.is_file() {
            return Some(exe.to_string_lossy().to_string());
        }
        // some packages put binary at package root
        let root_exe = pkg_path.join(format!("{stem}.exe"));
        if root_exe.is_file() {
            return Some(root_exe.to_string_lossy().to_string());
        }
    }

    // Direct: npm\node_modules\opencode-ai\bin\opencode.exe when shim is npm\opencode
    let opencode = node_modules
        .join("opencode-ai")
        .join("bin")
        .join("opencode.exe");
    if stem.eq_ignore_ascii_case("opencode") && opencode.is_file() {
        return Some(opencode.to_string_lossy().to_string());
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_exe_above_extensionless_shim() {
        assert!(rank_candidate(Path::new("opencode.exe")) < rank_candidate(Path::new("opencode")));
        assert!(rank_candidate(Path::new("opencode.cmd")) < rank_candidate(Path::new("opencode")));
    }

    #[test]
    fn resolves_something_on_path() {
        let command = if cfg!(target_os = "windows") {
            "where.exe"
        } else {
            "sh"
        };
        let resolved = resolve_spawn_command(command).expect("should resolve");
        assert!(!resolved.program.is_empty());
    }

    #[test]
    fn humanize_flags_command_not_found_and_eperm() {
        let en = humanize_install_error(
            "'node' is not recognized as an internal or external command,\r\noperable program or batch file.",
        );
        assert!(en.contains("Tip:"), "expected actionable tip, got {en}");

        let eperm = humanize_install_error("Error: EPERM: operation not permitted, unlink 'claude.exe'");
        assert!(eperm.to_ascii_lowercase().contains("locked") || eperm.contains("Tip:"));
    }

    #[test]
    fn decode_process_bytes_keeps_utf8() {
        assert_eq!(decode_process_bytes(b"npm ERR! 404"), "npm ERR! 404");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn batch_relative_path_expands_dp0_tokens() {
        let dir = std::env::temp_dir().join(format!(
            "marionette-batch-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("node_modules").join("pkg")).unwrap();
        let js = dir.join("node_modules").join("pkg").join("index.js");
        std::fs::write(&js, "console.log(1)").unwrap();
        let cmd = dir.join("tool.cmd");
        std::fs::write(&cmd, "@echo off\n").unwrap();

        // After `%` split: `~dp0\node_modules\pkg\index.js`
        let hit = resolve_batch_relative_path(
            &cmd,
            r"~dp0\node_modules\pkg\index.js",
        )
        .expect("expand ~dp0");
        assert!(hit.ends_with(Path::new("index.js")));

        // After `%` split of `%dp0%\node_modules\...` → `\node_modules\...`
        let hit2 = resolve_batch_relative_path(&cmd, r"\node_modules\pkg\index.js")
            .expect("expand drive-root-looking relative");
        assert!(hit2.is_file());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_npm_for_install_finds_something_when_node_present() {
        // CI / dev machines with Node should resolve; pure containers may skip.
        if resolve_spawn_command("node").is_err() && resolve_node_program().is_none() {
            return;
        }
        let npm = resolve_npm_for_install().expect("npm should resolve via node or PATH");
        assert!(!npm.program.is_empty());
    }
}
