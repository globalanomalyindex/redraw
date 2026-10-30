//! Input Dynamics: the crosshair reacting to *your* input (fire button held,
//! movement keys held, how long since you touched anything). It never looks
//! at the game, so it knows nothing about real weapon spread; it is cosmetic.
//!
//! Everything is quantized (whole pixels, 15 degree steps, 16 opacity levels)
//! so the overlay only needs to redraw when the quantized frame changes.

use crate::crosshair::{Crosshair, Kind};
use crate::experimental::{Dynamics, Motion};

const BLOOM_PX: [f32; 4] = [0.0, 2.0, 4.0, 7.0];
const SPREAD_PX: [f32; 4] = [0.0, 1.0, 3.0, 5.0];
const IDLE_SECS: [f32; 4] = [f32::INFINITY, 3.0, 6.0, 12.0];
const IDLE_OPACITY: f32 = 0.25;
const ATTACK: f32 = 30.0;
const RELEASE: f32 = 9.0;
const FADE_RATE: f32 = 3.0;
const PULSE_HZ: f32 = 1.0;
const SPIN_DEG_PER_SEC: f32 = 45.0;

#[derive(Clone, Copy, Debug, Default)]
pub struct Inputs {
    pub firing: bool,
    pub moving: bool,
    pub idle_secs: f32,
}

/// What to draw: pixels added to every radius, degrees added to rotations, opacity multiplier.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub spread: i32,
    pub rot: i32,
    pub opacity: f32,
}

pub const NEUTRAL: Frame = Frame { spread: 0, rot: 0, opacity: 1.0 };

#[derive(Debug)]
pub struct Dyn {
    spread: f32,
    fade: f32,
    phase: f32,
}

fn approach(value: f32, target: f32, rate: f32, dt: f32) -> f32 {
    let v = value + (target - value) * (1.0 - (-rate * dt).exp());
    if (v - target).abs() < 0.01 {
        target
    } else {
        v
    }
}

impl Dyn {
    pub fn new() -> Dyn {
        Dyn { spread: 0.0, fade: 1.0, phase: 0.0 }
    }

    fn targets(cfg: &Dynamics, inp: &Inputs) -> (f32, f32) {
        let spread = if inp.firing { BLOOM_PX[cfg.bloom.min(3) as usize] } else { 0.0 } + if inp.moving { SPREAD_PX[cfg.spread.min(3) as usize] } else { 0.0 };
        let fade = if inp.idle_secs >= IDLE_SECS[cfg.idle.min(3) as usize] { IDLE_OPACITY } else { 1.0 };
        (spread, fade)
    }

    pub fn step(&mut self, cfg: &Dynamics, inp: &Inputs, dt: f32) -> Frame {
        if !cfg.enabled {
            *self = Dyn::new();
            return NEUTRAL;
        }
        let dt = dt.clamp(0.0, 0.25);
        let (spread_t, fade_t) = Dyn::targets(cfg, inp);
        let rate = if spread_t > self.spread { ATTACK } else { RELEASE };
        self.spread = approach(self.spread, spread_t, rate, dt);
        self.fade = approach(self.fade, fade_t, FADE_RATE, dt);
        if cfg.motion != Motion::None {
            self.phase = (self.phase + dt) % 3600.0;
        }
        let mut opacity = self.fade;
        let mut rot = 0;
        match cfg.motion {
            Motion::None => {}
            Motion::Pulse => opacity *= 0.7 + 0.3 * (0.5 + 0.5 * (self.phase * PULSE_HZ * std::f32::consts::TAU).sin()),
            Motion::Spin => rot = ((self.phase * SPIN_DEG_PER_SEC / 15.0).floor() as i32 * 15).rem_euclid(360),
        }
        Frame { spread: self.spread.round() as i32, rot, opacity: (opacity * 16.0).round() / 16.0 }
    }

    /// True while the next `step` can still change what is drawn, so the frame
    /// clock has to keep running. A resting crosshair needs no clock at all.
    pub fn animating(&self, cfg: &Dynamics, inp: &Inputs) -> bool {
        if !cfg.enabled {
            return false;
        }
        let (spread_t, fade_t) = Dyn::targets(cfg, inp);
        cfg.motion != Motion::None || (self.spread - spread_t).abs() > 0.005 || (self.fade - fade_t).abs() > 0.005
    }
}

