//! Pure crosshair engine: layers in, a 33x33 grid of whole pixels out.
//!
//! No windows, no I/O, no floats leaking past `raster`. Every pixel is either
//! fully on or fully off, so the result is machine sharp; opacity is applied
//! uniformly afterwards in `premultiplied`.

pub const N: usize = 33;
pub const C: i32 = 16;
pub const MAX_LAYERS: usize = 8;

/// Tolerance for pixels that sit exactly on a shape boundary, so mirrored
/// angles (120 vs 240 degrees) agree instead of depending on last-bit sin/cos noise.
const EPS: f64 = 1e-9;
/// The same for angles in degrees (arc ends).
const EPS_DEG: f64 = 1e-6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Spokes,
    Ring,
    Diamond,
    Bracket,
    Dot,
    Chevron,
    Polygon,
    Arc,
    Ladder,
    Line,
    Disc,
}

impl Kind {
    pub const ALL: [Kind; 11] = [Kind::Spokes, Kind::Ring, Kind::Diamond, Kind::Bracket, Kind::Dot, Kind::Chevron, Kind::Polygon, Kind::Arc, Kind::Ladder, Kind::Line, Kind::Disc];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Spokes => "Spokes",
            Kind::Ring => "Ring",
            Kind::Diamond => "Diamond",
            Kind::Bracket => "Bracket",
            Kind::Dot => "Dot",
            Kind::Chevron => "Chevron",
            Kind::Polygon => "Polygon",
            Kind::Arc => "Arc",
            Kind::Ladder => "Ladder",
            Kind::Line => "Line",
            Kind::Disc => "Disc",
        }
    }

    pub fn tag(self) -> &'static str {
        match self {
            Kind::Spokes => "spk",
            Kind::Ring => "rng",
            Kind::Diamond => "dia",
            Kind::Bracket => "brk",
            Kind::Dot => "dot",
            Kind::Chevron => "chv",
            Kind::Polygon => "ply",
            Kind::Arc => "arc",
            Kind::Ladder => "lad",
            Kind::Line => "lin",
            Kind::Disc => "dsc",
        }
    }

    pub fn from_tag(tag: &str) -> Option<Kind> {
        match tag {
            "spo" | "spk" | "spokes" => Some(Kind::Spokes),
            "rin" | "rng" | "ring" => Some(Kind::Ring),
            "dia" | "diamond" => Some(Kind::Diamond),
            "bra" | "brk" | "bracket" => Some(Kind::Bracket),
            "dot" => Some(Kind::Dot),
            "chv" | "chevron" => Some(Kind::Chevron),
            "ply" | "polygon" => Some(Kind::Polygon),
            "arc" => Some(Kind::Arc),
            "lad" | "ladder" => Some(Kind::Ladder),
            "lin" | "line" => Some(Kind::Line),
            "dsc" | "disc" => Some(Kind::Disc),
            _ => None,
        }
    }
}

/// Which axes a layer is also reflected across (after its offset), so one shape can make a symmetric pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mirror {
    Off,
    Horizontal,
    Vertical,
    Both,
}

impl Mirror {
    pub const ALL: [Mirror; 4] = [Mirror::Off, Mirror::Horizontal, Mirror::Vertical, Mirror::Both];

    pub fn name(self) -> &'static str {
        match self {
            Mirror::Off => "Off",
            Mirror::Horizontal => "Horizontal",
            Mirror::Vertical => "Vertical",
            Mirror::Both => "Both",
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Mirror::Off => "",
            Mirror::Horizontal => "h",
            Mirror::Vertical => "v",
            Mirror::Both => "b",
        }
    }

    fn from_tag(tag: &str) -> Option<Mirror> {
        Mirror::ALL.into_iter().find(|m| !m.tag().is_empty() && m.tag() == tag)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layer {
    pub kind: Kind,
    /// 0xRRGGBB
    pub color: u32,
    /// spokes: length, ring/diamond/bracket: radius, dot: size
    pub reach: i32,
    /// spokes: gap from center, bracket: arm length
    pub gap: i32,
    /// stroke width, odd so shapes stay centered on the center pixel
    pub thick: i32,
    /// spokes: spoke count, ring: dash segments
    pub count: i32,
    /// degrees
    pub rot: i32,
    pub visible: bool,
    pub blend: Blend,
    /// 1..=10 tenths: this layer's own opacity, multiplied with the crosshair's
    pub opacity: u8,
    /// whole-pixel shift of the shape, -12..=12; the grid clips what leaves it
    pub ox: i32,
    pub oy: i32,
    pub mirror: Mirror,
    /// 0 keeps the color, 1..=3 turns its hue slowly, steadily or fast
    pub cycle: u8,
}

/// How a layer is combined with what is behind it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blend {
    /// ordinary alpha
    Normal,
    /// adds light to the backdrop
    Glow,
    /// lens: the backdrop shown inverted inside the layer's shape
    Invert,
    /// lens: the backdrop with boosted contrast
    Contrast,
    /// lens: the backdrop in grayscale
    Gray,
}

impl Blend {
    pub const ALL: [Blend; 5] = [Blend::Normal, Blend::Glow, Blend::Invert, Blend::Contrast, Blend::Gray];

    pub fn name(self) -> &'static str {
        match self {
            Blend::Normal => "Normal",
            Blend::Glow => "Glow",
            Blend::Invert => "Invert",
            Blend::Contrast => "Contrast",
            Blend::Gray => "Gray",
        }
    }

    pub fn tag(self) -> &'static str {
        match self {
            Blend::Normal => "n",
            Blend::Glow => "g",
            Blend::Invert => "i",
            Blend::Contrast => "c",
            Blend::Gray => "y",
        }
    }

    pub fn from_tag(tag: &str) -> Option<Blend> {
        Blend::ALL.into_iter().find(|b| b.tag() == tag)
    }
}

/// What one pixel of the merged crosshair is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cell {
    Empty,
    Normal,
    Glow,
    Outline,
    Lens(Blend),
}

/// The merged crosshair: a color, a blend, an opacity and a hue-cycle level per pixel.
pub struct Render {
    pub color: [u32; N * N],
    pub cell: [Cell; N * N],
    /// tenths of full opacity, 0 where the pixel is empty
    pub alpha: [u8; N * N],
    /// hue-cycle level of the layer that drew the pixel, 0 for a fixed color
    pub cycle: [u8; N * N],
}

/// A stroke only counts as inside the grid if its farthest point is closer than this.
const REACH_LIMIT: f64 = 17.0 - 1e-6;

impl Layer {
    pub fn new(kind: Kind, color: u32) -> Layer {
        let mut l = Layer { kind, color, reach: 6, gap: 4, thick: 1, count: 4, rot: 0, visible: true, blend: Blend::Normal, opacity: 10, ox: 0, oy: 0, mirror: Mirror::Off, cycle: 0 };
        match kind {
            Kind::Ring => {
                l.reach = 8;
                l.count = 1;
            }
            Kind::Chevron => {
                l.reach = 5;
                l.gap = 2;
                l.count = 1;
                l.rot = 270;
            }
            Kind::Polygon => {
                l.reach = 7;
                l.gap = 0;
                l.count = 5;
                l.rot = 270;
            }
            Kind::Arc => {
                l.reach = 8;
                l.count = 6;
                l.rot = 270;
            }
            Kind::Ladder => {
                l.reach = 4;
                l.gap = 3;
                l.count = 4;
                l.rot = 90;
            }
            Kind::Line => l.reach = 6,
            Kind::Disc => l.reach = 3,
            _ => {}
        }
        l.clamp();
        l
    }

