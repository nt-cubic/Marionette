//! Portable launch gate: the global data folder must be writable.
//!
//! `%USERPROFILE%\.marionette` holds the single-instance lock, the session list
//! and the dev diary. When the process cannot write there, the app used to leave
//! through the single-instance branch — a window-less `exit(0)` with no log line
//! and no dialog, so "the exe does nothing" was the only symptom.
//!
//! Windows produces exactly that state for a process with restricted rights: a
//! sandbox token, or an image carrying a **Low** mandatory integrity label
//! (`icacls <exe> /setintegritylevel Low`). Reads still succeed, writes are
//! denied with `Access is denied (5)`, and the low label is inherited by every
//! file created in such a directory — so one sandboxed build can leave a whole
//! project tree unable to write outside itself.
//!
//! Repair in that case: `icacls <project> /setintegritylevel (OI)(CI)Medium`.

/// Call before `tauri::Builder`. Exits the process with a native dialog.
pub fn prompt_and_exit(err: &str) -> ! {
    #[cfg(windows)]
    {
        windows::message_and_exit(err);
    }

    #[cfg(not(windows))]
    {
        eprintln!("Marionette cannot write its data folder: {err}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod windows {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONERROR, MB_OK,
    };

    /// Show the failure and exit — the app cannot start without its data folder.
    pub fn message_and_exit(err: &str) -> ! {
        let title = wide("Marionette — data folder is not writable");
        let body = wide(&format!(
            "Marionette could not write to its data folder:\n\n{err}\n\n\
             That folder holds the single-instance lock, the session list and the \
             debug log, so the app cannot start without it.\n\n\
             Usual causes:\n\
             \u{2022} this program was started with restricted rights \
             (a sandbox, or \"run as a different user\");\n\
             \u{2022} Windows file permissions on %USERPROFILE%\\.marionette deny writes.\n\n\
             Fix: launch Marionette normally, from the account that owns \
             %USERPROFILE%."
        ));

        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                body.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONERROR,
            );
        }

        std::process::exit(1);
    }

    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }
}
