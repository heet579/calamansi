//! Start-with-Windows and the Explorer "Send to" entry.
//!
//! Both are per-user and need no administrator rights.

use crate::util::wide;
use std::path::{Path, PathBuf};
use std::ptr::null_mut;

use windows_sys::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "Calamansi";

fn exe() -> Option<PathBuf> {
    std::env::current_exe().ok()
}

pub fn starts_at_login() -> bool {
    let (key, name) = (wide(RUN_KEY), wide(VALUE));
    let mut size = 0u32;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            null_mut(),
            &mut size,
        ) == 0
    }
}

pub fn set_starts_at_login(on: bool) -> bool {
    let (key, name) = (wide(RUN_KEY), wide(VALUE));
    unsafe {
        if on {
            let Some(exe) = exe() else { return false };
            let command = wide(&format!("\"{}\" --background", exe.display()));
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                command.as_ptr() as *const _,
                (command.len() * 2) as u32,
            ) == 0
        } else {
            let r = RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr());
            r == 0 || r == 2 // ERROR_FILE_NOT_FOUND: already off
        }
    }
}

fn send_to_link() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join(r"Microsoft\Windows\SendTo\Calamansi.lnk"))
}

pub fn in_send_to() -> bool {
    send_to_link().is_some_and(|p| p.exists())
}

/// Add or remove "Send to → Calamansi". Explorer appends the selected files
/// to the shortcut's arguments, so the app starts with `--pick <files…>`.
pub fn set_in_send_to(on: bool) -> bool {
    let Some(link) = send_to_link() else {
        return false;
    };
    if !on {
        return std::fs::remove_file(&link).is_ok() || !link.exists();
    }
    let Some(exe) = exe() else { return false };
    create_shortcut(&link, &exe, "--pick")
}

/// Shortcuts are COM objects (IShellLink); PowerShell's WScript.Shell wrapper
/// creates one without pulling a COM framework into the app.
fn create_shortcut(link: &Path, target: &Path, args: &str) -> bool {
    let quote = |p: &Path| p.display().to_string().replace('\'', "''");
    let script = format!(
        "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}');$s.TargetPath='{}';$s.Arguments='{}';$s.IconLocation='{},0';$s.Description='Convert with Calamansi';$s.Save()",
        quote(link),
        quote(target),
        args,
        quote(target),
    );
    #[allow(unused_mut)]
    let mut cmd = std::process::Command::new("powershell.exe");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        &script,
    ]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.status().map(|s| s.success()).unwrap_or(false) && link.exists()
}
