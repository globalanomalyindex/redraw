//! Lens windows, built on the Windows Magnification API (the accessibility
//! magnifier). Windows itself copies and transforms the screen region; this
//! program never sees those pixels. Proved on the real compositor by
//! `examples/spike.rs`:
//!   - a color matrix plus a window shape shows the backdrop inverted (or
//!     boosted, or gray) only inside the crosshair's shape (Lens blend);
//!   - a magnification transform scales the source region (Scope zoom).
//! The windows are click-through and top-most, and the overlay's own windows
//! are on the magnifier's exclusion list so nothing feeds back.

use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::Magnification::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::area::Geometry;
use crate::crosshair::{Blend, N};

const WDA_EXCLUDEFROMCAPTURE: u32 = 0x11;

/// The most often the lenses re-read the screen. Windows re-captures and repaints a lens every time
/// its source is set. Doing that on every refresh of a fast monitor (360 Hz here) cost several percent
/// of a core and made the compositor's frame timing worse (measured with `examples/lagprobe.rs`);
/// at 60 Hz the cost is mostly gone, and on a crosshair-sized patch 16 ms of staleness is invisible.
pub const LENS_HZ: u32 = 60;

/// The lens refresh rate for a frame-rate setting: `LENS_HZ`, or lower if the cap is lower.
pub fn lens_hz(fps_cap: Option<u32>) -> u32 {
    fps_cap.filter(|f| *f > 0).map_or(LENS_HZ, |f| f.min(LENS_HZ)).max(15)
}

/// Lets an action through at most `hz` times a second. Pure: the caller supplies the time.
#[derive(Debug)]
pub struct RateGate {
    period: Duration,
    last: Option<Instant>,
}

impl RateGate {
    pub fn new(hz: u32) -> RateGate {
        let mut g = RateGate { period: Duration::ZERO, last: None };
        g.set_hz(hz);
        g
    }

    pub fn set_hz(&mut self, hz: u32) {
        self.period = Duration::from_secs_f64(1.0 / hz.max(1) as f64);
    }

    /// Is the action due at `now`? If so, it is counted as done.
    pub fn allow(&mut self, now: Instant) -> bool {
        match self.last {
            Some(t) if now.saturating_duration_since(t) < self.period => false,
            _ => {
                self.last = Some(now);
                true
            }
        }
    }

    /// The action was done by other means at `now`.
    pub fn mark(&mut self, now: Instant) {
        self.last = Some(now);
    }
}

/// A magnified area (Scope zoom, or Aspect ratio): centered on `center`, shaped and scaled by `geo`.
pub struct AreaSpec {
    pub center: POINT,
    pub geo: Geometry,
    /// an ellipse inside its rectangle, instead of the whole rectangle
    pub ellipse: bool,
}

/// How many magnified areas there can be at once: Scope zoom, and Aspect ratio.
pub const AREAS: usize = 2;

pub struct Spec<'a> {
    /// screen position of the crosshair's grid, and the whole-number scale it is drawn at
    pub crosshair_pos: POINT,
    pub scale: i32,
    pub effects: &'a [(Blend, [bool; N * N])],
    pub areas: [Option<AreaSpec>; AREAS],
    /// windows of ours that must not appear inside a magnified view
    pub exclude: &'a [HWND],
    pub hide_from_capture: bool,
}

struct LensWin {
    host: HWND,
    mag: HWND,
    w: i32,
    h: i32,
    pos: POINT,
    shape: Vec<bool>,
    ellipse: bool,
    src: RECT,
    /// the horizontal and vertical scale last set
    scale: (f32, f32),
    effect: Option<Blend>,
}

