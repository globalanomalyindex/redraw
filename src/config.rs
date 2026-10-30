//! Plain-text config: `key=value` lines, `#` comments, unknown keys ignored.
//! Anything unreadable falls back to the default for that field, so a bad
//! edit can never stop the crosshair from drawing.

use crate::crosshair::Crosshair;
use crate::display::Display;
use crate::experimental::Experimental;
use crate::states::States;

pub const VK_F1: u32 = 0x70;
pub const VK_F8: u32 = 0x77;
pub const VK_F9: u32 = 0x78;
pub const VK_F10: u32 = 0x79;
pub const VK_F11: u32 = 0x7A;

/// Keys the menu offers for the ADS trigger: F1 to F8 and F12. F9, F10 and F11
/// belong to the overlay (show/hide, menu, reload).
pub const TRIGGER_KEYS: [u32; 9] = [0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x77, 0x7B];

/// A key the ADS trigger may use: not a mouse button, and not one of the
/// overlay's own hotkeys, which would collide.
fn usable_trigger_key(vk: u32) -> bool {
    !(1..=6).contains(&vk) && vk != VK_F9 && vk != VK_F10 && vk != VK_F11
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    Hip = 0,
    Ads = 1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    HoldRmb,
    ToggleRmb,
    Key,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub trigger: Trigger,
    /// virtual-key code used when `trigger` is `Key`
    pub key: u32,
    /// pixel offset of the crosshair from the screen center
    pub offset: (i32, i32),
    /// freeze the menu title instead of animating it
    pub reduce_motion: bool,
    /// menu section header color, 0xRRGGBB
    pub accent: u32,
    pub hip: Crosshair,
    pub ads: Crosshair,
    /// crossfade time between the hip fire and ADS crosshairs, ms (0 = cut)
    pub morph: u32,
    /// look for a newer version on GitHub at launch and every 30 minutes
    pub autoupdate: bool,
    pub states: States,
    pub exp: Experimental,
    pub display: Display,
}

pub const DEFAULT_ACCENT: u32 = 0x4AA8FF;
pub const DEFAULT_MORPH_MS: u32 = 150;

impl Default for Config {
    fn default() -> Config {
        Config {
            trigger: Trigger::HoldRmb,
            key: VK_F8,
            offset: (0, 0),
            reduce_motion: false,
            accent: DEFAULT_ACCENT,
            hip: Crosshair::classic(),
            ads: Crosshair::ads_default(),
            morph: DEFAULT_MORPH_MS,
            autoupdate: true,
            states: States::default(),
            exp: Experimental::default(),
            display: Display::default(),
        }
    }
}

impl Config {
    pub fn profile(&self, p: Profile) -> &Crosshair {
        match p {
            Profile::Hip => &self.hip,
            Profile::Ads => &self.ads,
        }
    }

    pub fn profile_mut(&mut self, p: Profile) -> &mut Crosshair {
        match p {
            Profile::Hip => &mut self.hip,
            Profile::Ads => &mut self.ads,
        }
    }
}

pub fn key_name(vk: u32) -> String {
    if (VK_F1..VK_F1 + 24).contains(&vk) {
        format!("F{}", vk - VK_F1 + 1)
    } else {
        format!("0x{vk:02X}")
    }
}

fn parse_key(text: &str) -> Option<u32> {
    let t = text.trim();
    if let Some(n) = t.strip_prefix(['F', 'f']).and_then(|n| n.parse::<u32>().ok()) {
        return (1..=24).contains(&n).then(|| VK_F1 + n - 1);
    }
    let hex = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X"))?;
    u32::from_str_radix(hex, 16).ok().filter(|vk| (1..=0xFE).contains(vk))
}

