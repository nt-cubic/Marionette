//! cua-driver lifecycle: pinned download from GitHub releases, SHA-256
//! verified twice (archive before extraction, executable by the helper before
//! every driver start), installed under `~/.marionette/cua/<version>/` with a
//! manifest. The engine.rs download shape (progress events, 2 attempts, temp
//! file + atomic replace, crash recovery sweep) travels with it.
//!
//! The Windows archive is SIX files, not one — the UIA provider, the SDK dll
//! and the node runtime are required siblings of `cua-driver.exe`; a
//! single-file copy installs a driver that handshakes and then can't see a
//! single window.

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use sha2::{Digest, Sha256};

use crate::app_paths::global_dir;

pub const DRIVER_VERSION: &str = "0.36.0";
pub const DRIVER_COMMAND: &str = "cua-driver.exe";

/// Pins for trycua/cua release `cua-driver-rs-v0.36.0` (win-x64), measured
/// 2026-10-31 against the release asset itself. Upgrade = replace these and
/// re-review, never a runtime flag.
const WIN_X64_URL: &str =
    "https://github.com/trycua/cua/releases/download/cua-driver-rs-v0.36.0/cua-driver-rs-0.36.0-windows-x86_64-binary.zip";
const WIN_X64_ARCHIVE_SHA256: &str =
    "f8c47024b626f4e0e70f8428d2998df39e4879110c5722a29d5fd03e367353ad";
/// The executable the helper spawns; re-checked before EVERY driver start
/// (the main process's install-time check says nothing about a file an agent
/// could replace afterwards).
pub const WIN_X64_EXECUTABLE_SHA256: &str =
    "99291d8d6a4851d9df96da5813a1ef2761f2c9d3a59d2a7036c0bf4d09652f4f";

/// Hard stop: the zip is ~35 MB; a corrupted redirect that streams forever
/// must not.
const MAX_DOWNLOAD_BYTES: u64 = 128 * 1024 * 1024;
const MIN_DOWNLOAD_BYTES: u64 = 30 * 1024 * 1024;

/// Download agent honoring the user's proxy (engine.rs shape, minus its
/// test hook — this path is never exercised in tests).
fn download_agent() -> Result<ureq::Agent, String> {
    let connector = ureq::native_tls::TlsConnector::new()
        .map_err(|e| format!("tls init: {e}"))?;
    let mut builder = ureq::AgentBuilder::new();
    builder = builder.tls_connector(std::sync::Arc::new(connector));
    let config = crate::proxy::load_proxy_config();
    if config.enabled && !config.url.trim().is_empty() {
        if let Ok(proxy) = ureq::Proxy::new(config.url.trim()) {
            builder = builder.proxy(proxy);
        }
    }
    Ok(builder.build())
}

pub fn cua_dir() -> Result<std::path::PathBuf, String> {
    global_dir()
        .map(|dir| dir.join("cua"))
        .map_err(|e| format!("global dir: {e}"))
}

pub fn installed_exe_path() -> Result<std::path::PathBuf, String> {
    Ok(cua_dir()?.join(DRIVER_VERSION).join(DRIVER_COMMAND))
}

pub fn is_installed() -> bool {
    installed_exe_path().map(|path| path.is_file()).unwrap_or(false)
}

fn manifest_path() -> Result<std::path::PathBuf, String> {
    Ok(cua_dir()?.join("manifest.json"))
}

pub fn driver_status() -> Value {
    json!({
        "version": DRIVER_VERSION,
        "installed": is_installed(),
        "path": installed_exe_path().ok().map(|p| p.to_string_lossy().to_string()),
    })
}

