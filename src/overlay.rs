//! The crosshair overlay: layered, topmost, click-through, per-pixel alpha.
//!
//! It owns one 33x33 window and redraws it only when something changed
//! (profile flip, config reload, show/hide, display change, menu edit, or a
//! frame of an enabled animation). Nothing here touches another process: no
//! handles to the game, no memory reads, no hooks, no injection. Outside input
//! is raw-input events addressed to our own window: mouse buttons and
//! movement, and only the movement keys you bound. The menu window lives in
//! `hud.rs`; this file routes its input and applies what it changes.

use std::cell::RefCell;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{CreateMutexW, ExitProcess, OpenProcess, WaitForSingleObject};
use windows_sys::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::Input::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::autocolor::{self, Picker};
use crate::color;
use crate::config::{Config, Profile, Trigger};
use crate::crosshair::{Blend, Cell, Crosshair, C, N};
use crate::display::{self, Info, MonitorSel};
use crate::dynamics::{self, Dyn, Frame, Inputs, NEUTRAL};
use crate::experimental::ZoomWhen;
use crate::frame::{FramePacer, WM_FRAME};
use crate::hud::{self, Hud};
use crate::area;
use crate::lens::{self, AreaSpec, Lenses};
use crate::menu::{Effects, Key};
use crate::monitors::{self, Target};
use crate::morph::{self, Morph};
use crate::states::{self, Tracker};
use crate::trace::{Demo, Tracer};
use crate::update::{self, Outcome, Status, Version};

const HK_SHOW: i32 = 1;
const HK_RELOAD: i32 = 2;
const HK_QUIT: i32 = 3;
const HK_KEY: i32 = 4;
const HK_MENU: i32 = 5;

const TIMER_TOPMOST: usize = 1;
const TIMER_SMOKE: usize = 2;
const TIMER_AUTO: usize = 3;
const TIMER_IDLE: usize = 4;
const TIMER_FOLLOW: usize = 5;
const TIMER_UPDATE: usize = 6;
const TIMER_RESTART: usize = 7;
/// Restart waits this long for an update check to answer before restarting anyway.
const RESTART_WAIT_MS: u32 = 12_000;
const FOLLOW_MS: u32 = 250;
/// the crosshair window is at most 4x the 33x33 grid
const CROSS_MAX: i32 = N as i32 * display::MAX_SCALE;
const TOPMOST_MS: u32 = 1000;
const IDLE_CHECK_MS: u32 = 250;
const AUTO_MS: [u32; 4] = [250, 250, 100, 50];

const WM_MOUSELEAVE: u32 = 0x02A3;
/// The update thread finished a check; the result is in `UPDATE_RESULT`.
const WM_UPDATE: u32 = WM_APP + 2;
const WDA_EXCLUDEFROMCAPTURE: u32 = 0x11;
/// the trace window is at most 2*256+1 pixels square
const TRACE_MAX: i32 = 513;

// RAWMOUSE.usButtonFlags
const RI_LEFT_DOWN: u16 = 0x0001;
const RI_LEFT_UP: u16 = 0x0002;
const RI_RIGHT_DOWN: u16 = 0x0004;
const RI_RIGHT_UP: u16 = 0x0008;

/// A raw input event, reduced to what reDraw uses.
enum Raw {
    Mouse { flags: u16, dx: i32, dy: i32, relative: bool },
    Key { vk: u16, up: bool },
}

/// The spray-trace window: a click-through layer sized to the tracer's range.
struct TraceWin {
    hwnd: HWND,
    dc: HDC,
    dib: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
    buf: Vec<u32>,
    shown: bool,
}

impl Drop for TraceWin {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            DeleteObject(self.dib);
            DeleteDC(self.dc);
            DestroyWindow(self.hwnd);
        }
    }
}

/// Runtime state of the experimental features.
struct Live {
    dynst: Dyn,
    frame: Frame,
    tracer: Tracer,
    picker: Picker,
    auto_color: Option<u32>,
    firing: bool,
    keys: [bool; 4],
    last_input: Instant,
    last_frame: Instant,
    /// which input states (crouch, sprint, jump) are on
    states: Tracker,
    /// the hip fire / ADS crossfade
    morph: Morph,
    /// the scripted spray of the menu's Test trace link, while it plays
    demo: Option<Demo>,
    /// the Restart button is waiting for an update check to finish
    restart_pending: bool,
    /// hue offsets the crosshair was last drawn with (hue-cycling layers)
    hues: [i32; 4],
    /// the hue clock starts here
    started: Instant,
}

struct App {
    cfg: Config,
    cfg_path: PathBuf,
    hwnd: HWND,
    mem_dc: HDC,
    bits: *mut u32,
    hud: Hud,
    lenses: Lenses,
    pacer: FramePacer,
    trace: TraceWin,
    live: Live,
    /// where the crosshair center is, and the monitor it is on (resolved from the Display settings)
    target: Target,
    /// whole-number scale of the crosshair, resolved from the Display settings
    scale: i32,
    pid: u32,
    ads: bool,
    visible: bool,
    present_ok: bool,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// A check is running on its background thread.
static UPDATE_BUSY: AtomicBool = AtomicBool::new(false);
/// A newer version is already on disk: the running copy is out of date, and checking again would
/// only download it again.
static UPDATE_DONE: AtomicBool = AtomicBool::new(false);
static UPDATE_RESULT: Mutex<Option<Outcome>> = Mutex::new(None);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Close the program now, for certain: the menu's ForceQuit. It does not go through the message
/// loop (which could be busy or stuck); the only thing it waits for, for at most half a second, is an
/// update that is in the middle of swapping the exe, so the file is never left half replaced.
fn force_quit() -> ! {
    update::wait_for_swap(Duration::from_millis(500));
    unsafe { ExitProcess(0) }
}

/// After "Restart": the new copy waits for the one that started it to be gone before it takes
/// the single-instance lock.
pub fn wait_for_exit(pid: u32) {
    const SYNCHRONIZE: u32 = 0x0010_0000;
    unsafe {
        let h = OpenProcess(SYNCHRONIZE, 0, pid);
        if !h.is_null() {
            WaitForSingleObject(h, 8000);
            CloseHandle(h);
        }
    }
}

fn read_config(path: &Path) -> std::io::Result<Config> {
    // Lossy decode: a stray non-UTF-8 byte must not throw the whole file away.
    std::fs::read(path).map(|bytes| Config::parse(&String::from_utf8_lossy(&bytes)))
}

/// Write via a temp file so a crash mid-save cannot leave a half-written config.
fn save_config(cfg: &Config, path: &Path) {
    let tmp = path.with_extension("cfg.tmp");
    if std::fs::write(&tmp, cfg.serialize()).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

fn blend_fn() -> BLENDFUNCTION {
    BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 }
}

impl App {
    fn active(&self) -> &Crosshair {
        if self.ads {
            &self.cfg.ads
        } else {
            &self.cfg.hip
        }
    }

