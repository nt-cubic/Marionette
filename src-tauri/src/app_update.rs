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

fn is_portable_exe(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".exe") && lower.contains("marionette") && !lower.contains("uncompressed")
}

/// Score a release asset. Canonical ship name is `Marionette.exe` (embedded
/// engine). Legacy dual-SKU names still match so old installs can update.
fn asset_score(name: &str) -> i32 {
    if !is_portable_exe(name) {
        return -1;
    }
    let lower = name.to_ascii_lowercase();
    if lower == "marionette.exe" {
        100
    } else if lower.contains("engine") && lower.contains("windows") {
        80
    } else if lower == "marionette-engine.exe" {
        75
    } else if lower.contains("windows") {
        70
    } else {
        50
    }
}

/// Pick the portable exe. One SKU since 0.3.x: embedded engine, named
/// `Marionette.exe`. Legacy Engine/plain assets still score so an older
/// release can update an older install.
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
        let score = asset_score(name);
        if score < 0 {
            continue;
        }
        if best.map(|(_, _, s)| score > s).unwrap_or(true) {
            best = Some((name, url, score));
        }
    }
    best.map(|(n, u, _)| (n, u))
}

/// If the last apply failed, read the bat's failure note and consume the
/// marker. The bat writes `apply-result.txt` in the updates dir and the app
/// exits before the copy result is known, so the next check surfaces it.
fn read_apply_result(dir: &Path) -> Option<String> {
    let marker = dir.join("apply-result.txt");
    let bytes = match fs::read(&marker) {
        Ok(b) => b,
        Err(_) => return None,
    };
    let _ = fs::remove_file(&marker);
    let text = String::from_utf8_lossy(&bytes);
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    // A successful copy leaves "1 file(s) copied." in the marker before it is
    // deleted; only report genuine failures.
    if !text.contains("Update copy failed") && !text.contains("timed out") {
        return None;
    }
    // Cap bytes without splitting a UTF-8 char (cmd may emit localized text).
    let mut end = text.len().min(300);
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    Some(format!("上次自动更新失败，未能替换程序文件：{}", &text[..end]))
}

pub fn check_for_update() -> AppUpdateInfo {
    let current = current_version();
    let failed_note = updates_dir().ok().and_then(|d| read_apply_result(&d));
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
                note: failed_note
                    .clone()
                    .or_else(|| Some(format!("TLS unavailable: {e}"))),
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
                note: failed_note
                    .clone()
                    .or_else(|| Some("Could not reach GitHub Releases".into())),
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
                note: failed_note
                    .clone()
                    .or_else(|| Some("Invalid GitHub Releases response".into())),
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
    } else if failed_note.is_some() {
        failed_note
    } else if asset_url.is_none() && !latest.is_empty() {
        Some("Release has no matching portable build".into())
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
    //
    // `chcp 65001` must run before any non-ASCII path is parsed: the bat is
    // written as UTF-8, and cmd otherwise decodes it with the system ANSI/OEM
    // codepage (Shift-JIS/GBK on CJK systems), which garbles non-ASCII paths
    // (工具 → mojibake) and silently breaks the copy. Verified to work under
    // the CREATE_NO_WINDOW spawn below.
    //
    // Failures (copy error / wait timeout) write apply-result.txt in this dir
    // so the next check_for_update() can surface them — the app has already
    // exited by the time the bat runs, so nothing else can report them.
    let bat = dir.join("apply-update.bat");
    let bat_body = format!(
        r#"@echo off
chcp 65001 >NUL
setlocal EnableDelayedExpansion
set "TARGET={target}"
set "NEW={new}"
set "PID={pid}"
set /a TRIES=0
:wait
tasklist /FI "PID eq %PID%" 2>NUL | find "%PID%" >NUL
if not errorlevel 1 (
  set /a TRIES+=1
  if !TRIES! GEQ 180 (
    echo timed out waiting for app exit > "%~dp0apply-result.txt"
    exit /b 2
  )
  timeout /t 1 /nobreak >NUL
  goto wait
)
copy /Y "%NEW%" "%TARGET%" > "%~dp0apply-result.txt" 2>&1
if errorlevel 1 (
  echo Update copy failed >> "%~dp0apply-result.txt"
  exit /b 1
)
del "%~dp0apply-result.txt" 2>NUL
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn assets(names: &[&str]) -> Vec<Value> {
        names
            .iter()
            .map(|n| {
                json!({
                    "name": n,
                    "browser_download_url": format!("https://example.invalid/{n}")
                })
            })
            .collect()
    }

    fn picked(names: &[&str]) -> Option<String> {
        pick_asset(&assets(names)).map(|(n, _)| n.to_string())
    }

    #[test]
    fn prefers_canonical_marionette_exe() {
        let names = [
            "Marionette.exe",
            "Marionette-0.3.0-windows-x64.exe",
            "Marionette-Engine-0.3.0-windows-x64.exe",
            "notes.md",
            "Marionette-uncompressed.exe",
        ];
        assert_eq!(picked(&names).as_deref(), Some("Marionette.exe"));
    }

    #[test]
    fn ignores_uncompressed_and_non_exe() {
        assert_eq!(
            picked(&["Marionette-uncompressed.exe", "notes.md"]),
            None
        );
    }

    #[test]
    fn legacy_engine_beats_legacy_plain() {
        let names = [
            "Marionette-0.3.0-windows-x64.exe",
            "Marionette-Engine-0.3.0-windows-x64.exe",
        ];
        assert_eq!(
            picked(&names).as_deref(),
            Some("Marionette-Engine-0.3.0-windows-x64.exe")
        );
    }

    #[test]
    fn legacy_unversioned_names_still_match() {
        assert_eq!(picked(&["Marionette.exe"]).as_deref(), Some("Marionette.exe"));
        assert_eq!(
            picked(&["Marionette-Engine.exe"]).as_deref(),
            Some("Marionette-Engine.exe")
        );
    }
}