pub struct Lenses {
    hinstance: HINSTANCE,
    class: Vec<u16>,
    inited: bool,
    failed: bool,
    cross: Vec<LensWin>,
    areas: [Option<LensWin>; AREAS],
    filter_key: Vec<usize>,
    capture_hidden: Option<bool>,
    gate: RateGate,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn matrix(effect: Blend) -> [f32; 25] {
    match effect {
        // output = 1 - input
        Blend::Invert => [-1.0, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 0.0, 1.0],
        // contrast x1.8 around mid-gray
        Blend::Contrast => [1.8, 0.0, 0.0, 0.0, 0.0, 0.0, 1.8, 0.0, 0.0, 0.0, 0.0, 0.0, 1.8, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, -0.4, -0.4, -0.4, 0.0, 1.0],
        // every channel = luma
        _ => [0.299, 0.299, 0.299, 0.0, 0.0, 0.587, 0.587, 0.587, 0.0, 0.0, 0.114, 0.114, 0.114, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0],
    }
}

impl LensWin {
    unsafe fn create(hinstance: HINSTANCE, class: &[u16], w: i32, h: i32) -> Option<LensWin> {
        let host = CreateWindowExW(WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, class.as_ptr(), null(), WS_POPUP, 0, 0, w, h, null_mut(), null_mut(), hinstance, null());
        if host.is_null() {
            return None;
        }
        SetLayeredWindowAttributes(host, 0, 255, LWA_ALPHA);
        let mag = CreateWindowExW(0, WC_MAGNIFIERW, wide("MagnifierWindow").as_ptr(), WS_CHILD | WS_VISIBLE, 0, 0, w, h, host, null_mut(), hinstance, null());
        if mag.is_null() {
            DestroyWindow(host);
            return None;
        }
        ShowWindow(host, SW_SHOWNOACTIVATE);
        Some(LensWin { host, mag, w, h, pos: POINT { x: i32::MIN, y: 0 }, shape: Vec::new(), ellipse: false, src: RECT { left: 0, top: 0, right: w, bottom: h }, scale: (1.0, 1.0), effect: None })
    }

    unsafe fn place(&mut self, pos: POINT) {
        if self.pos.x != pos.x || self.pos.y != pos.y {
            SetWindowPos(self.host, HWND_TOPMOST, pos.x, pos.y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
            self.pos = pos;
        }
    }

    /// Scale what the magnifier shows by `sx` across and `sy` down (they may differ: that is Stretch).
    unsafe fn set_transform(&mut self, sx: f32, sy: f32) {
        if (self.scale.0 - sx).abs() > 1e-4 || (self.scale.1 - sy).abs() > 1e-4 || self.effect.is_none() {
            let mut t = MAGTRANSFORM { v: [sx, 0.0, 0.0, 0.0, sy, 0.0, 0.0, 0.0, 1.0] };
            MagSetWindowTransform(self.mag, &mut t);
            self.scale = (sx, sy);
        }
    }

    unsafe fn destroy(&self) {
        DestroyWindow(self.host);
    }
}

/// The window shape for a crosshair mask drawn with each cell as a `k` x `k` block.
unsafe fn region_from_mask(mask: &[bool], k: i32) -> HRGN {
    let rgn = CreateRectRgn(0, 0, 0, 0);
    for y in 0..N {
        let mut x = 0;
        while x < N {
            if mask[y * N + x] {
                let start = x;
                while x < N && mask[y * N + x] {
                    x += 1;
                }
                let run = CreateRectRgn(start as i32 * k, y as i32 * k, x as i32 * k, (y as i32 + 1) * k);
                CombineRgn(rgn, rgn, run, RGN_OR);
                DeleteObject(run);
            } else {
                x += 1;
            }
        }
    }
    rgn
}

impl Lenses {
    pub fn new(hinstance: HINSTANCE) -> Lenses {
        let class = wide("reDrawLens");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(DefWindowProcW),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance,
            hIcon: null_mut(),
            hCursor: null_mut(),
            hbrBackground: null_mut(),
            lpszMenuName: null(),
            lpszClassName: class.as_ptr(),
            hIconSm: null_mut(),
        };
        unsafe { RegisterClassExW(&wc) };
        Lenses { hinstance, class, inited: false, failed: false, cross: Vec::new(), areas: [None, None], filter_key: Vec::new(), capture_hidden: None, gate: RateGate::new(LENS_HZ) }
    }

    /// True while a lens window exists and needs its source refreshed every frame.
    pub fn any(&self) -> bool {
        !self.cross.is_empty() || self.areas.iter().any(Option::is_some)
    }

