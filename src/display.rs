//! Display adaptation. Everything here is a pure decision from monitor facts,
//! so it can be tested without a monitor.
//!
//! The crosshair stays machine-sharp at any resolution by scaling in whole
//! numbers only: every crosshair pixel becomes a k x k block (nearest
//! neighbour), never a blend. `Auto` picks k from the monitor so the
//! crosshair keeps about the same apparent size from 1080p to 8K.

pub const FPS_CAPS: [u32; 6] = [30, 60, 120, 144, 165, 240];
pub const MAX_SCALE: i32 = 4;
pub const MAX_UI_SCALE: i32 = 3;

/// Whole-number scale for the crosshair: 1 up to 1440p-class screens, 2 at 4K, 4 at 8K.
/// Uses the smaller of the height and width ratios to 1920x1080, so an ultrawide
/// is judged by its height, not its width.
pub fn auto_scale(width: i32, height: i32) -> i32 {
    let ratio = (height as f32 / 1080.0).min(width as f32 / 1920.0).max(0.0);
    // Nearest whole number, so the apparent size stays as close to constant as whole pixels allow.
    ((ratio + 0.5).floor() as i32).clamp(1, MAX_SCALE)
}

/// Whole-number scale for the menu: from Windows' DPI scaling, but never smaller
/// than the resolution alone suggests (a 4K screen left at 100% still gets 2x).
pub fn auto_ui_scale(dpi: u32, width: i32, height: i32) -> i32 {
    let by_dpi = (((dpi as f32 / 96.0) + 0.25).floor() as i32).clamp(1, MAX_UI_SCALE);
    by_dpi.max(auto_scale(width, height).min(MAX_UI_SCALE))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Area {
    pub fn center(&self) -> (i32, i32) {
        (self.x + self.w / 2, self.y + self.h / 2)
    }
}

/// Where the crosshair belongs for a focused window: the center of its viewport
/// (client area) when that is a sizable part of the monitor (fullscreen, borderless,
/// or a big window), else the center of the monitor it is on. Covers windowed games,
/// stretched or letterboxed resolutions, and any aspect ratio.
pub fn follow_center(client: Area, monitor: Area) -> (i32, i32) {
    if client.w > 0 && client.h > 0 && client.w as i64 * client.h as i64 * 10 >= monitor.w as i64 * monitor.h as i64 * 3 {
        client.center()
    } else {
        monitor.center()
    }
}

/// Seconds between frames for a cap; `None` (Auto) means every display refresh.
pub fn frame_interval(fps: Option<u32>) -> Option<f32> {
    fps.filter(|f| *f > 0).map(|f| 1.0 / f as f32)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MonitorSel {
    /// the center of the primary monitor: always the middle of the screen (the default)
    Primary,
    /// opt-in: follow the focused window's viewport center, else its monitor's center
    Follow,
    /// the nth monitor from the left (1-based)
    Nth(u8),
}

/// What the display settings resolved to on this machine, shown next to "Auto".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    pub scale: i32,
    pub ui_scale: i32,
    pub refresh: u32,
}

impl Default for Info {
    fn default() -> Info {
        Info { scale: 1, ui_scale: 1, refresh: 60 }
    }
}

/// `None` in `scale`, `ui_scale` and `fps` means Auto. The monitor defaults to the
/// primary one, so the crosshair sits in the middle of the screen unless asked otherwise.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Display {
    pub monitor: MonitorSel,
    pub scale: Option<i32>,
    pub ui_scale: Option<i32>,
    pub fps: Option<u32>,
}

impl Default for Display {
    fn default() -> Display {
        Display { monitor: MonitorSel::Primary, scale: None, ui_scale: None, fps: None }
    }
}

fn auto_or<T>(v: &str, parse: impl Fn(&str) -> Option<T>) -> Option<Option<T>> {
    if v == "auto" {
        Some(None)
    } else {
        parse(v).map(Some)
    }
}

