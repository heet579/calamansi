//! Detecting "the user is dragging with Shift held" anywhere on screen.
//!
//! Windows has no global drag-started event, so a low-level mouse hook
//! watches for: left button down, movement past a small threshold, Shift
//! held. It then posts `WM_APP_DRAG_START` to the main window, which shows an
//! invisible (alpha = 1) drop target under the cursor. Only if OLE then
//! delivers *files* to that target does the wheel become visible, so a
//! Shift-drag that isn't a file drag (drawing a straight line in Paint,
//! selecting text) never shows anything.
//!
//! The hook does no work beyond comparing numbers and posting a message:
//! slow low-level hooks make the whole system's mouse lag, and Windows
//! silently removes hooks that take too long.

use crate::util::pack_point;
use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, Ordering};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_SHIFT};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub const WM_APP_DRAG_START: u32 = WM_APP + 2;
pub const WM_APP_DRAG_END: u32 = WM_APP + 3;

/// Pixels the pointer must travel before a press counts as a drag.
const THRESHOLD: i32 = 10;

static TARGET: AtomicPtr<c_void> = AtomicPtr::new(null_mut());
static HOOK: AtomicPtr<c_void> = AtomicPtr::new(null_mut());
static ENABLED: AtomicBool = AtomicBool::new(true);
static ARMED: AtomicBool = AtomicBool::new(false);
static FIRED: AtomicBool = AtomicBool::new(false);
static DOWN_X: AtomicI32 = AtomicI32::new(0);
static DOWN_Y: AtomicI32 = AtomicI32::new(0);

/// Where the drag began (used to check the source app against exclusions).
pub fn down_point() -> (i32, i32) {
    (
        DOWN_X.load(Ordering::Relaxed),
        DOWN_Y.load(Ordering::Relaxed),
    )
}

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Install on the UI thread, which must run a message loop.
pub fn install(target: HWND) -> bool {
    TARGET.store(target, Ordering::SeqCst);
    // Some Windows versions reject a null module handle for global hooks.
    let module =
        unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) };
    let hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(proc), module, 0) };
    HOOK.store(hook, Ordering::SeqCst);
    !hook.is_null()
}

pub fn uninstall() {
    let hook = HOOK.swap(null_mut(), Ordering::SeqCst);
    if !hook.is_null() {
        unsafe { UnhookWindowsHookEx(hook) };
    }
}

fn key_down(vk: u16) -> bool {
    unsafe { (GetAsyncKeyState(i32::from(vk)) as u16 & 0x8000) != 0 }
}

unsafe extern "system" fn proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && ENABLED.load(Ordering::Relaxed) {
        let info = &*(lparam as *const MSLLHOOKSTRUCT);
        let (x, y) = (info.pt.x, info.pt.y);
        match wparam as u32 {
            WM_LBUTTONDOWN => {
                DOWN_X.store(x, Ordering::Relaxed);
                DOWN_Y.store(y, Ordering::Relaxed);
                FIRED.store(false, Ordering::Relaxed);
                ARMED.store(true, Ordering::Relaxed);
            }
            WM_MOUSEMOVE if ARMED.load(Ordering::Relaxed) && !FIRED.load(Ordering::Relaxed) => {
                let (dx, dy) = (
                    x - DOWN_X.load(Ordering::Relaxed),
                    y - DOWN_Y.load(Ordering::Relaxed),
                );
                if dx.abs().max(dy.abs()) >= THRESHOLD && key_down(VK_SHIFT) {
                    FIRED.store(true, Ordering::Relaxed);
                    let tools = key_down(VK_CONTROL) as usize;
                    PostMessageW(
                        TARGET.load(Ordering::Relaxed),
                        WM_APP_DRAG_START,
                        tools,
                        pack_point(x, y),
                    );
                }
            }
            WM_LBUTTONUP => {
                ARMED.store(false, Ordering::Relaxed);
                if FIRED.swap(false, Ordering::Relaxed) {
                    PostMessageW(TARGET.load(Ordering::Relaxed), WM_APP_DRAG_END, 0, 0);
                }
            }
            _ => {}
        }
    }
    CallNextHookEx(null_mut(), code, wparam, lparam)
}
