//! Portable launch gate: require WebView2 before Tauri creates a window.
//!
//! No silent install — if the runtime is missing, show a native dialog with a
//! download link so the user stays in control.

/// Evergreen Bootstrapper (≈2 MB). Opens the Microsoft download when missing.
const WEBVIEW2_BOOTSTRAPPER_URL: &str =
    "https://go.microsoft.com/fwlink/p/?LinkId=2124703";

/// Call before `tauri::Builder`. Exits the process if WebView2 is unavailable.
pub fn ensure_or_exit() {
    #[cfg(windows)]
    {
        if windows::is_installed() {
            return;
        }
        windows::prompt_and_exit();
    }

    #[cfg(not(windows))]
    {
        // Non-Windows builds are out of product scope for v0.1 portable.
    }
}

#[cfg(windows)]
mod windows {
    use super::WEBVIEW2_BOOTSTRAPPER_URL;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{ERROR_SUCCESS, MAX_PATH};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE,
        KEY_READ, REG_SZ,
    };
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, IDYES, MB_ICONWARNING, MB_YESNO, SW_SHOWNORMAL,
    };

    /// WebView2 Evergreen Runtime client GUID (Microsoft).
    const CLIENT_GUID: &str = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";

    const REG_PATHS: &[(&str, HKEY)] = &[
        (
            r"SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}",
            HKEY_LOCAL_MACHINE,
        ),
        (
            r"SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}",
            HKEY_LOCAL_MACHINE,
        ),
        (
            r"SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}",
            HKEY_CURRENT_USER,
        ),
    ];

    pub fn is_installed() -> bool {
        for (path, root) in REG_PATHS {
            if key_has_pv(*root, path) {
                return true;
            }
        }
        // Also accept key existence without pv (some layouts only set presence).
        for (path, root) in REG_PATHS {
            if key_exists(*root, path) {
                return true;
            }
        }
        let _ = CLIENT_GUID; // documented constant; paths embed the same GUID
        false
    }

    fn key_exists(root: HKEY, path: &str) -> bool {
        let mut hkey: HKEY = std::ptr::null_mut();
        let status = unsafe {
            RegOpenKeyExW(
                root,
                wide(path).as_ptr(),
                0,
                KEY_READ,
                &mut hkey,
            )
        };
        if status == ERROR_SUCCESS {
            unsafe {
                RegCloseKey(hkey);
            }
            true
        } else {
            false
        }
    }

    /// Prefer keys that have a non-empty `pv` (product version) value.
    fn key_has_pv(root: HKEY, path: &str) -> bool {
        let mut hkey: HKEY = std::ptr::null_mut();
        let status = unsafe {
            RegOpenKeyExW(
                root,
                wide(path).as_ptr(),
                0,
                KEY_READ,
                &mut hkey,
            )
        };
        if status != ERROR_SUCCESS {
            return false;
        }

        let mut data = [0u16; MAX_PATH as usize];
        let mut data_size = (data.len() * 2) as u32;
        let mut ty = 0u32;
        let q = unsafe {
            RegQueryValueExW(
                hkey,
                wide("pv").as_ptr(),
                std::ptr::null_mut(),
                &mut ty,
                data.as_mut_ptr() as *mut u8,
                &mut data_size,
            )
        };
        unsafe {
            RegCloseKey(hkey);
        }

        if q != ERROR_SUCCESS || ty != REG_SZ {
            return false;
        }
        let n = (data_size as usize / 2).saturating_sub(1);
        let ver = String::from_utf16_lossy(&data[..n.min(data.len())]);
        !ver.trim().is_empty() && ver.trim() != "0.0.0.0"
    }

    pub fn prompt_and_exit() -> ! {
        let title = wide("Marionette — missing WebView2");
        let body = wide(
            "Marionette needs Microsoft Edge WebView2 Runtime to run.\n\n\
             This is a free system component from Microsoft (usually already \
             installed on Windows 10/11).\n\n\
             Open the official WebView2 installer download page now?",
        );

        let answer = unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                body.as_ptr(),
                title.as_ptr(),
                MB_YESNO | MB_ICONWARNING,
            )
        };

        if answer == IDYES as i32 {
            open_url(WEBVIEW2_BOOTSTRAPPER_URL);
        }

        std::process::exit(1);
    }

    fn open_url(url: &str) {
        let op = wide("open");
        let target = wide(url);
        unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                op.as_ptr(),
                target.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL as i32,
            );
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }
}
