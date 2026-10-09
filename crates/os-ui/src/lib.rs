//! OpenStudio painting: a CPU canvas with shapes (tiny-skia) and text (fontdue).
//!
//! All coordinates are logical pixels; the canvas multiplies by `scale` (the window's DPI factor).
//! Text uses a system font loaded at runtime (nothing is bundled).
// spec: ui/main-window.md §10 (typography)

use std::collections::HashMap;

pub use os_theme::Color;
use tiny_skia::{FillRule, LineCap, Mask, Paint, PathBuilder, Pixmap, Stroke, Transform};

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
}

/// Which system font face to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Face {
    /// Condensed UI text.
    Ui,
    /// Thin display digits (song position).
    Light,
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
            let (metrics, coverage) = self.font(face).rasterize(c, px);
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
    clips: Vec<(Rect, Option<Mask>)>,
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
        self.clips.last().map_or_else(
            || {
                let (w, h) = self.size();
                Rect::new(0.0, 0.0, w, h)
            },
            |(r, _)| *r,
        )
    }

    /// Restrict painting to `r` (intersected with the current clip) until `pop_clip`.
    pub fn push_clip(&mut self, r: Rect) {
        let r = r.intersect(&self.clip_rect());
        let mut mask = Mask::new(self.pixmap.width(), self.pixmap.height());
        if let (Some(m), Some(rect)) = (mask.as_mut(), tiny_skia::Rect::from_xywh(r.x, r.y, r.w.max(0.01), r.h.max(0.01))) {
            let path = PathBuilder::from_rect(rect);
            m.fill_path(&path, FillRule::Winding, false, self.transform());
        }
        self.clips.push((r, mask));
    }

    pub fn pop_clip(&mut self) {
        self.clips.pop();
    }

    pub fn clear(&mut self, c: Color) {
        self.pixmap.fill(tiny_skia::Color::from_rgba8(c.r, c.g, c.b, 255));
    }

    pub fn fill_rect(&mut self, r: Rect, c: Color) {
        if let Some(rect) = tiny_skia::Rect::from_xywh(r.x, r.y, r.w, r.h) {
            let mut p = paint(c);
            p.anti_alias = false;
            let t = self.transform();
            let mask = self.clips.last().and_then(|(_, m)| m.as_ref());
            self.pixmap.fill_rect(rect, &p, t, mask);
        }
    }

    /// Vertical gradient fill.
    pub fn fill_rect_gradient(&mut self, r: Rect, top: Color, bottom: Color, radius: f32) {
        let Some(path) = rrect_path(r, radius) else { return };
        let tc = |c: Color| tiny_skia::Color::from_rgba8(c.r, c.g, c.b, c.a);
        let Some(shader) = tiny_skia::LinearGradient::new(
            tiny_skia::Point::from_xy(0.0, r.y),
            tiny_skia::Point::from_xy(0.0, r.bottom()),
            vec![tiny_skia::GradientStop::new(0.0, tc(top)), tiny_skia::GradientStop::new(1.0, tc(bottom))],
            tiny_skia::SpreadMode::Pad,
            Transform::identity(),
        ) else {
            return;
        };
        let p = Paint { shader, anti_alias: true, ..Paint::default() };
        let t = self.transform();
        let mask = self.clips.last().and_then(|(_, m)| m.as_ref());
        self.pixmap.fill_path(&path, &p, FillRule::Winding, t, mask);
    }

    pub fn fill_rrect(&mut self, r: Rect, radius: f32, c: Color) {
        if let Some(path) = rrect_path(r, radius) {
            self.fill(&path, c);
        }
    }

    pub fn stroke_rrect(&mut self, r: Rect, radius: f32, width: f32, c: Color) {
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
            let mut s = Stroke { width, ..Stroke::default() };
            s.line_cap = LineCap::Round;
            let t = self.transform();
            let p = paint(c);
            let mask = self.clips.last().and_then(|(_, m)| m.as_ref());
            self.pixmap.stroke_path(&path, &p, &s, t, mask);
        }
    }

    pub fn fill(&mut self, path: &tiny_skia::Path, c: Color) {
        let t = self.transform();
        let p = paint(c);
        let mask = self.clips.last().and_then(|(_, m)| m.as_ref());
        self.pixmap.fill_path(path, &p, FillRule::Winding, t, mask);
    }

    pub fn stroke(&mut self, path: &tiny_skia::Path, width: f32, c: Color) {
        let t = self.transform();
        let p = paint(c);
        let s = Stroke { width, ..Stroke::default() };
        let mask = self.clips.last().and_then(|(_, m)| m.as_ref());
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
