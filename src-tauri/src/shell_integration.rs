//! File Explorer "Open Marionette here" context menu + launch handoff.
//!
//! - Registers under HKCU (no admin) for Directory + Directory\Background.
//! - Launch with `--open-path <dir>` (also accepts a bare directory path).
//! - Single-instance: a second start writes a pending path and wakes the first.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};

/// Event the frontend listens for when a second process hands off a path.
pub const OPEN_PATH_EVENT: &str = "marionette-open-path";

const SHELL_KEY_NAME: &str = "MarionetteOpenHere";
const MENU_LABEL: &str = "在此处打开 Marionette";
const PENDING_FILE_NAME: &str = "pending-open-path.txt";
const INSTANCE_LOCK_NAME: &str = "instance.lock";

/// Path requested at process start (`--open-path`). Consumed once by the UI.
static LAUNCH_OPEN_PATH: Mutex<Option<String>> = Mutex::new(None);

/// Keep the exclusive instance lock open for the process lifetime.
static INSTANCE_LOCK: OnceLock<std::fs::File> = OnceLock::new();

/// Parse CLI for a folder to open as a project.
///
/// Supported:
/// - `--open-path <dir>` / `--open-here <dir>` / `-p <dir>`
/// - `--open-path=<dir>`
/// - a single bare path argument that is an existing directory (or a file → parent)
pub fn parse_open_path_from_args() -> Option<String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        return None;
    }

    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--open-path" || a == "--open-here" || a == "-p" {
            if let Some(next) = args.get(i + 1) {
                return normalize_open_path(next);
            }
            return None;
        }
        if let Some(rest) = a
            .strip_prefix("--open-path=")
            .or_else(|| a.strip_prefix("--open-here="))
        {
            return normalize_open_path(rest);
        }
        i += 1;
    }

    // Single non-flag argument that looks like a path (Explorer "%V" / drag-drop).
    if args.len() == 1 {
        let only = args[0].as_str();
        if !only.starts_with('-') {
            return normalize_open_path(only);
        }
    }

    None
}

/// Resolve to a real directory path string suitable for `add_project`.
pub fn normalize_open_path(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_matches('"').trim();
    if trimmed.is_empty() {
        return None;
    }
    let path = Path::new(trimmed);
    let canon = fs::canonicalize(path).ok()?;
    let dir = if canon.is_dir() {
        canon
    } else if canon.is_file() {
        canon.parent()?.to_path_buf()
    } else {
        return None;
    };
    Some(strip_extended_prefix(dir))
}

fn strip_extended_prefix(path: PathBuf) -> String {
    let s = path.to_string_lossy();
    // Windows canonicalize often yields `\\?\C:\...` — drop the prefix for display / match.
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        if let Some(unc) = rest.strip_prefix(r"UNC\") {
            return format!(r"\\{unc}");
        }
        return rest.to_string();
    }
    s.into_owned()
}

pub fn set_launch_open_path(path: Option<String>) {
    if let Ok(mut g) = LAUNCH_OPEN_PATH.lock() {
        *g = path;
    }
}

/// Take the path once (startup). Further requests return `None`.
pub fn take_launch_open_path() -> Option<String> {
    LAUNCH_OPEN_PATH.lock().ok().and_then(|mut g| g.take())
}

fn pending_path_file() -> Option<PathBuf> {
    crate::app_paths::global_dir()
        .ok()
        .map(|d| d.join(PENDING_FILE_NAME))
}

fn instance_lock_file() -> Option<PathBuf> {
    crate::app_paths::global_dir()
        .ok()
        .map(|d| d.join(INSTANCE_LOCK_NAME))
}

pub fn write_pending_open_path(path: &str) -> Result<(), String> {
    let file = pending_path_file().ok_or_else(|| "No global data dir".to_string())?;
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Create data dir failed: {e}"))?;
    }
    fs::write(&file, path).map_err(|e| format!("Write pending open path failed: {e}"))
}

