//! The menu window and its text. Everything the menu draws is composed in
//! `paint.rs` into a pixel buffer; this file owns the Win32 pieces around it:
//! a layered top-most window, GDI used only to rasterize non-anti-aliased
//! glyphs into that buffer, and the clipboard.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::time::Instant;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::DataExchange::*;
use windows_sys::Win32::System::Memory::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::config::Config;
use crate::menu::{Menu, Metrics, Rect, BORDER, CONTENT_W, CONTENT_X, HUD_W, PAD_TOP, TITLE_H};
use crate::paint::{self, Canvas, Damage, Font, Painted, TextBackend, TITLE_PERIOD_SECS};

pub const TIMER_TITLE: usize = 1;
const TITLE_TICK_MS: u32 = 66;
const SCRATCH_W: i32 = 256;
const SCRATCH_H: i32 = 32;
const HUD_MARGIN: i32 = 6;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn make_dib(dc: HDC, w: i32, h: i32) -> (HBITMAP, *mut u32) {
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [RGBQUAD { rgbBlue: 0, rgbGreen: 0, rgbRed: 0, rgbReserved: 0 }],
    };
    let mut bits: *mut c_void = null_mut();
    let dib = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
    (dib, bits as *mut u32)
}

/// A fixed-pitch font with no anti-aliasing: Cascadia Mono, else Consolas, else Courier New.
/// Returns the font, the advance of `0` and the line height.
unsafe fn make_font(dc: HDC, px: i32, bold: bool) -> (HFONT, i32, i32) {
    let mut chosen: HFONT = null_mut();
    for face in ["Cascadia Mono", "Consolas", "Courier New"] {
        let f = CreateFontW(
            -px,
            0,
            0,
            0,
            if bold { 700 } else { 400 },
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            NONANTIALIASED_QUALITY as u32,
            (FIXED_PITCH | FF_MODERN) as u32,
            wide(face).as_ptr(),
        );
        if f.is_null() {
            continue;
        }
        let old = SelectObject(dc, f);
        let mut buf = [0u16; 64];
        let n = GetTextFaceW(dc, 64, buf.as_mut_ptr()).clamp(0, 63) as usize;
        SelectObject(dc, old);
        let got = String::from_utf16_lossy(&buf[..n]);
        let got = got.trim_end_matches('\0');
        if got.eq_ignore_ascii_case(face) || face == "Courier New" {
            chosen = f;
            break;
        }
        DeleteObject(f);
    }
    if chosen.is_null() {
        chosen = GetStockObject(ANSI_FIXED_FONT) as HFONT;
    }
    let old = SelectObject(dc, chosen);
    let mut size = SIZE { cx: 0, cy: 0 };
    let zero = [b'0' as u16];
    GetTextExtentPoint32W(dc, zero.as_ptr(), 1, &mut size);
    let mut tm: TEXTMETRICW = std::mem::zeroed();
    GetTextMetricsW(dc, &mut tm);
    SelectObject(dc, old);
    (chosen, size.cx.max(1), tm.tmHeight.max(1))
}

/// One rasterized character: an on/off mask, `w` x `h`.
struct Glyph {
    w: i32,
    h: i32,
    bits: Vec<u8>,
}

/// Glyph shapes come from GDI drawing white-on-black into a scratch bitmap;
/// the result is read back as an on/off mask and painted in the wanted color.
/// Each character is rasterized once and kept, so redrawing text is just
/// copying masks: no GDI call after the first time a character is seen.
pub struct GdiText {
    dc: HDC,
    scratch: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
    fonts: [HFONT; 3],
    cells: [i32; 3],
    heights: [i32; 3],
    glyphs: RefCell<HashMap<(u8, char), Glyph>>,
}