    /// Screen pixel the crosshair's center sits on: the target's center plus the configured offset.
    fn center(&self) -> POINT {
        POINT { x: self.target.center.0 + self.cfg.offset.0, y: self.target.center.1 + self.cfg.offset.1 }
    }

    /// Top-left of the crosshair window. The center cell (a k x k block) is centered on `center()`.
    fn window_pos(&self) -> POINT {
        let c = self.center();
        let k = self.scale;
        POINT { x: c.x - C * k - k / 2, y: c.y - C * k - k / 2 }
    }

    /// Resolve the Display settings against the real monitors. Returns true if the
    /// crosshair has to be redrawn (it moved or its scale changed).
    fn refresh_display(&mut self) -> bool {
        let d = self.cfg.display.clone();
        let target = monitors::resolve(d.monitor, self.pid, Some(self.target));
        let m = target.monitor.rect;
        let scale = d.scale.unwrap_or_else(|| display::auto_scale(m.w, m.h));
        let primary = monitors::primary();
        let ui = d.ui_scale.unwrap_or_else(|| display::auto_ui_scale(primary.dpi, primary.rect.w, primary.rect.h));
        let moved = target.center != self.target.center || scale != self.scale;
        self.target = target;
        self.scale = scale;
        self.hud.set_scale(ui, &self.cfg);
        let info = Info { scale, ui_scale: self.hud.scale(), refresh: target.monitor.refresh };
        if self.hud.menu.set_info(info) && self.hud.visible {
            self.hud.paint(&self.cfg);
        }
        moved
    }

    /// The Display settings changed (or the monitors did): re-resolve and redraw.
    fn apply_display(&mut self) {
        self.lenses.set_rate(lens::lens_hz(self.cfg.display.fps));
        self.refresh_display();
        unsafe {
            if self.cfg.display.monitor == MonitorSel::Follow {
                SetTimer(self.hwnd, TIMER_FOLLOW, FOLLOW_MS, None);
            } else {
                KillTimer(self.hwnd, TIMER_FOLLOW);
            }
        }
        self.present();
        if self.trace.shown {
            self.present_trace();
        }
    }

    /// Follow window: when the focused window's viewport (or monitor) changes, so does the crosshair.
    fn on_follow_tick(&mut self) {
        if self.refresh_display() {
            self.present();
            if self.trace.shown {
                self.present_trace();
            }
        }
    }

    /// Crossfade time in ms: none when motion is reduced.
    fn morph_ms(&self) -> u32 {
        if self.cfg.reduce_motion {
            0
        } else {
            self.cfg.morph
        }
    }

    /// Switch profile at once (the menu is editing it, or a reload): no crossfade.
    fn snap_ads(&mut self, ads: bool) {
        self.ads = ads;
        self.live.morph.go(ads, 0, true);
    }

    /// The crosshair as drawn right now, and the opacity multiplier for this frame.
    fn effective_crosshair(&self) -> (Crosshair, f32) {
        self.crosshair_for(self.ads)
    }

    /// The hip fire or ADS crosshair with the active input state and dynamics applied.
    fn crosshair_for(&self, ads: bool) -> (Crosshair, f32) {
        let e = &self.cfg.exp;
        let mut base = if ads { self.cfg.ads.clone() } else { self.cfg.hip.clone() };
        // States are for playing; while the menu is open the crosshair shows what is being edited.
        if !self.hud.visible {
            if let Some(st) = self.live.states.active(&self.cfg.states) {
                base = states::apply(&base, self.cfg.states.rule(st));
            }
        }
        let mut c = if e.dynamics.enabled { dynamics::apply(&base, &self.live.frame) } else { base };
        if e.auto.enabled {
            if let Some(color) = self.live.auto_color {
                c = autocolor::recolor(&c, color);
            }
        }
        (c, if e.dynamics.enabled { self.live.frame.opacity } else { 1.0 })
    }

    /// Does anything on screen turn its hue right now? Then the clock has to run.
    fn hue_cycling(&self) -> bool {
        !self.cfg.reduce_motion && (self.active().cycling() || (self.live.morph.animating() && (self.cfg.hip.cycling() || self.cfg.ads.cycling())))
    }

    fn present(&mut self) {
        let level = self.live.morph.level();
        let any_cycle = !self.cfg.reduce_motion && (self.cfg.hip.cycling() || self.cfg.ads.cycling());
        let hues = if any_cycle { color::cycle_hues(self.live.started.elapsed().as_millis() as u64) } else { [0; 4] };
        self.live.hues = hues;
        // Both ends are drawn only while the crossfade is between them; at rest it is one crosshair.
        let hip = (level < morph::LEVELS).then(|| self.crosshair_for(false));
        let ads = (level > 0).then(|| self.crosshair_for(true));
        let pixel_of = |c: &(Crosshair, f32)| c.0.premultiplied_hue(c.1, hues);
        let pixels = match (&hip, &ads) {
            (Some(h), Some(a)) => morph::mix(&pixel_of(h), &pixel_of(a), level),
            (Some(h), None) => pixel_of(h),
            (None, Some(a)) => pixel_of(a),
            (None, None) => [0; N * N],
        };
        // Lenses and the zoom cannot be crossfaded: they follow whichever end is nearer.
        let c = if self.live.morph.nearer_ads() { ads.map(|a| a.0) } else { hip.map(|h| h.0) }.unwrap_or_else(|| self.effective_crosshair().0);
        // Each crosshair pixel becomes a k x k block: whole-number scaling, never a blend.
        let k = self.scale;
        let side = N as i32 * k;
        // SAFETY: `bits` points at the CROSS_MAX x CROSS_MAX u32 DIB section created in `run`; side <= CROSS_MAX.
        unsafe {
            for y in 0..side {
                let row = &pixels[(y / k) as usize * N..][..N];
                let dst = self.bits.add((y * CROSS_MAX) as usize);
                for x in 0..side {
                    *dst.add(x as usize) = row[(x / k) as usize];
                }
            }
        }
        let pos = self.window_pos();
        let size = SIZE { cx: side, cy: side };
        let src = POINT { x: 0, y: 0 };
        let blend = blend_fn();
        self.present_ok = unsafe { UpdateLayeredWindow(self.hwnd, null_mut(), &pos, &size, self.mem_dc, &src, 0, &blend, ULW_ALPHA) } != 0;
        self.sync_lenses(&c);
        self.wake();
    }

