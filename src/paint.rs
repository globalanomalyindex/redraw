//! Software painter for the HUD. Writes premultiplied ARGB straight into a
//! pixel buffer, so every edge is a whole pixel. Text is the one thing it
//! cannot do alone: glyph shapes come from a `TextBackend` (GDI in the real
//! program, a stub in tests).

use crate::config::Config;
use crate::menu::*;
use crate::update::Status;

pub const COLOR_BORDER: u32 = 0x3A4150;
pub const COLOR_HAIRLINE: u32 = 0x2F3541;
pub const COLOR_TEXT: u32 = 0xE6E9EF;
pub const COLOR_LABEL: u32 = 0x8B93A3;
pub const COLOR_ACTIVE: u32 = 0xFFFFFF;
/// Header of the Experimental [!] section: yellow, to flag possible instability.
pub const COLOR_WARN: u32 = 0xFFE500;
/// Claude's clay orange: the `[Dev]` tag and the build time in the title.
pub const COLOR_DEV: u32 = 0xD97757;
/// "up to date" in the title area.
pub const COLOR_OK: u32 = 0x3DDC6E;
/// The ForceQuit button.
pub const COLOR_DANGER: u32 = 0xFF4D4D;
/// UTC date and time of the last build, stamped by `build.rs`.
const BUILT_UTC: &str = env!("REDRAW_BUILT_UTC");

/// `YYYY-MM-DD HH:MM` as (year, month, day, hour, minute).
fn parse_stamp(s: &str) -> Option<(u16, u16, u16, u16, u16)> {
    let b = s.as_bytes();
    if b.len() != 16 || b[4] != b'-' || b[7] != b'-' || b[10] != b' ' || b[13] != b':' {
        return None;
    }
    let n = |a: usize, z: usize| s.get(a..z)?.parse::<u16>().ok();
    Some((n(0, 4)?, n(5, 7)?, n(8, 10)?, n(11, 13)?, n(14, 16)?))
}

#[cfg(windows)]
fn utc_to_local(utc: &str) -> String {
    use std::ptr::null;
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::Time::SystemTimeToTzSpecificLocalTime;
    let Some((y, mo, d, h, mi)) = parse_stamp(utc) else { return utc.to_string() };
    let from = SYSTEMTIME { wYear: y, wMonth: mo, wDayOfWeek: 0, wDay: d, wHour: h, wMinute: mi, wSecond: 0, wMilliseconds: 0 };
    let mut to: SYSTEMTIME = unsafe { std::mem::zeroed() };
    if unsafe { SystemTimeToTzSpecificLocalTime(null(), &from, &mut to) } == 0 {
        return utc.to_string();
    }
    format!("{:04}-{:02}-{:02} {:02}:{:02}", to.wYear, to.wMonth, to.wDay, to.wHour, to.wMinute)
}

#[cfg(not(windows))]
fn utc_to_local(utc: &str) -> String {
    utc.to_string()
}

/// The build time as the person running the program sees it: the UTC stamp, in their local time.
pub fn built() -> &'static str {
    static LOCAL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    LOCAL.get_or_init(|| utc_to_local(BUILT_UTC))
}
pub const DEV_TAG: &str = "[Dev]";
const PANEL_RGB: u32 = 0x0C0E12;
const PANEL_ALPHA: u32 = 209; // 0.82 of 255
const HOVER_ALPHA: u32 = 23; // 0.09 of 255
const PREVIEW_BG: u32 = 0x5A6070;
const PREVIEW_LIGHT: u32 = 0xD9DDE6;
const PREVIEW_DARK: u32 = 0x101216;
const GRADIENT: [u32; 7] = [0xFF5F5F, 0xFFB627, 0xFFE94D, 0x6BFF8A, 0x4AA8FF, 0xB388FF, 0xFF5F5F];
pub const TITLE: &str = "reDraw";
pub const CREDIT: &str = "by christopher robin fiore";
pub const TITLE_PERIOD_SECS: f32 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Font {
    Body = 0,
    Title = 1,
    Small = 2,
}

pub trait TextBackend {
    /// advance of one glyph cell, in pixels
    fn cell(&self, font: Font) -> i32;
    /// Draw `text` with its line box starting at (x, y) and `line_h` tall.
    fn draw(&self, canvas: &mut Canvas, x: i32, y: i32, line_h: i32, text: &str, rgb: u32, font: Font);
}

pub struct Canvas<'a> {
    pub px: &'a mut [u32],
    pub w: i32,
    pub h: i32,
    pub clip: Rect,
}

/// Exact `x / 255` for `x` up to 65534, without a divide instruction.
fn div255(x: u32) -> u32 {
    (x + 1 + (x >> 8)) >> 8
}

/// The panel color as a premultiplied pixel: what the panel fill leaves on an empty buffer.
fn panel_px() -> u32 {
    let ch = |shift: u32| div255(((PANEL_RGB >> shift) & 0xFF) * PANEL_ALPHA + 127) << shift;
    (div255(PANEL_ALPHA * 255 + 127) << 24) | ch(16) | ch(8) | ch(0)
}

