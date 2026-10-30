//! Spray trace: a tracer drawn from your own *relative* mouse movement.
//!
//! It never uses the cursor position. Games that hold the cursor at the screen
//! center (and warp it back every frame) still produce correct relative motion
//! through raw input, and a warp itself produces no raw event, so there is
//! nothing to glitch on. Movement is summed between frames (mice report at
//! 1000 Hz and up) and drawn once per display refresh, so the line is smooth
//! at any polling rate. Pure model: the caller feeds counts and a time step
//! and asks for a pixel buffer.

use std::collections::VecDeque;

use crate::experimental::{Trace as Cfg, TraceActive, TraceReturn, TraceStyle};

const MAX_POINTS: usize = 600;
/// px/s of mouse speed (in counts/s) that counts as "fast" for speed coloring
const FAST_COUNTS_PER_SEC: f32 = 4000.0;
const SMOOTH_TAU: [f32; 4] = [0.0, 0.025, 0.06, 0.12];
const FADE_RETURN_RATE: f32 = 1.2;
const SNAP_RETURN_RATE: f32 = 14.0;
/// how long the mouse must be still before the line starts coming home
const STILL_SECS: f32 = 0.06;

#[derive(Clone, Copy, Debug)]
struct Pt {
    x: f32,
    y: f32,
    age: f32,
    speed: f32,
}

#[derive(Debug)]
pub struct Tracer {
    pos: (f32, f32),
    vis: (f32, f32),
    pending: (f32, f32),
    pts: VecDeque<Pt>,
    held: bool,
    still: f32,
    speed: f32,
    was_active: bool,
}

fn lerp(a: u32, b: u32, t: f32) -> u32 {
    let ch = |s: u32| {
        let (x, y) = (((a >> s) & 0xFF) as f32, ((b >> s) & 0xFF) as f32);
        ((x + (y - x) * t.clamp(0.0, 1.0)).round() as u32) << s
    };
    ch(16) | ch(8) | ch(0)
}

/// Slow movement is cyan, medium yellow, fast red.
pub fn speed_color(counts_per_sec: f32) -> u32 {
    let t = (counts_per_sec / FAST_COUNTS_PER_SEC).clamp(0.0, 1.0);
    if t < 0.5 {
        lerp(0x00E5FF, 0xFFE500, t * 2.0)
    } else {
        lerp(0xFFE500, 0xFF3B30, (t - 0.5) * 2.0)
    }
}

impl Tracer {
    pub fn new() -> Tracer {
        Tracer { pos: (0.0, 0.0), vis: (0.0, 0.0), pending: (0.0, 0.0), pts: VecDeque::new(), held: false, still: 0.0, speed: 0.0, was_active: false }
    }

    pub fn reset(&mut self) {
        *self = Tracer::new();
    }

    fn tracing(&self, cfg: &Cfg) -> bool {
        match cfg.active {
            TraceActive::Always => true,
            TraceActive::Fire => self.held,
        }
    }

    /// Raw relative mouse counts. Ignored while tracing is not active.
    pub fn feed(&mut self, cfg: &Cfg, dx: i32, dy: i32) {
        if cfg.enabled && self.tracing(cfg) {
            self.pending.0 += dx as f32;
            self.pending.1 += dy as f32;
        }
    }

    /// The fire button went down or up. A new burst starts from the center.
    pub fn set_fire(&mut self, down: bool) {
        if down && !self.held {
            self.pos = (0.0, 0.0);
            self.vis = (0.0, 0.0);
            self.pending = (0.0, 0.0);
            self.pts.clear();
        }
        self.held = down;
    }

    /// Whether anything is still moving or on screen, so the frame clock has to run.
    pub fn active(&self) -> bool {
        !self.pts.is_empty() || self.pos.0.abs() > 0.3 || self.pos.1.abs() > 0.3 || self.vis.0.abs() > 0.3 || self.vis.1.abs() > 0.3 || self.pending != (0.0, 0.0)
    }