    /// Lens blends follow the crosshair's shape; the scope zoom follows ADS.
    fn sync_lenses(&mut self, c: &Crosshair) {
        let effects: Vec<(Blend, [bool; N * N])> = if self.visible { c.lens_effects().into_iter().map(|e| (e, c.lens_mask(e))).collect() } else { Vec::new() };
        // Two magnified areas, both centered on the crosshair (and so following Follow window, the monitor
        // choice and the offset): Scope zoom, and Aspect ratio, which has a shape of its own.
        let (z, p) = (&self.cfg.exp.zoom, &self.cfg.exp.aspect);
        let on = |enabled: bool, when: ZoomWhen| enabled && self.visible && (when == ZoomWhen::Always || self.ads);
        let center = self.center();
        let zoom = on(z.enabled, z.when).then(|| AreaSpec { center, geo: area::zoom(z.size, z.factor as f32 / 10.0), ellipse: z.circle });
        let aspect = on(p.enabled, p.when).then(|| AreaSpec { center, geo: area::aspect(p.size, p.ratio, p.fit, p.factor as f32 / 10.0), ellipse: p.ellipse });
        let exclude = [self.hwnd, self.trace.hwnd, self.hud.hwnd];
        self.lenses.update(&lens::Spec { crosshair_pos: self.window_pos(), scale: self.scale, effects: &effects, areas: [zoom, aspect], exclude: &exclude, hide_from_capture: !self.cfg.exp.show_on_capture });
        if self.lenses.any() {
            self.reassert_topmost();
            self.wake();
        }
    }

    fn present_trace(&mut self) {
        let t = &self.cfg.exp.trace;
        let side = 2 * t.range + 1;
        self.trace.buf.resize((side * side) as usize, 0);
        self.live.tracer.render(t, &mut self.trace.buf, side);
        // SAFETY: the DIB is TRACE_MAX x TRACE_MAX and side <= TRACE_MAX.
        unsafe {
            for y in 0..side {
                std::ptr::copy_nonoverlapping(self.trace.buf.as_ptr().add((y * side) as usize), self.trace.bits.add((y * TRACE_MAX) as usize), side as usize);
            }
        }
        let c = self.center();
        let pos = POINT { x: c.x - t.range, y: c.y - t.range };
        let size = SIZE { cx: side, cy: side };
        let blend = blend_fn();
        unsafe { UpdateLayeredWindow(self.trace.hwnd, null_mut(), &pos, &size, self.trace.dc, &POINT { x: 0, y: 0 }, 0, &blend, ULW_ALPHA) };
    }

