//! Resolve agent CLIs on Windows (npm shims, missing PATH, .cmd wrappers).

use std::env;
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
            // Prefer unwrapping npm .cmd → real .exe when possible
            if let Some(exe) = unwrap_npm_cmd_to_exe(&path) {
                return Ok(ResolvedCommand {
                    program: exe.clone(),
                    prefix_args: Vec::new(),
                    resolved_path: exe,
                });
            }
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
        if token.to_ascii_lowercase().ends_with(".exe") {
            let path = if Path::new(token).is_absolute() {
                PathBuf::from(token)
            } else {
                cmd_path.parent()?.join(token.replace('/', "\\"))
            };
            if path.is_file() {
                return Some(path.to_string_lossy().to_string());
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
}
