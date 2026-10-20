//! The tray icon, drawn in code so the build needs no resource compiler.

use std::ffi::c_void;
use std::ptr::null_mut;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, HICON, ICONINFO};

/// Straight-alpha RGBA pixels of a calamansi slice, `size × size`.
pub fn pixels(size: u32) -> Vec<[u8; 4]> {
    let s = size as f32;
    let c = s / 2.0;
    let r = s / 2.0 - 0.5;
    let rind = [74u8, 124, 9];
    let flesh = [163u8, 230, 53];
    let pith = [236u8, 252, 203];
    let mut out = Vec::with_capacity((size * size) as usize);
    for y in 0..size {
        for x in 0..size {
            let (dx, dy) = (x as f32 + 0.5 - c, y as f32 + 0.5 - c);
            let d = (dx * dx + dy * dy).sqrt();
            let cover = (r - d + 0.5).clamp(0.0, 1.0);
            let color = if d > r * 0.80 {
                rind
            } else if d < r * 0.14 {
                pith
            } else {
                // Eight segments separated by thin pith lines.
                let a = dy.atan2(dx).rem_euclid(std::f32::consts::TAU / 8.0);
                let edge = a.min(std::f32::consts::TAU / 8.0 - a) * d;
                if edge < (s / 32.0).max(0.6) {
                    pith
                } else {
                    flesh
                }
            };
            out.push([color[0], color[1], color[2], (cover * 255.0) as u8]);
        }
    }
    out
}

/// Build an `HICON`. Returns null on failure (the tray then uses no icon).
pub fn create(size: u32) -> HICON {
    let px = pixels(size);
    unsafe {
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = size as i32;
        info.bmiHeader.biHeight = -(size as i32);
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        let screen = GetDC(null_mut());
        let mut bits: *mut c_void = null_mut();
        let color = CreateDIBSection(screen, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        ReleaseDC(null_mut(), screen);
        if color.is_null() || bits.is_null() {
            return null_mut();
        }
        let dst = std::slice::from_raw_parts_mut(bits as *mut u8, px.len() * 4);
        for (i, p) in px.iter().enumerate() {
            dst[i * 4..i * 4 + 4].copy_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
        // With a 32-bit colour bitmap the alpha channel decides transparency;
        // the mask just has to exist.
        let mask_bits = vec![0u8; (size.div_ceil(16) * 2 * size) as usize];
        let mask = CreateBitmap(
            size as i32,
            size as i32,
            1,
            1,
            mask_bits.as_ptr() as *const c_void,
        );
        let icon_info = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let icon = CreateIconIndirect(&icon_info);
        DeleteObject(color);
        DeleteObject(mask);
        icon
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn icon_is_round() {
        let p = super::pixels(32);
        assert_eq!(p[0][3], 0, "corner transparent");
        assert_eq!(p[16 * 32 + 16][3], 255, "centre opaque");
    }
}
