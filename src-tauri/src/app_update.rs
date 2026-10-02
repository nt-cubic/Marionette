//! App self-update from GitHub Releases (portable single-exe).
//!
//! Flow: check latest release → user confirms → download asset beside a
//! staging dir → write a bat that waits for this process to exit, replaces
//! the exe, relaunches. Never overwrite a running image in place on Windows.

use crate::agent_update::is_newer;
use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const GITHUB_REPO: &str = "nt-cubic/Marionette";
const USER_AGENT: &str = "Marionette/0.1 (app-update)";
const TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUpdateInfo {
    pub current_version: String,
    pub latest_version: Option<String>,
    pub update_available: bool,
    pub release_url: Option<String>,
    pub asset_name: Option<String>,
    pub asset_url: Option<String>,
    pub notes: Option<String>,
    pub note: Option<String>,
}

fn current_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

fn strip_v(tag: &str) -> &str {
    tag.trim().trim_start_matches(['v', 'V'])
}

/// Prefer a platform-appropriate update asset from the release.
fn pick_asset(assets: &[Value]) -> Option<(&str, &str)> {
    #[cfg(target_os = "macos")]
    let suffixes: &[&str] = &[".dmg", ".app.tar.gz", ".zip", ".app"];
    #[cfg(target_os = "windows")]
    let suffixes: &[&str] = &[".exe"];
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let suffixes: &[&str] = &[".AppImage", ".tar.gz", ".zip"];

    let mut best: Option<(&str, &str, i32)> = None;
    for a in assets {
        let name = a.get("name").and_then(Value::as_str).unwrap_or("");
        let url = a
            .get("browser_download_url")
            .and_then(Value::as_str)
            .unwrap_or("");
        if name.is_empty() || url.is_empty() {
            continue;
        }
        let lower = name.to_ascii_lowercase();
        if !suffixes.iter().any(|s| lower.ends_with(s)) {
            continue;
        }
        // Score: prefer plain Marionette.<suffix>, then anything named marionette.
        let score = if lower.starts_with("marionette.") {
            100
        } else if lower.starts_with("marionette") {
            80
        } else if lower.contains("marionette") {
            60
        } else {
            10
        };
        if best.map(|(_, _, s)| score > s).unwrap_or(true) {
            best = Some((name, url, score));
        }
    }
    best.map(|(n, u, _)| (n, u))
}

