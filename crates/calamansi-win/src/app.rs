//! The tray app: windows, the wheel, the progress pill and jobs.
//!
//! All UI state lives on the UI thread in `APP`. Window procedures and the
//! drop target reach it through [`with`], which never blocks: if the state is
//! already borrowed (a message sent synchronously while we're mid-update),
//! the message falls through to default handling. Modal loops (menus, file
//! dialogs) are always entered with the state released.

use crate::droptarget::{self, Handler};
use crate::hook::{self, WM_APP_DRAG_END, WM_APP_DRAG_START};
use crate::layered::{self, Content, Text};
use crate::util::{self, file_name, unpack_point, wide, wide_path};
use crate::{icon, integration};
use calamansi_core::batch::{self, Action, Outcome, Report, Tool};
use calamansi_core::format::{self, Format};
use calamansi_core::media::Ffmpeg;
use calamansi_core::office::Office;
use calamansi_core::routes::Available;
use calamansi_core::settings::{OutputLocation, Settings};
use calamansi_core::wheel::{self, Canvas, Geometry, Hit, Item, Mode, Theme};
use std::cell::RefCell;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use windows_sys::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::UI::Controls::Dialogs::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, VK_ESCAPE};
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub const MAIN_CLASS: &str = "Calamansi.Main";
const WHEEL_CLASS: &str = "Calamansi.Wheel";
const PILL_CLASS: &str = "Calamansi.Pill";

pub const WM_APP_TRAY: u32 = WM_APP + 1;
const WM_APP_PROGRESS: u32 = WM_APP + 4;
const WM_APP_DONE: u32 = WM_APP + 5;
pub const WM_APP_PICK: u32 = WM_APP + 6;
const WM_APP_SELFTEST: u32 = WM_APP + 7;

/// `COPYDATASTRUCT::dwData` tags used by a second instance.
pub const COPY_ALREADY_RUNNING: usize = 0xCA1A_0000;
pub const COPY_PICK_CONVERT: usize = 0xCA1A_0001;
pub const COPY_PICK_TOOLS: usize = 0xCA1A_0002;

const TIMER_PILL: usize = 1;
const TIMER_WHEEL: usize = 2;

const ID_CONVERT: usize = 1;
const ID_TOOLS: usize = 2;
const ID_WHEEL: usize = 10;
const ID_LOGIN: usize = 11;
const ID_SENDTO: usize = 12;
const ID_OUT_BESIDE: usize = 20;
const ID_OUT_DOWNLOADS: usize = 21;
const ID_OUT_DESKTOP: usize = 22;
const ID_SETTINGS: usize = 30;
const ID_FFMPEG: usize = 31;
const ID_LOGS: usize = 32;
const ID_ABOUT: usize = 40;
const ID_SELFTEST: usize = 41;
const ID_OFFICE: usize = 33;
const ID_QUIT: usize = 99;

const TEXT: [u8; 3] = Theme::DARK.text;
const MUTED: [u8; 3] = [170, 170, 176];
const GREEN: [u8; 3] = [132, 204, 22];
const RED: [u8; 3] = [248, 113, 113];
const BLUE: [u8; 3] = [96, 165, 250];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WheelState {
    Hidden,
    /// Shift-drag detected; invisible target waiting to see if files arrive.
    Probe,
    /// Files are being dragged over the visible wheel.
    Dragging,
    /// Opened from the tray or "Send to": choose with a click.
    Picking,
}

struct WheelUi {
    hwnd: HWND,
    state: WheelState,
    mode: Mode,
    files: Vec<PathBuf>,
    items: Vec<Item>,
    geom: Geometry,
    origin: (i32, i32),
    hovered: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PillKind {
    Working,
    Success,
    Failure,
    Info,
}

struct PillUi {
    hwnd: HWND,
    visible: bool,
    kind: PillKind,
    title: String,
    detail: String,
    progress: Option<f32>,
    outputs: Vec<PathBuf>,
    origin: (i32, i32),
    scale: f32,
}

struct Job {
    action: Action,
    count: usize,
    cancel: Arc<AtomicBool>,
}

pub struct App {
    main: HWND,
    icon: HICON,
    taskbar_created: u32,
    settings_path: PathBuf,
    settings: Settings,
    ffmpeg: Option<Ffmpeg>,
    office: Option<Office>,
    wheel: WheelUi,
    pill: PillUi,
    job: Option<Job>,
    pending_pick: Option<(Vec<PathBuf>, Mode)>,
}

static MODAL_OPEN: AtomicBool = AtomicBool::new(false);

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Run `f` with the app state, unless it's unavailable or already borrowed.
fn with<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| match cell.try_borrow_mut() {
        Ok(mut guard) => guard.as_mut().map(f),
        Err(_) => None,
    })
}

fn available() -> bool {
    APP.with(|cell| cell.try_borrow_mut().is_ok())
}

// ---------------------------------------------------------------------------
// Startup and shutdown
// ---------------------------------------------------------------------------

unsafe fn register(class: &str, proc: WNDPROC) -> bool {
    let name = wide(class);
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: 0,
        lpfnWndProc: proc,
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance(),
        hIcon: null_mut(),
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        hbrBackground: null_mut(),
        lpszMenuName: null(),
        lpszClassName: name.as_ptr(),
        hIconSm: null_mut(),
    };
    RegisterClassExW(&wc) != 0
}

fn instance() -> HINSTANCE {
    unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(null()) }
}

unsafe fn create(class: &str, ex: WINDOW_EX_STYLE, style: WINDOW_STYLE) -> HWND {
    let name = wide(class);
    let title = wide("Calamansi");
    CreateWindowExW(
        ex,
        name.as_ptr(),
        title.as_ptr(),
        style,
        0,
        0,
        0,
        0,
        null_mut(),
        null_mut(),
        instance(),
        null(),
    )
}

const OVERLAY_EX: WINDOW_EX_STYLE =
    WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;