impl GdiText {
    unsafe fn new() -> GdiText {
        let dc = CreateCompatibleDC(null_mut());
        let (scratch, bits) = make_dib(dc, SCRATCH_W, SCRATCH_H);
        let old = SelectObject(dc, scratch);
        let (body, body_cell, body_h) = make_font(dc, 11, false);
        let (title, title_cell, title_h) = make_font(dc, 11, true);
        let (small, small_cell, small_h) = make_font(dc, 9, false);
        GdiText { dc, scratch, old, bits, fonts: [body, title, small], cells: [body_cell, title_cell, small_cell], heights: [body_h, title_h, small_h], glyphs: RefCell::new(HashMap::new()) }
    }

    /// Rasterize one character with GDI (non-anti-aliased) into an on/off mask.
    fn render_glyph(&self, i: usize, ch: char) -> Glyph {
        let mut buf = [0u16; 2];
        let utf16 = ch.encode_utf16(&mut buf);
        let (rows, cols) = (self.heights[i].min(SCRATCH_H), (self.cells[i] + 4).min(SCRATCH_W));
        unsafe {
            for row in 0..rows {
                std::ptr::write_bytes(self.bits.add((row * SCRATCH_W) as usize), 0, cols as usize);
            }
            let prev = SelectObject(self.dc, self.fonts[i]);
            SetBkMode(self.dc, TRANSPARENT as i32);
            SetTextColor(self.dc, 0x00FF_FFFF);
            TextOutW(self.dc, 0, 0, utf16.as_ptr(), utf16.len() as i32);
            GdiFlush();
            SelectObject(self.dc, prev);
            let mut bits = Vec::with_capacity((rows * cols) as usize);
            for row in 0..rows {
                for col in 0..cols {
                    bits.push(u8::from(*self.bits.add((row * SCRATCH_W + col) as usize) & 0x00FF_FFFF != 0));
                }
            }
            Glyph { w: cols, h: rows, bits }
        }
    }

    /// The old way, a whole string through GDI at once. Kept for the test that proves the
    /// glyph cache draws exactly the same pixels.
    #[cfg(test)]
    fn draw_whole_run(&self, c: &mut Canvas, x: i32, y: i32, line_h: i32, text: &str, rgb: u32, font: Font) {
        let i = font as usize;
        let utf16: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            std::ptr::write_bytes(self.bits, 0, (SCRATCH_W * SCRATCH_H) as usize);
            let prev = SelectObject(self.dc, self.fonts[i]);
            SetBkMode(self.dc, TRANSPARENT as i32);
            SetTextColor(self.dc, 0x00FF_FFFF);
            TextOutW(self.dc, 0, 0, utf16.as_ptr(), utf16.len() as i32);
            GdiFlush();
            SelectObject(self.dc, prev);
            let ty = y + (line_h - self.heights[i]) / 2;
            for row in 0..self.heights[i].min(SCRATCH_H) {
                for col in 0..SCRATCH_W {
                    if *self.bits.add((row * SCRATCH_W + col) as usize) & 0x00FF_FFFF != 0 {
                        c.blend(x + col, ty + row, rgb, 255);
                    }
                }
            }
        }
    }
}

impl Drop for GdiText {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            DeleteObject(self.scratch);
            DeleteDC(self.dc);
        }
    }
}

impl TextBackend for GdiText {
    fn cell(&self, font: Font) -> i32 {
        self.cells[font as usize]
    }

    fn draw(&self, c: &mut Canvas, x: i32, y: i32, line_h: i32, text: &str, rgb: u32, font: Font) {
        let i = font as usize;
        let cell = self.cells[i];
        let ty = y + (line_h - self.heights[i]) / 2;
        let mut cache = self.glyphs.borrow_mut();
        for (n, ch) in text.chars().enumerate() {
            if ch == ' ' {
                continue;
            }
            let g = cache.entry((i as u8, ch)).or_insert_with(|| self.render_glyph(i, ch));
            let gx = x + n as i32 * cell;
            for row in 0..g.h {
                for col in 0..g.w {
                    if g.bits[(row * g.w + col) as usize] != 0 {
                        c.blend(gx + col, ty + row, rgb, 255);
                    }
                }
            }
        }
    }
}