impl Config {
    pub fn parse(text: &str) -> Config {
        let text = text.trim_start_matches('\u{feff}');
        let mut cfg = Config::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else { continue };
            let (key, value) = (key.trim(), value.trim());
            match key {
                "trigger" => match value {
                    "hold" => cfg.trigger = Trigger::HoldRmb,
                    "toggle" => cfg.trigger = Trigger::ToggleRmb,
                    "key" => cfg.trigger = Trigger::Key,
                    _ => {}
                },
                "key" => {
                    if let Some(vk) = parse_key(value).filter(|vk| usable_trigger_key(*vk)) {
                        cfg.key = vk;
                    }
                }
                "reduce_motion" => match value {
                    "1" | "true" | "yes" => cfg.reduce_motion = true,
                    "0" | "false" | "no" => cfg.reduce_motion = false,
                    _ => {}
                },
                "accent" => {
                    let hex = value.trim_start_matches('#');
                    if hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                        if let Ok(rgb) = u32::from_str_radix(hex, 16) {
                            cfg.accent = rgb;
                        }
                    }
                }
                "offset" => {
                    if let Some((x, y)) = value.split_once(',') {
                        if let (Ok(x), Ok(y)) = (x.trim().parse::<i32>(), y.trim().parse::<i32>()) {
                            cfg.offset = (x.clamp(-4000, 4000), y.clamp(-4000, 4000));
                        }
                    }
                }
                "autoupdate" => match value {
                    "1" | "true" | "yes" | "on" => cfg.autoupdate = true,
                    "0" | "false" | "no" | "off" => cfg.autoupdate = false,
                    _ => {}
                },
                "morph" => {
                    if let Ok(ms) = value.parse::<i64>() {
                        cfg.morph = ms.clamp(0, 1000) as u32;
                    }
                }
                _ => {
                    if cfg.exp.set(key, value) || cfg.display.set(key, value) || cfg.states.set(key, value) {
                        continue;
                    }
                    let Some((profile, field)) = key.split_once('.') else { continue };
                    let target = match profile {
                        "hip" => &mut cfg.hip,
                        "ads" => &mut cfg.ads,
                        _ => continue,
                    };
                    match field {
                        "outline" => {
                            if let Ok(n) = value.parse::<i64>() {
                                target.outline = n.clamp(0, 2) as u8;
                            }
                        }
                        "opacity" => {
                            if let Ok(n) = value.parse::<i64>() {
                                target.opacity = n.clamp(1, 10) as u8;
                            }
                        }
                        "layers" => {
                            if let Some(layers) = Crosshair::parse_layers(value) {
                                target.layers = layers;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        cfg
    }

    pub fn serialize(&self) -> String {
        let trigger = match self.trigger {
            Trigger::HoldRmb => "hold",
            Trigger::ToggleRmb => "toggle",
            Trigger::Key => "key",
        };
        let mut s = String::new();
        s.push_str("# reDraw config. F10 opens the menu (it saves here), F11 reloads this file, F9 shows or hides, Ctrl+Alt+F9 quits.\n");
        s.push_str("# trigger: hold (hold right mouse for ADS), toggle (right click flips), key (the key below flips)\n");
        s.push_str(&format!("trigger={trigger}\nkey={}\noffset={},{}\n", key_name(self.key), self.offset.0, self.offset.1));
        s.push_str(&format!("reduce_motion={}\naccent=#{:06x}\nmorph={}\nautoupdate={}\n", u8::from(self.reduce_motion), self.accent, self.morph, u8::from(self.autoupdate)));
        for (name, c) in [("hip", &self.hip), ("ads", &self.ads)] {
            s.push_str(&format!("{name}.outline={}\n{name}.opacity={}\n{name}.layers={}\n", c.outline, c.opacity, c.to_persist()));
        }
        self.states.serialize_into(&mut s);
        self.display.serialize_into(&mut s);
        self.exp.serialize_into(&mut s);
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_round_trips() {
        let cfg = Config::default();
        assert_eq!(Config::parse(&cfg.serialize()), cfg);
    }

    #[test]
    fn edited_values_apply() {
        let cfg = Config::parse("trigger=key\nkey=F6\noffset=3, -2\nads.opacity=5\nhip.layers=oc1 dot.ff0000.3.0.1.1.0\n");        assert_eq!(cfg.trigger, Trigger::Key);
        assert_eq!(cfg.key, VK_F1 + 5);
        assert_eq!(cfg.offset, (3, -2));
        assert_eq!(cfg.ads.opacity, 5);
        assert_eq!(cfg.hip.layers.len(), 1);
        assert_eq!(cfg.hip.layers[0].color, 0xFF0000);
    }

    #[test]
    fn garbage_falls_back_to_defaults() {
        for text in ["", "###", "trigger=banana", "key=zzz", "offset=1", "hip.layers=nope", "hip.opacity=99999", "=", "a.b.c=1", "\u{0}\u{1}"] {
            let cfg = Config::parse(text);
            assert!(!cfg.hip.layers.is_empty() && !cfg.ads.layers.is_empty(), "{text:?}");
            assert!((1..=10).contains(&cfg.hip.opacity), "{text:?}");
        }
        assert_eq!(Config::parse("hip.opacity=99999").hip.opacity, Crosshair::classic().opacity);
    }

    #[test]
    fn out_of_range_numbers_clamp_instead_of_falling_back() {
        let cfg = Config::parse("hip.outline=999\nhip.opacity=-5\nads.outline=-1\nads.opacity=0\n");
        assert_eq!(cfg.hip.outline, 2);
        assert_eq!(cfg.hip.opacity, 1);
        assert_eq!(cfg.ads.outline, 0);
        assert_eq!(cfg.ads.opacity, 1);
    }

    #[test]
    fn bom_and_key_collisions() {
        let cfg = Config::parse("\u{feff}trigger=toggle\nkey=F9\n");
        assert_eq!(cfg.trigger, Trigger::ToggleRmb, "BOM must not hide the first line");
        assert_eq!(cfg.key, VK_F8, "F9 collides with show/hide, keep the default");
        for bad in ["key=F11", "key=0x01", "key=0x06"] {
            assert_eq!(Config::parse(bad).key, VK_F8, "{bad}");
        }
        assert_eq!(Config::parse("key=F6").key, VK_F1 + 5);
    }

    #[test]
    fn hidden_layers_survive_a_save() {
        let mut cfg = Config::default();
        cfg.hip.layers[1].visible = false;
        let back = Config::parse(&cfg.serialize());
        assert!(!back.hip.layers[1].visible);
        assert!(back.hip.layers[0].visible);
    }

    #[test]
    fn a_config_saved_by_the_old_follow_by_default_build_loads_at_the_screen_center() {
        // What the previous build wrote on first run.
        let old = "trigger=hold\nkey=F8\noffset=0,0\ndisplay.monitor=auto\ndisplay.scale=auto\ndisplay.ui_scale=auto\ndisplay.fps=auto\n";
        let cfg = Config::parse(old);
        assert_eq!(cfg.display.monitor, crate::display::MonitorSel::Primary);
        assert_eq!(Config::default().display.monitor, crate::display::MonitorSel::Primary);
        assert!(cfg.serialize().contains("display.monitor=primary"), "and it is saved back as primary");
    }

    #[test]
    fn wave_one_settings_survive_a_save_and_an_old_file_still_loads() {
        let mut cfg = Config::default();
        cfg.morph = 250;
        cfg.states.crouch.key = 0x11;
        cfg.states.crouch.toggle = true;
        cfg.states.sprint.color = Some(0xFF9500);
        let l = &mut cfg.hip.layers[0];
        l.opacity = 6;
        l.ox = 2;
        l.mirror = crate::crosshair::Mirror::Both;
        l.cycle = 1;
        assert_eq!(Config::parse(&cfg.serialize()), cfg);
        // A file written before these settings existed keeps its crosshair and gets the defaults.
        let old = "trigger=hold\nkey=F8\nhip.layers=oc1 spk.ff0000.6.4.3.4.0\n";
        let loaded = Config::parse(old);
        assert_eq!((loaded.morph, loaded.states.clone()), (DEFAULT_MORPH_MS, States::default()));
        assert_eq!(loaded.hip.layers[0].color, 0xFF0000);
        assert_eq!(Config::parse("morph=99999").morph, 1000);
        assert_eq!(Config::parse("morph=-5").morph, 0);
        assert_eq!(Config::parse("morph=fast").morph, DEFAULT_MORPH_MS);
    }

    #[test]
    fn auto_update_is_on_by_default_and_can_be_turned_off() {
        assert!(Config::default().autoupdate);
        assert!(Config::parse("trigger=hold\n").autoupdate, "an older file without the key gets the default");
        let mut cfg = Config::default();
        cfg.autoupdate = false;
        assert!(!Config::parse(&cfg.serialize()).autoupdate);
        assert!(!Config::parse("autoupdate=off").autoupdate && Config::parse("autoupdate=on").autoupdate);
        assert!(Config::parse("autoupdate=maybe").autoupdate, "unreadable values keep the default");
    }

    #[test]
    fn keys_parse_both_ways() {
        assert_eq!(parse_key("F8"), Some(VK_F8));
        assert_eq!(parse_key("0x41"), Some(0x41));
        assert_eq!(parse_key("F25"), None);
        assert_eq!(parse_key("0x0"), None);
        assert_eq!(key_name(0x41), "0x41");
    }
}
