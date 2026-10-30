//! A frame clock tied to the display: a small thread waits for the desktop
//! compositor's next refresh (`DwmFlush`) and posts one message per refresh to
//! the overlay window. It runs only while something is animating, so a resting
//! crosshair costs nothing, and animation is paced to the monitor (60, 144,
//! 240 Hz) instead of a coarse timer.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Dwm::DwmFlush;
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_FRAME: u32 = WM_APP + 1;

pub struct FramePacer {
    hwnd: usize,
    /// set by the pacer thread, cleared by the window when it handles the frame,
    /// so a busy window never has a backlog of frame messages
    pending: Arc<AtomicBool>,
    /// one flag per started thread; stopping drops it and the thread exits at its next refresh
    run: Option<Arc<AtomicBool>>,
    /// microseconds between frames; 0 means one per display refresh
    period_us: Arc<AtomicU32>,
}

impl FramePacer {
    pub fn new(hwnd: HWND) -> FramePacer {
        FramePacer { hwnd: hwnd as usize, pending: Arc::new(AtomicBool::new(false)), run: None, period_us: Arc::new(AtomicU32::new(0)) }
    }

    /// At most one frame per `period` instead of one per display refresh, for work that does not
    /// need the monitor's full rate (a lens refreshed 60 times a second on a 360 Hz display). Frames
    /// still land right after a compositor refresh: that timing matters, because a lens refreshed at
    /// a random moment makes the compositor miss its frame about five times as often (measured with
    /// `examples/lagprobe.rs`). `None` goes back to one per refresh.
    pub fn set_period(&self, period: Option<Duration>) {
        self.period_us.store(period.map_or(0, |p| p.as_micros().min(u32::MAX as u128) as u32), Ordering::Release);
    }

    pub fn running(&self) -> bool {
        self.run.is_some()
    }

    /// Start posting `WM_FRAME` once per display refresh (no-op if already running).
    pub fn start(&mut self) {
        if self.run.is_some() {
            return;
        }
        let run = Arc::new(AtomicBool::new(true));
        let (flag, pending, hwnd, period) = (run.clone(), self.pending.clone(), self.hwnd, self.period_us.clone());
        self.run = Some(run);
        std::thread::spawn(move || {
            let mut last_post = Instant::now();
            while flag.load(Ordering::Acquire) {
                // DwmFlush blocks until the next composition. If the compositor is
                // unavailable it returns an error at once; fall back to ~120 Hz.
                if unsafe { DwmFlush() } != 0 {
                    std::thread::sleep(Duration::from_millis(8));
                }
                let slow = period.load(Ordering::Acquire);
                if slow > 0 && last_post.elapsed() < Duration::from_micros(slow as u64 * 9 / 10) {
                    continue;
                }
                if flag.load(Ordering::Acquire) && !pending.swap(true, Ordering::AcqRel) {
                    last_post = Instant::now();
                    unsafe { PostMessageW(hwnd as HWND, WM_FRAME, 0, 0) };
                }
            }
        });
    }

    pub fn stop(&mut self) {
        if let Some(run) = self.run.take() {
            run.store(false, Ordering::Release);
        }
    }

    /// The window handled a frame: allow the next one.
    pub fn ack(&self) {
        self.pending.store(false, Ordering::Release);
    }
}

impl Drop for FramePacer {
    fn drop(&mut self) {
        self.stop();
    }
}
