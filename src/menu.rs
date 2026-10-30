//! The HUD menu as data: which rows exist, where they sit, and what input does.
//!
//! No Win32 and no drawing here. `layout` turns config + UI state into a flat
//! list of rectangles, `mouse_*` / `key` / `wheel` turn input into changes to
//! the config, and every call returns `Effects` telling the caller what to
//! redraw, save or copy. Geometry follows DESIGN.md section 1, in HUD-local
//! pixels (origin = top-left of the HUD window).

use crate::color;
use crate::config::{key_name, Config, Profile, Trigger, TRIGGER_KEYS};
use crate::crosshair::{presets, Blend, Crosshair, Kind, Layer, Mirror, MAX_LAYERS, PALETTE};
use crate::display::{Info, MonitorSel, FPS_CAPS};
use crate::experimental::{AutoMode, Motion, TraceActive, TraceReturn, TraceStyle, ZoomWhen, TRACE_RANGES};
use crate::morph;
use crate::share;
use crate::states::{self, State};
use crate::update::{self, Status};

pub const HUD_W: i32 = 212;
pub const BORDER: i32 = 1;
pub const PAD_TOP: i32 = 4;
pub const PAD_BOTTOM: i32 = 6;
pub const CONTENT_X: i32 = 7;
pub const CONTENT_W: i32 = 198;
pub const RULE_X: i32 = 10;
pub const ROW_X: i32 = 19;
pub const ROW_W: i32 = 186;
pub const ROW_H: i32 = 14;
pub const TITLE_H: i32 = 14;
pub const CREDIT_H: i32 = 11;
/// The line between the title and the credit: the update status, in the small type.
pub const UPDATE_H: i32 = 11;
pub const SECTION_GAP: i32 = 4;
pub const LINK_GAP: i32 = 8;
pub const SLIDER_GAP: i32 = 4;
pub const SLIDER_W: i32 = 92;
pub const THUMB_W: i32 = 4;
pub const VALUE_W: i32 = 30;
pub const PREVIEW: i32 = 165;
pub const EXPORT_LINE_H: i32 = 12;
pub const WHEEL_STEP: i32 = 42;
/// Extra indent of rows inside a sub-group: 3 margin + 1 rule + 4 padding.
pub const GROUP_INDENT: i32 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Metrics {
    /// advance of `0` at 11px
    pub body_cell: i32,
    /// advance of `0` at 9px
    pub small_cell: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Profile = 0,
    Layers = 1,
    Layer = 2,
    Presets = 3,
    Output = 4,
    Preview = 5,
    Experimental = 6,
    Display = 7,
    States = 8,
    Updates = 9,
}

/// The four rows of the Display section; each has an Auto choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disp {
    Monitor,
    Scale,
    UiScale,
    Fps,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Param {
    Reach,
    Gap,
    Thick,
    Count,
    Rot,
}

/// Collapsible sub-groups inside the Experimental [!] section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Blend = 0,
    Dynamics = 1,
    Trace = 2,
    Zoom = 3,
    Auto = 4,
    Color = 5,
    Transform = 6,
    Jump = 7,
    Sprint = 8,
    Crouch = 9,
}

impl Group {
    fn of(s: State) -> Group {
        match s {
            State::Jump => Group::Jump,
            State::Sprint => Group::Sprint,
            State::Crouch => Group::Crouch,
        }
    }
}

/// The custom-color and transform rows of the selected layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lx {
    Hue,
    Sat,
    Val,
    Cycle,
    CopyHex,
    PasteHex,
    Opacity,
    OffX,
    OffY,
    Mirror,
}

impl Lx {
    pub fn is_slider(self) -> bool {
        matches!(self, Lx::Hue | Lx::Sat | Lx::Val | Lx::Opacity | Lx::OffX | Lx::OffY)
    }
}

/// The rows of one input state (crouch, sprint, jump).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sx {
    Key,
    Mode,
    Gap,
    Size,
    Color,
}

impl Sx {
    pub fn is_slider(self) -> bool {
        matches!(self, Sx::Gap | Sx::Size)
    }
}

/// Text the overlay should fetch from the clipboard and hand back to the menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paste {
    Hex,
    Code,
}

/// Every control in the Experimental [!] section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exp {
    CaptureShow,
    Blend,
    DynEnable,
    DynBloom,
    DynSpread,
    DynIdle,
    DynMotion,
    TraceEnable,
    TraceActive,
    TraceStyle,
    TraceReturn,
    TraceColor,
    TraceSpeedColor,
    TraceWidth,
    TraceScale,
    TraceFade,
    TraceRange,
    TraceSmooth,
    TraceTest,
    ZoomEnable,
    ZoomFactor,
    ZoomSize,
    ZoomShape,
    ZoomWhen,
    AutoEnable,
    AutoMode,
    AutoRate,
}

impl Exp {
    pub fn is_slider(self) -> bool {
        matches!(self, Exp::TraceWidth | Exp::TraceScale | Exp::TraceFade | Exp::ZoomFactor | Exp::ZoomSize)
    }
}

