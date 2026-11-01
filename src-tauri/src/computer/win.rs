//! Native Windows facts the helper needs: process identity (image path +
//! start time — pid reuse makes pid alone worthless), the own-app/blocklist
//! check that decides grantability, and the clipboard (text read/write plus
//! the sequence number that decides whether it still holds what the agent
//! wrote). All `windows-sys`, no COM apartment games.

#![allow(non_snake_case, non_upper_case_globals, non_camel_case_types)]

use serde::Serialize;


#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessFacts {
    pub image_path: String,
    pub started_at_ms: u64,
}

pub fn process_facts(pid: u32) -> Option<ProcessFacts> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut name = [0u16; 1024];
        let mut size = name.len() as u32;
        let mut path = String::new();
        if QueryFullProcessImageNameW(handle, 0, name.as_mut_ptr(), &mut size) != 0 {
            path = String::from_utf16_lossy(&name[..size as usize]);
        }
        let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let mut creation = zero;
        let mut exit = zero;
        let mut kernel = zero;
        let mut user = zero;
        let mut started = 0u64;
        if GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) != 0 {
            let ticks = ((creation.dwHighDateTime as u64) << 32) | creation.dwLowDateTime as u64;
            // FILETIME → Unix ms: 116444736000000000 is the 1601→1970 offset.
            started = ticks.saturating_sub(116_444_736_000_000_000) / 10_000;
        }
        CloseHandle(handle);
        if path.is_empty() && started == 0 {
            return None;
        }
        Some(ProcessFacts { image_path: path, started_at_ms: started })
    }
}

/// Is this image one Marionette itself runs? The own-exe check is by path;
/// the name fallback catches a dev build sitting next to an installed one.
pub fn is_own_image(image_path: &str, own_exe: &str) -> bool {
    let lower = image_path.to_ascii_lowercase();
    if !own_exe.is_empty() {
        let own = own_exe.to_ascii_lowercase();
        if lower == own || lower.replace('/', "\\") == own.replace('/', "\\") {
            return true;
        }
    }
    lower.ends_with("\\marionette.exe")
}

pub fn is_blocklisted(image_path: &str) -> bool {
    let file = image_path
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    super::keys::DEFAULT_BLOCKLIST.contains(&file.as_str())
}

/// The clipboard sequence number: bumped by every write. Remembered after an
/// agent's own clipboard write, an unchanged number at press time is what
/// makes pasting it back acceptable.
pub fn clipboard_sequence() -> u32 {
    use windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber;
    unsafe { GetClipboardSequenceNumber() }
}

/// Text clipboard read (CF_UNICODETEXT). Empty when another format holds it.
pub fn clipboard_read_text() -> Option<String> {
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    };
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    use windows_sys::Win32::System::Ole::CF_UNICODETEXT;
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return None;
        }
        let result = (|| {
            if IsClipboardFormatAvailable(CF_UNICODETEXT as u32) == 0 {
                return None;
            }
            let handle: HANDLE = GetClipboardData(CF_UNICODETEXT as u32);
            if handle.is_null() {
                return None;
            }
            let lock = GlobalLock(handle);
            if lock.is_null() {
                return None;
            }
            let size = GlobalSize(handle);
            // The clipboard guarantees a NUL; cut there, bounded by size.
            let bytes = std::slice::from_raw_parts(lock as *const u8, size);
            let mut end = 0usize;
            while end + 1 < bytes.len() {
                if bytes[end] == 0 && bytes[end + 1] == 0 {
                    break;
                }
                end += 2;
            }
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(
                lock as *const u16,
                end / 2,
            ));
            GlobalUnlock(handle);
            Some(text)
        })();
        CloseClipboard();
        result
    }
}

/// Text clipboard write; returns the new sequence number.
pub fn clipboard_write_text(text: &str) -> Option<u32> {
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows_sys::Win32::System::Ole::CF_UNICODETEXT;
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return None;
        }
        let result = (|| {
            if EmptyClipboard() == 0 {
                return None;
            }
            let wide: Vec<u16> = text.encode_utf16().chain([0]).collect();
            let bytes = wide.len() * 2;
            let handle = GlobalAlloc(GMEM_MOVEABLE, bytes);
            if handle.is_null() {
                return None;
            }
            let lock = GlobalLock(handle);
            if lock.is_null() {
                return None;
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), lock as *mut u16, wide.len());
            GlobalUnlock(handle);
            if SetClipboardData(CF_UNICODETEXT as u32, handle as HANDLE).is_null() {
                return None;
            }
            Some(clipboard_sequence())
        })();
        CloseClipboard();
        result
    }
}
