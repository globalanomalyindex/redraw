//! Throwaway proof for the experimental features. Opens real windows over known
//! colors, captures the composited screen, and prints PASS/FAIL per claim:
//!   glow      alpha-0 premultiplied pixels add light instead of replacing
//!   capture   WDA_EXCLUDEFROMCAPTURE hides a window from screen capture
//!   plain     a plain BitBlt (no CAPTUREBLT) does not see layered windows
//!   lens      a Magnifier window with a color matrix and a window region
//!             shows the backdrop inverted only inside the shape
//!   zoom      a Magnifier transform scales the source region
//! Run: cargo run --example spike

use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows_sys::Win32::UI::Magnification::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

unsafe extern "system" fn solid_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_ERASEBKGND {
        let brush = CreateSolidBrush(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as u32);
        let mut r: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut r);
        FillRect(wp as HDC, &r, brush);
        DeleteObject(brush);
        return 1;
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

unsafe fn pump(ms: u64) {
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        let mut msg: MSG = std::mem::zeroed();
        while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

unsafe fn solid_window(x: i32, y: i32, w: i32, h: i32, color: u32) -> HWND {
    let hwnd = CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, wide("SpikeSolid").as_ptr(), null(), WS_POPUP | WS_VISIBLE, x, y, w, h, null_mut(), null_mut(), GetModuleHandleW(null()), null());
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, color as isize);
    InvalidateRect(hwnd, null(), 1);
    UpdateWindow(hwnd);
    hwnd
}

/// Top-most, click-through, per-pixel-alpha window showing `pixels` (premultiplied ARGB, w*h).
unsafe fn overlay(x: i32, y: i32, w: i32, h: i32, pixels: &[u32]) -> HWND {
    let hwnd = CreateWindowExW(WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, wide("SpikeSolid").as_ptr(), null(), WS_POPUP, x, y, w, h, null_mut(), null_mut(), GetModuleHandleW(null()), null());
    set_pixels(hwnd, x, y, w, h, pixels);
    ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    hwnd
}

unsafe fn set_pixels(hwnd: HWND, x: i32, y: i32, w: i32, h: i32, pixels: &[u32]) {
    let dc = CreateCompatibleDC(null_mut());
    let (dib, bits) = dib(dc, w, h);
    std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits, (w * h) as usize);
    let old = SelectObject(dc, dib);
    let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
    let ok = UpdateLayeredWindow(hwnd, null_mut(), &POINT { x, y }, &SIZE { cx: w, cy: h }, dc, &POINT { x: 0, y: 0 }, 0, &blend, ULW_ALPHA);
    assert!(ok != 0, "UpdateLayeredWindow failed");
    SelectObject(dc, old);
    DeleteObject(dib);
    DeleteDC(dc);
    pump(60);
}

unsafe fn dib(dc: HDC, w: i32, h: i32) -> (HBITMAP, *mut u32) {
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB, biSizeImage: 0, biXPelsPerMeter: 0, biYPelsPerMeter: 0, biClrUsed: 0, biClrImportant: 0 },
        bmiColors: [RGBQUAD { rgbBlue: 0, rgbGreen: 0, rgbRed: 0, rgbReserved: 0 }],
    };
    let mut bits: *mut c_void = null_mut();
    let d = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
    (d, bits as *mut u32)
}

/// Screen pixels as (r,g,b), row-major. `layered` = include layered windows (CAPTUREBLT).
unsafe fn capture(x: i32, y: i32, w: i32, h: i32, layered: bool) -> Vec<(u8, u8, u8)> {
    let screen = GetDC(null_mut());
    let dc = CreateCompatibleDC(screen);
    let (d, bits) = dib(dc, w, h);
    let old = SelectObject(dc, d);
    BitBlt(dc, 0, 0, w, h, screen, x, y, if layered { SRCCOPY | CAPTUREBLT } else { SRCCOPY });
    GdiFlush();
    let px: Vec<(u8, u8, u8)> = (0..(w * h) as usize).map(|i| { let p = *bits.add(i); ((p >> 16) as u8, (p >> 8) as u8, p as u8) }).collect();
    SelectObject(dc, old);
    DeleteObject(d);
    DeleteDC(dc);
    ReleaseDC(null_mut(), screen);
    px
}

fn near(a: (u8, u8, u8), b: (u8, u8, u8)) -> bool {
    (a.0 as i32 - b.0 as i32).abs() <= 3 && (a.1 as i32 - b.1 as i32).abs() <= 3 && (a.2 as i32 - b.2 as i32).abs() <= 3
}

fn report(name: &str, ok: bool, detail: String) {
    println!("{} {name}: {detail}", if ok { "PASS" } else { "FAIL" });
}

struct Lens {
    host: HWND,
    mag: HWND,
}

unsafe fn lens(x: i32, y: i32, w: i32, h: i32) -> Lens {
    let hi = GetModuleHandleW(null());
    let host = CreateWindowExW(WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, wide("SpikeSolid").as_ptr(), null(), WS_POPUP, x, y, w, h, null_mut(), null_mut(), hi, null());
    SetLayeredWindowAttributes(host, 0, 255, LWA_ALPHA);
    let mag = CreateWindowExW(0, WC_MAGNIFIERW, wide("MagnifierWindow").as_ptr(), WS_CHILD | WS_VISIBLE, 0, 0, w, h, host, null_mut(), hi, null());
    assert!(!mag.is_null(), "magnifier control was not created");
    let mut list = [host];
    MagSetWindowFilterList(mag, MW_FILTERMODE_EXCLUDE, 1, list.as_mut_ptr());
    ShowWindow(host, SW_SHOWNOACTIVATE);
    Lens { host, mag }
}

