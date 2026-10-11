//! OpenStudio painting: a CPU canvas with shapes (tiny-skia) and text (GDI ClearType on Windows).
//!
//! All coordinates are logical pixels; the canvas multiplies by `scale` (the window's DPI factor).
//! Text uses the bundled Cuprum face (see `text`).
// spec: ui/main-window.md §10 (typography)

mod glyph;
pub mod kit;
pub mod tabler;
mod text;

use std::collections::HashMap;

pub use os_theme::Color;
pub use glyph::Glyph;
pub use text::{Face, Font};
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

/// The frame being painted.
pub struct Canvas {
    pub pixmap: Pixmap,
    pub scale: f32,
    text: text::TextEngine,
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
            text: text::TextEngine::new()?,
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
        // Masks are canvas-sized; a stale one makes clipped fills fail silently.
        self.masks.clear();
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
        // An empty clip (e.g. a pushed clip entirely outside the damage rect) shows nothing.
        let (x0, y0, x1, y1) = self.clip_px();
        if x1 <= x0 || y1 <= y0 {
            return true;
        }
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

    /// One cell of a picture made of adjacent cells (meter and analyzer columns, waveform columns, heatmap cells):
    /// both edges rounded, so neighbours tile without gaps or overlaps at any scale. `fill_rect` keeps a thin
    /// rect's width instead, right for a lone line but at 150 % it leaves a gap after every other 1 px column
    /// (spec: ui/ui-scaling.md §4).
    pub fn fill_cell(&mut self, r: Rect, c: Color) {
        let k = self.scale;
        let span = |a: f32, len: f32| ((a * k).round() as i32, ((a + len) * k).round() as i32);
        self.fill_px(span(r.x, r.w), span(r.y, r.h), c, c);
    }

    fn fill_rect_vgrad(&mut self, r: Rect, top: Color, bottom: Color) {
        let sc = self.scale;
        self.fill_px(snap_span(r.x, r.w, sc), snap_span(r.y, r.h, sc), top, bottom);
    }

