//! OpenStudio painting: a CPU canvas with shapes (tiny-skia) and text (fontdue).
//!
//! All coordinates are logical pixels; the canvas multiplies by `scale` (the window's DPI factor).
//! Text uses a system font loaded at runtime (nothing is bundled).
// spec: ui/main-window.md §10 (typography)

use std::collections::HashMap;

pub use os_theme::Color;
use tiny_skia::{FillRule, LineCap, Mask, Paint, PathBuilder, Pixmap, PremultipliedColorU8, Stroke, Transform};

/// An axis-aligned rectangle in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Rect { x, y, w, h }
    }
    /// From inclusive pixel ranges, as the spec tables give them (`x0–x1`, `y0–y1`).
    pub const fn span(x0: f32, x1: f32, y0: f32, y1: f32) -> Self {
        Rect { x: x0, y: y0, w: x1 - x0 + 1.0, h: y1 - y0 + 1.0 }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn cx(&self) -> f32 {
        self.x + self.w / 2.0
    }
    pub fn cy(&self) -> f32 {
        self.y + self.h / 2.0
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
    pub fn inset(&self, d: f32) -> Rect {
        Rect::new(self.x + d, self.y + d, (self.w - 2.0 * d).max(0.0), (self.h - 2.0 * d).max(0.0))
    }
    pub fn intersect(&self, o: &Rect) -> Rect {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        Rect::new(x, y, (self.right().min(o.right()) - x).max(0.0), (self.bottom().min(o.bottom()) - y).max(0.0))
    }
    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }
    pub fn union(&self, o: &Rect) -> Rect {
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        Rect::new(x, y, self.right().max(o.right()) - x, self.bottom().max(o.bottom()) - y)
    }
    pub fn outset(&self, d: f32) -> Rect {
        Rect::new(self.x - d, self.y - d, self.w + 2.0 * d, self.h + 2.0 * d)
    }
}

/// Which system font face to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Face {
    /// Condensed UI text.
    Ui,
    /// Thin display digits (song position).
    Light,
}

/// Horizontal scale applied to UI glyphs: the UI face is a condensed look.
const CONDENSE: f32 = 0.84;

/// Squeeze a rasterized glyph horizontally (box filter), keeping its height.
fn condense(mut m: fontdue::Metrics, cov: Vec<u8>, k: f32) -> (fontdue::Metrics, Vec<u8>) {
    m.advance_width *= k;
    if m.width == 0 || m.height == 0 {
        return (m, cov);
    }
    let nw = ((m.width as f32 * k).round() as usize).max(1);
    let mut out = vec![0u8; nw * m.height];
    let ratio = m.width as f32 / nw as f32;
    for y in 0..m.height {
        for x in 0..nw {
            let (a, b) = (x as f32 * ratio, (x + 1) as f32 * ratio);
            let (mut sum, mut wsum) = (0.0f32, 0.0f32);
            let mut i = a.floor() as usize;
            while (i as f32) < b && i < m.width {
                let w = (b.min(i as f32 + 1.0) - a.max(i as f32)).max(0.0);
                sum += f32::from(cov[y * m.width + i]) * w;
                wsum += w;
                i += 1;
            }
            out[y * nw + x] = if wsum > 0.0 { (sum / wsum).round() as u8 } else { 0 };
        }
    }
    m.xmin = (m.xmin as f32 * k).round() as i32;
    m.width = nw;
    (m, out)
}

struct Glyph {
    metrics: fontdue::Metrics,
    coverage: Vec<u8>,
}

struct Fonts {
    ui: fontdue::Font,
    light: fontdue::Font,
    cache: HashMap<(Face, char, u32), Glyph>,
}

fn load_font(candidates: &[&str]) -> Option<fontdue::Font> {
    let dirs: Vec<std::path::PathBuf> = std::env::var_os("WINDIR")
        .map(|w| std::path::PathBuf::from(w).join("Fonts"))
        .into_iter()
        .chain(["/usr/share/fonts/truetype/dejavu", "/System/Library/Fonts"].map(Into::into))
        .collect();
    for name in candidates {
        for d in &dirs {
            if let Ok(bytes) = std::fs::read(d.join(name)) {
                if let Ok(f) = fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()) {
                    return Some(f);
                }
            }
        }
    }
    None
}