/// Create windows, tray icon, hook and drop target. Returns the main window.
pub fn start(first_run: bool, initial: Option<(Vec<PathBuf>, Mode)>) -> Result<HWND, String> {
    unsafe {
        if !(register(MAIN_CLASS, Some(main_proc))
            && register(WHEEL_CLASS, Some(wheel_proc))
            && register(PILL_CLASS, Some(pill_proc)))
        {
            return Err("couldn't register window classes".into());
        }
        let main = create(MAIN_CLASS, WS_EX_TOOLWINDOW, WS_OVERLAPPED);
        let wheel_hwnd = create(WHEEL_CLASS, OVERLAY_EX, WS_POPUP);
        let pill_hwnd = create(PILL_CLASS, OVERLAY_EX, WS_POPUP);
        if main.is_null() || wheel_hwnd.is_null() || pill_hwnd.is_null() {
            return Err("couldn't create windows".into());
        }

        let settings_path =
            Settings::default_path().unwrap_or_else(|| PathBuf::from("settings.json"));
        let settings = Settings::load(&settings_path);
        if first_run {
            let _ = settings.save(&settings_path);
        }
        hook::set_enabled(settings.drag_wheel);
        let ffmpeg = Ffmpeg::locate(settings.ffmpeg_path.as_deref());
        let office = Office::locate(settings.office_path.as_deref());
        util::log(&format!(
            "start {} | ffmpeg: {} | office: {}",
            env!("CARGO_PKG_VERSION"),
            ffmpeg
                .as_ref()
                .map(|f| f.path.display().to_string())
                .unwrap_or_else(|| "not found".into()),
            office.as_ref().map(Office::name).unwrap_or("not found"),
        ));
        let tb = wide("TaskbarCreated");

        let app = App {
            main,
            icon: icon::create(GetSystemMetrics(SM_CXSMICON).max(16) as u32),
            taskbar_created: RegisterWindowMessageW(tb.as_ptr()),
            settings_path,
            settings,
            ffmpeg,
            office,
            wheel: WheelUi {
                hwnd: wheel_hwnd,
                state: WheelState::Hidden,
                mode: Mode::Convert,
                files: Vec::new(),
                items: Vec::new(),
                geom: Geometry::for_scale(1.0),
                origin: (0, 0),
                hovered: None,
            },
            pill: PillUi {
                hwnd: pill_hwnd,
                visible: false,
                kind: PillKind::Info,
                title: String::new(),
                detail: String::new(),
                progress: None,
                outputs: Vec::new(),
                origin: (0, 0),
                scale: 1.0,
            },
            job: None,
            pending_pick: initial,
        };
        APP.with(|cell| *cell.borrow_mut() = Some(app));

        with(|a| a.add_tray_icon());

        static DROP_HANDLER: DropHandler = DropHandler;
        let target = droptarget::create(&DROP_HANDLER);
        let hr = windows_sys::Win32::System::Ole::RegisterDragDrop(wheel_hwnd, target);
        if hr < 0 {
            util::log(&format!("RegisterDragDrop failed: 0x{hr:08x}"));
        }
        if !hook::install(main) {
            util::log("mouse hook not installed; drag wheel unavailable");
        }

        if util::is_elevated() {
            util::log("running as administrator: Windows blocks drag-and-drop from Explorer");
            with(|a| {
                a.show_pill(
                    PillKind::Failure,
                    "Running as administrator",
                    "Windows blocks drops from Explorer. Start Calamansi normally.",
                    None,
                    Vec::new(),
                )
            });
        } else if first_run {
            with(|a| {
                a.show_pill(
                    PillKind::Info,
                    "Calamansi is in your system tray",
                    "Drag files with Shift held to convert them",
                    None,
                    Vec::new(),
                )
            });
        }
        PostMessageW(main, WM_APP_PICK, 0, 0);
        Ok(main)
    }
}

pub fn shutdown() {
    hook::uninstall();
    with(|a| unsafe {
        windows_sys::Win32::System::Ole::RevokeDragDrop(a.wheel.hwnd);
        if let Some(job) = &a.job {
            job.cancel.store(true, Ordering::SeqCst);
        }
        let nid = a.tray_data();
        Shell_NotifyIconW(NIM_DELETE, &nid);
        if !a.icon.is_null() {
            DestroyIcon(a.icon);
        }
    });
}

// ---------------------------------------------------------------------------
// Drop target → app
// ---------------------------------------------------------------------------

struct DropHandler;

impl Handler for DropHandler {
    fn enter(&self, files: Vec<PathBuf>, x: i32, y: i32) -> bool {
        with(|a| a.drag_enter(files, x, y)).unwrap_or(false)
    }
    fn over(&self, x: i32, y: i32) -> bool {
        with(|a| a.drag_over(x, y)).unwrap_or(false)
    }
    fn leave(&self) {
        with(|a| a.drag_leave());
    }
    fn drop(&self, files: Vec<PathBuf>, x: i32, y: i32) -> bool {
        with(|a| a.drag_drop(files, x, y)).unwrap_or(false)
    }
}

// ---------------------------------------------------------------------------
// The app
// ---------------------------------------------------------------------------

impl App {
    fn reload_settings(&mut self) {
        self.settings = Settings::load(&self.settings_path);
        hook::set_enabled(self.settings.drag_wheel);
    }

    /// Locating FFmpeg runs it twice, so only look again while it's missing
    /// (picks up a fresh `winget install ffmpeg` without a restart).
    fn refresh_ffmpeg_if_missing(&mut self) {
        let configured = self.settings.ffmpeg_path.clone();
        let stale = match (&self.ffmpeg, &configured) {
            (None, _) => true,
            (Some(f), Some(p)) => !f.path.starts_with(p) && f.path != *p,
            (Some(f), None) => !f.path.is_file(),
        };
        if stale {
            self.ffmpeg = Ffmpeg::locate(configured.as_deref());
        }
        if self.office.is_none() {
            self.office = Office::locate(self.settings.office_path.as_deref());
        }
    }

    fn available(&self) -> Available {
        Available {
            ffmpeg: self.ffmpeg.is_some(),
            office: self.office.is_some(),
        }
    }

    fn save_settings(&mut self) {
        if let Err(e) = self.settings.save(&self.settings_path) {
            util::log(&format!("saving settings failed: {e}"));
        }
        hook::set_enabled(self.settings.drag_wheel);
    }

    fn items_for(&self, files: &[PathBuf], mode: Mode) -> Vec<Item> {
        let formats: Option<Vec<Format>> = files.iter().map(|p| format::detect(p)).collect();
        match formats {
            Some(f) => wheel::items(mode, &f, self.available()),
            // Unrecognised files still get the generic file actions.
            None if mode == Mode::Tools && !files.is_empty() => {
                [Tool::Duplicate, Tool::CopyPath, Tool::Sha256]
                    .into_iter()
                    .map(|t| Item {
                        label: t.label().into(),
                        action: Action::Tool(t),
                    })
                    .collect()
            }
            None => Vec::new(),
        }
    }

    // ----- Wheel -----------------------------------------------------------