impl<'a> Canvas<'a> {
    pub fn new(px: &'a mut [u32], w: i32, h: i32) -> Canvas<'a> {
        Canvas { px, w, h, clip: Rect { x: 0, y: 0, w, h } }
    }

    /// `r` limited to the clip rectangle and the buffer, as (x0, y0, x1, y1); None if nothing is left.
    fn clipped(&self, r: Rect) -> Option<(i32, i32, i32, i32)> {
        let c = self.clip;
        let (x0, y0) = (r.x.max(c.x).max(0), r.y.max(c.y).max(0));
        let (x1, y1) = ((r.x + r.w).min(c.x + c.w).min(self.w), (r.y + r.h).min(c.y + c.h).min(self.h));
        (x0 < x1 && y0 < y1).then_some((x0, y0, x1, y1))
    }

    /// Source-over a color at `alpha` (0..=255) onto a premultiplied buffer.
    pub fn blend(&mut self, x: i32, y: i32, rgb: u32, alpha: u32) {
        if !(self.clip.contains(x, y) && x >= 0 && y >= 0 && x < self.w && y < self.h) {
            return;
        }
        let dst = &mut self.px[(y * self.w + x) as usize];
        if alpha >= 255 {
            *dst = 0xFF00_0000 | (rgb & 0x00FF_FFFF);
            return;
        }
        let inv = 255 - alpha;
        let ch = |shift: u32| div255(((rgb >> shift) & 0xFF) * alpha + ((*dst >> shift) & 0xFF) * inv + 127).min(255) << shift;
        let a = div255(alpha * 255 + ((*dst >> 24) & 0xFF) * inv + 127);
        *dst = (a.min(255) << 24) | ch(16) | ch(8) | ch(0);
    }

    /// Source-over a rectangle. Clipped once, then whole rows at a time.
    pub fn fill(&mut self, r: Rect, rgb: u32, alpha: u32) {
        let Some((x0, y0, x1, y1)) = self.clipped(r) else { return };
        let alpha = alpha.min(255);
        if alpha == 255 {
            let px = 0xFF00_0000 | (rgb & 0x00FF_FFFF);
            for y in y0..y1 {
                self.px[(y * self.w + x0) as usize..(y * self.w + x1) as usize].fill(px);
            }
            return;
        }
        let inv = 255 - alpha;
        let (sr, sg, sb) = (((rgb >> 16) & 0xFF) * alpha, ((rgb >> 8) & 0xFF) * alpha, (rgb & 0xFF) * alpha);
        for y in y0..y1 {
            for d in &mut self.px[(y * self.w + x0) as usize..(y * self.w + x1) as usize] {
                let r = div255(sr + ((*d >> 16) & 0xFF) * inv + 127).min(255);
                let g = div255(sg + ((*d >> 8) & 0xFF) * inv + 127).min(255);
                let b = div255(sb + (*d & 0xFF) * inv + 127).min(255);
                let a = div255(alpha * 255 + ((*d >> 24) & 0xFF) * inv + 127).min(255);
                *d = (a << 24) | (r << 16) | (g << 8) | b;
            }
        }
    }

    /// Overwrite (not blend) a rectangle with one premultiplied pixel value.
    pub fn set_rect(&mut self, r: Rect, px: u32) {
        let Some((x0, y0, x1, y1)) = self.clipped(r) else { return };
        for y in y0..y1 {
            self.px[(y * self.w + x0) as usize..(y * self.w + x1) as usize].fill(px);
        }
    }

    pub fn outline(&mut self, r: Rect, rgb: u32) {
        self.fill(Rect { x: r.x, y: r.y, w: r.w, h: 1 }, rgb, 255);
        self.fill(Rect { x: r.x, y: r.y + r.h - 1, w: r.w, h: 1 }, rgb, 255);
        self.fill(Rect { x: r.x, y: r.y + 1, w: 1, h: r.h - 2 }, rgb, 255);
        self.fill(Rect { x: r.x + r.w - 1, y: r.y + 1, w: 1, h: r.h - 2 }, rgb, 255);
    }
}

/// Title gradient at `g` in [0,1): linear sRGB blend between the 7 evenly spaced stops.
pub fn gradient(g: f32) -> u32 {
    let g = g.rem_euclid(1.0);
    let pos = g * (GRADIENT.len() - 1) as f32;
    let i = (pos.floor() as usize).min(GRADIENT.len() - 2);
    let t = pos - i as f32;
    let (a, b) = (GRADIENT[i], GRADIENT[i + 1]);
    let mix = |shift: u32| {
        let (x, y) = (((a >> shift) & 0xFF) as f32, ((b >> shift) & 0xFF) as f32);
        ((x + (y - x) * t).round() as u32) << shift
    };
    mix(16) | mix(8) | mix(0)
}

/// Color of the title glyph at x offset `x` of a title `w` wide, `u` in [0,1) through the loop.
pub fn title_color(x: i32, w: i32, u: f32) -> u32 {
    gradient((x as f32 / (2.0 * w.max(1) as f32) + u).rem_euclid(1.0))
}

/// The checkmark, 7x5, drawn inside the 8x8 interior of a checkbox.
const CHECKMARK: [&str; 5] = ["....XX.", "X...XX.", "XX.XX..", ".XXX...", "..X...."];

fn arrow(c: &mut Canvas, x: i32, row_y: i32, open: bool, rgb: u32) {
    // 7px cell. open: 5x3 triangle pointing down, closed: 3x5 pointing right.
    let (shape, ox, oy): (&[&str], i32, i32) = if open { (&["XXXXX", ".XXX.", "..X.."], 1, 5) } else { (&["X..", "XX.", "XXX", "XX.", "X.."], 2, 4) };
    for (dy, line) in shape.iter().enumerate() {
        for (dx, ch) in line.bytes().enumerate() {
            if ch == b'X' {
                c.blend(x + ox + dx as i32, row_y + oy + dy as i32, rgb, 255);
            }
        }
    }
}

/// Repaint only the title row (the animation). Returns false if it is scrolled out of view.
pub fn paint_title(c: &mut Canvas, scroll: i32, text: &dyn TextBackend, u: f32) -> bool {
    let y = BORDER + PAD_TOP - scroll;
    let r = Rect { x: CONTENT_X, y, w: CONTENT_W, h: TITLE_H };
    if y + TITLE_H <= BORDER || y >= c.h - BORDER {
        return false;
    }
    let saved = c.clip;
    c.clip = Rect { x: BORDER, y: BORDER, w: c.w - 2 * BORDER, h: c.h - 2 * BORDER };
    c.set_rect(r, panel_px());
    let cell = text.cell(Font::Title);
    let total = TITLE.chars().count() as i32 * cell;
    for (i, ch) in TITLE.chars().enumerate() {
        let x = i as i32 * cell;
        let mut buf = [0u8; 4];
        text.draw(c, CONTENT_X + x, y, TITLE_H, ch.encode_utf8(&mut buf), title_color(x, total, u), Font::Title);
    }
    // "[Dev]" and the build time follow the name, in clay orange.
    let body = text.cell(Font::Body);
    let dev_x = CONTENT_X + total + body;
    text.draw(c, dev_x, y, TITLE_H, DEV_TAG, COLOR_DEV, Font::Body);
    text.draw(c, dev_x + DEV_TAG.chars().count() as i32 * body + body, y, TITLE_H, built(), COLOR_DEV, Font::Small);
    c.clip = saved;
    true
}

/// What was last drawn into the buffer, so the next draw can redo only what changed.
#[derive(Clone, Debug, PartialEq)]
pub struct Painted {
    items: Vec<Item>,
    bodies: Vec<(i32, i32, i32)>,
    hover: Option<Action>,
    focus: Option<Action>,
    scroll: i32,
    h: i32,
    accent: u32,
    crosshair: crate::crosshair::Crosshair,
    update: Status,
}

/// Which part of the window a draw changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Damage {
    None,
    /// everything (first draw, or the size, scroll or row layout changed)
    Full,
    /// the bounding box of the rows that changed
    Rect(Rect),
}

fn interior(c: &Canvas) -> Rect {
    Rect { x: BORDER, y: BORDER, w: c.w - 2 * BORDER, h: c.h - 2 * BORDER }
}

/// Draw the menu, but only the rows that differ from `prev` when the structure is unchanged.
/// A hover or focus move, a slider drag, or a value change redraws just those rows.
pub fn paint_update(c: &mut Canvas, prev: &mut Option<Painted>, layout: &Layout, cfg: &Config, menu: &Menu, text: &dyn TextBackend, accent: u32, u: f32) -> Damage {
    let now = Painted {
        items: layout.items.clone(),
        bodies: layout.bodies.clone(),
        hover: menu.hover(),
        focus: menu.focus_ring(),
        scroll: menu.scroll(),
        h: c.h,
        accent,
        crosshair: cfg.profile(menu.editing).clone(),
        update: menu.update_status().clone(),
    };
    let same_structure = prev.as_ref().is_some_and(|p| {
        p.h == now.h && p.scroll == now.scroll && p.accent == accent && p.update == now.update && p.bodies == now.bodies && p.items.len() == now.items.len() && p.items.iter().zip(&now.items).all(|(a, b)| a.action == b.action && a.rect == b.rect)
    });
    if !same_structure {
        paint(c, layout, cfg, menu, text, accent, u);
        *prev = Some(now);
        return Damage::Full;
    }
    let old = prev.take().expect("checked above");
    let sy = -menu.scroll();
    let mut bounds: Option<Rect> = None;
    c.clip = interior(c);
    for (i, item) in now.items.iter().enumerate() {
        let (was, is) = (&old.items[i], item);
        let status = |hover: Option<Action>, focus: Option<Action>, it: &Item| (it.action.is_some() && it.action == hover, it.action.is_some() && it.action == focus);
        let changed = was.kind != is.kind || status(old.hover, old.focus, was) != status(now.hover, now.focus, is) || (matches!(is.kind, ItemKind::Preview) && old.crosshair != now.crosshair);
        if !changed {
            continue;
        }
        let r = Rect { x: item.rect.x, y: item.rect.y + sy, w: item.rect.w, h: item.rect.h };
        c.set_rect(r, panel_px());
        draw_item(c, item, sy, now.hover, now.focus, cfg, menu, text, accent);
        let vis = Rect { x: r.x, y: r.y.max(BORDER), w: r.w, h: (r.y + r.h).min(c.h - BORDER) - r.y.max(BORDER) };
        if vis.h > 0 {
            bounds = Some(match bounds {
                None => vis,
                Some(b) => {
                    let (x0, y0, x1, y1) = (b.x.min(vis.x), b.y.min(vis.y), (b.x + b.w).max(vis.x + vis.w), (b.y + b.h).max(vis.y + vis.h));
                    Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
                }
            });
        }
    }
    *prev = Some(now);
    bounds.map_or(Damage::None, Damage::Rect)
}