    /// Advance by `dt` seconds. Returns true if the picture needs to be redrawn.
    pub fn step(&mut self, cfg: &Cfg, dt: f32) -> bool {
        let dt = dt.clamp(0.0001, 0.25);
        let range = cfg.range as f32;
        if !cfg.enabled {
            let had = self.was_active;
            self.reset();
            return had;
        }

        let px_per_count = cfg.scale as f32 * 0.1;
        let (cx, cy) = self.pending;
        self.pending = (0.0, 0.0);
        let moved = cx != 0.0 || cy != 0.0;
        self.pos.0 = (self.pos.0 + cx * px_per_count).clamp(-range, range);
        self.pos.1 = (self.pos.1 + cy * px_per_count).clamp(-range, range);
        let inst = ((cx * cx + cy * cy).sqrt()) / dt;
        self.speed += (inst - self.speed) * (1.0 - (-dt * 20.0).exp());
        self.still = if moved { 0.0 } else { self.still + dt };

        let can_return = self.still > STILL_SECS
            && match cfg.ret {
                TraceReturn::Hold => false,
                _ => match cfg.active {
                    TraceActive::Fire => !self.held,
                    TraceActive::Always => true,
                },
            };
        if can_return {
            let rate = if cfg.ret == TraceReturn::Snap { SNAP_RETURN_RATE } else { FADE_RETURN_RATE };
            let k = (-rate * dt).exp();
            self.pos = (self.pos.0 * k, self.pos.1 * k);
            if self.pos.0.abs() < 0.25 && self.pos.1.abs() < 0.25 {
                self.pos = (0.0, 0.0);
            }
        }

        let prev_vis = self.vis;
        let tau = SMOOTH_TAU[cfg.smooth.min(3) as usize];
        if tau == 0.0 {
            self.vis = self.pos;
        } else {
            let a = 1.0 - (-dt / tau).exp();
            self.vis = (self.vis.0 + (self.pos.0 - self.vis.0) * a, self.vis.1 + (self.pos.1 - self.vis.1) * a);
            if (self.pos.0 - self.vis.0).abs() < 0.05 && (self.pos.1 - self.vis.1).abs() < 0.05 {
                self.vis = self.pos;
            }
        }

        let fade = cfg.fade as f32 / 10.0;
        for p in self.pts.iter_mut() {
            p.age += dt;
        }
        while self.pts.front().is_some_and(|p| p.age > fade) {
            self.pts.pop_front();
        }
        // A trail point is added only when the drawn position really moved, so a
        // resting line leaves its old points to age out instead of refilling.
        let vis_changed = (self.vis.0 - prev_vis.0).abs() > 1e-4 || (self.vis.1 - prev_vis.1).abs() > 1e-4;
        let last = self.pts.back().map(|p| (p.x, p.y));
        let far_enough = last.map_or(vis_changed, |(x, y)| (self.vis.0 - x).abs() >= 0.5 || (self.vis.1 - y).abs() >= 0.5);
        if vis_changed && far_enough && (self.tracing(cfg) || moved) {
            if self.pts.is_empty() {
                self.pts.push_back(Pt { x: 0.0, y: 0.0, age: 0.0, speed: 0.0 });
            }
            if self.pts.len() >= MAX_POINTS {
                self.pts.pop_front();
            }
            self.pts.push_back(Pt { x: self.vis.0, y: self.vis.1, age: 0.0, speed: self.speed });
        }

        let now = self.active();
        let redraw = now || self.was_active;
        self.was_active = now;
        redraw
    }

    /// Draw into `out` (side x side, premultiplied ARGB, center at side/2).
    pub fn render(&self, cfg: &Cfg, out: &mut [u32], side: i32) {
        for p in out.iter_mut() {
            *p = 0;
        }
        let c = side / 2;
        let fade = cfg.fade as f32 / 10.0;
        let w = cfg.width.clamp(1, 3);
        let color_for = |speed: f32| if cfg.speed_color { speed_color(speed) } else { cfg.color };
        if matches!(cfg.style, TraceStyle::Trail | TraceStyle::Both) {
            let v: Vec<&Pt> = self.pts.iter().collect();
            for pair in v.windows(2) {
                let a = (1.0 - pair[1].age / fade).clamp(0.0, 1.0);
                line(out, side, (c + pair[0].x.round() as i32, c + pair[0].y.round() as i32), (c + pair[1].x.round() as i32, c + pair[1].y.round() as i32), w, color_for(pair[1].speed), a);
            }
        }
        if matches!(cfg.style, TraceStyle::Line | TraceStyle::Both) && (self.vis.0.abs() > 0.3 || self.vis.1.abs() > 0.3) {
            let end = (c + self.vis.0.round() as i32, c + self.vis.1.round() as i32);
            line(out, side, (c, c), end, w, color_for(self.speed), 1.0);
            plot(out, side, end.0, end.1, w + 1, color_for(self.speed), 1.0);
        }
    }
}

