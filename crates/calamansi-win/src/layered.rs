//! Per-pixel-alpha windows: shapes come from `calamansi_core::wheel::Canvas`,
//! text is drawn with GDI on top, then everything is premultiplied and handed
//! to `UpdateLayeredWindow`.

use crate::util::wide;
use calamansi_core::wheel::Canvas;
use std::ffi::c_void;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{HWND, POINT, RECT, SIZE};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::{UpdateLayeredWindow, ULW_ALPHA};

#[derive(Clone, Debug)]
pub struct Text {
    pub text: String,
    /// Box the text is laid out in, relative to the window's top-left.
    pub rect: (i32, i32, i32, i32),
    pub size_px: i32,
    pub bold: bool,
    pub color: [u8; 3],
    pub centered: bool,
}

pub enum Content<'a> {
    /// Visible pixels with text.
    Visible(&'a Canvas, &'a [Text]),
    /// Invisible but hit-testable (alpha 1) wherever the canvas has coverage.
    Probe(&'a Canvas),
}

/// Draw `content` into `hwnd` with its top-left at `pos` (screen pixels).
pub fn present(hwnd: HWND, content: Content, pos: (i32, i32)) -> bool {
    let canvas = match &content {
        Content::Visible(c, _) | Content::Probe(c) => *c,
    };
    let (w, h) = (canvas.width as i32, canvas.height as i32);
    unsafe {
        let screen = GetDC(null_mut());
        let mem = CreateCompatibleDC(screen);
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = w;
        info.bmiHeader.biHeight = -h; // top-down rows
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut c_void = null_mut();
        let bitmap = CreateDIBSection(mem, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        if bitmap.is_null() || bits.is_null() {
            DeleteDC(mem);
            ReleaseDC(null_mut(), screen);
            return false;
        }
        let old = SelectObject(mem, bitmap);
        let px = std::slice::from_raw_parts_mut(bits as *mut u8, (w * h * 4) as usize);

        match content {
            Content::Probe(c) => px.copy_from_slice(&c.probe()),
            Content::Visible(c, texts) => {
                // 1. Opaque colours, so GDI's anti-aliased text blends correctly.
                for (i, rgb) in c.rgb.iter().enumerate() {
                    px[i * 4..i * 4 + 4].copy_from_slice(&[rgb[2], rgb[1], rgb[0], 0]);
                }
                // 2. Text.
                SetBkMode(mem, TRANSPARENT as i32);
                for t in texts {
                    draw_text(mem, t);
                }
                GdiFlush();
                // 3. Read back, apply the canvas's alpha, premultiply.
                let rgb: Vec<[u8; 3]> = px.chunks_exact(4).map(|p| [p[2], p[1], p[0]]).collect();
                px.copy_from_slice(&c.to_premultiplied_bgra(&rgb));
            }
        }

        let dst = POINT { x: pos.0, y: pos.1 };
        let size = SIZE { cx: w, cy: h };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let ok =
            UpdateLayeredWindow(hwnd, screen, &dst, &size, mem, &src, 0, &blend, ULW_ALPHA) != 0;

        SelectObject(mem, old);
        DeleteObject(bitmap);
        DeleteDC(mem);
        ReleaseDC(null_mut(), screen);
        ok
    }
}

unsafe fn draw_text(dc: HDC, t: &Text) {
    let face = wide("Segoe UI");
    let font = CreateFontW(
        -t.size_px,
        0,
        0,
        0,
        if t.bold {
            FW_SEMIBOLD as i32
        } else {
            FW_NORMAL as i32
        },
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        OUT_DEFAULT_PRECIS as u32,
        CLIP_DEFAULT_PRECIS as u32,
        // Grayscale AA: ClearType's coloured fringes look wrong on alpha surfaces.
        ANTIALIASED_QUALITY as u32,
        DEFAULT_PITCH as u32,
        face.as_ptr(),
    );
    let old = SelectObject(dc, font);
    SetTextColor(dc, rgb(t.color));
    let (x, y, w, h) = t.rect;
    let mut rect = RECT {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    };
    let text: Vec<u16> = t.text.encode_utf16().collect();
    let align = if t.centered { DT_CENTER } else { DT_LEFT };
    DrawTextW(
        dc,
        text.as_ptr(),
        text.len() as i32,
        &mut rect,
        align | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
    );
    SelectObject(dc, old);
    DeleteObject(font);
}

pub fn rgb(c: [u8; 3]) -> u32 {
    u32::from(c[0]) | (u32::from(c[1]) << 8) | (u32::from(c[2]) << 16)
}
