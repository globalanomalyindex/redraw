//! Reads a small square of screen pixels for auto-contrast. The caller turns
//! it into one average color and drops it; nothing is kept or sent anywhere.

use std::ffi::c_void;
use std::ptr::null_mut;

use windows_sys::Win32::Graphics::Gdi::*;

/// Pixels (0xRRGGBB, row-major) of the `side` x `side` screen square centered on (cx, cy).
pub fn region(cx: i32, cy: i32, side: i32) -> Option<Vec<u32>> {
    unsafe {
        let screen = GetDC(null_mut());
        if screen.is_null() {
            return None;
        }
        let dc = CreateCompatibleDC(screen);
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: side,
                biHeight: -side,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [RGBQUAD { rgbBlue: 0, rgbGreen: 0, rgbRed: 0, rgbReserved: 0 }],
        };
        let mut bits: *mut c_void = null_mut();
        let dib = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        let mut out = None;
        if !dib.is_null() && !bits.is_null() {
            let old = SelectObject(dc, dib);
            if BitBlt(dc, 0, 0, side, side, screen, cx - side / 2, cy - side / 2, SRCCOPY | CAPTUREBLT) != 0 {
                GdiFlush();
                let px = std::slice::from_raw_parts(bits as *const u32, (side * side) as usize);
                out = Some(px.iter().map(|p| p & 0x00FF_FFFF).collect());
            }
            SelectObject(dc, old);
            DeleteObject(dib);
        }
        DeleteDC(dc);
        ReleaseDC(null_mut(), screen);
        out
    }
}
