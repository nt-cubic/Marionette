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

/// File-list clipboard read (CF_HDROP — an Explorer copy puts its files
/// here). The real paths, empty when no files are on the clipboard.
pub fn clipboard_file_paths() -> Vec<String> {
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    };
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    const CF_HDROP: u32 = 15;
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return Vec::new();
        }
        let result = (|| {
            if IsClipboardFormatAvailable(CF_HDROP) == 0 {
                return Vec::new();
            }
            let handle: HANDLE = GetClipboardData(CF_HDROP);
            if handle.is_null() {
                return Vec::new();
            }
            let lock = GlobalLock(handle);
            if lock.is_null() {
                return Vec::new();
            }
            let size = GlobalSize(handle);
            let bytes = std::slice::from_raw_parts(lock as *const u8, size);
            let paths = parse_dropfiles(bytes);
            GlobalUnlock(handle);
            paths
        })();
        CloseClipboard();
        result
    }
}

/// A CF_HDROP payload is a 20-byte DROPFILES header — `pFiles` (u32 LE) the
/// offset of the file list, `fWide` (i32 LE) at byte 16 — followed by the
/// list: NUL-terminated strings, wide or narrow per `fWide`, ending with an
/// extra NUL. Parsed by hand: DragQueryFileW would pull in the whole Shell
/// feature for twenty lines of pointer math.
fn parse_dropfiles(bytes: &[u8]) -> Vec<String> {
    if bytes.len() < 20 {
        return Vec::new();
    }
    let p_files = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    let wide = i32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]) != 0;
    if p_files == 0 || p_files >= bytes.len() {
        return Vec::new();
    }
    let list = &bytes[p_files..];
    let mut paths = Vec::new();
    if wide {
        let units: Vec<u16> = list
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        for part in units.split(|&u| u == 0) {
            if part.is_empty() {
                break; // the extra NUL ends the list
            }
            paths.push(String::from_utf16_lossy(part));
        }
    } else {
        for part in list.split(|&b| b == 0) {
            if part.is_empty() {
                break;
            }
            paths.push(String::from_utf8_lossy(part).into_owned());
        }
    }
    paths
}

#[cfg(test)]
mod dropfiles_tests {
    use super::parse_dropfiles;

    /// A real CF_HDROP global: 20-byte header (pFiles=20, fWide=1) + the
    /// wide NUL-terminated list, closed by an extra NUL.
    fn hdrop(paths: &[&str]) -> Vec<u8> {
        let mut out = vec![0u8; 20];
        out[0..4].copy_from_slice(&20u32.to_le_bytes());
        out[16..20].copy_from_slice(&1i32.to_le_bytes());
        for p in paths {
            for u in p.encode_utf16() {
                out.extend_from_slice(&u.to_le_bytes());
            }
            out.extend_from_slice(&0u16.to_le_bytes());
        }
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    #[test]
    fn reads_wide_and_narrow_lists() {
        let got = parse_dropfiles(&hdrop(&[
            "D:\\docs\\需求.md",
            "D:\\with space.txt",
        ]));
        assert_eq!(got, vec!["D:\\docs\\需求.md", "D:\\with space.txt"]);

        let mut narrow = vec![0u8; 20];
        narrow[0..4].copy_from_slice(&20u32.to_le_bytes());
        narrow.extend_from_slice(b"a.txt\0b.txt\0\0");
        assert_eq!(parse_dropfiles(&narrow), vec!["a.txt", "b.txt"]);
    }

    #[test]
    fn refuses_short_and_malformed_payloads() {
        assert!(parse_dropfiles(&[]).is_empty());
        assert!(parse_dropfiles(&[0u8; 10]).is_empty());
        // pFiles beyond the payload / zero.
        let mut bad = vec![0u8; 40];
        bad[0..4].copy_from_slice(&999u32.to_le_bytes());
        assert!(parse_dropfiles(&bad).is_empty());
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