pub fn take_pending_open_path() -> Option<String> {
    let file = pending_path_file()?;
    if !file.exists() {
        return None;
    }
    let raw = fs::read_to_string(&file).ok()?;
    let _ = fs::remove_file(&file);
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        // Sentinel: second instance started without a path (just focus).
        return Some(String::new());
    }
    normalize_open_path(trimmed).or_else(|| Some(trimmed.to_string()))
}

/// Current process image path (for registry command / icon).
pub fn current_exe_string() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let canon = fs::canonicalize(&exe).unwrap_or(exe);
    Ok(strip_extended_prefix(canon))
}

// ── Single-instance ────────────────────────────────────────────────────────

/// Returns `true` if this process should continue as the primary UI.
/// Returns `false` after handing off to an already-running instance (caller should exit).
pub fn acquire_single_instance_or_handoff(open_path: Option<&str>) -> bool {
    let Some(lock_path) = instance_lock_file() else {
        return true;
    };
    if let Some(parent) = lock_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    match try_acquire_instance_lock(&lock_path) {
        Some(file) => {
            let _ = INSTANCE_LOCK.set(file);
            true
        }
        None => {
            // Another Marionette holds the lock — hand off and exit.
            if let Some(path) = open_path {
                let _ = write_pending_open_path(path);
            } else {
                let _ = write_pending_open_path("");
            }
            focus_main_window();
            false
        }
    }
}

fn try_acquire_instance_lock(path: &Path) -> Option<std::fs::File> {
    use std::fs::OpenOptions;
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // share_mode 0 = exclusive; a second process fails while the first holds the file.
        OpenOptions::new()
            .create(true)
            .write(true)
            .read(true)
            .share_mode(0)
            .open(path)
            .ok()
    }
    #[cfg(not(windows))]
    {
        // Best-effort lock via create_new; not perfect across crashes but fine for UX.
        if path.exists() {
            // Stale lock from a crash — try remove if process is gone is hard; allow multi.
            let _ = fs::remove_file(path);
        }
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .ok()
    }
}

fn focus_main_window() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            FindWindowW, IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE,
        };

        let title: Vec<u16> = "Marionette\0".encode_utf16().collect();
        unsafe {
            let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
            if hwnd.is_null() {
                return;
            }
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            }
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

// ── Explorer context menu (HKCU) ───────────────────────────────────────────

/// Install / refresh the Explorer right-click entries with the current exe path.
pub fn register_context_menu() -> Result<(), String> {
    #[cfg(windows)]
    {
        return windows_register_context_menu();
    }
    #[cfg(not(windows))]
    {
        Ok(())
    }
}

pub fn unregister_context_menu() -> Result<(), String> {
    #[cfg(windows)]
    {
        return windows_unregister_context_menu();
    }
    #[cfg(not(windows))]
    {
        Ok(())
    }
}

