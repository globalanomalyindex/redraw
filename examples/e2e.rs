//! End-to-end check of the real redraw.exe on the real compositor.
//! For each feature it writes a config next to a copy of the exe, launches it
//! over backdrop windows of a known color, waits, captures the screen, and
//! checks pixels. Test tooling only: nothing here ships in redraw.exe.
//! Run: cargo run --release --example e2e -- <path to redraw.exe>
//!
//! The trace case moves the mouse pointer a little with SendInput and moves
//! it back; it presses no buttons.

use std::ffi::c_void;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::process::{Child, Command};
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

// The updater's own logic (SHA-256 and the like), so the test hashes exactly as the program does.
#[path = "../src/update.rs"]
#[allow(dead_code)]
mod update;

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

unsafe fn solid(x: i32, y: i32, w: i32, h: i32, color: u32) -> HWND {
    let hwnd = CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, wide("E2eSolid").as_ptr(), null(), WS_POPUP | WS_VISIBLE, x, y, w, h, null_mut(), null_mut(), GetModuleHandleW(null()), null());
    recolor(hwnd, color);
    hwnd
}

unsafe fn recolor(hwnd: HWND, color: u32) {
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, color as isize);
    InvalidateRect(hwnd, null(), 1);
    UpdateWindow(hwnd);
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
    let px: Vec<(u8, u8, u8)> = (0..(w * h) as usize).map(|i| { let p = *(bits as *const u32).add(i); ((p >> 16) as u8, (p >> 8) as u8, p as u8) }).collect();
    SelectObject(dc, old);
    DeleteObject(dib);
    DeleteDC(dc);
    ReleaseDC(null_mut(), screen);
    px
}

fn near(a: (u8, u8, u8), b: (u8, u8, u8), tol: i32) -> bool {
    (a.0 as i32 - b.0 as i32).abs() <= tol && (a.1 as i32 - b.1 as i32).abs() <= tol && (a.2 as i32 - b.2 as i32).abs() <= tol
}

struct Run {
    child: Child,
}

impl Drop for Run {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn launch(dir: &PathBuf, cfg: &str) -> Run {
    std::fs::write(dir.join("redraw.cfg"), cfg).unwrap();
    let child = Command::new(dir.join("redraw.exe")).current_dir(dir).spawn().expect("start redraw.exe");
    Run { child }
}

/// Later keys win, so `extra` can override the pinned monitor and scale.
fn base_cfg(layers: &str, extra: &str) -> String {
    format!("trigger=key\nkey=F8\nhip.outline=0\nhip.opacity=10\nhip.layers={layers}\nads.layers=oc1 dot.ffffff.1.0.1.1.0\ndisplay.monitor=primary\ndisplay.scale=1\nautoupdate={}\n{extra}\n", std::env::var("E2E_AUTOUPDATE").unwrap_or_else(|_| "0".to_string()))
}

/// Make `hwnd` the foreground window. Windows only lets the foreground process do that, so
/// attach to its input thread for the call (the standard workaround).
unsafe fn take_foreground(hwnd: HWND) -> bool {
    let current = GetForegroundWindow();
    let their_thread = GetWindowThreadProcessId(current, null_mut());
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    let my_thread = GetCurrentThreadId();
    if their_thread != 0 && their_thread != my_thread {
        AttachThreadInput(my_thread, their_thread, 1);
    }
    BringWindowToTop(hwnd);
    SetForegroundWindow(hwnd);
    if their_thread != 0 && their_thread != my_thread {
        AttachThreadInput(my_thread, their_thread, 0);
    }
    pump(250);
    GetForegroundWindow() == hwnd
}

/// Tick count of the last real keyboard or mouse input, system-wide.
unsafe fn last_input_tick() -> u32 {
    let mut li = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    GetLastInputInfo(&mut li);
    li.dwTime
}

unsafe fn cursor_x() -> i32 {
    let mut p = POINT { x: 0, y: 0 };
    GetCursorPos(&mut p);
    p.x
}

/// Press and release one key (used to open and close the menu with F10).
unsafe fn key_tap(vk: u16) {
    for flags in [0, KEYEVENTF_KEYUP] {
        let input = INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } } };
        SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
    }
}

/// One key event: down or up (used to hold a state key).
unsafe fn key_event(vk: u16, up: bool) {
    let input = INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { 0 }, time: 0, dwExtraInfo: 0 } } };
    SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
}

/// F10 opens or closes the menu. The test build (which runs next to a real copy) uses Ctrl+F10 instead,
/// so a test never opens the real copy's menu. Ctrl alone has no side effects on the focused window.
unsafe fn menu_tap() {
    if cfg!(feature = "multi-instance") {
        key_event(VK_CONTROL, false);
        key_tap(0x79);
        key_event(VK_CONTROL, true);
    } else {
        key_tap(0x79);
    }
}