    fn stroke(&self) -> f64 {
        (self.thick - 1) as f64 / 2.0 + 0.5
    }

    /// Farthest distance from the center any lit pixel of the (unshifted) shape can have, for the
    /// shapes whose size depends on several fields. The simple shapes are bounded by their ranges.
    fn extent(&self) -> f64 {
        let a = self.stroke();
        match self.kind {
            Kind::Chevron => self.segments().iter().flat_map(|s| [(s[0], s[1]), (s[2], s[3])]).map(|(x, y)| x.hypot(y)).fold(0.0, f64::max) + a,
            Kind::Ladder => ((self.gap * self.count) as f64 + a).hypot(self.reach as f64 + 0.5),
            Kind::Line => (self.reach as f64 + 0.5).hypot(a),
            _ => 0.0,
        }
    }

    fn fits(&self) -> bool {
        self.extent() < REACH_LIMIT
    }

    /// Largest value of a field (searching down from `hi`) that still fits the grid, other fields as they are.
    fn largest_fitting(&self, set: impl Fn(&mut Layer, i32), hi: i32, lo: i32) -> i32 {
        (lo..=hi)
            .rev()
            .find(|v| {
                let mut l = self.clone();
                set(&mut l, *v);
                l.fits()
            })
            .unwrap_or(lo)
    }

    /// Largest `reach` that keeps this layer inside the grid; what the Length/Arm slider offers.
    pub fn reach_max(&self) -> i32 {
        match self.kind {
            Kind::Chevron => self.largest_fitting(|l, v| l.reach = v, 12, 1),
            Kind::Ladder => self.largest_fitting(|l, v| l.reach = v, 12, 1),
            Kind::Line => self.largest_fitting(|l, v| l.reach = v, 15, 1),
            Kind::Spokes => 15.min(16 - self.gap),
            Kind::Ring | Kind::Diamond | Kind::Bracket | Kind::Polygon | Kind::Arc => 13,
            Kind::Dot => 9,
            Kind::Disc => 14,
        }
    }

    /// Largest `count` that keeps this layer inside the grid.
    pub fn count_max(&self) -> i32 {
        match self.kind {
            Kind::Chevron => self.largest_fitting(|l, v| l.count = v, 4, 1),
            Kind::Ladder => self.largest_fitting(|l, v| l.count = v, 8, 1),
            Kind::Spokes => 12,
            Kind::Polygon => 12,
            Kind::Arc => 24,
            _ => 16,
        }
    }

    /// Force every field into the range that keeps the shape inside the 33x33 grid.
    pub fn clamp(&mut self) {
        self.color &= 0x00FF_FFFF;
        self.thick = self.thick.clamp(1, 7);
        if self.thick % 2 == 0 {
            self.thick = (self.thick + 1).min(7);
        }
        self.rot = self.rot.rem_euclid(360);
        self.opacity = self.opacity.clamp(1, 10);
        self.ox = self.ox.clamp(-12, 12);
        self.oy = self.oy.clamp(-12, 12);
        self.cycle = self.cycle.min(3);
        match self.kind {
            Kind::Spokes => {
                self.count = self.count.clamp(1, 12);
                self.gap = self.gap.clamp(0, 10);
                self.reach = self.reach.clamp(1, 15).min(16 - self.gap);
            }
            Kind::Ring => {
                self.count = self.count.clamp(1, 16);
                self.reach = self.reach.clamp(2, 13);
            }
            Kind::Diamond => {
                self.count = self.count.clamp(1, 16);
                self.reach = self.reach.clamp(2, 13);
            }
            Kind::Bracket => {
                self.count = self.count.clamp(1, 16);
                self.gap = self.gap.clamp(1, 10);
                self.reach = self.reach.clamp(2, 13);
            }
            Kind::Dot => {
                self.count = self.count.clamp(1, 16);
                self.reach = self.reach.clamp(1, 9);
                if self.reach % 2 == 0 {
                    self.reach = (self.reach + 1).min(9);
                }
            }
            Kind::Chevron => {
                self.count = self.count.clamp(1, 4);
                self.gap = self.gap.clamp(0, 10);
                self.reach = self.reach.clamp(1, 12);
                while !self.fits() && self.count > 1 {
                    self.count -= 1;
                }
                while !self.fits() && self.reach > 1 {
                    self.reach -= 1;
                }
            }
            Kind::Polygon => {
                self.count = self.count.clamp(3, 12);
                self.reach = self.reach.clamp(2, 13);
                self.gap = self.gap.clamp(0, self.reach - 1);
            }
            Kind::Arc => {
                self.count = self.count.clamp(1, 24);
                self.reach = self.reach.clamp(2, 13);
            }
            Kind::Ladder => {
                self.count = self.count.clamp(1, 8);
                let min_gap = (self.thick + 1).max(2);
                self.gap = self.gap.clamp(min_gap, 8.max(min_gap));
                self.reach = self.reach.clamp(1, 12);
                while !self.fits() && self.count > 1 {
                    self.count -= 1;
                }
                while !self.fits() && self.reach > 1 {
                    self.reach -= 1;
                }
            }
            Kind::Line => {
                self.count = self.count.clamp(1, 16);
                self.reach = self.reach.clamp(1, 15);
                while !self.fits() && self.reach > 1 {
                    self.reach -= 1;
                }
            }
            Kind::Disc => {
                self.count = self.count.clamp(1, 16);
                self.reach = self.reach.clamp(1, 14);
            }
        }
    }