    fn ensure_init(&mut self) -> bool {
        if !self.inited && !self.failed {
            self.inited = unsafe { MagInitialize() } != 0;
            self.failed = !self.inited;
        }
        self.inited
    }

    fn destroy_all(&mut self) {
        unsafe {
            for l in self.cross.drain(..) {
                l.destroy();
            }
            for slot in self.areas.iter_mut() {
                if let Some(a) = slot.take() {
                    a.destroy();
                }
            }
        }
        self.filter_key.clear();
        self.capture_hidden = None;
    }

    /// Make the lens windows match `spec`: create, move, reshape or remove them.
    pub fn update(&mut self, spec: &Spec) {
        if spec.effects.is_empty() && spec.areas.iter().all(Option::is_none) {
            if self.any() {
                self.destroy_all();
            }
            return;
        }
        if !self.ensure_init() {
            return;
        }
        let mut changed_set = false;
        let k = spec.scale.max(1);
        let size = N as i32 * k;
        unsafe {
            let before = self.cross.len();
            self.cross.retain(|l| {
                let keep = l.w == size && spec.effects.iter().any(|(e, _)| Some(*e) == l.effect);
                if !keep {
                    l.destroy();
                }
                keep
            });
            changed_set |= self.cross.len() != before;

            for (effect, mask) in spec.effects {
                let idx = match self.cross.iter().position(|l| l.effect == Some(*effect)) {
                    Some(i) => i,
                    None => {
                        let Some(mut w) = LensWin::create(self.hinstance, &self.class, size, size) else { continue };
                        let mut fx = MAGCOLOREFFECT { transform: matrix(*effect) };
                        MagSetColorEffect(w.mag, &mut fx);
                        w.effect = Some(*effect);
                        self.cross.push(w);
                        changed_set = true;
                        self.cross.len() - 1
                    }
                };
                let w = &mut self.cross[idx];
                w.place(spec.crosshair_pos);
                w.src = RECT { left: spec.crosshair_pos.x, top: spec.crosshair_pos.y, right: spec.crosshair_pos.x + size, bottom: spec.crosshair_pos.y + size };
                if w.shape.as_slice() != mask {
                    SetWindowRgn(w.host, region_from_mask(mask, k), 1);
                    w.shape = mask.to_vec();
                }
            }

            for (slot, want) in spec.areas.iter().enumerate() {
                match want {
                    None => {
                        if let Some(w) = self.areas[slot].take() {
                            w.destroy();
                            changed_set = true;
                        }
                    }
                    Some(a) => {
                        let g = &a.geo;
                        if self.areas[slot].as_ref().is_some_and(|w| w.w != g.w || w.h != g.h) {
                            self.areas[slot].take().map(|w| w.destroy());
                        }
                        if self.areas[slot].is_none() {
                            if let Some(w) = LensWin::create(self.hinstance, &self.class, g.w, g.h) {
                                self.areas[slot] = Some(w);
                                changed_set = true;
                            }
                        }
                        if let Some(w) = self.areas[slot].as_mut() {
                            w.place(POINT { x: a.center.x - g.w / 2, y: a.center.y - g.h / 2 });
                            w.set_transform(g.sx, g.sy);
                            let (left, top) = (a.center.x - g.src_w / 2, a.center.y - g.src_h / 2);
                            w.src = RECT { left, top, right: left + g.src_w, bottom: top + g.src_h };
                            if w.shape.is_empty() || w.ellipse != a.ellipse {
                                if a.ellipse {
                                    SetWindowRgn(w.host, CreateEllipticRgn(0, 0, g.w + 1, g.h + 1), 1);
                                } else {
                                    SetWindowRgn(w.host, null_mut(), 1);
                                }
                                w.ellipse = a.ellipse;
                                w.shape = vec![true];
                            }
                        }
                    }
                }
            }

            // Every lens hides the overlay's own windows and every lens window from itself.
            let mut all: Vec<HWND> = spec.exclude.to_vec();
            all.extend(self.cross.iter().map(|l| l.host));
            all.extend(self.areas.iter().flatten().map(|l| l.host));
            let key: Vec<usize> = all.iter().map(|h| *h as usize).collect();
            if changed_set || key != self.filter_key {
                for w in self.cross.iter().chain(self.areas.iter().flatten()) {
                    MagSetWindowFilterList(w.mag, MW_FILTERMODE_EXCLUDE, all.len() as i32, all.as_mut_ptr());
                }
                self.filter_key = key;
            }
            if changed_set || self.capture_hidden != Some(spec.hide_from_capture) {
                for w in self.cross.iter().chain(self.areas.iter().flatten()) {
                    SetWindowDisplayAffinity(w.host, if spec.hide_from_capture { WDA_EXCLUDEFROMCAPTURE } else { 0 });
                }
                self.capture_hidden = Some(spec.hide_from_capture);
            }
            // A change (new lens, moved, reshaped) shows at once; the cap only paces the steady refresh.
            self.refresh();
            self.gate.mark(Instant::now());
        }
    }

