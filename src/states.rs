//! Input-driven states: crouch, sprint and jump as keys you bind. While a bound key is held
//! (or switched on, in toggle mode) the crosshair is nudged: wider or tighter, bigger or smaller,
//! optionally another color.
//!
//! It only ever sees your own key presses, never the game, so it is cosmetic. If you remap a key
//! in the game, or toggle crouch there but hold it here, the two simply disagree.

use crate::crosshair::Crosshair;
use crate::dynamics::{self, Frame};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Jump,
    Sprint,
    Crouch,
}

/// Highest priority first: when several are active, the first of these wins.
pub const ALL: [State; 3] = [State::Jump, State::Sprint, State::Crouch];

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Jump => "Jump",
            State::Sprint => "Sprint",
            State::Crouch => "Crouch",
        }
    }

    fn tag(self) -> &'static str {
        match self {
            State::Jump => "jump",
            State::Sprint => "sprint",
            State::Crouch => "crouch",
        }
    }

    fn index(self) -> usize {
        ALL.iter().position(|s| *s == self).unwrap_or(0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    /// virtual-key code as raw input reports it (Shift is 0x10, not left/right); 0 = unbound
    pub key: u16,
    /// the key switches the state on and off, instead of holding it
    pub toggle: bool,
    /// pixels added to every spread (spoke gap, ring radius, ...): -6..=8
    pub gap: i32,
    /// pixels added to every size: -4..=4
    pub size: i32,
    pub color: Option<u32>,
}

pub const GAP_RANGE: (i32, i32) = (-6, 8);
pub const SIZE_RANGE: (i32, i32) = (-4, 4);

/// Keys the menu offers, in order. Letters are kept off the movement keys the defaults use.
pub const KEYS: [u16; 14] = [0, 0x10, 0x11, 0x12, 0x20, 0x43, 0x58, 0x5A, 0x56, 0x51, 0x45, 0x46, 0x47, 0x14];

pub fn key_name(vk: u16) -> String {
    match vk {
        0 => "None".to_string(),
        0x10 => "Shift".to_string(),
        0x11 => "Ctrl".to_string(),
        0x12 => "Alt".to_string(),
        0x14 => "Caps".to_string(),
        0x20 => "Space".to_string(),
        0x41..=0x5A => ((vk as u8) as char).to_string(),
        _ => format!("0x{vk:02X}"),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct States {
    pub jump: Rule,
    pub sprint: Rule,
    pub crouch: Rule,
}

impl Default for States {
    fn default() -> States {
        let rule = |gap| Rule { key: 0, toggle: false, gap, size: 0, color: None };
        States { jump: rule(4), sprint: rule(3), crouch: rule(-2) }
    }
}

impl States {
    pub fn rule(&self, s: State) -> &Rule {
        match s {
            State::Jump => &self.jump,
            State::Sprint => &self.sprint,
            State::Crouch => &self.crouch,
        }
    }

    pub fn rule_mut(&mut self, s: State) -> &mut Rule {
        match s {
            State::Jump => &mut self.jump,
            State::Sprint => &mut self.sprint,
            State::Crouch => &mut self.crouch,
        }
    }

    /// Is any state bound to a key? Then the keyboard has to be listened to.
    pub fn any_bound(&self) -> bool {
        ALL.iter().any(|s| self.rule(*s).key != 0)
    }

    /// Apply one `state.<name>.<field>=<value>` line. Returns false if the key is not one of ours.
    pub fn set(&mut self, key: &str, value: &str) -> bool {
        let Some(rest) = key.strip_prefix("state.") else { return false };
        let Some((name, field)) = rest.split_once('.') else { return false };
        let Some(state) = ALL.into_iter().find(|s| s.tag() == name) else { return false };
        let r = self.rule_mut(state);
        match field {
            "key" => {
                if value == "none" {
                    r.key = 0;
                } else if let Some(vk) = value.strip_prefix("0x").and_then(|h| u16::from_str_radix(h, 16).ok()).filter(|vk| (7..=0xFE).contains(vk)) {
                    r.key = vk;
                }
            }
            "mode" => match value {
                "hold" => r.toggle = false,
                "toggle" => r.toggle = true,
                _ => {}
            },
            "gap" => {
                if let Ok(n) = value.parse::<i64>() {
                    r.gap = n.clamp(GAP_RANGE.0 as i64, GAP_RANGE.1 as i64) as i32;
                }
            }
            "size" => {
                if let Ok(n) = value.parse::<i64>() {
                    r.size = n.clamp(SIZE_RANGE.0 as i64, SIZE_RANGE.1 as i64) as i32;
                }
            }
            "color" => {
                if value == "none" {
                    r.color = None;
                } else {
                    let hex = value.trim_start_matches('#');
                    if hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                        r.color = u32::from_str_radix(hex, 16).ok();
                    }
                }
            }
            _ => {}
        }
        true
    }

    pub fn serialize_into(&self, out: &mut String) {
        for s in ALL {
            let r = self.rule(s);
            let key = if r.key == 0 { "none".to_string() } else { format!("0x{:02X}", r.key) };
            let color = r.color.map_or("none".to_string(), |c| format!("#{c:06x}"));
            let (tag, mode, gap, size) = (s.tag(), if r.toggle { "toggle" } else { "hold" }, r.gap, r.size);
            out.push_str(&format!("state.{tag}.key={key}\nstate.{tag}.mode={mode}\nstate.{tag}.gap={gap}\nstate.{tag}.size={size}\nstate.{tag}.color={color}\n"));
        }
    }
}

/// Which keys are down, and which toggles are switched on.
#[derive(Debug, Default)]
pub struct Tracker {
    down: [bool; 3],
    on: [bool; 3],
}

impl Tracker {
    pub fn new() -> Tracker {
        Tracker::default()
    }

    /// Forget everything: the rules changed, so held keys and latched toggles mean nothing now.
    pub fn reset(&mut self) {
        *self = Tracker::default();
    }

    /// Feed one key event. Returns true if the state that is active changed.
    pub fn key(&mut self, states: &States, vk: u16, up: bool) -> bool {
        let before = self.active(states);
        for s in ALL {
            let r = states.rule(s);
            if r.key == 0 || r.key != vk {
                continue;
            }
            let i = s.index();
            if up {
                self.down[i] = false;
            } else if !self.down[i] {
                // Key repeat sends more downs while held; only the first one counts.
                self.down[i] = true;
                if r.toggle {
                    self.on[i] = !self.on[i];
                }
            }
        }
        self.active(states) != before
    }

    /// The state in effect: the highest-priority one that is held or switched on.
    pub fn active(&self, states: &States) -> Option<State> {
        ALL.into_iter().find(|s| {
            let r = states.rule(*s);
            r.key != 0 && if r.toggle { self.on[s.index()] } else { self.down[s.index()] }
        })
    }
}

/// The crosshair as drawn while `rule` is in effect.
pub fn apply(base: &Crosshair, rule: &Rule) -> Crosshair {
    let mut c = dynamics::apply(base, &Frame { spread: rule.gap, rot: 0, opacity: 1.0 });
    for l in &mut c.layers {
        l.reach += rule.size;
        if let Some(color) = rule.color {
            l.color = color;
        }
        l.clamp();
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bound() -> States {
        let mut s = States::default();
        s.sprint.key = 0x10;
        s.crouch.key = 0x11;
        s.jump.key = 0x20;
        s
    }

    #[test]
    fn nothing_is_bound_by_default_so_nothing_listens() {
        let s = States::default();
        assert!(!s.any_bound());
        let mut t = Tracker::new();
        assert!(!t.key(&s, 0x10, false) && t.active(&s).is_none(), "an unbound key does nothing");
        assert!(bound().any_bound());
    }

    #[test]
    fn hold_follows_the_key_and_ignores_repeat() {
        let s = bound();
        let mut t = Tracker::new();
        assert!(t.key(&s, 0x10, false));
        assert_eq!(t.active(&s), Some(State::Sprint));
        assert!(!t.key(&s, 0x10, false), "key repeat changes nothing");
        assert!(t.key(&s, 0x10, true));
        assert_eq!(t.active(&s), None);
        assert!(!t.key(&s, 0x41, false), "other keys are not looked at");
    }

    #[test]
    fn toggle_flips_once_per_press() {
        let mut s = bound();
        s.crouch.toggle = true;
        let mut t = Tracker::new();
        t.key(&s, 0x11, false);
        t.key(&s, 0x11, false);
        t.key(&s, 0x11, true);
        assert_eq!(t.active(&s), Some(State::Crouch), "stays on after release; repeat did not flip it back");
        t.key(&s, 0x11, false);
        t.key(&s, 0x11, true);
        assert_eq!(t.active(&s), None, "second press switches it off");
        t.key(&s, 0x11, false);
        t.reset();
        assert_eq!(t.active(&s), None, "reset forgets a latched toggle");
    }

    #[test]
    fn the_highest_priority_active_state_wins() {
        let s = bound();
        let mut t = Tracker::new();
        t.key(&s, 0x11, false);
        assert_eq!(t.active(&s), Some(State::Crouch));
        t.key(&s, 0x10, false);
        assert_eq!(t.active(&s), Some(State::Sprint), "sprint outranks crouch");
        t.key(&s, 0x20, false);
        assert_eq!(t.active(&s), Some(State::Jump), "jump outranks both");
        t.key(&s, 0x20, true);
        t.key(&s, 0x10, true);
        assert_eq!(t.active(&s), Some(State::Crouch), "and it falls back down the list");
    }

    #[test]
    fn apply_nudges_spread_size_and_color_inside_the_grid() {
        let base = Crosshair::classic();
        let tight = apply(&base, &Rule { key: 1, toggle: false, gap: -2, size: 0, color: None });
        assert_eq!(tight.layers[0].gap, base.layers[0].gap - 2);
        assert_eq!(tight.layers[1], base.layers[1], "the dot stays");
        let big = apply(&base, &Rule { key: 1, toggle: false, gap: 0, size: 2, color: Some(0xFF0000) });
        assert_eq!(big.layers[0].reach, base.layers[0].reach + 2);
        assert!(big.layers.iter().all(|l| l.color == 0xFF0000));
        let neutral = apply(&base, &Rule { key: 1, toggle: false, gap: 0, size: 0, color: None });
        assert_eq!(neutral, base, "a rule that changes nothing leaves the crosshair as it was");
        let huge = apply(&base, &Rule { key: 1, toggle: false, gap: 8, size: 4, color: None });
        let l = &huge.layers[0];
        assert!(l.gap + l.reach <= 16);
        let tiny = apply(&base, &Rule { key: 1, toggle: false, gap: -6, size: -4, color: None });
        assert!(tiny.layers[0].gap >= 0 && tiny.layers[0].reach >= 1);
        assert_eq!(tiny.layers[1].thick % 2, 1, "dot size stays valid");
    }

    #[test]
    fn config_lines_round_trip_and_bad_ones_are_ignored() {
        let mut s = States::default();
        s.crouch = Rule { key: 0x43, toggle: true, gap: -3, size: 1, color: Some(0x12ABEF) };
        let mut text = String::new();
        s.serialize_into(&mut text);
        let mut back = States::default();
        for line in text.lines() {
            let (k, v) = line.split_once('=').expect("key=value");
            assert!(back.set(k, v), "{k} is ours");
        }
        assert_eq!(back, s);
        let mut bad = States::default();
        for (k, v) in [("state.crouch.key", "0xZZ"), ("state.crouch.key", "0x01"), ("state.crouch.key", "0x1FF"), ("state.crouch.mode", "sometimes"), ("state.crouch.gap", "x"), ("state.crouch.color", "#12"), ("state.crouch.nope", "1")] {
            assert!(bad.set(k, v), "{k} is recognised even when the value is not");
        }
        assert_eq!(bad, States::default(), "nothing bad got through");
        assert!(!bad.set("state.dive.key", "0x41") && !bad.set("trigger", "hold") && !bad.set("state.crouch", "1"));
        assert!(bad.set("state.crouch.gap", "99") && bad.crouch.gap == GAP_RANGE.1, "out of range clamps");
    }

    #[test]
    fn every_menu_key_has_a_readable_name() {
        assert_eq!(key_name(0), "None");
        assert_eq!(key_name(0x10), "Shift");
        assert_eq!(key_name(0x43), "C");
        let names: std::collections::BTreeSet<String> = KEYS.iter().map(|k| key_name(*k)).collect();
        assert_eq!(names.len(), KEYS.len(), "no two keys share a name");
        assert!(KEYS.iter().all(|k| !names.contains(&format!("0x{k:02X}")) || *k == 0), "none falls back to a hex code");
    }
}
