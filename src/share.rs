//! Share codes: a whole crosshair as one line of text, and back.
//!
//! `oc2 o1 p10 <layer> <layer> ...` is one crosshair (outline `o`, opacity `p`, then the layer
//! tokens with everything a layer has). `oc2 hip o1 p10 ... ads o1 p10 ...` is both profiles.
//! The older `oc1 <layer> ...` (layers only) still reads. Pure text in, pure data out; whatever
//! is pasted is treated as untrusted, so every number is clamped and the length is capped.

use crate::crosshair::{Crosshair, Layer, MAX_LAYERS};

/// Longer than any code the menu can make (8 layers of about 50 characters, twice, plus headers).
const MAX_CODE_LEN: usize = 2048;

/// What a pasted code held.
#[derive(Debug, PartialEq, Eq, Default)]
pub struct Shared {
    /// a code for one crosshair: it goes to whichever profile is being edited
    pub single: Option<Crosshair>,
    pub hip: Option<Crosshair>,
    pub ads: Option<Crosshair>,
}

fn section(c: &Crosshair) -> String {
    let layers: Vec<String> = c.layers.iter().take(MAX_LAYERS).map(Layer::to_persist_token).collect();
    format!("o{} p{} {}", c.outline.min(2), c.opacity.clamp(1, 10), layers.join(" "))
}

/// A code for one crosshair.
pub fn encode(c: &Crosshair) -> String {
    format!("oc2 {}", section(c))
}

/// A code for both profiles.
pub fn encode_both(hip: &Crosshair, ads: &Crosshair) -> String {
    format!("oc2 hip {} ads {}", section(hip), section(ads))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    Single,
    Hip,
    Ads,
}

fn fresh(fallback: &Crosshair) -> Crosshair {
    Crosshair { layers: Vec::new(), outline: fallback.outline, opacity: fallback.opacity }
}

/// Move the crosshair being read into the slot it was read for, if it got any layers.
fn flush(cur: &mut Option<(Slot, Crosshair)>, shared: &mut Shared) {
    if let Some((slot, c)) = cur.take() {
        if !c.layers.is_empty() {
            match slot {
                Slot::Single => shared.single = Some(c),
                Slot::Hip => shared.hip = Some(c),
                Slot::Ads => shared.ads = Some(c),
            }
        }
    }
}

