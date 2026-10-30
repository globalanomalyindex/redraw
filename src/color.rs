//! Color math for the custom-color sliders and the hue cycle. Pure.
//! Hue is in degrees (0..360), saturation and value are fractions (0..=1).

pub fn rgb_to_hsv(rgb: u32) -> (f32, f32, f32) {
    let (r, g, b) = (((rgb >> 16) & 0xFF) as f32 / 255.0, ((rgb >> 8) & 0xFF) as f32 / 255.0, (rgb & 0xFF) as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d < 1e-6 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (h.rem_euclid(360.0), if max < 1e-6 { 0.0 } else { d / max }, max)
}

pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> u32 {
    let (s, v) = (s.clamp(0.0, 1.0), v.clamp(0.0, 1.0));
    let h = h.rem_euclid(360.0) / 60.0;
    let c = v * s;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let ch = |f: f32| (((f + m) * 255.0).round() as u32).min(255);
    (ch(r) << 16) | (ch(g) << 8) | ch(b)
}

/// The same color with its hue turned by `degrees`. Grays have no hue and stay as they are.
pub fn rotate_hue(rgb: u32, degrees: i32) -> u32 {
    let (h, s, v) = rgb_to_hsv(rgb);
    if s < 1e-6 {
        return rgb;
    }
    hsv_to_rgb(h + degrees as f32, s, v)
}

/// Hue 0..=355 in steps of 5, saturation and value 0..=100 in steps of 5: what the menu sliders show.
#[cfg(test)]
pub fn to_slider_hsv(rgb: u32) -> (i32, i32, i32) {
    let (h, s, v) = rgb_to_hsv(rgb);
    slider_steps(h, s, v)
}

/// Where the sliders sit for an exact hue (degrees), saturation and value (0..=1): the nearest step of 5.
pub fn slider_steps(h: f32, s: f32, v: f32) -> (i32, i32, i32) {
    let snap = |x: f32, top: i32| (((x / 5.0).round() as i32) * 5).clamp(0, top);
    (snap(h, 355), snap(s * 100.0, 100), snap(v * 100.0, 100))
}

#[cfg(test)]
pub fn from_slider_hsv(h: i32, s: i32, v: i32) -> u32 {
    hsv_to_rgb(h as f32, s as f32 / 100.0, v as f32 / 100.0)
}

/// Names of the hue-cycle levels a layer can have.
pub const CYCLE_NAMES: [&str; 4] = ["Off", "Slow", "Medium", "Fast"];
/// Degrees of hue per second at each level.
const CYCLE_DPS: [u64; 4] = [0, 24, 72, 216];

/// How far each cycle level has turned the hue after `ms` milliseconds, in 5 degree steps so the
/// overlay only redraws when the picture actually changes. Index 0 (fixed color) is always 0.
pub fn cycle_hues(ms: u64) -> [i32; 4] {
    let mut out = [0; 4];
    for (level, dps) in CYCLE_DPS.iter().enumerate().skip(1) {
        out[level] = ((ms * dps / 1000 / 5 * 5) % 360) as i32;
    }
    out
}

/// `#rrggbb`, `rrggbb`, `#rgb` or `rgb`, with stray whitespace or quotes around it.
pub fn parse_hex(text: &str) -> Option<u32> {
    let t = text.trim().trim_matches(|c| c == '"' || c == '\'').trim();
    let t = t.strip_prefix('#').unwrap_or(t);
    if !t.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    match t.len() {
        6 => u32::from_str_radix(t, 16).ok(),
        3 => {
            let v = u32::from_str_radix(t, 16).ok()?;
            let (r, g, b) = ((v >> 8) & 0xF, (v >> 4) & 0xF, v & 0xF);
            Some((r * 17) << 16 | (g * 17) << 8 | b * 17)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crosshair::PALETTE;

    #[test]
    fn hue_cycle_runs_at_three_speeds_in_five_degree_steps() {
        assert_eq!(cycle_hues(0), [0, 0, 0, 0]);
        let a = cycle_hues(1000);
        assert_eq!((a[0], a[1], a[2], a[3]), (0, 20, 70, 215), "24, 72 and 216 degrees a second, floored to 5");
        assert_eq!(cycle_hues(15_000)[1], 0, "24 degrees a second makes a full turn in 15s");
        assert_eq!(cycle_hues(5_000)[3], 0, "216 degrees a second: 1080 is three turns");
        let mut last = cycle_hues(0);
        let mut changes = 0;
        for ms in 0..2000 {
            let now = cycle_hues(ms);
            assert!(now.iter().all(|h| h % 5 == 0 && (0..360).contains(h)));
            if now != last {
                changes += 1;
                last = now;
            }
        }
        assert!(changes < 200, "{changes} distinct pictures in two seconds, far fewer than frames");
    }

    #[test]
    fn hex_is_read_loosely_and_rejected_strictly() {
        for (text, want) in [("#ff9500", 0xFF9500), ("ff9500", 0xFF9500), ("  #FF9500\r\n", 0xFF9500), ("\"#00e5ff\"", 0x00E5FF), ("#f80", 0xFF8800), ("abc", 0xAABBCC)] {
            assert_eq!(parse_hex(text), Some(want), "{text:?}");
        }
        for bad in ["", "#", "#12", "#12345", "#1234567", "#gg0000", "red", "0xff9500", "#ff95 00", "#+f9500"] {
            assert_eq!(parse_hex(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn primaries_and_grays_convert_exactly() {
        assert_eq!(hsv_to_rgb(0.0, 1.0, 1.0), 0xFF0000);
        assert_eq!(hsv_to_rgb(120.0, 1.0, 1.0), 0x00FF00);
        assert_eq!(hsv_to_rgb(240.0, 1.0, 1.0), 0x0000FF);
        assert_eq!(hsv_to_rgb(60.0, 1.0, 1.0), 0xFFFF00);
        assert_eq!(hsv_to_rgb(0.0, 0.0, 1.0), 0xFFFFFF);
        assert_eq!(hsv_to_rgb(200.0, 0.7, 0.0), 0x000000);
        assert_eq!(rgb_to_hsv(0xFF0000), (0.0, 1.0, 1.0));
        assert_eq!(rgb_to_hsv(0xFFFFFF), (0.0, 0.0, 1.0));
        assert_eq!(rgb_to_hsv(0x000000), (0.0, 0.0, 0.0));
        let (h, s, v) = rgb_to_hsv(0x00FF66);
        assert!((h - 144.0).abs() < 0.5 && s == 1.0 && v == 1.0, "{h} {s} {v}");
    }

    #[test]
    fn every_byte_triple_round_trips_through_hsv_within_one_step() {
        for r in (0..=255).step_by(15) {
            for g in (0..=255).step_by(15) {
                for b in (0..=255).step_by(15) {
                    let rgb = (r << 16) | (g << 8) | b;
                    let (h, s, v) = rgb_to_hsv(rgb);
                    let back = hsv_to_rgb(h, s, v);
                    for shift in [16, 8, 0] {
                        let (x, y) = ((rgb >> shift) & 0xFF, (back >> shift) & 0xFF);
                        assert!((x as i32 - y as i32).abs() <= 1, "{rgb:06x} -> {back:06x}");
                    }
                }
            }
        }
    }

    #[test]
    fn rotating_the_hue_wraps_and_leaves_grays_alone() {
        assert_eq!(rotate_hue(0xFF0000, 120), 0x00FF00);
        assert_eq!(rotate_hue(0xFF0000, -120), 0x0000FF);
        assert_eq!(rotate_hue(0xFF0000, 360), 0xFF0000);
        assert_eq!(rotate_hue(0xFF0000, 0), 0xFF0000);
        for gray in [0x000000, 0x808080, 0xFFFFFF] {
            assert_eq!(rotate_hue(gray, 77), gray);
        }
        let c = 0x00E5FF;
        let back = rotate_hue(rotate_hue(c, 90), -90);
        for shift in [16, 8, 0] {
            let (x, y) = ((c >> shift) & 0xFF, (back >> shift) & 0xFF);
            assert!((x as i32 - y as i32).abs() <= 1, "turning there and back loses at most one byte step: {back:06x}");
        }
    }

    #[test]
    fn slider_values_are_on_the_five_steps_and_stay_in_range() {
        for (_, rgb) in PALETTE {
            let (h, s, v) = to_slider_hsv(rgb);
            assert!(h % 5 == 0 && s % 5 == 0 && v % 5 == 0, "{rgb:06x}");
            assert!((0..=355).contains(&h) && (0..=100).contains(&s) && (0..=100).contains(&v));
            let back = from_slider_hsv(h, s, v);
            for shift in [16, 8, 0] {
                let (x, y) = ((rgb >> shift) & 0xFF, (back >> shift) & 0xFF);
                assert!((x as i32 - y as i32).abs() <= 18, "palette color {rgb:06x} is within a slider step of itself: {back:06x}");
            }
        }
        assert_eq!(to_slider_hsv(0xFFFFFF), (0, 0, 100));
        assert_eq!(from_slider_hsv(120, 100, 100), 0x00FF00);
        assert_eq!(from_slider_hsv(355, 100, 100) >> 16, 255);
    }
}