impl Fonts {
    fn load() -> Result<Fonts, String> {
        let ui = load_font(&["bahnschrift.ttf", "segoeui.ttf", "DejaVuSansCondensed.ttf", "Helvetica.ttc"])
            .ok_or("no usable system UI font found")?;
        let light = load_font(&["segoeuil.ttf", "segoeui.ttf", "DejaVuSans.ttf"]).unwrap_or_else(|| ui.clone());
        Ok(Fonts { ui, light, cache: HashMap::new() })
    }

    fn font(&self, face: Face) -> &fontdue::Font {
        match face {
            Face::Ui => &self.ui,
            Face::Light => &self.light,
        }
    }

    fn glyph(&mut self, face: Face, c: char, px: f32) -> &Glyph {
        let key = (face, c, (px * 4.0).round() as u32);
        if !self.cache.contains_key(&key) {
            let (mut metrics, mut coverage) = self.font(face).rasterize(c, px);
            if face == Face::Ui {
                (metrics, coverage) = condense(metrics, coverage, CONDENSE);
            }
            self.cache.insert(key, Glyph { metrics, coverage });
        }
        &self.cache[&key]
    }

    fn ascent(&self, face: Face, px: f32) -> f32 {
        self.font(face).horizontal_line_metrics(px).map_or(px * 0.8, |m| m.ascent)
    }
}

/// The frame being painted.
pub struct Canvas {
    pub pixmap: Pixmap,
    pub scale: f32,
    fonts: Fonts,
    clips: Vec<Rect>,
    /// Clip masks for path drawing, built lazily and reused across frames.
    masks: HashMap<[i32; 4], Mask>,
}

fn paint(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(c.r, c.g, c.b, c.a);
    p.anti_alias = true;
    p
}

impl Canvas {
    pub fn new(width: u32, height: u32, scale: f32) -> Result<Canvas, String> {
        Ok(Canvas {
            pixmap: Pixmap::new(width.max(1), height.max(1)).ok_or("bad canvas size")?,
            scale,
            fonts: Fonts::load()?,
            clips: Vec::new(),
            masks: HashMap::new(),
        })
    }

    /// Resize (contents are cleared).
    pub fn resize(&mut self, width: u32, height: u32, scale: f32) {
        if let Some(p) = Pixmap::new(width.max(1), height.max(1)) {
            self.pixmap = p;
        }
        self.scale = scale;
        self.clips.clear();
    }

    /// Logical size.
    pub fn size(&self) -> (f32, f32) {
        (self.pixmap.width() as f32 / self.scale, self.pixmap.height() as f32 / self.scale)
    }

    fn transform(&self) -> Transform {
        Transform::from_scale(self.scale, self.scale)
    }

    /// Current clip in logical pixels.
    pub fn clip_rect(&self) -> Rect {
        self.clips.last().copied().unwrap_or_else(|| {
            let (w, h) = self.size();
            Rect::new(0.0, 0.0, w, h)
        })
    }

    /// Restrict painting to `r` (intersected with the current clip) until `pop_clip`.
    pub fn push_clip(&mut self, r: Rect) {
        let r = r.intersect(&self.clip_rect());
        self.clips.push(r);
    }

    pub fn pop_clip(&mut self) {
        self.clips.pop();
    }

    /// Device-pixel bounds of the current clip: (x0, y0, x1, y1), end-exclusive.
    fn clip_px(&self) -> (i32, i32, i32, i32) {
        let c = self.clip_rect();
        let s = self.scale;
        (
            ((c.x * s).round() as i32).max(0),
            ((c.y * s).round() as i32).max(0),
            ((c.right() * s).round() as i32).min(self.pixmap.width() as i32),
            ((c.bottom() * s).round() as i32).min(self.pixmap.height() as i32),
        )
    }

