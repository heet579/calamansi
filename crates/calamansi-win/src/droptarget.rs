//! A minimal hand-written COM `IDropTarget` for the wheel window.
//!
//! Written against raw vtables (rather than a COM framework) to keep the
//! dependency tree to `windows-sys` alone. The object lives for the whole
//! process, so reference counting is honoured but never frees it.
//!
//! Safety rule: the effect returned is only ever `DROPEFFECT_COPY`, `LINK` or
//! `NONE`, never `MOVE`. Shift normally means "move" in Explorer,
//! and returning MOVE would make Explorer delete the user's original files.

use crate::util::path_from_wide;
use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicU32, Ordering};

use windows_sys::core::{GUID, HRESULT};
use windows_sys::Win32::Foundation::{E_NOINTERFACE, E_POINTER, POINTL, S_OK};
use windows_sys::Win32::System::Com::{FORMATETC, STGMEDIUM};
use windows_sys::Win32::System::Ole::{
    ReleaseStgMedium, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_NONE,
};
use windows_sys::Win32::UI::Shell::DragQueryFileW;

const CF_HDROP: u16 = 15;
const DVASPECT_CONTENT: u32 = 1;
const TYMED_HGLOBAL: u32 = 1;

const IID_IUNKNOWN: GUID = GUID::from_u128(0x00000000_0000_0000_c000_000000000046);
const IID_IDROPTARGET: GUID = GUID::from_u128(0x00000122_0000_0000_c000_000000000046);

/// What the drop target asks the app.
pub trait Handler {
    /// Files entered the wheel. Return whether they're accepted.
    fn enter(&self, files: Vec<PathBuf>, x: i32, y: i32) -> bool;
    /// Pointer moved; return whether a drop here would do something.
    fn over(&self, x: i32, y: i32) -> bool;
    fn leave(&self);
    /// Files dropped; return whether an action started.
    fn drop(&self, files: Vec<PathBuf>, x: i32, y: i32) -> bool;
}

#[repr(C)]
struct Vtbl {
    query_interface:
        unsafe extern "system" fn(*mut Target, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut Target) -> u32,
    release: unsafe extern "system" fn(*mut Target) -> u32,
    drag_enter:
        unsafe extern "system" fn(*mut Target, *mut c_void, u32, POINTL, *mut u32) -> HRESULT,
    drag_over: unsafe extern "system" fn(*mut Target, u32, POINTL, *mut u32) -> HRESULT,
    drag_leave: unsafe extern "system" fn(*mut Target) -> HRESULT,
    drop: unsafe extern "system" fn(*mut Target, *mut c_void, u32, POINTL, *mut u32) -> HRESULT,
}

#[repr(C)]
struct Target {
    vtbl: *const Vtbl,
    refs: AtomicU32,
    handler: &'static dyn Handler,
}

static VTBL: Vtbl = Vtbl {
    query_interface,
    add_ref,
    release,
    drag_enter,
    drag_over,
    drag_leave,
    drop,
};

/// Create the COM object. The returned pointer is an `IDropTarget*`.
pub fn create(handler: &'static dyn Handler) -> *mut c_void {
    Box::into_raw(Box::new(Target {
        vtbl: &VTBL,
        refs: AtomicU32::new(1),
        handler,
    })) as *mut c_void
}

fn same(a: &GUID, b: &GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

unsafe extern "system" fn query_interface(
    this: *mut Target,
    iid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    if out.is_null() || iid.is_null() {
        return E_POINTER;
    }
    if same(&*iid, &IID_IUNKNOWN) || same(&*iid, &IID_IDROPTARGET) {
        add_ref(this);
        *out = this as *mut c_void;
        S_OK
    } else {
        *out = null_mut();
        E_NOINTERFACE
    }
}

unsafe extern "system" fn add_ref(this: *mut Target) -> u32 {
    (*this).refs.fetch_add(1, Ordering::SeqCst) + 1
}

unsafe extern "system" fn release(this: *mut Target) -> u32 {
    // Never reaches zero: the app holds the initial reference for its lifetime.
    let n = (*this).refs.fetch_sub(1, Ordering::SeqCst) - 1;
    n.max(1)
}

/// Answer with COPY (or LINK if the source doesn't offer COPY), never MOVE.
unsafe fn set_effect(effect: *mut u32, accept: bool) {
    if effect.is_null() {
        return;
    }
    let allowed = *effect;
    *effect = if !accept {
        DROPEFFECT_NONE
    } else if allowed & DROPEFFECT_COPY != 0 {
        DROPEFFECT_COPY
    } else if allowed & DROPEFFECT_LINK != 0 {
        DROPEFFECT_LINK
    } else {
        DROPEFFECT_NONE
    };
}

unsafe extern "system" fn drag_enter(
    this: *mut Target,
    data: *mut c_void,
    _keys: u32,
    pt: POINTL,
    effect: *mut u32,
) -> HRESULT {
    let files = files_from(data);
    let accepted = !files.is_empty() && (*this).handler.enter(files, pt.x, pt.y);
    let over = accepted && (*this).handler.over(pt.x, pt.y);
    set_effect(effect, over);
    S_OK
}

unsafe extern "system" fn drag_over(
    this: *mut Target,
    _keys: u32,
    pt: POINTL,
    effect: *mut u32,
) -> HRESULT {
    set_effect(effect, (*this).handler.over(pt.x, pt.y));
    S_OK
}

unsafe extern "system" fn drag_leave(this: *mut Target) -> HRESULT {
    (*this).handler.leave();
    S_OK
}

unsafe extern "system" fn drop(
    this: *mut Target,
    data: *mut c_void,
    _keys: u32,
    pt: POINTL,
    effect: *mut u32,
) -> HRESULT {
    let files = files_from(data);
    let started = !files.is_empty() && (*this).handler.drop(files, pt.x, pt.y);
    set_effect(effect, started);
    S_OK
}

/// Minimal `IDataObject` vtable prefix: we only call `GetData`.
#[repr(C)]
struct DataObjectVtbl {
    _query_interface: usize,
    _add_ref: usize,
    _release: usize,
    get_data: unsafe extern "system" fn(*mut c_void, *const FORMATETC, *mut STGMEDIUM) -> HRESULT,
}

/// Regular files (not folders) carried by a drag, in the source's order.
unsafe fn files_from(data: *mut c_void) -> Vec<PathBuf> {
    if data.is_null() {
        return Vec::new();
    }
    let vtbl = *(data as *const *const DataObjectVtbl);
    let format = FORMATETC {
        cfFormat: CF_HDROP,
        ptd: null_mut(),
        dwAspect: DVASPECT_CONTENT,
        lindex: -1,
        tymed: TYMED_HGLOBAL,
    };
    let mut medium: STGMEDIUM = std::mem::zeroed();
    if ((*vtbl).get_data)(data, &format, &mut medium) < 0 {
        return Vec::new();
    }
    let hdrop = medium.u.hGlobal;
    let mut files = Vec::new();
    let count = DragQueryFileW(hdrop, u32::MAX, null_mut(), 0);
    for i in 0..count.min(10_000) {
        let len = DragQueryFileW(hdrop, i, null_mut(), 0);
        let mut buf = vec![0u16; len as usize + 1];
        if DragQueryFileW(hdrop, i, buf.as_mut_ptr(), len + 1) > 0 {
            let p = path_from_wide(&buf);
            if p.is_file() {
                files.push(p);
            }
        }
    }
    ReleaseStgMedium(&mut medium);
    files
}
