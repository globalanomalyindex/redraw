//! The shape of a magnified area: how big its window is, which part of the screen it reads, and how
//! much it scales that part across and down. Pure, so the maths behind Scope zoom and Aspect ratio is
//! tested without a screen. (`examples/aspectspike.rs` proved on the real compositor that the
//! magnifier accepts different horizontal and vertical scales, which is what Stretch needs.)

use crate::experimental::Fit;

/// A magnifier window's size, the screen region it reads, and the scale between the two.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    pub w: i32,
    pub h: i32,
    pub src_w: i32,
    pub src_h: i32,
    pub sx: f32,
    pub sy: f32,
}

const MIN: i32 = 8;

fn px(v: f32) -> i32 {
    (v.round() as i32).max(MIN)
}

/// Scope zoom: a square window `size` wide that reads a square `size / factor` wide, scaled evenly.
pub fn zoom(size: i32, factor: f32) -> Geometry {
    let src = px(size as f32 / factor);
    Geometry { w: size, h: size, src_w: src, src_h: src, sx: factor, sy: factor }
}

/// Aspect ratio: a window shaped `ratio` (width, height) whose long side is `size`.
/// - `Crop` reads a patch of the same shape and scales it evenly, so nothing is distorted.
/// - `Stretch` reads the same square the zoom would, and fits it to the window: wider than tall
///   squeezes it down, taller than wide squeezes it across (the "stretched resolution" look).
pub fn aspect(size: i32, ratio: (i32, i32), fit: Fit, factor: f32) -> Geometry {
    let (rw, rh) = (ratio.0.max(1), ratio.1.max(1));
    let (w, h) = if rw >= rh { (size, (size * rh + rw / 2) / rw) } else { ((size * rw + rh / 2) / rh, size) };
    let (w, h) = (w.max(MIN), h.max(MIN));
    match fit {
        Fit::Crop => {
            let (src_w, src_h) = (px(w as f32 / factor), px(h as f32 / factor));
            Geometry { w, h, src_w, src_h, sx: w as f32 / src_w as f32, sy: h as f32 / src_h as f32 }
        }
        Fit::Stretch => {
            let q = px(size as f32 / factor);
            Geometry { w, h, src_w: q, src_h: q, sx: w as f32 / q as f32, sy: h as f32 / q as f32 }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::experimental::RATIOS;

    #[test]
    fn scope_zoom_is_a_square_read_at_one_over_the_factor() {
        assert_eq!(zoom(220, 2.0), Geometry { w: 220, h: 220, src_w: 110, src_h: 110, sx: 2.0, sy: 2.0 });
        assert_eq!(zoom(100, 4.0).src_w, 25);
        assert_eq!(zoom(100, 40.0).src_w, MIN, "never a source too small to read");
    }

    #[test]
    fn a_wide_ratio_crops_to_a_wide_patch_and_stretches_a_square_one() {
        let crop = aspect(260, (16, 9), Fit::Crop, 2.0);
        assert_eq!((crop.w, crop.h, crop.src_w, crop.src_h), (260, 146, 130, 73));
        assert_eq!((crop.sx, crop.sy), (2.0, 2.0), "evenly scaled: nothing distorted");
        let stretch = aspect(260, (16, 9), Fit::Stretch, 2.0);
        assert_eq!((stretch.w, stretch.h, stretch.src_w, stretch.src_h), (260, 146, 130, 130), "the same square the zoom reads");
        assert_eq!(stretch.sx, 2.0);
        assert!((stretch.sy - 146.0 / 130.0).abs() < 1e-6, "squeezed vertically: {}", stretch.sy);
    }

    #[test]
    fn a_tall_ratio_is_as_tall_as_it_is_long() {
        let g = aspect(260, (9, 16), Fit::Stretch, 2.0);
        assert_eq!((g.w, g.h), (146, 260));
        assert!(g.sx < g.sy, "squeezed across: {} vs {}", g.sx, g.sy);
        let s = aspect(240, (1, 1), Fit::Stretch, 2.0);
        assert_eq!((s.w, s.h, s.sx, s.sy), (240, 240, 2.0, 2.0), "1:1 is plain zoom");
    }

    #[test]
    fn every_ratio_size_zoom_and_fit_makes_a_sound_window() {
        for (rw, rh) in RATIOS {
            for size in (100..=400).step_by(20) {
                for tenths in (10..=40).step_by(5) {
                    let factor = tenths as f32 / 10.0;
                    for fit in [Fit::Stretch, Fit::Crop] {
                        let g = aspect(size, (rw, rh), fit, factor);
                        let what = format!("{rw}:{rh} size {size} x{factor} {fit:?} -> {g:?}");
                        assert_eq!(g.w.max(g.h), size, "long side is the size: {what}");
                        assert!(g.w >= MIN && g.h >= MIN && g.src_w >= MIN && g.src_h >= MIN, "{what}");
                        let (want, got) = (rw as f32 / rh as f32, g.w as f32 / g.h as f32);
                        assert!(((got - want) / want).abs() < 0.02, "shape is the ratio within a pixel: {what}");
                        assert!((g.sx * g.src_w as f32 - g.w as f32).abs() < 0.01 && (g.sy * g.src_h as f32 - g.h as f32).abs() < 0.01, "the read region fills the window exactly: {what}");
                        match fit {
                            Fit::Stretch => assert!(((g.sx / g.sy) - got).abs() / got < 0.001, "stretch distorts by exactly the shape: {what}"),
                            Fit::Crop => assert!(((g.sx / g.sy) - 1.0).abs() < 0.12, "crop is (nearly) even: {what}"),
                        }
                    }
                }
            }
        }
    }
}