/// Everything except the rows: panel, border, title, credit, rules.
fn paint_chrome(c: &mut Canvas, layout: &Layout, menu: &Menu, text: &dyn TextBackend, u: f32) {
    let (w, h) = (c.w, c.h);
    c.clip = Rect { x: 0, y: 0, w, h };
    c.px[..(w * h) as usize].fill(panel_px());
    c.outline(Rect { x: 0, y: 0, w, h }, COLOR_BORDER);
    c.clip = interior(c);
    let sy = -menu.scroll();

    paint_title(c, menu.scroll(), text, u);
    let (status, status_color) = status_line(menu.update_status());
    text.draw(c, CONTENT_X, BORDER + PAD_TOP + TITLE_H + sy, UPDATE_H, &status, status_color, Font::Small);
    text.draw(c, CONTENT_X, BORDER + PAD_TOP + TITLE_H + UPDATE_H + sy, CREDIT_H, CREDIT, COLOR_LABEL, Font::Small);

    for &(x, y0, y1) in &layout.bodies {
        c.fill(Rect { x, y: y0 + sy, w: 1, h: y1 - y0 }, COLOR_HAIRLINE, 255);
    }
}

/// The line under the title: what the updater is doing, in the same small type as the build time.
pub fn status_line(s: &Status) -> (String, u32) {
    match s {
        Status::Checking => ("checking for updates".to_string(), COLOR_LABEL),
        Status::UpToDate => ("up to date".to_string(), COLOR_OK),
        Status::Installed(v) => (format!("v{v} installed · restart to use"), COLOR_WARN),
        Status::Failed => ("update check failed".to_string(), COLOR_LABEL),
        Status::Off => ("auto-update off".to_string(), COLOR_LABEL),
    }
}

/// Draw the whole menu from scratch.
pub fn paint(c: &mut Canvas, layout: &Layout, cfg: &Config, menu: &Menu, text: &dyn TextBackend, accent: u32, u: f32) {
    let (w, h) = (c.w, c.h);
    paint_chrome(c, layout, menu, text, u);
    let sy = -menu.scroll();
    for item in &layout.items {
        if item.rect.y + sy + item.rect.h < 0 || item.rect.y + sy > h {
            continue;
        }
        draw_item(c, item, sy, menu.hover(), menu.focus_ring(), cfg, menu, text, accent);
    }

    c.clip = Rect { x: 0, y: 0, w, h };
    let max_scroll = layout.height - h;
    if max_scroll > 0 {
        let thumb_h = ((h * h) / layout.height).max(8);
        let y = BORDER + (menu.scroll() * (h - 2 * BORDER - thumb_h) / max_scroll).max(0);
        c.fill(Rect { x: w - BORDER - 4, y, w: 4, h: thumb_h }, COLOR_BORDER, 255);
    }
}

/// One row, drawn at its position shifted by `sy` (the scroll).
fn draw_item(c: &mut Canvas, item: &Item, sy: i32, hover: Option<Action>, focus: Option<Action>, cfg: &Config, menu: &Menu, text: &dyn TextBackend, accent: u32) {
    let cell = text.cell(Font::Body);
    {
        let r = Rect { x: item.rect.x, y: item.rect.y + sy, w: item.rect.w, h: item.rect.h };
        let hovered = item.action.is_some() && item.action == hover;
        let focused = item.action.is_some() && item.action == focus;
        if hovered {
            c.fill(r, 0xFFFFFF, HOVER_ALPHA);
        }
        match &item.kind {
            ItemKind::Header { title, open } => {
                let color = if item.action == Some(Action::Header(Section::Experimental)) { COLOR_WARN } else { accent };
                arrow(c, r.x, r.y, *open, color);
                text.draw(c, r.x + 7 + cell, r.y, ROW_H, title, color, Font::Body);
            }
            ItemKind::GroupHead { title, open } => {
                arrow(c, r.x, r.y, *open, COLOR_TEXT);
                text.draw(c, r.x + 7 + cell, r.y, ROW_H, title, COLOR_TEXT, Font::Body);
            }
            ItemKind::Check { label, on } => {
                // 10x10 box (the swatch footprint); a checkmark inside when on.
                c.outline(Rect { x: r.x, y: r.y + 2, w: 10, h: 10 }, COLOR_TEXT);
                if *on {
                    for (dy, line) in CHECKMARK.iter().enumerate() {
                        for (dx, ch) in line.bytes().enumerate() {
                            if ch == b'X' {
                                c.blend(r.x + 1 + dx as i32, r.y + 4 + dy as i32, COLOR_TEXT, 255);
                            }
                        }
                    }
                }
                text.draw(c, r.x + 10 + 4, r.y, ROW_H, label, COLOR_TEXT, Font::Body);
            }
            ItemKind::Cycle { label, value, swatch } => {
                let mut x = r.x;
                text.draw(c, x, r.y, ROW_H, label, COLOR_LABEL, Font::Body);
                x += label.chars().count() as i32 * cell + 4;
                if let Some(rgb) = swatch {
                    c.outline(Rect { x, y: r.y + 2, w: 10, h: 10 }, COLOR_TEXT);
                    c.fill(Rect { x: x + 1, y: r.y + 3, w: 8, h: 8 }, *rgb, 255);
                    x += 10 + 4;
                }
                text.draw(c, x, r.y, ROW_H, value, COLOR_TEXT, Font::Body);
            }
            ItemKind::Pick { text: t, active } => {
                text.draw(c, r.x, r.y, ROW_H, t, if *active { COLOR_ACTIVE } else { COLOR_LABEL }, Font::Body);
            }
            ItemKind::Slider { label, min, max, value, text: t, .. } => {
                text.draw(c, r.x, r.y, ROW_H, label_with_colon(label).as_str(), COLOR_LABEL, Font::Body);
                let tx = item.track_x();
                let mid = r.y + ROW_H / 2;
                c.fill(Rect { x: tx, y: mid, w: SLIDER_W, h: 1 }, COLOR_LABEL, 255);
                c.fill(Rect { x: tx + thumb_x(*value, *min, *max), y: mid - 4, w: THUMB_W, h: 9 }, COLOR_TEXT, 255);
                let vx = tx + SLIDER_W + SLIDER_GAP + VALUE_W - t.chars().count() as i32 * cell;
                text.draw(c, vx, r.y, ROW_H, t, COLOR_TEXT, Font::Body);
            }
            ItemKind::Link { text: t } => {
                text.draw(c, r.x, r.y, ROW_H, t, COLOR_LABEL, Font::Body);
                c.fill(Rect { x: r.x, y: r.y + ROW_H - 2, w: r.w, h: 1 }, COLOR_LABEL, 255);
            }
            ItemKind::Export { lines } => {
                for (i, line) in lines.iter().enumerate() {
                    text.draw(c, r.x, r.y + 2 + i as i32 * EXPORT_LINE_H, EXPORT_LINE_H, line, COLOR_LABEL, Font::Small);
                }
            }
            ItemKind::Preview => paint_preview(c, r, cfg.profile(menu.editing)),
            ItemKind::Danger { text: t } => {
                // Like a section header without the arrow: flush left, and the same hover tint as every row.
                text.draw(c, r.x, r.y, ROW_H, t, COLOR_DANGER, Font::Body);
            }
        }
        if focused {
            let ring = match item.kind {
                ItemKind::Slider { .. } => Rect { x: item.track_x(), y: r.y, w: SLIDER_W, h: r.h },
                _ => r,
            };
            c.outline(ring, COLOR_TEXT);
        }
    }
}

fn label_with_colon(label: &str) -> String {
    format!("{label}:")
}

/// A premultiplied ARGB pixel laid over an opaque backdrop, the way the compositor does it:
/// `src + backdrop * (1 - alpha)`. A Glow pixel (alpha 0, color in the channels) therefore adds light.
fn over(src: u32, backdrop: u32) -> u32 {
    let a = src >> 24;
    let ch = |s: u32| (((src >> s) & 0xFF) + (((backdrop >> s) & 0xFF) * (255 - a) + 127) / 255).min(255) << s;
    ch(16) | ch(8) | ch(0)
}