pub struct Hud {
    pub hwnd: HWND,
    mem_dc: HDC,
    dib: HBITMAP,
    old_bitmap: HGDIOBJ,
    bits: *mut u32,
    /// the menu is drawn at its design size ("logical") and scaled up by whole pixels
    scale: i32,
    logical: Vec<u32>,
    /// what is drawn in `logical` right now, so a redraw can redo only the rows that changed
    painted: Option<Painted>,
    /// logical height available to the menu
    max_h: i32,
    origin: POINT,
    text: GdiText,
    pub menu: Menu,
    pub visible: bool,
    started: Instant,
}

fn work_area() -> RECT {
    let mut r = RECT { left: 0, top: 0, right: 1920, bottom: 1080 };
    unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut r as *mut RECT as *mut c_void, 0) };
    r
}

impl Hud {
    /// Creates the (hidden) menu window. `wndproc` receives its messages.
    pub unsafe fn create(hinstance: HINSTANCE, wndproc: WNDPROC) -> Option<Hud> {
        let class = wide("reDrawMenu");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: wndproc,
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance,
            hIcon: null_mut(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: null_mut(),
            lpszMenuName: null(),
            lpszClassName: class.as_ptr(),
            hIconSm: null_mut(),
        };
        if RegisterClassExW(&wc) == 0 {
            return None;
        }
        let hwnd = CreateWindowExW(WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW, class.as_ptr(), wide("reDraw menu").as_ptr(), WS_POPUP, 0, 0, HUD_W, 100, null_mut(), null_mut(), hinstance, null());
        if hwnd.is_null() {
            return None;
        }
        let work = work_area();
        let max_h = (work.bottom - work.top - 2 * HUD_MARGIN).max(60);
        let mem_dc = CreateCompatibleDC(null_mut());
        let (dib, bits) = make_dib(mem_dc, HUD_W, max_h);
        if dib.is_null() || bits.is_null() {
            return None;
        }
        let old_bitmap = SelectObject(mem_dc, dib);
        let text = GdiText::new();
        let mut menu = Menu::new();
        menu.set_view(max_h, Metrics { body_cell: text.cell(Font::Body), small_cell: text.cell(Font::Small) });
        Some(Hud {
            hwnd,
            mem_dc,
            dib,
            old_bitmap,
            bits,
            scale: 1,
            logical: vec![0; (HUD_W * max_h) as usize],
            painted: None,
            max_h,
            origin: POINT { x: work.left + HUD_MARGIN, y: work.top + HUD_MARGIN },
            text,
            menu,
            visible: false,
            started: Instant::now(),
        })
    }

    /// Recompute the position and size for the current work area and scale, and reallocate the buffers.
    fn rebuild(&mut self) {
        let work = work_area();
        let s = self.scale;
        let max_h = ((work.bottom - work.top - 2 * HUD_MARGIN * s) / s).max(60);
        self.origin = POINT { x: work.left + HUD_MARGIN * s, y: work.top + HUD_MARGIN * s };
        unsafe {
            let (dib, bits) = make_dib(self.mem_dc, HUD_W * s, max_h * s);
            if !dib.is_null() && !bits.is_null() {
                SelectObject(self.mem_dc, dib);
                DeleteObject(self.dib);
                self.dib = dib;
                self.bits = bits;
                self.max_h = max_h;
                self.logical = vec![0; (HUD_W * max_h) as usize];
                self.painted = None;
                self.menu.set_view(max_h, Metrics { body_cell: self.text.cell(Font::Body), small_cell: self.text.cell(Font::Small) });
            }
        }
    }

    /// The work area changed (resolution, taskbar): re-pin the menu and resize its buffer.
    pub fn refresh_work_area(&mut self, cfg: &Config) {
        self.rebuild();
        if self.visible {
            self.paint(cfg);
        }
    }

    /// Whole-number menu scale (1 to 3). Everything is drawn at its design size and
    /// scaled by whole pixels, so it stays sharp.
    pub fn set_scale(&mut self, scale: i32, cfg: &Config) {
        let s = scale.clamp(1, 3);
        if s != self.scale {
            self.scale = s;
            self.rebuild();
            if self.visible {
                self.paint(cfg);
            }
        }
    }

