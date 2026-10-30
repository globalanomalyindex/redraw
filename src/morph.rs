//! The hip fire to ADS transition: instead of cutting from one crosshair to the other, they
//! crossfade over a short, eased time. Pure and clocked by the caller, so it costs nothing at rest.
//!
//! The mix is quantized to 16 levels, so the overlay redraws only when what you see changes, and
//! never more often than the frame clock ticks.

use crate::crosshair::N;

/// Morph times the menu offers, in milliseconds. 0 is an instant cut.
pub const MS: [u32; 4] = [0, 80, 150, 250];

pub const LEVELS: u8 = 16;

pub fn name(ms: u32) -> &'static str {
    match ms {
        0 => "Off",
        1..=100 => "Fast",
        101..=200 => "Medium",
        _ => "Slow",
    }
}

#[derive(Debug)]
pub struct Morph {
    /// progress, 0 = hip fire, 1 = ADS
    t: f32,
    target: f32,
}

fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

impl Morph {
    pub fn new() -> Morph {
        Morph { t: 0.0, target: 0.0 }
    }

    /// Head for a profile. With no morph time, or `instant`, it is there at once.
    pub fn go(&mut self, ads: bool, ms: u32, instant: bool) {
        self.target = if ads { 1.0 } else { 0.0 };
        if ms == 0 || instant {
            self.t = self.target;
        }
    }

    /// Advance by `dt` seconds. Returns true if the quantized level changed.
    pub fn step(&mut self, dt: f32, ms: u32) -> bool {
        let before = self.level();
        if ms == 0 {
            self.t = self.target;
        } else {
            let d = (dt.clamp(0.0, 0.25) * 1000.0 / ms as f32).max(0.0);
            self.t = if self.t < self.target { (self.t + d).min(self.target) } else { (self.t - d).max(self.target) };
        }
        self.level() != before
    }

    /// True while the next `step` can still change what is drawn.
    pub fn animating(&self) -> bool {
        self.t != self.target
    }

    /// 0 = all hip fire .. `LEVELS` = all ADS, eased.
    pub fn level(&self) -> u8 {
        (smooth(self.t) * LEVELS as f32).round() as u8
    }

    /// Which profile the thing that cannot be crossfaded (lenses, zoom) should follow.
    pub fn nearer_ads(&self) -> bool {
        self.level() * 2 >= LEVELS
    }
}

/// Premultiplied ARGB pixels `a` and `b` mixed `level` sixteenths of the way to `b`, channel by channel.
pub fn mix(a: &[u32; N * N], b: &[u32; N * N], level: u8) -> [u32; N * N] {
    let l = level.min(LEVELS) as u32;
    if l == 0 {
        return *a;
    }
    if l == LEVELS as u32 {
        return *b;
    }
    let mut out = [0u32; N * N];
    for i in 0..N * N {
        let ch = |shift: u32| ((((a[i] >> shift) & 0xFF) * (LEVELS as u32 - l) + ((b[i] >> shift) & 0xFF) * l + LEVELS as u32 / 2) / LEVELS as u32) << shift;
        out[i] = ch(24) | ch(16) | ch(8) | ch(0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(m: &mut Morph, ms: u32, secs: f32) {
        for _ in 0..(secs * 240.0) as usize {
            m.step(1.0 / 240.0, ms);
        }
    }

    #[test]
    fn it_takes_about_the_time_asked_and_stops() {
        let mut m = Morph::new();
        m.go(true, 150, false);
        assert!(m.animating() && m.level() == 0 && !m.nearer_ads());
        run(&mut m, 150, 0.075);
        assert!(m.level() > 4 && m.level() < 12, "about halfway at half the time: {}", m.level());
        run(&mut m, 150, 0.09);
        assert_eq!(m.level(), LEVELS);
        assert!(!m.animating() && m.nearer_ads(), "the clock can stop once it arrives");
    }

    #[test]
    fn it_can_turn_around_mid_way_without_a_jump() {
        let mut m = Morph::new();
        m.go(true, 200, false);
        run(&mut m, 200, 0.1);
        let mid = m.level();
        m.go(false, 200, false);
        assert_eq!(m.level(), mid, "reversing does not snap");
        run(&mut m, 200, 0.3);
        assert_eq!((m.level(), m.animating()), (0, false));
    }

    #[test]
    fn off_and_instant_cut_straight_over() {
        let mut m = Morph::new();
        m.go(true, 0, false);
        assert_eq!((m.level(), m.animating()), (LEVELS, false));
        m.go(false, 150, true);
        assert_eq!((m.level(), m.animating()), (0, false), "instant overrides the time (editing in the menu)");
        m.go(true, 150, false);
        assert!(!m.step(1.0, 0) || m.level() == LEVELS, "a step with no time arrives at once");
    }

    #[test]
    fn it_only_reports_a_change_when_the_picture_changes() {
        let mut m = Morph::new();
        m.go(true, 250, false);
        let mut changes = 0;
        for _ in 0..400 {
            if m.step(1.0 / 240.0, 250) {
                changes += 1;
            }
        }
        assert!(changes <= LEVELS as usize && changes >= 8, "{changes} redraws for the whole transition");
    }

    #[test]
    fn mix_is_exact_at_the_ends_and_linear_between() {
        let mut a = [0u32; N * N];
        let mut b = [0u32; N * N];
        a[0] = 0xFF00_0000;
        b[0] = 0x0000_0000;
        a[1] = 0x0000_0000;
        b[1] = 0xFF80_FF40;
        assert_eq!(mix(&a, &b, 0), a);
        assert_eq!(mix(&a, &b, LEVELS), b);
        let half = mix(&a, &b, 8);
        assert_eq!(half[0] >> 24, 0x80, "alpha fades out");
        assert_eq!(half[1], 0x80_40_80_20, "alpha and color fade in together");
        let q = mix(&a, &b, 4);
        assert_eq!(q[0] >> 24, 0xBF, "a quarter of the way: 255 * 12/16 rounded down");
        assert!(mix(&a, &b, 200) == b, "levels past the top are clamped");
    }

    #[test]
    fn names_match_the_menu_list() {
        assert_eq!(MS.map(name), ["Off", "Fast", "Medium", "Slow"]);
    }
}
