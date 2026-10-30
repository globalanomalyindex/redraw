//! Lag probe (test tooling, ships nothing): runs the real redraw.exe in each blend mode over an
//! animated stand-in for a game and measures what the desktop compositor and the machine feel:
//!   - redraw.exe's own CPU,
//!   - the compositor's late / dropped / missed frame counts (DwmGetCompositionTimingInfo),
//!   - the cadence of the desktop refresh as a probe thread sees it (DwmFlush intervals).
//! Run: cargo run --release --example lagprobe -- <path to redraw.exe> [only-substring]
//! It never touches the real redraw.cfg: each run uses a copy of the exe in a temp folder.

use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Dwm::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetProcessTimes;
use windows_sys::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
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

fn ft(f: FILETIME) -> f64 {
    (((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64) as f64 / 10_000.0 // ms
}

unsafe fn cpu_ms(child: &Child) -> f64 {
    let (mut c, mut e, mut k, mut u): (FILETIME, FILETIME, FILETIME, FILETIME) = std::mem::zeroed();
    GetProcessTimes(child.as_raw_handle() as HANDLE, &mut c, &mut e, &mut k, &mut u);
    ft(k) + ft(u)
}

unsafe fn timing() -> DWM_TIMING_INFO {
    let mut t: DWM_TIMING_INFO = std::mem::zeroed();
    t.cbSize = std::mem::size_of::<DWM_TIMING_INFO>() as u32;
    DwmGetCompositionTimingInfo(null_mut(), &mut t);
    t
}

fn pct(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[(((sorted.len() - 1) as f64) * p).round() as usize]
}

struct Scenario {
    name: &'static str,
    layers: &'static str,
    extra: &'static str,
}

const BASE: &str = "spk.ffffff.6.4.3.4.0";
const SCENARIOS: &[Scenario] = &[
    Scenario { name: "nothing running", layers: "", extra: "" },
    Scenario { name: "normal", layers: "oc2 spk.ffffff.6.4.3.4.0 dot.ffffff.1.4.1.4.0", extra: "" },
    // The frame clock alone: a slow hue cycle runs the clock at the display rate but redraws ~5 times a second.
    Scenario { name: "frame clock only", layers: "oc2 spk.ffffff.6.4.3.4.0.c1 dot.ffffff.1.4.1.4.0", extra: "" },
    Scenario { name: "glow", layers: "oc2 spk.ffffff.6.4.3.4.0~g dot.ffffff.1.4.1.4.0~g", extra: "" },
    Scenario { name: "invert", layers: "oc2 spk.ffffff.6.4.3.4.0~i dot.ffffff.1.4.1.4.0~i", extra: "" },
    Scenario { name: "contrast", layers: "oc2 spk.ffffff.6.4.3.4.0~c dot.ffffff.1.4.1.4.0~c", extra: "" },
    Scenario { name: "gray", layers: "oc2 spk.ffffff.6.4.3.4.0~y dot.ffffff.1.4.1.4.0~y", extra: "" },
    Scenario { name: "contrast, your capture=0", layers: "oc2 spk.ffffff.6.4.3.4.0~c dot.ffffff.1.4.1.4.0~c", extra: "capture=0" },
    Scenario { name: "gray, your capture=0", layers: "oc2 spk.ffffff.6.4.3.4.0~y dot.ffffff.1.4.1.4.0~y", extra: "capture=0" },
    Scenario { name: "contrast, scale 3", layers: "oc2 spk.ffffff.6.4.3.4.0~c dot.ffffff.1.4.1.4.0~c", extra: "display.scale=3" },
    Scenario { name: "invert+contrast+gray", layers: "oc2 spk.ffffff.6.4.3.4.0~i ring.ffffff.9.0.1.1.0~c dot.ffffff.1.4.1.4.0~y", extra: "" },
];

fn main() {
    let _ = BASE;
    let exe = PathBuf::from(std::env::args().nth(1).expect("usage: lagprobe <path to redraw.exe> [only]"));
    let only = std::env::args().nth(2).unwrap_or_default();
    let dir = std::env::temp_dir().join("redraw-lagprobe");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(&exe, dir.join("redraw.exe")).unwrap();
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let class = wide("LagBack");
        let wc = WNDCLASSEXW { cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32, style: 0, lpfnWndProc: Some(DefWindowProcW), cbClsExtra: 0, cbWndExtra: 0, hInstance: GetModuleHandleW(null()), hIcon: null_mut(), hCursor: null_mut(), hbrBackground: null_mut(), lpszMenuName: null(), lpszClassName: class.as_ptr(), hIconSm: null_mut() };
        assert!(RegisterClassExW(&wc) != 0);
        let (cx, cy) = (GetSystemMetrics(SM_CXSCREEN) / 2, GetSystemMetrics(SM_CYSCREEN) / 2);
        // The animated stand-in for a game: repaints constantly under the crosshair.
        let back = CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, class.as_ptr(), null(), WS_POPUP | WS_VISIBLE, cx - 300, cy - 300, 600, 600, null_mut(), null_mut(), GetModuleHandleW(null()), null());
        let dc = GetDC(back);
        let mut refresh_hz = 0.0;
        let mut rows: Vec<(String, f64, f64)> = Vec::new();

        println!("{:<26} {:>7} {:>8} {:>8} {:>8} {:>8} {:>7} {:>7} {:>7} {:>8}", "scenario", "cpu %", "p50 ms", "p95 ms", "p99 ms", "max ms", "late/s", "drop/s", "miss/s", "frames/s");
        for sc in SCENARIOS.iter().filter(|s| only.is_empty() || s.name.contains(only.as_str())) {
            let _run = if sc.layers.is_empty() {
                None
            } else {
                std::fs::write(dir.join("redraw.cfg"), format!("trigger=key\nkey=F8\nhip.outline=0\nhip.opacity=10\nhip.layers={}\nads.layers=oc1 dot.ffffff.1.0.1.1.0\ndisplay.monitor=primary\ndisplay.scale=1\noffset=150,150\n{}\n", sc.layers, sc.extra)).unwrap();
                Some(Run { child: Command::new(dir.join("redraw.exe")).current_dir(&dir).spawn().expect("start redraw.exe") })
            };
            let mut tick = 0u32;
            let mut animate = |ms: u64| {
                let end = Instant::now() + Duration::from_millis(ms);
                while Instant::now() < end {
                    let mut msg: MSG = std::mem::zeroed();
                    while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                        TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                    tick = tick.wrapping_add(1);
                    // slow color drift (easy on the eyes); the bars still move every frame
                    let c = ((tick / 16) % 256) as u8;
                    let brush = CreateSolidBrush(c as u32 | ((255 - c) as u32) << 8 | ((tick / 24 % 256) as u32) << 16);
                    let x = (tick as i32 * 5) % 500;
                    FillRect(dc, &RECT { left: x, top: 0, right: x + 100, bottom: 600 }, brush);
                    FillRect(dc, &RECT { left: 0, top: (x * 3 / 5) % 500, right: 600, bottom: (x * 3 / 5) % 500 + 100 }, brush);
                    DeleteObject(brush);
                    std::thread::sleep(Duration::from_millis(2));
                }
            };
            animate(1800); // warm up: window created, frame clock running
            let mut early = None;
            if let Some(r) = _run.as_ref() {
                // A copy that quit right away (another instance holding the lock, bad config) would read as "free".
                let child = &r.child as *const Child as *mut Child;
                if let Ok(Some(status)) = (*child).try_wait() {
                    early = Some(status);
                }
            }
            if let Some(status) = early {
                println!("{:<26} EXITED EARLY ({status}): this row measures nothing", sc.name);
                continue;
            }
            let gaps = Arc::new(Mutex::new(Vec::<f64>::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let probe = {
                let (gaps, stop) = (gaps.clone(), stop.clone());
                std::thread::spawn(move || {
                    let mut last = Instant::now();
                    while !stop.load(Ordering::Relaxed) {
                        DwmFlush();
                        let now = Instant::now();
                        gaps.lock().unwrap().push(now.duration_since(last).as_secs_f64() * 1000.0);
                        last = now;
                    }
                })
            };
            let (t0, c0) = (timing(), _run.as_ref().map_or(0.0, |r| cpu_ms(&r.child)));
            let start = Instant::now();
            animate(5000);
            let secs = start.elapsed().as_secs_f64();
            let (t1, c1) = (timing(), _run.as_ref().map_or(0.0, |r| cpu_ms(&r.child)));
            stop.store(true, Ordering::Relaxed);
            probe.join().unwrap();
            let mut g = gaps.lock().unwrap().clone();
            g.sort_by(|a, b| a.partial_cmp(b).unwrap());
            if refresh_hz == 0.0 && t1.rateRefresh.uiDenominator != 0 {
                refresh_hz = t1.rateRefresh.uiNumerator as f64 / t1.rateRefresh.uiDenominator as f64;
            }
            // how often the refresh hiccups: intervals longer than 1.5 refresh periods
            let slow = 100.0 * g.iter().filter(|x| **x > 4.2).count() as f64 / g.len().max(1) as f64;
            print!("slow {slow:>5.1}%  ");
            println!(
                "{:<26} {:>7.2} {:>8.2} {:>8.2} {:>8.2} {:>8.2} {:>7.1} {:>7.1} {:>7.1} {:>8.0}",
                sc.name,
                100.0 * (c1 - c0) / (secs * 1000.0),
                pct(&g, 0.5),
                pct(&g, 0.95),
                pct(&g, 0.99),
                g.last().copied().unwrap_or(0.0),
                (t1.cFramesLate - t0.cFramesLate) as f64 / secs,
                (t1.cFramesDropped - t0.cFramesDropped) as f64 / secs,
                (t1.cFramesMissed - t0.cFramesMissed) as f64 / secs,
                (t1.cFrame - t0.cFrame) as f64 / secs,
            );
            rows.push((sc.name.to_string(), 100.0 * (c1 - c0) / (secs * 1000.0), pct(&g, 0.99)));
            drop(_run);
            animate(600);
        }
        println!("compositor refresh rate: {refresh_hz:.0} Hz");
        if std::env::var("LAGPROBE_CHECK").is_ok() {
            // Verdict: every lens row must stay close to the no-lens row, and cheap. One command, red or green.
            let base = rows.iter().find(|r| r.0 == "normal").map(|r| r.2).unwrap_or(0.0);
            let mut bad = 0;
            // One lens kind at a time; the cost grows with how many kinds are on at once, so the
            // three-at-once row is printed above for reference but not judged.
            for (name, cpu, p99) in rows.iter().filter(|r| !r.0.contains('+') && ["invert", "contrast", "gray"].iter().any(|k| r.0.starts_with(k))) {
                let ok = *cpu < 3.0 && *p99 <= base + 0.8;
                println!("{} {name}: {cpu:.2}% cpu (limit 3.0), p99 {p99:.2} ms (limit {:.2} = no-lens {base:.2} + 0.8)", if ok { "PASS" } else { "FAIL" }, base + 0.8);
                if !ok {
                    bad += 1;
                }
            }
            println!("{}", if bad == 0 { "LENS COST OK" } else { "LENS COST TOO HIGH" });
            std::process::exit(if bad == 0 { 0 } else { 1 });
        }
        ReleaseDC(back, dc);
        DestroyWindow(back);
    }
}
