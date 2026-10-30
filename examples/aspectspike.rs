//! Spike (test tooling, ships nothing): can the Windows Magnification API show a screen region
//! stretched by different amounts horizontally and vertically, so one area can change aspect ratio?
//! It puts four colored quadrants on screen, points a magnifier at them with an unequal x/y scale into
//! a wide window, and reads back where the colors landed.
//! Run: cargo run --release --example aspectspike

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

unsafe fn capture(x: i32, y: i32, w: i32, h: i32) -> Vec<(u8, u8, u8)> {
    let screen = GetDC(null_mut());
    let dc = CreateCompatibleDC(screen);
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB, biSizeImage: 0, biXPelsPerMeter: 0, biYPelsPerMeter: 0, biClrUsed: 0, biClrImportant: 0 },
        bmiColors: [RGBQUAD { rgbBlue: 0, rgbGreen: 0, rgbRed: 0, rgbReserved: 0 }],
    };
    let mut bits: *mut c_void = null_mut();
    let dib = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
    let old = SelectObject(dc, dib);
    BitBlt(dc, 0, 0, w, h, screen, x, y, SRCCOPY | CAPTUREBLT);
    GdiFlush();
    let px = (0..(w * h) as usize).map(|i| { let p = *(bits as *const u32).add(i); ((p >> 16) as u8, (p >> 8) as u8, p as u8) }).collect();
    SelectObject(dc, old);
    DeleteObject(dib);
    DeleteDC(dc);
    ReleaseDC(null_mut(), screen);
    px
}

unsafe extern "system" fn quad_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_ERASEBKGND {
        let mut r: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut r);
        let (mx, my) = (r.right / 2, r.bottom / 2);
        let fill = |l: i32, t: i32, rr: i32, b: i32, c: u32| {
            let brush = CreateSolidBrush(c);
            FillRect(wp as HDC, &RECT { left: l, top: t, right: rr, bottom: b }, brush);
            DeleteObject(brush);
        };
        fill(0, 0, mx, my, 0x0000FF); // red (COLORREF is BGR)
        fill(mx, 0, r.right, my, 0x00FF00); // green
        fill(0, my, mx, r.bottom, 0xFF0000); // blue
        fill(mx, my, r.right, r.bottom, 0x00FFFF); // yellow
        return 1;
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

fn name(p: (u8, u8, u8)) -> &'static str {
    match (p.0 > 200, p.1 > 200, p.2 > 200) {
        (true, false, false) => "red",
        (false, true, false) => "green",
        (false, false, true) => "blue",
        (true, true, false) => "yellow",
        _ => "other",
    }
}

fn main() {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let hi = GetModuleHandleW(null());
        let class = wide("AspectSpikeQuad");
        let wc = WNDCLASSEXW { cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32, style: 0, lpfnWndProc: Some(quad_proc), cbClsExtra: 0, cbWndExtra: 0, hInstance: hi, hIcon: null_mut(), hCursor: null_mut(), hbrBackground: null_mut(), lpszMenuName: null(), lpszClassName: class.as_ptr(), hIconSm: null_mut() };
        assert!(RegisterClassExW(&wc) != 0);
        let (cx, cy) = (GetSystemMetrics(SM_CXSCREEN) / 2, GetSystemMetrics(SM_CYSCREEN) / 2);
        let quad = CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, class.as_ptr(), null(), WS_POPUP | WS_VISIBLE, cx - 50, cy - 50, 100, 100, null_mut(), null_mut(), hi, null());
        InvalidateRect(quad, null(), 1);
        UpdateWindow(quad);
        pump(300);
        assert!(MagInitialize() != 0, "MagInitialize");
        // A wide magnifier window to the right: 200 x 100, fed a 100 x 100 source, x scale 2.0, y scale 1.0.
        let (hx, hy, hw, hh) = (cx + 200, cy - 50, 200, 100);
        let host = CreateWindowExW(WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, class.as_ptr(), null(), WS_POPUP, hx, hy, hw, hh, null_mut(), null_mut(), hi, null());
        SetLayeredWindowAttributes(host, 0, 255, LWA_ALPHA);
        let mag = CreateWindowExW(0, WC_MAGNIFIERW, wide("MagnifierWindow").as_ptr(), WS_CHILD | WS_VISIBLE, 0, 0, hw, hh, host, null_mut(), hi, null());
        assert!(!mag.is_null(), "magnifier child window");
        ShowWindow(host, SW_SHOWNOACTIVATE);
        for (sx, sy) in [(2.0f32, 1.0f32), (2.0, 2.0), (1.0, 2.0)] {
            let mut t = MAGTRANSFORM { v: [sx, 0.0, 0.0, 0.0, sy, 0.0, 0.0, 0.0, 1.0] };
            let ok = MagSetWindowTransform(mag, &mut t) != 0;
            // the source rect is what the window can show at this scale
            let (sw, sh) = ((hw as f32 / sx).round() as i32, (hh as f32 / sy).round() as i32);
            let src = RECT { left: cx - sw / 2, top: cy - sh / 2, right: cx - sw / 2 + sw, bottom: cy - sh / 2 + sh };
            let mut tries = 0;
            let shot = loop {
                MagSetWindowSource(mag, src);
                pump(120);
                let s = capture(hx, hy, hw, hh);
                tries += 1;
                if name(s[(hh as usize / 4) * hw as usize + hw as usize / 4]) != "other" || tries > 8 {
                    break s;
                }
            };
            let at = |fx: f32, fy: f32| name(shot[((hh as f32 * fy) as usize) * hw as usize + (hw as f32 * fx) as usize]);
            // find where red turns green along a row in the top quarter, and red turns blue down a column on the left quarter
            let row = (hh as usize) / 4;
            let col = (hw as usize) / 4;
            let red_to_green = (0..hw as usize).find(|x| name(shot[row * hw as usize + x]) == "green").unwrap_or(0);
            let red_to_blue = (0..hh as usize).find(|y| name(shot[y * hw as usize + col]) == "blue").unwrap_or(0);
            println!(
                "transform ({sx}, {sy}) set={ok}: corners TL {} TR {} BL {} BR {}; red->green at x={red_to_green} of {hw}, red->blue at y={red_to_blue} of {hh}",
                at(0.1, 0.1),
                at(0.9, 0.1),
                at(0.1, 0.9),
                at(0.9, 0.9)
            );
        }
        MagUninitialize();
        DestroyWindow(host);
        DestroyWindow(quad);
    }
}