    fn reassert_topmost(&self) {
        unsafe {
            SetWindowPos(self.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    fn uses_mouse(&self) -> bool {
        self.cfg.trigger != Trigger::Key
    }

    /// Subscribe to (or drop) raw mouse and keyboard events for our own window, only as needed.
    fn update_raw_input(&self) {
        let e = &self.cfg.exp;
        let want_mouse = self.uses_mouse() || e.trace.enabled || (e.dynamics.enabled && (e.dynamics.bloom != 0 || e.dynamics.idle != 0));
        let want_keys = (e.dynamics.enabled && e.dynamics.spread != 0) || self.cfg.states.any_bound();
        for (usage, want) in [(2u16, want_mouse), (6u16, want_keys)] {
            let rid = if want {
                RAWINPUTDEVICE { usUsagePage: 1, usUsage: usage, dwFlags: RIDEV_INPUTSINK, hwndTarget: self.hwnd }
            } else {
                RAWINPUTDEVICE { usUsagePage: 1, usUsage: usage, dwFlags: RIDEV_REMOVE, hwndTarget: null_mut() }
            };
            unsafe { RegisterRawInputDevices(&rid, 1, std::mem::size_of::<RAWINPUTDEVICE>() as u32) };
        }
    }

    /// Returns false if the OS refused the ADS key (another program owns it).
    fn register_key_hotkey(&self) -> bool {
        unsafe {
            UnregisterHotKey(self.hwnd, HK_KEY);
            if self.cfg.trigger == Trigger::Key {
                return RegisterHotKey(self.hwnd, HK_KEY, MOD_NOREPEAT, self.cfg.key) != 0;
            }
        }
        true
    }

    fn set_ads(&mut self, ads: bool) {
        if self.ads != ads {
            self.ads = ads;
            let ms = self.morph_ms();
            self.live.morph.go(ads, ms, false);
            if self.visible {
                self.present();
            }
        }
    }

    // --- experimental features -------------------------------------------------

    fn dyn_inputs(&self) -> Inputs {
        Inputs { firing: self.live.firing, moving: self.live.keys.iter().any(|k| *k), idle_secs: self.live.last_input.elapsed().as_secs_f32() }
    }

    /// Is something other than a lens moving? That needs a frame every display refresh.
    fn animating_beyond_lenses(&self) -> bool {
        let e = &self.cfg.exp;
        self.live.tracer.active() || self.live.demo.is_some() || self.live.morph.animating() || self.hue_cycling() || (e.dynamics.enabled && self.live.dynst.animating(&e.dynamics, &self.dyn_inputs()))
    }

    /// Does anything still need frames?
    fn needs_frames(&self) -> bool {
        self.visible && (self.lenses.any() || self.animating_beyond_lenses())
    }

    /// Frames come once per display refresh while anything animates; when a lens is the only thing
    /// that needs them, at the lens rate, so the clock does not wake 360 times a second for nothing.
    fn sync_frame_period(&self) {
        let period = if self.animating_beyond_lenses() { None } else { Some(Duration::from_secs_f64(1.0 / lens::lens_hz(self.cfg.display.fps) as f64)) };
        self.pacer.set_period(period);
    }

    fn wake(&mut self) {
        self.sync_frame_period();
        if self.needs_frames() && !self.pacer.running() {
            self.live.last_frame = Instant::now();
            self.pacer.start();
        }
    }

    /// Advance the dynamics; true if what they draw changed (the caller presents).
    fn step_dynamics(&mut self, dt: f32) -> bool {
        if !self.cfg.exp.dynamics.enabled {
            return false;
        }
        let inp = self.dyn_inputs();
        let frame = self.live.dynst.step(&self.cfg.exp.dynamics, &inp, dt);
        if frame != self.live.frame {
            self.live.frame = frame;
            return true;
        }
        false
    }

    fn tick_dynamics(&mut self, dt: f32) {
        if self.step_dynamics(dt) {
            self.present();
        }
    }

    fn on_frame(&mut self) {
        self.pacer.ack();
        // Frame rate: Auto does a frame per display refresh (whatever the monitor runs at);
        // a fixed cap skips refreshes so the work happens at most that often.
        if let Some(gap) = display::frame_interval(self.cfg.display.fps) {
            if self.live.last_frame.elapsed().as_secs_f32() < gap * 0.85 {
                return;
            }
        }
        let now = Instant::now();
        let dt = now.duration_since(self.live.last_frame).as_secs_f32();
        self.live.last_frame = now;
        let ms = self.morph_ms();
        let mut redraw = self.live.morph.step(dt, ms);
        if self.hue_cycling() && color::cycle_hues(self.live.started.elapsed().as_millis() as u64) != self.live.hues {
            redraw = true;
        }
        if self.step_dynamics(dt) || redraw {
            self.present();
        }
        if let Some(d) = self.live.demo.as_mut() {
            match d.step(dt) {
                Some((dx, dy)) => self.live.tracer.feed(&self.cfg.exp.trace, dx, dy),
                None => {
                    self.live.demo = None;
                    self.live.tracer.set_fire(self.live.firing);
                }
            }
        }
        if self.live.tracer.step(&self.cfg.exp.trace, dt) {
            self.present_trace();
        }
        self.lenses.tick();
        self.sync_frame_period();
        if !self.needs_frames() {
            self.pacer.stop();
        }
    }

    /// Slow tick while idle fade is on, so the fade starts even with no input at all.
    fn on_idle_tick(&mut self) {
        if self.visible && self.cfg.exp.dynamics.enabled {
            let now = Instant::now();
            let dt = now.duration_since(self.live.last_frame).as_secs_f32();
            self.live.last_frame = now;
            self.tick_dynamics(dt);
            self.wake();
        }
    }

    /// Pick a crosshair color from the pixels behind it (ignoring the ones we painted ourselves).
    fn on_auto_tick(&mut self) {
        if !self.visible || !self.cfg.exp.auto.enabled {
            return;
        }
        let c = self.center();
        let (cross, _) = self.effective_crosshair();
        let (k, side) = (self.scale, N as i32 * self.scale);
        let cells = cross.render().cell;
        let ignore: Vec<bool> = (0..side * side).map(|i| cells[((i / side) / k) as usize * N + ((i % side) / k) as usize] != Cell::Empty).collect();
        let Some(pixels) = crate::sample::region(c.x, c.y, side) else { return };
        let Some(avg) = autocolor::average(&pixels, &ignore) else { return };
        let color = self.live.picker.pick(self.cfg.exp.auto.mode, avg);
        if self.live.auto_color != Some(color) {
            self.live.auto_color = Some(color);
            self.present();
        }
    }

    /// The menu's Test trace link: play a scripted spray so the trace can be seen with the menu open.
    fn start_trace_demo(&mut self) {
        if !self.cfg.exp.trace.enabled {
            return;
        }
        self.live.tracer.set_fire(true);
        self.live.demo = Some(Demo::new());
        self.set_trace_visible(self.visible);
        self.live.last_frame = Instant::now();
        self.wake();
    }

    fn set_trace_visible(&mut self, show: bool) {
        if self.trace.shown != show {
            self.trace.shown = show;
            unsafe { ShowWindow(self.trace.hwnd, if show { SW_SHOWNOACTIVATE } else { SW_HIDE }) };
        }
    }

    /// Bring the running state in line with the Experimental settings.
    fn apply_experimental(&mut self) {
        let e = self.cfg.exp.clone();
        let affinity = if e.show_on_capture { 0 } else { WDA_EXCLUDEFROMCAPTURE };
        unsafe {
            for w in [self.hwnd, self.trace.hwnd, self.hud.hwnd] {
                SetWindowDisplayAffinity(w, affinity);
            }
        }
        self.update_raw_input();
        if !e.dynamics.enabled {
            self.live.dynst = Dyn::new();
            self.live.frame = NEUTRAL;
        }
        unsafe {
            if e.dynamics.enabled && e.dynamics.idle != 0 {
                SetTimer(self.hwnd, TIMER_IDLE, IDLE_CHECK_MS, None);
            } else {
                KillTimer(self.hwnd, TIMER_IDLE);
            }
            if e.auto.enabled {
                SetTimer(self.hwnd, TIMER_AUTO, AUTO_MS[e.auto.rate.clamp(1, 3) as usize], None);
            } else {
                KillTimer(self.hwnd, TIMER_AUTO);
            }
        }
        if !e.auto.enabled {
            self.live.auto_color = None;
            self.live.picker.forget();
        }
        if !e.trace.enabled {
            self.live.tracer.reset();
            self.present_trace();
        }
        self.set_trace_visible(e.trace.enabled && self.visible);
        self.present();
        self.wake();
    }

    // --- menu, hotkeys, input --------------------------------------------------

    fn reload(&mut self) {
        if let Ok(cfg) = read_config(&self.cfg_path) {
            self.cfg = cfg;
        }
        let ads = self.hud.visible && self.hud.menu.editing == Profile::Ads;
        self.snap_ads(ads);
        self.live.states.reset();
        self.register_key_hotkey();
        self.apply_experimental();
        if self.hud.visible {
            self.hud.paint(&self.cfg);
        }
    }

    fn show_menu(&mut self) {
        let ads = self.hud.menu.editing == Profile::Ads;
        self.snap_ads(ads);
        self.live.states.reset();
        self.hud.show(&self.cfg);
        // Drawn after the menu is up: input states are off while it is open.
        self.present();
    }

    fn hide_menu(&mut self) {
        self.hud.hide();
        self.snap_ads(false);
        self.present();
    }

    fn on_hotkey(&mut self, id: i32) {
        match id {
            HK_SHOW => {
                self.visible = !self.visible;
                unsafe { ShowWindow(self.hwnd, if self.visible { SW_SHOWNOACTIVATE } else { SW_HIDE }) };
                self.set_trace_visible(self.visible && self.cfg.exp.trace.enabled);
                self.present();
            }
            HK_RELOAD => self.reload(),
            HK_MENU => {
                if self.hud.visible {
                    self.hide_menu();
                } else {
                    self.show_menu();
                }
            }
            HK_QUIT => unsafe {
                DestroyWindow(self.hwnd);
            },
            HK_KEY => {
                self.live.last_input = Instant::now();
                if !self.hud.visible {
                    let flipped = !self.ads;
                    self.set_ads(flipped);
                }
                self.wake();
            }
            _ => {}
        }
    }

    /// ADS trigger from a raw mouse event. Hold: ADS follows the button. Toggle: each press flips.
    /// Ignored while the menu is open (its clicks must not flip the crosshair).
    fn on_mouse_flags(&mut self, flags: u16) {
        if !self.visible || !self.uses_mouse() || self.hud.visible {
            return;
        }
        // With "swap primary and secondary buttons" the game's secondary click is the physical left button.
        let (down, up) = if unsafe { GetSystemMetrics(SM_SWAPBUTTON) } != 0 { (RI_LEFT_DOWN, RI_LEFT_UP) } else { (RI_RIGHT_DOWN, RI_RIGHT_UP) };
        match self.cfg.trigger {
            Trigger::HoldRmb => {
                if flags & down != 0 {
                    self.set_ads(true);
                } else if flags & up != 0 {
                    self.set_ads(false);
                }
            }
            Trigger::ToggleRmb => {
                if flags & down != 0 {
                    let flipped = !self.ads;
                    self.set_ads(flipped);
                }
            }
            Trigger::Key => {}
        }
    }

    fn on_raw(&mut self, ev: Raw) {
        match ev {
            Raw::Key { vk, up } => {
                // Only the four bound movement keys and the keys bound to a state are looked at;
                // every other key is dropped here.
                if !self.hud.visible && self.live.states.key(&self.cfg.states, vk, up) && self.visible {
                    self.present();
                }
                if let Some(i) = self.cfg.exp.dynamics.keys.iter().position(|k| *k as u16 == vk) {
                    self.live.keys[i] = !up;
                    if !up {
                        self.live.last_input = Instant::now();
                    }
                    self.wake_dynamics();
                }
            }
            Raw::Mouse { flags, dx, dy, relative } => {
                // The fire button is the primary one: left, or right when buttons are swapped.
                let (fire_down, fire_up) = if unsafe { GetSystemMetrics(SM_SWAPBUTTON) } != 0 { (RI_RIGHT_DOWN, RI_RIGHT_UP) } else { (RI_LEFT_DOWN, RI_LEFT_UP) };
                if flags & fire_down != 0 {
                    self.live.firing = true;
                    self.live.tracer.set_fire(true);
                }
                if flags & fire_up != 0 {
                    self.live.firing = false;
                    self.live.tracer.set_fire(false);
                }
                if self.visible && !self.hud.visible {
                    if flags != 0 || dx != 0 || dy != 0 {
                        self.live.last_input = Instant::now();
                    }
                    self.on_mouse_flags(flags);
                    // Relative counts only: absolute pointers (tablets, remote desktop) carry no motion we can use.
                    if relative && (dx != 0 || dy != 0) {
                        self.live.tracer.feed(&self.cfg.exp.trace, dx, dy);
                    }
                }
                self.wake_dynamics();
                self.wake();
            }
        }
    }

    /// Input changed what dynamics should do: step once now so the first frame is not late.
    fn wake_dynamics(&mut self) {
        if self.cfg.exp.dynamics.enabled && self.visible {
            let now = Instant::now();
            let dt = now.duration_since(self.live.last_frame).as_secs_f32();
            if !self.pacer.running() {
                self.live.last_frame = now;
            }
            self.tick_dynamics(dt.min(0.02));
        }
        self.wake();
    }

    fn on_display_change(&mut self) {
        self.hud.refresh_work_area(&self.cfg);
        self.refresh_display();
        self.present();
        if self.trace.shown {
            self.present_trace();
        }
    }

    fn on_topmost_timer(&self) {
        if self.visible {
            self.reassert_topmost();
        }
        unsafe {
            if self.trace.shown {
                SetWindowPos(self.trace.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
            if self.hud.visible {
                SetWindowPos(self.hud.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }
    }

    /// Carry out what a menu interaction asked for.
    fn apply(&mut self, fx: Effects) {
        if fx.trigger {
            self.register_key_hotkey();
            self.update_raw_input();
        }
        if fx.display {
            self.apply_display();
        }
        if fx.experimental {
            self.apply_experimental();
        }
        if fx.quit {
            force_quit();
        }
        if fx.update_cfg {
            self.apply_autoupdate();
        }
        if fx.update_check {
            self.check_now();
        }
        if fx.restart {
            self.restart_and_update();
        }
        if fx.trace_demo {
            self.start_trace_demo();
        }
        if fx.states {
            self.live.states.reset();
            self.update_raw_input();
            self.present();
        }
        if fx.crosshair {
            let ads = self.hud.visible && self.hud.menu.editing == Profile::Ads;
            self.snap_ads(ads);
            self.present();
        }
        if fx.persist {
            save_config(&self.cfg, &self.cfg_path);
        }
        if let Some(kind) = fx.paste {
            if let Some(text) = hud::get_clipboard(self.hud.hwnd) {
                let pasted = self.hud.menu.pasted(&mut self.cfg, kind, &text);
                self.apply(pasted);
            }
        }
        if let Some(text) = &fx.clipboard {
            hud::set_clipboard(self.hud.hwnd, text);
        }
        if fx.hide {
            self.hide_menu();
        } else if fx.repaint && self.hud.visible {
            self.hud.paint(&self.cfg);
        }
    }

    // The menu window reports physical pixels; the menu thinks in design-size pixels.
    fn hud_mouse_down(&mut self, x: i32, y: i32, right: bool) {
        let s = self.hud.scale();
        let fx = self.hud.menu.mouse_down(&mut self.cfg, x / s, y / s, right);
        self.apply(fx);
    }

    fn hud_mouse_up(&mut self, x: i32, y: i32, right: bool) {
        let s = self.hud.scale();
        let fx = self.hud.menu.mouse_up(&mut self.cfg, x / s, y / s, right);
        self.apply(fx);
    }

    fn hud_mouse_move(&mut self, x: i32, y: i32, left_down: bool) {
        let s = self.hud.scale();
        let fx = self.hud.menu.mouse_move(&mut self.cfg, x / s, y / s, left_down);
        self.apply(fx);
    }

    fn hud_mouse_leave(&mut self) {
        let fx = self.hud.menu.mouse_leave();
        self.apply(fx);
    }

    fn hud_wheel(&mut self, notches: i32) {
        let fx = self.hud.menu.wheel(&self.cfg, notches);
        self.apply(fx);
    }

    fn hud_key(&mut self, key: Key) {
        let fx = self.hud.menu.key(&mut self.cfg, key);
        self.apply(fx);
    }

    // --- updates ---------------------------------------------------------------

    fn set_update_status(&mut self, status: Status) {
        if self.hud.menu.set_update(status) && self.hud.visible {
            self.hud.paint(&self.cfg);
        }
    }

    /// Start or stop the 30 minute checks to match the Auto-update setting (and check now if it is on).
    fn apply_autoupdate(&mut self) {
        unsafe {
            if self.cfg.autoupdate {
                SetTimer(self.hwnd, TIMER_UPDATE, update::CHECK_EVERY.as_millis() as u32, None);
            } else {
                KillTimer(self.hwnd, TIMER_UPDATE);
            }
        }
        if UPDATE_DONE.load(Ordering::SeqCst) {
            return; // already installed: the status stays "installed, restart to use"
        }
        if self.cfg.autoupdate {
            self.set_update_status(Status::Checking);
            self.spawn_check();
        } else {
            self.set_update_status(Status::Off);
        }
    }

    fn on_update_timer(&mut self) {
        if self.cfg.autoupdate {
            self.spawn_check();
        }
    }

    /// "Check now" in the menu: works even with Auto-update off.
    fn check_now(&mut self) {
        if !UPDATE_DONE.load(Ordering::SeqCst) {
            self.set_update_status(Status::Checking);
            self.spawn_check();
        }
    }

    /// One check on a background thread: the menu and the crosshair never wait for the network.
    fn spawn_check(&self) {
        if UPDATE_DONE.load(Ordering::SeqCst) || UPDATE_BUSY.swap(true, Ordering::SeqCst) {
            return;
        }
        let hwnd = self.hwnd as usize;
        std::thread::spawn(move || {
            let outcome = match (std::env::current_exe(), Version::parse(update::VERSION)) {
                (Ok(exe), Some(current)) => update::check_and_install(&exe, &current, &update::manifest_url(), &|url, max| crate::http::get(url, max)),
                (Err(e), _) => Outcome::Failed(e.to_string()),
                (_, None) => Outcome::Failed("this build has no version".to_string()),
            };
            if matches!(outcome, Outcome::Installed(_)) {
                UPDATE_DONE.store(true, Ordering::SeqCst);
            }
            if let Ok(mut slot) = UPDATE_RESULT.lock() {
                *slot = Some(outcome);
            }
            UPDATE_BUSY.store(false, Ordering::SeqCst);
            unsafe { PostMessageW(hwnd as HWND, WM_UPDATE, 0, 0) };
        });
    }

    fn on_update_result(&mut self) {
        let Some(outcome) = UPDATE_RESULT.lock().ok().and_then(|mut slot| slot.take()) else { return };
        let status = match outcome {
            Outcome::UpToDate => Status::UpToDate,
            Outcome::Installed(v) => Status::Installed(v),
            Outcome::Failed(_) => Status::Failed,
        };
        self.set_update_status(status);
        // The Restart button was waiting on this check: restart now, whatever it found (or failed to).
        if std::mem::take(&mut self.live.restart_pending) {
            unsafe { KillTimer(self.hwnd, TIMER_RESTART) };
            self.restart_now();
        }
    }

    /// The menu's Restart button: restart, installing a newer version first if there is one. With
    /// Auto-update on and nothing installed yet it checks first, and restarts when the check answers (or
    /// after a few seconds if it does not); with Auto-update off, or an update already waiting, it just restarts.
    fn restart_and_update(&mut self) {
        if UPDATE_DONE.load(Ordering::SeqCst) || !self.cfg.autoupdate {
            self.restart_now();
            return;
        }
        self.live.restart_pending = true;
        self.set_update_status(Status::Checking);
        self.spawn_check();
        unsafe { SetTimer(self.hwnd, TIMER_RESTART, RESTART_WAIT_MS, None) };
    }

    fn on_restart_timer(&mut self) {
        unsafe { KillTimer(self.hwnd, TIMER_RESTART) };
        if std::mem::take(&mut self.live.restart_pending) {
            self.restart_now();
        }
    }

    /// Start the installed version, which waits for this one to be gone, and quit.
    fn restart_now(&self) {
        let Ok(exe) = std::env::current_exe() else { return };
        if std::process::Command::new(exe).arg("--restart-from").arg(std::process::id().to_string()).spawn().is_ok() {
            force_quit();
        }
    }

    fn hud_title_tick(&mut self) {
        if self.hud.visible {
            self.hud.paint_title(&self.cfg);
        }
    }
}

fn with_app(f: impl FnOnce(&mut App)) {
    APP.with(|a| {
        if let Ok(mut guard) = a.try_borrow_mut() {
            if let Some(app) = guard.as_mut() {
                f(app);
            }
        }
    });
}

unsafe fn read_raw(lp: LPARAM) -> Option<Raw> {
    let mut ri: RAWINPUT = std::mem::zeroed();
    let mut size = std::mem::size_of::<RAWINPUT>() as u32;
    let got = GetRawInputData(lp as HRAWINPUT, RID_INPUT, &mut ri as *mut RAWINPUT as *mut c_void, &mut size, std::mem::size_of::<RAWINPUTHEADER>() as u32);
    if got == u32::MAX {
        return None;
    }
    match ri.header.dwType {
        RIM_TYPEMOUSE => {
            let m = ri.data.mouse;
            Some(Raw::Mouse { flags: m.Anonymous.Anonymous.usButtonFlags, dx: m.lLastX, dy: m.lLastY, relative: m.usFlags & 1 == 0 })
        }
        RIM_TYPEKEYBOARD => {
            let k = ri.data.keyboard;
            Some(Raw::Key { vk: k.VKey, up: k.Flags & 1 != 0 })
        }
        _ => None,
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_HOTKEY => {
            with_app(|app| app.on_hotkey(wp as i32));
            0
        }
        WM_INPUT => {
            if let Some(ev) = read_raw(lp) {
                with_app(|app| app.on_raw(ev));
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_FRAME => {
            with_app(|app| app.on_frame());
            0
        }
        WM_UPDATE => {
            with_app(|app| app.on_update_result());
            0
        }
        WM_TIMER => {
            match wp {
                TIMER_SMOKE => PostQuitMessage(0),
                TIMER_AUTO => with_app(|app| app.on_auto_tick()),
                TIMER_IDLE => with_app(|app| app.on_idle_tick()),
                TIMER_FOLLOW => with_app(|app| app.on_follow_tick()),
                TIMER_UPDATE => with_app(|app| app.on_update_timer()),
                TIMER_RESTART => with_app(|app| app.on_restart_timer()),
                _ => with_app(|app| app.on_topmost_timer()),
            }
            0
        }
        WM_DISPLAYCHANGE | WM_DPICHANGED | WM_SETTINGCHANGE => {
            with_app(|app| app.on_display_change());
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn menu_key(vk: u16) -> Option<Key> {
    Some(match vk {
        VK_UP => Key::Up,
        VK_DOWN => Key::Down,
        VK_HOME => Key::Home,
        VK_END => Key::End,
        VK_LEFT => Key::Left,
        VK_RIGHT => Key::Right,
        VK_PRIOR => Key::PageUp,
        VK_NEXT => Key::PageDown,
        VK_RETURN => Key::Enter,
        VK_SPACE => Key::Space,
        VK_ESCAPE => Key::Esc,
        _ => return None,
    })
}

unsafe extern "system" fn hud_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let x = (lp & 0xFFFF) as u16 as i16 as i32;
    let y = ((lp >> 16) & 0xFFFF) as u16 as i16 as i32;
    match msg {
        WM_LBUTTONDOWN | WM_RBUTTONDOWN => {
            SetCapture(hwnd);
            with_app(|app| app.hud_mouse_down(x, y, msg == WM_RBUTTONDOWN));
            0
        }
        WM_LBUTTONUP | WM_RBUTTONUP => {
            ReleaseCapture();
            with_app(|app| app.hud_mouse_up(x, y, msg == WM_RBUTTONUP));
            0
        }
        WM_MOUSEMOVE => {
            let mut track = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
            TrackMouseEvent(&mut track);
            with_app(|app| app.hud_mouse_move(x, y, wp & 0x0001 != 0));
            0
        }
        WM_MOUSELEAVE => {
            with_app(|app| app.hud_mouse_leave());
            0
        }
        WM_MOUSEWHEEL => {
            let delta = ((wp >> 16) & 0xFFFF) as u16 as i16 as i32;
            let notches = if delta.abs() >= 120 { delta / 120 } else { delta.signum() };
            with_app(|app| app.hud_wheel(notches));
            0
        }
        WM_KEYDOWN => {
            if let Some(key) = menu_key(wp as u16) {
                with_app(|app| app.hud_key(key));
            }
            0
        }
        WM_TIMER => {
            with_app(|app| app.hud_title_tick());
            0
        }
        WM_ERASEBKGND => 1,
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn warn(smoke: bool, text: &str) {
    if smoke {
        return;
    }
    unsafe { MessageBoxW(null_mut(), wide(text).as_ptr(), wide("reDraw").as_ptr(), MB_OK | MB_ICONWARNING) };
}

unsafe fn make_dib(dc: HDC, w: i32, h: i32) -> (HBITMAP, *mut u32) {
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
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
    (dib, bits as *mut u32)
}

/// `--dump <base>`: draw the menu off-screen in a few states and save `<base>-*.bmp`. Shows nothing.
pub fn dump(base: &str) -> i32 {
    use crate::menu::{Action, Group, Section};
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let Some(mut hud) = Hud::create(GetModuleHandleW(null()), Some(hud_proc)) else { return 7 };
        let mut cfg = Config::default();
        hud.menu.set_update(Status::UpToDate);
        hud.dump_bmp(&cfg, &format!("{base}-default.bmp"));
        for s in [Section::Presets, Section::Output, Section::Preview] {
            hud.menu.activate(&mut cfg, Action::Header(s), 1);
        }
        hud.dump_bmp(&cfg, &format!("{base}-open.bmp"));
        hud.menu.activate(&mut cfg, Action::AdsVia, 1);
        hud.menu.activate(&mut cfg, Action::AdsVia, 1);
        for _ in 0..4 {
            hud.menu.key(&mut cfg, Key::Down);
        }
        hud.menu.mouse_move(&mut cfg, 60, 120, false);
        hud.dump_bmp(&cfg, &format!("{base}-focus.bmp"));

        // Experimental [!]: closed groups, then everything open with every feature on.
        let mut cfg = Config::default();
        hud.menu = crate::menu::Menu::new();
        for s in [Section::Profile, Section::Layers, Section::Layer] {
            hud.menu.activate(&mut cfg, Action::Header(s), 1);
        }
        hud.menu.activate(&mut cfg, Action::Header(Section::Experimental), 1);
        hud.dump_bmp(&cfg, &format!("{base}-experimental.bmp"));
        for g in [Group::Blend, Group::Dynamics, Group::Trace, Group::Zoom, Group::Aspect, Group::Auto] {
            hud.menu.activate(&mut cfg, Action::Group(g), 1);
        }
        cfg.exp.dynamics.enabled = true;
        cfg.exp.trace.enabled = true;
        cfg.exp.zoom.enabled = true;
        cfg.exp.aspect.enabled = true;
        cfg.exp.auto.enabled = true;
        hud.dump_bmp(&cfg, &format!("{base}-experimental-open.bmp"));

        // Only the Aspect ratio group open, so it fits the picture.
        hud.menu = crate::menu::Menu::new();
        for s in [Section::Profile, Section::Layers, Section::Layer] {
            hud.menu.activate(&mut cfg, Action::Header(s), 1);
        }
        hud.menu.activate(&mut cfg, Action::Header(Section::Experimental), 1);
        hud.menu.activate(&mut cfg, Action::Group(Group::Aspect), 1);
        hud.dump_bmp(&cfg, &format!("{base}-aspect.bmp"));
        hud.menu = crate::menu::Menu::new();
        for s in [Section::Profile, Section::Layers, Section::Layer] {
            hud.menu.activate(&mut cfg, Action::Header(s), 1);
        }
        hud.menu.activate(&mut cfg, Action::Header(Section::Experimental), 1);
        for g in [Group::Blend, Group::Dynamics, Group::Trace, Group::Zoom, Group::Aspect, Group::Auto] {
            hud.menu.activate(&mut cfg, Action::Group(g), 1);
        }

        // Display section, and the menu at 2x.
        hud.menu = crate::menu::Menu::new();
        hud.menu.activate(&mut cfg, Action::Header(Section::Display), 1);
        hud.menu.set_info(crate::display::Info { scale: 2, ui_scale: 2, refresh: 144 });
        hud.dump_bmp(&cfg, &format!("{base}-display.bmp"));
        hud.set_scale(2, &cfg);
        hud.dump_bmp(&cfg, &format!("{base}-ui2.bmp"));
        hud.set_scale(1, &cfg);

        // Layer extras (custom color, transform) on a chevron, and the States section with keys bound.
        let mut cfg = Config::default();
        cfg.hip.layers[0] = crate::crosshair::Layer::new(crate::crosshair::Kind::Chevron, 0xFF9500);
        cfg.hip.layers[0].mirror = crate::crosshair::Mirror::Both;
        cfg.hip.layers[0].opacity = 6;
        cfg.hip.layers[0].ox = 3;
        cfg.hip.layers[0].cycle = 2;
        cfg.states.crouch.key = 0x11;
        cfg.states.sprint.key = 0x10;
        cfg.states.sprint.toggle = true;
        cfg.states.sprint.color = Some(0xFF3B30);
        hud.menu = crate::menu::Menu::new();
        for g in [Group::Color, Group::Transform] {
            hud.menu.activate(&mut cfg, Action::Group(g), 1);
        }
        hud.dump_bmp(&cfg, &format!("{base}-layer-open.bmp"));
        hud.menu = crate::menu::Menu::new();
        for s in [Section::Profile, Section::Layers, Section::Layer] {
            hud.menu.activate(&mut cfg, Action::Header(s), 1);
        }
        for s in [Section::States, Section::Output] {
            hud.menu.activate(&mut cfg, Action::Header(s), 1);
        }
        for g in [Group::Crouch, Group::Sprint, Group::Jump] {
            hud.menu.activate(&mut cfg, Action::Group(g), 1);
        }
        hud.dump_bmp(&cfg, &format!("{base}-states.bmp"));

        // Updates section with a newer version installed, and the ForceQuit button at the bottom.
        hud.menu = crate::menu::Menu::new();
        hud.menu.set_update(Status::Installed("0.2.9".to_string()));
        for s in [Section::Profile, Section::Layers, Section::Layer] {
            hud.menu.activate(&mut cfg, Action::Header(s), 1);
        }
        hud.menu.activate(&mut cfg, Action::Header(Section::Updates), 1);
        hud.dump_bmp(&cfg, &format!("{base}-updates.bmp"));
    }
    0
}

/// Runs the overlay until quit. Returns the process exit code.
pub fn run(cfg_path: PathBuf, smoke: bool) -> i32 {
    unsafe {
        // `multi-instance` is for test builds only: a copy next to a real one, without its hotkeys.
        let probe = cfg!(feature = "multi-instance");
        let name = wide(if probe { "reDraw.SingleInstance.probe" } else { "reDraw.SingleInstance" });
        let mutex = CreateMutexW(null(), 0, name.as_ptr());
        if mutex.is_null() || GetLastError() == ERROR_ALREADY_EXISTS {
            return 0;
        }

        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        // Only a missing file is replaced by a fresh default. Any other read
        // problem runs on defaults in memory and leaves the user's file alone.
        let cfg = match read_config(&cfg_path) {
            Ok(cfg) => cfg,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let cfg = Config::default();
                let _ = std::fs::write(&cfg_path, cfg.serialize());
                cfg
            }
            Err(_) => Config::default(),
        };

        let hinstance = GetModuleHandleW(null());
        let class = wide("reDrawOverlay");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(wndproc),
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
        if RegisterClassExW(&wc) == 0 {
            return 2;
        }

        let ex = WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
        let hwnd = CreateWindowExW(ex, class.as_ptr(), wide("reDraw").as_ptr(), WS_POPUP, 0, 0, N as i32, N as i32, null_mut(), null_mut(), hinstance, null());
        if hwnd.is_null() {
            return 3;
        }

        let mem_dc = CreateCompatibleDC(null_mut());
        let (dib, bits) = make_dib(mem_dc, CROSS_MAX, CROSS_MAX);
        if dib.is_null() || bits.is_null() {
            return 4;
        }
        let old_bitmap = SelectObject(mem_dc, dib);

        // The spray-trace layer: hidden until the feature is on.
        let trace_hwnd = CreateWindowExW(ex, class.as_ptr(), wide("reDraw trace").as_ptr(), WS_POPUP, 0, 0, 1, 1, null_mut(), null_mut(), hinstance, null());
        let trace_dc = CreateCompatibleDC(null_mut());
        let (trace_dib, trace_bits) = make_dib(trace_dc, TRACE_MAX, TRACE_MAX);
        if trace_hwnd.is_null() || trace_dib.is_null() || trace_bits.is_null() {
            return 8;
        }
        let trace_old = SelectObject(trace_dc, trace_dib);
        let trace = TraceWin { hwnd: trace_hwnd, dc: trace_dc, dib: trace_dib, old: trace_old, bits: trace_bits, buf: Vec::new(), shown: false };

        let Some(hud) = Hud::create(hinstance, Some(hud_proc)) else { return 7 };
        let now = Instant::now();
        let live = Live {
            dynst: Dyn::new(),
            frame: NEUTRAL,
            tracer: Tracer::new(),
            picker: Picker::new(),
            auto_color: None,
            firing: false,
            keys: [false; 4],
            last_input: now,
            last_frame: now,
            states: Tracker::new(),
            morph: Morph::new(),
            demo: None,
            restart_pending: false,
            hues: [0; 4],
            started: now,
        };
        let pid = std::process::id();
        let target = monitors::resolve(cfg.display.monitor, pid, None);
        let mut app = App { cfg, cfg_path, hwnd, mem_dc, bits, hud, lenses: Lenses::new(hinstance), pacer: FramePacer::new(hwnd), trace, live, target, scale: 1, pid, ads: false, visible: true, present_ok: false };

        // Test builds sit next to a real copy, so their hotkeys carry Ctrl (and Shift for quit).
        let plain = if probe { MOD_CONTROL | MOD_NOREPEAT } else { MOD_NOREPEAT };
        let mut failed: Vec<&str> = Vec::new();
        if RegisterHotKey(hwnd, HK_SHOW, plain, VK_F9 as u32) == 0 {
            failed.push("F9 (show/hide)");
        }
        if RegisterHotKey(hwnd, HK_MENU, plain, VK_F10 as u32) == 0 {
            failed.push("F10 (menu)");
        }
        if RegisterHotKey(hwnd, HK_RELOAD, plain, VK_F11 as u32) == 0 {
            failed.push("F11 (reload config)");
        }
        let quit_ok = RegisterHotKey(hwnd, HK_QUIT, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT | if probe { MOD_SHIFT } else { 0 }, VK_F9 as u32) != 0;
        if !quit_ok {
            failed.push("Ctrl+Alt+F9 (quit)");
        }
        if !app.register_key_hotkey() {
            failed.push("the ADS key from the config");
        }
        if !failed.is_empty() && !probe {
            warn(smoke, &format!("Another program already uses: {}.\nThose shortcuts will not work.", failed.join(", ")));
            if !quit_ok {
                // An overlay with no way to quit would have to be killed from Task Manager.
                DestroyWindow(hwnd);
                return 6;
            }
        }
        SetTimer(hwnd, TIMER_TOPMOST, TOPMOST_MS, None);

        app.present();
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        app.reassert_topmost();
        app.apply_display();
        app.apply_experimental();
        if let Ok(exe) = std::env::current_exe() {
            update::clean_leftovers(&exe);
        }
        if !smoke {
            app.apply_autoupdate();
        }
        if smoke {
            // Exercise the menu window too: draw it once, then quit.
            app.hud.show(&app.cfg);
            SetTimer(hwnd, TIMER_SMOKE, 1200, None);
        }
        APP.with(|a| *a.borrow_mut() = Some(app));

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        let ok = APP.with(|a| a.borrow().as_ref().map(|app| app.present_ok && (!smoke || app.hud.visible)).unwrap_or(false));
        APP.with(|a| *a.borrow_mut() = None);
        SelectObject(mem_dc, old_bitmap);
        DeleteObject(dib);
        DeleteDC(mem_dc);
        if smoke && !ok {
            return 5;
        }
        0
    }
}