    /// Make sure a mask exists for the current clip; returns its key (None when unclipped).
    fn ensure_mask(&mut self, bounds: tiny_skia::Rect, pad: f32) -> Option<[i32; 4]> {
        let (x0, y0, x1, y1) = self.clip_px();
        if self.clips.is_empty() {
            return None;
        }
        // A path entirely inside the (rectangular) clip needs no mask.
        let k = self.scale;
        let inside = (bounds.left() - pad) * k >= x0 as f32
            && (bounds.top() - pad) * k >= y0 as f32
            && (bounds.right() + pad) * k <= x1 as f32
            && (bounds.bottom() + pad) * k <= y1 as f32;
        if inside {
            return None;
        }
        let key = [x0, y0, x1, y1];
        if !self.masks.contains_key(&key) {
            if self.masks.len() > 512 {
                self.masks.clear();
            }
            let mut mask = Mask::new(self.pixmap.width(), self.pixmap.height())?;
            if let Some(rect) = tiny_skia::Rect::from_ltrb(x0 as f32, y0 as f32, (x1.max(x0 + 1)) as f32, (y1.max(y0 + 1)) as f32) {
                mask.fill_path(&PathBuilder::from_rect(rect), FillRule::Winding, false, Transform::identity());
            }
            self.masks.insert(key, mask);
        }
        Some(key)
    }

    /// Start a frame. With `damage`, everything is clipped to that rectangle (the rest of the
    /// canvas keeps last frame's pixels) and primitives outside it are skipped outright.
    pub fn begin_frame(&mut self, damage: Option<Rect>) {
        self.clips.clear();
        if let Some(d) = damage {
            let (w, h) = self.size();
            self.clips.push(d.intersect(&Rect::new(0.0, 0.0, w, h)));
        }
    }

    pub fn clear(&mut self, c: Color) {
        let (w, h) = self.size();
        self.fill_rect(Rect::new(0.0, 0.0, w, h), c);
    }

    /// True when `b` (logical, padded by `pad`) lies entirely outside the current clip.
    fn outside(&self, b: tiny_skia::Rect, pad: f32) -> bool {
        let c = self.clip_rect();
        b.right() + pad < c.x || b.left() - pad > c.right() || b.bottom() + pad < c.y || b.top() - pad > c.bottom()
    }