pub fn check_for_update() -> AppUpdateInfo {
    let current = current_version();
    let url = format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest");
    let agent = match crate::http_client::agent() {
        Ok(a) => a,
        Err(e) => {
            return AppUpdateInfo {
                current_version: current,
                latest_version: None,
                update_available: false,
                release_url: None,
                asset_name: None,
                asset_url: None,
                notes: None,
                note: Some(format!("TLS unavailable: {e}")),
            };
        }
    };
    let resp = match agent
        .get(&url)
        .set("Accept", "application/vnd.github+json")
        .set("User-Agent", USER_AGENT)
        .timeout(TIMEOUT)
        .call()
    {
        Ok(r) => r,
        Err(_) => {
            return AppUpdateInfo {
                current_version: current,
                latest_version: None,
                update_available: false,
                release_url: None,
                asset_name: None,
                asset_url: None,
                notes: None,
                note: Some("Could not reach GitHub Releases".into()),
            };
        }
    };

    let value: Value = match resp.into_json() {
        Ok(v) => v,
        Err(_) => {
            return AppUpdateInfo {
                current_version: current,
                latest_version: None,
                update_available: false,
                release_url: None,
                asset_name: None,
                asset_url: None,
                notes: None,
                note: Some("Invalid GitHub Releases response".into()),
            };
        }
    };

    let tag = value
        .get("tag_name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let latest = strip_v(&tag).to_string();
    let release_url = value
        .get("html_url")
        .and_then(Value::as_str)
        .map(str::to_string);
    let notes = value
        .get("body")
        .and_then(Value::as_str)
        .map(|s| {
            let t = s.trim();
            // Cap by UTF-8 bytes without splitting a multi-byte character —
            // release notes often contain CJK/emoji; `&t[..400]` panics mid-codepoint.
            if t.len() > 400 {
                let mut end = 400;
                while end > 0 && !t.is_char_boundary(end) {
                    end -= 1;
                }
                format!("{}…", &t[..end])
            } else {
                t.to_string()
            }
        })
        .filter(|s| !s.is_empty());

    let assets = value
        .get("assets")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let (asset_name, asset_url) = pick_asset(&assets)
        .map(|(n, u)| (Some(n.to_string()), Some(u.to_string())))
        .unwrap_or((None, None));

    let update_available = !latest.is_empty() && is_newer(&latest, &current) && asset_url.is_some();
    let note = if update_available {
        None
    } else if asset_url.is_none() && !latest.is_empty() {
        Some("Release has no .exe asset".into())
    } else {
        None
    };

    AppUpdateInfo {
        current_version: current,
        latest_version: if latest.is_empty() {
            None
        } else {
            Some(latest)
        },
        update_available,
        release_url,
        asset_name,
        asset_url,
        notes,
        note,
    }
}

fn updates_dir() -> Result<PathBuf, String> {
    let dir = crate::app_paths::global_dir()?.join("updates");
    fs::create_dir_all(&dir).map_err(|e| format!("Create updates dir failed: {e}"))?;
    Ok(dir)
}

/// Download the latest release asset into `%USERPROFILE%\.marionette\updates\`.
pub fn download_latest() -> Result<(PathBuf, String), String> {
    let info = check_for_update();
    let version = info
        .latest_version
        .clone()
        .ok_or_else(|| "No newer version found".to_string())?;
    let url = info
        .asset_url
        .clone()
        .ok_or_else(|| "No downloadable .exe on the latest release".to_string())?;
    let name = info
        .asset_name
        .clone()
        .unwrap_or_else(|| "Marionette.exe".into());

    let dir = updates_dir()?;
    let dest = dir.join(format!("Marionette-{version}.exe"));
    let meta_path = dir.join("pending.json");

    let resp = crate::http_client::agent()?
        .get(&url)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/octet-stream")
        .timeout(Duration::from_secs(120))
        .call()
        .map_err(|e| format!("Download failed: {e}"))?;

    let mut bytes: Vec<u8> = Vec::new();
    resp.into_reader()
        .take(200 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Read download failed: {e}"))?;
    if bytes.len() < 1024 {
        return Err("Downloaded file looks too small to be an exe".into());
    }
    fs::write(&dest, &bytes).map_err(|e| format!("Write download failed: {e}"))?;

    let meta = serde_json::json!({
        "version": version,
        "path": dest.to_string_lossy(),
        "assetName": name,
        "downloadedAt": chrono_like_now(),
    });
    // chrono-less stamp
    let _ = fs::write(
        &meta_path,
        serde_json::to_string_pretty(&meta).unwrap_or_else(|_| "{}".into()),
    );

    crate::debug_log::append(
        "update",
        "info",
        "",
        &format!("downloaded app update {version}"),
        Some(&dest.display().to_string()),
    );

    Ok((dest, version))
}

fn chrono_like_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("{ms}")
}

fn current_exe_path() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| format!("current_exe failed: {e}"))
}