    fn place_wheel(&mut self, x: i32, y: i32) {
        let pt = POINT { x, y };
        self.wheel.geom = Geometry::for_scale(util::scale_at(pt));
        let size = self.wheel.geom.size as i32;
        let area = util::work_area_at(pt);
        let ox = (x - size / 2).clamp(area.left, (area.right - size).max(area.left));
        let oy = (y - size / 2).clamp(area.top, (area.bottom - size).max(area.top));
        self.wheel.origin = (ox, oy);
    }

    fn wheel_hit(&self, x: i32, y: i32) -> Hit {
        let (ox, oy) = self.wheel.origin;
        self.wheel
            .geom
            .hit((x - ox) as f32, (y - oy) as f32, self.wheel.items.len())
    }

    fn render_wheel(&self) {
        let w = &self.wheel;
        let canvas = w.geom.render(w.items.len(), w.hovered, &Theme::DARK);
        if w.state == WheelState::Probe {
            layered::present(w.hwnd, Content::Probe(&canvas), w.origin);
            return;
        }
        let s = w.geom.size as f32 / Geometry::BASE;
        let px = |v: f32| (v * s).round() as i32;
        let mut texts = Vec::new();
        let n = w.items.len();
        for (i, item) in w.items.iter().enumerate() {
            let (lx, ly) = w.geom.label_point(i, n);
            texts.push(Text {
                text: item.label.clone(),
                rect: (
                    lx as i32 - px(48.0),
                    ly as i32 - px(12.0),
                    px(96.0),
                    px(24.0),
                ),
                size_px: px(if n > 8 { 12.5 } else { 14.0 }),
                bold: true,
                color: TEXT,
                centered: true,
            });
        }
        let c = w.geom.size as i32 / 2;
        let count = w.files.len();
        let (title, sub) = if n == 0 {
            ("Nothing to do".to_string(), "for these files".to_string())
        } else {
            let files = if count == 1 {
                "1 file".to_string()
            } else {
                format!("{count} files")
            };
            let sub = match w.hovered {
                Some(i) => w.items[i].label.clone(),
                None if w.state == WheelState::Picking => "Click to choose".into(),
                None => match w.mode {
                    Mode::Convert => "Convert".into(),
                    Mode::Tools => "Tools".into(),
                },
            };
            (files, sub)
        };
        texts.push(Text {
            text: title,
            rect: (c - px(52.0), c - px(20.0), px(104.0), px(22.0)),
            size_px: px(15.0),
            bold: true,
            color: TEXT,
            centered: true,
        });
        texts.push(Text {
            text: sub,
            rect: (c - px(52.0), c + px(1.0), px(104.0), px(20.0)),
            size_px: px(12.0),
            bold: false,
            color: MUTED,
            centered: true,
        });
        layered::present(w.hwnd, Content::Visible(&canvas, &texts), w.origin);
    }

    fn hide_wheel(&mut self) {
        let was = self.wheel.state;
        self.wheel.state = WheelState::Hidden;
        self.wheel.files.clear();
        self.wheel.items.clear();
        self.wheel.hovered = None;
        unsafe {
            KillTimer(self.main, TIMER_WHEEL);
            ShowWindow(self.wheel.hwnd, SW_HIDE);
            if was == WheelState::Picking {
                ReleaseCapture();
                SetWindowLongPtrW(self.wheel.hwnd, GWL_EXSTYLE, OVERLAY_EX as isize);
            }
        }
    }

    fn on_drag_start(&mut self, tools: bool, x: i32, y: i32) {
        self.reload_settings();
        if !self.settings.drag_wheel || matches!(self.wheel.state, WheelState::Picking) {
            return;
        }
        let (dx, dy) = hook::down_point();
        if let Some(exe) = util::exe_at(POINT { x: dx, y: dy }) {
            let own = std::env::current_exe()
                .ok()
                .map(|p| file_name(&p))
                .unwrap_or_default();
            if self.settings.is_excluded(&exe) || exe.eq_ignore_ascii_case(&own) {
                return;
            }
        }
        util::log(&format!("drag start at {x},{y} (tools: {tools})"));
        unsafe { KillTimer(self.main, TIMER_WHEEL) };
        self.wheel.mode = if tools { Mode::Tools } else { Mode::Convert };
        self.wheel.state = WheelState::Probe;
        self.wheel.files.clear();
        self.wheel.items.clear();
        self.wheel.hovered = None;
        self.place_wheel(x, y);
        self.render_wheel();
        unsafe { ShowWindow(self.wheel.hwnd, SW_SHOWNOACTIVATE) };
    }

    fn on_drag_end(&mut self) {
        // The OLE Drop call may still be on its way; hide shortly after.
        if matches!(self.wheel.state, WheelState::Probe | WheelState::Dragging) {
            unsafe { SetTimer(self.main, TIMER_WHEEL, 250, None) };
        }
    }