/// Identity of an interactive row. Focus, hover and press all refer to rows by
/// this, so they survive the layout reflowing underneath them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Header(Section),
    Editing,
    AdsVia,
    TriggerKey,
    CopyHipToAds,
    ResetProfile,
    SelectLayer(usize),
    AddLayer,
    DupLayer,
    DelLayer,
    UpLayer,
    DownLayer,
    LayerType,
    LayerColor,
    LayerVisible,
    Slider(Param),
    Preset(usize),
    Outline,
    Opacity,
    CopyExport,
    Group(Group),
    Exp(Exp),
    Disp(Disp),
    Lx(Lx),
    State(State, Sx),
    Morph,
    CopyBoth,
    PasteCode,
    AutoUpdate,
    CheckNow,
    RestartNow,
    ForceQuit,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ItemKind {
    Header { title: String, open: bool },
    Cycle { label: &'static str, value: String, swatch: Option<u32> },
    Pick { text: String, active: bool },
    Slider { label: &'static str, min: i32, max: i32, step: i32, value: i32, text: String },
    Link { text: String },
    Export { lines: Vec<String> },
    Preview,
    /// `[x] Label` / `[ ] Label`
    Check { label: &'static str, on: bool },
    /// A collapsible sub-group header inside the Experimental section
    GroupHead { title: String, open: bool },
    /// The red ForceQuit button
    Danger { text: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub action: Option<Action>,
    pub kind: ItemKind,
    pub rect: Rect,
}

impl Item {
    /// The part of the row that reacts to the mouse (a slider only reacts over its track).
    pub fn hit_rect(&self) -> Rect {
        match self.kind {
            ItemKind::Slider { .. } => Rect { x: self.track_x(), y: self.rect.y, w: SLIDER_W, h: self.rect.h },
            _ => self.rect,
        }
    }

    /// Left edge of a slider row's track: the value column is fixed at the right,
    /// so rows inside a sub-group (narrower) give up label width, not track width.
    pub fn track_x(&self) -> i32 {
        self.rect.x + self.rect.w - VALUE_W - SLIDER_GAP - SLIDER_W
    }
}

#[derive(Clone, Debug)]
pub struct Layout {
    pub items: Vec<Item>,
    /// (x, start, end) of each open section or group body, for its hairline rule
    pub bodies: Vec<(i32, i32, i32)>,
    /// full window height if nothing scrolled
    pub height: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Home,
    End,
    Left,
    Right,
    PageUp,
    PageDown,
    Enter,
    Space,
    Esc,
}

/// What a call changed, for the caller to act on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Effects {
    pub repaint: bool,
    /// the crosshair itself (or which profile is shown) changed
    pub crosshair: bool,
    /// the config should be written to disk
    pub persist: bool,
    /// the ADS trigger changed: re-register hotkeys and mouse input
    pub trigger: bool,
    pub clipboard: Option<String>,
    pub hide: bool,
    /// an Experimental setting changed: re-apply lens, zoom, trace, dynamics, capture
    pub experimental: bool,
    /// a Display setting changed: re-resolve the monitor, scales and frame cap
    pub display: bool,
    /// a state rule changed: restart the key tracking and redraw if one is active
    pub states: bool,
    /// ask the overlay for the clipboard text and give it to `Menu::pasted`
    pub paste: Option<Paste>,
    /// play the scripted spray through the tracer (the Test trace link)
    pub trace_demo: bool,
    /// Auto-update was switched: start or stop the 30 minute checks
    pub update_cfg: bool,
    /// check for an update now
    pub update_check: bool,
    /// restart into the newly installed version
    pub restart: bool,
    /// ForceQuit: close the program, at once
    pub quit: bool,
}

impl Effects {
    fn merge(mut self, o: Effects) -> Effects {
        self.repaint |= o.repaint;
        self.crosshair |= o.crosshair;
        self.persist |= o.persist;
        self.trigger |= o.trigger;
        self.hide |= o.hide;
        self.experimental |= o.experimental;
        self.display |= o.display;
        self.states |= o.states;
        self.trace_demo |= o.trace_demo;
        self.update_cfg |= o.update_cfg;
        self.update_check |= o.update_check;
        self.restart |= o.restart;
        self.quit |= o.quit;
        if o.clipboard.is_some() {
            self.clipboard = o.clipboard;
        }
        if o.paste.is_some() {
            self.paste = o.paste;
        }
        self
    }
}

#[derive(Clone, Copy, Debug)]
struct Drag {
    action: Action,
    origin_x: i32,
    min: i32,
    max: i32,
    step: i32,
}

pub struct Menu {
    open: [bool; 10],
    /// what the updater is doing, shown under the title
    update: Status,
    groups: [bool; 10],
    /// the hue/saturation/value the color sliders last set, so a gray or black color keeps its hue
    hsv_mem: Option<(u32, (f32, f32, f32))>,
    /// what the Auto choices resolved to on this machine, shown beside "Auto"
    info: Info,
    pub editing: Profile,
    sel: [usize; 2],
    scroll: i32,
    max_h: i32,
    metrics: Metrics,
    hover: Option<Action>,
    focus: Option<Action>,
    focus_visible: bool,
    pressed: Option<(Action, bool)>,
    drag: Option<Drag>,
}

fn cycle<T: Copy + PartialEq>(list: &[T], cur: T, dir: i32) -> T {
    let n = list.len() as i32;
    let i = list.iter().position(|x| *x == cur).map(|i| i as i32).unwrap_or(if dir < 0 { 0 } else { -1 });
    list[(i + dir).rem_euclid(n) as usize]
}

fn kind_name(k: Kind) -> &'static str {
    k.name()
}

fn color_name(rgb: u32) -> String {
    PALETTE.iter().find(|(_, c)| *c == rgb).map(|(n, _)| n.to_string()).unwrap_or_else(|| format!("#{rgb:06x}"))
}

fn trigger_name(t: Trigger) -> &'static str {
    match t {
        Trigger::HoldRmb => "Hold RMB",
        Trigger::ToggleRmb => "Toggle RMB",
        Trigger::Key => "Key",
    }
}

/// (param, label, min, max, step) for the sliders a layer type shows, in order.
pub fn slider_defs(l: &Layer) -> Vec<(Param, &'static str, i32, i32, i32)> {
    match l.kind {
        Kind::Spokes => vec![
            (Param::Reach, "Length", 1, 15i32.min(16 - l.gap), 1),
            (Param::Gap, "Gap", 0, 10i32.min(16 - l.reach), 1),
            (Param::Thick, "Width", 1, 7, 2),
            (Param::Count, "Spokes", 1, 12, 1),
            (Param::Rot, "Rotate", 0, 345, 15),
        ],
        Kind::Ring => vec![(Param::Reach, "Radius", 2, 13, 1), (Param::Thick, "Width", 1, 7, 2), (Param::Count, "Dashes", 1, 16, 1), (Param::Rot, "Rotate", 0, 345, 15)],
        Kind::Diamond => vec![(Param::Reach, "Radius", 2, 13, 1), (Param::Thick, "Width", 1, 7, 2)],
        Kind::Bracket => vec![(Param::Reach, "Radius", 2, 13, 1), (Param::Gap, "Arm", 1, 10, 1), (Param::Thick, "Width", 1, 7, 2)],
        Kind::Dot => vec![(Param::Reach, "Size", 1, 9, 2)],
        Kind::Chevron => vec![
            (Param::Reach, "Arm", 1, l.reach_max(), 1),
            (Param::Gap, "Tip", 0, 10, 1),
            (Param::Thick, "Width", 1, 7, 2),
            (Param::Count, "Stack", 1, l.count_max(), 1),
            (Param::Rot, "Point", 0, 345, 15),
        ],
        Kind::Polygon => vec![
            (Param::Reach, "Radius", 2, 13, 1),
            (Param::Gap, "Star", 0, l.reach - 1, 1),
            (Param::Thick, "Width", 1, 7, 2),
            (Param::Count, "Sides", 3, 12, 1),
            (Param::Rot, "Rotate", 0, 345, 15),
        ],
        Kind::Arc => vec![(Param::Reach, "Radius", 2, 13, 1), (Param::Thick, "Width", 1, 7, 2), (Param::Count, "Span", 1, 24, 1), (Param::Rot, "Aim", 0, 345, 15)],
        Kind::Ladder => vec![
            (Param::Reach, "Length", 1, l.reach_max(), 1),
            (Param::Gap, "Step", (l.thick + 1).max(2), 8.max((l.thick + 1).max(2)), 1),
            (Param::Thick, "Width", 1, 7, 2),
            (Param::Count, "Ticks", 1, l.count_max(), 1),
            (Param::Rot, "Aim", 0, 345, 15),
        ],
        Kind::Line => vec![(Param::Reach, "Length", 1, l.reach_max(), 1), (Param::Thick, "Width", 1, 7, 2), (Param::Rot, "Rotate", 0, 345, 15)],
        Kind::Disc => vec![(Param::Reach, "Radius", 1, 14, 1)],
    }
}

fn param_get(l: &Layer, p: Param) -> i32 {
    match p {
        Param::Reach => l.reach,
        Param::Gap => l.gap,
        Param::Thick => l.thick,
        Param::Count => l.count,
        Param::Rot => l.rot,
    }
}

fn param_set(l: &mut Layer, p: Param, v: i32) {
    match p {
        Param::Reach => l.reach = v,
        Param::Gap => l.gap = v,
        Param::Thick => l.thick = v,
        Param::Count => l.count = v,
        Param::Rot => l.rot = v,
    }
}

/// Largest value the slider can actually take: `min + k*step <= max`.
fn aligned_max(min: i32, max: i32, step: i32) -> i32 {
    min + ((max - min) / step) * step
}

/// Value under a click `x_rel` pixels into the slider track (thumb centered on the pointer).
pub fn slider_value_at(x_rel: i32, min: i32, max: i32, step: i32) -> i32 {
    let top = aligned_max(min, max, step);
    if top <= min {
        return min;
    }
    let span = (SLIDER_W - THUMB_W) as f64;
    let t = ((x_rel - THUMB_W / 2).clamp(0, span as i32)) as f64 / span;
    let steps = (t * (top - min) as f64 / step as f64).round() as i32;
    (min + steps * step).clamp(min, top)
}

/// Thumb x (relative to the track) for a value, per DESIGN.md.
pub fn thumb_x(value: i32, min: i32, max: i32) -> i32 {
    if max <= min {
        return 0;
    }
    (((value - min) as f64 / (max - min) as f64) * (SLIDER_W - THUMB_W) as f64).round() as i32
}

fn default_profile(p: Profile) -> Crosshair {
    match p {
        Profile::Hip => Crosshair::classic(),
        Profile::Ads => Crosshair::ads_default(),
    }
}

struct Builder {
    items: Vec<Item>,
    bodies: Vec<(i32, i32, i32)>,
    y: i32,
    body_start: i32,
    group_start: i32,
    indent: i32,
}

fn check(b: &mut Builder, id: Exp, label: &'static str, on: bool) {
    b.row(Some(Action::Exp(id)), ItemKind::Check { label, on }, ROW_H);
}

fn pick_row(b: &mut Builder, id: Exp, label: &'static str, value: &str, swatch: Option<u32>) {
    b.row(Some(Action::Exp(id)), ItemKind::Cycle { label, value: value.to_string(), swatch }, ROW_H);
}

fn slide(b: &mut Builder, id: Exp, label: &'static str, range: (i32, i32, i32), value: i32, text: String) {
    b.row(Some(Action::Exp(id)), ItemKind::Slider { label, min: range.0, max: range.1, step: range.2, value, text }, ROW_H);
}

/// `+3`, `-2`, and a plain `0`.
fn signed(v: i32) -> String {
    if v == 0 {
        "0".to_string()
    } else {
        format!("{v:+}")
    }
}

fn lslide(b: &mut Builder, id: Lx, label: &'static str, range: (i32, i32, i32), value: i32, text: String) {
    b.row(Some(Action::Lx(id)), ItemKind::Slider { label, min: range.0, max: range.1, step: range.2, value, text }, ROW_H);
}

fn sslide(b: &mut Builder, s: State, id: Sx, label: &'static str, range: (i32, i32), value: i32) {
    b.row(Some(Action::State(s, id)), ItemKind::Slider { label, min: range.0, max: range.1, step: 1, value, text: signed(value) }, ROW_H);
}

const LEVELS: [&str; 4] = ["Off", "Low", "Medium", "High"];
const IDLE_NAMES: [&str; 4] = ["Off", "3s", "6s", "12s"];

impl Builder {
    fn header(&mut self, section: Section, title: String, open: bool) {
        self.y += SECTION_GAP;
        self.items.push(Item {
            action: Some(Action::Header(section)),
            kind: ItemKind::Header { title, open },
            rect: Rect { x: CONTENT_X, y: self.y, w: CONTENT_W, h: ROW_H },
        });
        self.y += ROW_H;
        self.body_start = self.y;
    }

    fn end_body(&mut self) {
        self.bodies.push((RULE_X, self.body_start, self.y));
    }

    /// A sub-group header; while it is open its rows are indented by `GROUP_INDENT`.
    fn group_head(&mut self, group: Group, title: String, open: bool) {
        self.items.push(Item { action: Some(Action::Group(group)), kind: ItemKind::GroupHead { title, open }, rect: Rect { x: ROW_X, y: self.y, w: ROW_W, h: ROW_H } });
        self.y += ROW_H;
        self.group_start = self.y;
        if open {
            self.indent = GROUP_INDENT;
        }
    }

    fn end_group(&mut self) {
        self.bodies.push((ROW_X + 3, self.group_start, self.y));
        self.indent = 0;
    }

    fn row(&mut self, action: Option<Action>, kind: ItemKind, h: i32) {
        self.items.push(Item { action, kind, rect: Rect { x: ROW_X + self.indent, y: self.y, w: ROW_W - self.indent, h } });
        self.y += h;
    }

    fn links(&mut self, links: &[(&str, Action)], cell: i32) {
        let mut x = 0;
        for (text, action) in links {
            let w = text.chars().count() as i32 * cell;
            if x > 0 && x + w > ROW_W - self.indent {
                x = 0;
                self.y += ROW_H;
            }
            self.items.push(Item { action: Some(*action), kind: ItemKind::Link { text: text.to_string() }, rect: Rect { x: ROW_X + self.indent + x, y: self.y, w, h: ROW_H } });
            x += w + LINK_GAP;
        }
        self.y += ROW_H;
    }
}

impl Menu {
    pub fn new() -> Menu {
        Menu {
            open: [true, true, true, false, false, false, false, false, false, false],
            update: Status::Checking,
            groups: [false; 10],
            hsv_mem: None,
            info: Info::default(),
            editing: Profile::Hip,
            sel: [0, 0],
            scroll: 0,
            max_h: 600,
            metrics: Metrics { body_cell: 6, small_cell: 5 },
            hover: None,
            focus: None,
            focus_visible: false,
            pressed: None,
            drag: None,
        }
    }

    /// Tell the menu what the updater is doing. Returns true if the title area needs redrawing.
    pub fn set_update(&mut self, status: Status) -> bool {
        let changed = self.update != status;
        self.update = status;
        changed
    }

    pub fn update_status(&self) -> &Status {
        &self.update
    }

    /// Tell the menu what Auto resolved to. Returns true if the rows need redrawing.
    pub fn set_info(&mut self, info: Info) -> bool {
        let changed = self.info != info;
        self.info = info;
        changed
    }

    pub fn set_view(&mut self, max_h: i32, metrics: Metrics) {
        self.max_h = max_h.max(60);
        self.metrics = metrics;
    }

    pub fn scroll(&self) -> i32 {
        self.scroll
    }

    pub fn hover(&self) -> Option<Action> {
        self.hover
    }

    /// The row to ring with the focus outline, only after keyboard input.
    pub fn focus_ring(&self) -> Option<Action> {
        if self.focus_visible {
            self.focus
        } else {
            None
        }
    }

    /// Exact hue (degrees), saturation and value (0..=1) of a color: what the sliders last set if it
    /// still maps to this color, so dragging Value to 0 and back does not forget the hue, and touching
    /// one slider moves only that part of the color instead of snapping the rest to steps of 5.
    fn hsv_exact(&self, rgb: u32) -> (f32, f32, f32) {
        match self.hsv_mem {
            Some((c, hsv)) if c == rgb => hsv,
            _ => color::rgb_to_hsv(rgb),
        }
    }

    /// Where the Hue, Sat and Value sliders sit for a color.
    fn hsv_of(&self, rgb: u32) -> (i32, i32, i32) {
        let (h, s, v) = self.hsv_exact(rgb);
        color::slider_steps(h, s, v)
    }

    pub fn selected_layer(&self, cfg: &Config) -> usize {
        self.sel[self.editing as usize].min(cfg.profile(self.editing).layers.len() - 1)
    }

    pub fn window_height(&self, cfg: &Config) -> i32 {
        self.layout(cfg).height.min(self.max_h)
    }

    pub fn clamp_scroll(&mut self, content_h: i32) {
        let view = content_h.min(self.max_h);
        self.scroll = self.scroll.clamp(0, (content_h - view).max(0));
    }

    pub fn layout(&self, cfg: &Config) -> Layout {
        let p = self.editing;
        let ch = cfg.profile(p);
        let sel = self.selected_layer(cfg);
        let layer = &ch.layers[sel];
        let small = self.metrics.small_cell.max(1);
        let cell = self.metrics.body_cell;
        let mut b = Builder { items: Vec::new(), bodies: Vec::new(), y: BORDER + PAD_TOP + TITLE_H + UPDATE_H + CREDIT_H, body_start: 0, group_start: 0, indent: 0 };
        let open = |s: Section| self.open[s as usize];

        let profile_name = if p == Profile::Hip { "Hip fire" } else { "ADS" };
        b.header(Section::Profile, format!("Profile · {profile_name}"), open(Section::Profile));
        if open(Section::Profile) {
            b.row(Some(Action::Editing), ItemKind::Cycle { label: "Editing:", value: profile_name.to_string(), swatch: None }, ROW_H);
            b.row(Some(Action::AdsVia), ItemKind::Cycle { label: "ADS via:", value: trigger_name(cfg.trigger).to_string(), swatch: None }, ROW_H);
            if cfg.trigger == Trigger::Key {
                b.row(Some(Action::TriggerKey), ItemKind::Cycle { label: "Key:", value: key_name(cfg.key), swatch: None }, ROW_H);
            }
            b.row(Some(Action::Morph), ItemKind::Cycle { label: "Morph:", value: morph::name(cfg.morph).to_string(), swatch: None }, ROW_H);
            b.links(&[("Copy hip > ads", Action::CopyHipToAds), ("Reset", Action::ResetProfile)], cell);
            b.end_body();
        }

        b.header(Section::Layers, "Layers".to_string(), open(Section::Layers));
        if open(Section::Layers) {
            for (i, l) in ch.layers.iter().enumerate() {
                let text = format!("{}{} {:02} {}", if i == sel { "> " } else { "  " }, if l.visible { "x" } else { "-" }, i + 1, kind_name(l.kind));
                b.row(Some(Action::SelectLayer(i)), ItemKind::Pick { text, active: i == sel }, ROW_H);
            }
            b.links(&[("Add", Action::AddLayer), ("Dup", Action::DupLayer), ("Del", Action::DelLayer), ("Up", Action::UpLayer), ("Down", Action::DownLayer)], cell);
            b.end_body();
        }

        b.header(Section::Layer, format!("Layer {:02}", sel + 1), open(Section::Layer));
        if open(Section::Layer) {
            b.row(Some(Action::LayerType), ItemKind::Cycle { label: "Type:", value: kind_name(layer.kind).to_string(), swatch: None }, ROW_H);
            b.row(Some(Action::LayerColor), ItemKind::Cycle { label: "Color:", value: color_name(layer.color), swatch: Some(layer.color) }, ROW_H);
            for (param, label, min, max, step) in slider_defs(layer) {
                let value = param_get(layer, param);
                let text = match (param, layer.kind) {
                    (Param::Rot, _) => format!("{value}°"),
                    (Param::Count, Kind::Arc) => format!("{}°", value * 15),
                    _ => value.to_string(),
                };
                b.row(Some(Action::Slider(param)), ItemKind::Slider { label, min, max, step, value, text }, ROW_H);
            }
            b.row(Some(Action::LayerVisible), ItemKind::Cycle { label: "Visible:", value: if layer.visible { "On" } else { "Off" }.to_string(), swatch: None }, ROW_H);

            let grp = |g: Group| self.groups[g as usize];
            b.group_head(Group::Color, "Custom color".to_string(), grp(Group::Color));
            if grp(Group::Color) {
                let (h, s, v) = self.hsv_of(layer.color);
                lslide(&mut b, Lx::Hue, "Hue", (0, 355, 5), h, format!("{h}°"));
                lslide(&mut b, Lx::Sat, "Sat", (0, 100, 5), s, format!("{s}%"));
                lslide(&mut b, Lx::Val, "Value", (0, 100, 5), v, format!("{v}%"));
                b.row(Some(Action::Lx(Lx::Cycle)), ItemKind::Cycle { label: "Cycle:", value: color::CYCLE_NAMES[layer.cycle.min(3) as usize].to_string(), swatch: None }, ROW_H);
                b.links(&[("Copy hex", Action::Lx(Lx::CopyHex)), ("Paste hex", Action::Lx(Lx::PasteHex))], cell);
                b.end_group();
            }
            b.group_head(Group::Transform, "Transform".to_string(), grp(Group::Transform));
            if grp(Group::Transform) {
                let o = layer.opacity as i32;
                lslide(&mut b, Lx::Opacity, "Opacity", (1, 10, 1), o, format!("{}%", o * 10));
                lslide(&mut b, Lx::OffX, "Shift X", (-12, 12, 1), layer.ox, signed(layer.ox));
                lslide(&mut b, Lx::OffY, "Shift Y", (-12, 12, 1), layer.oy, signed(layer.oy));
                b.row(Some(Action::Lx(Lx::Mirror)), ItemKind::Cycle { label: "Mirror:", value: layer.mirror.name().to_string(), swatch: None }, ROW_H);
                b.end_group();
            }
            b.end_body();
        }

        b.header(Section::Presets, "Presets".to_string(), open(Section::Presets));
        if open(Section::Presets) {
            for (i, (name, _)) in presets().iter().enumerate() {
                b.row(Some(Action::Preset(i)), ItemKind::Pick { text: format!("  {name}"), active: false }, ROW_H);
            }
            b.end_body();
        }

        b.header(Section::States, "States".to_string(), open(Section::States));
        if open(Section::States) {
            for st in [State::Crouch, State::Sprint, State::Jump] {
                let r = cfg.states.rule(st);
                let g = Group::of(st);
                let title = if r.key == 0 { st.name().to_string() } else { format!("{} · {}", st.name(), states::key_name(r.key)) };
                b.group_head(g, title, self.groups[g as usize]);
                if self.groups[g as usize] {
                    b.row(Some(Action::State(st, Sx::Key)), ItemKind::Cycle { label: "Key:", value: states::key_name(r.key), swatch: None }, ROW_H);
                    if r.key != 0 {
                        b.row(Some(Action::State(st, Sx::Mode)), ItemKind::Cycle { label: "Mode:", value: if r.toggle { "Toggle" } else { "Hold" }.to_string(), swatch: None }, ROW_H);
                        sslide(&mut b, st, Sx::Gap, "Gap", states::GAP_RANGE, r.gap);
                        sslide(&mut b, st, Sx::Size, "Size", states::SIZE_RANGE, r.size);
                        let (value, swatch) = match r.color {
                            Some(c) => (color_name(c), Some(c)),
                            None => ("Same".to_string(), None),
                        };
                        b.row(Some(Action::State(st, Sx::Color)), ItemKind::Cycle { label: "Color:", value, swatch }, ROW_H);
                    }
                    b.end_group();
                }
            }
            b.end_body();
        }

        b.header(Section::Output, "Output".to_string(), open(Section::Output));
        if open(Section::Output) {
            let outline = ["Off", "1px", "2px"][ch.outline.min(2) as usize];
            b.row(Some(Action::Outline), ItemKind::Cycle { label: "Outline:", value: outline.to_string(), swatch: None }, ROW_H);
            let v = ch.opacity as i32;
            b.row(Some(Action::Opacity), ItemKind::Slider { label: "Opacity", min: 1, max: 10, step: 1, value: v, text: format!("{}%", v * 10) }, ROW_H);
            let chars_per_line = (ROW_W / small).max(1) as usize;
            let export: Vec<char> = share::encode(ch).chars().collect();
            let lines: Vec<String> = export.chunks(chars_per_line).map(|c| c.iter().collect()).collect();
            let h = lines.len() as i32 * EXPORT_LINE_H + 4;
            b.row(Some(Action::CopyExport), ItemKind::Export { lines }, h);
            b.links(&[("Copy both", Action::CopyBoth), ("Paste code", Action::PasteCode)], cell);
            b.end_body();
        }

        b.header(Section::Display, "Display".to_string(), open(Section::Display));
        if open(Section::Display) {
            let d = &cfg.display;
            let x = |k: i32| format!("{k}x");
            let monitor = match d.monitor {
                MonitorSel::Primary => "Primary".to_string(),
                MonitorSel::Follow => "Follow window".to_string(),
                MonitorSel::Nth(n) => n.to_string(),
            };
            let disp = |b: &mut Builder, id: Disp, label: &'static str, value: String| b.row(Some(Action::Disp(id)), ItemKind::Cycle { label, value, swatch: None }, ROW_H);
            disp(&mut b, Disp::Monitor, "Monitor:", monitor);
            disp(&mut b, Disp::Scale, "Scale:", d.scale.map_or(format!("Auto ({})", x(self.info.scale)), x));
            disp(&mut b, Disp::UiScale, "UI scale:", d.ui_scale.map_or(format!("Auto ({})", x(self.info.ui_scale)), x));
            disp(&mut b, Disp::Fps, "Frame rate:", d.fps.map_or(format!("Auto ({} Hz)", self.info.refresh), |f| format!("{f} Hz")));
            b.end_body();
        }

        b.header(Section::Updates, "Updates".to_string(), open(Section::Updates));
        if open(Section::Updates) {
            b.row(Some(Action::AutoUpdate), ItemKind::Cycle { label: "Auto-update:", value: if cfg.autoupdate { "On" } else { "Off" }.to_string(), swatch: None }, ROW_H);
            b.row(None, ItemKind::Cycle { label: "Version:", value: update::VERSION.to_string(), swatch: None }, ROW_H);
            let mut links = vec![("Check now", Action::CheckNow)];
            if matches!(self.update, Status::Installed(_)) {
                links.push(("Restart now", Action::RestartNow));
            }
            b.links(&links, cell);
            b.end_body();
        }

        b.header(Section::Preview, "Preview 5x".to_string(), open(Section::Preview));
        if open(Section::Preview) {
            b.y += 2;
            b.items.push(Item { action: None, kind: ItemKind::Preview, rect: Rect { x: ROW_X, y: b.y, w: PREVIEW, h: PREVIEW } });
            b.y += PREVIEW + 2;
            b.end_body();
        }

        b.header(Section::Experimental, "Experimental [!]".to_string(), open(Section::Experimental));
        if open(Section::Experimental) {
            let e = &cfg.exp;
            let (d, t, z, a) = (&e.dynamics, &e.trace, &e.zoom, &e.auto);
            let grp = |g: Group| self.groups[g as usize];
            check(&mut b, Exp::CaptureShow, "Show on screen capture", e.show_on_capture);

            b.group_head(Group::Blend, format!("Blend · Layer {:02}", sel + 1), grp(Group::Blend));
            if grp(Group::Blend) {
                pick_row(&mut b, Exp::Blend, "Blend:", layer.blend.name(), None);
                b.end_group();
            }

            b.group_head(Group::Dynamics, "Input Dynamics".to_string(), grp(Group::Dynamics));
            if grp(Group::Dynamics) {
                check(&mut b, Exp::DynEnable, "Enabled", d.enabled);
                if d.enabled {
                    pick_row(&mut b, Exp::DynBloom, "Bloom:", LEVELS[d.bloom.min(3) as usize], None);
                    pick_row(&mut b, Exp::DynSpread, "Move:", LEVELS[d.spread.min(3) as usize], None);
                    pick_row(&mut b, Exp::DynIdle, "Idle:", IDLE_NAMES[d.idle.min(3) as usize], None);
                    pick_row(&mut b, Exp::DynMotion, "Motion:", ["None", "Pulse", "Spin"][d.motion as usize], None);
                }
                b.end_group();
            }

            b.group_head(Group::Trace, "Spray trace".to_string(), grp(Group::Trace));
            if grp(Group::Trace) {
                check(&mut b, Exp::TraceEnable, "Enabled", t.enabled);
                if t.enabled {
                    pick_row(&mut b, Exp::TraceActive, "Active:", ["Fire", "Always"][t.active as usize], None);
                    pick_row(&mut b, Exp::TraceStyle, "Style:", ["Trail", "Line", "Both"][t.style as usize], None);
                    pick_row(&mut b, Exp::TraceReturn, "Return:", ["Fade", "Snap", "Hold"][t.ret as usize], None);
                    pick_row(&mut b, Exp::TraceColor, "Color:", &color_name(t.color), Some(t.color));
                    check(&mut b, Exp::TraceSpeedColor, "Color by speed", t.speed_color);
                    slide(&mut b, Exp::TraceWidth, "Width", (1, 3, 1), t.width, format!("{}px", t.width));
                    slide(&mut b, Exp::TraceScale, "Scale", (1, 10, 1), t.scale, format!("{:.1}", t.scale as f32 / 10.0));
                    slide(&mut b, Exp::TraceFade, "Fade", (3, 30, 1), t.fade, format!("{:.1}s", t.fade as f32 / 10.0));
                    pick_row(&mut b, Exp::TraceRange, "Range:", &format!("{}px", t.range), None);
                    pick_row(&mut b, Exp::TraceSmooth, "Smooth:", LEVELS[t.smooth.min(3) as usize], None);
                    b.links(&[("Test trace", Action::Exp(Exp::TraceTest))], cell);
                }
                b.end_group();
            }

            b.group_head(Group::Zoom, "Scope zoom".to_string(), grp(Group::Zoom));
            if grp(Group::Zoom) {
                check(&mut b, Exp::ZoomEnable, "Enabled", z.enabled);
                if z.enabled {
                    slide(&mut b, Exp::ZoomFactor, "Zoom", (15, 40, 5), z.factor, format!("{:.1}x", z.factor as f32 / 10.0));
                    slide(&mut b, Exp::ZoomSize, "Size", (100, 400, 20), z.size, z.size.to_string());
                    pick_row(&mut b, Exp::ZoomShape, "Shape:", if z.circle { "Circle" } else { "Square" }, None);
                    pick_row(&mut b, Exp::ZoomWhen, "Active:", if z.when == ZoomWhen::Ads { "ADS" } else { "Always" }, None);
                }
                b.end_group();
            }

            b.group_head(Group::Auto, "Auto contrast".to_string(), grp(Group::Auto));
            if grp(Group::Auto) {
                check(&mut b, Exp::AutoEnable, "Enabled", a.enabled);
                if a.enabled {
                    pick_row(&mut b, Exp::AutoMode, "Mode:", if a.mode == AutoMode::LightDark { "Light/dark" } else { "Palette" }, None);
                    pick_row(&mut b, Exp::AutoRate, "Rate:", ["", "Slow", "Medium", "Fast"][a.rate.clamp(1, 3) as usize], None);
                }
                b.end_group();
            }
            b.end_body();
        }

        // The last row of the menu: closes the program, whatever else is going on.
        b.y += SECTION_GAP;
        b.row(Some(Action::ForceQuit), ItemKind::Danger { text: "ForceQuit".to_string() }, ROW_H + 2);

        let height = b.y + PAD_BOTTOM + BORDER;
        Layout { items: b.items, bodies: b.bodies, height }
    }

    fn hit(&self, layout: &Layout, x: i32, y: i32) -> Option<usize> {
        let cy = y + self.scroll;
        layout.items.iter().position(|i| i.action.is_some() && i.hit_rect().contains(x, cy))
    }

    fn mark(fx: &mut Effects) {
        fx.crosshair = true;
        fx.persist = true;
    }

    fn set_slider(&mut self, cfg: &mut Config, action: Action, value: i32) -> bool {
        let p = self.editing;
        let sel = self.selected_layer(cfg);
        let ch = cfg.profile_mut(p);
        match action {
            Action::Slider(param) => {
                let l = &mut ch.layers[sel];
                let before = l.clone();
                param_set(l, param, value);
                l.clamp();
                *l != before
            }
            Action::Opacity => {
                let v = value.clamp(1, 10) as u8;
                let changed = ch.opacity != v;
                ch.opacity = v;
                changed
            }
            Action::Lx(x) => {
                let current = self.hsv_exact(ch.layers[sel].color);
                let l = &mut ch.layers[sel];
                let before = l.clone();
                match x {
                    Lx::Opacity => l.opacity = value.clamp(1, 10) as u8,
                    Lx::OffX => l.ox = value,
                    Lx::OffY => l.oy = value,
                    Lx::Hue | Lx::Sat | Lx::Val => {
                        let (mut h, mut s, mut v) = current;
                        match x {
                            Lx::Hue => h = value.clamp(0, 355) as f32,
                            Lx::Sat => s = value.clamp(0, 100) as f32 / 100.0,
                            _ => v = value.clamp(0, 100) as f32 / 100.0,
                        }
                        l.color = color::hsv_to_rgb(h, s, v);
                        self.hsv_mem = Some((l.color, (h, s, v)));
                    }
                    _ => {}
                }
                l.clamp();
                *l != before
            }
            Action::State(st, Sx::Gap) => {
                let r = cfg.states.rule_mut(st);
                let v = value.clamp(states::GAP_RANGE.0, states::GAP_RANGE.1);
                let changed = r.gap != v;
                r.gap = v;
                changed
            }
            Action::State(st, Sx::Size) => {
                let r = cfg.states.rule_mut(st);
                let v = value.clamp(states::SIZE_RANGE.0, states::SIZE_RANGE.1);
                let changed = r.size != v;
                r.size = v;
                changed
            }
            Action::Exp(id) => {
                let e = &mut cfg.exp;
                let before = e.clone();
                match id {
                    Exp::TraceWidth => e.trace.width = value.clamp(1, 3),
                    Exp::TraceScale => e.trace.scale = value.clamp(1, 10),
                    Exp::TraceFade => e.trace.fade = value.clamp(3, 30),
                    Exp::ZoomFactor => e.zoom.factor = (value.clamp(15, 40) + 2) / 5 * 5,
                    Exp::ZoomSize => e.zoom.size = (value.clamp(100, 400) + 10) / 20 * 20,
                    _ => {}
                }
                *e != before
            }
            _ => false,
        }
    }

    /// The overlay read the clipboard for a `Paste` request; apply what it held. Text that is not
    /// a color or a share code changes nothing.
    pub fn pasted(&mut self, cfg: &mut Config, kind: Paste, text: &str) -> Effects {
        let p = self.editing;
        let sel = self.selected_layer(cfg);
        let mut fx = Effects { repaint: true, ..Effects::default() };
        match kind {
            Paste::Hex => {
                if let Some(rgb) = color::parse_hex(text) {
                    cfg.profile_mut(p).layers[sel].color = rgb;
                    self.hsv_mem = None;
                    Self::mark(&mut fx);
                }
            }
            Paste::Code => {
                if let Some(shared) = share::decode(text, cfg.profile(p)) {
                    if let Some(c) = shared.single {
                        *cfg.profile_mut(p) = c;
                        self.sel[p as usize] = 0;
                    }
                    if let Some(c) = shared.hip {
                        cfg.hip = c;
                        self.sel[Profile::Hip as usize] = 0;
                    }
                    if let Some(c) = shared.ads {
                        cfg.ads = c;
                        self.sel[Profile::Ads as usize] = 0;
                    }
                    Self::mark(&mut fx);
                }
            }
        }
        fx
    }

    /// Set a slider row to a value, as a drag would (used by the painter's tests).
    #[cfg(test)]
    pub fn set_slider_for_test(&mut self, cfg: &mut Config, action: Action, value: i32) -> bool {
        self.set_slider(cfg, action, value)
    }

    /// The Experimental [!] checkboxes and dropdown-style rows.
    fn exp_action(&mut self, cfg: &mut Config, id: Exp, dir: i32) -> Effects {
        let mut fx = Effects { repaint: true, experimental: true, persist: true, ..Effects::default() };
        if id == Exp::Blend {
            let sel = self.selected_layer(cfg);
            let l = &mut cfg.profile_mut(self.editing).layers[sel];
            l.blend = cycle(&Blend::ALL, l.blend, dir);
            fx.crosshair = true;
            return fx;
        }
        let e = &mut cfg.exp;
        let colors: Vec<u32> = PALETTE.iter().map(|(_, c)| *c).collect();
        match id {
            Exp::CaptureShow => e.show_on_capture = !e.show_on_capture,
            Exp::DynEnable => e.dynamics.enabled = !e.dynamics.enabled,
            Exp::DynBloom => e.dynamics.bloom = cycle(&[0u8, 1, 2, 3], e.dynamics.bloom, dir),
            Exp::DynSpread => e.dynamics.spread = cycle(&[0u8, 1, 2, 3], e.dynamics.spread, dir),
            Exp::DynIdle => e.dynamics.idle = cycle(&[0u8, 1, 2, 3], e.dynamics.idle, dir),
            Exp::DynMotion => e.dynamics.motion = cycle(&[Motion::None, Motion::Pulse, Motion::Spin], e.dynamics.motion, dir),
            Exp::TraceEnable => e.trace.enabled = !e.trace.enabled,
            Exp::TraceActive => e.trace.active = cycle(&[TraceActive::Fire, TraceActive::Always], e.trace.active, dir),
            Exp::TraceStyle => e.trace.style = cycle(&[TraceStyle::Trail, TraceStyle::Line, TraceStyle::Both], e.trace.style, dir),
            Exp::TraceReturn => e.trace.ret = cycle(&[TraceReturn::Fade, TraceReturn::Snap, TraceReturn::Hold], e.trace.ret, dir),
            Exp::TraceColor => e.trace.color = cycle(&colors, e.trace.color, dir),
            Exp::TraceSpeedColor => e.trace.speed_color = !e.trace.speed_color,
            Exp::TraceRange => e.trace.range = cycle(&TRACE_RANGES, e.trace.range, dir),
            Exp::TraceSmooth => e.trace.smooth = cycle(&[0u8, 1, 2, 3], e.trace.smooth, dir),
            Exp::ZoomEnable => e.zoom.enabled = !e.zoom.enabled,
            Exp::ZoomShape => e.zoom.circle = !e.zoom.circle,
            Exp::ZoomWhen => e.zoom.when = cycle(&[ZoomWhen::Ads, ZoomWhen::Always], e.zoom.when, dir),
            Exp::AutoEnable => e.auto.enabled = !e.auto.enabled,
            Exp::AutoMode => e.auto.mode = cycle(&[AutoMode::LightDark, AutoMode::Palette], e.auto.mode, dir),
            Exp::AutoRate => e.auto.rate = cycle(&[1u8, 2, 3], e.auto.rate, dir),
            Exp::Blend | Exp::TraceWidth | Exp::TraceScale | Exp::TraceFade | Exp::ZoomFactor | Exp::ZoomSize => {
                fx.experimental = false;
                fx.persist = false;
            }
            Exp::TraceTest => {
                fx.experimental = false;
                fx.persist = false;
                fx.trace_demo = true;
            }
        }
        fx
    }

    /// Run a row's action. `dir` is +1 for next, -1 for previous (cycle rows).
    pub fn activate(&mut self, cfg: &mut Config, action: Action, dir: i32) -> Effects {
        let mut fx = Effects { repaint: true, ..Effects::default() };
        let p = self.editing;
        let pi = p as usize;
        let sel = self.selected_layer(cfg);
        match action {
            Action::Header(s) => self.open[s as usize] = !self.open[s as usize],
            Action::Editing => {
                self.editing = if p == Profile::Hip { Profile::Ads } else { Profile::Hip };
                fx.crosshair = true;
            }
            Action::AdsVia => {
                cfg.trigger = cycle(&[Trigger::HoldRmb, Trigger::ToggleRmb, Trigger::Key], cfg.trigger, dir);
                fx.trigger = true;
                fx.persist = true;
            }
            Action::TriggerKey => {
                cfg.key = cycle(&TRIGGER_KEYS, cfg.key, dir);
                fx.trigger = true;
                fx.persist = true;
            }
            Action::CopyHipToAds => {
                cfg.ads = cfg.hip.clone();
                self.sel[Profile::Ads as usize] = self.sel[Profile::Hip as usize];
                self.editing = Profile::Ads;
                Self::mark(&mut fx);
            }
            Action::ResetProfile => {
                *cfg.profile_mut(p) = default_profile(p);
                self.sel[pi] = 0;
                Self::mark(&mut fx);
            }
            Action::SelectLayer(i) => {
                if i < cfg.profile(p).layers.len() {
                    self.sel[pi] = i;
                }
            }
            Action::AddLayer => {
                let ch = cfg.profile_mut(p);
                if ch.layers.len() < MAX_LAYERS {
                    ch.layers.push(Layer::new(Kind::Ring, 0x00FF66));
                    self.sel[pi] = ch.layers.len() - 1;
                    Self::mark(&mut fx);
                }
            }
            Action::DupLayer => {
                let ch = cfg.profile_mut(p);
                if ch.layers.len() < MAX_LAYERS {
                    let copy = ch.layers[sel].clone();
                    ch.layers.push(copy);
                    self.sel[pi] = ch.layers.len() - 1;
                    Self::mark(&mut fx);
                }
            }
            Action::DelLayer => {
                let ch = cfg.profile_mut(p);
                if ch.layers.len() > 1 {
                    ch.layers.remove(sel);
                    self.sel[pi] = sel.saturating_sub(1);
                    Self::mark(&mut fx);
                }
            }
            Action::UpLayer => {
                if sel > 0 {
                    cfg.profile_mut(p).layers.swap(sel, sel - 1);
                    self.sel[pi] = sel - 1;
                    Self::mark(&mut fx);
                }
            }
            Action::DownLayer => {
                let ch = cfg.profile_mut(p);
                if sel + 1 < ch.layers.len() {
                    ch.layers.swap(sel, sel + 1);
                    self.sel[pi] = sel + 1;
                    Self::mark(&mut fx);
                }
            }
            Action::LayerType => {
                let l = &mut cfg.profile_mut(p).layers[sel];
                l.kind = cycle(&Kind::ALL, l.kind, dir);
                l.clamp();
                Self::mark(&mut fx);
            }
            Action::LayerColor => {
                let colors: Vec<u32> = PALETTE.iter().map(|(_, c)| *c).collect();
                let l = &mut cfg.profile_mut(p).layers[sel];
                l.color = cycle(&colors, l.color, dir);
                Self::mark(&mut fx);
            }
            Action::LayerVisible => {
                let l = &mut cfg.profile_mut(p).layers[sel];
                l.visible = !l.visible;
                Self::mark(&mut fx);
            }
            Action::Slider(_) | Action::Opacity => {}
            Action::Morph => {
                cfg.morph = cycle(&morph::MS, cfg.morph, dir);
                fx.persist = true;
            }
            Action::CopyBoth => fx.clipboard = Some(share::encode_both(&cfg.hip, &cfg.ads)),
            Action::PasteCode => fx.paste = Some(Paste::Code),
            Action::Lx(x) => {
                let l = &mut cfg.profile_mut(p).layers[sel];
                match x {
                    Lx::Cycle => {
                        l.cycle = cycle(&[0u8, 1, 2, 3], l.cycle, dir);
                        Self::mark(&mut fx);
                    }
                    Lx::Mirror => {
                        l.mirror = cycle(&Mirror::ALL, l.mirror, dir);
                        Self::mark(&mut fx);
                    }
                    Lx::CopyHex => fx.clipboard = Some(format!("#{:06x}", l.color)),
                    Lx::PasteHex => fx.paste = Some(Paste::Hex),
                    // sliders are dragged, not clicked
                    Lx::Hue | Lx::Sat | Lx::Val | Lx::Opacity | Lx::OffX | Lx::OffY => {}
                }
            }
            Action::State(st, sx) => {
                let r = cfg.states.rule_mut(st);
                match sx {
                    Sx::Key => r.key = cycle(&states::KEYS, r.key, dir),
                    Sx::Mode => r.toggle = !r.toggle,
                    Sx::Color => {
                        let mut list: Vec<Option<u32>> = vec![None];
                        list.extend(PALETTE.iter().map(|(_, c)| Some(*c)));
                        r.color = cycle(&list, r.color, dir);
                    }
                    Sx::Gap | Sx::Size => {}
                }
                if !sx.is_slider() {
                    fx.states = true;
                    fx.persist = true;
                }
            }
            Action::Preset(i) => {
                if let Some((_, layers)) = presets().into_iter().nth(i) {
                    cfg.profile_mut(p).layers = layers;
                    self.sel[pi] = 0;
                    Self::mark(&mut fx);
                }
            }
            Action::Outline => {
                let ch = cfg.profile_mut(p);
                ch.outline = cycle(&[0u8, 1, 2], ch.outline.min(2), dir);
                Self::mark(&mut fx);
            }
            Action::CopyExport => fx.clipboard = Some(share::encode(cfg.profile(p))),
            Action::Disp(d) => {
                let c = &mut cfg.display;
                match d {
                    Disp::Monitor => c.monitor = cycle(&[MonitorSel::Primary, MonitorSel::Follow, MonitorSel::Nth(1), MonitorSel::Nth(2), MonitorSel::Nth(3), MonitorSel::Nth(4)], c.monitor, dir),
                    Disp::Scale => c.scale = cycle(&[None, Some(1), Some(2), Some(3), Some(4)], c.scale, dir),
                    Disp::UiScale => c.ui_scale = cycle(&[None, Some(1), Some(2), Some(3)], c.ui_scale, dir),
                    Disp::Fps => {
                        let mut list = vec![None];
                        list.extend(FPS_CAPS.iter().map(|f| Some(*f)));
                        c.fps = cycle(&list, c.fps, dir);
                    }
                }
                fx.display = true;
                fx.persist = true;
            }
            Action::AutoUpdate => {
                cfg.autoupdate = !cfg.autoupdate;
                fx.update_cfg = true;
                fx.persist = true;
            }
            Action::CheckNow => fx.update_check = true,
            Action::RestartNow => fx.restart = true,
            Action::ForceQuit => fx.quit = true,
            Action::Group(g) => self.groups[g as usize] = !self.groups[g as usize],
            Action::Exp(id) => fx = fx.merge(self.exp_action(cfg, id, dir)),
        }
        fx
    }

    pub fn mouse_down(&mut self, cfg: &mut Config, x: i32, y: i32, right: bool) -> Effects {
        self.focus_visible = false;
        let layout = self.layout(cfg);
        let Some(idx) = self.hit(&layout, x, y) else { return Effects::default() };
        let item = &layout.items[idx];
        let Some(action) = item.action else { return Effects::default() };
        self.focus = Some(action);
        self.pressed = Some((action, right));
        let mut fx = Effects { repaint: true, ..Effects::default() };
        if let (false, ItemKind::Slider { min, max, step, .. }) = (right, &item.kind) {
            let drag = Drag { action, origin_x: item.hit_rect().x, min: *min, max: *max, step: *step };
            self.drag = Some(drag);
            fx = fx.merge(self.drag_to(cfg, drag, x));
        }
        fx
    }

    fn drag_to(&mut self, cfg: &mut Config, drag: Drag, x: i32) -> Effects {
        let v = slider_value_at(x - drag.origin_x, drag.min, drag.max, drag.step);
        let mut fx = Effects { repaint: true, ..Effects::default() };
        if self.set_slider(cfg, drag.action, v) {
            match drag.action {
                Action::Exp(_) => fx.experimental = true,
                Action::State(..) => fx.states = true,
                _ => fx.crosshair = true,
            }
        }
        fx
    }

    pub fn mouse_move(&mut self, cfg: &mut Config, x: i32, y: i32, left_down: bool) -> Effects {
        let mut fx = Effects::default();
        if let (Some(drag), true) = (self.drag, left_down) {
            fx = fx.merge(self.drag_to(cfg, drag, x));
        }
        let layout = self.layout(cfg);
        let over = self.hit(&layout, x, y).and_then(|i| layout.items[i].action);
        let over = match over {
            Some(Action::Slider(_)) | Some(Action::Opacity) => None,
            Some(Action::Exp(e)) if e.is_slider() => None,
            Some(Action::Lx(x)) if x.is_slider() => None,
            Some(Action::State(_, x)) if x.is_slider() => None,
            other => other,
        };
        if over != self.hover {
            self.hover = over;
            fx.repaint = true;
        }
        fx
    }

    pub fn mouse_leave(&mut self) -> Effects {
        let had = self.hover.take().is_some();
        Effects { repaint: had, ..Effects::default() }
    }

    pub fn mouse_up(&mut self, cfg: &mut Config, x: i32, y: i32, right: bool) -> Effects {
        let mut fx = Effects::default();
        if !right {
            if let Some(drag) = self.drag.take() {
                fx = fx.merge(self.drag_to(cfg, drag, x));
                fx.persist = true;
                fx.repaint = true;
                self.pressed = None;
                return fx;
            }
        }
        let Some((action, was_right)) = self.pressed.take() else { return fx };
        if was_right != right {
            return fx;
        }
        let layout = self.layout(cfg);
        let Some(idx) = self.hit(&layout, x, y) else { return fx };
        let item = &layout.items[idx];
        if item.action != Some(action) {
            return fx;
        }
        match (&item.kind, right) {
            (ItemKind::Slider { .. }, _) => {}
            (ItemKind::Cycle { .. }, true) => fx = fx.merge(self.activate(cfg, action, -1)),
            (_, true) => {}
            (_, false) => fx = fx.merge(self.activate(cfg, action, 1)),
        }
        fx
    }

    pub fn wheel(&mut self, cfg: &Config, notches: i32) -> Effects {
        let h = self.layout(cfg).height;
        let before = self.scroll;
        self.scroll -= notches * WHEEL_STEP;
        self.clamp_scroll(h);
        Effects { repaint: self.scroll != before, ..Effects::default() }
    }

    fn reveal(&mut self, rect: Rect, content_h: i32) {
        let view = content_h.min(self.max_h);
        if rect.y - self.scroll < BORDER {
            self.scroll = rect.y - BORDER;
        } else if rect.y + rect.h - self.scroll > view - BORDER {
            self.scroll = rect.y + rect.h - (view - BORDER);
        }
        self.clamp_scroll(content_h);
    }

    pub fn key(&mut self, cfg: &mut Config, key: Key) -> Effects {
        if key == Key::Esc {
            return Effects { hide: true, repaint: true, ..Effects::default() };
        }
        self.focus_visible = true;
        let layout = self.layout(cfg);
        let focusable: Vec<usize> = (0..layout.items.len()).filter(|i| layout.items[*i].action.is_some()).collect();
        if focusable.is_empty() {
            return Effects::default();
        }
        let cur = self.focus.and_then(|a| focusable.iter().position(|i| layout.items[*i].action == Some(a)));
        let mut fx = Effects { repaint: true, ..Effects::default() };
        let go = |this: &mut Menu, pos: usize| {
            let item = &layout.items[focusable[pos]];
            this.focus = item.action;
            this.reveal(item.rect, layout.height);
        };
        match key {
            Key::Up => go(self, cur.map(|c| c.saturating_sub(1)).unwrap_or(0)),
            Key::Down => go(self, cur.map(|c| (c + 1).min(focusable.len() - 1)).unwrap_or(0)),
            Key::Home => go(self, 0),
            Key::End => go(self, focusable.len() - 1),
            Key::Left | Key::Right | Key::PageUp | Key::PageDown | Key::Enter | Key::Space => {
                let Some(c) = cur else { return fx };
                let item = layout.items[focusable[c]].clone();
                let Some(action) = item.action else { return fx };
                let dir = if matches!(key, Key::Left | Key::PageUp) { -1 } else { 1 };
                match (&item.kind, key) {
                    (ItemKind::Cycle { .. } | ItemKind::Check { .. }, Key::Left | Key::Right) => fx = fx.merge(self.activate(cfg, action, dir)),
                    (ItemKind::Cycle { .. } | ItemKind::Check { .. }, Key::Enter | Key::Space) => fx = fx.merge(self.activate(cfg, action, 1)),
                    (ItemKind::Slider { min, max, step, value, .. }, Key::Left | Key::Right | Key::PageUp | Key::PageDown) => {
                        let big = matches!(key, Key::PageUp | Key::PageDown);
                        let delta = if big { (((max - min) as f64 / 10.0 / *step as f64).round() as i32).max(1) * step } else { *step };
                        let top = aligned_max(*min, *max, *step);
                        if self.set_slider(cfg, action, (value + dir * delta).clamp(*min, top)) {
                            match action {
                                Action::Exp(_) => {
                                    fx.experimental = true;
                                    fx.persist = true;
                                }
                                Action::State(..) => {
                                    fx.states = true;
                                    fx.persist = true;
                                }
                                _ => Self::mark(&mut fx),
                            }
                        }
                    }
                    (ItemKind::Pick { .. } | ItemKind::Link { .. } | ItemKind::Header { .. } | ItemKind::GroupHead { .. } | ItemKind::Export { .. } | ItemKind::Danger { .. }, Key::Enter | Key::Space) => {
                        fx = fx.merge(self.activate(cfg, action, 1))
                    }
                    _ => {}
                }
            }
            Key::Esc => {}
        }
        fx
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::states::States;

    fn setup() -> (Menu, Config) {
        (Menu::new(), Config::default())
    }

    fn find(layout: &Layout, action: Action) -> Item {
        layout.items.iter().find(|i| i.action == Some(action)).unwrap_or_else(|| panic!("no row for {action:?}")).clone()
    }

    fn click(m: &mut Menu, cfg: &mut Config, action: Action, right: bool) -> Effects {
        let it = find(&m.layout(cfg), action);
        let (x, y) = (it.hit_rect().x + 3, it.hit_rect().y + 3 - m.scroll());
        let a = m.mouse_down(cfg, x, y, right);
        a.merge(m.mouse_up(cfg, x, y, right))
    }

    #[test]
    fn default_layout_matches_the_spec() {
        let (m, cfg) = setup();
        let l = m.layout(&cfg);
        let headers: Vec<_> = l.items.iter().filter_map(|i| if let ItemKind::Header { title, open } = &i.kind { Some((title.clone(), *open)) } else { None }).collect();
        assert_eq!(
            headers,
            vec![("Profile · Hip fire".into(), true), ("Layers".into(), true), ("Layer 01".into(), true), ("Presets".into(), false), ("States".into(), false), ("Output".into(), false), ("Display".into(), false), ("Updates".into(), false), ("Preview 5x".into(), false), ("Experimental [!]".into(), false)]
        );
        let first = &l.items[0];
        assert_eq!(first.rect, Rect { x: CONTENT_X, y: BORDER + PAD_TOP + TITLE_H + UPDATE_H + CREDIT_H + SECTION_GAP, w: CONTENT_W, h: ROW_H });
        let editing = find(&l, Action::Editing);
        assert_eq!((editing.rect.x, editing.rect.w), (ROW_X, ROW_W));
        assert_eq!(l.bodies.len(), 3);
        for it in &l.items {
            assert!(it.rect.x >= CONTENT_X && it.rect.x + it.rect.w <= CONTENT_X + CONTENT_W, "{it:?} inside content width");
        }
    }

    #[test]
    fn rows_are_14px_with_no_gaps_inside_a_body() {
        let (m, cfg) = setup();
        let l = m.layout(&cfg);
        let layer_rows: Vec<&Item> = l.items.iter().filter(|i| matches!(i.action, Some(Action::SelectLayer(_)))).collect();
        assert_eq!(layer_rows.len(), 2);
        assert_eq!(layer_rows[1].rect.y - layer_rows[0].rect.y, ROW_H);
    }

    #[test]
    fn header_toggles_a_section_and_pushes_everything_below() {
        let (mut m, mut cfg) = setup();
        let h0 = m.layout(&cfg).height;
        let fx = click(&mut m, &mut cfg, Action::Header(Section::Presets), false);
        assert!(fx.repaint && !fx.crosshair);
        let l = m.layout(&cfg);
        assert_eq!(l.height - h0, presets().len() as i32 * ROW_H, "one row per preset appears in place");
        click(&mut m, &mut cfg, Action::Header(Section::Presets), false);
        assert_eq!(m.layout(&cfg).height, h0);
    }

    #[test]
    fn editing_switches_profile_and_right_click_goes_back() {
        let (mut m, mut cfg) = setup();
        let fx = click(&mut m, &mut cfg, Action::Editing, false);
        assert_eq!(m.editing, Profile::Ads);
        assert!(fx.crosshair);
        click(&mut m, &mut cfg, Action::Editing, true);
        assert_eq!(m.editing, Profile::Hip, "right click steps backwards (wraps)");
        assert_eq!(cfg.hip, Config::default().hip, "switching never edits anything");
    }

    #[test]
    fn ads_via_cycles_and_asks_for_trigger_refresh() {
        let (mut m, mut cfg) = setup();
        let fx = click(&mut m, &mut cfg, Action::AdsVia, false);
        assert_eq!(cfg.trigger, Trigger::ToggleRmb);
        assert!(fx.trigger && fx.persist);
        assert!(!m.layout(&cfg).items.iter().any(|i| i.action == Some(Action::TriggerKey)));
        click(&mut m, &mut cfg, Action::AdsVia, false);
        assert_eq!(cfg.trigger, Trigger::Key);
        let key_row = find(&m.layout(&cfg), Action::TriggerKey);
        assert!(matches!(&key_row.kind, ItemKind::Cycle { value, .. } if value == "F8"));
        click(&mut m, &mut cfg, Action::TriggerKey, false);
        assert_eq!(cfg.key, 0x7B, "F8 is followed by F12, skipping the overlay's own F9 to F11");
        click(&mut m, &mut cfg, Action::AdsVia, false);
        assert_eq!(cfg.trigger, Trigger::HoldRmb);
    }

    #[test]
    fn layer_list_rules_hold() {
        let (mut m, mut cfg) = setup();
        click(&mut m, &mut cfg, Action::DelLayer, false);
        assert_eq!(cfg.hip.layers.len(), 1);
        assert!(!click(&mut m, &mut cfg, Action::DelLayer, false).crosshair, "last layer cannot be deleted");
        for _ in 0..10 {
            click(&mut m, &mut cfg, Action::AddLayer, false);
        }
        assert_eq!(cfg.hip.layers.len(), MAX_LAYERS);
        assert_eq!(m.selected_layer(&cfg), MAX_LAYERS - 1, "new layer is selected");
        assert!(!click(&mut m, &mut cfg, Action::DupLayer, false).crosshair, "no-op at 8 layers");
        let before = cfg.hip.layers.clone();
        click(&mut m, &mut cfg, Action::DownLayer, false);
        assert_eq!(cfg.hip.layers, before, "Down at the bottom does nothing");
        click(&mut m, &mut cfg, Action::UpLayer, false);
        assert_eq!(m.selected_layer(&cfg), MAX_LAYERS - 2, "selection follows the moved layer");
        assert_eq!(cfg.hip.layers[MAX_LAYERS - 2], before[MAX_LAYERS - 1]);
        click(&mut m, &mut cfg, Action::DelLayer, false);
        assert_eq!(m.selected_layer(&cfg), MAX_LAYERS - 3, "selection becomes index-1");
    }

    #[test]
    fn changing_type_clamps_shared_values() {
        let (mut m, mut cfg) = setup();
        click(&mut m, &mut cfg, Action::SelectLayer(0), false);
        click(&mut m, &mut cfg, Action::LayerType, false);
        let l = &cfg.hip.layers[0];
        assert_eq!(l.kind, Kind::Ring);
        assert!(l.reach >= 2 && l.reach <= 13 && l.thick % 2 == 1);
        m.activate(&mut cfg, Action::LayerType, 1);
        m.activate(&mut cfg, Action::LayerType, 1);
        m.activate(&mut cfg, Action::LayerType, 1);
        assert_eq!(cfg.hip.layers[0].kind, Kind::Dot);
        assert!(cfg.hip.layers[0].reach % 2 == 1 && cfg.hip.layers[0].reach <= 9);
    }

    #[test]
    fn slider_click_drag_and_release() {
        let (mut m, mut cfg) = setup();
        let row = find(&m.layout(&cfg), Action::Slider(Param::Rot));
        let x0 = row.hit_rect().x;
        let y = row.hit_rect().y + 2;
        let fx = m.mouse_down(&mut cfg, x0 + SLIDER_W - 2, y, false);
        assert!(fx.crosshair);
        assert_eq!(cfg.hip.layers[0].rot, 345, "far right is the maximum");
        let fx = m.mouse_move(&mut cfg, x0 + 2, y, true);
        assert!(fx.crosshair && !fx.persist);
        assert_eq!(cfg.hip.layers[0].rot, 0);
        let fx = m.mouse_up(&mut cfg, x0 + 2, y, false);
        assert!(fx.persist, "released drag saves");
        assert!(!m.mouse_move(&mut cfg, x0 + 40, y, false).crosshair, "no drag after release");
    }

    #[test]
    fn slider_math_round_trips_the_thumb() {
        for (min, max, step) in [(1, 15, 1), (0, 345, 15), (1, 7, 2), (1, 10, 1), (1, 9, 2)] {
            let mut v = min;
            while v <= aligned_max(min, max, step) {
                let x = thumb_x(v, min, max) + THUMB_W / 2;
                assert_eq!(slider_value_at(x, min, max, step), v, "{min}..{max}/{step} at {v}");
                v += step;
            }
        }
        assert_eq!(slider_value_at(-50, 1, 15, 1), 1);
        assert_eq!(slider_value_at(500, 1, 15, 1), 15);
    }

    #[test]
    fn spokes_sliders_respect_the_16_pixel_limit() {
        let (mut m, mut cfg) = setup();
        let l = &mut cfg.hip.layers[0];
        l.gap = 10;
        l.reach = 6;
        let row = find(&m.layout(&cfg), Action::Slider(Param::Reach));
        assert!(matches!(row.kind, ItemKind::Slider { max: 6, .. }), "length max follows the gap");
        let y = row.hit_rect().y + 2;
        m.mouse_down(&mut cfg, row.hit_rect().x + SLIDER_W, y, false);
        let l = &cfg.hip.layers[0];
        assert!(l.gap + l.reach <= 16);
    }

    #[test]
    fn copy_hip_to_ads_and_reset() {
        let (mut m, mut cfg) = setup();
        click(&mut m, &mut cfg, Action::LayerColor, false);
        click(&mut m, &mut cfg, Action::Header(Section::Output), false);
        click(&mut m, &mut cfg, Action::Outline, false);
        assert_eq!(cfg.hip.outline, 2);
        let fx = click(&mut m, &mut cfg, Action::CopyHipToAds, false);
        assert!(fx.crosshair && fx.persist);
        assert_eq!(cfg.ads, cfg.hip);
        assert_eq!(m.editing, Profile::Ads);
        click(&mut m, &mut cfg, Action::ResetProfile, false);
        assert_eq!(cfg.ads, Crosshair::ads_default());
        assert_ne!(cfg.hip, Crosshair::classic(), "reset only touches the profile being edited");
    }

    #[test]
    fn presets_replace_layers_but_keep_outline_and_opacity() {
        let (mut m, mut cfg) = setup();
        click(&mut m, &mut cfg, Action::Header(Section::Presets), false);
        cfg.hip.opacity = 4;
        cfg.hip.outline = 2;
        click(&mut m, &mut cfg, Action::Preset(4), false);
        assert_eq!(cfg.hip.layers.len(), 4, "Sniper");
        assert_eq!((cfg.hip.opacity, cfg.hip.outline), (4, 2));
        assert_eq!(m.selected_layer(&cfg), 0);
    }

    #[test]
    fn export_row_copies_the_spec_string() {
        let (mut m, mut cfg) = setup();
        click(&mut m, &mut cfg, Action::Header(Section::Output), false);
        let fx = click(&mut m, &mut cfg, Action::CopyExport, false);
        assert_eq!(fx.clipboard.as_deref(), Some("oc2 o1 p10 spk.00ff66.6.4.3.4.0 dot.00ff66.1.4.1.4.0"));
        let row = find(&m.layout(&cfg), Action::CopyExport);
        if let ItemKind::Export { lines } = row.kind {
            assert_eq!(lines.concat(), share::encode(&cfg.hip));
            assert!(lines.iter().all(|l| l.chars().count() as i32 * 5 <= ROW_W));
            assert_eq!(row.rect.h, lines.len() as i32 * EXPORT_LINE_H + 4);
        } else {
            panic!("not an export row");
        }
    }

    #[test]
    fn keyboard_navigation_and_values() {
        let (mut m, mut cfg) = setup();
        m.key(&mut cfg, Key::Down);
        assert_eq!(m.focus_ring(), Some(Action::Header(Section::Profile)));
        m.key(&mut cfg, Key::Down);
        assert_eq!(m.focus_ring(), Some(Action::Editing));
        m.key(&mut cfg, Key::Right);
        assert_eq!(m.editing, Profile::Ads);
        m.key(&mut cfg, Key::Left);
        assert_eq!(m.editing, Profile::Hip, "left cycles back");
        m.key(&mut cfg, Key::Home);
        m.key(&mut cfg, Key::Space);
        let l = m.layout(&cfg);
        assert!(matches!(&l.items[0].kind, ItemKind::Header { open: false, .. }), "space toggles a header");
        m.key(&mut cfg, Key::End);
        assert_eq!(m.focus_ring(), l.items.iter().rev().find_map(|i| i.action));
        assert!(m.key(&mut cfg, Key::Esc).hide);
    }

    #[test]
    fn keyboard_slider_steps_and_pages() {
        let (mut m, mut cfg) = setup();
        click(&mut m, &mut cfg, Action::Slider(Param::Count), false);
        let start = cfg.hip.layers[0].count;
        m.key(&mut cfg, Key::Right);
        assert_eq!(cfg.hip.layers[0].count, start + 1);
        m.key(&mut cfg, Key::PageDown);
        assert_eq!(cfg.hip.layers[0].count, (start + 2).min(12), "page moves at least one step");
        for _ in 0..20 {
            m.key(&mut cfg, Key::Left);
        }
        assert_eq!(cfg.hip.layers[0].count, 1, "clamped at the minimum");
        let before = cfg.hip.layers[0].rot;
        m.focus = Some(Action::Slider(Param::Rot));
        m.key(&mut cfg, Key::Right);
        assert_eq!(cfg.hip.layers[0].rot, before + 15);
    }

    #[test]
    fn focus_follows_the_control_when_rows_reflow() {
        let (mut m, mut cfg) = setup();
        m.focus = Some(Action::LayerType);
        m.key(&mut cfg, Key::Right);
        assert_eq!(m.focus, Some(Action::LayerType), "still on Type although the slider rows changed");
        assert!(m.layout(&cfg).items.iter().any(|i| i.action == Some(Action::LayerType)));
    }

    #[test]
    fn wheel_scrolls_in_three_row_notches_and_clamps() {
        let (mut m, mut cfg) = setup();
        m.set_view(150, Metrics { body_cell: 6, small_cell: 5 });
        for s in [Section::Presets, Section::Output, Section::Preview] {
            m.activate(&mut cfg, Action::Header(s), 1);
        }
        let h = m.layout(&cfg).height;
        assert!(h > 150);
        assert_eq!(m.window_height(&cfg), 150);
        m.wheel(&cfg, -1);
        assert_eq!(m.scroll(), WHEEL_STEP);
        m.wheel(&cfg, -1000);
        assert_eq!(m.scroll(), h - 150);
        m.wheel(&cfg, 1000);
        assert_eq!(m.scroll(), 0);
    }

    #[test]
    fn keyboard_focus_scrolls_into_view() {
        let (mut m, mut cfg) = setup();
        m.set_view(120, Metrics { body_cell: 6, small_cell: 5 });
        m.key(&mut cfg, Key::End);
        let h = m.layout(&cfg).height;
        let last = m.layout(&cfg).items.iter().rev().find(|i| i.action.is_some()).unwrap().rect;
        assert!(last.y + last.h - m.scroll() <= 120 - BORDER);
        assert!(m.scroll() <= h - 120);
    }

    #[test]
    fn links_wrap_when_they_do_not_fit() {
        let (mut m, cfg) = setup();
        m.set_view(600, Metrics { body_cell: 12, small_cell: 5 });
        let l = m.layout(&cfg);
        let copy = find(&l, Action::CopyHipToAds);
        let reset = find(&l, Action::ResetProfile);
        assert_eq!(reset.rect.y - copy.rect.y, ROW_H, "second link wraps to a new line");
        assert_eq!(reset.rect.x, ROW_X);
    }

    #[test]
    fn hover_ignores_sliders_and_clears_on_leave() {
        let (mut m, mut cfg) = setup();
        let l = m.layout(&cfg);
        let t = find(&l, Action::LayerType);
        assert!(m.mouse_move(&mut cfg, t.rect.x + 2, t.rect.y + 2, false).repaint);
        assert_eq!(m.hover(), Some(Action::LayerType));
        let s = find(&l, Action::Slider(Param::Rot));
        m.mouse_move(&mut cfg, s.hit_rect().x + 5, s.rect.y + 2, false);
        assert_eq!(m.hover(), None, "sliders get no hover fill");
        m.mouse_move(&mut cfg, t.rect.x + 2, t.rect.y + 2, false);
        assert!(m.mouse_leave().repaint);
        assert_eq!(m.hover(), None);
    }

    #[test]
    fn press_must_release_on_the_same_row() {
        let (mut m, mut cfg) = setup();
        let l = m.layout(&cfg);
        let a = find(&l, Action::Editing);
        let b = find(&l, Action::AdsVia);
        m.mouse_down(&mut cfg, a.rect.x + 2, a.rect.y + 2, false);
        let fx = m.mouse_up(&mut cfg, b.rect.x + 2, b.rect.y + 2, false);
        assert!(!fx.crosshair && !fx.trigger);
        assert_eq!(cfg.trigger, Trigger::HoldRmb);
        assert_eq!(m.editing, Profile::Hip);
    }

    /// Everything in Experimental open and every feature switched on, so all rows exist.
    fn open_everything(m: &mut Menu, cfg: &mut Config) {
        m.open = [true; 10];
        m.groups = [true; 10];
        for st in states::ALL {
            cfg.states.rule_mut(st).key = 0x43;
        }
        cfg.exp.dynamics.enabled = true;
        cfg.exp.trace.enabled = true;
        cfg.exp.zoom.enabled = true;
        cfg.exp.auto.enabled = true;
    }

    #[test]
    fn experimental_is_closed_by_default_and_opens_in_place() {
        let (mut m, mut cfg) = setup();
        assert!(!m.layout(&cfg).items.iter().any(|i| i.action == Some(Action::Exp(Exp::CaptureShow))));
        let h0 = m.layout(&cfg).height;
        click(&mut m, &mut cfg, Action::Header(Section::Experimental), false);
        let l = m.layout(&cfg);
        let capture = find(&l, Action::Exp(Exp::CaptureShow));
        assert!(matches!(capture.kind, ItemKind::Check { on: true, label: "Show on screen capture" }));
        assert_eq!(l.height - h0, 6 * ROW_H, "capture check plus five collapsed group headers");
        for g in [Group::Blend, Group::Dynamics, Group::Trace, Group::Zoom, Group::Auto] {
            assert!(matches!(find(&l, Action::Group(g)).kind, ItemKind::GroupHead { open: false, .. }));
        }
    }

    #[test]
    fn a_group_expands_indented_with_its_own_rule() {
        let (mut m, mut cfg) = setup();
        click(&mut m, &mut cfg, Action::Header(Section::Experimental), false);
        click(&mut m, &mut cfg, Action::Group(Group::Dynamics), false);
        let l = m.layout(&cfg);
        let en = find(&l, Action::Exp(Exp::DynEnable));
        assert_eq!((en.rect.x, en.rect.w), (ROW_X + GROUP_INDENT, ROW_W - GROUP_INDENT));
        assert!(l.bodies.iter().any(|b| b.0 == ROW_X + 3), "group hairline");
        assert!(!l.items.iter().any(|i| i.action == Some(Action::Exp(Exp::DynBloom))), "options hidden until enabled");
    }

    #[test]
    fn checkbox_turns_a_feature_on_and_reveals_its_options() {
        let (mut m, mut cfg) = setup();
        click(&mut m, &mut cfg, Action::Header(Section::Experimental), false);
        click(&mut m, &mut cfg, Action::Group(Group::Trace), false);
        let fx = click(&mut m, &mut cfg, Action::Exp(Exp::TraceEnable), false);
        assert!(cfg.exp.trace.enabled);
        assert!(fx.experimental && fx.persist && !fx.crosshair);
        let l = m.layout(&cfg);
        for e in [Exp::TraceActive, Exp::TraceStyle, Exp::TraceReturn, Exp::TraceColor, Exp::TraceSpeedColor, Exp::TraceWidth, Exp::TraceScale, Exp::TraceFade, Exp::TraceRange, Exp::TraceSmooth] {
            find(&l, Action::Exp(e));
        }
        click(&mut m, &mut cfg, Action::Exp(Exp::TraceEnable), false);
        assert!(!cfg.exp.trace.enabled);
        assert!(!m.layout(&cfg).items.iter().any(|i| i.action == Some(Action::Exp(Exp::TraceStyle))));
    }

    #[test]
    fn the_test_trace_link_plays_the_demo_without_touching_the_config() {
        let (mut m, mut cfg) = setup();
        click(&mut m, &mut cfg, Action::Header(Section::Experimental), false);
        click(&mut m, &mut cfg, Action::Group(Group::Trace), false);
        assert!(!m.layout(&cfg).items.iter().any(|i| i.action == Some(Action::Exp(Exp::TraceTest))), "nothing to test until Spray trace is on");
        click(&mut m, &mut cfg, Action::Exp(Exp::TraceEnable), false);
        let before = cfg.clone();
        let row = find(&m.layout(&cfg), Action::Exp(Exp::TraceTest));
        assert!(matches!(&row.kind, ItemKind::Link { text } if text == "Test trace"));
        let fx = click(&mut m, &mut cfg, Action::Exp(Exp::TraceTest), false);
        assert!(fx.trace_demo && !fx.persist && !fx.experimental && !fx.crosshair);
        assert_eq!(cfg, before, "testing changes no setting");
        m.focus = Some(Action::Exp(Exp::TraceTest));
        assert!(m.key(&mut cfg, Key::Enter).trace_demo, "Enter plays it too");
    }

    #[test]
    fn the_updates_section_toggles_checks_and_offers_a_restart_only_when_one_is_ready() {
        let (mut m, mut cfg) = setup();
        click(&mut m, &mut cfg, Action::Header(Section::Updates), false);
        let l = m.layout(&cfg);
        assert!(matches!(&find(&l, Action::AutoUpdate).kind, ItemKind::Cycle { label: "Auto-update:", value, .. } if value == "On"));
        let version = l.items.iter().find(|i| matches!(&i.kind, ItemKind::Cycle { label: "Version:", .. })).expect("version row");
        assert!(version.action.is_none(), "the version is information, not a control");
        assert!(matches!(&version.kind, ItemKind::Cycle { value, .. } if value == update::VERSION));
        find(&l, Action::CheckNow);
        assert!(!l.items.iter().any(|i| i.action == Some(Action::RestartNow)), "nothing to restart into yet");

        let fx = click(&mut m, &mut cfg, Action::AutoUpdate, false);
        assert!(!cfg.autoupdate && fx.update_cfg && fx.persist && !fx.crosshair);
        assert!(matches!(&find(&m.layout(&cfg), Action::AutoUpdate).kind, ItemKind::Cycle { value, .. } if value == "Off"));
        click(&mut m, &mut cfg, Action::AutoUpdate, true);
        assert!(cfg.autoupdate, "right click toggles back too");

        let fx = click(&mut m, &mut cfg, Action::CheckNow, false);
        assert!(fx.update_check && !fx.persist && !fx.update_cfg);

        assert!(m.set_update(Status::Installed("0.2.9".to_string())), "a new status asks for a redraw");
        assert!(!m.set_update(Status::Installed("0.2.9".to_string())), "the same one does not");
        let fx = click(&mut m, &mut cfg, Action::RestartNow, false);
        assert!(fx.restart && !fx.quit);
        m.set_update(Status::UpToDate);
        assert!(!m.layout(&cfg).items.iter().any(|i| i.action == Some(Action::RestartNow)), "the link goes away again");
    }

    #[test]
    fn force_quit_is_the_red_button_at_the_very_bottom_and_only_a_click_or_enter_fires_it() {
        let (mut m, mut cfg) = setup();
        for s in [Section::Presets, Section::States, Section::Output, Section::Display, Section::Updates, Section::Preview, Section::Experimental] {
            m.activate(&mut cfg, Action::Header(s), 1);
        }
        let l = m.layout(&cfg);
        let last = l.items.last().expect("rows");
        assert_eq!(last.action, Some(Action::ForceQuit), "the last row, with everything open");
        assert!(matches!(&last.kind, ItemKind::Danger { text } if text == "ForceQuit"));
        assert!(l.items.iter().all(|i| i.rect.y <= last.rect.y), "nothing sits below it");
        assert!(last.rect.y + last.rect.h + PAD_BOTTOM + BORDER == l.height, "and it ends the window");
        assert!(!click(&mut m, &mut cfg, Action::Header(Section::Presets), false).quit, "other rows never quit");
        let fx = click(&mut m, &mut cfg, Action::ForceQuit, true);
        assert!(!fx.quit, "a right click (stepping back) does not quit");
        let fx = click(&mut m, &mut cfg, Action::ForceQuit, false);
        assert!(fx.quit && !fx.persist && !fx.hide);
        m.focus = Some(Action::ForceQuit);
        assert!(m.key(&mut cfg, Key::Enter).quit, "Enter on the focused button quits");
        m.focus = Some(Action::ForceQuit);
        assert!(!m.key(&mut cfg, Key::Left).quit, "arrows only move");
        assert!(!m.key(&mut cfg, Key::Esc).quit, "Esc just closes the menu");
    }

    #[test]
    fn capture_checkbox_defaults_on_and_toggles() {
        let (mut m, mut cfg) = setup();
        click(&mut m, &mut cfg, Action::Header(Section::Experimental), false);
        assert!(cfg.exp.show_on_capture);
        let fx = click(&mut m, &mut cfg, Action::Exp(Exp::CaptureShow), false);
        assert!(!cfg.exp.show_on_capture && fx.experimental && fx.persist);
    }

    #[test]
    fn blend_row_cycles_the_selected_layers_blend() {
        let (mut m, mut cfg) = setup();
        click(&mut m, &mut cfg, Action::Header(Section::Experimental), false);
        click(&mut m, &mut cfg, Action::Group(Group::Blend), false);
        click(&mut m, &mut cfg, Action::SelectLayer(1), false);
        let fx = click(&mut m, &mut cfg, Action::Exp(Exp::Blend), false);
        assert_eq!(cfg.hip.layers[1].blend, Blend::Glow);
        assert_eq!(cfg.hip.layers[0].blend, Blend::Normal, "only the selected layer");
        assert!(fx.crosshair && fx.experimental && fx.persist);
        click(&mut m, &mut cfg, Action::Exp(Exp::Blend), true);
        assert_eq!(cfg.hip.layers[1].blend, Blend::Normal, "right click steps back");
        for _ in 0..5 {
            m.activate(&mut cfg, Action::Exp(Exp::Blend), 1);
        }
        assert_eq!(cfg.hip.layers[1].blend, Blend::Normal, "five steps wrap around");
        let row = find(&m.layout(&cfg), Action::Exp(Exp::Blend));
        assert!(matches!(&row.kind, ItemKind::Cycle { label: "Blend:", value, .. } if value == "Normal"));
        let g = find(&m.layout(&cfg), Action::Group(Group::Blend));
        assert!(matches!(&g.kind, ItemKind::GroupHead { title, .. } if title == "Blend · Layer 02"));
    }

    #[test]
    fn every_experimental_cycle_and_check_changes_the_config() {
        let (mut m, mut cfg) = setup();
        open_everything(&mut m, &mut cfg);
        let l = m.layout(&cfg);
        let mut tested = 0;
        for item in &l.items {
            let Some(Action::Exp(id)) = item.action else { continue };
            if id.is_slider() || id == Exp::Blend || id == Exp::TraceTest {
                continue;
            }
            let before = cfg.exp.clone();
            let fx = m.activate(&mut cfg, Action::Exp(id), 1);
            assert!(fx.experimental && fx.persist && fx.repaint, "{id:?}");
            assert_ne!(cfg.exp, before, "{id:?} must change something");
            m.activate(&mut cfg, Action::Exp(id), -1);
            if !matches!(id, Exp::CaptureShow | Exp::DynEnable | Exp::TraceEnable | Exp::ZoomEnable | Exp::AutoEnable) {
                assert_eq!(cfg.exp, before, "{id:?}: previous undoes next");
            } else {
                assert_eq!(cfg.exp, before, "{id:?}: toggling twice restores");
            }
            tested += 1;
        }
        assert!(tested >= 17, "covered {tested} controls");
    }

    #[test]
    fn experimental_sliders_step_clamp_and_snap() {
        let (mut m, mut cfg) = setup();
        open_everything(&mut m, &mut cfg);
        m.focus = Some(Action::Exp(Exp::ZoomFactor));
        let fx = m.key(&mut cfg, Key::Right);
        assert_eq!(cfg.exp.zoom.factor, 25, "2.0x steps by 0.5x");
        assert!(fx.experimental && fx.persist && !fx.crosshair);
        for _ in 0..20 {
            m.key(&mut cfg, Key::Right);
        }
        assert_eq!(cfg.exp.zoom.factor, 40, "clamped at 4.0x");
        m.focus = Some(Action::Exp(Exp::ZoomSize));
        m.key(&mut cfg, Key::Left);
        assert_eq!(cfg.exp.zoom.size, 200, "size steps by 20");
        m.focus = Some(Action::Exp(Exp::TraceWidth));
        for _ in 0..9 {
            m.key(&mut cfg, Key::Right);
        }
        assert_eq!(cfg.exp.trace.width, 3);
        m.focus = Some(Action::Exp(Exp::TraceFade));
        m.key(&mut cfg, Key::PageUp);
        assert_eq!(cfg.exp.trace.fade, 8 - 3, "page step is a tenth of the range, at least one step");
    }

    #[test]
    fn dragging_an_experimental_slider_reports_experimental_not_crosshair() {
        let (mut m, mut cfg) = setup();
        open_everything(&mut m, &mut cfg);
        let row = find(&m.layout(&cfg), Action::Exp(Exp::TraceScale));
        let fx = m.mouse_down(&mut cfg, row.hit_rect().x + SLIDER_W - 2, row.hit_rect().y + 3, false);
        assert_eq!(cfg.exp.trace.scale, 10);
        assert!(fx.experimental && !fx.crosshair);
        let fx = m.mouse_up(&mut cfg, row.hit_rect().x + SLIDER_W - 2, row.hit_rect().y + 3, false);
        assert!(fx.persist);
    }

    #[test]
    fn indented_rows_still_fit_and_their_labels_fit_the_narrower_column() {
        let (mut m, mut cfg) = setup();
        open_everything(&mut m, &mut cfg);
        let l = m.layout(&cfg);
        let cell = 6;
        for item in &l.items {
            assert!(item.rect.x >= CONTENT_X && item.rect.x + item.rect.w <= CONTENT_X + CONTENT_W, "{:?} inside the content width", item.action);
            match &item.kind {
                ItemKind::Slider { label, .. } => {
                    let label_col = item.track_x() - SLIDER_GAP - item.rect.x;
                    assert!((label.chars().count() as i32 + 1) * cell <= label_col, "{label}: label column is {label_col}px");
                    assert!(item.track_x() + SLIDER_W + SLIDER_GAP + VALUE_W <= item.rect.x + item.rect.w + 0);
                }
                ItemKind::Cycle { label, value, swatch } => {
                    let w = (label.chars().count() + value.chars().count()) as i32 * cell + 4 + if swatch.is_some() { 14 } else { 0 };
                    assert!(w <= item.rect.w, "{label} {value}: {w}px in {}", item.rect.w);
                }
                ItemKind::Check { label, .. } => assert!(14 + label.chars().count() as i32 * cell <= item.rect.w, "{label}"),
                _ => {}
            }
        }
        let tops: Vec<i32> = l.items.iter().map(|i| i.rect.y).collect();
        assert!(tops.windows(2).all(|w| w[0] <= w[1]), "rows are in reading order");
    }

    #[test]
    fn keyboard_toggles_checkboxes_and_group_headers() {
        let (mut m, mut cfg) = setup();
        click(&mut m, &mut cfg, Action::Header(Section::Experimental), false);
        m.focus = Some(Action::Exp(Exp::CaptureShow));
        m.key(&mut cfg, Key::Space);
        assert!(!cfg.exp.show_on_capture);
        m.key(&mut cfg, Key::Right);
        assert!(cfg.exp.show_on_capture, "left and right toggle too");
        m.focus = Some(Action::Group(Group::Zoom));
        m.key(&mut cfg, Key::Enter);
        assert!(m.layout(&cfg).items.iter().any(|i| i.action == Some(Action::Exp(Exp::ZoomEnable))));
    }

    #[test]
    fn display_rows_show_what_auto_resolved_to() {
        let (mut m, mut cfg) = setup();
        click(&mut m, &mut cfg, Action::Header(Section::Display), false);
        m.set_info(Info { scale: 2, ui_scale: 2, refresh: 144 });
        let l = m.layout(&cfg);
        let val = |id: Disp| match find(&l, Action::Disp(id)).kind {
            ItemKind::Cycle { value, .. } => value,
            _ => panic!("not a cycle row"),
        };
        assert_eq!(val(Disp::Monitor), "Primary", "the default is the middle of the screen");
        assert_eq!(val(Disp::Scale), "Auto (2x)");
        assert_eq!(val(Disp::UiScale), "Auto (2x)");
        assert_eq!(val(Disp::Fps), "Auto (144 Hz)");
        assert!(m.set_info(Info { scale: 1, ui_scale: 1, refresh: 60 }), "a change asks for a redraw");
        assert!(!m.set_info(Info { scale: 1, ui_scale: 1, refresh: 60 }), "no change, no redraw");
    }

    #[test]
    fn display_rows_cycle_through_auto_and_fixed_values() {
        let (mut m, mut cfg) = setup();
        click(&mut m, &mut cfg, Action::Header(Section::Display), false);
        let fx = click(&mut m, &mut cfg, Action::Disp(Disp::Scale), false);
        assert_eq!(cfg.display.scale, Some(1));
        assert!(fx.display && fx.persist && !fx.crosshair, "the overlay re-resolves; nothing is drawn by the menu itself");
        for want in [Some(2), Some(3), Some(4), None, Some(1)] {
            m.activate(&mut cfg, Action::Disp(Disp::Scale), 1);
            assert_eq!(cfg.display.scale, want);
        }
        m.activate(&mut cfg, Action::Disp(Disp::Scale), -1);
        assert_eq!(cfg.display.scale, None, "right click goes back to Auto");
        m.activate(&mut cfg, Action::Disp(Disp::Fps), 1);
        assert_eq!(cfg.display.fps, Some(30));
        for _ in 0..FPS_CAPS.len() {
            m.activate(&mut cfg, Action::Disp(Disp::Fps), 1);
        }
        assert_eq!(cfg.display.fps, None, "wraps back to Auto after the last cap");
        assert_eq!(cfg.display.monitor, MonitorSel::Primary, "starts on Primary");
        for want in [MonitorSel::Follow, MonitorSel::Nth(1), MonitorSel::Nth(2), MonitorSel::Nth(3), MonitorSel::Nth(4), MonitorSel::Primary] {
            m.activate(&mut cfg, Action::Disp(Disp::Monitor), 1);
            assert_eq!(cfg.display.monitor, want);
        }
        for _ in 0..3 {
            m.activate(&mut cfg, Action::Disp(Disp::UiScale), 1);
        }
        assert_eq!(cfg.display.ui_scale, Some(3));
        assert_eq!(Config::parse(&cfg.serialize()), cfg, "saved and reloaded");
    }

    #[test]
    fn every_layer_type_offers_sliders_that_reach_their_own_ends() {
        let (mut m, mut cfg) = setup();
        for kind in Kind::ALL {
            for thick in [1, 3, 7] {
                let mut base = Layer::new(kind, 0xFFFFFF);
                base.thick = thick;
                base.clamp();
                for (param, label, min, max, step) in slider_defs(&base) {
                    assert!((label.chars().count() as i32 + 1) * 6 <= 56, "{kind:?} {label}: label fits the 56px column");
                    for v in [min, aligned_max(min, max, step)] {
                        cfg.hip.layers[0] = base.clone();
                        m.set_slider(&mut cfg, Action::Slider(param), v);
                        assert_eq!(param_get(&cfg.hip.layers[0], param), v, "{kind:?} thick {thick}: {label} can be set to {v} (range {min}..{max})");
                    }
                }
            }
        }
    }

    #[test]
    fn the_layer_type_row_walks_through_every_shape_and_back() {
        let (mut m, mut cfg) = setup();
        let start = cfg.hip.layers[0].kind;
        for _ in 0..Kind::ALL.len() {
            m.activate(&mut cfg, Action::LayerType, 1);
            let l = &cfg.hip.layers[0];
            assert!(!slider_defs(l).is_empty() && !cfg.hip.raster().is_empty(), "{:?}", l.kind);
        }
        assert_eq!(cfg.hip.layers[0].kind, start, "a full lap comes back");
        m.activate(&mut cfg, Action::LayerType, -1);
        assert_eq!(cfg.hip.layers[0].kind, Kind::Disc, "left from Spokes wraps to the last shape");
    }

    #[test]
    fn arc_span_reads_in_degrees() {
        let (m, mut cfg) = setup();
        cfg.hip.layers[0] = Layer::new(Kind::Arc, 0xFFFFFF);
        let row = find(&m.layout(&cfg), Action::Slider(Param::Count));
        assert!(matches!(&row.kind, ItemKind::Slider { label: "Span", text, .. } if text == "90°"), "{:?}", row.kind);
    }

    #[test]
    fn custom_color_and_transform_are_closed_groups_that_open_in_place() {
        let (mut m, mut cfg) = setup();
        let l = m.layout(&cfg);
        for g in [Group::Color, Group::Transform] {
            assert!(matches!(find(&l, Action::Group(g)).kind, ItemKind::GroupHead { open: false, .. }));
        }
        assert!(!l.items.iter().any(|i| matches!(i.action, Some(Action::Lx(_)))));
        let h0 = l.height;
        click(&mut m, &mut cfg, Action::Group(Group::Color), false);
        let l = m.layout(&cfg);
        for x in [Lx::Hue, Lx::Sat, Lx::Val, Lx::Cycle, Lx::CopyHex, Lx::PasteHex] {
            let it = find(&l, Action::Lx(x));
            assert!(it.rect.x >= ROW_X + GROUP_INDENT, "{x:?} is indented under its group");
        }
        assert_eq!(l.height - h0, 5 * ROW_H, "three sliders, the cycle row and one row of links");
        click(&mut m, &mut cfg, Action::Group(Group::Transform), false);
        for x in [Lx::Opacity, Lx::OffX, Lx::OffY, Lx::Mirror] {
            find(&m.layout(&cfg), Action::Lx(x));
        }
    }

    #[test]
    fn the_color_sliders_make_any_color_and_remember_the_hue() {
        let (mut m, mut cfg) = setup();
        click(&mut m, &mut cfg, Action::Group(Group::Color), false);
        let fx = {
            let row = find(&m.layout(&cfg), Action::Lx(Lx::Hue));
            m.mouse_down(&mut cfg, row.hit_rect().x + SLIDER_W - 2, row.hit_rect().y + 3, false)
        };
        assert!(fx.crosshair, "dragging a color slider redraws the crosshair");
        let hue_max = cfg.hip.layers[0].color;
        assert_eq!(hue_max, color::from_slider_hsv(355, 100, 100), "full hue, saturation and value of the green: the slider moved only the hue");
        m.set_slider(&mut cfg, Action::Lx(Lx::Hue), 120);
        assert_eq!(cfg.hip.layers[0].color, 0x00FF00);
        m.set_slider(&mut cfg, Action::Lx(Lx::Val), 0);
        assert_eq!(cfg.hip.layers[0].color, 0x000000);
        let row = find(&m.layout(&cfg), Action::Lx(Lx::Hue));
        assert!(matches!(row.kind, ItemKind::Slider { value: 120, .. }), "black still shows hue 120");
        m.set_slider(&mut cfg, Action::Lx(Lx::Val), 100);
        assert_eq!(cfg.hip.layers[0].color, 0x00FF00, "and raising Value brings the green back");
        let name = find(&m.layout(&cfg), Action::LayerColor);
        assert!(matches!(&name.kind, ItemKind::Cycle { value, .. } if value == "#00ff00"), "an unnamed color shows its hex");
        assert_eq!(Config::parse(&cfg.serialize()), cfg, "and it is saved");
    }

    #[test]
    fn touching_one_color_slider_does_not_shift_the_rest_of_the_color() {
        let (mut m, mut cfg) = setup();
        click(&mut m, &mut cfg, Action::Group(Group::Color), false);
        // Pasted colors sit between slider steps; moving Value must change only the brightness.
        for hex in [0x336699u32, 0xFF9500, 0x12AB34, 0xB388FF, 0x7F7F80, 0xD97757] {
            m.pasted(&mut cfg, Paste::Hex, &format!("#{hex:06x}"));
            let (h0, s0, v0) = color::rgb_to_hsv(hex);
            m.set_slider(&mut cfg, Action::Lx(Lx::Val), (v0 * 100.0).round() as i32);
            let (h1, s1, _) = color::rgb_to_hsv(cfg.hip.layers[0].color);
            assert!((h1 - h0).abs() < 1.5 || (h1 - h0).abs() > 358.5, "#{hex:06x}: hue stayed ({h0} -> {h1})");
            assert!((s1 - s0).abs() < 0.02, "#{hex:06x}: saturation stayed ({s0} -> {s1})");
            let back = cfg.hip.layers[0].color;
            for shift in [16, 8, 0] {
                let (a, b) = ((hex >> shift) & 0xFF, (back >> shift) & 0xFF);
                assert!((a as i32 - b as i32).abs() <= 3, "#{hex:06x} -> #{back:06x}: touching Value at its own value barely moves the color");
            }
        }
    }

    #[test]
    fn copy_hex_and_paste_hex_go_through_the_clipboard_request() {
        let (mut m, mut cfg) = setup();
        click(&mut m, &mut cfg, Action::Group(Group::Color), false);
        let fx = click(&mut m, &mut cfg, Action::Lx(Lx::CopyHex), false);
        assert_eq!(fx.clipboard.as_deref(), Some("#00ff66"));
        let fx = click(&mut m, &mut cfg, Action::Lx(Lx::PasteHex), false);
        assert_eq!(fx.paste, Some(Paste::Hex));
        assert!(!fx.crosshair, "nothing changes until the text arrives");
        let fx = m.pasted(&mut cfg, Paste::Hex, " #FF9500 \n");
        assert!(fx.crosshair && fx.persist);
        assert_eq!(cfg.hip.layers[0].color, 0xFF9500);
        let fx = m.pasted(&mut cfg, Paste::Hex, "not a color");
        assert!(!fx.crosshair && !fx.persist, "junk changes nothing");
        assert_eq!(cfg.hip.layers[0].color, 0xFF9500);
    }

    #[test]
    fn transform_rows_edit_only_the_selected_layer() {
        let (mut m, mut cfg) = setup();
        click(&mut m, &mut cfg, Action::Group(Group::Transform), false);
        click(&mut m, &mut cfg, Action::SelectLayer(1), false);
        m.set_slider(&mut cfg, Action::Lx(Lx::Opacity), 4);
        m.set_slider(&mut cfg, Action::Lx(Lx::OffX), -5);
        m.set_slider(&mut cfg, Action::Lx(Lx::OffY), 99);
        let l = &cfg.hip.layers[1];
        assert_eq!((l.opacity, l.ox, l.oy), (4, -5, 12), "out-of-range clamps");
        assert_eq!(cfg.hip.layers[0], Config::default().hip.layers[0], "the other layer is untouched");
        let fx = click(&mut m, &mut cfg, Action::Lx(Lx::Mirror), false);
        assert!(fx.crosshair && fx.persist);
        assert_eq!(cfg.hip.layers[1].mirror, Mirror::Horizontal);
        click(&mut m, &mut cfg, Action::Lx(Lx::Mirror), true);
        assert_eq!(cfg.hip.layers[1].mirror, Mirror::Off, "right click steps back");
        click(&mut m, &mut cfg, Action::Group(Group::Color), false);
        click(&mut m, &mut cfg, Action::Lx(Lx::Cycle), false);
        assert_eq!(cfg.hip.layers[1].cycle, 1);
        let row = find(&m.layout(&cfg), Action::Lx(Lx::OffX));
        assert!(matches!(&row.kind, ItemKind::Slider { text, value: -5, .. } if text == "-5"));
        assert_eq!(Config::parse(&cfg.serialize()), cfg);
    }

    #[test]
    fn pasting_a_code_replaces_the_profile_being_edited() {
        let (mut m, mut cfg) = setup();
        let mut other = Crosshair::ads_default();
        other.outline = 0;
        other.opacity = 6;
        other.layers[0].mirror = Mirror::Both;
        let code = share::encode(&other);
        let fx = click(&mut m, &mut cfg, Action::Header(Section::Output), false);
        assert!(fx.paste.is_none());
        let fx = click(&mut m, &mut cfg, Action::PasteCode, false);
        assert_eq!(fx.paste, Some(Paste::Code));
        let before_ads = cfg.ads.clone();
        let fx = m.pasted(&mut cfg, Paste::Code, &code);
        assert!(fx.crosshair && fx.persist);
        assert_eq!(cfg.hip, other, "outline, opacity, mirror and all landed in the hip profile");
        assert_eq!(cfg.ads, before_ads);
        click(&mut m, &mut cfg, Action::Editing, false);
        m.pasted(&mut cfg, Paste::Code, &share::encode(&Crosshair::classic()));
        assert_eq!(cfg.ads, Crosshair::classic(), "with ADS selected, ADS is replaced");
    }

    #[test]
    fn a_pair_code_fills_both_profiles_and_junk_changes_nothing() {
        let (mut m, mut cfg) = setup();
        click(&mut m, &mut cfg, Action::Header(Section::Output), false);
        let fx = click(&mut m, &mut cfg, Action::CopyBoth, false);
        let code = fx.clipboard.expect("a code");
        assert_eq!(code, share::encode_both(&cfg.hip, &cfg.ads));
        let mut other = Config::default();
        other.hip.layers.truncate(1);
        other.ads.outline = 2;
        let mut m2 = Menu::new();
        m2.pasted(&mut other, Paste::Code, &code);
        assert_eq!((other.hip, other.ads), (cfg.hip.clone(), cfg.ads.clone()));
        let before = cfg.clone();
        for junk in ["", "hello", "oc2", "#ff0000", "oc2 hip"] {
            let fx = m.pasted(&mut cfg, Paste::Code, junk);
            assert!(!fx.crosshair && !fx.persist, "{junk:?}");
        }
        assert_eq!(cfg, before);
    }

    #[test]
    fn the_morph_row_cycles_the_times_and_asks_only_to_save() {
        let (mut m, mut cfg) = setup();
        let row = find(&m.layout(&cfg), Action::Morph);
        assert!(matches!(&row.kind, ItemKind::Cycle { label: "Morph:", value, .. } if value == "Medium"));
        let fx = click(&mut m, &mut cfg, Action::Morph, false);
        assert_eq!(cfg.morph, 250);
        assert!(fx.persist && !fx.crosshair && !fx.experimental);
        click(&mut m, &mut cfg, Action::Morph, false);
        assert_eq!(cfg.morph, 0, "wraps to Off");
        click(&mut m, &mut cfg, Action::Morph, true);
        assert_eq!(cfg.morph, 250, "right click steps back");
        cfg.morph = 40;
        assert!(matches!(&find(&m.layout(&cfg), Action::Morph).kind, ItemKind::Cycle { value, .. } if value == "Fast"), "a hand-edited time still shows a sensible name");
    }

    #[test]
    fn states_are_closed_until_opened_and_hide_their_rows_until_a_key_is_bound() {
        let (mut m, mut cfg) = setup();
        click(&mut m, &mut cfg, Action::Header(Section::States), false);
        let l = m.layout(&cfg);
        for st in [State::Crouch, State::Sprint, State::Jump] {
            let head = find(&l, Action::Group(Group::of(st)));
            assert!(matches!(&head.kind, ItemKind::GroupHead { title, open: false } if title == st.name()));
        }
        click(&mut m, &mut cfg, Action::Group(Group::Crouch), false);
        let l = m.layout(&cfg);
        find(&l, Action::State(State::Crouch, Sx::Key));
        for x in [Sx::Mode, Sx::Gap, Sx::Size, Sx::Color] {
            assert!(!l.items.iter().any(|i| i.action == Some(Action::State(State::Crouch, x))), "{x:?} waits for a key");
        }
        let fx = click(&mut m, &mut cfg, Action::State(State::Crouch, Sx::Key), false);
        assert!(fx.states && fx.persist && !fx.crosshair);
        assert_eq!(cfg.states.crouch.key, 0x10, "None is followed by Shift");
        let l = m.layout(&cfg);
        for x in [Sx::Mode, Sx::Gap, Sx::Size, Sx::Color] {
            find(&l, Action::State(State::Crouch, x));
        }
        let head = find(&l, Action::Group(Group::Crouch));
        assert!(matches!(&head.kind, ItemKind::GroupHead { title, .. } if title == "Crouch · Shift"), "the group title names the key");
        assert!(!l.items.iter().any(|i| i.action == Some(Action::State(State::Sprint, Sx::Gap))), "other states stay closed");
    }

    #[test]
    fn state_rows_edit_one_rule_and_report_states_not_crosshair() {
        let (mut m, mut cfg) = setup();
        click(&mut m, &mut cfg, Action::Header(Section::States), false);
        click(&mut m, &mut cfg, Action::Group(Group::Sprint), false);
        click(&mut m, &mut cfg, Action::State(State::Sprint, Sx::Key), false);
        let fx = click(&mut m, &mut cfg, Action::State(State::Sprint, Sx::Mode), false);
        assert!(cfg.states.sprint.toggle && fx.states && fx.persist);
        click(&mut m, &mut cfg, Action::State(State::Sprint, Sx::Color), false);
        assert_eq!(cfg.states.sprint.color, Some(PALETTE[0].1));
        click(&mut m, &mut cfg, Action::State(State::Sprint, Sx::Color), true);
        assert_eq!(cfg.states.sprint.color, None, "right click steps back to Same");
        let row = find(&m.layout(&cfg), Action::State(State::Sprint, Sx::Gap));
        let fx = m.mouse_down(&mut cfg, row.hit_rect().x + SLIDER_W - 2, row.hit_rect().y + 3, false);
        assert_eq!(cfg.states.sprint.gap, states::GAP_RANGE.1);
        assert!(fx.states && !fx.crosshair);
        m.mouse_up(&mut cfg, row.hit_rect().x + SLIDER_W - 2, row.hit_rect().y + 3, false);
        m.focus = Some(Action::State(State::Sprint, Sx::Size));
        let fx = m.key(&mut cfg, Key::Left);
        assert_eq!(cfg.states.sprint.size, -1);
        assert!(fx.states && fx.persist);
        assert_eq!(cfg.states.crouch, States::default().crouch, "other rules are untouched");
        assert_eq!(Config::parse(&cfg.serialize()), cfg);
        assert!(matches!(&find(&m.layout(&cfg), Action::State(State::Sprint, Sx::Size)).kind, ItemKind::Slider { text, .. } if text == "-1"));
    }

    #[test]
    fn experimental_settings_survive_a_save() {
        let (mut m, mut cfg) = setup();
        open_everything(&mut m, &mut cfg);
        m.activate(&mut cfg, Action::Exp(Exp::TraceStyle), 1);
        m.activate(&mut cfg, Action::Exp(Exp::DynMotion), 1);
        m.activate(&mut cfg, Action::Exp(Exp::Blend), 1);
        m.activate(&mut cfg, Action::Exp(Exp::CaptureShow), 1);
        assert_eq!(Config::parse(&cfg.serialize()), cfg);
    }
}