/// Read a code. `fallback` supplies the outline and opacity for an old `oc1` code, which has none.
/// `None` if the text holds no crosshair at all.
pub fn decode(text: &str, fallback: &Crosshair) -> Option<Shared> {
    let text = text.trim().trim_start_matches('\u{feff}');
    if text.is_empty() || text.len() > MAX_CODE_LEN {
        return None;
    }
    let mut words = text.split_whitespace().peekable();
    match words.next()? {
        "oc1" | "oc2" => {}
        _ => return None,
    }
    let mut slot = Slot::Single;
    let mut shared = Shared::default();
    let mut current: Option<(Slot, Crosshair)> = None;
    for word in words {
        if word == "hip" || word == "ads" {
            flush(&mut current, &mut shared);
            slot = if word == "hip" { Slot::Hip } else { Slot::Ads };
            current = Some((slot, fresh(fallback)));
            continue;
        }
        let c = &mut current.get_or_insert_with(|| (slot, fresh(fallback))).1;
        // Header words: `o1` outline, `p10` opacity. A layer token always has dots, so they cannot collide.
        if let Some(n) = word.strip_prefix('o').filter(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit())) {
            c.outline = n.parse::<u32>().unwrap_or(1).min(2) as u8;
        } else if let Some(n) = word.strip_prefix('p').filter(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit())) {
            c.opacity = n.parse::<u32>().unwrap_or(10).clamp(1, 10) as u8;
        } else if let Some(layer) = Layer::from_token(word) {
            if c.layers.len() < MAX_LAYERS {
                c.layers.push(layer);
            }
        }
    }
    flush(&mut current, &mut shared);
    if shared.single.is_none() && shared.hip.is_none() && shared.ads.is_none() {
        None
    } else {
        Some(shared)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crosshair::{Blend, Kind, Mirror};

    fn rich() -> Crosshair {
        let mut a = Layer::new(Kind::Chevron, 0x123456);
        a.blend = Blend::Glow;
        a.opacity = 6;
        a.ox = 3;
        a.oy = -2;
        a.mirror = Mirror::Both;
        a.cycle = 3;
        let mut b = Layer::new(Kind::Polygon, 0xABCDEF);
        b.visible = false;
        Crosshair { layers: vec![a, b], outline: 2, opacity: 7 }
    }

    #[test]
    fn a_single_code_keeps_everything() {
        let c = rich();
        let code = encode(&c);
        assert!(code.starts_with("oc2 o2 p7 chv."), "{code}");
        let got = decode(&code, &Crosshair::classic()).expect("decodes");
        assert_eq!(got.single, Some(c));
        assert!(got.hip.is_none() && got.ads.is_none());
    }

    #[test]
    fn a_pair_code_names_the_profiles() {
        let (hip, ads) = (rich(), Crosshair::ads_default());
        let code = encode_both(&hip, &ads);
        assert!(code.starts_with("oc2 hip o2 p7 ") && code.contains(" ads o1 p10 "), "{code}");
        let got = decode(&code, &Crosshair::classic()).expect("decodes");
        assert_eq!((got.hip, got.ads, got.single), (Some(hip), Some(ads), None));
    }

    #[test]
    fn the_default_crosshairs_round_trip() {
        for c in [Crosshair::classic(), Crosshair::ads_default()] {
            assert_eq!(decode(&encode(&c), &c).and_then(|s| s.single), Some(c));
        }
        for (_, layers) in crate::crosshair::presets() {
            let c = Crosshair { layers, outline: 1, opacity: 10 };
            assert_eq!(decode(&encode(&c), &c).and_then(|s| s.single), Some(c));
        }
    }

    #[test]
    fn an_old_oc1_code_reads_with_the_fallback_outline_and_opacity() {
        let fallback = Crosshair { layers: vec![], outline: 2, opacity: 4 };
        let got = decode("oc1 spk.00ff66.6.4.3.4.0 dot.00ff66.1.4.1.4.0", &fallback).expect("decodes").single.expect("single");
        assert_eq!((got.outline, got.opacity, got.layers.len()), (2, 4, 2));
        assert_eq!(got.layers, Crosshair::classic().layers);
    }

    #[test]
    fn junk_and_hostile_input_never_panics_and_never_half_applies() {
        let f = Crosshair::classic();
        for bad in ["", "   ", "hello", "oc2", "oc2 o1 p10", "oc9 spk.00ff66.6.4.3.4.0", "oc2 hip", "oc2 o1 p10 nonsense", "\u{0}\u{1}oc2", "oc2 hip ads"] {
            assert_eq!(decode(bad, &f), None, "{bad:?}");
        }
        let huge = format!("oc2 {}", "spk.00ff66.6.4.3.4.0 ".repeat(500));
        assert_eq!(decode(&huge, &f), None, "over the length cap");
        let many = format!("oc2 {}", "dot.00ff66.1.4.1.4.0 ".repeat(30));
        assert_eq!(decode(&many, &f).and_then(|s| s.single).map(|c| c.layers.len()), Some(MAX_LAYERS), "extra layers are dropped, not trusted");
        let wild = decode("oc2 o999 p999 spk.00ff66.99.99.99.99.99999.o99.x999.y-999", &f).and_then(|s| s.single).expect("clamped, not rejected");
        assert_eq!((wild.outline, wild.opacity), (2, 10));
        let l = &wild.layers[0];
        assert!(l.reach + l.gap <= 16 && l.thick <= 7 && (-12..=12).contains(&l.ox) && (-12..=12).contains(&l.oy));
        let bom = decode("\u{feff}oc2 o1 p10 dot.00ff66.1.4.1.4.0", &f);
        assert!(bom.is_some(), "a byte-order mark from a text file is ignored");
    }

    #[test]
    fn a_code_with_one_bad_token_keeps_the_good_ones() {
        let got = decode("oc2 o1 p5 spk.zzz.1.1.1.1.0 dot.00ff66.1.4.1.4.0", &Crosshair::classic()).and_then(|s| s.single).expect("decodes");
        assert_eq!((got.layers.len(), got.opacity), (1, 5));
    }

    #[test]
    fn codes_fit_in_the_cap_even_at_full_size() {
        let full = Crosshair { layers: (0..MAX_LAYERS).map(|_| rich().layers.remove(0)).collect(), outline: 2, opacity: 7 };
        let code = encode_both(&full, &full);
        assert!(code.len() < MAX_CODE_LEN, "{} bytes", code.len());
        assert_eq!(decode(&code, &full).map(|s| (s.hip, s.ads)), Some((Some(full.clone()), Some(full))));
    }
}