impl Display {
    /// Apply one `key=value` line. Returns false if the key is not one of ours.
    pub fn set(&mut self, key: &str, value: &str) -> bool {
        match key {
            "display.monitor" => match value {
                "follow" => self.monitor = MonitorSel::Follow,
                // "auto" is what older configs saved when following windows was the default
                "primary" | "auto" => self.monitor = MonitorSel::Primary,
                n => {
                    if let Ok(n) = n.parse::<u8>() {
                        self.monitor = MonitorSel::Nth(n.clamp(1, 8));
                    }
                }
            },
            "display.scale" => {
                if let Some(v) = auto_or(value, |s| s.parse::<i32>().ok().map(|n| n.clamp(1, MAX_SCALE))) {
                    self.scale = v;
                }
            }
            "display.ui_scale" => {
                if let Some(v) = auto_or(value, |s| s.parse::<i32>().ok().map(|n| n.clamp(1, MAX_UI_SCALE))) {
                    self.ui_scale = v;
                }
            }
            "display.fps" => {
                if let Some(v) = auto_or(value, |s| s.parse::<i64>().ok().and_then(|n| FPS_CAPS.iter().copied().min_by_key(|c| (*c as i64 - n).abs()))) {
                    self.fps = v;
                }
            }
            _ => return false,
        }
        true
    }