/// What a lens blend does to the backdrop behind it (the same math as the magnifier's color matrix).
fn lens_preview(effect: crate::crosshair::Blend, backdrop: u32) -> u32 {
    use crate::crosshair::Blend;
    let (r, g, b) = (((backdrop >> 16) & 0xFF) as f32, ((backdrop >> 8) & 0xFF) as f32, (backdrop & 0xFF) as f32);
    let (r2, g2, b2) = match effect {
        Blend::Invert => (255.0 - r, 255.0 - g, 255.0 - b),
        Blend::Contrast => (r * 1.8 - 102.0, g * 1.8 - 102.0, b * 1.8 - 102.0),
        _ => {
            let y = 0.299 * r + 0.587 * g + 0.114 * b;
            (y, y, y)
        }
    };
    let q = |v: f32| v.round().clamp(0.0, 255.0) as u32;
    (q(r2) << 16) | (q(g2) << 8) | q(b2)
}

/// The crosshair as the overlay draws it, laid over a light, a mid and a dark backdrop: layer and
/// crosshair opacity, outline and Glow included, lens layers showing what they do to each backdrop.
/// (Auto contrast, input states and the hue cycle are live and are not shown here.)
fn paint_preview(c: &mut Canvas, r: Rect, ch: &crate::crosshair::Crosshair) {
    use crate::crosshair::{Cell, N};
    c.fill(r, PREVIEW_BG, 255);
    c.fill(Rect { x: r.x, y: r.y, w: 55, h: r.h }, PREVIEW_LIGHT, 255);
    c.fill(Rect { x: r.x + 110, y: r.y, w: 55, h: r.h }, PREVIEW_DARK, 255);
    let drawn = ch.premultiplied_hue(1.0, [0; 4]);
    let cells = ch.render().cell;
    for gy in 0..N {
        for gx in 0..N {
            let i = gy * N + gx;
            // the three backdrops are 11 cells wide each
            let backdrop = match gx / 11 {
                0 => PREVIEW_LIGHT,
                1 => PREVIEW_BG,
                _ => PREVIEW_DARK,
            };
            let rgb = match cells[i] {
                Cell::Empty => continue,
                Cell::Lens(effect) => lens_preview(effect, backdrop),
                _ => over(drawn[i], backdrop),
            };
            c.fill(Rect { x: r.x + gx as i32 * 5, y: r.y + gy as i32 * 5, w: 5, h: 5 }, rgb, 255);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::cell::Cell;

    /// Draws every glyph as a solid cell, and counts calls.
    struct Boxes {
        calls: Cell<usize>,
    }

    impl TextBackend for Boxes {
        fn cell(&self, font: Font) -> i32 {
            match font {
                Font::Title => 7,
                Font::Body => 6,
                Font::Small => 5,
            }
        }
        fn draw(&self, c: &mut Canvas, x: i32, y: i32, line_h: i32, text: &str, rgb: u32, font: Font) {
            self.calls.set(self.calls.get() + 1);
            let cell = self.cell(font);
            for (i, ch) in text.chars().enumerate() {
                if ch != ' ' {
                    c.fill(Rect { x: x + i as i32 * cell, y: y + line_h / 2 - 2, w: cell - 1, h: 4 }, rgb, 255);
                }
            }
        }
    }

    fn render(menu: &mut Menu, cfg: &Config, u: f32) -> (Vec<u32>, i32, i32) {
        let layout = menu.layout(cfg);
        let h = layout.height.min(600);
        menu.clamp_scroll(layout.height);
        let mut px = vec![0u32; (HUD_W * h) as usize];
        let t = Boxes { calls: Cell::new(0) };
        let mut c = Canvas::new(&mut px, HUD_W, h);
        paint(&mut c, &layout, cfg, menu, &t, 0x4AA8FF, u);
        (px, HUD_W, h)
    }

    /// Colors to throw at the painters: random ones, the palette, and the awkward corners.
    fn many_colors() -> Vec<u32> {
        let mut seed = 0x1234_5678u32;
        let mut v: Vec<u32> = (0..3000)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                seed >> 8
            })
            .collect();
        v.extend(crate::crosshair::PALETTE.iter().map(|(_, c)| *c));
        v.extend([0x000000, 0xFFFFFF, 0x010101, 0xFEFEFE, 0x808080, 0x7F7F7F, 0xFF0000, 0x00FF00, 0x0000FF]);
        v
    }

    /// The whole menu drawn at its full height (no scrolling), so any row can be inspected.
    fn render_full(menu: &mut Menu, cfg: &Config) -> (Vec<u32>, i32, Layout) {
        let layout = menu.layout(cfg);
        let mut px = vec![0u32; (HUD_W * layout.height) as usize];
        let t = Boxes { calls: Cell::new(0) };
        let mut c = Canvas::new(&mut px, HUD_W, layout.height);
        paint(&mut c, &layout, cfg, menu, &t, 0x4AA8FF, 0.0);
        (px, HUD_W, layout)
    }

    /// Drag the hue, saturation and value sliders and paste hex colors, repainting only what changed each time
    /// (the way the real window does): the buffer must always equal a from-scratch paint of the same state.
    #[test]
    fn partial_repaints_never_leave_a_stale_color_swatch() {
        let (mut m, mut cfg) = (Menu::new(), Config::default());
        m.activate(&mut cfg, Action::Group(Group::Color), 1);
        m.activate(&mut cfg, Action::Header(Section::Preview), 1);
        let t = Boxes { calls: Cell::new(0) };
        let layout = m.layout(&cfg);
        let h = layout.height;
        let mut px = vec![0u32; (HUD_W * h) as usize];
        let mut prev: Option<Painted> = None;
        paint_update(&mut Canvas::new(&mut px, HUD_W, h), &mut prev, &layout, &cfg, &m, &t, 0x4AA8FF, 0.0);
        let mut steps = 0;
        for (i, c) in many_colors().into_iter().take(400).enumerate() {
            // alternate how the color changes: a slider drag, or a pasted hex
            if i % 2 == 0 {
                m.pasted(&mut cfg, Paste::Hex, &format!("#{c:06x}"));
            } else {
                let (hh, ss, vv) = (c as i32 % 356, (c >> 8) as i32 % 101, (c >> 16) as i32 % 101);
                for (x, v) in [(Lx::Hue, hh), (Lx::Sat, ss), (Lx::Val, vv)] {
                    m.set_slider_for_test(&mut cfg, Action::Lx(x), v);
                    let layout = m.layout(&cfg);
                    paint_update(&mut Canvas::new(&mut px, HUD_W, h), &mut prev, &layout, &cfg, &m, &t, 0x4AA8FF, 0.0);
                }
            }
            let layout = m.layout(&cfg);
            assert_eq!(layout.height, h, "the layout keeps its size, so every repaint is a partial one");
            paint_update(&mut Canvas::new(&mut px, HUD_W, h), &mut prev, &layout, &cfg, &m, &t, 0x4AA8FF, 0.0);
            let (fresh, _, _) = render_full(&mut m, &cfg);
            assert!(px == fresh, "step {i} (#{:06x}): the partial repaint differs from a full paint", cfg.hip.layers[0].color);
            steps += 1;
        }
        assert_eq!(steps, 400);
    }

    #[test]
    fn the_title_area_says_up_to_date_in_green_above_the_credit() {
        let (mut m, cfg) = (Menu::new(), Config::default());
        m.set_update(Status::UpToDate);
        let (px, w, _) = render(&mut m, &cfg, 0.0);
        let count = |y0: i32, y1: i32, ink: u32| (y0..y1).flat_map(|y| (0..w).map(move |x| (x, y))).filter(|(x, y)| px[(*y * w + *x) as usize] == 0xFF00_0000 | ink).count();
        let top = BORDER + PAD_TOP + TITLE_H;
        assert!(count(top, top + UPDATE_H, COLOR_OK) > 20, "green text in the status line");
        assert_eq!(count(top + UPDATE_H, top + UPDATE_H + CREDIT_H, COLOR_OK), 0, "the credit below is not green");
        assert!(count(top + UPDATE_H, top + UPDATE_H + CREDIT_H, COLOR_LABEL) > 20, "the credit stays gray, under the status");
        assert_eq!(status_line(&Status::UpToDate), ("up to date".to_string(), COLOR_OK));
        for s in [Status::Checking, Status::Failed, Status::Off] {
            assert_ne!(status_line(&s).1, COLOR_OK, "green is only for up to date: {s:?}");
        }
        assert!(status_line(&Status::Installed("0.2.9".to_string())).0.contains("0.2.9"), "an installed update names its version");
    }

    #[test]
    fn a_status_change_repaints_the_title_area_and_matches_a_full_paint() {
        let (mut m, cfg) = (Menu::new(), Config::default());
        let t = Boxes { calls: Cell::new(0) };
        let layout = m.layout(&cfg);
        let mut px = vec![0u32; (HUD_W * layout.height) as usize];
        let mut prev: Option<Painted> = None;
        paint_update(&mut Canvas::new(&mut px, HUD_W, layout.height), &mut prev, &layout, &cfg, &m, &t, 0x4AA8FF, 0.0);
        m.set_update(Status::UpToDate);
        let d = paint_update(&mut Canvas::new(&mut px, HUD_W, layout.height), &mut prev, &layout, &cfg, &m, &t, 0x4AA8FF, 0.0);
        assert_eq!(d, Damage::Full, "the title area is chrome, so a status change is a full repaint");
        let (fresh, _, _) = render_full(&mut m, &cfg);
        assert!(px == fresh);
    }

    #[test]
    fn the_force_quit_button_is_a_flush_left_red_label_in_a_header_row_with_the_normal_hover() {
        let (mut m, mut cfg) = (Menu::new(), Config::default());
        let (px, w, layout) = render_full(&mut m, &cfg);
        let item = layout.items.last().expect("rows");
        let r = item.rect;
        assert!(matches!(item.kind, ItemKind::Danger { .. }));
        let header = layout.items.iter().find(|i| i.action == Some(Action::Header(Section::Presets))).expect("a header").rect;
        assert_eq!((r.x, r.w, r.h), (header.x, header.w, header.h), "the same row as a dropdown header");
        let red = 0xFF00_0000 | COLOR_DANGER;
        let reds: Vec<(i32, i32)> = (r.y..r.y + r.h).flat_map(|y| (0..w).map(move |x| (x, y))).filter(|(x, y)| px[(*y * w + *x) as usize] == red).collect();
        assert!(reds.len() > 20, "the label is red ({} px)", reds.len());
        let (min_x, max_x) = (reds.iter().map(|p| p.0).min().unwrap(), reds.iter().map(|p| p.0).max().unwrap());
        assert_eq!(min_x, r.x, "flush left, where a header's arrow would start");
        assert!(max_x < r.x + 9 * 6, "the label ends with its nine letters, not centered across the row (ends at {max_x})");
        let probe = ((r.y + 1) * w + r.x + r.w - 5) as usize;
        assert_ne!(px[((r.y + r.h / 2) * w + r.x + r.w - 1) as usize], red, "no outline");
        // Hovering tints the whole row white, exactly as it does a dropdown header.
        let before = px[probe];
        m.mouse_move(&mut cfg, r.x + r.w - 5, r.y + 1, false);
        assert_eq!(m.hover(), Some(Action::ForceQuit));
        let (hovered, _, _) = render_full(&mut m, &cfg);
        assert!(((hovered[probe] >> 16) & 0xFF) > ((before >> 16) & 0xFF), "the row brightens on hover: #{before:08x} -> #{:08x}", hovered[probe]);
        let header_probe = ((header.y + 1) * w + header.x + header.w - 5) as usize;
        let (header_before, header_hover) = {
            let (mut m2, mut cfg2) = (Menu::new(), Config::default());
            let (a, _, _) = render_full(&mut m2, &cfg2);
            m2.mouse_move(&mut cfg2, header.x + header.w - 5, header.y + 1, false);
            let (b, _, _) = render_full(&mut m2, &cfg2);
            (a[header_probe], b[header_probe])
        };
        assert_eq!(hovered[probe].wrapping_sub(before), header_hover.wrapping_sub(header_before), "the very same tint");
    }

    #[test]
    fn the_color_swatch_paints_exactly_the_hex() {
        let (mut m, mut cfg) = (Menu::new(), Config::default());
        for c in many_colors() {
            cfg.hip.layers[0].color = c;
            let (px, w, layout) = render_full(&mut m, &cfg);
            let row = layout.items.iter().find(|i| i.action == Some(Action::LayerColor)).expect("Color row").rect;
            // label "Color:" is 6 cells of 6px, then 4px, then the 10x10 box with an 8x8 fill
            let (x, y) = (row.x + 6 * 6 + 4 + 5, row.y + 7);
            assert_eq!(px[(y * w + x) as usize], 0xFF00_0000 | c, "swatch for #{c:06x}");
        }
    }

    /// The preview pixel for a crosshair cell, found by painting just the Preview row.
    fn preview_pixel(ch: &crate::crosshair::Crosshair, gx: usize, gy: usize) -> u32 {
        let r = Rect { x: 0, y: 0, w: 165, h: 165 };
        let mut px = vec![0u32; 165 * 165];
        paint_preview(&mut Canvas::new(&mut px, 165, 165), r, ch);
        px[(gy * 5 + 2) * 165 + gx * 5 + 2] & 0xFFFFFF
    }

    #[test]
    fn the_preview_matches_what_the_overlay_draws_for_opacity_glow_and_lenses() {
        use crate::crosshair::{Blend, Crosshair, Kind, Layer};
        let dot = |color: u32, tune: &dyn Fn(&mut Layer)| {
            let mut l = Layer::new(Kind::Dot, color);
            l.reach = 1;
            tune(&mut l);
            Crosshair { layers: vec![l], outline: 0, opacity: 10 }
        };
        // grid cells whose backdrop is the light third (x 5), the mid third (x 16) and the dark third (x 27)
        // are reached with a horizontal offset of the 1-cell dot
        let at = |ch: &mut Crosshair, gx: i32| ch.layers[0].ox = gx - 16;
        // Half-opacity white over each backdrop is the average of the two.
        let mut half = dot(0xFFFFFF, &|l| l.opacity = 5);
        for (gx, bg) in [(5, PREVIEW_LIGHT), (16, PREVIEW_BG), (27, PREVIEW_DARK)] {
            at(&mut half, gx);
            let want = |s: u32| ((0xFF + ((bg >> s) & 0xFF) + 1) / 2) << s;
            let got = preview_pixel(&half, gx as usize, 16);
            for s in [16, 8, 0] {
                assert!(((got >> s) & 0xFF) as i32 - ((want(s) >> s) & 0xFF) as i32 <= 1 && ((want(s) >> s) & 0xFF) as i32 - ((got >> s) & 0xFF) as i32 <= 1, "layer opacity over #{bg:06x}: got #{got:06x}");
            }
        }
        // Glow adds light to the backdrop instead of replacing it.
        let mut glow = dot(0x282828, &|l| l.blend = Blend::Glow);
        at(&mut glow, 16);
        assert_eq!(preview_pixel(&glow, 16, 16), 0x5A6070 + 0x282828, "glow on the mid backdrop");
        // A lens layer shows its effect on the backdrop behind it.
        let mut invert = dot(0xFFFFFF, &|l| l.blend = Blend::Invert);
        at(&mut invert, 5);
        assert_eq!(preview_pixel(&invert, 5, 16), 0xFFFFFF - PREVIEW_LIGHT, "invert over the light backdrop");
        let mut gray = dot(0xFFFFFF, &|l| l.blend = Blend::Gray);
        at(&mut gray, 16);
        let g = preview_pixel(&gray, 16, 16);
        assert!((g >> 16) == ((g >> 8) & 0xFF) && (g >> 8 & 0xFF) == (g & 0xFF), "gray is gray: #{g:06x}");
    }

    #[test]
    fn the_preview_paints_exactly_the_hex_at_full_opacity() {
        let (mut m, mut cfg) = (Menu::new(), Config::default());
        m.activate(&mut cfg, Action::Header(Section::Preview), 1);
        for c in many_colors() {
            for l in &mut cfg.hip.layers {
                l.color = c;
            }
            let (px, w, layout) = render_full(&mut m, &cfg);
            let r = layout.items.iter().find(|i| matches!(i.kind, ItemKind::Preview)).expect("Preview row").rect;
            let (x, y) = (r.x + 16 * 5 + 2, r.y + 16 * 5 + 2);
            assert_eq!(px[(y * w + x) as usize], 0xFF00_0000 | c, "preview center for #{c:06x}");
        }
    }

    #[test]
    fn the_utc_stamp_is_read_strictly_and_shown_in_local_time() {
        assert_eq!(parse_stamp("2026-10-30 04:07"), Some((2026, 10, 30, 4, 7)));
        for bad in ["", "2026-10-30", "2026-10-30T04:07", "2026/10/30 04:07", "2026-10-30 04:7", "abcd-ef-gh ij:kl", "2026-10-30 04:07 "] {
            assert_eq!(parse_stamp(bad), None, "{bad:?}");
        }
        #[cfg(windows)]
        {
            // Local time is UTC plus a whole number of 15 minute steps (-12h to +14h), whatever zone this machine is in.
            let local = utc_to_local("2026-06-15 12:00");
            let (_, _, d, h, mi) = parse_stamp(&local).expect("a stamp comes back");
            let minutes = ((d as i32 - 15) * 24 + h as i32 - 12) * 60 + mi as i32;
            assert!((-12 * 60..=14 * 60).contains(&minutes) && minutes % 15 == 0, "{local} is {minutes} minutes from UTC");
            assert_eq!(utc_to_local("not a time"), "not a time", "an unreadable stamp is shown as it is");
        }
    }

    #[test]
    fn gradient_hits_the_stops_and_loops() {
        assert_eq!(gradient(0.0), 0xFF5F5F);
        assert_eq!(gradient(1.0 / 6.0), 0xFFB627);
        assert_eq!(gradient(0.5), 0x6BFF8A);
        assert_eq!(gradient(1.0 / 6.0 * 5.0), 0xB388FF);
        assert_eq!(gradient(0.999999), gradient(-0.000001));
        assert_eq!(gradient(1.0), gradient(0.0), "first stop equals last");
        let mid = gradient(1.0 / 12.0);
        assert_eq!(mid, 0xFF8B43, "halfway between red and amber");
    }

    #[test]
    fn title_scrolls_and_loops_every_period() {
        let w = 6 * 7;
        assert_eq!(title_color(0, w, 0.0), title_color(0, w, 1.0));
        assert_eq!(title_color(10, w, 0.25), title_color(10, w, 1.25));
        assert_ne!(title_color(0, w, 0.0), title_color(0, w, 0.3), "colors move over time");
    }

    #[test]
    fn panel_is_premultiplied_and_border_opaque() {
        let (mut m, cfg) = (Menu::new(), Config::default());
        let (px, w, h) = render(&mut m, &cfg, 0.0);
        let inner = px[(40 * w + 150) as usize];
        assert_eq!(inner >> 24, PANEL_ALPHA, "panel alpha");
        assert!((inner >> 16) & 0xFF <= PANEL_ALPHA && (inner & 0xFF) <= PANEL_ALPHA, "premultiplied: channel never exceeds alpha");
        assert_eq!(px[0], 0xFF00_0000 | COLOR_BORDER, "corner is the opaque border");
        assert_eq!(px[((h - 1) * w + w - 1) as usize], 0xFF00_0000 | COLOR_BORDER);
        for p in &px {
            let a = p >> 24;
            assert!(((p >> 16) & 0xFF) <= a && ((p >> 8) & 0xFF) <= a && (p & 0xFF) <= a, "{p:#x} not premultiplied");
        }
    }

    #[test]
    fn nothing_paints_outside_the_window_with_everything_open() {
        let (mut m, cfg) = (Menu::new(), Config::default());
        let mut c = cfg.clone();
        for s in [Section::Presets, Section::Output, Section::Preview] {
            m.activate(&mut c, Action::Header(s), 1);
        }
        let (px, w, h) = render(&mut m, &cfg, 0.5);
        assert_eq!(px.len() as i32, w * h);
        assert!(h <= 600);
    }

    #[test]
    fn open_and_closed_arrows_are_drawn_as_bitmaps() {
        let mut px = vec![0u32; 16 * 16];
        let mut c = Canvas::new(&mut px, 16, 16);
        arrow(&mut c, 0, 0, true, 0xFFFFFF);
        let lit: Vec<(i32, i32)> = (0..16).flat_map(|y| (0..16).map(move |x| (x, y))).filter(|(x, y)| px[(*y * 16 + *x) as usize] != 0).collect();
        assert_eq!(lit.len(), 9, "5 + 3 + 1");
        assert_eq!(*lit.first().unwrap(), (1, 5));
        let mut px = vec![0u32; 16 * 16];
        let mut c = Canvas::new(&mut px, 16, 16);
        arrow(&mut c, 0, 0, false, 0xFFFFFF);
        assert_eq!(px.iter().filter(|p| **p != 0).count(), 1 + 2 + 3 + 2 + 1);
    }

    #[test]
    fn slider_thumb_and_track_land_where_the_spec_says() {
        let (mut m, cfg) = (Menu::new(), Config::default());
        let layout = m.layout(&cfg);
        let (px, w, _) = render(&mut m, &cfg, 0.0);
        let row = layout.items.iter().find(|i| i.action == Some(Action::Slider(Param::Rot))).unwrap();
        let tx = row.track_x();
        let mid = row.rect.y + ROW_H / 2;
        assert_eq!(px[(mid * w + tx + 20) as usize], 0xFF00_0000 | COLOR_LABEL, "1px label-colored track");
        assert_eq!(px[((mid - 1) * w + tx + 20) as usize] >> 24, PANEL_ALPHA, "no track above");
        assert_eq!(px[((mid - 4) * w + tx) as usize], 0xFF00_0000 | COLOR_TEXT, "thumb starts 4px above the track at min");
        assert_eq!(px[((mid + 4) * w + tx + 3) as usize], 0xFF00_0000 | COLOR_TEXT);
        assert_ne!(px[((mid + 5) * w + tx) as usize], 0xFF00_0000 | COLOR_TEXT, "thumb is 9px tall");
    }

    #[test]
    fn hover_and_focus_are_drawn() {
        let (mut m, mut cfg) = (Menu::new(), Config::default());
        let l = m.layout(&cfg);
        let t = l.items.iter().find(|i| i.action == Some(Action::LayerType)).unwrap().rect;
        let plain = render(&mut m, &cfg, 0.0).0;
        m.mouse_move(&mut cfg, t.x + 2, t.y + 2, false);
        let hovered = render(&mut m, &cfg, 0.0).0;
        let i = ((t.y + 12) * HUD_W + t.x + 150) as usize;
        assert_ne!(plain[i], hovered[i], "hover fill changes background pixels");
        m.key(&mut cfg, Key::Home);
        let focused = render(&mut m, &cfg, 0.0).0;
        let hdr = l.items[0].rect;
        assert_eq!(focused[(hdr.y * HUD_W + hdr.x) as usize], 0xFF00_0000 | COLOR_TEXT, "focus ring corner");
    }

    #[test]
    fn scrollbar_appears_only_when_content_overflows() {
        let (mut m, mut cfg) = (Menu::new(), Config::default());
        m.set_view(120, Metrics { body_cell: 6, small_cell: 5 });
        let layout = m.layout(&cfg);
        let h = layout.height.min(120);
        let mut px = vec![0u32; (HUD_W * h) as usize];
        let t = Boxes { calls: Cell::new(0) };
        paint(&mut Canvas::new(&mut px, HUD_W, h), &layout, &cfg, &m, &t, 0x4AA8FF, 0.0);
        assert_eq!(px[(10 * HUD_W + HUD_W - 3) as usize], 0xFF00_0000 | COLOR_BORDER, "4px thumb at the right edge");
        m.set_view(2000, Metrics { body_cell: 6, small_cell: 5 });
        let _ = &mut cfg;
        let (px, w, _) = render(&mut m, &cfg, 0.0);
        assert_ne!(px[(10 * w + w - 3) as usize], 0xFF00_0000 | COLOR_BORDER);
    }

    #[test]
    fn checkbox_shows_a_checkmark_not_a_filled_square() {
        let mut cfg = Config::default();
        let mut m = Menu::new();
        m.activate(&mut cfg, Action::Header(Section::Experimental), 1);
        let row = |on: bool, cfg: &mut Config, m: &mut Menu| {
            cfg.exp.show_on_capture = on;
            let layout = m.layout(cfg);
            let item = layout.items.iter().find(|i| i.action == Some(Action::Exp(Exp::CaptureShow))).unwrap().clone();
            let (px, w, _) = render(m, cfg, 0.0);
            (px, w, item.rect)
        };
        let (off, w, r) = row(false, &mut cfg, &mut m);
        let (on, _, _) = row(true, &mut cfg, &mut m);
        let at = |px: &Vec<u32>, dx: i32, dy: i32| px[((r.y + dy) * w + r.x + dx) as usize];
        let ink = 0xFF00_0000 | COLOR_TEXT;
        assert_eq!(at(&on, 0, 2), ink, "box outline is drawn");
        assert_eq!(at(&on, 1 + 2, 4 + 4), ink, "checkmark's bottom tip");
        assert_eq!(at(&on, 1 + 5, 4), ink, "checkmark's long arm");
        assert_ne!(at(&on, 5, 7), ink, "the middle of the box stays empty: it is not a filled square");
        assert_ne!(at(&off, 1 + 2, 4 + 4), ink, "unchecked box is empty");
        let ink_pixels = |px: &Vec<u32>| (0..10).flat_map(|dy| (0..10).map(move |dx| (dx, dy))).filter(|(dx, dy)| at(px, *dx, 2 + *dy) == ink).count();
        let mark_pixels: usize = CHECKMARK.iter().map(|l| l.bytes().filter(|b| *b == b'X').count()).sum();
        assert_eq!(ink_pixels(&on) - ink_pixels(&off), mark_pixels, "exactly the checkmark's pixels were added, not a block");
        assert!(mark_pixels < 20);
    }

    #[test]
    fn title_row_carries_the_dev_tag_and_build_time() {
        let built = built();
        let bytes: Vec<char> = built.chars().collect();
        assert_eq!(bytes.len(), 16, "{built}");
        assert!(bytes.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == '-',
            10 => *c == ' ',
            13 => *c == ':',
            _ => c.is_ascii_digit(),
        }), "{built} is YYYY-MM-DD HH:MM");
        assert!(built.as_bytes()[0] == b'2', "a real year, stamped by the build script");
        assert!(parse_stamp(BUILT_UTC).is_some(), "the build script's UTC stamp parses: {BUILT_UTC}");
        let (mut m, cfg) = (Menu::new(), Config::default());
        let (px, w, _) = render(&mut m, &cfg, 0.0);
        let tag_x = CONTENT_X + 6 * 7 + 6;
        let dev_ink = 0xFF00_0000 | COLOR_DEV;
        let found = (BORDER + PAD_TOP..BORDER + PAD_TOP + TITLE_H).flat_map(|y| (tag_x..tag_x + 60).map(move |x| (x, y))).any(|(x, y)| px[(y * w + x) as usize] == dev_ink);
        assert!(found, "clay-orange pixels where [Dev] goes");
        let all_clay: Vec<i32> = (BORDER + PAD_TOP..BORDER + PAD_TOP + TITLE_H).flat_map(|y| (0..w).map(move |x| (x, y))).filter(|(x, y)| px[(*y * w + *x) as usize] == dev_ink).map(|(x, _)| x).collect();
        assert!(all_clay.iter().max().unwrap() < &(CONTENT_X + CONTENT_W), "tag and time fit the content width");
    }

    /// The original per-pixel source-over, kept as the reference the fast paths must match.
    fn reference_blend(dst: u32, rgb: u32, alpha: u32) -> u32 {
        let ch = |shift: u32| {
            let s = ((rgb >> shift) & 0xFF) * alpha;
            let d = ((dst >> shift) & 0xFF) * (255 - alpha);
            ((s + d + 127) / 255).min(255) << shift
        };
        let a = (alpha * 255 + ((dst >> 24) & 0xFF) * (255 - alpha) + 127) / 255;
        (a.min(255) << 24) | ch(16) | ch(8) | ch(0)
    }

    #[test]
    fn div255_is_exact_over_every_value_it_is_used_with() {
        for x in 0..=65152u32 {
            assert_eq!(div255(x), x / 255, "{x}");
        }
    }

    #[test]
    fn the_panel_pixel_is_what_the_old_clear_then_fill_produced() {
        assert_eq!(panel_px(), reference_blend(0, PANEL_RGB, PANEL_ALPHA));
    }

    #[test]
    fn fast_fill_and_blend_match_the_old_per_pixel_math() {
        let mut seed = 0x1234_5678u32;
        let mut rnd = |n: u32| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) % n
        };
        for case in 0..400 {
            let (w, h) = (12 + rnd(20) as i32, 9 + rnd(12) as i32);
            let start: Vec<u32> = (0..w * h)
                .map(|_| {
                    let a = rnd(256);
                    (a << 24) | (rnd(a + 1) << 16) | (rnd(a + 1) << 8) | rnd(a + 1)
                })
                .collect();
            let rect = Rect { x: rnd(30) as i32 - 8, y: rnd(24) as i32 - 6, w: rnd(24) as i32, h: rnd(16) as i32 };
            let clip = Rect { x: rnd(6) as i32 - 2, y: rnd(5) as i32 - 2, w: rnd(30) as i32, h: rnd(20) as i32 };
            let rgb = rnd(0x100_0000);
            let alpha = [0, 1, 23, 100, 209, 254, 255][rnd(7) as usize];

            let mut fast = start.clone();
            {
                let mut c = Canvas::new(&mut fast, w, h);
                c.clip = clip;
                c.fill(rect, rgb, alpha);
            }
            let mut slow = start.clone();
            for y in 0..h {
                for x in 0..w {
                    if rect.contains(x, y) && clip.contains(x, y) {
                        let i = (y * w + x) as usize;
                        slow[i] = reference_blend(slow[i], rgb, alpha);
                    }
                }
            }
            assert_eq!(fast, slow, "fill, case {case}: rect {rect:?} clip {clip:?} rgb {rgb:06x} alpha {alpha}");

            let (px, py) = (rnd(w as u32 + 4) as i32 - 2, rnd(h as u32 + 4) as i32 - 2);
            let mut one = start.clone();
            {
                let mut c = Canvas::new(&mut one, w, h);
                c.clip = clip;
                c.blend(px, py, rgb, alpha);
            }
            let mut want = start.clone();
            if clip.contains(px, py) && px >= 0 && py >= 0 && px < w && py < h {
                let i = (py * w + px) as usize;
                want[i] = reference_blend(want[i], rgb, alpha);
            }
            assert_eq!(one, want, "blend, case {case}");
        }
    }

    /// Everything open, every feature on: the busiest menu.
    fn busiest() -> (Menu, Config) {
        let (mut m, mut cfg) = (Menu::new(), Config::default());
        for s in [Section::Presets, Section::Output, Section::Display, Section::Preview, Section::Experimental] {
            m.activate(&mut cfg, Action::Header(s), 1);
        }
        for g in [Group::Blend, Group::Dynamics, Group::Trace, Group::Zoom, Group::Auto] {
            m.activate(&mut cfg, Action::Group(g), 1);
        }
        cfg.exp.dynamics.enabled = true;
        cfg.exp.trace.enabled = true;
        cfg.exp.zoom.enabled = true;
        cfg.exp.auto.enabled = true;
        m.set_view(2000, Metrics { body_cell: 6, small_cell: 5 });
        (m, cfg)
    }

    /// Draw with `paint_update` into a buffer that keeps its contents between calls, then
    /// assert it equals a from-scratch `paint` of the same state. Returns what was damaged.
    fn update_and_compare(m: &mut Menu, cfg: &Config, live: &mut Vec<u32>, prev: &mut Option<Painted>, what: &str) -> Damage {
        let t = Boxes { calls: Cell::new(0) };
        let layout = m.layout(cfg);
        m.clamp_scroll(layout.height);
        let h = m.window_height(cfg);
        live.resize((HUD_W * h) as usize, 0);
        let damage = paint_update(&mut Canvas::new(&mut live[..], HUD_W, h), prev, &layout, cfg, m, &t, 0x4AA8FF, 0.0);
        let mut full = vec![0u32; (HUD_W * h) as usize];
        paint(&mut Canvas::new(&mut full, HUD_W, h), &layout, cfg, m, &t, 0x4AA8FF, 0.0);
        assert!(live[..] == full[..], "partial redraw differs from a full redraw after: {what} (damage {damage:?})");
        damage
    }

    #[test]
    fn redrawing_only_the_changed_rows_gives_exactly_the_full_redraw() {
        let (mut m, mut cfg) = busiest();
        let (mut live, mut prev) = (Vec::new(), None);
        assert_eq!(update_and_compare(&mut m, &cfg, &mut live, &mut prev, "first draw"), Damage::Full);
        assert_eq!(update_and_compare(&mut m, &cfg, &mut live, &mut prev, "nothing changed"), Damage::None);

        // Hover every row in turn, top to bottom and back.
        let rows: Vec<(i32, i32)> = m.layout(&cfg).items.iter().filter(|i| i.action.is_some()).map(|i| (i.hit_rect().x + 2, i.hit_rect().y + 2)).collect();
        assert!(rows.len() > 50);
        for (n, (x, y)) in rows.iter().chain(rows.iter().rev()).enumerate() {
            m.mouse_move(&mut cfg, *x, *y, false);
            let d = update_and_compare(&mut m, &cfg, &mut live, &mut prev, &format!("hover step {n} at {x},{y}"));
            assert!(!matches!(d, Damage::Full), "a hover move must not redraw everything (step {n})");
        }
        m.mouse_leave();
        update_and_compare(&mut m, &cfg, &mut live, &mut prev, "mouse left");

        // Keyboard focus ring walking down the whole menu.
        for n in 0..rows.len() {
            m.key(&mut cfg, Key::Down);
            let d = update_and_compare(&mut m, &cfg, &mut live, &mut prev, &format!("focus step {n}"));
            assert!(!matches!(d, Damage::Full), "moving focus must not redraw everything (step {n})");
        }

        // Dragging a layer slider also redraws the preview, which depends on the crosshair.
        let row = m.layout(&cfg).items.iter().find(|i| i.action == Some(Action::Slider(Param::Rot))).unwrap().clone();
        let (x0, y) = (row.hit_rect().x, row.hit_rect().y + 3);
        m.mouse_down(&mut cfg, x0 + 2, y, false);
        update_and_compare(&mut m, &cfg, &mut live, &mut prev, "slider pressed");
        for step in 0..30 {
            m.mouse_move(&mut cfg, x0 + step * 3, y, true);
            let d = update_and_compare(&mut m, &cfg, &mut live, &mut prev, &format!("slider drag {step}"));
            assert!(!matches!(d, Damage::Full), "dragging a slider redraws rows, not the window (step {step})");
        }
        m.mouse_up(&mut cfg, x0 + 88, y, false);
        update_and_compare(&mut m, &cfg, &mut live, &mut prev, "slider released");

        // Value changes, toggles and reshapes of the layout.
        for (what, action) in [
            ("layer color", Action::LayerColor),
            ("layer type", Action::LayerType),
            ("outline", Action::Outline),
            ("trace color", Action::Exp(Exp::TraceColor)),
            ("speed color check", Action::Exp(Exp::TraceSpeedColor)),
            ("blend", Action::Exp(Exp::Blend)),
            ("scale", Action::Disp(Disp::Scale)),
            ("add layer", Action::AddLayer),
            ("del layer", Action::DelLayer),
            ("trace off", Action::Exp(Exp::TraceEnable)),
            ("trace on", Action::Exp(Exp::TraceEnable)),
            ("close a group", Action::Group(Group::Zoom)),
            ("open it again", Action::Group(Group::Zoom)),
            ("switch profile", Action::Editing),
            ("copy to ads", Action::CopyHipToAds),
            ("close Experimental", Action::Header(Section::Experimental)),
            ("open Experimental", Action::Header(Section::Experimental)),
        ] {
            m.activate(&mut cfg, action, 1);
            update_and_compare(&mut m, &cfg, &mut live, &mut prev, what);
        }

        // Scrolling moves everything: a full redraw, and it must still match.
        m.set_view(200, Metrics { body_cell: 6, small_cell: 5 });
        update_and_compare(&mut m, &cfg, &mut live, &mut prev, "view made small");
        for notches in [-1, -1, -3, 2, 1, -40, 40] {
            m.wheel(&cfg, notches);
            update_and_compare(&mut m, &cfg, &mut live, &mut prev, &format!("wheel {notches}"));
        }
        m.mouse_move(&mut cfg, 60, 60, false);
        update_and_compare(&mut m, &cfg, &mut live, &mut prev, "hover while scrolled");
        m.key(&mut cfg, Key::End);
        update_and_compare(&mut m, &cfg, &mut live, &mut prev, "focus the last row while scrolled");
    }

    #[test]
    fn a_hover_between_neighbouring_rows_damages_only_those_rows() {
        let (mut m, mut cfg) = busiest();
        let (mut live, mut prev) = (Vec::new(), None);
        update_and_compare(&mut m, &cfg, &mut live, &mut prev, "first draw");
        let l = m.layout(&cfg);
        let a = l.items.iter().find(|i| i.action == Some(Action::LayerType)).unwrap().rect;
        let b = l.items.iter().find(|i| i.action == Some(Action::LayerColor)).unwrap().rect;
        m.mouse_move(&mut cfg, a.x + 2, a.y + 2, false);
        let first = update_and_compare(&mut m, &cfg, &mut live, &mut prev, "hover A");
        assert!(matches!(first, Damage::Rect(r) if r.h <= ROW_H), "one row: {first:?}");
        m.mouse_move(&mut cfg, b.x + 2, b.y + 2, false);
        let second = update_and_compare(&mut m, &cfg, &mut live, &mut prev, "hover B");
        match second {
            Damage::Rect(r) => assert!(r.h <= 2 * ROW_H && r.y >= a.y.min(b.y) && r.y + r.h <= a.y.max(b.y) + ROW_H, "old and new row only: {r:?}"),
            other => panic!("expected a small rectangle, got {other:?}"),
        }
        m.mouse_move(&mut cfg, b.x + 4, b.y + 4, false);
        assert_eq!(update_and_compare(&mut m, &cfg, &mut live, &mut prev, "moving within the same row"), Damage::None, "same row: nothing to draw");
    }

    #[test]
    fn title_repaint_touches_only_the_title_row() {
        let (mut m, cfg) = (Menu::new(), Config::default());
        let (mut px, w, h) = render(&mut m, &cfg, 0.0);
        let before = px.clone();
        let t = Boxes { calls: Cell::new(0) };
        assert!(paint_title(&mut Canvas::new(&mut px, w, h), 0, &t, 0.5));
        for y in 0..h {
            for x in 0..w {
                if before[(y * w + x) as usize] != px[(y * w + x) as usize] {
                    assert!(y >= BORDER + PAD_TOP && y < BORDER + PAD_TOP + TITLE_H && x >= CONTENT_X, "changed pixel {x},{y} outside the title");
                }
            }
        }
        assert_ne!(before, px, "colors changed");
        assert!(!paint_title(&mut Canvas::new(&mut px, w, h), 500, &t, 0.0), "scrolled out of view");
    }
}