/// How long the scripted spray lasts, in seconds.
pub const DEMO_SECS: f32 = 2.4;

/// Where the made-up spray has pulled to after `t` seconds, in mouse counts: a hard pull down that
/// eases off, with a side-to-side sway that builds up.
fn demo_path(t: f32) -> (f32, f32) {
    let pull = 420.0 * (1.0 - (-2.2 * t).exp());
    let sway = 160.0 * (t * std::f32::consts::TAU / 1.1).sin() * (t / 0.5).min(1.0);
    (sway, pull)
}

/// A scripted spray for the menu's Test link. It hands the tracer the mouse counts of an invented
/// recoil pull, so you can see your color, width, fade, smoothing and return settings at work even
/// with the menu open (where real mouse movement is, rightly, not traced).
#[derive(Debug)]
pub struct Demo {
    t: f32,
    carry: (f32, f32),
}

impl Demo {
    pub fn new() -> Demo {
        Demo { t: 0.0, carry: (0.0, 0.0) }
    }

    /// The mouse counts for the next `dt` seconds, or `None` once the spray is over.
    pub fn step(&mut self, dt: f32) -> Option<(i32, i32)> {
        if self.t >= DEMO_SECS {
            return None;
        }
        let (x0, y0) = demo_path(self.t);
        self.t = (self.t + dt.clamp(0.0, 0.1)).min(DEMO_SECS);
        let (x1, y1) = demo_path(self.t);
        // Whole counts only; the remainder carries into the next step so short steps still add up.
        self.carry.0 += x1 - x0;
        self.carry.1 += y1 - y0;
        let out = (self.carry.0.trunc() as i32, self.carry.1.trunc() as i32);
        self.carry = (self.carry.0 - out.0 as f32, self.carry.1 - out.1 as f32);
        Some(out)
    }
}

/// Set a `w` x `w` square of pixels, keeping the stronger of old and new alpha.
fn plot(out: &mut [u32], side: i32, x: i32, y: i32, w: i32, rgb: u32, alpha: f32) {
    let a = (alpha.clamp(0.0, 1.0) * 255.0).round() as u32;
    if a == 0 {
        return;
    }
    let ch = |s: u32| ((((rgb >> s) & 0xFF) * a + 127) / 255) << s;
    let px = (a << 24) | ch(16) | ch(8) | ch(0);
    let lo = -(w - 1) / 2;
    for dy in lo..lo + w {
        for dx in lo..lo + w {
            let (xx, yy) = (x + dx, y + dy);
            if xx >= 0 && yy >= 0 && xx < side && yy < side {
                let i = (yy * side + xx) as usize;
                if (out[i] >> 24) < a {
                    out[i] = px;
                }
            }
        }
    }
}