    /// Refresh what each lens shows. Call once per display frame while `any()`.
    pub fn tick(&mut self) {
        if self.gate.allow(Instant::now()) {
            self.refresh();
        }
    }

    /// Re-read the screen into every lens, right now.
    fn refresh(&self) {
        unsafe {
            for w in self.cross.iter().chain(self.areas.iter().flatten()) {
                MagSetWindowSource(w.mag, w.src);
            }
        }
    }

    /// Cap how often the lenses re-read the screen (see `LENS_HZ`).
    pub fn set_rate(&mut self, hz: u32) {
        self.gate.set_hz(hz);
    }
}

impl Drop for Lenses {
    fn drop(&mut self) {
        self.destroy_all();
        if self.inited {
            unsafe { MagUninitialize() };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Number of times the gate lets an action through when ticked at `clock_hz` for `secs` seconds.
    fn passes(gate: &mut RateGate, clock_hz: u32, secs: f64) -> usize {
        let start = Instant::now();
        let steps = (clock_hz as f64 * secs) as u32;
        (0..steps).filter(|i| gate.allow(start + Duration::from_secs_f64(*i as f64 / clock_hz as f64))).count()
    }

    #[test]
    fn a_fast_display_clock_reaches_the_lenses_at_most_sixty_times_a_second() {
        // The regression: the lens source was set on every refresh, 360 times a second on a 360 Hz monitor.
        for clock in [60, 120, 144, 240, 360] {
            let n = passes(&mut RateGate::new(LENS_HZ), clock, 1.0);
            assert!(n <= LENS_HZ as usize + 1, "{clock} Hz clock let {n} refreshes through in a second");
            assert!(n >= 40, "{clock} Hz clock still keeps the lens fresh ({n}/s)");
        }
        let n = passes(&mut RateGate::new(LENS_HZ), 360, 1.0);
        assert!((50..=60).contains(&n), "360 Hz: about every sixth refresh ({n}/s)");
    }

    #[test]
    fn the_first_tick_always_passes_and_marking_counts_as_a_refresh() {
        let t = Instant::now();
        let mut g = RateGate::new(60);
        assert!(g.allow(t), "nothing has refreshed yet");
        assert!(!g.allow(t + Duration::from_millis(5)));
        assert!(g.allow(t + Duration::from_millis(17)));
        g.mark(t + Duration::from_millis(20));
        assert!(!g.allow(t + Duration::from_millis(30)), "a forced refresh at 20 ms holds off the next one");
        assert!(g.allow(t + Duration::from_millis(37)));
        assert!(!g.allow(t), "time going backwards never panics or fires");
    }

    #[test]
    fn the_lens_rate_follows_a_lower_frame_cap_but_never_exceeds_sixty() {
        assert_eq!(lens_hz(None), 60);
        assert_eq!(lens_hz(Some(240)), 60);
        assert_eq!(lens_hz(Some(60)), 60);
        assert_eq!(lens_hz(Some(30)), 30);
        assert_eq!(lens_hz(Some(0)), 60, "a zero cap means no cap");
        assert_eq!(lens_hz(Some(5)), 15, "and it never gets so slow the lens looks frozen");
    }
}
