//! Finds monitors and the focused window's viewport. Uses only window and
//! monitor geometry that Windows hands out to any program (monitor rectangles,
//! DPI, refresh rate, and the position and size of the focused window). It does
//! not open the game's process or read anything from it.

use std::ptr::null;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::HiDpi::GetDpiForMonitor;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::display::{follow_center, Area, MonitorSel};

#[derive(Clone, Copy, Debug)]
pub struct Monitor {
    pub rect: Area,
    pub primary: bool,
    pub dpi: u32,
    pub refresh: u32,
}

/// Where the crosshair center goes, and the monitor it is on.
#[derive(Clone, Copy, Debug)]
pub struct Target {
    pub center: (i32, i32),
    pub monitor: Monitor,
}

fn area(r: RECT) -> Area {
    Area { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top }
}

unsafe fn info_of(hmon: HMONITOR) -> Option<Monitor> {
    let mut mi: MONITORINFOEXW = std::mem::zeroed();
    mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    if GetMonitorInfoW(hmon, &mut mi as *mut MONITORINFOEXW as *mut MONITORINFO) == 0 {
        return None;
    }
    let (mut dx, mut dy) = (96u32, 96u32);
    if GetDpiForMonitor(hmon, 0, &mut dx, &mut dy) != 0 {
        dx = 96;
    }
    let mut dm: DEVMODEW = std::mem::zeroed();
    dm.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
    let refresh = if EnumDisplaySettingsW(mi.szDevice.as_ptr(), ENUM_CURRENT_SETTINGS, &mut dm) != 0 && dm.dmDisplayFrequency > 1 { dm.dmDisplayFrequency } else { 60 };
    Some(Monitor { rect: area(mi.monitorInfo.rcMonitor), primary: mi.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0, dpi: dx.max(96), refresh })
}

unsafe extern "system" fn collect(hmon: HMONITOR, _dc: HDC, _rect: *mut RECT, lp: LPARAM) -> BOOL {
    let list = &mut *(lp as *mut Vec<Monitor>);
    if let Some(m) = info_of(hmon) {
        list.push(m);
    }
    1
}

/// All monitors, left to right (then top to bottom). Never empty.
pub fn list() -> Vec<Monitor> {
    let mut v: Vec<Monitor> = Vec::new();
    unsafe { EnumDisplayMonitors(std::ptr::null_mut(), null(), Some(collect), &mut v as *mut Vec<Monitor> as LPARAM) };
    v.sort_by_key(|m| (m.rect.x, m.rect.y));
    if v.is_empty() {
        let (w, h) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        v.push(Monitor { rect: Area { x: 0, y: 0, w, h }, primary: true, dpi: 96, refresh: 60 });
    }
    v
}

pub fn primary() -> Monitor {
    let all = list();
    all.iter().copied().find(|m| m.primary).unwrap_or(all[0])
}

fn on_monitor(m: Monitor) -> Target {
    Target { center: m.rect.center(), monitor: m }
}

/// The focused window's viewport (see `display::follow_center`). `None` means "keep
/// what you had": the focused window is ours, or there is none.
fn follow_focus(own_pid: u32) -> Option<Target> {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(fg, &mut pid);
        if pid == own_pid {
            return None;
        }
        let mut class = [0u16; 128];
        let n = GetClassNameW(fg, class.as_mut_ptr(), 128).max(0) as usize;
        let class = String::from_utf16_lossy(&class[..n]);
        let hmon = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
        let monitor = info_of(hmon)?;
        // The desktop and taskbar are not a game: use the primary monitor's center.
        if matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
            return Some(on_monitor(primary()));
        }
        if IsIconic(fg) != 0 || IsWindowVisible(fg) == 0 {
            return Some(on_monitor(monitor));
        }
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(fg, &mut rc);
        let mut origin = POINT { x: 0, y: 0 };
        ClientToScreen(fg, &mut origin);
        let client = Area { x: origin.x, y: origin.y, w: rc.right - rc.left, h: rc.bottom - rc.top };
        Some(Target { center: follow_center(client, monitor.rect), monitor })
    }
}

/// Resolve the Monitor setting. `last` is what Follow keeps while our own window has focus.
pub fn resolve(sel: MonitorSel, own_pid: u32, last: Option<Target>) -> Target {
    match sel {
        MonitorSel::Primary => on_monitor(primary()),
        MonitorSel::Nth(n) => {
            let all = list();
            on_monitor(all.get(n as usize - 1).copied().unwrap_or_else(|| all.iter().copied().find(|m| m.primary).unwrap_or(all[0])))
        }
        MonitorSel::Follow => follow_focus(own_pid).or(last).unwrap_or_else(|| on_monitor(primary())),
    }
}