/// Menu windows of one process (`pid`), or of every other process when `others` is true.
unsafe fn menu_windows(pid: u32, others: bool) -> Vec<HWND> {
    let (mut out, mut h) = (Vec::new(), null_mut());
    loop {
        h = FindWindowExW(null_mut(), h, wide("reDrawMenu").as_ptr(), null());
        if h.is_null() {
            return out;
        }
        let mut owner = 0u32;
        GetWindowThreadProcessId(h, &mut owner);
        if (owner == pid) != others {
            out.push(h);
        }
    }
}

/// The menu window of the program under test (not anyone else's).
unsafe fn find_menu(pid: u32) -> HWND {
    menu_windows(pid, false).first().copied().unwrap_or(null_mut())
}

/// Is a menu of some other reDraw (say, a real copy a person is using) on screen? Pointer tests
/// must then stand down: they would hover and click on it.
unsafe fn foreign_menu_open(pid: u32) -> bool {
    menu_windows(pid, true).iter().any(|h| IsWindowVisible(*h) != 0)
}

/// Press or release the left mouse button where the pointer is (keep it over our own window).
unsafe fn left_button(down: bool) {
    let input = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx: 0, dy: 0, mouseData: 0, dwFlags: if down { MOUSEEVENTF_LEFTDOWN } else { MOUSEEVENTF_LEFTUP }, time: 0, dwExtraInfo: 0 } } };
    SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
}

unsafe fn move_mouse(dx: i32, dy: i32) {
    let input = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: 0, dwFlags: MOUSEEVENTF_MOVE, time: 0, dwExtraInfo: 0 } } };
    SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A tiny HTTP server for the update test: answers each request path from `routes`, 404 otherwise,
/// and records every path it was asked for. `make` gets the port so a manifest can name its own address.
fn serve(make: impl FnOnce(u16) -> Vec<(String, Vec<u8>)>) -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let routes = make(port);
    let asked = Arc::new(Mutex::new(Vec::new()));
    let log = asked.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
            log.lock().unwrap().push(path.clone());
            match routes.iter().find(|(p, _)| *p == path) {
                Some((_, body)) => {
                    let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes());
                    let _ = s.write_all(body);
                }
                None => {
                    let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                }
            }
        }
    });
    (port, asked)
}