/// Bresenham line, whole pixels only.
fn line(out: &mut [u32], side: i32, from: (i32, i32), to: (i32, i32), w: i32, rgb: u32, alpha: f32) {
    let (mut x, mut y) = from;
    let (dx, dy) = ((to.0 - x).abs(), -(to.1 - y).abs());
    let (sx, sy) = (if x < to.0 { 1 } else { -1 }, if y < to.1 { 1 } else { -1 });
    let mut err = dx + dy;
    loop {
        plot(out, side, x, y, w, rgb, alpha);
        if (x, y) == to {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::experimental::Experimental;

    #[test]
    fn the_demo_spray_adds_up_to_its_path_and_then_ends() {
        let mut d = Demo::new();
        let (mut sum, mut steps) = ((0i32, 0i32), 0);
        let mut biggest = 0;
        while let Some((dx, dy)) = d.step(1.0 / 144.0) {
            sum = (sum.0 + dx, sum.1 + dy);
            biggest = biggest.max(dx.abs()).max(dy.abs());
            steps += 1;
            assert!(steps < 1000, "it must end");
        }
        let (ex, ey) = demo_path(DEMO_SECS);
        assert!((sum.0 as f32 - ex).abs() <= 2.0 && (sum.1 as f32 - ey).abs() <= 2.0, "{sum:?} vs ({ex}, {ey})");
        assert!(sum.1 > 300, "it pulls down hard: {sum:?}");
        assert!(biggest <= 12, "smooth steps at 144 Hz, not jumps: biggest {biggest}");
        assert!((steps as f32 / 144.0 - DEMO_SECS).abs() < 0.05);
        assert_eq!(d.step(0.01), None, "and stays over");
        // A slow frame rate gives the same path in bigger steps.
        let mut slow = Demo::new();
        let mut total = (0, 0);
        while let Some((dx, dy)) = slow.step(0.05) {
            total = (total.0 + dx, total.1 + dy);
        }
        assert!((total.1 - sum.1).abs() <= 3, "{total:?} vs {sum:?}");
    }

    #[test]
    fn the_demo_draws_a_line_through_the_real_tracer_in_either_mode() {
        for active in [TraceActive::Fire, TraceActive::Always] {
            let mut c = cfg();
            c.active = active;
            c.ret = TraceReturn::Hold;
            let mut t = Tracer::new();
            // What the overlay does for the Test link: press "fire", then feed the script.
            t.set_fire(true);
            let mut d = Demo::new();
            while let Some((dx, dy)) = d.step(1.0 / 144.0) {
                t.feed(&c, dx, dy);
                t.step(&c, 1.0 / 144.0);
            }
            let side = 2 * c.range + 1;
            let mut out = vec![0u32; (side * side) as usize];
            t.render(&c, &mut out, side);
            let lit = out.iter().filter(|p| **p >> 24 != 0).count();
            assert!(lit > 40, "{active:?}: the demo spray is visible ({lit} px)");
        }
    }

    fn cfg() -> Cfg {
        let mut c = Experimental::default().trace;
        c.enabled = true;
        c.smooth = 0;
        c
    }

    fn run(t: &mut Tracer, c: &Cfg, secs: f32) {
        for _ in 0..(secs * 240.0) as usize {
            t.step(c, 1.0 / 240.0);
        }
    }

    fn lit(out: &[u32]) -> usize {
        out.iter().filter(|p| **p >> 24 != 0).count()
    }

    #[test]
    fn nothing_is_traced_unless_firing_in_fire_mode() {
        let c = cfg();
        let mut t = Tracer::new();
        t.feed(&c, 500, 500);
        run(&mut t, &c, 0.1);
        assert!(!t.active(), "movement without fire is dropped");
        t.set_fire(true);
        t.feed(&c, 100, 0);
        t.step(&c, 1.0 / 120.0);
        assert!(t.active());
    }

    #[test]
    fn always_mode_traces_without_fire() {
        let mut c = cfg();
        c.active = TraceActive::Always;
        let mut t = Tracer::new();
        t.feed(&c, 100, 0);
        t.step(&c, 1.0 / 120.0);
        assert!(t.active());
    }

    #[test]
    fn counts_scale_to_pixels_and_clamp_to_the_range() {
        let mut c = cfg();
        c.scale = 5; // 0.5 px per count
        let mut t = Tracer::new();
        t.set_fire(true);
        t.feed(&c, 100, -60);
        t.step(&c, 1.0 / 120.0);
        assert_eq!((t.pos.0.round() as i32, t.pos.1.round() as i32), (50, -30));
        t.feed(&c, 100_000, 100_000);
        t.step(&c, 1.0 / 120.0);
        assert_eq!((t.pos.0, t.pos.1), (128.0, 128.0), "clamped to the 128px range");
    }

    #[test]
    fn a_new_burst_starts_from_the_center() {
        let c = cfg();
        let mut t = Tracer::new();
        t.set_fire(true);
        t.feed(&c, 300, 300);
        t.step(&c, 1.0 / 120.0);
        t.set_fire(false);
        t.set_fire(true);
        assert_eq!(t.pos, (0.0, 0.0));
        assert!(t.pts.is_empty());
    }

    #[test]
    fn snap_returns_to_center_fast_and_hold_never_does() {
        for (ret, home) in [(TraceReturn::Snap, true), (TraceReturn::Fade, false), (TraceReturn::Hold, false)] {
            let mut c = cfg();
            c.ret = ret;
            let mut t = Tracer::new();
            t.set_fire(true);
            t.feed(&c, 400, 200);
            t.step(&c, 1.0 / 120.0);
            t.set_fire(false);
            run(&mut t, &c, 0.6);
            let at_center = t.pos == (0.0, 0.0);
            assert_eq!(at_center, home, "{ret:?} after 0.6s: pos {:?}", t.pos);
        }
        let mut c = cfg();
        c.ret = TraceReturn::Fade;
        let mut t = Tracer::new();
        t.set_fire(true);
        t.feed(&c, 400, 0);
        t.step(&c, 1.0 / 120.0);
        t.set_fire(false);
        run(&mut t, &c, 6.0);
        assert_eq!(t.pos, (0.0, 0.0), "fade return gets there eventually");
    }

    #[test]
    fn holding_fire_without_moving_does_not_pull_the_line_home() {
        let mut c = cfg();
        c.ret = TraceReturn::Snap;
        let mut t = Tracer::new();
        t.set_fire(true);
        t.feed(&c, 400, 0);
        t.step(&c, 1.0 / 120.0);
        run(&mut t, &c, 1.0);
        assert!(t.pos.0 > 100.0, "still held: {:?}", t.pos);
    }

    #[test]
    fn trail_points_fade_out_by_age() {
        let mut c = cfg();
        c.fade = 5;
        c.ret = TraceReturn::Hold;
        let mut t = Tracer::new();
        t.set_fire(true);
        for _ in 0..20 {
            t.feed(&c, 30, 10);
            t.step(&c, 1.0 / 120.0);
        }
        assert!(t.pts.len() > 5);
        run(&mut t, &c, 0.7);
        assert!(t.pts.is_empty(), "all trail points are older than 0.5s");
    }

    #[test]
    fn smoothing_lags_then_catches_up_without_overshoot() {
        let mut c = cfg();
        c.smooth = 3;
        c.ret = TraceReturn::Hold;
        let mut t = Tracer::new();
        t.set_fire(true);
        t.feed(&c, 1000, 0);
        t.step(&c, 1.0 / 240.0);
        assert!(t.vis.0 > 0.0 && t.vis.0 < t.pos.0 * 0.5, "visual position trails the real one: {:?} vs {:?}", t.vis, t.pos);
        let mut prev = t.vis.0;
        for _ in 0..600 {
            t.step(&c, 1.0 / 240.0);
            assert!(t.vis.0 >= prev - 1e-4 && t.vis.0 <= t.pos.0 + 1e-3, "monotonic, no overshoot");
            prev = t.vis.0;
        }
        assert_eq!(t.vis, t.pos, "settles exactly");
    }

    #[test]
    fn center_locked_cursor_jitter_is_stable() {
        // A game that recenters the cursor every frame still delivers relative counts.
        // Tiny alternating counts must not make the line jump or produce huge steps.
        let mut c = cfg();
        c.smooth = 2;
        c.active = TraceActive::Always;
        let mut t = Tracer::new();
        let mut last = t.vis;
        for i in 0..2000 {
            t.feed(&c, if i % 2 == 0 { 3 } else { -3 }, 0);
            t.step(&c, 1.0 / 240.0);
            assert!((t.vis.0 - last.0).abs() < 1.0, "no jumps");
            last = t.vis;
        }
    }

    #[test]
    fn many_events_per_frame_equal_one_big_event() {
        let c = cfg();
        let mut a = Tracer::new();
        let mut b = Tracer::new();
        a.set_fire(true);
        b.set_fire(true);
        for _ in 0..8 {
            a.feed(&c, 5, -2);
        }
        b.feed(&c, 40, -16);
        a.step(&c, 1.0 / 144.0);
        b.step(&c, 1.0 / 144.0);
        assert_eq!(a.pos, b.pos, "8 kHz mice are summed per frame");
    }

    #[test]
    fn speed_color_runs_cyan_yellow_red() {
        assert_eq!(speed_color(0.0), 0x00E5FF);
        assert_eq!(speed_color(FAST_COUNTS_PER_SEC * 0.5), 0xFFE500);
        assert_eq!(speed_color(FAST_COUNTS_PER_SEC * 5.0), 0xFF3B30);
    }

    #[test]
    fn render_draws_a_line_and_head_and_clears_each_time() {
        let mut c = cfg();
        c.style = TraceStyle::Line;
        c.ret = TraceReturn::Hold;
        let side = 2 * c.range + 1;
        let mut buf = vec![0u32; (side * side) as usize];
        let mut t = Tracer::new();
        t.render(&c, &mut buf, side);
        assert_eq!(lit(&buf), 0, "nothing at rest");
        t.set_fire(true);
        t.feed(&c, 200, 0);
        t.step(&c, 1.0 / 120.0);
        t.render(&c, &mut buf, side);
        let ctr = c.range;
        assert!(buf[(ctr * side + ctr + 10) as usize] >> 24 == 0xFF, "pixel on the line");
        assert!(lit(&buf) > 50);
        assert_eq!(buf[(ctr * side + ctr + 10) as usize] & 0xFFFFFF, c.color, "base color, fully opaque");
        t.reset();
        t.render(&c, &mut buf, side);
        assert_eq!(lit(&buf), 0, "buffer is cleared");
    }

    #[test]
    fn width_and_style_change_the_picture() {
        let mut c = cfg();
        c.ret = TraceReturn::Hold;
        c.style = TraceStyle::Line;
        let side = 2 * c.range + 1;
        let mut t = Tracer::new();
        t.set_fire(true);
        t.feed(&c, 300, 100);
        t.step(&c, 1.0 / 120.0);
        let mut thin = vec![0u32; (side * side) as usize];
        t.render(&c, &mut thin, side);
        c.width = 3;
        let mut thick = vec![0u32; (side * side) as usize];
        t.render(&c, &mut thick, side);
        assert!(lit(&thick) > lit(&thin) * 2);
        c.style = TraceStyle::Trail;
        let mut trail = vec![0u32; (side * side) as usize];
        t.render(&c, &mut trail, side);
        assert_ne!(trail, thick);
    }

    #[test]
    fn trail_alpha_falls_with_age_and_never_leaves_the_buffer() {
        let mut c = cfg();
        c.style = TraceStyle::Trail;
        c.ret = TraceReturn::Hold;
        c.fade = 10;
        let side = 2 * c.range + 1;
        let mut t = Tracer::new();
        t.set_fire(true);
        for _ in 0..30 {
            t.feed(&c, 60, 20);
            t.step(&c, 1.0 / 120.0);
        }
        run(&mut t, &c, 0.5);
        let mut buf = vec![0u32; (side * side) as usize];
        t.render(&c, &mut buf, side);
        let alphas: Vec<u32> = buf.iter().map(|p| p >> 24).filter(|a| *a != 0).collect();
        assert!(alphas.iter().any(|a| *a < 255), "older points are dimmer");
        assert!(alphas.iter().all(|a| *a <= 255));
        assert_eq!(buf.len(), (side * side) as usize);
    }

    #[test]
    fn disabling_clears_everything_once() {
        let mut c = cfg();
        let mut t = Tracer::new();
        t.set_fire(true);
        t.feed(&c, 300, 300);
        assert!(t.step(&c, 1.0 / 120.0));
        c.enabled = false;
        assert!(t.step(&c, 1.0 / 120.0), "one last redraw to clear the window");
        assert!(!t.step(&c, 1.0 / 120.0));
        assert!(!t.active());
    }

    #[test]
    fn point_history_is_capped() {
        let mut c = cfg();
        c.ret = TraceReturn::Hold;
        c.fade = 30;
        c.range = 256;
        c.active = TraceActive::Always;
        let mut t = Tracer::new();
        for i in 0..3000 {
            t.feed(&c, if i % 2 == 0 { 20 } else { -19 }, 7);
            t.step(&c, 1.0 / 240.0);
        }
        assert!(t.pts.len() <= MAX_POINTS);
    }
}
