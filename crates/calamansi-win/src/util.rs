//! Small Win32 helpers: strings, monitors, processes, clipboard, Explorer.

use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::GlobalFree;
use windows_sys::Win32::Foundation::{CloseHandle, HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::Shell::{
    ILCreateFromPathW, ILFree, SHOpenFolderAndSelectItems, ShellExecuteW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetWindowThreadProcessId, WindowFromPoint, GA_ROOT, SW_SHOWNORMAL,
};

const CF_UNICODETEXT: u32 = 13;

/// NUL-terminated UTF-16 for a `&str`.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// NUL-terminated UTF-16 for a path, preserving non-Unicode names on Windows.
pub fn wide_path(p: &Path) -> Vec<u16> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        p.as_os_str().encode_wide().chain(Some(0)).collect()
    }
    #[cfg(not(windows))]
    {
        wide(&p.to_string_lossy())
    }
}

pub fn path_from_wide(buf: &[u16]) -> PathBuf {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        PathBuf::from(std::ffi::OsString::from_wide(&buf[..end]))
    }
    #[cfg(not(windows))]
    {
        PathBuf::from(String::from_utf16_lossy(&buf[..end]))
    }
}

pub fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

/// Pack a screen point into an LPARAM (x in the low 32 bits, y in the high).
pub fn pack_point(x: i32, y: i32) -> isize {
    (((y as u32 as u64) << 32) | x as u32 as u64) as isize
}

pub fn unpack_point(l: isize) -> (i32, i32) {
    let v = l as u64;
    ((v & 0xFFFF_FFFF) as u32 as i32, (v >> 32) as u32 as i32)
}

/// Display scale (1.0 = 96 DPI) of the monitor containing `pt`.
pub fn scale_at(pt: POINT) -> f32 {
    unsafe {
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let (mut x, mut y) = (96u32, 96u32);
        if GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut x, &mut y) != 0 {
            return 1.0;
        }
        x as f32 / 96.0
    }
}

/// Work area (screen minus taskbar) of the monitor containing `pt`.
pub fn work_area_at(pt: POINT) -> RECT {
    unsafe {
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(mon, &mut info) == 0 {
            return RECT {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            };
        }
        info.rcWork
    }
}

/// Executable file name (e.g. "explorer.exe") of the top-level window at `pt`.
pub fn exe_at(pt: POINT) -> Option<String> {
    unsafe {
        let hwnd = WindowFromPoint(pt);
        if hwnd.is_null() {
            return None;
        }
        let root = GetAncestor(hwnd, GA_ROOT);
        let mut pid = 0u32;
        GetWindowThreadProcessId(if root.is_null() { hwnd } else { root }, &mut pid);
        if pid == 0 {
            return None;
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(process, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(process);
        if ok == 0 {
            return None;
        }
        Some(file_name(&path_from_wide(&buf[..len as usize])))
    }
}

pub fn copy_to_clipboard(owner: HWND, text: &str) -> bool {
    let data: Vec<u16> = wide(text);
    let bytes = data.len() * 2;
    unsafe {
        if OpenClipboard(owner) == 0 {
            return false;
        }
        EmptyClipboard();
        let mem = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if mem.is_null() {
            CloseClipboard();
            return false;
        }
        let dst = GlobalLock(mem) as *mut u16;
        if dst.is_null() {
            GlobalFree(mem);
            CloseClipboard();
            return false;
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
        GlobalUnlock(mem);
        // On success the clipboard owns `mem`; free it only on failure.
        let ok = !SetClipboardData(CF_UNICODETEXT, mem).is_null();
        if !ok {
            GlobalFree(mem);
        }
        CloseClipboard();
        ok
    }
}

/// Open File Explorer with `path` selected.
pub fn reveal(path: &Path) {
    let w = wide_path(path);
    unsafe {
        let pidl = ILCreateFromPathW(w.as_ptr());
        if pidl.is_null() {
            return open(path.parent().unwrap_or(path));
        }
        SHOpenFolderAndSelectItems(pidl, 0, null(), 0);
        ILFree(pidl);
    }
}

/// Open a file or folder with its default handler.
pub fn open(path: &Path) {
    let verb = wide("open");
    let w = wide_path(path);
    unsafe {
        ShellExecuteW(
            null_mut(),
            verb.as_ptr(),
            w.as_ptr(),
            null(),
            null(),
            SW_SHOWNORMAL,
        );
    }
}

/// Append a line to `%LOCALAPPDATA%\Calamansi\calamansi.log`. Never fails loudly.
pub fn log(msg: &str) {
    use std::io::Write;
    let Some(dir) = dirs::data_local_dir().map(|d| d.join("Calamansi")) else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("calamansi.log");
    if std::fs::metadata(&path)
        .map(|m| m.len() > 1_000_000)
        .unwrap_or(false)
    {
        let _ = std::fs::rename(&path, dir.join("calamansi.old.log"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "[{t}] {msg}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_packing_round_trips_negative_coordinates() {
        for (x, y) in [(0, 0), (-1920, 50), (3840, -200), (i32::MIN, i32::MAX)] {
            assert_eq!(unpack_point(pack_point(x, y)), (x, y));
        }
    }

    #[test]
    fn wide_is_nul_terminated() {
        assert_eq!(wide("ab"), vec![97, 98, 0]);
        assert_eq!(path_from_wide(&[97, 0, 98]), PathBuf::from("a"));
    }
}