    /// Line segments (x1, y1, x2, y2; y down) of the shapes drawn as strokes along segments.
    fn segments(&self) -> Vec<[f64; 4]> {
        let (sn, cs) = (self.rot as f64).to_radians().sin_cos();
        // local (u along the pointing direction, v across it) to grid offsets
        let at = |u: f64, v: f64| (u * cs - v * sn, u * sn + v * cs);
        match self.kind {
            Kind::Chevron => {
                let step = (self.thick + 2) as f64;
                let l = self.reach as f64 * std::f64::consts::FRAC_1_SQRT_2;
                (0..self.count)
                    .flat_map(|k| {
                        let t = self.gap as f64 - k as f64 * step;
                        let tip = at(t, 0.0);
                        [at(t - l, l), at(t - l, -l)].map(|e| [tip.0, tip.1, e.0, e.1])
                    })
                    .collect()
            }
            Kind::Polygon => {
                let n = self.count as usize;
                let (outer, inner) = (self.reach as f64, (self.reach - self.gap).max(1) as f64);
                let corners = if self.gap == 0 { n } else { 2 * n };
                let point = |k: usize| {
                    let deg = self.rot as f64 + k as f64 * 360.0 / corners as f64;
                    let r = if self.gap != 0 && k % 2 == 1 { inner } else { outer };
                    let (s, c) = deg.to_radians().sin_cos();
                    (r * c, r * s)
                };
                (0..corners)
                    .map(|k| {
                        let (p, q) = (point(k), point((k + 1) % corners));
                        [p.0, p.1, q.0, q.1]
                    })
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    /// Is pixel (dx, dy), relative to the center, lit by the unshifted, unmirrored shape?
    #[cfg(test)]
    fn hit(&self, dx: i32, dy: i32) -> bool {
        self.hit_with(&self.segments(), dx, dy)
    }

    fn hit_with(&self, segs: &[[f64; 4]], dx: i32, dy: i32) -> bool {
        let a = self.stroke();
        let (fx, fy) = (dx as f64, dy as f64);
        match self.kind {
            Kind::Chevron | Kind::Polygon => segs.iter().any(|s| seg_dist(fx, fy, s) <= a + EPS),
            Kind::Arc => {
                if (fx.hypot(fy) - self.reach as f64).abs() > a + EPS {
                    return false;
                }
                let span = self.count as f64 * 15.0;
                if span >= 360.0 {
                    return true;
                }
                let mut ang = (fy.atan2(fx).to_degrees() - self.rot as f64).rem_euclid(360.0);
                if ang > 180.0 {
                    ang -= 360.0;
                }
                ang.abs() <= span / 2.0 + EPS_DEG
            }
            Kind::Ladder => {
                let (sn, cs) = (self.rot as f64).to_radians().sin_cos();
                let along = fx * cs + fy * sn;
                let perp = -fx * sn + fy * cs;
                (1..=self.count).any(|k| {
                    let w = if k % 2 == 1 { self.reach } else { (self.reach / 2).max(1) };
                    (along - (self.gap * k) as f64).abs() <= a + EPS && perp.abs() <= w as f64 + 0.5 + EPS
                })
            }
            Kind::Line => {
                let (sn, cs) = (self.rot as f64).to_radians().sin_cos();
                let along = fx * cs + fy * sn;
                let perp = -fx * sn + fy * cs;
                along.abs() <= self.reach as f64 + 0.5 + EPS && perp.abs() <= a + EPS
            }
            Kind::Disc => fx.hypot(fy) <= self.reach as f64 - 0.3 + EPS,
            _ => self.hit_simple(a, fx, fy, dx, dy),
        }
    }

    fn hit_simple(&self, a: f64, fx: f64, fy: f64, dx: i32, dy: i32) -> bool {
        match self.kind {
            Kind::Spokes => {
                let (gap, reach) = (self.gap as f64, self.reach as f64);
                (0..self.count).any(|k| {
                    let f = (self.rot as f64 + k as f64 * 360.0 / self.count as f64).to_radians();
                    let (sn, cs) = f.sin_cos();
                    let along = fx * cs + fy * sn;
                    let perp = -fx * sn + fy * cs;
                    along >= gap + 0.5 - EPS && along <= gap + reach + 0.5 + EPS && perp.abs() <= a + EPS
                })
            }
            Kind::Ring => {
                if (fx.hypot(fy) - self.reach as f64).abs() > a {
                    return false;
                }
                if self.count > 1 {
                    let ang = (fy.atan2(fx).to_degrees() - self.rot as f64).rem_euclid(360.0);
                    let seg = 360.0 / self.count as f64;
                    if (ang % seg) / seg > 0.6 {
                        return false;
                    }
                }
                true
            }
            Kind::Diamond => ((dx.abs() + dy.abs()) as f64 - self.reach as f64).abs() <= a,
            Kind::Bracket => {
                let (ax, ay, r, m) = (dx.abs(), dy.abs(), self.reach, self.gap);
                let w = (self.thick - 1) / 2;
                let vertical = (ax - r).abs() <= w && ay >= r + w - m + 1 && ay <= r + w;
                let horizontal = (ay - r).abs() <= w && ax >= r + w - m + 1 && ax <= r + w;
                vertical || horizontal
            }
            Kind::Dot => dx.abs().max(dy.abs()) <= (self.reach - 1) / 2,
            _ => false,
        }
    }

    /// The unshifted shape as a mask over the grid.
    fn mask(&self) -> [bool; N * N] {
        let segs = self.segments();
        let mut m = [false; N * N];
        for y in 0..N {
            for x in 0..N {
                m[y * N + x] = self.hit_with(&segs, x as i32 - C, y as i32 - C);
            }
        }
        m
    }

    /// The pixels this layer lights: its shape, shifted by the offset, then reflected across the mirror axes.
    pub fn cells(&self) -> [bool; N * N] {
        let base = self.mask();
        let at = |dx: i32, dy: i32| {
            let (sx, sy) = (dx - self.ox, dy - self.oy);
            sx.abs() <= C && sy.abs() <= C && base[(sy + C) as usize * N + (sx + C) as usize]
        };
        let (flip_x, flip_y) = (matches!(self.mirror, Mirror::Horizontal | Mirror::Both), matches!(self.mirror, Mirror::Vertical | Mirror::Both));
        let mut out = [false; N * N];
        for y in 0..N {
            for x in 0..N {
                let (dx, dy) = (x as i32 - C, y as i32 - C);
                out[y * N + x] = at(dx, dy) || (flip_x && at(-dx, dy)) || (flip_y && at(dx, -dy)) || (flip_x && flip_y && at(-dx, -dy));
            }
        }
        out
    }

    /// Optional parts of a token, in a fixed order, each only when it differs from the default.
    fn extras(&self) -> String {
        let mut s = String::new();
        if self.opacity != 10 {
            s += &format!(".o{}", self.opacity);
        }
        if self.ox != 0 {
            s += &format!(".x{}", self.ox);
        }
        if self.oy != 0 {
            s += &format!(".y{}", self.oy);
        }
        if self.mirror != Mirror::Off {
            s += &format!(".m{}", self.mirror.tag());
        }
        if self.cycle != 0 {
            s += &format!(".c{}", self.cycle);
        }
        s
    }

    /// Spec token: `spk.00ff66.6.4.3.4.0`. Visibility is not part of the export format.
    pub fn to_token(&self) -> String {
        format!(
            "{}.{:06x}.{}.{}.{}.{}.{}",
            self.kind.tag(),
            self.color,
            self.reach,
            self.gap,
            self.thick,
            self.count,
            self.rot
        )
    }

    /// Config and share token: the spec token, prefixed with `-` when the layer is hidden, then the
    /// parts that differ from the default (`.o5` opacity, `.x2` `.y-1` offset, `.mh` `.mv` `.mb`
    /// mirror, `.c2` hue cycle), then `~g`, `~i`, `~c` or `~y` for a non-normal blend.
    pub fn to_persist_token(&self) -> String {
        let blend = if self.blend == Blend::Normal { String::new() } else { format!("~{}", self.blend.tag()) };
        format!("{}{}{}{}", if self.visible { "" } else { "-" }, self.to_token(), self.extras(), blend)
    }

    pub fn from_token(token: &str) -> Option<Layer> {
        let (visible, rest) = match token.strip_prefix('-') {
            Some(r) => (false, r),
            None => (true, token),
        };
        let (rest, blend) = match rest.split_once('~') {
            Some((body, tag)) => (body, Blend::from_tag(tag)?),
            None => (rest, Blend::Normal),
        };
        let parts: Vec<&str> = rest.split('.').collect();
        if parts.len() < 7 {
            return None;
        }
        let mut layer = Layer {
            kind: Kind::from_tag(parts[0])?,
            color: Some(parts[1])
                .filter(|h| h.len() == 6 && h.bytes().all(|b| b.is_ascii_hexdigit()))
                .and_then(|h| u32::from_str_radix(h, 16).ok())?,
            reach: parts[2].parse().ok()?,
            gap: parts[3].parse().ok()?,
            thick: parts[4].parse().ok()?,
            count: parts[5].parse().ok()?,
            rot: parts[6].parse().ok()?,
            visible,
            blend,
            opacity: 10,
            ox: 0,
            oy: 0,
            mirror: Mirror::Off,
            cycle: 0,
        };
        for extra in &parts[7..] {
            let (key, value) = extra.split_at(extra.chars().next()?.len_utf8());
            match key {
                "o" => layer.opacity = value.parse::<i64>().ok()?.clamp(1, 10) as u8,
                "x" => layer.ox = value.parse().ok()?,
                "y" => layer.oy = value.parse().ok()?,
                "m" => layer.mirror = Mirror::from_tag(value)?,
                "c" => layer.cycle = value.parse::<i64>().ok()?.clamp(0, 3) as u8,
                _ => return None,
            }
        }
        layer.clamp();
        Some(layer)
    }
}

/// Distance from a point to a line segment.
fn seg_dist(px: f64, py: f64, s: &[f64; 4]) -> f64 {
    let (dx, dy) = (s[2] - s[0], s[3] - s[1]);
    let len2 = dx * dx + dy * dy;
    let t = if len2 < 1e-12 { 0.0 } else { (((px - s[0]) * dx + (py - s[1]) * dy) / len2).clamp(0.0, 1.0) };
    (px - (s[0] + t * dx)).hypot(py - (s[1] + t * dy))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Crosshair {
    pub layers: Vec<Layer>,
    /// 0..=2 pixels of black outline
    pub outline: u8,
    /// 1..=10, tenths of full opacity
    pub opacity: u8,
}

/// The eight layer colors, in the order the menu cycles them.
pub const PALETTE: [(&str, u32); 8] = [
    ("Green", 0x00FF66),
    ("Red", 0xFF3B30),
    ("White", 0xFFFFFF),
    ("Yellow", 0xFFE500),
    ("Cyan", 0x00E5FF),
    ("Magenta", 0xFF4DDB),
    ("Orange", 0xFF9500),
    ("Violet", 0xB388FF),
];

fn layer(kind: Kind, color: u32, tune: impl FnOnce(&mut Layer)) -> Layer {
    let mut l = Layer::new(kind, color);
    tune(&mut l);
    l.clamp();
    l
}

/// Named layer stacks the menu can load. The first one is the hip-fire default.
pub fn presets() -> Vec<(&'static str, Vec<Layer>)> {
    let green = 0x00FF66;
    let red = 0xFF3B30;
    let cyan = 0x00E5FF;
    vec![
        ("Classic", vec![layer(Kind::Spokes, green, |l| l.thick = 3), layer(Kind::Dot, green, |l| l.reach = 1)]),
        (
            "Ringed",
            vec![
                layer(Kind::Ring, 0xFFFFFF, |l| {
                    l.reach = 9;
                    l.count = 1;
                }),
                layer(Kind::Spokes, green, |l| {
                    l.reach = 3;
                    l.gap = 2;
                }),
                layer(Kind::Dot, green, |l| l.reach = 1),
            ],
        ),
        (
            "Star",
            vec![
                layer(Kind::Spokes, 0xFFE500, |l| {
                    l.reach = 7;
                    l.gap = 2;
                    l.count = 8;
                }),
                layer(Kind::Dot, red, |l| l.reach = 3),
            ],
        ),
        (
            "Brackets",
            vec![
                layer(Kind::Bracket, cyan, |l| {
                    l.reach = 9;
                    l.gap = 4;
                }),
                layer(Kind::Dot, cyan, |l| l.reach = 1),
            ],
        ),
        (
            "Sniper",
            vec![
                layer(Kind::Spokes, red, |l| {
                    l.reach = 12;
                    l.gap = 3;
                }),
                layer(Kind::Ring, red, |l| {
                    l.reach = 13;
                    l.count = 12;
                }),
                layer(Kind::Diamond, red, |l| l.reach = 5),
                layer(Kind::Dot, red, |l| l.reach = 1),
            ],
        ),
        (
            "Chevron",
            vec![
                layer(Kind::Chevron, cyan, |l| {
                    l.reach = 6;
                    l.gap = 3;
                    l.count = 2;
                }),
                layer(Kind::Dot, cyan, |l| l.reach = 1),
            ],
        ),
        (
            "Ladder",
            vec![
                layer(Kind::Line, 0xFFE500, |l| l.reach = 9),
                layer(Kind::Ladder, 0xFFE500, |l| {
                    l.reach = 5;
                    l.gap = 3;
                    l.count = 4;
                }),
                layer(Kind::Dot, red, |l| l.reach = 1),
            ],
        ),
        (
            "Star 5",
            vec![
                layer(Kind::Polygon, 0xFFFFFF, |l| {
                    l.reach = 9;
                    l.gap = 4;
                    l.count = 5;
                }),
                layer(Kind::Dot, red, |l| l.reach = 1),
            ],
        ),
    ]
}

impl Crosshair {
    pub fn classic() -> Crosshair {
        Crosshair { layers: presets().remove(0).1, outline: 1, opacity: 10 }
    }

    pub fn ads_default() -> Crosshair {
        let mut spokes = Layer::new(Kind::Spokes, 0x00E5FF);
        spokes.reach = 3;
        spokes.gap = 2;
        let mut dot = Layer::new(Kind::Dot, 0x00E5FF);
        dot.reach = 1;
        Crosshair { layers: vec![spokes, dot], outline: 1, opacity: 10 }
    }

    /// Every visible layer merged, with the blend each pixel was drawn with.
    pub fn render(&self) -> Render {
        let mut color = [0u32; N * N];
        let mut cell = [Cell::Empty; N * N];
        let mut alpha = [0u8; N * N];
        let mut cycle = [0u8; N * N];
        for layer in self.layers.iter().take(MAX_LAYERS).filter(|l| l.visible) {
            let kind = match layer.blend {
                Blend::Normal => Cell::Normal,
                Blend::Glow => Cell::Glow,
                lens => Cell::Lens(lens),
            };
            let lit = layer.cells();
            for i in 0..N * N {
                if lit[i] {
                    color[i] = layer.color;
                    cell[i] = kind;
                    alpha[i] = layer.opacity;
                    cycle[i] = layer.cycle;
                }
            }
        }
        let o = self.outline.min(2) as i32;
        if o > 0 {
            let (src, src_alpha) = (cell, alpha);
            for y in 0..N as i32 {
                for x in 0..N as i32 {
                    if src[y as usize * N + x as usize] != Cell::Empty {
                        continue;
                    }
                    // The outline is as strong as the strongest pixel it hugs.
                    let mut near = 0u8;
                    for dy in -o..=o {
                        for dx in -o..=o {
                            let (xx, yy) = (x + dx, y + dy);
                            if xx >= 0 && yy >= 0 && xx < N as i32 && yy < N as i32 && src[yy as usize * N + xx as usize] != Cell::Empty {
                                near = near.max(src_alpha[yy as usize * N + xx as usize]);
                            }
                        }
                    }
                    if near > 0 {
                        cell[y as usize * N + x as usize] = Cell::Outline;
                        color[y as usize * N + x as usize] = 0;
                        alpha[y as usize * N + x as usize] = near;
                    }
                }
            }
        }
        Render { color, cell, alpha, cycle }
    }

    /// Does any visible layer turn its hue? Then the overlay needs a clock.
    pub fn cycling(&self) -> bool {
        self.layers.iter().take(MAX_LAYERS).any(|l| l.visible && l.cycle != 0)
    }

    /// Straight-alpha grid: 0 is empty, 0xFFRRGGBB is a set pixel (blend ignored; used for previews).
    #[cfg(test)]
    pub fn raster(&self) -> [u32; N * N] {
        let r = self.render();
        let mut grid = [0u32; N * N];
        for i in 0..N * N {
            if r.cell[i] != Cell::Empty {
                grid[i] = 0xFF00_0000 | r.color[i];
            }
        }
        grid
    }

    /// Premultiplied ARGB with opacity applied, ready for a 32-bit layered-window bitmap.
    #[cfg(test)]
    pub fn premultiplied(&self) -> [u32; N * N] {
        self.premultiplied_scaled(1.0)
    }

    /// Like `premultiplied`, with opacity further multiplied by `mul` (0..=1).
    ///
    /// Normal and outline pixels are ordinary alpha. Glow pixels have alpha 0 and
    /// color in the channels: the compositor then adds them to what is behind
    /// (proved on the real compositor by `examples/spike.rs`). Lens pixels are left
    /// transparent here; the lens windows show them.
    #[cfg(test)]
    pub fn premultiplied_scaled(&self, mul: f32) -> [u32; N * N] {
        self.premultiplied_hue(mul, [0; 4])
    }

    /// Like `premultiplied_scaled`, with the hue of every pixel from a cycling layer turned by
    /// `hue[level]` degrees (index 0 is unused: level 0 is a fixed color).
    pub fn premultiplied_hue(&self, mul: f32, hue: [i32; 4]) -> [u32; N * N] {
        let r = self.render();
        let mut memo: Vec<(u32, u8, u32)> = Vec::new();
        let mut out = [0u32; N * N];
        for i in 0..N * N {
            // layer opacity (tenths) times crosshair opacity (tenths) times the frame multiplier
            let a = ((r.alpha[i] as f32 * self.opacity.clamp(1, 10) as f32 * 255.0 / 100.0) * mul.clamp(0.0, 1.0)).floor() as u32;
            let level = r.cycle[i].min(3);
            let mut c = r.color[i];
            if level > 0 && hue[level as usize] != 0 && r.cell[i] != Cell::Outline {
                c = match memo.iter().find(|m| m.0 == c && m.1 == level) {
                    Some(m) => m.2,
                    None => {
                        let t = crate::color::rotate_hue(c, hue[level as usize]);
                        memo.push((c, level, t));
                        t
                    }
                };
            }
            let ch = |shift: u32| ((((c >> shift) & 0xFF) * a + 127) / 255) << shift;
            out[i] = match r.cell[i] {
                Cell::Empty | Cell::Lens(_) => 0,
                Cell::Normal | Cell::Outline => (a << 24) | ch(16) | ch(8) | ch(0),
                Cell::Glow => ch(16) | ch(8) | ch(0),
            };
        }
        out
    }

    /// Cells drawn with this lens effect, for the lens window's shape.
    pub fn lens_mask(&self, effect: Blend) -> [bool; N * N] {
        let r = self.render();
        let mut mask = [false; N * N];
        for i in 0..N * N {
            mask[i] = r.cell[i] == Cell::Lens(effect);
        }
        mask
    }

    /// The distinct lens effects in use, in a fixed order.
    pub fn lens_effects(&self) -> Vec<Blend> {
        // Most crosshairs have no lens layer: skip drawing the whole grid just to find that out.
        if !self.layers.iter().take(MAX_LAYERS).any(|l| l.visible && matches!(l.blend, Blend::Invert | Blend::Contrast | Blend::Gray)) {
            return Vec::new();
        }
        let r = self.render();
        [Blend::Invert, Blend::Contrast, Blend::Gray].into_iter().filter(|e| r.cell.contains(&Cell::Lens(*e))).collect()
    }

    /// Spec export string, as shown by the design: `oc1 spk.00ff66.6.4.3.4.0 dot.00ff66.1.4.1.4.0`.
    /// Blend is not part of it.
    #[cfg(test)]
    pub fn to_export(&self) -> String {
        let tokens: Vec<String> = self.layers.iter().map(Layer::to_token).collect();
        format!("oc1 {}", tokens.join(" "))
    }

    /// Same string for the config file, but with everything a layer has: hidden layers keep their
    /// `-` marker, and opacity, offset, mirror, cycle and blend ride along.
    pub fn to_persist(&self) -> String {
        let tokens: Vec<String> = self.layers.iter().map(Layer::to_persist_token).collect();
        format!("oc2 {}", tokens.join(" "))
    }

    /// Parse an `oc1 ...` or `oc2 ...` string. Bad tokens are skipped; `None` if no layer survives.
    pub fn parse_layers(text: &str) -> Option<Vec<Layer>> {
        let layers: Vec<Layer> = text
            .split_whitespace()
            .filter(|t| *t != "oc1" && *t != "oc2")
            .filter_map(Layer::from_token)
            .take(MAX_LAYERS)
            .collect();
        if layers.is_empty() {
            None
        } else {
            Some(layers)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(grid: &[u32; N * N], x: i32, y: i32) -> bool {
        grid[y as usize * N + x as usize] >> 24 != 0
    }

    #[test]
    fn classic_is_centered_and_symmetric() {
        let g = Crosshair::classic().raster();
        assert!(on(&g, C, C), "center pixel is set");
        for d in 5..=10 {
            assert!(on(&g, C + d, C) && on(&g, C - d, C) && on(&g, C, C + d) && on(&g, C, C - d), "arm pixel {d}");
        }
        assert!(!on(&g, C + 2, C + 2), "diagonal stays empty");
        for y in 0..N as i32 {
            for x in 0..N as i32 {
                assert_eq!(on(&g, x, y), on(&g, 2 * C - x, y), "mirror x at {x},{y}");
                assert_eq!(on(&g, x, y), on(&g, x, 2 * C - y), "mirror y at {x},{y}");
            }
        }
    }

    #[test]
    fn four_spokes_are_ninety_degree_symmetric() {
        let g = Crosshair::classic().raster();
        for y in 0..N as i32 {
            for x in 0..N as i32 {
                assert_eq!(on(&g, x, y), on(&g, C - (y - C), C + (x - C)), "rot90 at {x},{y}");
            }
        }
    }

    #[test]
    fn outline_surrounds_but_never_overwrites() {
        let mut c = Crosshair::classic();
        c.outline = 1;
        let with = c.raster();
        c.outline = 0;
        let without = c.raster();
        for i in 0..N * N {
            if without[i] != 0 {
                assert_eq!(with[i], without[i], "fill pixel unchanged");
            }
        }
        assert_eq!(with[C as usize * N + (C as usize + 11)], 0xFF00_0000, "black ring just past the arm tip");
        assert_eq!(without[C as usize * N + (C as usize + 11)], 0);
    }

    #[test]
    fn clamp_keeps_everything_inside_the_grid() {
        let mut l = Layer::new(Kind::Spokes, 0xFFFFFF);
        l.gap = 10;
        l.reach = 15;
        l.thick = 4;
        l.clamp();
        assert_eq!((l.gap + l.reach, l.thick % 2), (16, 1));
        let mut c = Crosshair::classic();
        c.layers = vec![l];
        c.outline = 2;
        c.raster();
        let mut ring = Layer::new(Kind::Ring, 0xFFFFFF);
        ring.reach = 99;
        ring.thick = 99;
        ring.clamp();
        assert_eq!((ring.reach, ring.thick), (13, 7));
    }

    #[test]
    fn export_round_trips() {
        let c = Crosshair::classic();
        let parsed = Crosshair::parse_layers(&c.to_export()).expect("parses");
        assert_eq!(parsed, c.layers);
        let mut hidden = Layer::new(Kind::Bracket, 0x123456);
        hidden.visible = false;
        assert_eq!(Layer::from_token(&hidden.to_persist_token()), Some(hidden));
    }

    #[test]
    fn export_matches_the_design_spec_exactly() {
        assert_eq!(Crosshair::classic().to_export(), "oc1 spk.00ff66.6.4.3.4.0 dot.00ff66.1.4.1.4.0");
        let mut hidden = Layer::new(Kind::Ring, 0x00FF66);
        hidden.visible = false;
        assert!(!hidden.to_token().starts_with('-'), "visibility is not in the export format");
        assert!(hidden.to_persist_token().starts_with('-'));
        for (kind, tag) in [(Kind::Spokes, "spk"), (Kind::Ring, "rng"), (Kind::Diamond, "dia"), (Kind::Bracket, "brk"), (Kind::Dot, "dot")] {
            assert_eq!(kind.tag(), tag);
            assert_eq!(Kind::from_tag(tag), Some(kind));
        }
        for kind in Kind::ALL {
            assert_eq!(Kind::from_tag(kind.tag()), Some(kind), "{kind:?} round-trips through its tag");
        }
        assert_eq!(Kind::from_tag("spo"), Some(Kind::Spokes), "older spelling still reads");
    }

    #[test]
    fn symmetric_spoke_sets_stay_symmetric() {
        for count in [2, 4, 6, 8, 10, 12] {
            for thick in [1, 3, 5, 7] {
                for gap in 0..=3 {
                    let mut l = Layer::new(Kind::Spokes, 0xFFFFFF);
                    l.count = count;
                    l.thick = thick;
                    l.gap = gap;
                    l.reach = 6;
                    l.clamp();
                    for dy in -20..=20 {
                        for dx in -20..=20 {
                            assert_eq!(l.hit(dx, dy), l.hit(-dx, -dy), "count {count} thick {thick} gap {gap} at {dx},{dy}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn no_clamped_shape_reaches_past_the_grid() {
        for kind in Kind::ALL {
            for thick in [1, 3, 5, 7] {
                for rot in (0..360).step_by(15) {
                    let mut l = Layer::new(kind, 0xFFFFFF);
                    l.reach = 99;
                    l.gap = 99;
                    l.count = 99;
                    l.thick = thick;
                    l.rot = rot;
                    l.clamp();
                    for dy in -30..=30i32 {
                        for dx in -30..=30i32 {
                            if dx.abs() > C || dy.abs() > C {
                                assert!(!l.hit(dx, dy), "{kind:?} thick {thick} rot {rot} leaks to {dx},{dy}");
                            }
                        }
                    }
                }
            }
        }
    }

    fn solid_dot(blend: Blend) -> Crosshair {
        let mut dot = Layer::new(Kind::Dot, 0x40C080);
        dot.reach = 3;
        dot.blend = blend;
        Crosshair { layers: vec![dot], outline: 0, opacity: 10 }
    }

    #[test]
    fn glow_pixels_have_zero_alpha_and_scaled_color() {
        let full = solid_dot(Blend::Glow).premultiplied();
        let c = C as usize * N + C as usize;
        assert_eq!(full[c], 0x0040_C080, "alpha 0, color in the channels: the compositor adds it");
        let mut half = solid_dot(Blend::Glow);
        half.opacity = 5;
        assert_eq!(half.premultiplied()[c] >> 24, 0);
        assert!(((half.premultiplied()[c] >> 8) & 0xFF) < 0xC0, "opacity scales the added light");
        assert_eq!(solid_dot(Blend::Normal).premultiplied()[c] >> 24, 0xFF);
    }

    #[test]
    fn lens_pixels_are_transparent_here_and_masked_for_the_lens() {
        let c = solid_dot(Blend::Invert);
        let centre = C as usize * N + C as usize;
        assert_eq!(c.premultiplied()[centre], 0);
        assert!(c.lens_mask(Blend::Invert)[centre]);
        assert!(!c.lens_mask(Blend::Gray)[centre]);
        assert_eq!(c.lens_effects(), vec![Blend::Invert]);
        assert!(solid_dot(Blend::Glow).lens_effects().is_empty());
        let mask_pixels = c.lens_mask(Blend::Invert).iter().filter(|m| **m).count();
        assert_eq!(mask_pixels, 3 * 3, "a dot of size 3");
    }

    #[test]
    fn opacity_multiplier_scales_normal_pixels() {
        let c = solid_dot(Blend::Normal);
        let i = C as usize * N + C as usize;
        assert_eq!(c.premultiplied_scaled(0.5)[i] >> 24, 127);
        assert_eq!(c.premultiplied_scaled(0.0)[i] >> 24, 0);
        assert_eq!(c.premultiplied_scaled(9.0)[i] >> 24, 255, "multiplier is clamped");
    }

    #[test]
    fn blend_persists_but_is_not_exported() {
        let mut l = Layer::new(Kind::Ring, 0x00FF66);
        l.blend = Blend::Gray;
        assert!(l.to_persist_token().ends_with("~y"));
        assert!(!l.to_token().contains('~'));
        assert_eq!(Layer::from_token(&l.to_persist_token()), Some(l.clone()));
        let c = Crosshair { layers: vec![l], outline: 1, opacity: 10 };
        assert!(!c.to_export().contains('~'));
        assert_eq!(Layer::from_token("spk.00ff66.6.4.3.4.0~z"), None, "unknown blend tag is rejected");
        let mut glow = Layer::new(Kind::Dot, 0x112233);
        glow.visible = false;
        glow.blend = Blend::Glow;
        assert_eq!(Layer::from_token(&glow.to_persist_token()), Some(glow));
    }

    #[test]
    fn garbage_is_rejected_without_panicking() {
        for bad in [
            "",
            "oc1",
            "xyz",
            "spk.zzz.1.1.1.1.0",
            "spk.00ff66.1.1.1.1",
            "spk.1000000.1.1.1.1.0",
            "spk.f00.6.4.3.4.0",
            "spk.+00ff6.6.4.3.4.0",
        ] {
            assert_eq!(Crosshair::parse_layers(bad), None, "{bad:?}");
        }
        assert!(Crosshair::parse_layers("garbage dot.00ff66.1.0.1.1.0").is_some(), "good token survives bad neighbours");
    }

    #[test]
    fn premultiplied_applies_opacity_to_set_pixels_only() {
        let mut c = Crosshair::classic();
        c.opacity = 5;
        let p = c.premultiplied();
        assert_eq!(p[0], 0, "empty stays transparent");
        let center = p[C as usize * N + C as usize];
        assert_eq!(center >> 24, 127);
        assert!((center >> 8) & 0xFF <= 127, "green channel premultiplied");
    }

    fn lit(l: &Layer) -> std::collections::BTreeSet<(i32, i32)> {
        let cells = l.cells();
        (0..N * N).filter(|i| cells[*i]).map(|i| ((i % N) as i32 - C, (i / N) as i32 - C)).collect()
    }

    fn with(kind: Kind, tune: impl FnOnce(&mut Layer)) -> Layer {
        layer(kind, 0xFFFFFF, tune)
    }

    #[test]
    fn a_chevron_points_the_way_it_is_turned() {
        let up = lit(&Layer::new(Kind::Chevron, 0xFFFFFF));
        for p in [(0, -2), (1, -1), (-1, -1), (2, 0), (-2, 0), (3, 1)] {
            assert!(up.contains(&p), "{p:?} is on the arms of an upward chevron");
        }
        assert!(!up.contains(&(0, -1)) && !up.contains(&(0, 0)), "the inside of the V is empty");
        for &(x, y) in &up {
            assert!(up.contains(&(-x, y)), "upward chevron is symmetric left to right at {x},{y}");
        }
        let down = lit(&with(Kind::Chevron, |l| l.rot = 90));
        assert!(down.contains(&(0, 2)) && down.contains(&(1, 1)) && !down.contains(&(0, -2)));
        let stacked = lit(&with(Kind::Chevron, |l| l.count = 3));
        assert!(stacked.contains(&(0, 1)) && stacked.contains(&(0, 4)), "stacked chevrons trail behind the first, three pixels apart");
    }

    #[test]
    fn polygons_and_stars_have_the_corners_they_say() {
        let hex = with(Kind::Polygon, |l| {
            l.reach = 6;
            l.count = 6;
            l.gap = 0;
            l.rot = 0;
        });
        let h = lit(&hex);
        for p in [(6, 0), (-6, 0), (3, 5), (-3, -5)] {
            assert!(h.contains(&p), "hexagon corner near {p:?}");
        }
        assert!(!h.contains(&(0, 0)), "hollow");
        for &(x, y) in &h {
            assert!(h.contains(&(-x, y)) && h.contains(&(x, -y)), "regular hexagon is symmetric at {x},{y}");
        }
        let star = lit(&with(Kind::Polygon, |l| {
            l.reach = 9;
            l.count = 5;
            l.gap = 4;
            l.rot = 270;
        }));
        let pentagon = lit(&with(Kind::Polygon, |l| {
            l.reach = 9;
            l.count = 5;
            l.gap = 0;
            l.rot = 270;
        }));
        assert!(star.contains(&(0, -9)) && pentagon.contains(&(0, -9)), "both point up");
        assert!(star.contains(&(3, -4)), "the star's inner corner sits at radius 5");
        assert_ne!(star, pentagon);
    }

    #[test]
    fn an_arc_covers_only_its_span() {
        let quarter = lit(&with(Kind::Arc, |l| {
            l.reach = 8;
            l.count = 6;
            l.rot = 0;
        }));
        assert!(quarter.contains(&(8, 0)) && quarter.contains(&(6, 6)) && quarter.contains(&(6, -6)), "a 90 degree arc facing right reaches 45 degrees each way");
        assert!(!quarter.contains(&(-8, 0)) && !quarter.contains(&(0, 8)) && !quarter.contains(&(0, -8)));
        for &(x, y) in &quarter {
            assert!(quarter.contains(&(x, -y)), "symmetric about its axis at {x},{y}");
        }
        let full = lit(&with(Kind::Arc, |l| {
            l.reach = 8;
            l.count = 24;
        }));
        let ring = lit(&with(Kind::Ring, |l| {
            l.reach = 8;
            l.count = 1;
        }));
        assert_eq!(full, ring, "a 360 degree arc is the ring");
    }

    #[test]
    fn ladder_ticks_step_away_and_alternate_in_width() {
        let ticks = lit(&Layer::new(Kind::Ladder, 0xFFFFFF));
        for (y, half) in [(3, 4), (6, 2), (9, 4), (12, 2)] {
            for x in -half..=half {
                assert!(ticks.contains(&(x, y)), "tick at row {y} covers {x}");
            }
            assert!(!ticks.contains(&(half + 1, y)), "and stops at {half}");
        }
        assert!(ticks.iter().all(|(_, y)| [3, 6, 9, 12].contains(y)), "nothing between the ticks or above the center");
        let above = lit(&with(Kind::Ladder, |l| l.rot = 270));
        assert!(above.contains(&(0, -3)) && !above.contains(&(0, 3)));
    }

    #[test]
    fn a_line_is_centered_and_a_disc_is_round() {
        let bar = lit(&with(Kind::Line, |l| l.reach = 6));
        assert_eq!(bar.len(), 13);
        assert!(bar.iter().all(|(_, y)| *y == 0) && bar.contains(&(-6, 0)) && bar.contains(&(6, 0)));
        let tall = lit(&with(Kind::Line, |l| {
            l.reach = 4;
            l.rot = 90;
            l.thick = 3;
        }));
        assert_eq!(tall.len(), 9 * 3);
        assert_eq!(lit(&with(Kind::Disc, |l| l.reach = 1)).len(), 1);
        let disc = lit(&with(Kind::Disc, |l| l.reach = 3));
        assert_eq!(disc.len(), 21, "a pixel-art disc of diameter 5");
        for &(x, y) in &disc {
            assert!(disc.contains(&(-x, y)) && disc.contains(&(x, -y)) && disc.contains(&(y, x)), "round at {x},{y}");
        }
    }

    #[test]
    fn offset_moves_and_clips_and_mirror_reflects() {
        let dot = |ox, oy, mirror| lit(&with(Kind::Dot, |l| {
            l.reach = 1;
            l.ox = ox;
            l.oy = oy;
            l.mirror = mirror;
        }));
        assert_eq!(dot(3, -2, Mirror::Off), [(3, -2)].into());
        assert_eq!(dot(3, 0, Mirror::Horizontal), [(3, 0), (-3, 0)].into());
        assert_eq!(dot(0, 2, Mirror::Vertical), [(0, 2), (0, -2)].into());
        assert_eq!(dot(3, 2, Mirror::Both), [(3, 2), (-3, 2), (3, -2), (-3, -2)].into());
        assert_eq!(dot(0, 0, Mirror::Both).len(), 1, "mirroring something on the axis adds nothing");
        let ring = |ox| lit(&with(Kind::Ring, |l| {
            l.reach = 13;
            l.ox = ox;
        }));
        assert!(ring(12).len() < ring(0).len(), "what leaves the grid is clipped");
        assert!(ring(12).iter().all(|(x, y)| x.abs() <= C && y.abs() <= C));
        let mut l = with(Kind::Dot, |_| {});
        l.ox = 99;
        l.oy = -99;
        l.clamp();
        assert_eq!((l.ox, l.oy), (12, -12));
    }

    #[test]
    fn layer_opacity_multiplies_and_the_outline_follows_the_strongest_neighbour() {
        let mut dim = Layer::new(Kind::Dot, 0xFFFFFF);
        dim.reach = 1;
        dim.opacity = 5;
        let mut c = Crosshair { layers: vec![dim.clone()], outline: 1, opacity: 10 };
        let center = C as usize * N + C as usize;
        assert_eq!(c.premultiplied()[center] >> 24, 127);
        assert_eq!(c.premultiplied()[center + 1] >> 24, 127, "outline hugging a half-strength pixel is half strength");
        c.opacity = 5;
        assert_eq!(c.premultiplied()[center] >> 24, 63, "layer and crosshair opacity multiply");
        let mut strong = Layer::new(Kind::Dot, 0xFFFFFF);
        strong.reach = 1;
        strong.ox = 2;
        let c = Crosshair { layers: vec![dim, strong], outline: 1, opacity: 10 };
        assert_eq!(c.premultiplied()[center + 1] >> 24, 255, "between a faint and a strong pixel the outline takes the strong one");
    }

    #[test]
    fn hue_cycle_turns_only_cycling_pixels() {
        let mut red = Layer::new(Kind::Dot, 0xFF0000);
        red.reach = 1;
        red.cycle = 2;
        let mut fixed = Layer::new(Kind::Dot, 0xFF0000);
        fixed.reach = 1;
        fixed.ox = 4;
        let c = Crosshair { layers: vec![red, fixed], outline: 1, opacity: 10 };
        assert!(c.cycling());
        let still = c.premultiplied_scaled(1.0);
        let turned = c.premultiplied_hue(1.0, [0, 0, 120, 0]);
        let at = |dx: i32| C as usize * N + (C + dx) as usize;
        assert_eq!(still[at(0)], 0xFFFF_0000);
        assert_eq!(turned[at(0)], 0xFF00_FF00, "red turned 120 degrees is green");
        assert_eq!(turned[at(4)], 0xFFFF_0000, "a layer that does not cycle keeps its color");
        assert_eq!(turned[at(1)], still[at(1)], "the outline stays black");
        assert_eq!(c.premultiplied_hue(1.0, [0, 120, 0, 0]), still, "other levels do not move this one");
        let mut off = c.clone();
        off.layers[0].visible = false;
        assert!(!off.cycling());
    }

    #[test]
    fn extras_ride_in_the_token_and_defaults_stay_out() {
        let plain = Layer::new(Kind::Dot, 0x112233);
        assert_eq!(plain.to_persist_token().split('.').count(), 7, "nothing extra when everything is default");
        let mut l = Layer::new(Kind::Chevron, 0x112233);
        l.visible = false;
        l.blend = Blend::Glow;
        l.opacity = 5;
        l.ox = 2;
        l.oy = -3;
        l.mirror = Mirror::Horizontal;
        l.cycle = 2;
        let t = l.to_persist_token();
        assert_eq!(t, "-chv.112233.5.2.1.1.270.o5.x2.y-3.mh.c2~g");
        assert_eq!(Layer::from_token(&t), Some(l.clone()));
        assert_eq!(l.to_token(), "chv.112233.5.2.1.1.270", "the oc1 spec token never carries extras");
        assert_eq!(Layer::from_token("spk.00ff66.6.4.3.4.0").map(|l| (l.opacity, l.ox, l.oy, l.mirror, l.cycle)), Some((10, 0, 0, Mirror::Off, 0)), "old 7-part tokens still read");
        for bad in ["spk.00ff66.6.4.3.4.0.q3", "spk.00ff66.6.4.3.4.0.mx", "spk.00ff66.6.4.3.4.0.o", "spk.00ff66.6.4.3.4.0.xz", "spk.00ff66.6.4.3.4.0.", "spk.00ff66.6.4.3.4.0.c"] {
            assert_eq!(Layer::from_token(bad), None, "{bad}");
        }
        assert_eq!(Layer::from_token("spk.00ff66.6.4.3.4.0.o99").map(|l| l.opacity), Some(10), "out of range clamps");
        let c = Crosshair { layers: vec![l.clone(), plain], outline: 1, opacity: 10 };
        assert!(c.to_persist().starts_with("oc2 "));
        assert_eq!(Crosshair::parse_layers(&c.to_persist()), Some(c.layers.clone()));
        assert!(!c.to_export().contains(".o5"), "oc1 export has no extras");
    }

    #[test]
    fn limits_offered_by_the_menu_are_limits_the_clamp_accepts() {
        for kind in Kind::ALL {
            for thick in [1, 3, 5, 7] {
                let mut l = Layer::new(kind, 0xFFFFFF);
                l.thick = thick;
                l.clamp();
                let mut by_reach = l.clone();
                by_reach.reach = by_reach.reach_max();
                by_reach.clamp();
                assert_eq!(by_reach.reach, l.reach_max(), "{kind:?} thick {thick}: the longest offered length stays");
                let mut by_count = l.clone();
                by_count.count = by_count.count_max();
                by_count.clamp();
                assert_eq!(by_count.count, l.count_max(), "{kind:?} thick {thick}: the highest offered count stays");
            }
        }
        let mut ladder = Layer::new(Kind::Ladder, 0xFFFFFF);
        ladder.thick = 7;
        ladder.clamp();
        assert!(ladder.gap >= 8 && ladder.fits(), "thick ticks keep a gap between them");
    }

    #[test]
    fn every_preset_draws_inside_the_grid_and_something() {
        for (name, layers) in presets() {
            let c = Crosshair { layers, outline: 2, opacity: 10 };
            assert!(c.raster().iter().any(|p| *p != 0), "{name} draws something");
        }
        assert!(presets().len() >= 8);
    }
}
