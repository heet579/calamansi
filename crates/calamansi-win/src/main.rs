//! Calamansi for Windows.
//!
//! A system-tray app: Shift-drag files anywhere and a conversion wheel
//! appears under the cursor; drop on a format to convert. Ctrl+Shift-drag
//! opens the tools wheel. Everything is processed locally.
//!
//! Command line:
//!   Calamansi.exe                    start in the tray
//!   Calamansi.exe --background       start quietly (used at login)
//!   Calamansi.exe --pick FILES…      open the convert wheel for FILES ("Send to")
//!   Calamansi.exe --tools FILES…     open the tools wheel for FILES

#![cfg_attr(not(test), windows_subsystem = "windows")]

mod app;
mod droptarget;
mod hook;
mod icon;
mod integration;
mod layered;
mod util;

use calamansi_core::settings::Settings;
use calamansi_core::wheel::Mode;
use std::path::PathBuf;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::System::Ole::{OleInitialize, OleUninitialize};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

struct Args {
    pick: Option<(Vec<PathBuf>, Mode)>,
}

fn parse_args() -> Args {
    let mut mode = None;
    let mut files = Vec::new();
    for arg in std::env::args_os().skip(1) {
        match arg.to_str() {
            Some("--pick") => mode = Some(Mode::Convert),
            Some("--tools") => mode = Some(Mode::Tools),
            Some("--background") => {}
            _ => files.push(PathBuf::from(arg)),
        }
    }
    let files: Vec<PathBuf> = files.into_iter().filter(|p| p.is_file()).collect();
    let pick = if files.is_empty() {
        None
    } else {
        Some((files, mode.unwrap_or(Mode::Convert)))
    };
    Args { pick }
}

/// If Calamansi is already running, hand it our files and return true.
fn forward_to_running_instance(args: &Args) -> bool {
    let name = util::wide("Local\\Calamansi.SingleInstance");
    unsafe {
        let mutex = CreateMutexW(null(), 0, name.as_ptr());
        if mutex.is_null() || GetLastError() != ERROR_ALREADY_EXISTS {
            // We're first. The mutex handle is intentionally kept open for
            // the life of the process.
            return false;
        }
        let class = util::wide(app::MAIN_CLASS);
        let existing = FindWindowW(class.as_ptr(), null());
        if existing.is_null() {
            return true; // starting up or shutting down; don't run twice
        }
        // Let the running instance bring its wheel to the foreground.
        let mut pid = 0u32;
        GetWindowThreadProcessId(existing, &mut pid);
        AllowSetForegroundWindow(pid);

        let (tag, mut payload) = match &args.pick {
            Some((files, mode)) => {
                let mut data = Vec::new();
                for f in files {
                    data.extend(util::wide_path(f));
                }
                let tag = if *mode == Mode::Tools {
                    app::COPY_PICK_TOOLS
                } else {
                    app::COPY_PICK_CONVERT
                };
                (tag, data)
            }
            None => (app::COPY_ALREADY_RUNNING, vec![0u16]),
        };
        let cds = COPYDATASTRUCT {
            dwData: tag,
            cbData: (payload.len() * 2) as u32,
            lpData: payload.as_mut_ptr() as *mut _,
        };
        SendMessageW(existing, WM_COPYDATA, 0, &cds as *const _ as isize);
        true
    }
}

fn main() {
    let args = parse_args();
    if forward_to_running_instance(&args) {
        return;
    }
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // OLE (not just COM) is required for drag-and-drop.
        if OleInitialize(null()) < 0 {
            util::log("OleInitialize failed");
            return;
        }
    }
    let first_run = Settings::default_path().is_some_and(|p| !p.exists());
    std::panic::set_hook(Box::new(|info| util::log(&format!("panic: {info}"))));

    if let Err(e) = app::start(first_run, args.pick) {
        util::log(&format!("startup failed: {e}"));
        unsafe { OleUninitialize() };
        return;
    }

    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    app::shutdown();
    unsafe { OleUninitialize() };
}