    pub fn scale(&self) -> i32 {
        self.scale
    }

    fn title_u(&self, cfg: &Config) -> f32 {
        if cfg.reduce_motion {
            0.0
        } else {
            (self.started.elapsed().as_secs_f32() % TITLE_PERIOD_SECS) / TITLE_PERIOD_SECS
        }
    }

    /// Scale the drawn menu (`h` logical rows) up into the window bitmap and show it. With
    /// `dirty`, only that logical rectangle is copied and handed to the compositor.
    fn present(&self, h: i32, dirty: Option<Rect>) {
        let s = self.scale;
        let stride = (HUD_W * s) as usize;
        let d = match dirty {
            Some(r) => {
                let (x0, y0) = (r.x.clamp(0, HUD_W), r.y.clamp(0, h));
                let (x1, y1) = ((r.x + r.w).clamp(0, HUD_W), (r.y + r.h).clamp(0, h));
                Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
            }
            None => Rect { x: 0, y: 0, w: HUD_W, h },
        };
        if d.w <= 0 || d.h <= 0 {
            return;
        }
        // SAFETY: `bits` is a (HUD_W*s) x (max_h*s) bitmap and h <= max_h; `logical` is HUD_W x max_h.
        unsafe {
            for y in d.y * s..(d.y + d.h) * s {
                let row = &self.logical[((y / s) * HUD_W) as usize..][..HUD_W as usize];
                let dst = self.bits.add(y as usize * stride);
                if s == 1 {
                    std::ptr::copy_nonoverlapping(row.as_ptr().add(d.x as usize), dst.add(d.x as usize), d.w as usize);
                } else {
                    for x in d.x * s..(d.x + d.w) * s {
                        *dst.add(x as usize) = row[(x / s) as usize];
                    }
                }
            }
        }
        let size = SIZE { cx: HUD_W * s, cy: h * s };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        unsafe {
            if dirty.is_some() {
                let rc = RECT { left: d.x * s, top: d.y * s, right: (d.x + d.w) * s, bottom: (d.y + d.h) * s };
                let info = UPDATELAYEREDWINDOWINFO { cbSize: std::mem::size_of::<UPDATELAYEREDWINDOWINFO>() as u32, hdcDst: null_mut(), pptDst: &self.origin, psize: &size, hdcSrc: self.mem_dc, pptSrc: &src, crKey: 0, pblend: &blend, dwFlags: ULW_ALPHA, prcDirty: &rc };
                UpdateLayeredWindowIndirect(self.hwnd, &info);
            } else {
                UpdateLayeredWindow(self.hwnd, null_mut(), &self.origin, &size, self.mem_dc, &src, 0, &blend, ULW_ALPHA);
            }
        }
    }

    /// Redraw the menu: everything the first time or when the layout changed, otherwise only
    /// the rows that differ from what is on screen.
    pub fn paint(&mut self, cfg: &Config) {
        let layout = self.menu.layout(cfg);
        self.menu.clamp_scroll(layout.height);
        let h = layout.height.min(self.max_h);
        let u = self.title_u(cfg);
        let px = &mut self.logical[..(HUD_W * h) as usize];
        let damage = paint::paint_update(&mut Canvas::new(px, HUD_W, h), &mut self.painted, &layout, cfg, &self.menu, &self.text, cfg.accent, u);
        match damage {
            Damage::None => {}
            Damage::Full => self.present(h, None),
            Damage::Rect(r) => self.present(h, Some(r)),
        }
    }

    /// Redraw just the animated title (about 15 times a second while the menu is open).
    pub fn paint_title(&mut self, cfg: &Config) {
        let h = self.menu.window_height(cfg);
        let u = self.title_u(cfg);
        let px = &mut self.logical[..(HUD_W * h) as usize];
        if paint::paint_title(&mut Canvas::new(px, HUD_W, h), self.menu.scroll(), &self.text, u) {
            let top = BORDER + PAD_TOP - self.menu.scroll();
            let (y0, y1) = (top.max(BORDER), (top + TITLE_H).min(h - BORDER));
            self.present(h, Some(Rect { x: CONTENT_X, y: y0, w: CONTENT_W, h: y1 - y0 }));
        }
    }