/// The crosshair as drawn for this frame: radii widened, rotations advanced.
pub fn apply(base: &Crosshair, f: &Frame) -> Crosshair {
    let mut c = base.clone();
    for l in &mut c.layers {
        match l.kind {
            Kind::Spokes => {
                l.gap += f.spread;
                l.rot += f.rot;
            }
            Kind::Ring | Kind::Polygon | Kind::Arc => {
                l.reach += f.spread;
                l.rot += f.rot;
            }
            Kind::Diamond | Kind::Bracket | Kind::Ladder => l.reach += f.spread,
            Kind::Chevron => l.gap += f.spread,
            Kind::Dot | Kind::Line | Kind::Disc => {}
        }
        l.clamp();
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on() -> Dynamics {
        Dynamics { enabled: true, bloom: 2, spread: 1, idle: 0, motion: Motion::None, keys: *b"WASD" }
    }

    fn run(d: &mut Dyn, cfg: &Dynamics, inp: Inputs, secs: f32) -> Frame {
        let mut f = NEUTRAL;
        for _ in 0..(secs * 120.0) as usize {
            f = d.step(cfg, &inp, 1.0 / 120.0);
        }
        f
    }

    #[test]
    fn disabled_is_always_neutral_and_forgets() {
        let mut d = Dyn::new();
        let mut cfg = on();
        run(&mut d, &cfg, Inputs { firing: true, ..Inputs::default() }, 0.5);
        cfg.enabled = false;
        assert_eq!(d.step(&cfg, &Inputs { firing: true, ..Inputs::default() }, 0.1), NEUTRAL);
        assert!(!d.animating(&cfg, &Inputs::default()));
        cfg.enabled = true;
        assert_eq!(d.step(&cfg, &Inputs::default(), 0.01).spread, 0, "starts from rest again");
    }

    #[test]
    fn firing_blooms_quickly_and_releases_back() {
        let mut d = Dyn::new();
        let cfg = on();
        let fire = Inputs { firing: true, ..Inputs::default() };
        assert_eq!(run(&mut d, &cfg, fire, 0.4).spread, 4, "medium bloom is 4px");
        let after = run(&mut d, &cfg, Inputs::default(), 1.5);
        assert_eq!(after.spread, 0, "back to rest");
        assert!(!d.animating(&cfg, &Inputs::default()), "clock can stop once settled");
    }

    #[test]
    fn bloom_and_move_spread_add_up_and_levels_scale() {
        let both = Inputs { firing: true, moving: true, idle_secs: 0.0 };
        for (bloom, spread, want) in [(1u8, 1u8, 3), (3, 3, 12), (0, 0, 0), (2, 3, 9)] {
            let mut cfg = on();
            cfg.bloom = bloom;
            cfg.spread = spread;
            assert_eq!(run(&mut Dyn::new(), &cfg, both, 1.0).spread, want, "bloom {bloom} spread {spread}");
        }
    }

    #[test]
    fn idle_fades_out_and_input_brings_it_back() {
        let mut cfg = on();
        cfg.idle = 1;
        let mut d = Dyn::new();
        assert_eq!(run(&mut d, &cfg, Inputs { idle_secs: 1.0, ..Inputs::default() }, 0.1).opacity, 1.0, "below the 3s threshold nothing fades");
        let faded = run(&mut d, &cfg, Inputs { idle_secs: 5.0, ..Inputs::default() }, 3.0);
        assert!((faded.opacity - IDLE_OPACITY).abs() <= 1.0 / 16.0, "{}", faded.opacity);
        assert_eq!(run(&mut d, &cfg, Inputs { idle_secs: 0.0, ..Inputs::default() }, 3.0).opacity, 1.0);
        cfg.idle = 0;
        assert_eq!(run(&mut Dyn::new(), &cfg, Inputs { idle_secs: 999.0, ..Inputs::default() }, 3.0).opacity, 1.0, "idle fade off");
    }

    #[test]
    fn pulse_and_spin_keep_the_clock_running_and_quantize() {
        let mut cfg = on();
        cfg.motion = Motion::Pulse;
        let mut d = Dyn::new();
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..240 {
            let f = d.step(&cfg, &Inputs::default(), 1.0 / 120.0);
            assert!(f.opacity >= 0.68 && f.opacity <= 1.0, "{}", f.opacity);
            assert_eq!((f.opacity * 16.0).fract(), 0.0, "16 opacity levels");
            seen.insert((f.opacity * 16.0) as i32);
        }
        assert!(seen.len() > 2, "it actually pulses");
        assert!(d.animating(&cfg, &Inputs::default()));
        cfg.motion = Motion::Spin;
        let mut d = Dyn::new();
        let f = run(&mut d, &cfg, Inputs::default(), 2.05);
        assert_eq!(f.rot % 15, 0, "15 degree steps");
        assert_eq!(f.rot, 90, "45 degrees per second for just over 2s");
    }

    #[test]
    fn apply_widens_every_radius_and_clamps_inside_the_grid() {
        let base = Crosshair::classic();
        let wide = apply(&base, &Frame { spread: 5, rot: 0, opacity: 1.0 });
        assert_eq!(wide.layers[0].gap, base.layers[0].gap + 5);
        assert_eq!(wide.layers[1], base.layers[1], "dot does not move");
        let huge = apply(&base, &Frame { spread: 99, rot: 0, opacity: 1.0 });
        let l = &huge.layers[0];
        assert!(l.gap + l.reach <= 16, "stays inside the 33x33 grid");
        assert_eq!(apply(&base, &NEUTRAL), base, "neutral frame changes nothing");
        let spun = apply(&base, &Frame { spread: 0, rot: 45, opacity: 1.0 });
        assert_eq!(spun.layers[0].rot, 45);
    }
}