    pub fn serialize_into(&self, s: &mut String) {
        let monitor = match self.monitor {
            MonitorSel::Follow => "follow".to_string(),
            MonitorSel::Primary => "primary".to_string(),
            MonitorSel::Nth(n) => n.to_string(),
        };
        let opt = |v: Option<i64>| v.map_or("auto".to_string(), |n| n.to_string());
        s.push_str(&format!(
            "display.monitor={monitor}\ndisplay.scale={}\ndisplay.ui_scale={}\ndisplay.fps={}\n",
            opt(self.scale.map(i64::from)),
            opt(self.ui_scale.map(i64::from)),
            opt(self.fps.map(i64::from))
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crosshair_scale_follows_resolution_in_whole_steps() {
        for ((w, h), want) in [
            ((1280, 720), 1),
            ((1920, 1080), 1),
            ((2560, 1440), 1),
            ((3440, 1440), 1),
            ((5120, 1440), 1),
            ((3840, 1080), 1),
            ((2560, 1600), 1),
            ((3840, 2160), 2),
            ((5120, 2880), 3),
            ((7680, 4320), 4),
        ] {
            assert_eq!(auto_scale(w, h), want, "{w}x{h}");
        }
        assert_eq!(auto_scale(0, 0), 1, "a bad reading still gives a usable scale");
        assert_eq!(auto_scale(30000, 30000), MAX_SCALE);
    }

    #[test]
    fn aspect_ratio_does_not_change_the_scale() {
        // Same height, any width: the crosshair is judged by height (and the narrower ratio).
        for w in [2560, 3440, 3840, 5120, 7680] {
            assert_eq!(auto_scale(w, 1440), auto_scale(2560, 1440), "{w}x1440");
        }
        assert_eq!(auto_scale(1440, 1440), 1, "a square screen");
        assert_eq!(auto_scale(1080, 1920), 1, "a portrait screen is judged by its short side");
    }

    #[test]
    fn menu_scale_uses_the_larger_of_dpi_and_resolution() {
        assert_eq!(auto_ui_scale(96, 1920, 1080), 1);
        assert_eq!(auto_ui_scale(120, 2560, 1440), 1, "125% on 1440p");
        assert_eq!(auto_ui_scale(144, 2560, 1440), 1, "150% rounds down: the menu is pixel-exact, not blurred");
        assert_eq!(auto_ui_scale(192, 1920, 1080), 2, "200% on 1080p");
        assert_eq!(auto_ui_scale(96, 3840, 2160), 2, "4K left at 100% still gets a readable menu");
        assert_eq!(auto_ui_scale(144, 3840, 2160), 2);
        assert_eq!(auto_ui_scale(288, 3840, 2160), 3);
        assert_eq!(auto_ui_scale(960, 7680, 4320), MAX_UI_SCALE);
        assert_eq!(auto_ui_scale(0, 0, 0), 1);
    }

    #[test]
    fn crosshair_follows_the_viewport_of_a_big_window_else_the_monitor() {
        let mon = Area { x: 0, y: 0, w: 2560, h: 1440 };
        let full = Area { x: 0, y: 0, w: 2560, h: 1440 };
        assert_eq!(follow_center(full, mon), (1280, 720), "fullscreen");
        let borderless_43 = Area { x: 320, y: 0, w: 1920, h: 1440 };
        assert_eq!(follow_center(borderless_43, mon), (1280, 720), "4:3 centered");
        let windowed = Area { x: 100, y: 60, w: 1600, h: 900 };
        assert_eq!(follow_center(windowed, mon), (900, 510), "a big window: its own center, not the screen's");
        let letterboxed = Area { x: 0, y: 180, w: 2560, h: 1080 };
        assert_eq!(follow_center(letterboxed, mon), (1280, 720));
        let small = Area { x: 50, y: 50, w: 400, h: 300 };
        assert_eq!(follow_center(small, mon), (1280, 720), "a small window (Notepad) does not drag the crosshair");
        assert_eq!(follow_center(Area { x: 0, y: 0, w: 0, h: 0 }, mon), (1280, 720), "minimized or empty");
        let second = Area { x: 2560, y: 0, w: 1920, h: 1080 };
        assert_eq!(follow_center(Area { x: 2560, y: 0, w: 1920, h: 1080 }, second), (3520, 540), "a second monitor");
        let edge = Area { x: 0, y: 0, w: 1400, h: 1100 };
        assert_eq!(follow_center(edge, mon), (700, 550), "just over the 30% threshold counts");
        let just_under = Area { x: 0, y: 0, w: 1300, h: 850 };
        assert_eq!(follow_center(just_under, mon), (1280, 720), "just under does not");
    }

    #[test]
    fn frame_cap_is_an_interval_or_none() {
        assert_eq!(frame_interval(None), None);
        assert_eq!(frame_interval(Some(0)), None);
        assert!((frame_interval(Some(60)).unwrap() - 1.0 / 60.0).abs() < 1e-6);
        assert!(frame_interval(Some(240)).unwrap() < frame_interval(Some(144)).unwrap());
    }

    #[test]
    fn the_crosshair_defaults_to_the_middle_of_the_screen_not_to_windows() {
        assert_eq!(Display::default().monitor, MonitorSel::Primary);
        let mut d = Display { monitor: MonitorSel::Follow, ..Display::default() };
        // A config saved by an older build said "auto" and followed windows: it must now mean the screen center.
        assert!(d.set("display.monitor", "auto"));
        assert_eq!(d.monitor, MonitorSel::Primary, "legacy auto no longer follows windows");
        d.set("display.monitor", "follow");
        assert_eq!(d.monitor, MonitorSel::Follow, "following a window is opt-in");
        let mut s = String::new();
        d.serialize_into(&mut s);
        assert!(s.contains("display.monitor=follow"));
        let mut s = String::new();
        Display::default().serialize_into(&mut s);
        assert!(s.contains("display.monitor=primary"), "saving rewrites an old auto to primary");
    }

    #[test]
    fn settings_round_trip_and_default_to_auto() {
        let d = Display::default();
        assert_eq!((d.monitor, d.scale, d.ui_scale, d.fps), (MonitorSel::Primary, None, None, None));
        let mut s = String::new();
        d.serialize_into(&mut s);
        let mut back = Display { monitor: MonitorSel::Follow, scale: Some(3), ui_scale: Some(2), fps: Some(60) };
        for line in s.lines() {
            let (k, v) = line.split_once('=').unwrap();
            assert!(back.set(k, v));
        }
        assert_eq!(back, d);
        let custom = Display { monitor: MonitorSel::Nth(2), scale: Some(2), ui_scale: Some(3), fps: Some(144) };
        let mut s = String::new();
        custom.serialize_into(&mut s);
        let mut back = Display::default();
        for line in s.lines() {
            let (k, v) = line.split_once('=').unwrap();
            back.set(k, v);
        }
        assert_eq!(back, custom);
    }

    #[test]
    fn bad_values_are_tamed_or_ignored() {
        let mut d = Display::default();
        d.set("display.scale", "99");
        assert_eq!(d.scale, Some(MAX_SCALE));
        d.set("display.scale", "-3");
        assert_eq!(d.scale, Some(1));
        d.set("display.scale", "banana");
        assert_eq!(d.scale, Some(1), "unparseable keeps the old value");
        d.set("display.ui_scale", "9");
        assert_eq!(d.ui_scale, Some(MAX_UI_SCALE));
        d.set("display.fps", "100");
        assert_eq!(d.fps, Some(120));
        d.set("display.fps", "1000000");
        assert_eq!(d.fps, Some(240));
        d.set("display.fps", "auto");
        assert_eq!(d.fps, None);
        d.set("display.monitor", "0");
        assert_eq!(d.monitor, MonitorSel::Nth(1));
        d.set("display.monitor", "primary");
        assert_eq!(d.monitor, MonitorSel::Primary);
        assert!(!d.set("trigger", "key"));
        assert!(!d.set("display.nothing", "1"));
    }
}