    /// Draw the menu and save it as a 24-bit BMP over a flat game-ish backdrop (for checking the look).
    pub fn dump_bmp(&mut self, cfg: &Config, path: &str) {
        self.painted = None;
        self.paint(cfg);
        // The saved image is the real window bitmap: scaled, as it appears on screen.
        let (w, h) = (HUD_W * self.scale, self.menu.window_height(cfg) * self.scale);
        let px = unsafe { std::slice::from_raw_parts(self.bits, (w * h) as usize) };
        let row_bytes = ((w * 3 + 3) / 4) * 4;
        let size = 54 + row_bytes * h;
        let mut out: Vec<u8> = Vec::with_capacity(size as usize);
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&(size as u32).to_le_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(&54u32.to_le_bytes());
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&w.to_le_bytes());
        out.extend_from_slice(&h.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&24u16.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&((row_bytes * h) as u32).to_le_bytes());
        out.extend_from_slice(&2835u32.to_le_bytes());
        out.extend_from_slice(&2835u32.to_le_bytes());
        out.extend_from_slice(&[0; 8]);
        let backdrop = [0x36u32, 0x2e, 0x2a]; // b, g, r
        for y in (0..h).rev() {
            for x in 0..w {
                let p = px[(y * w + x) as usize];
                let inv = 255 - (p >> 24);
                for (shift, bg) in [(0u32, backdrop[0]), (8, backdrop[1]), (16, backdrop[2])] {
                    out.push((((p >> shift) & 0xFF) + bg * inv / 255).min(255) as u8);
                }
            }
            out.resize(out.len() + (row_bytes - w * 3) as usize, 0);
        }
        let _ = std::fs::write(path, out);
    }

    pub fn show(&mut self, cfg: &Config) {
        self.visible = true;
        self.painted = None;
        self.paint(cfg);
        unsafe {
            ShowWindow(self.hwnd, SW_SHOW);
            SetForegroundWindow(self.hwnd);
            if !cfg.reduce_motion {
                SetTimer(self.hwnd, TIMER_TITLE, TITLE_TICK_MS, None);
            }
        }
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.menu.mouse_leave();
        unsafe {
            KillTimer(self.hwnd, TIMER_TITLE);
            ShowWindow(self.hwnd, SW_HIDE);
        }
    }
}