    fn drag_enter(&mut self, files: Vec<PathBuf>, x: i32, y: i32) -> bool {
        if !matches!(self.wheel.state, WheelState::Probe | WheelState::Dragging) {
            return false;
        }
        self.wheel.items = self.items_for(&files, self.wheel.mode);
        util::log(&format!(
            "drag enter: {} file(s), {} option(s): {}",
            files.len(),
            self.wheel.items.len(),
            self.wheel
                .items
                .iter()
                .map(|i| i.label.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
        self.wheel.files = files;
        self.wheel.state = WheelState::Dragging;
        self.wheel.hovered = match self.wheel_hit(x, y) {
            Hit::Segment(i) => Some(i),
            _ => None,
        };
        self.render_wheel();
        true
    }

    fn drag_over(&mut self, x: i32, y: i32) -> bool {
        if self.wheel.state != WheelState::Dragging {
            return false;
        }
        let hovered = match self.wheel_hit(x, y) {
            Hit::Segment(i) => Some(i),
            _ => None,
        };
        if hovered != self.wheel.hovered {
            self.wheel.hovered = hovered;
            self.render_wheel();
        }
        hovered.is_some()
    }

    fn drag_leave(&mut self) {
        if self.wheel.state == WheelState::Dragging && self.wheel.hovered.is_some() {
            self.wheel.hovered = None;
            self.render_wheel();
        }
    }

    fn drag_drop(&mut self, files: Vec<PathBuf>, x: i32, y: i32) -> bool {
        if !matches!(self.wheel.state, WheelState::Probe | WheelState::Dragging) {
            return false;
        }
        // Use the dropped data itself, not state from DragEnter.
        let items = self.items_for(&files, self.wheel.mode);
        self.wheel.items = items.clone();
        let hit = self.wheel_hit(x, y);
        util::log(&format!(
            "drop: {} file(s) at {x},{y} -> {hit:?}",
            files.len()
        ));
        self.hide_wheel();
        match hit {
            Hit::Segment(i) if i < items.len() => {
                self.start_job(items[i].action, files);
                true
            }
            _ => false,
        }
    }

    /// Show the wheel at the cursor for choosing with a click.
    fn pick(&mut self, files: Vec<PathBuf>, mode: Mode) {
        if files.is_empty() {
            return;
        }
        self.reload_settings();
        self.refresh_ffmpeg_if_missing();
        if self.wheel.state != WheelState::Hidden {
            self.hide_wheel();
        }
        let mut pt = POINT { x: 0, y: 0 };
        unsafe { GetCursorPos(&mut pt) };
        self.wheel.mode = mode;
        self.wheel.items = self.items_for(&files, mode);
        self.wheel.files = files;
        self.wheel.state = WheelState::Picking;
        self.place_wheel(pt.x, pt.y);
        self.wheel.hovered = match self.wheel_hit(pt.x, pt.y) {
            Hit::Segment(i) => Some(i),
            _ => None,
        };
        self.render_wheel();
        let h = self.wheel.hwnd;
        unsafe {
            // Picking needs keyboard focus (Esc) and mouse capture (click outside cancels).
            SetWindowLongPtrW(h, GWL_EXSTYLE, (OVERLAY_EX & !WS_EX_NOACTIVATE) as isize);
            ShowWindow(h, SW_SHOW);
            SetForegroundWindow(h);
            SetCapture(h);
        }
    }

    fn pick_mouse_move(&mut self, x: i32, y: i32) {
        if self.wheel.state != WheelState::Picking {
            return;
        }
        let (ox, oy) = self.wheel.origin;
        let hovered = match self.wheel_hit(x + ox, y + oy) {
            Hit::Segment(i) => Some(i),
            _ => None,
        };
        if hovered != self.wheel.hovered {
            self.wheel.hovered = hovered;
            self.render_wheel();
        }
    }

    fn pick_click(&mut self, x: i32, y: i32) {
        if self.wheel.state != WheelState::Picking {
            return;
        }
        let (ox, oy) = self.wheel.origin;
        let hit = self.wheel_hit(x + ox, y + oy);
        let files = std::mem::take(&mut self.wheel.files);
        let items = std::mem::take(&mut self.wheel.items);
        self.hide_wheel();
        if let Hit::Segment(i) = hit {
            if let Some(item) = items.get(i) {
                self.start_job(item.action, files);
            }
        }
    }

    fn pick_cancel(&mut self) {
        if self.wheel.state == WheelState::Picking {
            self.hide_wheel();
        }
    }

    // ----- Jobs ------------------------------------------------------------

    fn start_job(&mut self, action: Action, files: Vec<PathBuf>) {
        if self.job.is_some() {
            self.show_pill(
                PillKind::Info,
                "Still working on the last job",
                "Click its progress to cancel it",
                None,
                Vec::new(),
            );
            return;
        }
        self.reload_settings();
        let cancel = Arc::new(AtomicBool::new(false));
        self.job = Some(Job {
            action,
            count: files.len(),
            cancel: cancel.clone(),
        });
        self.show_pill(
            PillKind::Working,
            &working_title(action, 0, files.len()),
            "Click to cancel",
            Some(0.0),
            Vec::new(),
        );

        let main = self.main as isize;
        let settings = self.settings.clone();
        std::thread::spawn(move || {
            let ffmpeg = Ffmpeg::locate(settings.ffmpeg_path.as_deref());
            let office = Office::locate(settings.office_path.as_deref());
            let cx = batch::Context {
                settings: &settings,
                ffmpeg: ffmpeg.as_ref(),
                office: office.as_ref(),
                cancel: &cancel,
            };
            let run = std::panic::AssertUnwindSafe(|| {
                batch::run(action, &files, &cx, &|p| unsafe {
                    PostMessageW(main as HWND, WM_APP_PROGRESS, p.done, p.total as isize);
                })
            });
            // A bug in a converter must not leave the UI stuck on "working".
            let report = std::panic::catch_unwind(run).unwrap_or_else(|_| Report {
                results: files
                    .iter()
                    .map(|f| batch::FileResult {
                        input: f.clone(),
                        outcome: Outcome::Failed("internal error (see log)".into()),
                    })
                    .collect(),
            });
            let boxed = Box::into_raw(Box::new(report));
            unsafe {
                if PostMessageW(main as HWND, WM_APP_DONE, 0, boxed as isize) == 0 {
                    drop(Box::from_raw(boxed));
                }
            }
        });
    }

    fn on_progress(&mut self, done: usize, total: usize) {
        let Some(job) = &self.job else { return };
        if job.cancel.load(Ordering::Relaxed) {
            return;
        }
        let title = working_title(job.action, done, total);
        let progress = if total == 0 {
            0.0
        } else {
            done as f32 / total as f32
        };
        self.pill.title = title;
        self.pill.progress = Some(progress);
        self.render_pill();
    }

    fn on_done(&mut self, report: Report) {
        let Some(job) = self.job.take() else { return };
        util::log(&format!(
            "job {:?}: {} ok, {} failed, {} output(s)",
            job.action,
            report.count(|o| matches!(o, Outcome::Done(_) | Outcome::Value(_))),
            report.failures().len(),
            report.outputs().len()
        ));
        for (path, why) in report.failures() {
            util::log(&format!("  failed {}: {why}", path.display()));
        }
        let outputs: Vec<PathBuf> = report.outputs().into_iter().map(PathBuf::from).collect();
        let failures = report.failures();
        let ok = report.count(|o| matches!(o, Outcome::Done(_) | Outcome::Value(_)));

        let is_value = matches!(job.action, Action::Tool(t) if t.is_value());
        if is_value && !report.values().is_empty() {
            let sha = job.action == Action::Tool(Tool::Sha256);
            let text: String = report
                .values()
                .iter()
                .map(|(p, v)| {
                    if sha {
                        format!("{v}  {}\r\n", file_name(p))
                    } else {
                        format!("{v}\r\n")
                    }
                })
                .collect();
            let copied = util::copy_to_clipboard(self.main, text.trim_end());
            let n = report.values().len();
            let what = if sha { "checksum" } else { "path" };
            let title = match (copied, n) {
                (false, _) => "Couldn't use the clipboard".to_string(),
                (true, 1) => format!("{}{} copied", what[..1].to_uppercase(), &what[1..]),
                (true, n) => format!("{n} {what}s copied"),
            };
            let detail = report
                .values()
                .first()
                .map(|(_, v)| {
                    if sha {
                        format!("SHA-256 {}…", &v[..16])
                    } else {
                        v.to_string()
                    }
                })
                .unwrap_or_default();
            return self.show_pill(
                if copied {
                    PillKind::Success
                } else {
                    PillKind::Failure
                },
                &title,
                &detail,
                None,
                Vec::new(),
            );
        }

        if report.cancelled() {
            let detail = if outputs.is_empty() {
                String::new()
            } else {
                "Finished files were kept".into()
            };
            return self.show_pill(PillKind::Info, "Cancelled", &detail, None, outputs);
        }

        if failures.is_empty() {
            if outputs.is_empty() {
                let why = report
                    .results
                    .iter()
                    .find_map(|r| match &r.outcome {
                        Outcome::Skipped(w) => Some(w.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();
                return self.show_pill(PillKind::Info, "Nothing to do", &why, None, Vec::new());
            }
            let title = match job.action {
                Action::Convert(f) if job.count == 1 => format!("Converted to {f}"),
                Action::Convert(f) => format!("Converted {ok} files to {f}"),
                Action::Tool(Tool::SplitPdf) => format!("Split into {} files", outputs.len()),
                Action::Tool(_) if outputs.len() == 1 => {
                    format!("Created {}", file_name(&outputs[0]))
                }
                Action::Tool(t) => format!("{}: {} files", t.label(), outputs.len()),
            };
            return self.show_pill(
                PillKind::Success,
                &title,
                "Click to show in File Explorer",
                None,
                outputs,
            );
        }

        let (first, why) = failures[0];
        let more = if failures.len() > 1 {
            format!(" (+{} more)", failures.len() - 1)
        } else {
            String::new()
        };
        let title = if ok == 0 {
            "Couldn't finish".to_string()
        } else {
            format!("{ok} of {} done", report.results.len())
        };
        let detail = format!("{}: {why}{more}", file_name(first));
        self.show_pill(PillKind::Failure, &title, &detail, None, outputs);
    }

    // ----- Self-test ------------------------------------------------------

    fn start_self_test(&mut self) {
        if self.job.is_some() {
            return;
        }
        self.reload_settings();
        let cancel = Arc::new(AtomicBool::new(false));
        self.job = Some(Job {
            action: Action::Tool(Tool::Sha256),
            count: 0,
            cancel: cancel.clone(),
        });
        self.show_pill(
            PillKind::Working,
            "Running self-test…",
            "Converting sample files",
            None,
            Vec::new(),
        );
        let main = self.main as isize;
        let mut settings = self.settings.clone();
        settings.output = OutputLocation::BesideSource;
        std::thread::spawn(move || {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let dir = std::env::temp_dir()
                .join("Calamansi self-test")
                .join(stamp.to_string());
            let ffmpeg = Ffmpeg::locate(settings.ffmpeg_path.as_deref());
            let office = Office::locate(settings.office_path.as_deref());
            let cx = batch::Context {
                settings: &settings,
                ffmpeg: ffmpeg.as_ref(),
                office: office.as_ref(),
                cancel: &cancel,
            };
            let checks = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                calamansi_core::selftest::run(&dir, &cx)
            }))
            .unwrap_or_default();
            let boxed = Box::into_raw(Box::new((dir, checks)));
            unsafe {
                if PostMessageW(main as HWND, WM_APP_SELFTEST, 0, boxed as isize) == 0 {
                    drop(Box::from_raw(boxed));
                }
            }
        });
    }

    fn on_self_test(&mut self, dir: PathBuf, checks: Vec<calamansi_core::selftest::Check>) {
        self.job = None;
        util::log(&format!("self-test in {}", dir.display()));
        for c in &checks {
            match &c.result {
                Ok(_) => util::log(&format!("  ok   {}", c.name)),
                Err(e) => util::log(&format!("  FAIL {}: {e}", c.name)),
            }
        }
        let failed: Vec<&calamansi_core::selftest::Check> =
            checks.iter().filter(|c| !c.passed()).collect();
        let first_output = checks
            .iter()
            .find_map(|c| c.result.as_ref().ok().and_then(|v| v.first().cloned()));
        let outputs = first_output.into_iter().collect();
        if checks.is_empty() {
            self.show_pill(
                PillKind::Failure,
                "Self-test crashed",
                "See the log for details",
                None,
                Vec::new(),
            );
        } else if failed.is_empty() {
            let title = format!("All {} checks passed", checks.len());
            self.show_pill(
                PillKind::Success,
                &title,
                "Click to see the test files",
                None,
                outputs,
            );
        } else {
            let title = format!("{} of {} checks failed", failed.len(), checks.len());
            let detail = match &failed[0].result {
                Err(e) => format!("{}: {e}", failed[0].name),
                Ok(_) => String::new(),
            };
            self.show_pill(PillKind::Failure, &title, &detail, None, outputs);
        }
    }

    // ----- Progress pill -----------------------------------------------------

    fn show_pill(
        &mut self,
        kind: PillKind,
        title: &str,
        detail: &str,
        progress: Option<f32>,
        outputs: Vec<PathBuf>,
    ) {
        let p = &mut self.pill;
        if !p.visible {
            let mut pt = POINT { x: 0, y: 0 };
            unsafe { GetCursorPos(&mut pt) };
            p.scale = util::scale_at(pt);
            let area = util::work_area_at(pt);
            let w = (400.0 * p.scale) as i32;
            p.origin = (
                (area.left + area.right - w) / 2,
                area.top + (12.0 * p.scale) as i32,
            );
        }
        p.kind = kind;
        p.title = title.to_string();
        p.detail = detail.to_string();
        p.progress = progress;
        p.outputs = outputs;
        p.visible = true;
        self.render_pill();
        unsafe {
            ShowWindow(self.pill.hwnd, SW_SHOWNOACTIVATE);
            KillTimer(self.main, TIMER_PILL);
            let ms = match kind {
                PillKind::Success => self.settings.success_dismiss_ms.max(500),
                PillKind::Info => 4000,
                // Work in progress and failures stay until clicked.
                PillKind::Working | PillKind::Failure => 0,
            };
            if ms > 0 {
                SetTimer(self.main, TIMER_PILL, ms, None);
            }
        }
    }

    fn render_pill(&self) {
        let p = &self.pill;
        if !p.visible {
            return;
        }
        let s = p.scale;
        let px = |v: f32| (v * s).round() as i32;
        let (w, h) = (px(400.0) as usize, px(60.0) as usize);
        let mut canvas =
            Canvas::rounded_rect(w, h, h as f32 / 2.0, Theme::DARK.ring, Theme::DARK.alpha);

        // Status dot.
        let accent = match p.kind {
            PillKind::Working => BLUE,
            PillKind::Success => GREEN,
            PillKind::Failure => RED,
            PillKind::Info => Theme::DARK.highlight,
        };
        let (cx, cy, r) = (h as f32 / 2.0, h as f32 / 2.0, 11.0 * s);
        for y in 0..h {
            for x in 0..w.min(h) {
                let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
                if dx * dx + dy * dy <= r * r {
                    canvas.rgb[y * w + x] = accent;
                }
            }
        }
        // Progress bar along the bottom edge.
        if let Some(progress) = p.progress {
            let (x0, x1) = (h / 2, w - h / 2);
            let filled = x0 + ((x1 - x0) as f32 * progress.clamp(0.0, 1.0)) as usize;
            let (y0, y1) = (h - px(7.0) as usize, h - px(4.0) as usize);
            canvas.fill_rect(x0, y0, x1, y1, [60, 60, 66]);
            canvas.fill_rect(x0, y0, filled, y1, BLUE);
        }

        let glyph = match p.kind {
            PillKind::Working => "",
            PillKind::Success => "\u{2713}",
            PillKind::Failure => "!",
            PillKind::Info => "i",
        };
        let left = h as i32;
        let texts = [
            Text {
                text: glyph.into(),
                rect: (
                    cx as i32 - px(11.0),
                    cy as i32 - px(11.0),
                    px(22.0),
                    px(22.0),
                ),
                size_px: px(14.0),
                bold: true,
                color: [20, 20, 22],
                centered: true,
            },
            Text {
                text: p.title.clone(),
                rect: (left, px(9.0), w as i32 - left - px(22.0), px(22.0)),
                size_px: px(15.0),
                bold: true,
                color: TEXT,
                centered: false,
            },
            Text {
                text: p.detail.clone(),
                rect: (left, px(30.0), w as i32 - left - px(22.0), px(20.0)),
                size_px: px(12.5),
                bold: false,
                color: MUTED,
                centered: false,
            },
        ];
        layered::present(p.hwnd, Content::Visible(&canvas, &texts), p.origin);
    }

    fn hide_pill(&mut self) {
        self.pill.visible = false;
        unsafe {
            KillTimer(self.main, TIMER_PILL);
            ShowWindow(self.pill.hwnd, SW_HIDE);
        }
    }

    fn pill_clicked(&mut self) {
        match self.pill.kind {
            PillKind::Working => {
                if let Some(job) = &self.job {
                    job.cancel.store(true, Ordering::SeqCst);
                    self.pill.title = "Cancelling…".into();
                    self.pill.detail = String::new();
                    self.render_pill();
                }
            }
            PillKind::Success | PillKind::Failure | PillKind::Info => {
                if let Some(first) = self.pill.outputs.first() {
                    util::reveal(first);
                }
                self.hide_pill();
            }
        }
    }

    // ----- Tray ---------------------------------------------------------------

    fn tray_data(&self) -> NOTIFYICONDATAW {
        let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = self.main;
        nid.uID = 1;
        nid
    }

    fn add_tray_icon(&self) {
        let mut nid = self.tray_data();
        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        nid.uCallbackMessage = WM_APP_TRAY;
        nid.hIcon = self.icon;
        let tip: Vec<u16> = "Calamansi — Shift-drag files to convert"
            .encode_utf16()
            .collect();
        let n = tip.len().min(nid.szTip.len() - 1);
        nid.szTip[..n].copy_from_slice(&tip[..n]);
        unsafe { Shell_NotifyIconW(NIM_ADD, &nid) };
    }

    fn menu_state(&mut self) -> MenuState {
        self.reload_settings();
        self.refresh_ffmpeg_if_missing();
        MenuState {
            wheel: self.settings.drag_wheel,
            login: integration::starts_at_login(),
            send_to: integration::in_send_to(),
            output: self.settings.output.clone(),
            ffmpeg: self.ffmpeg.as_ref().map(|f| f.version.clone()),
            office: self.office.as_ref().map(|o| o.name()),
            busy: self.job.is_some(),
        }
    }

    fn on_menu(&mut self, id: usize) {
        match id {
            ID_WHEEL => {
                self.settings.drag_wheel = !self.settings.drag_wheel;
                self.save_settings();
            }
            ID_LOGIN => {
                let on = !integration::starts_at_login();
                if !integration::set_starts_at_login(on) {
                    self.show_pill(
                        PillKind::Failure,
                        "Couldn't change startup setting",
                        "See the log for details",
                        None,
                        Vec::new(),
                    );
                }
            }
            ID_SENDTO => {
                let on = !integration::in_send_to();
                if integration::set_in_send_to(on) {
                    let msg = if on {
                        "Added to the “Send to” menu"
                    } else {
                        "Removed from “Send to”"
                    };
                    let detail = if on {
                        "Right-click files → Send to → Calamansi"
                    } else {
                        ""
                    };
                    self.show_pill(PillKind::Success, msg, detail, None, Vec::new());
                } else {
                    self.show_pill(
                        PillKind::Failure,
                        "Couldn't update “Send to”",
                        "PowerShell may be blocked on this PC",
                        None,
                        Vec::new(),
                    );
                }
            }
            ID_OUT_BESIDE | ID_OUT_DOWNLOADS | ID_OUT_DESKTOP => {
                self.settings.output = match id {
                    ID_OUT_DOWNLOADS => OutputLocation::Downloads,
                    ID_OUT_DESKTOP => OutputLocation::Desktop,
                    _ => OutputLocation::BesideSource,
                };
                self.save_settings();
            }
            ID_SETTINGS => {
                if !self.settings_path.exists() {
                    let _ = self.settings.save(&self.settings_path);
                }
                open_in_notepad(&self.settings_path);
            }
            ID_FFMPEG => match &self.ffmpeg {
                Some(f) => util::reveal(&f.path),
                None => self.show_pill(
                    PillKind::Info,
                    "FFmpeg adds audio & video",
                    "Run  winget install ffmpeg  then reopen this menu",
                    None,
                    Vec::new(),
                ),
            },
            ID_OFFICE => {
                if self.office.is_none() {
                    self.show_pill(
                        PillKind::Info,
                        "Office adds Word, Excel & PowerPoint",
                        "Install Microsoft Office or LibreOffice, then reopen this menu",
                        None,
                        Vec::new(),
                    );
                }
            }
            ID_SELFTEST => self.start_self_test(),
            ID_LOGS => {
                if let Some(d) = dirs::data_local_dir().map(|d| d.join("Calamansi")) {
                    let _ = std::fs::create_dir_all(&d);
                    util::open(&d);
                }
            }
            ID_ABOUT => self.show_pill(
                PillKind::Info,
                &format!("Calamansi {}", env!("CARGO_PKG_VERSION")),
                "Free & open source (GPL-3.0). Files never leave your PC.",
                None,
                Vec::new(),
            ),
            ID_QUIT => unsafe { PostQuitMessage(0) },
            _ => {}
        }
    }
}

fn working_title(action: Action, done: usize, total: usize) -> String {
    let verb = match action {
        Action::Convert(f) => format!("Converting to {f}"),
        Action::Tool(t) => t.label().to_string(),
    };
    if total > 1 {
        format!("{verb} · {} of {total}", (done + 1).min(total))
    } else {
        format!("{verb}…")
    }
}

fn open_in_notepad(path: &std::path::Path) {
    let verb = wide("open");
    let exe = wide("notepad.exe");
    let mut quoted = wide_path(path);
    quoted.pop();
    let mut args: Vec<u16> = "\"".encode_utf16().collect();
    args.extend(quoted);
    args.extend("\"".encode_utf16());
    args.push(0);
    unsafe {
        windows_sys::Win32::UI::Shell::ShellExecuteW(
            null_mut(),
            verb.as_ptr(),
            exe.as_ptr(),
            args.as_ptr(),
            null(),
            SW_SHOWNORMAL,
        );
    }
}

struct MenuState {
    wheel: bool,
    login: bool,
    send_to: bool,
    output: OutputLocation,
    ffmpeg: Option<String>,
    office: Option<&'static str>,
    busy: bool,
}

/// Build and show the tray menu with the app state *released*: the menu runs
/// a modal loop, during which progress messages and drops keep arriving.
fn show_tray_menu(main: HWND) {
    let Some(state) = with(|a| a.menu_state()) else {
        return;
    };
    unsafe {
        let menu = CreatePopupMenu();
        let out = CreatePopupMenu();
        let add = |m: HMENU, flags: MENU_ITEM_FLAGS, id: usize, text: &str| {
            let t = wide(text);
            AppendMenuW(m, flags, id, t.as_ptr());
        };
        let check = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
        let busy = if state.busy { MF_GRAYED } else { MF_ENABLED };
        add(menu, MF_STRING | busy, ID_CONVERT, "Convert files…");
        add(menu, MF_STRING | busy, ID_TOOLS, "Tools for files…");
        add(menu, MF_SEPARATOR, 0, "");
        add(
            menu,
            MF_STRING | check(state.wheel),
            ID_WHEEL,
            "Shift-drag shows the wheel",
        );
        add(
            menu,
            MF_STRING | check(state.send_to),
            ID_SENDTO,
            "Show in “Send to” menu",
        );
        add(
            menu,
            MF_STRING | check(state.login),
            ID_LOGIN,
            "Start with Windows",
        );
        add(
            out,
            MF_STRING | check(state.output == OutputLocation::BesideSource),
            ID_OUT_BESIDE,
            "Next to the original",
        );
        add(
            out,
            MF_STRING | check(state.output == OutputLocation::Downloads),
            ID_OUT_DOWNLOADS,
            "Downloads",
        );
        add(
            out,
            MF_STRING | check(state.output == OutputLocation::Desktop),
            ID_OUT_DESKTOP,
            "Desktop",
        );
        let label = wide("Save converted files to");
        AppendMenuW(menu, MF_POPUP, out as usize, label.as_ptr());
        add(menu, MF_SEPARATOR, 0, "");
        match &state.ffmpeg {
            Some(v) => {
                let short = v.split(" Copyright").next().unwrap_or(v);
                add(
                    menu,
                    MF_STRING,
                    ID_FFMPEG,
                    &format!("FFmpeg: {}", short.trim_start_matches("ffmpeg ")),
                );
            }
            None => add(
                menu,
                MF_STRING,
                ID_FFMPEG,
                "FFmpeg not found — how to add audio & video",
            ),
        }
        match state.office {
            Some(name) => add(
                menu,
                MF_STRING | MF_GRAYED,
                ID_OFFICE,
                &format!("Office: {name}"),
            ),
            None => add(
                menu,
                MF_STRING,
                ID_OFFICE,
                "Office not found — how to add Word/Excel",
            ),
        }
        add(menu, MF_STRING | busy, ID_SELFTEST, "Run self-test");
        add(menu, MF_STRING, ID_SETTINGS, "Edit settings file…");
        add(menu, MF_STRING, ID_LOGS, "Open log folder");
        add(menu, MF_STRING, ID_ABOUT, "About Calamansi");
        add(menu, MF_SEPARATOR, 0, "");
        add(menu, MF_STRING, ID_QUIT, "Quit");

        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        // Required so the menu closes when the user clicks elsewhere.
        SetForegroundWindow(main);
        let id = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
            pt.x,
            pt.y,
            0,
            main,
            null(),
        ) as usize;
        PostMessageW(main, WM_NULL, 0, 0);
        DestroyMenu(menu); // also destroys the submenu

        match id {
            ID_CONVERT => choose_files_then_pick(main, Mode::Convert),
            ID_TOOLS => choose_files_then_pick(main, Mode::Tools),
            0 => {}
            other => {
                with(|a| a.on_menu(other));
            }
        }
    }
}

/// Standard multi-select open dialog. Modal, so called with state released.
fn choose_files_then_pick(owner: HWND, mode: Mode) {
    let mut buf = vec![0u16; 65_536];
    let title = wide(match mode {
        Mode::Convert => "Choose files to convert",
        Mode::Tools => "Choose files",
    });
    let mut ofn: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    ofn.hwndOwner = owner;
    ofn.lpstrFile = buf.as_mut_ptr();
    ofn.nMaxFile = buf.len() as u32;
    ofn.lpstrTitle = title.as_ptr();
    ofn.Flags = OFN_ALLOWMULTISELECT
        | OFN_EXPLORER
        | OFN_FILEMUSTEXIST
        | OFN_PATHMUSTEXIST
        | OFN_NOCHANGEDIR;
    if unsafe { GetOpenFileNameW(&mut ofn) } == 0 {
        return;
    }
    let files = parse_multi_select(&buf);
    with(|a| a.pick(files, mode));
}

/// `dir\0name1\0name2\0\0`, or `fullpath\0\0` for a single file.
fn parse_multi_select(buf: &[u16]) -> Vec<PathBuf> {
    let parts: Vec<&[u16]> = buf
        .split(|&c| c == 0)
        .take_while(|s| !s.is_empty())
        .collect();
    match parts.as_slice() {
        [] => Vec::new(),
        [single] => vec![util::path_from_wide(single)],
        [dir, names @ ..] => {
            let dir = util::path_from_wide(dir);
            names
                .iter()
                .map(|n| dir.join(util::path_from_wide(n)))
                .collect()
        }
    }
}

// ---------------------------------------------------------------------------
// Window procedures
// ---------------------------------------------------------------------------

fn lparam_xy(l: LPARAM) -> (i32, i32) {
    (
        (l & 0xFFFF) as u16 as i16 as i32,
        ((l >> 16) & 0xFFFF) as u16 as i16 as i32,
    )
}

unsafe extern "system" fn main_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_APP_TRAY => {
            match l as u32 {
                // Menus and dialogs are modal; ignore clicks while one is open.
                WM_LBUTTONUP if !MODAL_OPEN.swap(true, Ordering::SeqCst) => {
                    choose_files_then_pick(hwnd, Mode::Convert);
                    MODAL_OPEN.store(false, Ordering::SeqCst);
                }
                WM_RBUTTONUP | WM_CONTEXTMENU if !MODAL_OPEN.swap(true, Ordering::SeqCst) => {
                    show_tray_menu(hwnd);
                    MODAL_OPEN.store(false, Ordering::SeqCst);
                }
                _ => {}
            }
            0
        }
        WM_APP_DRAG_START => {
            let (x, y) = unpack_point(l);
            with(|a| a.on_drag_start(w != 0, x, y));
            0
        }
        WM_APP_DRAG_END => {
            with(|a| a.on_drag_end());
            0
        }
        WM_APP_PROGRESS => {
            with(|a| a.on_progress(w, l as usize));
            0
        }
        WM_APP_DONE => {
            if !available() {
                // State is busy (shouldn't happen outside modal loops); try again shortly.
                PostMessageW(hwnd, WM_APP_DONE, w, l);
                return 0;
            }
            let report = *Box::from_raw(l as *mut Report);
            with(move |a| a.on_done(report));
            0
        }
        WM_APP_SELFTEST => {
            if !available() {
                PostMessageW(hwnd, WM_APP_SELFTEST, w, l);
                return 0;
            }
            let (dir, checks) =
                *Box::from_raw(l as *mut (PathBuf, Vec<calamansi_core::selftest::Check>));
            with(move |a| a.on_self_test(dir, checks));
            0
        }
        WM_APP_PICK => {
            if let Some(Some((files, mode))) = with(|a| a.pending_pick.take()) {
                with(|a| a.pick(files, mode));
            }
            0
        }
        WM_COPYDATA => {
            let cds = &*(l as *const COPYDATASTRUCT);
            let mode = match cds.dwData {
                COPY_PICK_CONVERT => Some(Mode::Convert),
                COPY_PICK_TOOLS => Some(Mode::Tools),
                _ => None,
            };
            let files: Vec<PathBuf> = if cds.lpData.is_null() || cds.cbData < 2 {
                Vec::new()
            } else {
                let data =
                    std::slice::from_raw_parts(cds.lpData as *const u16, cds.cbData as usize / 2);
                data.split(|&c| c == 0)
                    .filter(|s| !s.is_empty())
                    .map(util::path_from_wide)
                    .collect()
            };
            match (mode, files.is_empty()) {
                (Some(mode), false) => {
                    with(|a| a.pending_pick = Some((files, mode)));
                    PostMessageW(hwnd, WM_APP_PICK, 0, 0);
                }
                _ if cds.dwData == COPY_ALREADY_RUNNING => {
                    with(|a| {
                        a.show_pill(
                            PillKind::Info,
                            "Calamansi is already running",
                            "Find it in the system tray",
                            None,
                            Vec::new(),
                        )
                    });
                }
                _ => {}
            }
            1
        }
        WM_TIMER => {
            match w {
                TIMER_PILL => {
                    with(|a| a.hide_pill());
                }
                TIMER_WHEEL => {
                    with(|a| {
                        if matches!(a.wheel.state, WheelState::Probe | WheelState::Dragging) {
                            a.hide_wheel();
                        }
                    });
                }
                _ => {}
            }
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => {
            // Explorer restarted: the tray icon must be added again.
            if with(|a| msg != 0 && msg == a.taskbar_created).unwrap_or(false) {
                with(|a| a.add_tray_icon());
                return 0;
            }
            DefWindowProcW(hwnd, msg, w, l)
        }
    }
}

unsafe extern "system" fn wheel_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEMOVE => {
            let (x, y) = lparam_xy(l);
            with(|a| a.pick_mouse_move(x, y));
            0
        }
        WM_LBUTTONUP => {
            let (x, y) = lparam_xy(l);
            with(|a| a.pick_click(x, y));
            0
        }
        WM_RBUTTONUP => {
            with(|a| a.pick_cancel());
            0
        }
        WM_KEYDOWN if w == VK_ESCAPE as usize => {
            with(|a| a.pick_cancel());
            0
        }
        WM_ACTIVATE if (w & 0xFFFF) as u32 == WA_INACTIVE => {
            with(|a| a.pick_cancel());
            0
        }
        WM_CAPTURECHANGED if l as HWND != hwnd => {
            with(|a| a.pick_cancel());
            0
        }
        _ => DefWindowProcW(hwnd, msg, w, l),
    }
}

unsafe extern "system" fn pill_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_LBUTTONUP => {
            with(|a| a.pill_clicked());
            0
        }
        WM_RBUTTONUP => {
            with(|a| a.hide_pill());
            0
        }
        _ => DefWindowProcW(hwnd, msg, w, l),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_select_parsing() {
        let enc = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        let mut one = enc("C:\\a\\b.png");
        one.extend([0, 0]);
        assert_eq!(
            parse_multi_select(&one),
            vec![PathBuf::from("C:\\a\\b.png")]
        );
        let mut many = enc("C:\\pics\0x.png\0y.jpg\0\0");
        many.push(0);
        assert_eq!(parse_multi_select(&many).len(), 2);
    }

    #[test]
    fn titles() {
        assert_eq!(
            working_title(Action::Convert(Format::Jpeg), 0, 1),
            "Converting to JPEG…"
        );
        assert_eq!(
            working_title(Action::Convert(Format::Jpeg), 1, 4),
            "Converting to JPEG · 2 of 4"
        );
    }
}