unsafe fn lens_set(l: &Lens, src: RECT, zoom: f32, matrix: Option<[f32; 25]>) {
    let t = MAGTRANSFORM { v: [zoom, 0.0, 0.0, 0.0, zoom, 0.0, 0.0, 0.0, 1.0] };
    MagSetWindowTransform(l.mag, &t as *const MAGTRANSFORM as *mut MAGTRANSFORM);
    if let Some(m) = matrix {
        let e = MAGCOLOREFFECT { transform: m };
        MagSetColorEffect(l.mag, &e as *const MAGCOLOREFFECT as *mut MAGCOLOREFFECT);
    }
    for _ in 0..8 {
        MagSetWindowSource(l.mag, src);
        pump(25);
    }
}

fn main() {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let hi = GetModuleHandleW(null());
        let class_name = wide("SpikeSolid");
        let wc = WNDCLASSEXW { cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32, style: 0, lpfnWndProc: Some(solid_proc), cbClsExtra: 0, cbWndExtra: 0, hInstance: hi, hIcon: null_mut(), hCursor: null_mut(), hbrBackground: null_mut(), lpszMenuName: null(), lpszClassName: class_name.as_ptr(), hIconSm: null_mut() };
        assert!(RegisterClassExW(&wc) != 0);
        assert!(MagInitialize() != 0, "MagInitialize failed");

        // Backdrop: (200,100,50) at (300,300) 200x200.
        let back_rgb = (200u8, 100u8, 50u8);
        let backdrop = solid_window(300, 300, 200, 200, rgb(back_rgb.0, back_rgb.1, back_rgb.2));
        pump(150);

        // plain: a normal-alpha red 8x8 block.
        let red: Vec<u32> = vec![0xFFFF0000; 64];
        let ov = overlay(350, 350, 8, 8, &red);
        let p = capture(354, 354, 1, 1, true)[0];
        report("plain overlay (CAPTUREBLT)", near(p, (255, 0, 0)), format!("{p:?}"));
        let p = capture(354, 354, 1, 1, false)[0];
        report("plain BitBlt skips layered windows", near(p, back_rgb), format!("{p:?} (backdrop {back_rgb:?})"));

        // glow: alpha 0, color 40,40,40 premultiplied -> additive?
        let glow: Vec<u32> = vec![0x0028_2828; 64];
        set_pixels(ov, 350, 350, 8, 8, &glow);
        let p = capture(354, 354, 1, 1, true)[0];
        let want = (240u8, 140u8, 90u8);
        report("glow (alpha 0 adds light)", near(p, want), format!("{p:?} want {want:?}"));

        // capture exclusion
        set_pixels(ov, 350, 350, 8, 8, &red);
        let affinity_ok = SetWindowDisplayAffinity(ov, 0x11) != 0;
        pump(150);
        let p = capture(354, 354, 1, 1, true)[0];
        report("WDA_EXCLUDEFROMCAPTURE accepted", affinity_ok, String::new());
        report("excluded window absent from capture", near(p, back_rgb), format!("{p:?} (red would mean still captured)"));
        SetWindowDisplayAffinity(ov, 0);
        DestroyWindow(ov);

        // lens: invert through a window region (left half only).
        let l = lens(400, 400, 40, 40);
        SetWindowRgn(l.host, CreateRectRgn(0, 0, 20, 40), 1);
        let invert = [-1.0, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 0.0, 1.0];
        lens_set(&l, RECT { left: 400, top: 400, right: 440, bottom: 440 }, 1.0, Some(invert));
        let inside = capture(410, 420, 1, 1, true)[0];
        let outside = capture(430, 420, 1, 1, true)[0];
        let inv = (55u8, 155u8, 205u8);
        report("lens invert inside the shape", near(inside, inv), format!("{inside:?} want {inv:?}"));
        report("lens leaves outside the shape alone", near(outside, back_rgb), format!("{outside:?} want {back_rgb:?}"));
        DestroyWindow(l.host);

        // zoom: 4px white stripe on black, magnified 2x and 4x.
        let black = solid_window(500, 300, 120, 120, rgb(0, 0, 0));
        let stripe = solid_window(548, 320, 4, 60, rgb(255, 255, 255));
        pump(150);
        let z = lens(700, 350, 40, 40);
        for (zoom, want) in [(1.0f32, 4usize), (2.0, 8), (4.0, 16)] {
            let half = (20.0 / zoom) as i32;
            lens_set(&z, RECT { left: 550 - half, top: 350 - half, right: 550 + half, bottom: 350 + half }, zoom, None);
            let row = capture(700, 370, 40, 1, true);
            let white = row.iter().filter(|p| p.0 > 200 && p.1 > 200 && p.2 > 200).count();
            report(&format!("zoom {zoom}x scales a 4px stripe"), (white as i32 - want as i32).abs() <= 1, format!("{white} white px, want {want}"));
        }
        DestroyWindow(z.host);
        DestroyWindow(stripe);
        DestroyWindow(black);
        DestroyWindow(backdrop);
        MagUninitialize();
    }
}
