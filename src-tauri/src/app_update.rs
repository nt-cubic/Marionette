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

/// Prefer a Windows portable exe asset from the release.
fn pick_asset(assets: &[Value]) -> Option<(&str, &str)> {
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
        if !lower.ends_with(".exe") {
            continue;
        }
        // Score: prefer plain Marionette.exe, then anything with marionette.
        let score = if lower == "marionette.exe" {
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
    let resp = match ureq::get(&url)
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
            if t.len() > 400 {
                format!("{}…", &t[..400])
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

    let resp = ureq::get(&url)
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

/// Spawn a helper that replaces this exe after we exit, then exit.
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

    let target = current_exe_path()?;
    let target_str = target.to_string_lossy().to_string();
    let new_str = new_path.to_string_lossy().to_string();
    let pid = std::process::id();

    // Windows batch: wait for PID to die, copy, restart, self-delete.
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

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        Command::new("cmd")
            .args(["/C", &bat.to_string_lossy()])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("Spawn updater failed: {e}"))?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = bat;
        return Err("App update apply is only implemented on Windows".into());
    }

    crate::debug_log::append(
        "update",
        "info",
        "",
        "applying update and exiting",
        Some(&new_str),
    );

    // Leave quickly so the bat can replace the image.
    std::process::exit(0);
}