impl Drop for Hud {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.mem_dc, self.old_bitmap);
            DeleteObject(self.dib);
            DeleteDC(self.mem_dc);
            DestroyWindow(self.hwnd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::{Action, Group, Section};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

    /// The tallest, busiest menu: every section open, every feature on.
    fn busiest_menu() -> (Hud, Config) {
        let mut hud = unsafe { Hud::create(GetModuleHandleW(null()), Some(DefWindowProcW)) }.expect("menu window");
        let mut cfg = Config::default();
        for s in [Section::Presets, Section::States, Section::Output, Section::Display, Section::Preview, Section::Experimental] {
            hud.menu.activate(&mut cfg, Action::Header(s), 1);
        }
        for g in [Group::Blend, Group::Dynamics, Group::Trace, Group::Zoom, Group::Aspect, Group::Auto, Group::Color, Group::Transform, Group::Jump, Group::Sprint, Group::Crouch] {
            hud.menu.activate(&mut cfg, Action::Group(g), 1);
        }
        for st in crate::states::ALL {
            cfg.states.rule_mut(st).key = 0x10;
        }
        cfg.exp.dynamics.enabled = true;
        cfg.exp.trace.enabled = true;
        cfg.exp.zoom.enabled = true;
        cfg.exp.aspect.enabled = true;
        cfg.exp.auto.enabled = true;
        // Really on screen (but never stealing focus), so the compositor cost is included.
        hud.visible = true;
        hud.paint(&cfg);
        unsafe { ShowWindow(hud.hwnd, SW_SHOWNOACTIVATE) };
        (hud, cfg)
    }

    /// The glyph cache must draw exactly the pixels the old whole-string GDI path drew.
    #[test]
    fn glyph_cache_draws_the_same_pixels_as_drawing_the_whole_string() {
        let (hud, _cfg) = busiest_menu();
        let samples = ["Spray trace", "Show on screen capture", "0123456789 :.%x", "reDraw [Dev]", "2026-10-29 11:11", "Light/dark", "> x 01 Spokes", "15\u{b0}", "Profile \u{b7} Hip fire", "oc1 spk.00ff66.6.4.3.4.0 dot.00ff66.1.4.1.4.0"];
        for font in [Font::Body, Font::Title, Font::Small] {
            for text in samples {
                // The old whole-string path only had a 256px scratch bitmap; longer runs never occur in the menu.
                if text.chars().count() as i32 * hud.text.cell(font) > 250 {
                    continue;
                }
                let (w, h) = (300, 24);
                let (mut cached, mut whole) = (vec![0u32; (w * h) as usize], vec![0u32; (w * h) as usize]);
                hud.text.draw(&mut Canvas::new(&mut cached, w, h), 3, 2, 18, text, 0xE6E9EF, font);
                hud.text.draw_whole_run(&mut Canvas::new(&mut whole, w, h), 3, 2, 18, text, 0xE6E9EF, font);
                assert!(cached == whole, "{font:?} {text:?}: glyph cache differs from whole-string drawing");
                assert!(cached.iter().any(|p| *p != 0), "{font:?} {text:?}: something was drawn");
            }
        }
    }

    /// The paste path against the real clipboard. It borrows the clipboard for a moment and puts
    /// back whatever text was on it, so it is ignored unless asked for:
    ///   cargo test clipboard -- --ignored
    #[test]
    #[ignore = "touches the real clipboard"]
    fn clipboard_text_round_trips() {
        let before = get_clipboard(null_mut());
        let code = "oc2 o1 p10 chv.ff9500.5.2.1.1.270.o6.x3.mb.c2 dot.00ff66.1.4.1.4.0";
        set_clipboard(null_mut(), code);
        assert_eq!(get_clipboard(null_mut()).as_deref(), Some(code), "what was copied is what is read back");
        let shared = crate::share::decode(&get_clipboard(null_mut()).unwrap(), &crate::crosshair::Crosshair::classic());
        assert!(shared.and_then(|s| s.single).is_some(), "and it decodes as a share code");
        let unicode = "caf\u{e9} \u{1f3af} \u{4e2d}\u{6587}";
        set_clipboard(null_mut(), unicode);
        assert_eq!(get_clipboard(null_mut()).as_deref(), Some(unicode), "non-ASCII text survives");
        set_clipboard(null_mut(), &"x".repeat(20_000));
        assert_eq!(get_clipboard(null_mut()), None, "an unreasonably large clipboard is refused, not read");
        if let Some(text) = before {
            set_clipboard(null_mut(), &text);
        }
    }

    /// Gliding the cursor down the whole menu must stay cheap: this is the "it lags when I
    /// swipe over the menu" symptom. Timing, so run it alone and in release:
    ///   cargo test --release hover_glide -- --ignored --nocapture
    #[test]
    #[ignore = "timing test: run alone, in release"]
    fn hover_glide_stays_within_budget() {
        const BUDGET_P95_MS: f64 = 3.0;
        let (mut hud, mut cfg) = busiest_menu();
        let ms = |t: Instant| t.elapsed().as_secs_f64() * 1000.0;

        let t = Instant::now();
        for _ in 0..1000 {
            std::hint::black_box(hud.menu.layout(&cfg));
        }
        let layout_us = ms(t);
        let t = Instant::now();
        for _ in 0..50 {
            hud.menu.mouse_leave();
            hud.paint(&cfg);
        }
        let full_ms = ms(t) / 50.0;
        // Where does a full redraw spend its time: drawing into memory, or showing the window?
        let layout = hud.menu.layout(&cfg);
        let h = layout.height.min(hud.max_h);
        let t = Instant::now();
        for _ in 0..50 {
            let px = &mut hud.logical[..(HUD_W * h) as usize];
            paint::paint(&mut Canvas::new(px, HUD_W, h), &layout, &cfg, &hud.menu, &hud.text, cfg.accent, 0.0);
        }
        let draw_ms = ms(t) / 50.0;
        let t = Instant::now();
        for _ in 0..50 {
            hud.present(h, None);
        }
        let show_ms = ms(t) / 50.0;
        println!("   of which: drawing {draw_ms:.2} ms, showing the window {show_ms:.2} ms");

        // 3px steps from the top to the bottom of the menu, x wandering like a hand would.
        let height = hud.menu.window_height(&cfg);
        let mut times: Vec<f64> = Vec::new();
        let mut redraws = 0;
        let (mut y, mut step) = (4, 0);
        while y < height - 4 {
            let t = Instant::now();
            let fx = hud.menu.mouse_move(&mut cfg, 40 + (step * 7) % 120, y, false);
            if fx.repaint {
                redraws += 1;
                hud.paint(&cfg);
            }
            times.push(ms(t));
            y += 3;
            step += 1;
        }
        unsafe { ShowWindow(hud.hwnd, SW_HIDE) };
        let mut sorted = times.clone();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let p = |q: f64| sorted[((sorted.len() - 1) as f64 * q) as usize];
        let total: f64 = times.iter().sum();
        println!("menu height {height}px, {} hover events, {redraws} redraws", times.len());
        println!("layout build: {layout_us:.1} us each");
        println!("full redraw (draw + show): {full_ms:.2} ms each");
        println!("per hover event: avg {:.2} ms, p50 {:.2}, p95 {:.2}, max {:.2}", total / times.len() as f64, p(0.5), p(0.95), sorted[sorted.len() - 1]);
        println!("whole glide: {total:.0} ms of work for {} events", times.len());
        assert!(p(0.95) <= BUDGET_P95_MS, "p95 {:.2} ms is over the {BUDGET_P95_MS:.1} ms budget", p(0.95));
    }
}