/// Spawn a helper that replaces this app after we exit, then exit.
pub fn apply_and_relaunch() -> Result<(), String> {
    let dir = updates_dir()?;
    let meta_path = dir.join("pending.json");
    let meta_raw =
        fs::read_to_string(&meta_path).map_err(|_| "No pending update — download first".to_string())?;
    let meta: Value =
        serde_json::from_str(&meta_raw).map_err(|e| format!("Bad pending.json: {e}"))?;
    let new_exe = meta
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "pending.json missing path".to_string())?;
    let new_path = Path::new(new_exe);
    if !new_path.is_file() {
        return Err(format!("Pending update file missing: {new_exe}"));
    }

    // Windows batch: wait for PID to die, copy, restart, self-delete.
    #[cfg(target_os = "windows")]
    {
        let target_str = current_exe_path()?.to_string_lossy().to_string();
        let new_str = new_path.to_string_lossy().to_string();
        let pid = std::process::id();

        let bat = dir.join("apply-update.bat");
        let bat_body = format!(
            r#"@echo off
setlocal
set "TARGET={target}"
set "NEW={new}"
set "PID={pid}"
:wait
tasklist /FI "PID eq %PID%" 2>NUL | find "%PID%" >NUL
if not errorlevel 1 (
  timeout /t 1 /nobreak >NUL
  goto wait
)
copy /Y "%NEW%" "%TARGET%" >NUL
if errorlevel 1 (
  echo Update copy failed
  exit /b 1
)
start "" "%TARGET%"
del "%~f0"
"#,
            target = target_str.replace('"', ""),
            new = new_str.replace('"', ""),
            pid = pid,
        );
        fs::write(&bat, bat_body).map_err(|e| format!("Write updater bat failed: {e}"))?;

        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        Command::new("cmd")
            .args(["/C", &bat.to_string_lossy()])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("Spawn updater failed: {e}"))?;
    }

    // macOS: extract the new bundle, then a shell helper waits for this
    // process to die, swaps the .app in place and relaunches via `open`.
    #[cfg(target_os = "macos")]
    {
        // Running image: <App>.app/Contents/MacOS/marionette → three parents up.
        let app_dir = current_exe_path()?
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .map(Path::to_path_buf)
            .filter(|p| p.extension().map(|e| e == "app").unwrap_or(false))
            .ok_or_else(|| {
                "App update apply requires the bundled Marionette.app (dev builds cannot self-update)"
                    .to_string()
            })?;

        let staging = dir.join("staging");
        let _ = fs::remove_dir_all(&staging);
        fs::create_dir_all(&staging).map_err(|e| format!("Create staging dir failed: {e}"))?;
        let new_app = extract_app_bundle(new_path, &staging)?.to_string_lossy().to_string();
        let pid = std::process::id();

        let helper = dir.join("apply-update.sh");
        let helper_body = r#"#!/bin/sh
TARGET_APP="$1"
NEW_APP="$2"
PID="$3"
while kill -0 "$PID" 2>/dev/null; do sleep 1; done
rm -rf "$TARGET_APP"
ditto "$NEW_APP" "$TARGET_APP"
rm -rf "$(dirname "$NEW_APP")"
open "$TARGET_APP"
rm -f "$0"
"#;
        fs::write(&helper, helper_body).map_err(|e| format!("Write updater script failed: {e}"))?;
        Command::new("sh")
            .arg(&helper)
            .arg(&app_dir)
            .arg(&new_app)
            .arg(pid.to_string())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("Spawn updater failed: {e}"))?;
    }

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        return Err("App update apply is only implemented on Windows and macOS".into());
    }

    let new_str = new_path.to_string_lossy().to_string();
    crate::debug_log::append(
        "update",
        "info",
        "",
        "applying update and exiting",
        Some(&new_str),
    );

    // Leave quickly so the helper can swap the image.
    std::process::exit(0);
}