    /// Device-pixel rectangle of a logical one, clamped to the canvas: (x, y, w, h).
    pub fn device_rect(&self, r: Rect) -> (usize, usize, usize, usize) {
        let k = self.scale;
        let x0 = ((r.x * k).floor().max(0.0) as usize).min(self.pixmap.width() as usize);
        let y0 = ((r.y * k).floor().max(0.0) as usize).min(self.pixmap.height() as usize);
        let x1 = ((r.right() * k).ceil().max(0.0) as usize).min(self.pixmap.width() as usize);
        let y1 = ((r.bottom() * k).ceil().max(0.0) as usize).min(self.pixmap.height() as usize);
        (x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0))
    }

    /// Axis-aligned rectangle, snapped to device pixels. Bypasses the path rasterizer.
    pub fn fill_rect(&mut self, r: Rect, c: Color) {
        self.fill_rect_vgrad(r, c, c);
    }

    fn fill_rect_vgrad(&mut self, r: Rect, top: Color, bottom: Color) {
        let sc = self.scale;
        let (cx0, cy0, cx1, cy1) = self.clip_px();
        let x0 = ((r.x * sc).round() as i32).max(cx0);
        let x1 = ((r.right() * sc).round() as i32).min(cx1);
        let ry0 = (r.y * sc).round() as i32;
        let ry1 = (r.bottom() * sc).round() as i32;
        let (y0, y1) = (ry0.max(cy0), ry1.min(cy1));
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let pw = self.pixmap.width() as usize;
        let span = (ry1 - ry0).max(1) as f32;
        let flat = top == bottom;
        let pixels = self.pixmap.pixels_mut();
        for y in y0..y1 {
            let c = if flat { top } else { top.mix(bottom, (y - ry0) as f32 / span) };
            let row = &mut pixels[y as usize * pw + x0 as usize..y as usize * pw + x1 as usize];
            if c.a == 255 {
                row.fill(PremultipliedColorU8::from_rgba(c.r, c.g, c.b, 255).expect("opaque"));
            } else {
                let (sr, sg, sb, sa) = (u32::from(c.r), u32::from(c.g), u32::from(c.b), u32::from(c.a));
                for px in row.iter_mut() {
                    // Opaque canvas: destination is already premultiplied with alpha 255.
                    let mix = |s: u32, d: u8| ((s * sa + u32::from(d) * (255 - sa)) / 255) as u8;
                    *px = PremultipliedColorU8::from_rgba(mix(sr, px.red()), mix(sg, px.green()), mix(sb, px.blue()), 255)
                        .expect("opaque");
                }
            }
        }
    }

    /// Vertical gradient fill (rounded when `radius > 0`).
    pub fn fill_rect_gradient(&mut self, r: Rect, top: Color, bottom: Color, radius: f32) {
        if radius <= 0.0 {
            self.fill_rect_vgrad(r, top, bottom);
            return;
        }
        let Some(path) = rrect_path(r, radius) else { return };
        if self.outside(path.bounds(), 1.0) {
            return;
        }
        let tc = |c: Color| tiny_skia::Color::from_rgba8(c.r, c.g, c.b, c.a);
        let Some(shader) = tiny_skia::LinearGradient::new(
            tiny_skia::Point::from_xy(0.0, r.y * self.scale),
            tiny_skia::Point::from_xy(0.0, r.bottom() * self.scale),
            vec![tiny_skia::GradientStop::new(0.0, tc(top)), tiny_skia::GradientStop::new(1.0, tc(bottom))],
            tiny_skia::SpreadMode::Pad,
            Transform::identity(),
        ) else {
            return;
        };
        let p = Paint { shader, anti_alias: true, ..Paint::default() };
        let t = self.transform();
        let key = self.ensure_mask(path.bounds(), 1.0);
        let mask = key.and_then(|k| self.masks.get(&k));
        self.pixmap.fill_path(&path, &p, FillRule::Winding, t, mask);
    }

    pub fn fill_rrect(&mut self, r: Rect, radius: f32, c: Color) {
        // Large opaque shapes: a native middle plus two small corner caps (anti-aliased paths are
        // proportional to area, which makes tall strips expensive).
        let rad = radius.min(r.w / 2.0).min(r.h / 2.0);
        if c.a == 255 && rad > 0.0 && r.w * r.h > 1500.0 && r.h > 4.0 * rad + 2.0 {
            self.fill_rect(Rect::new(r.x, r.y + rad, r.w, r.h - 2.0 * rad), c);
            for cap in [Rect::new(r.x, r.y, r.w, 2.0 * rad), Rect::new(r.x, r.bottom() - 2.0 * rad, r.w, 2.0 * rad)] {
                if let Some(path) = rrect_path(cap, rad) {
                    self.fill(&path, c);
                }
            }
            return;
        }
        if let Some(path) = rrect_path(r, radius) {
            self.fill(&path, c);
        }
    }

    pub fn stroke_rrect(&mut self, r: Rect, radius: f32, width: f32, c: Color) {
        if radius <= 2.5 && c.a == 255 && (r.w > 40.0 || r.h > 40.0) {
            // Square-cornered outline from four native rects.
            self.fill_rect(Rect::new(r.x, r.y, r.w, width), c);
            self.fill_rect(Rect::new(r.x, r.bottom() - width, r.w, width), c);
            self.fill_rect(Rect::new(r.x, r.y + width, width, r.h - 2.0 * width), c);
            self.fill_rect(Rect::new(r.right() - width, r.y + width, width, r.h - 2.0 * width), c);
            return;
        }
        let r = r.inset(width / 2.0);
        if let Some(path) = rrect_path(r, radius) {
            self.stroke(&path, width, c);
        }
    }

    pub fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, c: Color) {
        let mut pb = PathBuilder::new();
        pb.move_to(x0, y0);
        pb.line_to(x1, y1);
        if let Some(path) = pb.finish() {
            self.stroke(&path, width, c);
        }
    }

    /// Polyline (open) or polygon outline.
    pub fn polyline(&mut self, pts: &[(f32, f32)], width: f32, c: Color) {
        if let Some(path) = poly_path(pts, false) {
            self.stroke(&path, width, c);
        }
    }

    pub fn fill_polygon(&mut self, pts: &[(f32, f32)], c: Color) {
        if let Some(path) = poly_path(pts, true) {
            self.fill(&path, c);
        }
    }

    pub fn fill_circle(&mut self, cx: f32, cy: f32, r: f32, c: Color) {
        if let Some(path) = PathBuilder::from_circle(cx, cy, r) {
            self.fill(&path, c);
        }
    }

    pub fn stroke_circle(&mut self, cx: f32, cy: f32, r: f32, width: f32, c: Color) {
        if let Some(path) = PathBuilder::from_circle(cx, cy, r) {
            self.stroke(&path, width, c);
        }
    }

    /// Arc from `start` sweeping `sweep` radians (0 = 3 o'clock, positive = clockwise on screen).
    pub fn arc(&mut self, cx: f32, cy: f32, r: f32, start: f32, sweep: f32, width: f32, c: Color) {
        let steps = ((sweep.abs() * r / 2.0).ceil() as usize).clamp(4, 128);
        let pts: Vec<(f32, f32)> = (0..=steps)
            .map(|i| {
                let a = start + sweep * i as f32 / steps as f32;
                (cx + r * a.cos(), cy + r * a.sin())
            })
            .collect();
        if let Some(path) = poly_path(&pts, false) {
            if self.outside(path.bounds(), width + 1.0) {
                return;
            }
            let mut s = Stroke { width, ..Stroke::default() };
            s.line_cap = LineCap::Round;
            let t = self.transform();
            let p = paint(c);
            let key = self.ensure_mask(path.bounds(), width + 1.0);
            let mask = key.and_then(|k| self.masks.get(&k));
            self.pixmap.stroke_path(&path, &p, &s, t, mask);
        }
    }

    pub fn fill(&mut self, path: &tiny_skia::Path, c: Color) {
        if self.outside(path.bounds(), 1.0) {
            return;
        }
        let t = self.transform();
        let p = paint(c);
        let key = self.ensure_mask(path.bounds(), 1.0);
        let mask = key.and_then(|k| self.masks.get(&k));
        self.pixmap.fill_path(path, &p, FillRule::Winding, t, mask);
    }

    pub fn stroke(&mut self, path: &tiny_skia::Path, width: f32, c: Color) {
        if self.outside(path.bounds(), width + 1.0) {
            return;
        }
        let t = self.transform();
        let p = paint(c);
        let s = Stroke { width, ..Stroke::default() };
        let key = self.ensure_mask(path.bounds(), width + 1.0);
        let mask = key.and_then(|k| self.masks.get(&k));
        self.pixmap.stroke_path(path, &p, &s, t, mask);
    }

    /// Width of `s` in logical pixels.
    pub fn text_width(&mut self, face: Face, size: f32, s: &str) -> f32 {
        let px = size * self.scale;
        let w: f32 = s.chars().map(|c| self.fonts.glyph(face, c, px).metrics.advance_width).sum();
        w / self.scale
    }

    /// Draw `s` with its top-left at (x, y) (top = ascent line). Returns the advance width.
    pub fn text(&mut self, face: Face, size: f32, x: f32, y: f32, c: Color, s: &str) -> f32 {
        self.text_dir(face, size, x, y, c, s, false)
    }

    /// Text centered in `r` (both axes).
    pub fn text_center(&mut self, face: Face, size: f32, r: Rect, c: Color, s: &str) {
        let w = self.text_width(face, size, s);
        self.text(face, size, r.cx() - w / 2.0, r.cy() - size * 0.5, c, s);
    }

    /// Text that shrinks (down to 70 %) to fit `max_w`, then is clipped to it. Returns the size used.
    pub fn text_fit(&mut self, face: Face, size: f32, x: f32, y: f32, max_w: f32, c: Color, s: &str) -> f32 {
        let mut size = size;
        let w = self.text_width(face, size, s);
        if w > max_w {
            size = (size * max_w / w).max(size * 0.7);
        }
        self.push_clip(Rect::new(x, y - 2.0, max_w, size * 1.5));
        self.text(face, size, x, y, c, s);
        self.pop_clip();
        size
    }

    /// Text right-aligned to `right`.
    pub fn text_right(&mut self, face: Face, size: f32, right: f32, y: f32, c: Color, s: &str) {
        let w = self.text_width(face, size, s);
        self.text(face, size, right - w, y, c, s);
    }

    /// Text rotated 90° counter-clockwise (reads bottom to top), starting at (x, y_bottom),
    /// with the glyph tops facing left at `x`.
    pub fn text_vertical(&mut self, face: Face, size: f32, x: f32, y_bottom: f32, c: Color, s: &str) -> f32 {
        self.text_dir(face, size, x, y_bottom, c, s, true)
    }

    fn text_dir(&mut self, face: Face, size: f32, x: f32, y: f32, c: Color, s: &str, vertical: bool) -> f32 {
        let scale = self.scale;
        let px = size * scale;
        let cr = self.clip_rect();
        let outside = if vertical {
            // Rotated text runs upward from `y` (at most a few hundred pixels).
            x > cr.right() || x + size * 1.6 < cr.x || y < cr.y || y - 600.0 > cr.bottom()
        } else {
            y > cr.bottom() || y + size * 1.6 < cr.y || x > cr.right()
        };
        if outside {
            return 0.0;
        }
        let ascent = self.fonts.ascent(face, px);
        // Fold the font's internal leading so `y` is close to the cap top.
        let top_pad = (ascent - px * 0.78).max(0.0);
        let (pw, ph) = (self.pixmap.width() as i32, self.pixmap.height() as i32);
        let clip = self.clip_rect();
        let (cx0, cy0) = ((clip.x * scale).floor() as i32, (clip.y * scale).floor() as i32);
        let (cx1, cy1) = ((clip.right() * scale).ceil() as i32, (clip.bottom() * scale).ceil() as i32);
        let (cx0, cy0, cx1, cy1) = (cx0.max(0), cy0.max(0), cx1.min(pw), cy1.min(ph));
        let mut pen = 0.0f32;
        let ox = x * scale;
        let oy = y * scale;
        for ch in s.chars() {
            let g = self.fonts.glyph(face, ch, px);
            let m = g.metrics;
            // Glyph-local origin: baseline at `ascent - top_pad` below the line top.
            let gx0 = pen + m.xmin as f32;
            let gy0 = ascent - top_pad - m.ymin as f32 - m.height as f32;
            let data = self.pixmap.data_mut();
            for j in 0..m.height {
                for i in 0..m.width {
                    let a = g.coverage[j * m.width + i];
                    if a == 0 {
                        continue;
                    }
                    let (lx, ly) = (gx0 + i as f32, gy0 + j as f32);
                    let (dx, dy) = if vertical { (ox + ly, oy - lx) } else { (ox + lx, oy + ly) };
                    let (dx, dy) = (dx.round() as i32, dy.round() as i32);
                    if dx < cx0 || dy < cy0 || dx >= cx1 || dy >= cy1 {
                        continue;
                    }
                    let k = ((dy * pw + dx) * 4) as usize;
                    let alpha = u32::from(a) * u32::from(c.a) / 255;
                    for (ch_i, v) in [c.r, c.g, c.b].into_iter().enumerate() {
                        let d = u32::from(data[k + ch_i]);
                        data[k + ch_i] = ((u32::from(v) * alpha + d * (255 - alpha)) / 255) as u8;
                    }
                    data[k + 3] = 255;
                }
            }
            pen += m.advance_width;
        }
        pen / scale
    }

    /// Copy a device-pixel region into a `0x00RRGGBB` buffer of the same width.
    pub fn copy_region_to_xrgb(&self, out: &mut [u32], (x, y, w, h): (usize, usize, usize, usize)) {
        let stride = self.pixmap.width() as usize;
        let px = self.pixmap.pixels();
        for row in y..y + h {
            let src = &px[row * stride + x..row * stride + x + w];
            let dst = &mut out[row * stride + x..row * stride + x + w];
            for (o, p) in dst.iter_mut().zip(src) {
                *o = (u32::from(p.red()) << 16) | (u32::from(p.green()) << 8) | u32::from(p.blue());
            }
        }
    }

    /// Copy the frame into a `0x00RRGGBB` buffer (softbuffer's format).
    pub fn copy_to_xrgb(&self, out: &mut [u32]) {
        for (o, p) in out.iter_mut().zip(self.pixmap.data().chunks_exact(4)) {
            *o = (u32::from(p[0]) << 16) | (u32::from(p[1]) << 8) | u32::from(p[2]);
        }
    }
}