    /// Fill the device-pixel span `[rx0, rx1) x [ry0, ry1)` inside the clip, `top` to `bottom` down the span.
    fn fill_px(&mut self, (rx0, rx1): (i32, i32), (ry0, ry1): (i32, i32), top: Color, bottom: Color) {
        let (cx0, cy0, cx1, cy1) = self.clip_px();
        let (x0, x1) = (rx0.max(cx0), rx1.min(cx1));
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

    /// Elliptical radial gradient over `r`: `center` at the middle, `edge` where the normalized
    /// distance reaches 1 (the half-width / half-height), falling off with the square of the distance.
    pub fn fill_radial(&mut self, r: Rect, center: Color, edge: Color) {
        let k = self.scale;
        let (cx0, cy0, cx1, cy1) = self.clip_px();
        let x0 = ((r.x * k).round() as i32).max(cx0);
        let x1 = ((r.right() * k).round() as i32).min(cx1);
        let y0 = ((r.y * k).round() as i32).max(cy0);
        let y1 = ((r.bottom() * k).round() as i32).min(cy1);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let (mx, my) = (r.cx() * k, r.cy() * k);
        let (hw, hh) = ((r.w * k / 2.0).max(0.5), (r.h * k / 2.0).max(0.5));
        let pw = self.pixmap.width() as usize;
        let data = self.pixmap.data_mut();
        for y in y0..y1 {
            let ny = (y as f32 + 0.5 - my) / hh;
            for x in x0..x1 {
                let nx = (x as f32 + 0.5 - mx) / hw;
                let t = nx * nx + ny * ny;
                let i = (y as usize * pw + x as usize) * 4;
                let l = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round().clamp(0.0, 255.0) as u8;
                data[i] = l(center.r, edge.r);
                data[i + 1] = l(center.g, edge.g);
                data[i + 2] = l(center.b, edge.b);
                data[i + 3] = 255;
            }
        }
    }

    /// Set single device pixels (corner details); `a` = 255 replaces, less blends.
    pub fn pixel(&mut self, x: f32, y: f32, c: Color) {
        self.fill_rect(Rect::new(x, y, 1.0, 1.0), c);
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
    pub fn text_width(&mut self, f: Font, s: &str) -> f32 {
        self.text.extent(f, self.scale, s).0 as f32 / self.scale
    }

    /// Cell height (ascent + descent) of `f` in logical pixels.
    pub fn line_height(&mut self, f: Font) -> f32 {
        self.text.metrics(f, self.scale).0 as f32 / self.scale
    }

    /// Ascent (cell top to baseline) of `f` in logical pixels.
    pub fn ascent(&mut self, f: Font) -> f32 {
        self.text.metrics(f, self.scale).1 as f32 / self.scale
    }

    /// Draw `s` with its text cell's top-left at (x, y). Returns the advance width.
    pub fn text(&mut self, f: Font, x: f32, y: f32, c: Color, s: &str) -> f32 {
        let k = self.scale;
        let clip = self.clip_px();
        let (xi, yi) = ((x * k).round() as i32, (y * k).round() as i32);
        let (w, h) = self.text.extent(f, k, s);
        if xi > clip.2 || yi > clip.3 || xi + w + 8 < clip.0 || yi + h + 4 < clip.1 {
            return w as f32 / k;
        }
        self.text.draw(&mut self.pixmap, clip, f, k, xi, yi, c, s, false);
        w as f32 / k
    }

    /// Text centered in `r` (both axes, by the cell box).
    pub fn text_center(&mut self, f: Font, r: Rect, c: Color, s: &str) {
        let w = self.text_width(f, s);
        let h = self.line_height(f);
        self.text(f, (r.cx() - w / 2.0).round(), (r.cy() - h / 2.0).round(), c, s);
    }

    /// `f`, horizontally condensed step by step (then shrunk) until `s` fits in `max_w`.
    pub fn fit_font(&mut self, f: Font, s: &str, max_w: f32) -> Font {
        if self.text_width(f, s) <= max_w {
            return f;
        }
        let mut g = f;
        let natural = (self.text_width(f, s) / s.chars().count().max(1) as f32).round() as u16;
        let mut w = natural.max(2);
        while w > 2 {
            w -= 1;
            g = Font { width: w, natural: false, ..f };
            if self.text_width(g, s) <= max_w {
                return g;
            }
        }
        g
    }

    /// Text clipped to `max_w` (FL cuts long labels at the box edge).
    pub fn text_clip(&mut self, f: Font, x: f32, y: f32, max_w: f32, c: Color, s: &str) {
        let h = self.line_height(f);
        self.push_clip(Rect::new(x - 2.0, y - 4.0, max_w + 2.0, h + 8.0));
        self.text(f, x, y, c, s);
        self.pop_clip();
    }

    /// Text right-aligned to `right`.
    pub fn text_right(&mut self, f: Font, right: f32, y: f32, c: Color, s: &str) {
        let w = self.text_width(f, s);
        self.text(f, right - w, y, c, s);
    }

    /// Text rotated 90° counter-clockwise (reads bottom to top), starting at (x, y_bottom),
    /// with the glyph tops facing left at `x`. Returns the advance (upward extent).
    pub fn text_vertical(&mut self, f: Font, x: f32, y_bottom: f32, c: Color, s: &str) -> f32 {
        let k = self.scale;
        let clip = self.clip_px();
        let (xi, yi) = ((x * k).round() as i32, (y_bottom * k).round() as i32);
        let (w, h) = self.text.extent(f, k, s);
        if xi > clip.2 || xi + h + 4 < clip.0 || yi + 4 < clip.1 || yi - w - 4 > clip.3 {
            return w as f32 / k;
        }
        self.text.draw(&mut self.pixmap, clip, f, k, xi, yi, c, s, true);
        w as f32 / k
    }

    /// Draw a Tabler icon (24-unit grid) centred in `r`, scaled to the shorter side, in color `c`.
    pub fn icon(&mut self, icon: &tabler::TablerIcon, r: Rect, c: Color) {
        let k = r.w.min(r.h) / 24.0;
        let (ox, oy) = (r.cx() - 12.0 * k, r.cy() - 12.0 * k);
        let mut g = Glyph::new();
        g.round_caps = true;
        let stroke = if icon.filled { None } else { Some((2.0 * k).max(1.1)) };
        g.commands(icon.d, ox, oy, k, stroke);
        self.glyph(c, &g);
    }

    /// An outline Tabler icon like `icon`, stroked `width` logical px wide whatever its size (small, bold glyphs).
    pub fn icon_stroke(&mut self, icon: &tabler::TablerIcon, r: Rect, c: Color, width: f32) {
        let k = r.w.min(r.h) / 24.0;
        let (ox, oy) = (r.cx() - 12.0 * k, r.cy() - 12.0 * k);
        let mut g = Glyph::new();
        g.round_caps = true;
        g.commands(icon.d, ox, oy, k, (!icon.filled).then_some(width));
        self.glyph(c, &g);
    }

    /// Draw an icon glyph with subpixel (ClearType-style) coverage in color `c`.
    pub fn glyph(&mut self, c: Color, g: &Glyph) {
        let Some(b) = g.bounds() else { return };
        let k = self.scale;
        let clip = self.clip_px();
        // Rasterize the whole glyph (the subpixel filter needs neighbours), blend inside the clip.
        let x0 = (b.x * k).floor() as i32 - 1;
        let y0 = (b.y * k).floor() as i32;
        let x1 = (b.right() * k).ceil() as i32 + 1;
        let y1 = (b.bottom() * k).ceil() as i32;
        if x0.max(clip.0) >= x1.min(clip.2) || y0.max(clip.1) >= y1.min(clip.3) {
            return;
        }
        let (w, h) = (x1 - x0, y1 - y0);
        let Some(cov) = glyph::coverage(g, k, x0, y0, w, h) else { return };
        let pw = self.pixmap.width() as usize;
        let data = self.pixmap.data_mut();
        let a = u32::from(c.a);
        for y in 0..h as usize {
            for x in 0..w as usize {
                let (dx, dy) = (x0 + x as i32, y0 + y as i32);
                if dx < clip.0 || dx >= clip.2 || dy < clip.1 || dy >= clip.3 {
                    continue;
                }
                let cv = cov[y * w as usize + x];
                if cv == [0, 0, 0] {
                    continue;
                }
                let i = (dy as usize * pw + dx as usize) * 4; // dx, dy are inside the clip (>= 0)
                for (ch, fg) in [c.r, c.g, c.b].into_iter().enumerate() {
                    let al = u32::from(cv[ch]) * a / 255;
                    let d = u32::from(data[i + ch]);
                    data[i + ch] = ((u32::from(fg) * al + d * (255 - al)) / 255) as u8;
                }
            }
        }
    }

    /// Decodes a PNG into `(w, h, 0x00RRGGBB pixels)` for [`Canvas::image`] (alpha is dropped).
    pub fn decode_png(bytes: &[u8]) -> Option<(usize, usize, Vec<u32>)> {
        let pm = tiny_skia::Pixmap::decode_png(bytes).ok()?;
        let px = pm.pixels().iter().map(|p| {
            let c = p.demultiply();
            u32::from(c.red()) << 16 | u32::from(c.green()) << 8 | u32::from(c.blue())
        });
        Some((pm.width() as usize, pm.height() as usize, px.collect()))
    }

    /// Draws a `w` x `h` image of `0x00RRGGBB` pixels stretched over `r` (clipped). Filtered, so a picture stays smooth
    /// at any display scale: each device pixel averages the source pixels it covers when shrinking, and blends the
    /// nearest four when enlarging. Pixel for pixel, the image is copied as it is. Spec: ui/ui-scaling.md §5.
    pub fn image(&mut self, r: Rect, w: usize, h: usize, px: &[u32]) {
        if w == 0 || h == 0 || px.len() < w * h {
            return;
        }
        let k = self.scale;
        let (cx0, cy0, cx1, cy1) = self.clip_px();
        let (x0, x1) = (((r.x * k).round() as i32).max(cx0), ((r.right() * k).round() as i32).min(cx1));
        let (y0, y1) = (((r.y * k).round() as i32).max(cy0), ((r.bottom() * k).round() as i32).min(cy1));
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let (rw, rh) = ((r.w * k).max(1.0), (r.h * k).max(1.0));
        // Source pixels per device pixel, and the source span (start, weights) each device column / row reads.
        let (fx, fy) = (w as f32 / rw, h as f32 / rh);
        let xs: Vec<Vec<(usize, f32)>> = (x0..x1).map(|x| taps(x as f32 + 0.5 - r.x * k, fx, w)).collect();
        let ys: Vec<Vec<(usize, f32)>> = (y0..y1).map(|y| taps(y as f32 + 0.5 - r.y * k, fy, h)).collect();
        let pw = self.pixmap.width() as usize;
        let pixels = self.pixmap.pixels_mut();
        for (y, ty) in (y0..y1).zip(&ys) {
            for (x, tx) in (x0..x1).zip(&xs) {
                let mut acc = [0.0f32; 3];
                for &(sy, wy) in ty {
                    for &(sx, wx) in tx {
                        let v = px[sy * w + sx];
                        let wgt = wx * wy;
                        acc[0] += ((v >> 16) & 0xff) as f32 * wgt;
                        acc[1] += ((v >> 8) & 0xff) as f32 * wgt;
                        acc[2] += (v & 0xff) as f32 * wgt;
                    }
                }
                let ch = |a: f32| a.round().clamp(0.0, 255.0) as u8;
                pixels[y as usize * pw + x as usize] = PremultipliedColorU8::from_rgba(ch(acc[0]), ch(acc[1]), ch(acc[2]), 255).expect("opaque");
            }
        }
    }

    /// Copy a device-pixel region into a `0x00RRGGBB` buffer of the same width.
    pub fn copy_region_to_xrgb(&self, out: &mut [u32], r: (usize, usize, usize, usize)) {
        self.copy_region_to_xrgb_at(out, self.pixmap.width() as usize, r, (0, 0));
    }

    /// Copy a device-pixel region into a larger `0x00RRGGBB` buffer (row length `out_stride`),
    /// with the canvas placed at `(ox, oy)` in it (the main window's border surrounds the canvas).
    pub fn copy_region_to_xrgb_at(&self, out: &mut [u32], out_stride: usize, (x, y, w, h): (usize, usize, usize, usize), (ox, oy): (usize, usize)) {
        let stride = self.pixmap.width() as usize;
        let px = self.pixmap.pixels();
        for row in y..y + h {
            let src = &px[row * stride + x..row * stride + x + w];
            let d = (row + oy) * out_stride + x + ox;
            let Some(dst) = out.get_mut(d..d + w) else { return };
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

/// Device span `[p0, p1)` of the logical span `[a, a + len)` at `scale`: both edges rounded, except that at a
/// fractional scale a thin span (a line or border, up to 2 logical px) keeps one width wherever it sits. Rounding
/// both edges of a 1 px line at 150 % gives 1 or 2 px depending on its position, so a grid's lines would alternate
/// thin and thick; this gives 1 px everywhere (spec: ui/ui-scaling.md §4).
pub fn snap_span(a: f32, len: f32, scale: f32) -> (i32, i32) {
    let p0 = (a * scale).round() as i32;
    if len > 0.0 && len <= 2.0 && scale.fract() != 0.0 {
        return (p0, p0 + ((len * scale + 0.25).floor() as i32).max(1));
    }
    (p0, ((a + len) * scale).round() as i32)
}

/// Source pixels (index, weight; weights sum to 1) that the device pixel centred at `d` (device px from the image's
/// left / top edge) reads from an `n`-pixel source when one device pixel spans `f` source pixels.
fn taps(d: f32, f: f32, n: usize) -> Vec<(usize, f32)> {
    let last = n as f32;
    if f > 1.0 {
        // Shrinking: average the covered span [s0, s1), partial pixels by their coverage.
        let (s0, s1) = (((d - 0.5) * f).clamp(0.0, last), ((d + 0.5) * f).clamp(0.0, last));
        let mut out = Vec::with_capacity(f.ceil() as usize + 1);
        let mut i = s0.floor() as usize;
        while (i as f32) < s1 && i < n {
            let cover = (s1.min(i as f32 + 1.0) - s0.max(i as f32)).max(0.0);
            if cover > 0.0 {
                out.push((i, cover / (s1 - s0)));
            }
            i += 1;
        }
        if out.is_empty() {
            out.push(((s0 as usize).min(n - 1), 1.0));
        }
        out
    } else {
        // Enlarging (or 1:1): linear between the two nearest source centres.
        let s = (d * f - 0.5).clamp(0.0, last - 1.0);
        let i = s.floor() as usize;
        let t = s - i as f32;
        if t <= 1e-4 || i + 1 >= n {
            vec![(i.min(n - 1), 1.0)]
        } else {
            vec![(i, 1.0 - t), (i + 1, t)]
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
    fn thin_rects_keep_their_width_at_fractional_scales() {
        // 150 %: a 1 px line is 1 device px wherever it sits, 2 px is 3; wide spans still meet edge to edge.
        for i in 0..8 {
            let x = i as f32 * 7.0 + 0.5 * (i % 2) as f32;
            let (a, b) = snap_span(x, 1.0, 1.5);
            assert_eq!(b - a, 1, "1 px at {x}");
            let (a, b) = snap_span(x, 2.0, 1.5);
            assert_eq!(b - a, 3, "2 px at {x}");
        }
        assert_eq!(snap_span(10.0, 1.0, 1.75), (18, 20));
        assert_eq!(snap_span(0.0, 10.0, 1.5).1, snap_span(10.0, 10.0, 1.5).0);
        // Integer scales are unchanged: both edges rounded.
        assert_eq!(snap_span(3.0, 1.0, 2.0), (6, 8));
        assert_eq!(snap_span(0.5, 1.0, 1.0), (1, 2));
    }

    #[test]
    fn cells_tile_at_fractional_scales() {
        // 1 px columns side by side (a meter) cover every device pixel once at 150 %; thin rects would leave gaps.
        for (scale, w) in [(1.5, 30), (1.25, 25), (1.75, 35), (2.0, 40)] {
            let Ok(mut c) = Canvas::new(w, 2, scale) else { return };
            c.fill_rect(Rect::new(0.0, 0.0, 20.0, 2.0), Color::rgb(0x000000));
            for x in 0..20 {
                c.fill_cell(Rect::new(x as f32, 0.0, 1.0, 1.0), Color::rgb(0x808080).with_alpha(128));
            }
            let row: Vec<u8> = c.pixmap.pixels()[..w as usize].iter().map(|p| p.red()).collect();
            assert!(row.iter().all(|&r| r == 64), "cells at {scale}: {row:?}");
        }
    }

    #[test]
    fn images_are_filtered_and_one_to_one_is_exact() {
        let px: Vec<u32> = (0..16).map(|i| if i % 2 == 0 { 0x000000 } else { 0xffffff }).collect();
        let Ok(mut c) = Canvas::new(16, 1, 1.0) else { return };
        c.image(Rect::new(0.0, 0.0, 16.0, 1.0), 16, 1, &px);
        let got: Vec<u8> = c.pixmap.pixels().iter().map(|p| p.red()).collect();
        assert_eq!(got, (0..16).map(|i| if i % 2 == 0 { 0 } else { 255 }).collect::<Vec<_>>(), "1:1 copies");
        // Half size: each device pixel averages a black and a white source pixel.
        let Ok(mut c) = Canvas::new(8, 1, 0.5) else { return };
        c.image(Rect::new(0.0, 0.0, 16.0, 2.0), 16, 1, &px);
        assert!(c.pixmap.pixels().iter().all(|p| (127..=128).contains(&p.red())), "box average when shrinking");
        // 150 %: enlarged pixels blend, no uneven doubling.
        let Ok(mut c) = Canvas::new(24, 1, 1.5) else { return };
        c.image(Rect::new(0.0, 0.0, 16.0, 1.0), 16, 1, &px);
        assert!(c.pixmap.pixels().iter().any(|p| p.red() > 20 && p.red() < 235), "linear when enlarging");
    }

    #[test]
    fn paints_text_and_shapes() {
        let Ok(mut c) = Canvas::new(200, 50, 1.0) else { return }; // no system font: skip
        c.clear(Color::rgb(0x000000));
        c.fill_rrect(Rect::new(5.0, 5.0, 20.0, 20.0), 3.0, Color::rgb(0xff0000));
        let w = c.text(Font::new(14), 40.0, 10.0, Color::rgb(0xffffff), "OpenStudio");
        assert!(w > 40.0);
        assert!(c.pixmap.data().chunks_exact(4).any(|p| p[0] > 200 && p[1] > 200));
    }
}