fn sha256_of_file(path: &std::path::Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("open: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = std::io::Read::read(&mut file, &mut buffer).map_err(|e| format!("read: {e}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn emit_progress(app: &AppHandle, downloaded: u64, total: Option<u64>) {
    let _ = app.emit(
        "marionette-computer-driver",
        json!({"phase": "downloading", "downloaded": downloaded, "total": total}),
    );
}

/// Install (or reinstall) the pinned driver. Synchronous, called from a
/// command thread; progress rides the `marionette-computer-driver` event.
pub fn install(app: &AppHandle) -> Result<Value, String> {
    let dir = cua_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create cua dir: {e}"))?;
    let version_dir = dir.join(DRIVER_VERSION);
    let manifest = manifest_path()?;

    if is_installed() {
        // Already there: still announce it — the frontend refreshes on the
        // event, not on the invoke result (2026-11-01 stuck-at-下载中 hang).
        let _ = app.emit("marionette-computer-driver", json!({"phase": "installed"}));
        return Ok(json!({"installed": true, "already": true}));
    }

    // Download to a pid-tagged temp file (two windows never share one).
    let zip_path = dir.join(format!("driver-{}.download", std::process::id()));
    let result = (|| -> Result<Value, String> {
        let mut last_emit = std::time::Instant::now();
        let agent = download_agent()?;
        let response = agent
            .get(WIN_X64_URL)
            .call()
            .map_err(|e| format!("download request failed: {e}"))?;
        let total = response
            .header("Content-Length")
            .and_then(|v| v.parse::<u64>().ok());
        let mut reader = response.into_reader();
        let mut file = std::fs::File::create(&zip_path).map_err(|e| format!("temp file: {e}"))?;
        let mut downloaded: u64 = 0;
        let mut chunk = [0u8; 64 * 1024];
        loop {
            use std::io::Read;
            let read = reader.read(&mut chunk).map_err(|e| format!("download: {e}"))?;
            if read == 0 {
                break;
            }
            downloaded += read as u64;
            if downloaded > MAX_DOWNLOAD_BYTES {
                return Err("download exceeded size ceiling".to_string());
            }
            std::io::Write::write_all(&mut file, &chunk[..read])
                .map_err(|e| format!("write download: {e}"))?;
            if last_emit.elapsed().as_millis() > 250 {
                emit_progress(app, downloaded, total);
                last_emit = std::time::Instant::now();
            }
        }
        if downloaded < MIN_DOWNLOAD_BYTES {
            return Err(format!("download suspiciously small: {downloaded} bytes"));
        }
        // Archive pin, before anything trusts its contents.
        let archive_sha = sha256_of_file(&zip_path)?;
        if archive_sha != WIN_X64_ARCHIVE_SHA256 {
            return Err(format!(
                "archive sha256 mismatch: expected {WIN_X64_ARCHIVE_SHA256}, got {archive_sha}"
            ));
        }
        // Extract with the system tar (bsdtar handles zip on Windows 10+);
        // the archive was hash-verified a moment ago.
        std::fs::create_dir_all(&version_dir).map_err(|e| format!("version dir: {e}"))?;
        let status = std::process::Command::new("tar.exe")
            .arg("-xf")
            .arg(&zip_path)
            .arg("-C")
            .arg(&version_dir)
            .output()
            .map_err(|e| format!("tar spawn: {e}"))?;
        if !status.status.success() {
            return Err(format!(
                "extract failed: {}",
                String::from_utf8_lossy(&status.stderr)
            ));
        }
        // Executable pin after extraction.
        let exe = version_dir.join(DRIVER_COMMAND);
        if !exe.is_file() {
            return Err("archive did not contain cua-driver.exe".to_string());
        }
        let exe_sha = sha256_of_file(&exe)?;
        if exe_sha != WIN_X64_EXECUTABLE_SHA256 {
            return Err(format!(
                "executable sha256 mismatch: expected {WIN_X64_EXECUTABLE_SHA256}, got {exe_sha}"
            ));
        }
        let manifest_body = json!({
            "version": DRIVER_VERSION,
            "archiveSha256": WIN_X64_ARCHIVE_SHA256,
            "executableSha256": WIN_X64_EXECUTABLE_SHA256,
            "installedAtMs": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
        });
        std::fs::write(&manifest, manifest_body.to_string())
            .map_err(|e| format!("manifest write: {e}"))?;
        Ok(json!({"installed": true}))
    })();
    let _ = std::fs::remove_file(&zip_path);
    let _ = app.emit(
        "marionette-computer-driver",
        json!({"phase": match &result {
            Ok(_) => "installed",
            Err(_) => "failed",
        }}),
    );
    result.map_err(|e| {
        // A failed install never leaves a half-trusted version dir behind.
        let _ = std::fs::remove_dir_all(&version_dir);
        crate::debug_log::append("computer", "warn", "", "driver install failed", Some(&e));
        e
    })
}

/// Remove the driver and every version dir (uninstall stops sharing first —
/// the command layer's job).
pub fn uninstall() -> Result<(), String> {
    let dir = cua_dir()?;
    // NTFS-locked children still let the DIRECTORY rename succeed; park the
    // tree in .trash and let a later sweep finish (Defender-lock lesson).
    let trash = dir.with_extension("trash");
    let _ = std::fs::remove_dir_all(&trash);
    std::fs::rename(&dir, &trash)
        .or_else(|_| std::fs::remove_dir_all(&dir))
        .map_err(|e| format!("uninstall: {e}"))?;
    Ok(())
}