/// The clipboard's Unicode text, if it holds some and it is not unreasonably large.
/// Only called when the user asks to paste (a menu click), never in the background.
pub fn get_clipboard(owner: HWND) -> Option<String> {
    const CF_UNICODETEXT: u32 = 13;
    const MAX_UNITS: usize = 8192;
    unsafe {
        if OpenClipboard(owner) == 0 {
            return None;
        }
        let mut text = None;
        let handle = GetClipboardData(CF_UNICODETEXT);
        if !handle.is_null() {
            let units = GlobalSize(handle) / 2;
            let src = GlobalLock(handle) as *const u16;
            if !src.is_null() {
                if units <= MAX_UNITS {
                    let all = std::slice::from_raw_parts(src, units);
                    let end = all.iter().position(|c| *c == 0).unwrap_or(all.len());
                    text = Some(String::from_utf16_lossy(&all[..end]));
                }
                GlobalUnlock(handle);
            }
        }
        CloseClipboard();
        text
    }
}

/// Put `text` on the clipboard as Unicode text.
pub fn set_clipboard(owner: HWND, text: &str) {
    let utf16 = wide(text);
    unsafe {
        if OpenClipboard(owner) == 0 {
            return;
        }
        EmptyClipboard();
        let bytes = utf16.len() * 2;
        let mem = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if !mem.is_null() {
            let dst = GlobalLock(mem) as *mut u16;
            if !dst.is_null() {
                std::ptr::copy_nonoverlapping(utf16.as_ptr(), dst, utf16.len());
                GlobalUnlock(mem);
                const CF_UNICODETEXT: u32 = 13;
                if SetClipboardData(CF_UNICODETEXT, mem as HANDLE).is_null() {
                    GlobalFree(mem);
                }
            } else {
                GlobalFree(mem);
            }
        }
        CloseClipboard();
    }
}