fn rrect_path(r: Rect, radius: f32) -> Option<tiny_skia::Path> {
    if r.w <= 0.0 || r.h <= 0.0 {
        return None;
    }
    let rad = radius.min(r.w / 2.0).min(r.h / 2.0);
    if rad <= 0.0 {
        return Some(PathBuilder::from_rect(tiny_skia::Rect::from_xywh(r.x, r.y, r.w, r.h)?));
    }
    let (x0, y0, x1, y1) = (r.x, r.y, r.right(), r.bottom());
    let k = rad * 0.4477; // cubic approximation of a quarter circle
    let mut pb = PathBuilder::new();
    pb.move_to(x0 + rad, y0);
    pb.line_to(x1 - rad, y0);
    pb.cubic_to(x1 - k, y0, x1, y0 + k, x1, y0 + rad);
    pb.line_to(x1, y1 - rad);
    pb.cubic_to(x1, y1 - k, x1 - k, y1, x1 - rad, y1);
    pb.line_to(x0 + rad, y1);
    pb.cubic_to(x0 + k, y1, x0, y1 - k, x0, y1 - rad);
    pb.line_to(x0, y0 + rad);
    pb.cubic_to(x0, y0 + k, x0 + k, y0, x0 + rad, y0);
    pb.close();
    pb.finish()
}

fn poly_path(pts: &[(f32, f32)], close: bool) -> Option<tiny_skia::Path> {
    let (first, rest) = pts.split_first()?;
    let mut pb = PathBuilder::new();
    pb.move_to(first.0, first.1);
    for p in rest {
        pb.line_to(p.0, p.1);
    }
    if close {
        pb.close();
    }
    pb.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_span_is_inclusive() {
        let r = Rect::span(73.0, 361.0, 5.0, 35.0);
        assert_eq!((r.w, r.h), (289.0, 31.0));
        assert!(r.contains(361.5, 35.5) && !r.contains(362.0, 20.0));
    }

    #[test]
    fn paints_text_and_shapes() {
        let Ok(mut c) = Canvas::new(200, 50, 1.0) else { return }; // no system font: skip
        c.clear(Color::rgb(0x000000));
        c.fill_rrect(Rect::new(5.0, 5.0, 20.0, 20.0), 3.0, Color::rgb(0xff0000));
        let w = c.text(Face::Ui, 14.0, 40.0, 10.0, Color::rgb(0xffffff), "OpenStudio");
        assert!(w > 40.0);
        assert!(c.pixmap.data().chunks_exact(4).any(|p| p[0] > 200 && p[1] > 200));
    }
}
