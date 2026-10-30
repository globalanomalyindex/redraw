//! Auto-contrast: pick the crosshair color that stands out from what is
//! behind it. The sampled pixels are only averaged into one color and then
//! thrown away; nothing here stores, exposes or acts on screen content beyond
//! choosing a crosshair color.

use crate::crosshair::{Blend, Crosshair, PALETTE};
use crate::experimental::AutoMode;

const LIGHT: u32 = 0xFFFFFF;
const DARK: u32 = 0x101216;
/// A new color has to beat the current one by this factor before it replaces it,
/// so the crosshair does not flicker on a borderline backdrop.
const HYSTERESIS: f32 = 1.25;

fn linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// WCAG relative luminance of an sRGB color.
pub fn luminance(rgb: u32) -> f32 {
    0.2126 * linear((rgb >> 16) as u8) + 0.7152 * linear((rgb >> 8) as u8) + 0.0722 * linear(rgb as u8)
}

pub fn contrast_ratio(a: u32, b: u32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Average of `pixels` (0xRRGGBB), skipping those flagged in `ignore` (the pixels the
/// overlay painted itself). `None` if nothing is left to average.
pub fn average(pixels: &[u32], ignore: &[bool]) -> Option<u32> {
    let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
    for (i, p) in pixels.iter().enumerate() {
        if ignore.get(i).copied().unwrap_or(false) {
            continue;
        }
        r += ((p >> 16) & 0xFF) as u64;
        g += ((p >> 8) & 0xFF) as u64;
        b += (p & 0xFF) as u64;
        n += 1;
    }
    if n == 0 {
        return None;
    }
    Some((((r / n) as u32) << 16) | (((g / n) as u32) << 8) | (b / n) as u32)
}

#[derive(Debug, Default)]
pub struct Picker {
    current: Option<u32>,
}

impl Picker {
    pub fn new() -> Picker {
        Picker { current: None }
    }

    pub fn forget(&mut self) {
        self.current = None;
    }

    /// The color to use over `backdrop`. Returns the same color until a clearly better one exists.
    pub fn pick(&mut self, mode: AutoMode, backdrop: u32) -> u32 {
        let candidates: Vec<u32> = match mode {
            AutoMode::LightDark => vec![LIGHT, DARK],
            AutoMode::Palette => PALETTE.iter().map(|(_, c)| *c).collect(),
        };
        let best = candidates.iter().copied().max_by(|a, b| contrast_ratio(*a, backdrop).total_cmp(&contrast_ratio(*b, backdrop))).unwrap_or(LIGHT);
        let keep = self.current.filter(|c| candidates.contains(c)).filter(|c| contrast_ratio(*c, backdrop) * HYSTERESIS >= contrast_ratio(best, backdrop));
        let chosen = keep.unwrap_or(best);
        self.current = Some(chosen);
        chosen
    }
}

/// The crosshair with `color` on every layer drawn with ordinary alpha. Glow and
/// lens layers keep their own look (their color is the light they add, or unused).
pub fn recolor(base: &Crosshair, color: u32) -> Crosshair {
    let mut c = base.clone();
    for l in &mut c.layers {
        if l.blend == Blend::Normal {
            l.color = color;
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn luminance_and_contrast_match_wcag() {
        assert!((luminance(0xFFFFFF) - 1.0).abs() < 1e-4);
        assert!(luminance(0x000000).abs() < 1e-6);
        assert!((contrast_ratio(0xFFFFFF, 0x000000) - 21.0).abs() < 0.01);
        assert!((contrast_ratio(0x777777, 0x777777) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn light_dark_picks_the_opposite_of_the_backdrop() {
        let mut p = Picker::new();
        assert_eq!(p.pick(AutoMode::LightDark, 0x0A0A12), LIGHT);
        let mut p = Picker::new();
        assert_eq!(p.pick(AutoMode::LightDark, 0xE8E8F0), DARK);
    }

    #[test]
    fn palette_never_picks_something_close_to_the_backdrop() {
        let mut p = Picker::new();
        let on_green = p.pick(AutoMode::Palette, 0x00FF66);
        assert_ne!(on_green, 0x00FF66);
        assert!(contrast_ratio(on_green, 0x00FF66) > 1.5);
        let best = PALETTE.iter().map(|(_, c)| contrast_ratio(*c, 0x00FF66)).fold(0.0f32, f32::max);
        assert!((contrast_ratio(on_green, 0x00FF66) - best).abs() < 1e-4, "it is the best of the palette");
    }

    #[test]
    fn hysteresis_prevents_flicker_on_a_borderline_backdrop() {
        let mut p = Picker::new();
        let first = p.pick(AutoMode::LightDark, 0x0A0A12);
        assert_eq!(first, LIGHT);
        // Mid-gray: dark is slightly better, but not by 25%, so white stays.
        let mid = 0x7E7E7E;
        assert!(contrast_ratio(DARK, mid) > contrast_ratio(LIGHT, mid), "premise: dark is marginally better here");
        assert!(contrast_ratio(DARK, mid) < contrast_ratio(LIGHT, mid) * HYSTERESIS, "premise: but not decisively");
        assert_eq!(p.pick(AutoMode::LightDark, mid), LIGHT);
        assert_eq!(p.pick(AutoMode::LightDark, 0xF0F0F0), DARK, "a clearly bright backdrop switches");
        p.forget();
        assert_eq!(p.pick(AutoMode::LightDark, mid), DARK, "no history: just the best");
    }

    #[test]
    fn switching_modes_drops_a_color_that_is_not_a_candidate() {
        let mut p = Picker::new();
        p.pick(AutoMode::LightDark, 0x000000);
        let c = p.pick(AutoMode::Palette, 0x000000);
        assert!(PALETTE.iter().any(|(_, x)| *x == c));
    }

    #[test]
    fn average_ignores_our_own_pixels() {
        let px = [0x000000, 0x000000, 0xFFFFFF, 0xFFFFFF];
        assert_eq!(average(&px, &[false, false, false, false]), Some(0x7F7F7F));
        assert_eq!(average(&px, &[false, false, true, true]), Some(0x000000));
        assert_eq!(average(&px, &[true; 4]), None);
        assert_eq!(average(&[], &[]), None);
        assert_eq!(average(&px, &[]), Some(0x7F7F7F), "short mask means nothing is ignored");
    }

    #[test]
    fn recolor_touches_only_ordinary_layers() {
        let mut c = Crosshair::classic();
        c.layers[1].blend = Blend::Glow;
        let r = recolor(&c, 0x123456);
        assert_eq!(r.layers[0].color, 0x123456);
        assert_eq!(r.layers[1].color, c.layers[1].color, "glow layer keeps its light color");
        assert_eq!(r.outline, c.outline);
    }
}