pub fn is_context_menu_registered() -> bool {
    #[cfg(windows)]
    {
        windows_is_registered()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(windows)]
fn windows_register_context_menu() -> Result<(), String> {
    let exe = current_exe_string()?;
    // "%V" = selected folder, or current folder for background click.
    let command = format!("\"{exe}\" --open-path \"%V\"");
    let icon = format!("\"{exe}\",0");

    for base in [
        r"Software\Classes\Directory\shell",
        r"Software\Classes\Directory\Background\shell",
    ] {
        let key_path = format!(r"{base}\{SHELL_KEY_NAME}");
        reg_set_string(&key_path, None, MENU_LABEL)?;
        reg_set_string(&key_path, Some("MUIVerb"), MENU_LABEL)?;
        reg_set_string(&key_path, Some("Icon"), &icon)?;
        reg_set_string(&key_path, Some("Position"), "Middle")?;
        let cmd_path = format!(r"{key_path}\command");
        reg_set_string(&cmd_path, None, &command)?;
    }
    Ok(())
}

#[cfg(windows)]
fn windows_unregister_context_menu() -> Result<(), String> {
    for base in [
        r"Software\Classes\Directory\shell",
        r"Software\Classes\Directory\Background\shell",
    ] {
        let key_path = format!(r"{base}\{SHELL_KEY_NAME}");
        // Delete command subkey first, then the shell verb key.
        let _ = reg_delete_key(&format!(r"{key_path}\command"));
        let _ = reg_delete_key(&key_path);
    }
    Ok(())
}

#[cfg(windows)]
fn windows_is_registered() -> bool {
    reg_key_exists(&format!(
        r"Software\Classes\Directory\shell\{SHELL_KEY_NAME}"
    ))
}

#[cfg(windows)]
fn reg_set_string(subkey: &str, value_name: Option<&str>, data: &str) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyW, RegSetValueExW, HKEY_CURRENT_USER, REG_SZ,
    };

    let sub_w: Vec<u16> = std::ffi::OsStr::new(subkey)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut hkey = std::ptr::null_mut();
    let status = unsafe { RegCreateKeyW(HKEY_CURRENT_USER, sub_w.as_ptr(), &mut hkey) };
    if status != ERROR_SUCCESS {
        return Err(format!("RegCreateKeyW({subkey}) failed: {status}"));
    }

    let name_w: Option<Vec<u16>> = value_name.map(|n| {
        std::ffi::OsStr::new(n)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    });
    // REG_SZ data must be UTF-16 including trailing NUL, size in bytes.
    let mut data_w: Vec<u16> = std::ffi::OsStr::new(data)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let bytes = (data_w.len() * 2) as u32;
    let name_ptr = name_w
        .as_ref()
        .map(|v| v.as_ptr())
        .unwrap_or(std::ptr::null());

    let set_status = unsafe {
        RegSetValueExW(
            hkey,
            name_ptr,
            0,
            REG_SZ,
            data_w.as_mut_ptr() as *const u8,
            bytes,
        )
    };
    unsafe {
        let _ = RegCloseKey(hkey);
    }
    if set_status != ERROR_SUCCESS {
        return Err(format!("RegSetValueExW failed: {set_status}"));
    }
    Ok(())
}

#[cfg(windows)]
fn reg_delete_key(subkey: &str) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{RegDeleteKeyW, HKEY_CURRENT_USER};

    let sub_w: Vec<u16> = std::ffi::OsStr::new(subkey)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let status = unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, sub_w.as_ptr()) };
    // 2 = ERROR_FILE_NOT_FOUND — already gone.
    if status != ERROR_SUCCESS && status != 2 {
        return Err(format!("RegDeleteKeyW({subkey}) failed: {status}"));
    }
    Ok(())
}

#[cfg(windows)]
fn reg_key_exists(subkey: &str) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, HKEY_CURRENT_USER, KEY_READ,
    };

    let sub_w: Vec<u16> = std::ffi::OsStr::new(subkey)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut hkey = std::ptr::null_mut();
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            sub_w.as_ptr(),
            0,
            KEY_READ,
            &mut hkey,
        )
    };
    if status == ERROR_SUCCESS {
        unsafe {
            let _ = RegCloseKey(hkey);
        }
        true
    } else {
        false
    }
}

/// Poll pending open-path file and emit to the frontend (second-instance handoff).
pub fn spawn_pending_open_watcher(app: AppHandle) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(450));
            // Empty string means "just focus" from a second start without path.
            let Some(raw) = take_pending_open_path() else {
                continue;
            };
            if raw.is_empty() {
                focus_main_window();
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.unminimize();
                    let _ = win.set_focus();
                    let _ = win.show();
                }
                continue;
            }
            crate::debug_log::append(
                "shell",
                "info",
                "",
                "open-here handoff from second instance",
                Some(&raw),
            );
            let _ = app.emit(OPEN_PATH_EVENT, raw);
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.unminimize();
                let _ = win.set_focus();
                let _ = win.show();
            }
            focus_main_window();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_normalize_is_none() {
        assert!(normalize_open_path("").is_none());
        assert!(normalize_open_path("   ").is_none());
    }
}
