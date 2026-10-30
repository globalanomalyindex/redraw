//! Settings for the Experimental [!] features: plain data, tolerant parsing,
//! and the exact lists the menu cycles through. Bad values fall back to the
//! default for that field, like the rest of the config.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    None,
    Pulse,
    Spin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceActive {
    Fire,
    Always,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceStyle {
    Trail,
    Line,
    Both,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceReturn {
    Fade,
    Snap,
    Hold,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZoomWhen {
    Ads,
    Always,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoMode {
    LightDark,
    Palette,
}

/// Crosshair that reacts to your own input (never to the game).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dynamics {
    pub enabled: bool,
    /// widening while the fire button is held: 0 off, 1 low, 2 medium, 3 high
    pub bloom: u8,
    /// widening while a movement key is held: 0 off .. 3 high
    pub spread: u8,
    /// fade after no input: 0 off, 1 3s, 2 6s, 3 12s
    pub idle: u8,
    pub motion: Motion,
    /// the four movement keys (forward, left, back, right) as upper-case ASCII
    pub keys: [u8; 4],
}

/// Tracer drawn from raw mouse movement around the crosshair.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trace {
    pub enabled: bool,
    pub active: TraceActive,
    pub style: TraceStyle,
    pub ret: TraceReturn,
    pub color: u32,
    /// color the line by mouse speed
    pub speed_color: bool,
    /// 1..=3 pixels
    pub width: i32,
    /// 1..=10, tenths of a pixel per mouse count
    pub scale: i32,
    /// reach of the tracer from the crosshair center: 64, 128 or 256 pixels
    pub range: i32,
    /// 3..=30, tenths of a second a trail point lasts
    pub fade: i32,
    /// 0 off .. 3 heavy
    pub smooth: u8,
}

/// Magnified circle or square at the screen center.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Zoom {
    pub enabled: bool,
    /// tenths: 15 is 1.5x
    pub factor: i32,
    pub size: i32,
    pub circle: bool,
    pub when: ZoomWhen,
}

/// Crosshair color picked from what is behind it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Auto {
    pub enabled: bool,
    pub mode: AutoMode,
    /// samples per second: 1 slow (4), 2 medium (10), 3 fast (20)
    pub rate: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Experimental {
    pub show_on_capture: bool,
    pub dynamics: Dynamics,
    pub trace: Trace,
    pub zoom: Zoom,
    pub auto: Auto,
}

pub const TRACE_RANGES: [i32; 3] = [64, 128, 256];

impl Default for Experimental {
    fn default() -> Experimental {
        Experimental {
            show_on_capture: true,
            dynamics: Dynamics { enabled: false, bloom: 2, spread: 1, idle: 0, motion: Motion::None, keys: *b"WASD" },
            trace: Trace {
                enabled: false,
                active: TraceActive::Fire,
                style: TraceStyle::Both,
                ret: TraceReturn::Fade,
                color: 0x00E5FF,
                speed_color: false,
                width: 1,
                scale: 3,
                range: 128,
                fade: 8,
                smooth: 1,
            },
            zoom: Zoom { enabled: false, factor: 20, size: 220, circle: true, when: ZoomWhen::Ads },
            auto: Auto { enabled: false, mode: AutoMode::LightDark, rate: 2 },
        }
    }
}

fn flag(v: &str) -> Option<bool> {
    match v {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn int(v: &str) -> Option<i64> {
    v.parse::<i64>().ok()
}

fn rgb(v: &str) -> Option<u32> {
    let hex = v.trim_start_matches('#');
    if hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        u32::from_str_radix(hex, 16).ok()
    } else {
        None
    }
}

fn motion_tag(m: Motion) -> &'static str {
    match m {
        Motion::None => "none",
        Motion::Pulse => "pulse",
        Motion::Spin => "spin",
    }
}

impl Experimental {
    /// Apply one `key=value` line. Returns false if the key is not one of ours.
    pub fn set(&mut self, key: &str, value: &str) -> bool {
        let (d, t, z, a) = (&mut self.dynamics, &mut self.trace, &mut self.zoom, &mut self.auto);
        match key {
            "capture" => {
                if let Some(v) = flag(value) {
                    self.show_on_capture = v;
                }
            }
            "dyn.enabled" => d.enabled = flag(value).unwrap_or(d.enabled),
            "dyn.bloom" => d.bloom = int(value).map(|n| n.clamp(0, 3) as u8).unwrap_or(d.bloom),
            "dyn.spread" => d.spread = int(value).map(|n| n.clamp(0, 3) as u8).unwrap_or(d.spread),
            "dyn.idle" => d.idle = int(value).map(|n| n.clamp(0, 3) as u8).unwrap_or(d.idle),
            "dyn.motion" => match value {
                "none" => d.motion = Motion::None,
                "pulse" => d.motion = Motion::Pulse,
                "spin" => d.motion = Motion::Spin,
                _ => {}
            },
            "dyn.keys" => {
                let b = value.as_bytes();
                if b.len() == 4 && b.iter().all(|c| c.is_ascii_alphanumeric()) {
                    d.keys = [b[0].to_ascii_uppercase(), b[1].to_ascii_uppercase(), b[2].to_ascii_uppercase(), b[3].to_ascii_uppercase()];
                }
            }
            "trace.enabled" => t.enabled = flag(value).unwrap_or(t.enabled),
            "trace.active" => match value {
                "fire" => t.active = TraceActive::Fire,
                "always" => t.active = TraceActive::Always,
                _ => {}
            },
            "trace.style" => match value {
                "trail" => t.style = TraceStyle::Trail,
                "line" => t.style = TraceStyle::Line,
                "both" => t.style = TraceStyle::Both,
                _ => {}
            },
            "trace.return" => match value {
                "fade" => t.ret = TraceReturn::Fade,
                "snap" => t.ret = TraceReturn::Snap,
                "hold" => t.ret = TraceReturn::Hold,
                _ => {}
            },
            "trace.color" => t.color = rgb(value).unwrap_or(t.color),
            "trace.speed_color" => t.speed_color = flag(value).unwrap_or(t.speed_color),
            "trace.width" => t.width = int(value).map(|n| n.clamp(1, 3) as i32).unwrap_or(t.width),
            "trace.scale" => t.scale = int(value).map(|n| n.clamp(1, 10) as i32).unwrap_or(t.scale),
            "trace.range" => {
                if let Some(n) = int(value) {
                    t.range = *TRACE_RANGES.iter().min_by_key(|r| (**r as i64 - n).abs()).unwrap_or(&128);
                }
            }
            "trace.fade" => t.fade = int(value).map(|n| n.clamp(3, 30) as i32).unwrap_or(t.fade),
            "trace.smooth" => t.smooth = int(value).map(|n| n.clamp(0, 3) as u8).unwrap_or(t.smooth),
            "zoom.enabled" => z.enabled = flag(value).unwrap_or(z.enabled),
            "zoom.factor" => z.factor = int(value).map(|n| (n.clamp(15, 40) as i32 + 2) / 5 * 5).unwrap_or(z.factor),
            "zoom.size" => z.size = int(value).map(|n| (n.clamp(100, 400) as i32 + 10) / 20 * 20).unwrap_or(z.size),
            "zoom.shape" => match value {
                "circle" => z.circle = true,
                "square" => z.circle = false,
                _ => {}
            },
            "zoom.when" => match value {
                "ads" => z.when = ZoomWhen::Ads,
                "always" => z.when = ZoomWhen::Always,
                _ => {}
            },
            "auto.enabled" => a.enabled = flag(value).unwrap_or(a.enabled),
            "auto.mode" => match value {
                "lightdark" => a.mode = AutoMode::LightDark,
                "palette" => a.mode = AutoMode::Palette,
                _ => {}
            },
            "auto.rate" => a.rate = int(value).map(|n| n.clamp(1, 3) as u8).unwrap_or(a.rate),
            _ => return false,
        }
        true
    }

    pub fn serialize_into(&self, s: &mut String) {
        let (d, t, z, a) = (&self.dynamics, &self.trace, &self.zoom, &self.auto);
        let b = |v: bool| u8::from(v);
        s.push_str(&format!("capture={}\n", b(self.show_on_capture)));
        s.push_str(&format!(
            "dyn.enabled={}\ndyn.bloom={}\ndyn.spread={}\ndyn.idle={}\ndyn.motion={}\ndyn.keys={}\n",
            b(d.enabled),
            d.bloom,
            d.spread,
            d.idle,
            motion_tag(d.motion),
            String::from_utf8_lossy(&d.keys)
        ));
        s.push_str(&format!(
            "trace.enabled={}\ntrace.active={}\ntrace.style={}\ntrace.return={}\ntrace.color=#{:06x}\ntrace.speed_color={}\ntrace.width={}\ntrace.scale={}\ntrace.range={}\ntrace.fade={}\ntrace.smooth={}\n",
            b(t.enabled),
            if t.active == TraceActive::Fire { "fire" } else { "always" },
            match t.style {
                TraceStyle::Trail => "trail",
                TraceStyle::Line => "line",
                TraceStyle::Both => "both",
            },
            match t.ret {
                TraceReturn::Fade => "fade",
                TraceReturn::Snap => "snap",
                TraceReturn::Hold => "hold",
            },
            t.color,
            b(t.speed_color),
            t.width,
            t.scale,
            t.range,
            t.fade,
            t.smooth
        ));
        s.push_str(&format!(
            "zoom.enabled={}\nzoom.factor={}\nzoom.size={}\nzoom.shape={}\nzoom.when={}\n",
            b(z.enabled),
            z.factor,
            z.size,
            if z.circle { "circle" } else { "square" },
            if z.when == ZoomWhen::Ads { "ads" } else { "always" }
        ));
        s.push_str(&format!(
            "auto.enabled={}\nauto.mode={}\nauto.rate={}\n",
            b(a.enabled),
            if a.mode == AutoMode::LightDark { "lightdark" } else { "palette" },
            a.rate
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(text: &str) -> Experimental {
        let mut e = Experimental::default();
        for line in text.lines() {
            if let Some((k, v)) = line.split_once('=') {
                e.set(k.trim(), v.trim());
            }
        }
        e
    }

    #[test]
    fn defaults_round_trip() {
        let e = Experimental::default();
        let mut s = String::new();
        e.serialize_into(&mut s);
        assert_eq!(apply(&s), e);
    }

    #[test]
    fn every_non_default_value_round_trips() {
        let mut e = Experimental::default();
        e.show_on_capture = false;
        e.dynamics = Dynamics { enabled: true, bloom: 3, spread: 2, idle: 3, motion: Motion::Spin, keys: *b"ESDF" };
        e.trace = Trace {
            enabled: true,
            active: TraceActive::Always,
            style: TraceStyle::Line,
            ret: TraceReturn::Snap,
            color: 0xFF4DDB,
            speed_color: true,
            width: 3,
            scale: 9,
            range: 256,
            fade: 25,
            smooth: 3,
        };
        e.zoom = Zoom { enabled: true, factor: 35, size: 340, circle: false, when: ZoomWhen::Always };
        e.auto = Auto { enabled: true, mode: AutoMode::Palette, rate: 3 };
        let mut s = String::new();
        e.serialize_into(&mut s);
        assert_eq!(apply(&s), e);
    }

    #[test]
    fn out_of_range_and_garbage_values_are_tamed() {
        let e = apply("dyn.bloom=99\ndyn.spread=-4\ntrace.width=0\ntrace.scale=1000\ntrace.fade=1\ntrace.range=100\nzoom.factor=999\nzoom.size=7\nauto.rate=0\ntrace.color=nope\ndyn.keys=ab\ndyn.motion=wobble\ncapture=maybe");
        assert_eq!((e.dynamics.bloom, e.dynamics.spread), (3, 0));
        assert_eq!((e.trace.width, e.trace.scale, e.trace.fade, e.trace.range), (1, 10, 3, 128));
        assert_eq!((e.zoom.factor, e.zoom.size), (40, 100));
        assert_eq!(e.auto.rate, 1);
        assert_eq!(e.trace.color, Experimental::default().trace.color);
        assert_eq!(e.dynamics.keys, *b"WASD");
        assert_eq!(e.dynamics.motion, Motion::None);
        assert!(e.show_on_capture, "unparseable flag keeps the default");
    }

    #[test]
    fn zoom_values_snap_to_the_menu_steps() {
        assert_eq!(apply("zoom.factor=22").zoom.factor, 20);
        assert_eq!(apply("zoom.factor=23").zoom.factor, 25);
        assert_eq!(apply("zoom.size=250").zoom.size, 260);
        assert_eq!(apply("trace.range=200").trace.range, 256);
    }

    #[test]
    fn unknown_keys_are_not_claimed() {
        let mut e = Experimental::default();
        assert!(!e.set("hip.outline", "1"));
        assert!(!e.set("trigger", "key"));
        assert!(e.set("dyn.bloom", "1"));
    }
}