/// Extract an update artifact into `staging` and locate the `.app` bundle.
///
/// Supports Tauri's macOS outputs: `.dmg` (mounted read-only via hdiutil),
/// `.tar.gz` / `.tgz`, `.zip`, or an already-extracted `.app` directory.
#[cfg(target_os = "macos")]
fn extract_app_bundle(update: &Path, staging: &Path) -> Result<PathBuf, String> {
    let lower = update.to_string_lossy().to_ascii_lowercase();
    if update.is_dir() {
        return find_app_bundle(update)
            .ok_or_else(|| "No .app bundle found in update directory".to_string());
    }
    if lower.ends_with(".dmg") {
        let mnt = staging.join("mnt");
        fs::create_dir_all(&mnt).map_err(|e| format!("Create mount dir failed: {e}"))?;
        run_quiet(
            "hdiutil",
            &[
                "attach",
                "-nobrowse",
                "-readonly",
                "-mountpoint",
                &mnt.to_string_lossy(),
                &update.to_string_lossy(),
            ],
        )?;
        let found = find_app_bundle(&mnt);
        let _ = run_quiet("hdiutil", &["detach", &mnt.to_string_lossy()]);
        return found.ok_or_else(|| "No .app bundle found in update .dmg".to_string());
    }
    if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        run_quiet(
            "tar",
            &[
                "-xzf",
                &update.to_string_lossy(),
                "-C",
                &staging.to_string_lossy(),
            ],
        )?;
        return find_app_bundle(staging)
            .ok_or_else(|| "No .app bundle found in update archive".to_string());
    }
    if lower.ends_with(".zip") {
        run_quiet(
            "unzip",
            &["-q", &update.to_string_lossy(), "-d", &staging.to_string_lossy()],
        )?;
        return find_app_bundle(staging)
            .ok_or_else(|| "No .app bundle found in update archive".to_string());
    }
    Err(format!(
        "Unsupported macOS update format: {lower} (expected .dmg, .tar.gz, or .zip)"
    ))
}

/// First `.app` bundle under `dir`, top level then one level deeper.
#[cfg(target_os = "macos")]
fn find_app_bundle(dir: &Path) -> Option<PathBuf> {
    let is_app = |p: &Path| p.is_dir() && p.extension().map(|e| e == "app").unwrap_or(false);
    let entries: Vec<PathBuf> = fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).collect();
    for p in &entries {
        if is_app(p) {
            return Some(p.clone());
        }
    }
    for p in &entries {
        if p.is_dir() {
            if let Ok(subs) = fs::read_dir(p) {
                for sub in subs.flatten() {
                    let sp = sub.path();
                    if is_app(&sp) {
                        return Some(sp);
                    }
                }
            }
        }
    }
    None
}

/// Run a helper binary silently; error on non-zero exit.
#[cfg(target_os = "macos")]
fn run_quiet(program: &str, args: &[&str]) -> Result<(), String> {
    let status = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("{program} failed to start: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} exited with {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset_list() -> Vec<Value> {
        serde_json::json!([
            {"name": "Marionette.exe", "browser_download_url": "https://example.invalid/Marionette.exe"},
            {"name": "Marionette_0.2.0_aarch64.dmg", "browser_download_url": "https://example.invalid/Marionette_0.2.0_aarch64.dmg"},
            {"name": "Marionette_0.2.0_aarch64.app.tar.gz", "browser_download_url": "https://example.invalid/a.app.tar.gz"},
            {"name": "NOTES.txt", "browser_download_url": "https://example.invalid/NOTES.txt"},
        ])
        .as_array()
        .unwrap()
        .clone()
    }

    #[test]
    fn picks_platform_appropriate_asset() {
        let assets = asset_list();
        #[cfg(target_os = "macos")]
        assert_eq!(
            pick_asset(&assets).map(|(n, _)| n),
            Some("Marionette_0.2.0_aarch64.dmg"),
            "macOS should prefer the .dmg over the .exe"
        );
        #[cfg(target_os = "windows")]
        assert_eq!(
            pick_asset(&assets).map(|(n, _)| n),
            Some("Marionette.exe"),
            "Windows should keep preferring the portable .exe"
        );
        #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
        assert_eq!(
            pick_asset(&assets).map(|(n, _)| n),
            Some("Marionette_0.2.0_aarch64.app.tar.gz")
        );
    }

    #[test]
    fn asset_matching_is_case_insensitive() {
        let assets: Vec<Value> = serde_json::json!([
            {"name": "marionette_0.2.0_universal.DMG", "browser_download_url": "https://example.invalid/u.dmg"},
        ])
        .as_array()
        .unwrap()
        .clone();
        #[cfg(target_os = "macos")]
        assert!(pick_asset(&assets).is_some(), ".DMG must match case-insensitively");
        #[cfg(not(target_os = "macos"))]
        assert!(pick_asset(&assets).is_none(), "a .dmg must be ignored off-macOS");
    }
}