fn main() {
    let exe = PathBuf::from(std::env::args().nth(1).expect("usage: e2e <path to redraw.exe>"));
    let dir = std::env::temp_dir().join("redraw-e2e");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(&exe, dir.join("redraw.exe")).unwrap();
    // Optional second argument: run only the cases whose name contains it (glow, lens, capture, auto, idle, zoom, trace, scale, shapes, cycle, morph, states, update, forcequit, restart, follow).
    let only = std::env::args().nth(2).unwrap_or_default();
    let go = |name: &str| only.is_empty() || name.contains(only.as_str());
    let mut failures = 0;
    let mut check = |name: &str, ok: bool, detail: String| {
        println!("{} {name}: {detail}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            failures += 1;
        }
    };
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let class_name = wide("E2eSolid");
        let wc = WNDCLASSEXW { cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32, style: 0, lpfnWndProc: Some(solid_proc), cbClsExtra: 0, cbWndExtra: 0, hInstance: GetModuleHandleW(null()), hIcon: null_mut(), hCursor: null_mut(), hbrBackground: null_mut(), lpszMenuName: null(), lpszClassName: class_name.as_ptr(), hIconSm: null_mut() };
        assert!(RegisterClassExW(&wc) != 0);
        let (cx, cy) = (GetSystemMetrics(SM_CXSCREEN) / 2, GetSystemMetrics(SM_CYSCREEN) / 2);
        let back = (200u8, 100u8, 50u8);
        let backdrop = solid(cx - 200, cy - 200, 400, 400, rgb(back.0, back.1, back.2));
        pump(200);

        // glow: alpha-0 light adds to the backdrop
        if go("glow") {
            let _r = launch(&dir, &base_cfg("oc1 dot.282828.9.0.1.1.0~g", ""));
            pump(1200);
            let p = capture(cx, cy, 1, 1)[0];
            check("glow adds light", near(p, (240, 140, 90), 3), format!("{p:?} want (240,140,90)"));
        }

        // lens: invert inside the crosshair shape only
        if go("lens") {
            let _r = launch(&dir, &base_cfg("oc1 dot.ffffff.5.0.1.1.0~i", ""));
            pump(1500);
            let inside = capture(cx, cy, 1, 1)[0];
            let outside = capture(cx + 12, cy, 1, 1)[0];
            check("lens invert inside the shape", near(inside, (55, 155, 205), 4), format!("{inside:?} want (55,155,205)"));
            check("lens leaves the rest alone", near(outside, back, 3), format!("{outside:?} want {back:?}"));
            // The lens is refreshed at most 60 times a second (to spare the compositor); it must still
            // follow a changing screen behind it, not look stuck.
            recolor(backdrop, rgb(100, 200, 150));
            let t = Instant::now();
            let mut latency = None;
            while t.elapsed() < Duration::from_millis(500) {
                if near(capture(cx, cy, 1, 1)[0], (155, 55, 105), 6) {
                    latency = Some(t.elapsed().as_millis());
                    break;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            check("lens follows a changing backdrop within 100 ms", latency.is_some_and(|l| l <= 100), format!("{latency:?} ms"));
            recolor(backdrop, rgb(back.0, back.1, back.2));
        }

        // capture hiding
        if go("capture") {
            let _r = launch(&dir, &base_cfg("oc1 dot.ff0000.9.0.1.1.0", "capture=1"));
            pump(1200);
            let shown = capture(cx, cy, 1, 1)[0];
            check("overlay visible to capture by default", near(shown, (255, 0, 0), 3), format!("{shown:?}"));
        }
        if go("capture") {
            let _r = launch(&dir, &base_cfg("oc1 dot.ff0000.9.0.1.1.0", "capture=0"));
            pump(1200);
            let hidden = capture(cx, cy, 1, 1)[0];
            check("capture=0 hides it from capture", near(hidden, back, 3), format!("{hidden:?} want backdrop {back:?}"));
        }

        // auto-contrast: dark over white, white over black
        if go("auto-contrast") || go("auto contrast") {
            recolor(backdrop, rgb(255, 255, 255));
            let _r = launch(&dir, &base_cfg("oc1 dot.00ff66.9.0.1.1.0", "auto.enabled=1\nauto.mode=lightdark\nauto.rate=3"));
            pump(1500);
            let on_white = capture(cx, cy, 1, 1)[0];
            check("auto-contrast picks dark on white", near(on_white, (16, 18, 22), 6), format!("{on_white:?}"));
            recolor(backdrop, rgb(0, 0, 0));
            pump(1200);
            let on_black = capture(cx, cy, 1, 1)[0];
            check("auto-contrast switches to light on black", near(on_black, (255, 255, 255), 4), format!("{on_black:?}"));
        }

        // idle fade (reacts to any real input, so it is skipped if someone touched the mouse or keyboard)
        if go("idle") {
            recolor(backdrop, rgb(0, 0, 0));
            let input_before = last_input_tick();
            let _r = launch(&dir, &base_cfg("oc1 dot.ffffff.9.0.1.1.0", "dyn.enabled=1\ndyn.idle=1\ndyn.bloom=0\ndyn.spread=0"));
            pump(1200);
            let before = capture(cx, cy, 1, 1)[0];
            pump(4500);
            let after = capture(cx, cy, 1, 1)[0];
            if last_input_tick() != input_before {
                println!("SKIP idle fade: the mouse or keyboard was used during the test (before {before:?}, after {after:?})");
            } else {
                check("idle fade: fully visible at first", before.0 > 240, format!("{before:?}"));
                check("idle fade: dims after 3s without input", (after.0 as i32 - 64).abs() <= 10, format!("{after:?} want about 64"));
            }
        }

        // scope zoom: a 4px stripe becomes 8px at 2x
        if go("zoom") {
            recolor(backdrop, rgb(0, 0, 0));
            let stripe = solid(cx - 2, cy - 150, 4, 300, rgb(255, 255, 255));
            pump(200);
            let _r = launch(&dir, &base_cfg("oc1 -dot.ffffff.1.0.1.1.0", "zoom.enabled=1\nzoom.when=always\nzoom.factor=20\nzoom.size=200\nzoom.shape=square"));
            pump(1800);
            let row = capture(cx - 60, cy + 40, 120, 1);
            let white = row.iter().filter(|p| p.0 > 200 && p.1 > 200 && p.2 > 200).count();
            check("scope zoom doubles the stripe", (7..=9).contains(&white), format!("{white} white px, want 8"));
            drop(_r);
            DestroyWindow(stripe);
        }

        // spray trace: relative mouse motion draws the tracer (the pointer is moved toward the
        // screen center a little and restored, so it always has room to move)
        if go("trace") {
            recolor(backdrop, rgb(0, 0, 0));
            let _r = launch(&dir, &base_cfg("oc1 -dot.ffffff.1.0.1.1.0", "trigger=key\ntrace.enabled=1\ntrace.active=always\ntrace.style=line\ntrace.return=hold\ntrace.smooth=0\ntrace.scale=10\ntrace.width=1\ntrace.color=#ff00ff"));
            pump(1200);
            let start_x = cursor_x();
            let dir_x = if start_x > cx { -1 } else { 1 };
            for _ in 0..20 {
                move_mouse(5 * dir_x, 0);
                pump(8);
            }
            pump(400);
            let wide_row = capture(cx - 150, cy, 301, 1);
            let is_magenta = |p: &(u8, u8, u8)| p.0 > 200 && p.1 < 60 && p.2 > 200;
            let left = wide_row[..150].iter().filter(|p| is_magenta(p)).count();
            let right = wide_row[151..].iter().filter(|p| is_magenta(p)).count();
            println!("   trace debug: pointer x {start_x} -> {} (screen center x {cx}); magenta left {left}, right {right}", cursor_x());
            let magenta = if dir_x > 0 { right } else { left };
            for _ in 0..20 {
                move_mouse(-5 * dir_x, 0);
                pump(8);
            }
            check("spray trace draws a line from raw mouse motion", magenta >= 20, format!("{magenta} magenta px on the {} side of center", if dir_x > 0 { "right" } else { "left" }));
        }

        // spray trace in its DEFAULT mode (Active: Fire): nothing while the button is up, a line while it is held.
        // The pointer is put over our own backdrop so the injected click lands on our own window.
        if go("trace-fire") {
            recolor(backdrop, rgb(0, 0, 0));
            let _r = launch(&dir, &base_cfg("oc1 -dot.ffffff.1.0.1.1.0", "trace.enabled=1
trace.active=fire
trace.style=line
trace.return=hold
trace.smooth=0
trace.scale=10
trace.width=1
trace.color=#ff00ff"));
            pump(1200);
            let mut saved = POINT { x: 0, y: 0 };
            GetCursorPos(&mut saved);
            SetCursorPos(cx - 100, cy);
            pump(150);
            let is_magenta = |p: &(u8, u8, u8)| p.0 > 200 && p.1 < 60 && p.2 > 200;
            let count = || capture(cx - 150, cy, 301, 1).iter().filter(|p| is_magenta(p)).count();
            for _ in 0..10 {
                move_mouse(5, 0);
                pump(8);
            }
            pump(300);
            let idle = count();
            left_button(true);
            pump(150);
            for _ in 0..20 {
                move_mouse(5, 0);
                pump(8);
            }
            pump(300);
            let firing = count();
            left_button(false);
            pump(200);
            let held_after = count();
            SetCursorPos(saved.x, saved.y);
            check("trace (Fire): moving without firing draws nothing", idle == 0, format!("{idle} magenta px"));
            check("trace (Fire): holding the button and moving draws a line", firing >= 20, format!("{firing} magenta px"));
            check("trace (Fire, Return: Hold): the line stays after release", held_after >= 20, format!("{held_after} magenta px"));
        }

        // whole-number scale: every crosshair pixel becomes a 2x2 block
        if go("scale") {
            recolor(backdrop, rgb(back.0, back.1, back.2));
            let _r = launch(&dir, &base_cfg("oc1 dot.ff0000.9.0.1.1.0", "display.scale=2"));
            pump(1200);
            let row = capture(cx - 20, cy, 40, 1);
            let red = row.iter().filter(|p| near(**p, (255, 0, 0), 3)).count();
            let col = capture(cx, cy - 20, 1, 40);
            let red_v = col.iter().filter(|p| near(**p, (255, 0, 0), 3)).count();
            check("scale 2x: a 9-cell dot is 18 pixels wide and tall", red == 18 && red_v == 18, format!("{red} wide, {red_v} tall"));
        }

        // The real menu window: hovering a row tints it, and moving on un-tints it. This is what the
        // partial (changed-rows) redraw has to get right on the actual compositor.
        if go("menu-hover") {
            // trigger=hold: no extra "Key:" row, so the two header rows below are where the layout puts them.
            let _r = launch(&dir, &base_cfg("oc1 dot.ff0000.9.0.1.1.0", "trigger=hold"));
            pump(1200);
            menu_tap(); // F10 opens the menu
            pump(700);
            let menu = find_menu(_r.child.id());
            let foreign = foreign_menu_open(_r.child.id());
            let mut rc: RECT = std::mem::zeroed();
            if foreign {
                println!("SKIP this menu check: another reDraw menu (a real copy, not the one under test) is open, and a pointer test must not touch it");
            } else if menu.is_null() || IsWindowVisible(menu) == 0 || GetWindowRect(menu, &mut rc) == 0 {
                check("menu hover: the menu opens with F10", false, "menu window not visible".to_string());
            } else {
                let s = ((rc.right - rc.left) / 212).max(1);
                let at = |lx: i32, ly: i32| (rc.left + lx * s, rc.top + ly * s);
                // Two header rows: Profile at logical y 45..59, Layers at 119..133 (below the update status line and the Profile rows, incl. Morph). x=200 is right of the text.
                let (row1, row2) = (at(200, 52), at(200, 126));
                SetCursorPos(cx, cy + 300); // somewhere that is not the menu
                pump(300);
                let base1 = capture(row1.0, row1.1, 1, 1)[0];
                let base2 = capture(row2.0, row2.1, 1, 1)[0];
                let brighter = |p: (u8, u8, u8), base: (u8, u8, u8)| p.0 as i32 - base.0 as i32 >= 4 && p.1 as i32 - base.1 as i32 >= 4 && p.2 as i32 - base.2 as i32 >= 4;
                let over1 = at(100, 52);
                SetCursorPos(over1.0, over1.1);
                pump(300);
                let lit1 = capture(row1.0, row1.1, 1, 1)[0];
                let over2 = at(100, 126);
                SetCursorPos(over2.0, over2.1);
                pump(300);
                let back1 = capture(row1.0, row1.1, 1, 1)[0];
                let lit2 = capture(row2.0, row2.1, 1, 1)[0];
                check("menu hover: the row under the pointer is tinted", brighter(lit1, base1), format!("{base1:?} -> {lit1:?}"));
                check("menu hover: moving on un-tints the old row", near(back1, base1, 1), format!("{back1:?} want {base1:?}"));
                check("menu hover: and tints the new one", brighter(lit2, base2), format!("{base2:?} -> {lit2:?}"));
                SetCursorPos(cx, cy + 300);
                menu_tap(); // F10 closes it
                pump(300);
            }
        }

        // The DEFAULT config must keep the crosshair at the screen center even with a big window focused.
        if go("default-center") {
            let fg = CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW, wide("E2eSolid").as_ptr(), null(), WS_POPUP | WS_VISIBLE, 150, 80, 1200, 700, null_mut(), null_mut(), GetModuleHandleW(null()), null());
            recolor(fg, rgb(30, 60, 90));
            if !take_foreground(fg) {
                println!("SKIP default stays at the screen center: this session cannot take the foreground");
            } else {
                // No display lines at all: whatever the program does by default.
                std::fs::write(dir.join("redraw.cfg"), "trigger=key\nkey=F8\nhip.outline=0\nhip.opacity=10\nhip.layers=oc1 dot.ff0000.9.0.1.1.0\nads.layers=oc1 dot.ffffff.1.0.1.1.0\n").unwrap();
                let child = Command::new(dir.join("redraw.exe")).current_dir(&dir).spawn().expect("start redraw.exe");
                let _r = Run { child };
                pump(1500);
                let at_screen_center = capture(cx, cy, 1, 1)[0];
                let at_window_center = capture(150 + 600, 80 + 350, 1, 1)[0];
                check("default config: crosshair stays at the screen center", near(at_screen_center, (255, 0, 0), 3) && !near(at_window_center, (255, 0, 0), 3), format!("screen center {at_screen_center:?}, window center {at_window_center:?}"));
            }
            DestroyWindow(fg);
        }

        // Follow window (opt-in): tracks the focused window's viewport
        if go("follow") {
            let fg = CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW, wide("E2eSolid").as_ptr(), null(), WS_POPUP | WS_VISIBLE, 150, 80, 1200, 700, null_mut(), null_mut(), GetModuleHandleW(null()), null());
            recolor(fg, rgb(30, 60, 90));
            if !take_foreground(fg) {
                println!("SKIP follow window: this session cannot take the foreground");
            } else {
                let _r = launch(&dir, &base_cfg("oc1 dot.ff0000.9.0.1.1.0", "display.monitor=follow"));
                pump(1500);
                let at_window_center = capture(150 + 600, 80 + 350, 1, 1)[0];
                let at_screen_center = capture(cx, cy, 1, 1)[0];
                check("auto monitor: crosshair sits at the focused window's viewport center", near(at_window_center, (255, 0, 0), 3) && !near(at_screen_center, (255, 0, 0), 3), format!("window center {at_window_center:?}, screen center {at_screen_center:?}"));
            }
            DestroyWindow(fg);
        }

        // wave 1: the new shapes, offset, mirror and per-layer opacity, drawn by the real compositor
        if go("shapes") {
            recolor(backdrop, rgb(0, 0, 0));
            let _r = launch(&dir, &base_cfg("oc2 chv.ff0000.5.2.1.1.270 lad.00ff00.4.3.1.4.90 dot.0000ff.1.4.1.4.0.x7.mh dot.ffffff.1.4.1.4.0.y-7.o5", ""));
            pump(1400);
            let px = |dx: i32, dy: i32| capture(cx + dx, cy + dy, 1, 1)[0];
            let red = |p: (u8, u8, u8)| near(p, (255, 0, 0), 3);
            check("chevron: tip and arm are lit, the inside of the V is not", red(px(0, -2)) && red(px(2, 0)) && !red(px(0, -1)), format!("tip {:?} arm {:?} inside {:?}", px(0, -2), px(2, 0), px(0, -1)));
            let green = |p: (u8, u8, u8)| near(p, (0, 255, 0), 3);
            check("ladder: a tick three below spans 4 each way and no more", green(px(0, 3)) && green(px(4, 3)) && !green(px(5, 3)) && !green(px(0, 4)), format!("{:?} {:?} {:?} {:?}", px(0, 3), px(4, 3), px(5, 3), px(0, 4)));
            let blue = |p: (u8, u8, u8)| near(p, (0, 0, 255), 3);
            check("offset and mirror: one dot shifted 7 and reflected to the other side", blue(px(7, 0)) && blue(px(-7, 0)) && !blue(px(0, 0)), format!("right {:?} left {:?} center {:?}", px(7, 0), px(-7, 0), px(0, 0)));
            let dim = px(0, -7);
            check("per-layer opacity: a 50% white dot over black is mid gray", near(dim, (127, 127, 127), 4), format!("{dim:?}"));
        }

        // hue cycle: a fast cycling layer changes color over time and stays vivid; Reduce motion freezes it
        if go("cycle") {
            recolor(backdrop, rgb(0, 0, 0));
            let _r = launch(&dir, &base_cfg("oc2 dot.ff0000.5.0.1.1.0.c3", ""));
            pump(1200);
            let a = capture(cx, cy, 1, 1)[0];
            pump(250);
            let b = capture(cx, cy, 1, 1)[0];
            pump(250);
            let c = capture(cx, cy, 1, 1)[0];
            let vivid = |p: (u8, u8, u8)| p.0.max(p.1).max(p.2) >= 245 && p.0.min(p.1).min(p.2) <= 10;
            check("hue cycle: the color keeps moving and stays vivid", a != b && b != c && vivid(a) && vivid(b) && vivid(c), format!("{a:?} -> {b:?} -> {c:?}"));
            drop(_r);
            let _r = launch(&dir, &base_cfg("oc2 dot.ff0000.5.0.1.1.0.c3", "reduce_motion=1"));
            pump(1200);
            let a = capture(cx, cy, 1, 1)[0];
            pump(400);
            let b = capture(cx, cy, 1, 1)[0];
            check("hue cycle: reduce motion holds the base color", near(a, (255, 0, 0), 3) && near(b, (255, 0, 0), 3), format!("{a:?} then {b:?}"));
        }

        // morph: flipping to ADS crossfades through in-between colors; with morph=0 it is a cut
        if go("morph") {
            recolor(backdrop, rgb(0, 0, 0));
            let cfg = |ms: u32| base_cfg("oc2 dot.00ff00.9.0.1.1.0", &format!("ads.layers=oc2 dot.0000ff.9.0.1.1.0\nmorph={ms}"));
            let is_green = |p: (u8, u8, u8)| near(p, (0, 255, 0), 6);
            let is_blue = |p: (u8, u8, u8)| near(p, (0, 0, 255), 6);
            for (ms, name) in [(250u32, "morph 250 ms"), (0, "morph off")] {
                let _r = launch(&dir, &cfg(ms));
                pump(1200);
                let start = capture(cx, cy, 1, 1)[0];
                key_tap(0x77); // F8 flips to ADS
                let t0 = Instant::now();
                let mut samples = Vec::new();
                while t0.elapsed() < Duration::from_millis(700) {
                    samples.push(capture(cx, cy, 1, 1)[0]);
                    std::thread::sleep(Duration::from_millis(3));
                }
                let end = *samples.last().unwrap();
                let mid = samples.iter().filter(|p| p.1 > 25 && p.2 > 25).count();
                let monotone = samples.windows(2).all(|w| w[1].2 as i32 >= w[0].2 as i32 - 2 && w[1].1 as i32 <= w[0].1 as i32 + 2);
                if ms == 0 {
                    check(&format!("{name}: a cut, never an in-between color"), is_green(start) && is_blue(end) && mid == 0, format!("{start:?} -> {end:?}, {mid} in-between of {}", samples.len()));
                } else {
                    check(&format!("{name}: green fades smoothly to blue"), is_green(start) && is_blue(end) && mid >= 4 && monotone, format!("{start:?} -> {end:?}, {mid} in-between of {}, monotone {monotone}", samples.len()));
                }
                key_tap(0x77);
                pump(500);
            }
        }

        // input states: a bound key widens the crosshair while held. F15 is used because nothing reacts to it.
        if go("states") {
            recolor(backdrop, rgb(0, 0, 0));
            let _r = launch(&dir, &base_cfg("oc2 spk.ffffff.4.3.1.4.0", "state.sprint.key=0x7E\nstate.sprint.mode=hold\nstate.sprint.gap=5\nstate.sprint.size=0"));
            pump(1400);
            let lit = |dx: i32| capture(cx + dx, cy, 1, 1)[0].0 > 240;
            let rest = (lit(5), lit(10));
            key_event(0x7E, false);
            pump(300);
            let held = (lit(5), lit(10));
            key_event(0x7E, true);
            pump(300);
            let after = (lit(5), lit(10));
            check("states: sprint key held moves the spokes 5 pixels out, release brings them back", rest == (true, false) && held == (false, true) && after == (true, false), format!("rest {rest:?}, held {held:?}, released {after:?} (inner, outer lit)"));
        }

        // auto-update, against a fake release server on this machine (this needs the test build, which can
        // talk plain http to localhost; the real exe only ever talks to github.com over https)
        if go("update") {
            let udir = std::env::temp_dir().join("redraw-e2e-update");
            let original = std::fs::read(&exe).unwrap();
            let mut newer = original.clone();
            newer.extend_from_slice(b"reDraw e2e update payload"); // still a valid program, but different bytes
            let attempt = |version: &str, served: &[u8], hashed: &[u8], patience_ms: u64| -> (Vec<u8>, bool, Vec<String>, bool) {
                let _ = std::fs::remove_dir_all(&udir);
                std::fs::create_dir_all(&udir).unwrap();
                std::fs::copy(&exe, udir.join("redraw.exe")).unwrap();
                std::fs::write(udir.join("redraw.cfg"), "trigger=key\nkey=F8\nautoupdate=1\nhip.layers=oc1 -dot.ffffff.1.0.1.1.0\ndisplay.monitor=primary\n").unwrap();
                let (ver, served_v, hashed_v) = (version.to_string(), served.to_vec(), hashed.to_vec());
                let (port, asked) = serve(move |port| {
                    let manifest = format!("version={ver}\nsha256={}\nsize={}\nurl=http://127.0.0.1:{port}/redraw.exe\n", hex(&update::sha256(&hashed_v)), hashed_v.len());
                    vec![("/latest.txt".to_string(), manifest.into_bytes()), ("/redraw.exe".to_string(), served_v)]
                });
                std::env::set_var("REDRAW_UPDATE_BASE", format!("http://127.0.0.1:{port}"));
                let mut child = Command::new(udir.join("redraw.exe")).current_dir(&udir).spawn().expect("start redraw.exe");
                std::env::remove_var("REDRAW_UPDATE_BASE");
                let start = Instant::now();
                while start.elapsed() < Duration::from_millis(patience_ms) {
                    pump(200);
                    if std::fs::read(udir.join("redraw.exe")).is_ok_and(|b| b != original) {
                        pump(300);
                        break;
                    }
                }
                let alive = child.try_wait().ok().flatten().is_none();
                let current = std::fs::read(udir.join("redraw.exe")).unwrap_or_default();
                let old_kept = udir.join("redraw.exe.old").exists();
                let requests = asked.lock().unwrap().clone();
                let _ = child.kill();
                let _ = child.wait();
                (current, old_kept, requests, alive)
            };

            let (current, old_kept, requests, alive) = attempt("9.9.9", &newer, &newer, 10_000);
            check("update: a newer release replaces the exe even while it is running", current == newer && alive, format!("replaced {}, still running {alive}", current == newer));
            check("update: and the previous version is kept as redraw.exe.old", old_kept && std::fs::read(udir.join("redraw.exe.old")).is_ok_and(|b| b == original) && !udir.join("redraw.exe.new").exists(), format!("old kept {old_kept}"));
            check("update: it read latest.txt, then the exe, nothing else", requests == ["/latest.txt", "/redraw.exe"], format!("{requests:?}"));

            let (current, old_kept, requests, alive) = attempt("0.0.1", &newer, &newer, 3_500);
            check("update: an older or equal release downloads nothing and changes nothing", current == original && !old_kept && requests == ["/latest.txt"] && alive, format!("{requests:?}, untouched {}", current == original));

            let mut tampered = newer.clone();
            tampered[4000] ^= 0xFF;
            let (current, old_kept, requests, alive) = attempt("9.9.9", &tampered, &newer, 4_500);
            check("update: a download that does not match its checksum is never installed", current == original && !old_kept && requests.len() == 2 && alive, format!("{requests:?}, untouched {}", current == original));
            let _ = std::fs::remove_dir_all(&udir);
        }

        // ForceQuit: the red button at the very bottom of the menu closes the program, for certain.
        if go("forcequit") {
            let _r = launch(&dir, &base_cfg("oc1 dot.ff0000.9.0.1.1.0", "trigger=hold"));
            let mut _r = _r;
            pump(1200);
            menu_tap();
            pump(700);
            let menu = find_menu(_r.child.id());
            let foreign = foreign_menu_open(_r.child.id());
            let mut rc: RECT = std::mem::zeroed();
            if foreign {
                println!("SKIP this menu check: another reDraw menu (a real copy, not the one under test) is open, and a pointer test must not touch it");
            } else if menu.is_null() || IsWindowVisible(menu) == 0 || GetWindowRect(menu, &mut rc) == 0 {
                check("forcequit: the menu opens", false, "menu window not visible".to_string());
            } else {
                let s = ((rc.right - rc.left) / 212).max(1);
                // Find the red button by its color, wherever the menu puts it (it is the last row).
                let (rw, rh) = (212 * s, 700);
                let shot = capture(rc.left, rc.top, rw, rh);
                let reds: Vec<(i32, i32)> = (0..rw * rh).filter(|i| near(shot[*i as usize], (255, 77, 77), 12)).map(|i| (i % rw, i / rw)).collect();
                check("forcequit: a red label sits at the bottom of the menu", reds.len() > 20, format!("{} red px", reds.len()));
                let (x0, x1) = (reds.iter().map(|p| p.0).min().unwrap_or(0), reds.iter().map(|p| p.0).max().unwrap_or(0));
                let (y0, y1) = (reds.iter().map(|p| p.1).min().unwrap_or(0), reds.iter().map(|p| p.1).max().unwrap_or(0));
                println!("   forcequit button found at menu pixels ({x0},{y0})-({x1},{y1})");
                let mut saved = POINT { x: 0, y: 0 };
                GetCursorPos(&mut saved);
                SetCursorPos(rc.left + (x0 + x1) / 2, rc.top + (y0 + y1) / 2);
                pump(150);
                let alive_before = _r.child.try_wait().ok().flatten().is_none();
                left_button(true);
                pump(60);
                left_button(false);
                let t = Instant::now();
                let mut gone = false;
                while t.elapsed() < Duration::from_millis(2500) {
                    pump(50);
                    if _r.child.try_wait().ok().flatten().is_some() {
                        gone = true;
                        break;
                    }
                }
                SetCursorPos(saved.x, saved.y);
                check("forcequit: clicking it closes the program", alive_before && gone, format!("running before {alive_before}, gone after {gone} in {} ms", t.elapsed().as_millis()));
                pump(300);
                let crosshair_gone = near(capture(cx, cy, 1, 1)[0], back, 3) || capture(cx, cy, 1, 1)[0] != (255, 0, 0);
                check("forcequit: and the crosshair is gone from the screen", crosshair_gone, format!("{:?}", capture(cx, cy, 1, 1)[0]));
            }
        }

        // Restart now: the new copy waits for the one that started it to be gone before it starts.
        if go("restart") {
            recolor(backdrop, rgb(back.0, back.1, back.2));
            std::fs::write(dir.join("redraw.cfg"), "trigger=key\nkey=F8\nhip.outline=0\nhip.opacity=10\nhip.layers=oc1 dot.ff0000.9.0.1.1.0\ndisplay.monitor=primary\ndisplay.scale=1\nautoupdate=0\n").unwrap();
            // a stand-in "old copy": a process that lives for about 3 seconds
            let mut old = Command::new("ping").args(["-n", "4", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn().expect("ping");
            let child = Command::new(dir.join("redraw.exe")).current_dir(&dir).arg("--restart-from").arg(old.id().to_string()).spawn().expect("start redraw.exe");
            let _r = Run { child };
            pump(1200);
            let early = capture(cx, cy, 1, 1)[0];
            let _ = old.wait();
            pump(1500);
            let late = capture(cx, cy, 1, 1)[0];
            check("restart: the new copy holds back while the old one is still running", near(early, back, 3), format!("{early:?} want backdrop {back:?}"));
            check("restart: and takes over as soon as the old one is gone", near(late, (255, 0, 0), 3), format!("{late:?}"));
        }

        DestroyWindow(backdrop);
    }
    println!("{}", if failures == 0 { "ALL PASSED" } else { "SOME FAILED" });
    std::process::exit(if failures == 0 { 0 } else { 1 });
}
