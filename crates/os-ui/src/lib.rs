// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Studio painting: a CPU canvas with shapes (tiny-skia) and text (GDI ClearType on Windows).
//!
//! All coordinates are logical pixels; the canvas multiplies by `scale` (the window's DPI factor).
//! Text uses the bundled Cuprum face (see `text`).
// spec: ui/main-window.md §10 (typography)

mod cache;
mod glyph;
pub mod kit;
pub mod tabler;
mod text;

use std::hash::Hasher;
use std::rc::Rc;

use cache::{Fx, Lru};
pub use os_theme::Color;
pub use glyph::{path_data, Glyph};
pub use text::{font_count, Face, Font};
use tiny_skia::{FillRule, LineCap, Paint, PathBuilder, Pixmap, PremultipliedColorU8, Stroke, Transform};

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
    /// The frame's damage region as disjoint device rects (x0, y0, x1, y1); empty = the whole canvas.
    region: Vec<(i32, i32, i32, i32)>,
    /// Device rect that will be painted over opaquely later: rect fills skip it (see `set_cover`).
    cover: Option<(i32, i32, i32, i32)>,
    /// Anti-aliased coverage of filled / stroked paths, by path (relative to its pixel origin) and scale: most frames
    /// repaint the same shapes, and a scrolled view the same shapes moved by whole pixels.
    shapes: Lru<Rc<Coverage>>,
    /// Subpixel coverage of icon glyphs, by glyph (or icon and box) and scale.
    glyphs: Lru<Rc<GlyphCoverage>>,
    /// Device bounds of everything drawn since the last `take_drawn` (x0, y0, x1, y1).
    drawn: Option<(i32, i32, i32, i32)>,
}

/// The scale and clips `Canvas::rescale` replaced, for `Canvas::unscale`.
pub struct Rescaled {
    scale: f32,
    clips: Vec<Rect>,
}

/// Coverage (0..=255) of a shape, `w` x `h` device pixels.
struct Coverage {
    w: i32,
    h: i32,
    a: Vec<u8>,
}

/// A shape's coverage placed with its top-left at device pixel (x0, y0).
struct Placed {
    x0: i32,
    y0: i32,
    cov: Rc<Coverage>,
}

/// Per-channel subpixel coverage of a glyph over the device rect at (x0, y0), `w` x `h`.
struct GlyphCoverage {
    x0: i32,
    y0: i32,
    w: i32,
    h: i32,
    cov: Vec<[u8; 3]>,
}

/// Shapes larger than this (device pixels) are rasterized for the visible part only and not cached.
const BIG_SHAPE: i64 = 1 << 20;

fn hash_path(h: &mut Fx, p: &tiny_skia::Path) {
    h.write_usize(p.verbs().len());
    for v in p.verbs() {
        h.write_u8(*v as u8);
    }
    for pt in p.points() {
        h.f32(pt.x);
        h.f32(pt.y);
    }
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
            region: Vec::new(),
            cover: None,
            shapes: Lru::new(32 << 20),
            glyphs: Lru::new(16 << 20),
            drawn: None,
        })
    }

    /// Resize (contents are cleared).
    pub fn resize(&mut self, width: u32, height: u32, scale: f32) {
        if let Some(p) = Pixmap::new(width.max(1), height.max(1)) {
            self.pixmap = p;
        }
        if scale != self.scale {
            // Cached coverage is per scale: entries for the old one would only wait to be evicted.
            self.shapes.clear();
            self.glyphs.clear();
        }
        self.scale = scale;
        self.clips.clear();
        self.region.clear();
    }

    /// Paints at `scale` from here until `unscale`: a part of the frame scaled apart from the rest (popup menus, spec:
    /// ui/ui-scaling.md §1). Logical px become `scale` device px; the clips keep their device bounds.
    pub fn rescale(&mut self, scale: f32) -> Rescaled {
        let old = Rescaled { scale: self.scale, clips: self.clips.clone() };
        let k = self.scale / scale;
        for c in &mut self.clips {
            *c = Rect::new(c.x * k, c.y * k, c.w * k, c.h * k);
        }
        self.scale = scale;
        old
    }

    /// Back to the scale and clips `rescale` replaced.
    pub fn unscale(&mut self, old: Rescaled) {
        self.scale = old.scale;
        self.clips = old.clips;
    }

    /// Logical size.
    pub fn size(&self) -> (f32, f32) {
        (self.pixmap.width() as f32 / self.scale, self.pixmap.height() as f32 / self.scale)
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

    /// Device-pixel bounds of the innermost clip: (x0, y0, x1, y1), end-exclusive.
    fn top_px(&self) -> (i32, i32, i32, i32) {
        let c = self.clip_rect();
        let s = self.scale;
        (
            ((c.x * s).round() as i32).max(0),
            ((c.y * s).round() as i32).max(0),
            ((c.right() * s).round() as i32).min(self.pixmap.width() as i32),
            ((c.bottom() * s).round() as i32).min(self.pixmap.height() as i32),
        )
    }

    /// Number of device rects drawing goes to (`clip_at`): one per rect of the frame's damage region.
    fn clip_count(&self) -> usize {
        self.region.len().max(1)
    }

    /// Device rect `i` drawing goes to: the innermost clip cut by damage rect `i` (may be empty).
    fn clip_at(&self, i: usize) -> (i32, i32, i32, i32) {
        let t = self.top_px();
        match self.region.get(i) {
            Some(&(x0, y0, x1, y1)) => (t.0.max(x0), t.1.max(y0), t.2.min(x1), t.3.min(y1)),
            None => t,
        }
    }

    /// Bounding box of every rect drawing goes to (x0 >= x1 when there is none).
    fn clip_bounds(&self) -> (i32, i32, i32, i32) {
        (0..self.clip_count()).map(|i| self.clip_at(i)).filter(|c| c.0 < c.2 && c.1 < c.3).fold(
            (i32::MAX, i32::MAX, i32::MIN, i32::MIN),
            |b, c| (b.0.min(c.0), b.1.min(c.1), b.2.max(c.2), b.3.max(c.3)),
        )
    }

    /// Whether the device rect `[x0, x1) x [y0, y1)` reaches any rect drawing goes to.
    fn reaches(&self, x0: i32, y0: i32, x1: i32, y1: i32) -> bool {
        (0..self.clip_count()).any(|i| {
            let c = self.clip_at(i);
            c.0 < c.2 && c.1 < c.3 && x0 < c.2 && x1 > c.0 && y0 < c.3 && y1 > c.1
        })
    }

    /// Anti-aliased coverage of `path` at the canvas scale, filled (`stroke` None) or stroked, rasterized by
    /// tiny-skia into a shape-sized buffer and cached by the path, so a repaint of the same shape only blends.
    fn coverage(&mut self, path: &tiny_skia::Path, stroke: Option<&Stroke>) -> Option<Placed> {
        let k = self.scale;
        // Device bounds, padded for anti-aliasing and the stroke (miter joins reach `miter_limit` half-widths out).
        let b = path.bounds();
        let pad = stroke.map_or(0.0, |s| s.width / 2.0 * s.miter_limit.max(1.5));
        let x0 = ((b.left() - pad) * k).floor() as i32 - 1;
        let y0 = ((b.top() - pad) * k).floor() as i32 - 1;
        let x1 = ((b.right() + pad) * k).ceil() as i32 + 1;
        let y1 = ((b.bottom() + pad) * k).ceil() as i32 + 1;
        let big = i64::from(x1 - x0) * i64::from(y1 - y0) > BIG_SHAPE;
        let (x0, y0, x1, y1) = if big {
            let (cx0, cy0, cx1, cy1) = self.clip_bounds();
            (x0.max(cx0), y0.max(cy0), x1.min(cx1), y1.min(cy1))
        } else {
            (x0, y0, x1, y1)
        };
        if x0 >= x1 || y0 >= y1 {
            return None;
        }
        let (w, h) = (x1 - x0, y1 - y0);
        // The shape is drawn from its points relative to its pixel origin, snapped to 1/64 px, and cached by those: the
        // same shape moved by whole pixels (a scrolled view) reuses its coverage, and the pixels depend only on the
        // key, so every repaint draws a shape the same way. Shapes too big to cache are drawn for the visible part.
        let q = |v: f32, o: i32| ((v * k - o as f32) * 64.0).round() as i32;
        let pts: Vec<(i32, i32)> = path.points().iter().map(|pt| (q(pt.x, x0), q(pt.y, y0))).collect();
        let key = (!big).then(|| {
            let mut hs = Fx::default();
            hs.f32(k);
            match stroke {
                None => hs.write_u8(1),
                Some(s) => {
                    hs.write_u8(2);
                    hs.f32(s.width);
                    hs.f32(s.miter_limit);
                    hs.write_u8(s.line_cap as u8);
                    hs.write_u8(s.line_join as u8);
                }
            }
            hs.write_i32(w);
            hs.write_i32(h);
            hs.write_usize(path.verbs().len());
            for v in path.verbs() {
                hs.write_u8(*v as u8);
            }
            for &(x, y) in &pts {
                hs.write_i32(x);
                hs.write_i32(y);
            }
            hs.finish()
        });
        if let Some(c) = key.and_then(|key| self.shapes.get(key)) {
            return Some(Placed { x0, y0, cov: c.clone() });
        }
        let snapped = rebuild_path(path, &pts)?;
        let mut pm = Pixmap::new(w as u32, h as u32)?;
        let p = paint(Color::rgb(0xffffff));
        match stroke {
            None => pm.fill_path(&snapped, &p, FillRule::Winding, Transform::identity(), None),
            Some(s) => {
                // In device pixels: the width scales with the canvas (hairlines below one device pixel as before).
                let s = Stroke { width: s.width * k, ..s.clone() };
                pm.stroke_path(&snapped, &p, &s, Transform::identity(), None)
            }
        }
        let a: Vec<u8> = pm.pixels().iter().map(|px| px.alpha()).collect();
        let c = Rc::new(Coverage { w, h, a });
        if let Some(key) = key {
            self.shapes.insert(key, c.clone(), c.a.len() + 64);
        }
        Some(Placed { x0, y0, cov: c })
    }

    /// Blend `top` through `cov` inside the clip; with `grad` = (bottom, y_top, y_bottom) in device pixels the
    /// colour runs from `top` to `bottom` down that span (a vertical linear gradient sampled at pixel centres).
    fn blend(&mut self, cov: &Placed, top: Color, grad: Option<(Color, f32, f32)>) {
        for i in 0..self.clip_count() {
            let clip = self.clip_at(i);
            self.blend_in(clip, cov, top, grad);
        }
    }

    fn blend_in(&mut self, (cx0, cy0, cx1, cy1): (i32, i32, i32, i32), p: &Placed, top: Color, grad: Option<(Color, f32, f32)>) {
        let cov = &*p.cov;
        let (x0, x1) = (p.x0.max(cx0), (p.x0 + cov.w).min(cx1));
        let (y0, y1) = (p.y0.max(cy0), (p.y0 + cov.h).min(cy1));
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        self.mark(x0, y0, x1, y1);
        let pw = self.pixmap.width() as usize;
        let n = (x1 - x0) as usize;
        let data = self.pixmap.data_mut();
        for y in y0..y1 {
            let c = match grad {
                Some((bottom, g0, g1)) => top.mix(bottom, ((y as f32 + 0.5 - g0) / (g1 - g0).max(1e-3)).clamp(0.0, 1.0)),
                None => top,
            };
            if c.a == 0 {
                continue;
            }
            let src = &cov.a[((y - p.y0) * cov.w + (x0 - p.x0)) as usize..][..n];
            let dst = &mut data[(y as usize * pw + x0 as usize) * 4..][..n * 4];
            blend_cov_row(dst.as_chunks_mut::<4>().0, src, c);
        }
    }

    /// Blend through `cov` with a colour per device-pixel column: column `x` (device) takes `cols[x - cov.x0]`.
    fn blend_columns(&mut self, p: &Placed, cols: &[Color]) {
        for i in 0..self.clip_count() {
            let clip = self.clip_at(i);
            self.blend_columns_in(clip, p, cols);
        }
    }

    fn blend_columns_in(&mut self, (cx0, cy0, cx1, cy1): (i32, i32, i32, i32), p: &Placed, cols: &[Color]) {
        let cov = &*p.cov;
        let (x0, x1) = (p.x0.max(cx0), (p.x0 + cov.w).min(cx1));
        let (y0, y1) = (p.y0.max(cy0), (p.y0 + cov.h).min(cy1));
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        self.mark(x0, y0, x1, y1);
        let pw = self.pixmap.width() as usize;
        let n = (x1 - x0) as usize;
        let cols = &cols[(x0 - p.x0) as usize..][..n];
        let data = self.pixmap.data_mut();
        for y in y0..y1 {
            let src = &cov.a[((y - p.y0) * cov.w + (x0 - p.x0)) as usize..][..n];
            let dst = &mut data[(y as usize * pw + x0 as usize) * 4..][..n * 4];
            let dst = dst.as_chunks_mut::<4>().0;
            // Runs of one colour blend together.
            let mut i = 0;
            while i < n {
                let c = cols[i];
                let j = (i + 1..n).find(|&j| cols[j] != c).unwrap_or(n);
                if c.a != 0 {
                    blend_cov_row(&mut dst[i..j], &src[i..j], c);
                }
                i = j;
            }
        }
    }

    /// Fill (`stroke` None) or stroke `path` with a colour per device-pixel column, `color` called once per column
    /// with the logical x of its centre. The shape's pixels are the ones `fill` / `stroke` draw.
    fn columns(&mut self, path: &tiny_skia::Path, stroke: Option<&Stroke>, color: &mut dyn FnMut(f32) -> Color) {
        let pad = stroke.map_or(1.0, |s| s.width + 1.0);
        if self.outside(path.bounds(), pad) {
            return;
        }
        if let Some(cov) = self.coverage(path, stroke) {
            let k = self.scale;
            let cols: Vec<Color> = (0..cov.cov.w).map(|i| color((cov.x0 + i) as f32 / k + 0.5 / k)).collect();
            self.blend_columns(&cov, &cols);
        }
    }

    /// Record that device pixels `[x0, x1) x [y0, y1)` were drawn.
    fn mark(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        self.drawn = Some(match self.drawn {
            Some((a, b, c, d)) => (a.min(x0), b.min(y0), c.max(x1), d.max(y1)),
            None => (x0, y0, x1, y1),
        });
    }

    /// Logical bounds of everything drawn since the last call (None = nothing), then start over. The app uses it to
    /// learn where each view paints, so a repaint that misses a view can skip it.
    pub fn take_drawn(&mut self) -> Option<Rect> {
        let (x0, y0, x1, y1) = self.drawn.take()?;
        let k = self.scale;
        Some(Rect::new(x0 as f32 / k, y0 as f32 / k, (x1 - x0) as f32 / k, (y1 - y0) as f32 / k))
    }

    /// Start learning where a view inside another one draws: hands back what was drawn so far, for `end_drawn`.
    pub fn begin_drawn(&mut self) -> Option<(i32, i32, i32, i32)> {
        self.drawn.take()
    }

    /// End a view started with `begin_drawn`: the logical bounds it drew (as `take_drawn`). The bounds from before
    /// it come back, grown by these, so the view around it still learns what it drew.
    pub fn end_drawn(&mut self, before: Option<(i32, i32, i32, i32)>) -> Option<Rect> {
        let mine = std::mem::replace(&mut self.drawn, before);
        let (x0, y0, x1, y1) = mine?;
        self.mark(x0, y0, x1, y1);
        let k = self.scale;
        Some(Rect::new(x0 as f32 / k, y0 as f32 / k, (x1 - x0) as f32 / k, (y1 - y0) as f32 / k))
    }

    /// Whether anything drawn inside `r` (logical) would reach the current clip: views skip invisible work with it.
    pub fn visible(&self, r: Rect) -> bool {
        let k = self.scale;
        let (x0, y0) = ((r.x * k).floor() as i32, (r.y * k).floor() as i32);
        let (x1, y1) = ((r.right() * k).ceil() as i32, (r.bottom() * k).ceil() as i32);
        self.reaches(x0, y0, x1, y1)
    }

    /// Start a frame. With `damage`, everything is clipped to that rectangle (the rest of the
    /// canvas keeps last frame's pixels) and primitives outside it are skipped outright.
    pub fn begin_frame(&mut self, damage: Option<Rect>) {
        match damage {
            Some(d) => self.begin_region(&[d]),
            None => {
                self.clips.clear();
                self.region.clear();
                self.cover = None;
            }
        }
    }

    /// Start a frame clipped to several rectangles at once (one paint for damage in separate places: a playhead,
    /// the clock and some meters). The rects become disjoint, so no pixel is drawn twice: overlapping rects merge
    /// when their bounding box costs little more, else the newer one is cut around the other (rects meeting at a
    /// fractional edge share a device pixel column; merging a scroll bar and a grid strip that way would repaint
    /// the whole grid).
    pub fn begin_region(&mut self, damage: &[Rect]) {
        self.clips.clear();
        self.region.clear();
        self.cover = None;
        type R = (i32, i32, i32, i32);
        let area = |r: &R| i64::from(r.2 - r.0) * i64::from(r.3 - r.1);
        let slack = (2048.0 * self.scale * self.scale) as i64;
        let mut todo: Vec<R> = damage
            .iter()
            .rev()
            .map(|d| self.device_rect(*d))
            .filter(|&(_, _, w, h)| w > 0 && h > 0)
            .map(|(x, y, w, h)| (x as i32, y as i32, (x + w) as i32, (y + h) as i32))
            .collect();
        let mut rs: Vec<R> = Vec::with_capacity(todo.len());
        while let Some(r) = todo.pop() {
            let Some(i) = rs.iter().position(|o| o.0 < r.2 && o.2 > r.0 && o.1 < r.3 && o.3 > r.1) else {
                rs.push(r);
                continue;
            };
            let o = rs[i];
            let u = (r.0.min(o.0), r.1.min(o.1), r.2.max(o.2), r.3.max(o.3));
            if area(&u) <= area(&o) + area(&r) + slack {
                rs.swap_remove(i);
                todo.push(u);
                continue;
            }
            // The parts of `r` outside `o`: the full-width bands above and below it, then the sides.
            let (y0, y1) = (r.1.max(o.1), r.3.min(o.3));
            let parts = [(r.0, r.1, r.2, o.1), (r.0, o.3, r.2, r.3), (r.0, y0, o.0, y1), (o.2, y0, r.2, y1)];
            todo.extend(parts.into_iter().filter(|p| p.0 < p.2 && p.1 < p.3));
        }
        // Nothing to paint: one empty rect (an empty region means the whole canvas).
        self.region = if rs.is_empty() { vec![(0, 0, 0, 0)] } else { rs };
    }

    /// The frame's damage region (`begin_region`) in logical pixels: disjoint rects; empty = the whole canvas.
    pub fn region(&self) -> Vec<Rect> {
        let k = self.scale;
        self.region.iter().filter(|r| r.0 < r.2 && r.1 < r.3).map(|r| Rect::new(r.0 as f32 / k, r.1 as f32 / k, (r.2 - r.0) as f32 / k, (r.3 - r.1) as f32 / k)).collect()
    }

    /// Promise that `r` will be painted over opaquely before the frame ends; until `set_cover(None)`, rect fills
    /// leave it alone (a window background under the grid that covers it, the workspace under a maximized window).
    /// Saves filling those pixels twice; the caller must really cover all of `r`.
    pub fn set_cover(&mut self, r: Option<Rect>) {
        self.cover = r.map(|r| {
            // The same rounding as a clip to `r`, which the covering paint is drawn through.
            let k = self.scale;
            ((r.x * k).round() as i32, (r.y * k).round() as i32, (r.right() * k).round() as i32, (r.bottom() * k).round() as i32)
        });
    }

    /// Move the pixels inside `r` by `dx`, `dy` whole device pixels (a scrolled view keeps what it already drew).
    /// `r` is rounded like a clip to it ([`shift_area`]); pixels moved past it are dropped, and the strip they leave
    /// behind keeps its old pixels for the caller to repaint. The device pixels wholly inside a `keep` rect
    /// ([`keep_area`]: a window drawn over the view) stay where they are.
    pub fn shift(&mut self, r: Rect, dx: i32, dy: i32, keep: &[Rect]) {
        let size = (self.pixmap.width(), self.pixmap.height());
        let area = shift_area(r, self.scale, size);
        let keep: Vec<_> = keep.iter().map(|k| keep_area(*k, self.scale, size)).collect();
        shift_pixels(self.pixmap.pixels_mut(), size.0 as usize, area, dx, dy, &keep);
    }

    /// Logical bounds of what drawing can reach now (the clip cut by the frame's region), or None: views cull work
    /// outside it (a waveform built for a strip that scrolled in, not for the whole clip).
    pub fn paint_bounds(&self) -> Option<Rect> {
        let (x0, y0, x1, y1) = self.clip_bounds();
        let k = self.scale;
        (x0 < x1 && y0 < y1).then(|| Rect::new(x0 as f32 / k, y0 as f32 / k, (x1 - x0) as f32 / k, (y1 - y0) as f32 / k))
    }

    pub fn clear(&mut self, c: Color) {
        let (w, h) = self.size();
        self.fill_rect(Rect::new(0.0, 0.0, w, h), c);
    }

    /// True when `b` (logical, padded by `pad`) lies entirely outside the current clip.
    fn outside(&self, b: tiny_skia::Rect, pad: f32) -> bool {
        !self.visible(Rect::new(b.left() - pad, b.top() - pad, b.width() + 2.0 * pad, b.height() + 2.0 * pad))
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
    fn fill_px(&mut self, rx: (i32, i32), ry: (i32, i32), top: Color, bottom: Color) {
        for i in 0..self.clip_count() {
            let clip = self.clip_at(i);
            self.fill_px_in(clip, rx, ry, top, bottom);
        }
    }

    fn fill_px_in(&mut self, (cx0, cy0, cx1, cy1): (i32, i32, i32, i32), (rx0, rx1): (i32, i32), (ry0, ry1): (i32, i32), top: Color, bottom: Color) {
        let (x0, x1) = (rx0.max(cx0), rx1.min(cx1));
        let (y0, y1) = (ry0.max(cy0), ry1.min(cy1));
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        if let Some((vx0, vy0, vx1, vy1)) = self.cover
            && vx0 < x1
            && vx1 > x0
            && vy0 < y1
            && vy1 > y0
        {
            // Fill only around the covered rect: the bands above and below, then left and right of it.
            let cover = self.cover.take();
            let (mx0, mx1) = (vx0.max(x0), vx1.min(x1));
            let (my0, my1) = (vy0.max(y0), vy1.min(y1));
            for (bx0, by0, bx1, by1) in [(x0, y0, x1, my0), (x0, my1, x1, y1), (x0, my0, mx0, my1), (mx1, my0, x1, my1)] {
                if bx0 < bx1 && by0 < by1 {
                    self.fill_px_in((bx0, by0, bx1, by1), (rx0, rx1), (ry0, ry1), top, bottom);
                }
            }
            self.cover = cover;
            return;
        }
        self.mark(x0, y0, x1, y1);
        let pw = self.pixmap.width() as usize;
        let span = (ry1 - ry0).max(1) as f32;
        let flat = top == bottom;
        let (x0, x1) = (x0 as usize, x1 as usize);
        let data = self.pixmap.data_mut();
        // SAFETY: a u32 view of whole RGBA pixels; `align_to_mut` hands back any unaligned head, and the pixmap's
        // allocation is aligned in practice (the byte path below covers it otherwise).
        let (head, px, _) = unsafe { data.align_to_mut::<u32>() };
        if !head.is_empty() {
            return Self::fill_bytes(data, pw, (x0, x1), (y0, y1), ry0, span, top, bottom);
        }
        for y in y0..y1 {
            let c = if flat { top } else { top.mix(bottom, (y - ry0) as f32 / span) };
            let row = &mut px[y as usize * pw + x0..y as usize * pw + x1];
            if c.a == 255 {
                let v = u32::from_ne_bytes([c.r, c.g, c.b, 255]);
                if row.len() <= 4 {
                    // Thin vertical lines (grid lines): one or two pixels a row, so no per-row fill call.
                    for p in row {
                        *p = v;
                    }
                } else {
                    row.fill(v);
                }
            } else if c.a > 0 {
                blend_row(row, c);
            }
        }
    }

    /// `fill_px_in` on bytes (an unaligned pixmap, not expected in practice).
    #[allow(clippy::too_many_arguments)]
    fn fill_bytes(data: &mut [u8], pw: usize, (x0, x1): (usize, usize), (y0, y1): (i32, i32), ry0: i32, span: f32, top: Color, bottom: Color) {
        for y in y0..y1 {
            let c = if top == bottom { top } else { top.mix(bottom, (y - ry0) as f32 / span) };
            let (sa, inv) = (u32::from(c.a), 255 - u32::from(c.a));
            for d in data[(y as usize * pw + x0) * 4..(y as usize * pw + x1) * 4].as_chunks_mut::<4>().0 {
                for (k, s) in [c.r, c.g, c.b].into_iter().enumerate() {
                    d[k] = ((u32::from(s) * sa + u32::from(d[k]) * inv) / 255) as u8;
                }
            }
        }
    }

    /// Elliptical radial gradient over `r`: `center` at the middle, `edge` where the normalized
    /// distance reaches 1 (the half-width / half-height), falling off with the square of the distance.
    pub fn fill_radial(&mut self, r: Rect, center: Color, edge: Color) {
        for i in 0..self.clip_count() {
            let clip = self.clip_at(i);
            self.fill_radial_in(clip, r, center, edge);
        }
    }

    fn fill_radial_in(&mut self, (cx0, cy0, cx1, cy1): (i32, i32, i32, i32), r: Rect, center: Color, edge: Color) {
        let k = self.scale;
        let x0 = ((r.x * k).round() as i32).max(cx0);
        let x1 = ((r.right() * k).round() as i32).min(cx1);
        let y0 = ((r.y * k).round() as i32).max(cy0);
        let y1 = ((r.bottom() * k).round() as i32).min(cy1);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        self.mark(x0, y0, x1, y1);
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
        if let Some(cov) = self.coverage(&path, None) {
            let k = self.scale;
            self.blend(&cov, top, Some((bottom, r.y * k, r.bottom() * k)));
        }
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

    /// `fill_polygon` coloured column by column (colourful waveforms): each device-pixel column takes `color(x)`, `x`
    /// the logical x of the column's centre.
    pub fn fill_polygon_columns(&mut self, pts: &[(f32, f32)], color: &mut dyn FnMut(f32) -> Color) {
        if let Some(path) = poly_path(pts, true) {
            self.columns(&path, None, color);
        }
    }

    /// `polyline` coloured column by column, as `fill_polygon_columns`.
    pub fn polyline_columns(&mut self, pts: &[(f32, f32)], width: f32, color: &mut dyn FnMut(f32) -> Color) {
        if let Some(path) = poly_path(pts, false) {
            let s = Stroke { width, ..Stroke::default() };
            self.columns(&path, Some(&s), color);
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
            let s = Stroke { width, line_cap: LineCap::Round, ..Stroke::default() };
            if let Some(cov) = self.coverage(&path, Some(&s)) {
                self.blend(&cov, c, None);
            }
        }
    }

    pub fn fill(&mut self, path: &tiny_skia::Path, c: Color) {
        if self.outside(path.bounds(), 1.0) {
            return;
        }
        if let Some(cov) = self.coverage(path, None) {
            self.blend(&cov, c, None);
        }
    }

    pub fn stroke(&mut self, path: &tiny_skia::Path, width: f32, c: Color) {
        if self.outside(path.bounds(), width + 1.0) {
            return;
        }
        let s = Stroke { width, ..Stroke::default() };
        if let Some(cov) = self.coverage(path, Some(&s)) {
            self.blend(&cov, c, None);
        }
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
        let (xi, yi) = ((x * k).round() as i32, (y * k).round() as i32);
        let (w, h) = self.text.extent(f, k, s);
        for i in 0..self.clip_count() {
            let clip = self.clip_at(i);
            if clip.0 >= clip.2 || clip.1 >= clip.3 || xi > clip.2 || yi > clip.3 || xi + w + 8 < clip.0 || yi + h + 4 < clip.1 {
                continue;
            }
            // Glyphs overhang their cell (italics, descenders, ClearType fringes): mark generously.
            self.mark((xi - h).max(clip.0), (yi - h).max(clip.1), (xi + w + h).min(clip.2), (yi + 2 * h).min(clip.3));
            self.text.draw(&mut self.pixmap, clip, f, k, xi, yi, c, s, false);
        }
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
        let (xi, yi) = ((x * k).round() as i32, (y_bottom * k).round() as i32);
        let (w, h) = self.text.extent(f, k, s);
        for i in 0..self.clip_count() {
            let clip = self.clip_at(i);
            if clip.0 >= clip.2 || clip.1 >= clip.3 || xi > clip.2 || xi + h + 4 < clip.0 || yi + 4 < clip.1 || yi - w - 4 > clip.3 {
                continue;
            }
            self.mark((xi - h).max(clip.0), (yi - w - h).max(clip.1), (xi + 2 * h).min(clip.2), (yi + h).min(clip.3));
            self.text.draw(&mut self.pixmap, clip, f, k, xi, yi, c, s, true);
        }
        w as f32 / k
    }

    /// Draw a Tabler icon (24-unit grid) centred in `r`, scaled to the shorter side, in color `c`.
    pub fn icon(&mut self, icon: &tabler::TablerIcon, r: Rect, c: Color) {
        let k = r.w.min(r.h) / 24.0;
        let stroke = if icon.filled { None } else { Some((2.0 * k).max(1.1)) };
        self.tabler(icon, r, c, stroke);
    }

    /// An outline Tabler icon like `icon`, stroked `width` logical px wide whatever its size (small, bold glyphs).
    pub fn icon_stroke(&mut self, icon: &tabler::TablerIcon, r: Rect, c: Color, width: f32) {
        self.tabler(icon, r, c, (!icon.filled).then_some(width));
    }

    /// A Tabler icon fitted into `r`; its coverage is cached by icon, box, stroke and scale (parsing and
    /// rasterizing it again every frame was most of the cost of drawing an icon).
    fn tabler(&mut self, icon: &tabler::TablerIcon, r: Rect, c: Color, stroke: Option<f32>) {
        let mut h = Fx::default();
        h.write_u8(4);
        h.write_usize(icon.d.as_ptr() as usize);
        h.write_usize(icon.d.len());
        for v in [r.x, r.y, r.w, r.h, self.scale, stroke.unwrap_or(-1.0)] {
            h.f32(v);
        }
        let key = h.finish();
        if let Some(cov) = self.glyphs.get(key).cloned() {
            self.blend_glyph(&cov, c);
            return;
        }
        // The icon lies in the square of the shorter side centred in `r`, plus half the stroke.
        if !self.visible(r.outset(stroke.unwrap_or(0.0) + 2.0)) {
            return;
        }
        let k = r.w.min(r.h) / 24.0;
        let (ox, oy) = (r.cx() - 12.0 * k, r.cy() - 12.0 * k);
        let mut g = Glyph::new();
        g.round_caps = true;
        g.commands(icon.d, ox, oy, k, stroke);
        if let Some(cov) = self.glyph_coverage(&g) {
            self.glyphs.insert(key, cov.clone(), cov.cov.len() * 3 + 64);
            self.blend_glyph(&cov, c);
        }
    }

    /// Draw an icon glyph with subpixel (ClearType-style) coverage in color `c`.
    pub fn glyph(&mut self, c: Color, g: &Glyph) {
        let mut h = Fx::default();
        h.write_u8(3);
        h.f32(self.scale);
        for (p, erase) in &g.shapes {
            h.write_u8(u8::from(*erase));
            hash_path(&mut h, p);
        }
        let key = h.finish();
        if let Some(cov) = self.glyphs.get(key).cloned() {
            self.blend_glyph(&cov, c);
            return;
        }
        let Some(b) = g.bounds() else { return };
        if !self.visible(b.outset(2.0)) {
            return;
        }
        if let Some(cov) = self.glyph_coverage(g) {
            self.glyphs.insert(key, cov.clone(), cov.cov.len() * 3 + 64);
            self.blend_glyph(&cov, c);
        }
    }

    /// Rasterize the whole glyph (the subpixel filter needs neighbours); blending clips it.
    fn glyph_coverage(&self, g: &Glyph) -> Option<Rc<GlyphCoverage>> {
        let b = g.bounds()?;
        let k = self.scale;
        let x0 = (b.x * k).floor() as i32 - 1;
        let y0 = (b.y * k).floor() as i32;
        let x1 = (b.right() * k).ceil() as i32 + 1;
        let y1 = (b.bottom() * k).ceil() as i32;
        let (w, h) = (x1 - x0, y1 - y0);
        if w <= 0 || h <= 0 {
            return None;
        }
        let cov = glyph::coverage(g, k, x0, y0, w, h)?;
        Some(Rc::new(GlyphCoverage { x0, y0, w, h, cov }))
    }

    fn blend_glyph(&mut self, g: &GlyphCoverage, c: Color) {
        for i in 0..self.clip_count() {
            let clip = self.clip_at(i);
            self.blend_glyph_in(clip, g, c);
        }
    }

    fn blend_glyph_in(&mut self, clip: (i32, i32, i32, i32), g: &GlyphCoverage, c: Color) {
        let (x0, x1) = (g.x0.max(clip.0), (g.x0 + g.w).min(clip.2));
        let (y0, y1) = (g.y0.max(clip.1), (g.y0 + g.h).min(clip.3));
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        self.mark(x0, y0, x1, y1);
        let pw = self.pixmap.width() as usize;
        let n = (x1 - x0) as usize;
        let data = self.pixmap.data_mut();
        let a = u32::from(c.a);
        let fg = [u32::from(c.r), u32::from(c.g), u32::from(c.b)];
        for y in y0..y1 {
            let src = &g.cov[((y - g.y0) * g.w + (x0 - g.x0)) as usize..][..n];
            let dst = &mut data[(y as usize * pw + x0 as usize) * 4..][..n * 4];
            for (d, cv) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src) {
                if *cv == [0, 0, 0] {
                    continue;
                }
                for ch in 0..3 {
                    let al = u32::from(cv[ch]) * a / 255;
                    d[ch] = ((fg[ch] * al + u32::from(d[ch]) * (255 - al)) / 255) as u8;
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
        for i in 0..self.clip_count() {
            let clip = self.clip_at(i);
            self.image_in(clip, r, w, h, px);
        }
    }

    fn image_in(&mut self, (cx0, cy0, cx1, cy1): (i32, i32, i32, i32), r: Rect, w: usize, h: usize, px: &[u32]) {
        let k = self.scale;
        let (x0, x1) = (((r.x * k).round() as i32).max(cx0), ((r.right() * k).round() as i32).min(cx1));
        let (y0, y1) = (((r.y * k).round() as i32).max(cy0), ((r.bottom() * k).round() as i32).min(cy1));
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        self.mark(x0, y0, x1, y1);
        let (rw, rh) = ((r.w * k).max(1.0), (r.h * k).max(1.0));
        let (ox, oy) = (r.x * k, r.y * k);
        if (rw, rh) == (w as f32, h as f32) && ox.fract() == 0.0 && oy.fract() == 0.0 {
            // Pixel for pixel on whole device pixels: what the filter gives, copied without it (a workspace picture
            // drawn behind the windows with every repaint).
            let (ox, oy) = (ox as i32, oy as i32);
            let pw = self.pixmap.width() as usize;
            let pixels = self.pixmap.pixels_mut();
            for y in y0..y1 {
                let src = &px[(y - oy) as usize * w..][..w];
                for x in x0..x1 {
                    let v = src[(x - ox) as usize];
                    pixels[y as usize * pw + x as usize] =
                        PremultipliedColorU8::from_rgba((v >> 16) as u8, (v >> 8) as u8, v as u8, 255).expect("opaque");
                }
            }
            return;
        }
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
        let data = self.pixmap.data();
        // SAFETY: a u32 view of whole RGBA pixels; an unaligned head (not expected) takes the per-byte path.
        let (head, px, _) = unsafe { data.align_to::<u32>() };
        for row in y..y + h {
            let d = (row + oy) * out_stride + x + ox;
            let Some(dst) = out.get_mut(d..d + w) else { return };
            if head.is_empty() && cfg!(target_endian = "little") {
                // Bytes R G B A read as a little-endian word: swap R and B, drop A (the compiler vectorizes this).
                for (o, &v) in dst.iter_mut().zip(&px[row * stride + x..row * stride + x + w]) {
                    *o = ((v & 0xff) << 16) | (v & 0xff00) | ((v >> 16) & 0xff);
                }
            } else {
                for (o, p) in dst.iter_mut().zip(data[(row * stride + x) * 4..(row * stride + x + w) * 4].as_chunks::<4>().0) {
                    *o = (u32::from(p[0]) << 16) | (u32::from(p[1]) << 8) | u32::from(p[2]);
                }
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

/// Blend `c` (straight alpha) over a row of opaque pixels: `(c * a + d * (255 - a)) / 255` per channel, truncated.
/// `x / 255 == (x * 0x8081) >> 23` for every x up to 255 * 255; four pixels at a time with SSE2 (x86-64 baseline).
fn blend_row(row: &mut [u32], c: Color) {
    let sa = u32::from(c.a);
    let inv = 255 - sa;
    let (pr, pg, pb) = (u32::from(c.r) * sa, u32::from(c.g) * sa, u32::from(c.b) * sa);
    #[cfg(target_arch = "x86_64")]
    let row = {
        use std::arch::x86_64::*;
        let (chunks, rest) = row.as_chunks_mut::<4>();
        // SAFETY: SSE2 is always available on x86-64; the loads and stores are unaligned and stay inside the chunk.
        unsafe {
            let (r, g, b, a) = (pr as i16, pg as i16, pb as i16, (255 * sa) as u16 as i16);
            let src = _mm_setr_epi16(r, g, b, a, r, g, b, a);
            let invv = _mm_set1_epi16(inv as i16);
            let magic = _mm_set1_epi16(0x8081u16 as i16);
            let zero = _mm_setzero_si128();
            for ch in chunks {
                let p = ch.as_mut_ptr().cast::<__m128i>();
                let d = _mm_loadu_si128(p);
                // Bytes to 16-bit lanes (two pixels per register); d * inv + src stays within 255 * 255.
                let lo = _mm_add_epi16(_mm_mullo_epi16(_mm_unpacklo_epi8(d, zero), invv), src);
                let hi = _mm_add_epi16(_mm_mullo_epi16(_mm_unpackhi_epi8(d, zero), invv), src);
                let lo = _mm_srli_epi16(_mm_mulhi_epu16(lo, magic), 7);
                let hi = _mm_srli_epi16(_mm_mulhi_epu16(hi, magic), 7);
                _mm_storeu_si128(p, _mm_packus_epi16(lo, hi));
            }
        }
        rest
    };
    let div = |x: u32| (x * 0x8081) >> 23;
    for p in row {
        let [r, g, b, _] = p.to_ne_bytes();
        *p = u32::from_ne_bytes([div(pr + u32::from(r) * inv) as u8, div(pg + u32::from(g) * inv) as u8, div(pb + u32::from(b) * inv) as u8, 255]);
    }
}

/// `path`'s verbs with new points given in 1/64 device pixels.
fn rebuild_path(path: &tiny_skia::Path, pts: &[(i32, i32)]) -> Option<tiny_skia::Path> {
    use tiny_skia::PathSegment;
    let mut it = pts.iter().map(|&(x, y)| (x as f32 / 64.0, y as f32 / 64.0));
    let mut pb = PathBuilder::new();
    // Segments come in point order: one point per move / line, two per quad, three per cubic.
    for seg in path.segments() {
        match seg {
            PathSegment::MoveTo(_) => {
                let (x, y) = it.next()?;
                pb.move_to(x, y);
            }
            PathSegment::LineTo(_) => {
                let (x, y) = it.next()?;
                pb.line_to(x, y);
            }
            PathSegment::QuadTo(..) => {
                let ((x1, y1), (x, y)) = (it.next()?, it.next()?);
                pb.quad_to(x1, y1, x, y);
            }
            PathSegment::CubicTo(..) => {
                let ((x1, y1), (x2, y2), (x, y)) = (it.next()?, it.next()?, it.next()?);
                pb.cubic_to(x1, y1, x2, y2, x, y);
            }
            PathSegment::Close => pb.close(),
        }
    }
    pb.finish()
}

/// Blend `c` over a row of opaque pixels through per-pixel coverage `cov`: with `al = (cov * a + 127) / 255`, each
/// channel becomes `(c * al + d * (255 - al) + 127) / 255`. Four pixels at a time with SSE2 (x86-64 baseline); the
/// division is `(x * 0x8081) >> 23`, exact for every 16-bit x.
fn blend_cov_row(row: &mut [[u8; 4]], cov: &[u8], c: Color) {
    let ca = u32::from(c.a);
    let div = |x: u32| (x * 0x8081) >> 23;
    #[cfg(target_arch = "x86_64")]
    let (row, cov) = {
        use std::arch::x86_64::*;
        let (chunks, rest) = row.as_chunks_mut::<4>();
        let n = chunks.len() * 4;
        // SAFETY: SSE2 is always available on x86-64; loads and stores are unaligned and stay inside each chunk.
        unsafe {
            let (r, g, b) = (i16::from(c.r), i16::from(c.g), i16::from(c.b));
            let src = _mm_setr_epi16(r, g, b, 255, r, g, b, 255);
            let ca_v = _mm_set1_epi16(c.a.into());
            let full = _mm_set1_epi16(255);
            let half = _mm_set1_epi16(127);
            let magic = _mm_set1_epi16(0x8081u16 as i16);
            let zero = _mm_setzero_si128();
            for (ch, cv) in chunks.iter_mut().zip(cov.as_chunks::<4>().0) {
                let cw = u32::from_le_bytes(*cv);
                if cw == 0 {
                    continue;
                }
                // Coverage to alpha (16-bit lanes), then each pixel's alpha spread over its four channels.
                let cv16 = _mm_unpacklo_epi8(_mm_cvtsi32_si128(cw as i32), zero);
                let al = _mm_srli_epi16(_mm_mulhi_epu16(_mm_add_epi16(_mm_mullo_epi16(cv16, ca_v), half), magic), 7);
                let al2 = _mm_unpacklo_epi16(al, al);
                let (alo, ahi) = (_mm_unpacklo_epi32(al2, al2), _mm_unpackhi_epi32(al2, al2));
                let p = ch.as_mut_ptr().cast::<__m128i>();
                let d = _mm_loadu_si128(p);
                let mix = |d16: __m128i, a: __m128i| {
                    let x = _mm_add_epi16(_mm_add_epi16(_mm_mullo_epi16(src, a), _mm_mullo_epi16(d16, _mm_sub_epi16(full, a))), half);
                    _mm_srli_epi16(_mm_mulhi_epu16(x, magic), 7)
                };
                let lo = mix(_mm_unpacklo_epi8(d, zero), alo);
                let hi = mix(_mm_unpackhi_epi8(d, zero), ahi);
                _mm_storeu_si128(p, _mm_packus_epi16(lo, hi));
            }
        }
        (rest, &cov[n..])
    };
    for (d, &a) in row.iter_mut().zip(cov) {
        if a == 0 {
            continue;
        }
        let al = div(u32::from(a) * ca + 127);
        let inv = 255 - al;
        for (k, s) in [c.r, c.g, c.b].into_iter().enumerate() {
            d[k] = div(u32::from(s) * al + u32::from(d[k]) * inv + 127) as u8;
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

/// The device pixels (x0, y0, x1, y1), end-exclusive, that [`Canvas::shift`] moves for `r` on a `size` canvas at
/// `scale`: the edges rounded as a clip to `r` rounds them, cut to the canvas.
pub fn shift_area(r: Rect, scale: f32, (w, h): (u32, u32)) -> (i32, i32, i32, i32) {
    let px = |v: f32, max: u32| ((v * scale).round() as i32).clamp(0, max as i32);
    (px(r.x, w), px(r.y, h), px(r.right(), w), px(r.bottom(), h))
}

/// The device pixels wholly inside `r` (x0, y0, x1, y1), cut to the canvas: what [`Canvas::shift`] keeps in place.
pub fn keep_area(r: Rect, scale: f32, (w, h): (u32, u32)) -> (i32, i32, i32, i32) {
    let lo = |v: f32, max: u32| ((v * scale).ceil() as i32).clamp(0, max as i32);
    let hi = |v: f32, max: u32| ((v * scale).floor() as i32).clamp(0, max as i32);
    (lo(r.x, w), lo(r.y, h), hi(r.right(), w), hi(r.bottom(), h))
}

/// The device pixels wholly inside `r`, as a logical rect (empty when there are none): what a kept `r` keeps. A
/// pixel `r` covers only in part is not kept (it blends with what is under `r`).
pub fn inner_rect(r: Rect, scale: f32) -> Rect {
    let (x0, y0) = ((r.x * scale).ceil(), (r.y * scale).ceil());
    let (x1, y1) = ((r.right() * scale).floor(), (r.bottom() * scale).floor());
    Rect::new(x0 / scale, y0 / scale, ((x1 - x0) / scale).max(0.0), ((y1 - y0) / scale).max(0.0))
}

/// Move the pixels of the device area `(x0, y0, x1, y1)` of an image (`stride` pixels a row) by `dx`, `dy`, except
/// those in the `keep` areas, as [`Canvas::shift`] does; also for a window buffer holding the same picture, so it
/// can follow the canvas.
pub fn shift_pixels<T: Copy>(data: &mut [T], stride: usize, (x0, y0, x1, y1): (i32, i32, i32, i32), dx: i32, dy: i32, keep: &[(i32, i32, i32, i32)]) {
    // The destination pixels that have a source inside the area.
    let (dx0, dx1) = (x0.max(x0 + dx), x1.min(x1 + dx));
    let (dy0, dy1) = (y0.max(y0 + dy), y1.min(y1 + dy));
    if dx0 >= dx1 || dy0 >= dy1 {
        return;
    }
    let kept: Vec<_> = keep
        .iter()
        .map(|k| (k.0.max(x0), k.1.max(y0), k.2.min(x1), k.3.min(y1)))
        .filter(|k| k.0 < k.2 && k.1 < k.3)
        .map(|k| {
            let rows = (k.1..k.3).flat_map(|y| &data[y as usize * stride + k.0 as usize..y as usize * stride + k.2 as usize]);
            (k, rows.copied().collect::<Vec<T>>())
        })
        .collect();
    let n = (dx1 - dx0) as usize;
    let mut copy = |y: i32| {
        let src = (y - dy) as usize * stride + (dx0 - dx) as usize;
        data.copy_within(src..src + n, y as usize * stride + dx0 as usize);
    };
    // Moving down, copy the bottom rows first so no source row is overwritten before it is read.
    if dy > 0 {
        (dy0..dy1).rev().for_each(&mut copy);
    } else {
        (dy0..dy1).for_each(&mut copy);
    }
    for (k, px) in kept {
        let w = (k.2 - k.0) as usize;
        for (y, row) in (k.1..k.3).zip(px.chunks_exact(w)) {
            let at = y as usize * stride + k.0 as usize;
            data[at..at + w].copy_from_slice(row);
        }
    }
}

/// A `w` x `h` image of `0x00RRGGBB` pixels resized to `dw` x `dh` with [`Canvas::image`]'s filter: drawn once at a
/// size that then copies pixel for pixel (a workspace picture, redrawn with every repaint behind the windows).
pub fn resample(w: usize, h: usize, px: &[u32], dw: usize, dh: usize) -> Vec<u32> {
    if w == 0 || h == 0 || px.len() < w * h || dw == 0 || dh == 0 {
        return vec![0; dw * dh];
    }
    let xs: Vec<Vec<(usize, f32)>> = (0..dw).map(|x| taps(x as f32 + 0.5, w as f32 / dw as f32, w)).collect();
    let mut out = Vec::with_capacity(dw * dh);
    for y in 0..dh {
        let ty = taps(y as f32 + 0.5, h as f32 / dh as f32, h);
        for tx in &xs {
            let mut acc = [0.0f32; 3];
            for &(sy, wy) in &ty {
                for &(sx, wx) in tx {
                    let v = px[sy * w + sx];
                    let wgt = wx * wy;
                    acc[0] += ((v >> 16) & 0xff) as f32 * wgt;
                    acc[1] += ((v >> 8) & 0xff) as f32 * wgt;
                    acc[2] += (v & 0xff) as f32 * wgt;
                }
            }
            let ch = |a: f32| u32::from(a.round().clamp(0.0, 255.0) as u8);
            out.push(ch(acc[0]) << 16 | ch(acc[1]) << 8 | ch(acc[2]));
        }
    }
    out
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
    fn present_copy_is_xrgb() {
        let Ok(mut c) = Canvas::new(7, 3, 1.0) else { return };
        c.fill_rect(Rect::new(0.0, 0.0, 7.0, 3.0), Color::rgb(0x123456));
        c.fill_rect(Rect::new(2.0, 1.0, 3.0, 1.0), Color::rgb(0xfedcba));
        let mut out = vec![0u32; 9 * 4];
        c.copy_region_to_xrgb_at(&mut out, 9, (1, 0, 5, 3), (2, 1));
        assert_eq!(out[9 + 3], 0x123456, "row 0 -> out row 1, x 1 -> 3");
        assert_eq!(out[2 * 9 + 4], 0xfedcba);
        assert_eq!(out[2 * 9 + 2], 0, "left of the copied region");
        assert_eq!(out[0], 0);
    }

    /// `rescale` paints at another scale inside the same clip (device bounds); `unscale` comes back.
    #[test]
    fn rescale_paints_at_another_scale() {
        let Ok(mut c) = Canvas::new(20, 10, 1.0) else { return };
        let px = |c: &Canvas, x: usize, y: usize| c.pixmap.data()[(y * 20 + x) * 4];
        c.push_clip(Rect::new(0.0, 0.0, 6.0, 10.0));
        let old = c.rescale(2.0);
        assert_eq!((c.size(), c.clip_rect()), ((10.0, 5.0), Rect::new(0.0, 0.0, 3.0, 5.0)));
        c.fill_rect(Rect::new(1.0, 1.0, 4.0, 1.0), Color::rgb(0xff0000));
        c.unscale(old);
        assert_eq!((c.scale, c.clip_rect()), (1.0, Rect::new(0.0, 0.0, 6.0, 10.0)));
        assert_eq!([px(&c, 2, 2), px(&c, 5, 3), px(&c, 6, 2), px(&c, 1, 2), px(&c, 2, 4)], [255, 255, 0, 0, 0], "device 2..6 x 2..4, cut at 6");
    }

    /// `shift` moves the pixels of its area by whole device pixels in any direction (overlapping copies included),
    /// leaves the strip it uncovers, everything outside the area and the kept pixels alone, and rounds the area like a
    /// clip (kept areas inwards).
    #[test]
    fn shift_moves_an_area() {
        let (w, h) = (23u32, 17u32);
        let px = |c: &Canvas, x: i32, y: i32| -> [u8; 4] {
            let i = (y as usize * w as usize + x as usize) * 4;
            c.pixmap.data()[i..i + 4].try_into().unwrap()
        };
        // Every pixel a different colour.
        let paint = |c: &mut Canvas| {
            for (i, p) in c.pixmap.data_mut().chunks_exact_mut(4).enumerate() {
                p.copy_from_slice(&[(i % 251) as u8, (i / 251) as u8, (i * 7 % 253) as u8, 255]);
            }
        };
        for scale in [1.0f32, 1.5] {
            let Ok(mut c) = Canvas::new(w, h, scale) else { return };
            // At 150 % the area (3.4, 2.2)-(13.4, 12.2) rounds to device (5, 3)-(20, 18), cut to the canvas.
            let r = Rect::new(3.4, 2.2, 10.0, 10.0);
            let (x0, y0, x1, y1) = shift_area(r, scale, (w, h));
            assert!(x1 <= w as i32 && y1 <= h as i32 && x0 < x1 && y0 < y1);
            // A window inside the area and one reaching out of it.
            let keeps = [Rect::new(6.3, 5.6, 4.0, 3.0), Rect::new(1.2, 9.5, 5.0, 4.4)];
            let kept: Vec<_> = keeps.iter().map(|k| keep_area(*k, scale, (w, h))).collect();
            let cases = [(3, 0), (-4, 0), (0, 2), (0, -5), (2, -3), (-1, 4), (30, 0), (0, 0)];
            for ((dx, dy), keep) in cases.into_iter().flat_map(|d| [(d, &[][..]), (d, &keeps[..])]) {
                let kept = if keep.is_empty() { &[][..] } else { &kept[..] };
                paint(&mut c);
                let before = c.pixmap.clone();
                let old = |x: i32, y: i32| -> [u8; 4] {
                    let i = (y as usize * w as usize + x as usize) * 4;
                    before.data()[i..i + 4].try_into().unwrap()
                };
                // A window buffer holding the canvas (wider, at an offset) follows it with the same move.
                let (bw, off) = (w as usize + 3, (2usize, 1usize));
                let mut buf = vec![0u32; bw * (h as usize + 2)];
                c.copy_region_to_xrgb_at(&mut buf, bw, (0, 0, w as usize, h as usize), off);
                c.shift(r, dx, dy, keep);
                let at = |k: &(i32, i32, i32, i32)| (k.0 + 2, k.1 + 1, k.2 + 2, k.3 + 1);
                shift_pixels(&mut buf, bw, (x0 + 2, y0 + 1, x1 + 2, y1 + 1), dx, dy, &kept.iter().map(at).collect::<Vec<_>>());
                let mut fresh = vec![0u32; bw * (h as usize + 2)];
                c.copy_region_to_xrgb_at(&mut fresh, bw, (0, 0, w as usize, h as usize), off);
                assert!(buf == fresh, "scale {scale} shift ({dx}, {dy}): the buffer follows the canvas");
                for y in 0..h as i32 {
                    for x in 0..w as i32 {
                        let inside = (x0..x1).contains(&x) && (y0..y1).contains(&y);
                        let (sx, sy) = (x - dx, y - dy);
                        let from_inside = (x0..x1).contains(&sx) && (y0..y1).contains(&sy);
                        let still = kept.iter().any(|k| (k.0..k.2).contains(&x) && (k.1..k.3).contains(&y));
                        let want = if inside && from_inside && !still { old(sx, sy) } else { old(x, y) };
                        assert_eq!(px(&c, x, y), want, "scale {scale} shift ({dx}, {dy}) at ({x}, {y})");
                    }
                }
            }
        }
    }

    /// `paint_bounds` is the clip cut by the frame's region, in logical pixels.
    #[test]
    fn paint_bounds_follow_the_region() {
        let Ok(mut c) = Canvas::new(100, 80, 2.0) else { return };
        assert_eq!(c.paint_bounds(), Some(Rect::new(0.0, 0.0, 50.0, 40.0)));
        c.begin_region(&[Rect::new(10.0, 5.0, 4.0, 3.0), Rect::new(30.0, 20.0, 2.0, 2.0)]);
        assert_eq!(c.paint_bounds(), Some(Rect::new(10.0, 5.0, 22.0, 17.0)));
        c.push_clip(Rect::new(0.0, 0.0, 12.0, 40.0));
        assert_eq!(c.paint_bounds(), Some(Rect::new(10.0, 5.0, 2.0, 3.0)));
        c.pop_clip();
        c.push_clip(Rect::new(40.0, 0.0, 5.0, 5.0));
        assert_eq!(c.paint_bounds(), None);
    }

    #[test]
    fn coverage_blend_matches_the_formula() {
        let mut seed = 777u32;
        let mut next = || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (seed >> 16) as u8
        };
        for a in [0u8, 1, 64, 127, 128, 200, 254, 255] {
            for n in 0..14 {
                let c = Color { r: next(), g: next(), b: next(), a };
                // Coverage with zeros, full and partial values.
                let cov: Vec<u8> = (0..n).map(|i| match i % 5 { 0 => 0, 1 => 255, _ => next() }).collect();
                let row: Vec<[u8; 4]> = (0..n).map(|_| [next(), next(), next(), 255]).collect();
                let mut got = row.clone();
                blend_cov_row(&mut got, &cov, c);
                for ((g, d), &cv) in got.iter().zip(&row).zip(&cov) {
                    let al = (u32::from(cv) * u32::from(a) + 127) / 255;
                    let mix = |s: u8, d: u8| ((u32::from(s) * al + u32::from(d) * (255 - al) + 127) / 255) as u8;
                    let want = if cv == 0 { *d } else { [mix(c.r, d[0]), mix(c.g, d[1]), mix(c.b, d[2]), 255] };
                    assert_eq!(*g, want, "alpha {a}, coverage {cv}");
                }
            }
        }
    }

    // spec: ui/settings-choices.md "Display: Colorful waveforms (2026-10-16)"
    /// A shape filled or stroked column by column covers the pixels the one-colour call does: with one colour the
    /// pictures are the same; with a colour per column each column has its own.
    #[test]
    fn column_colours_draw_the_same_shape() {
        let pts = [(3.2, 20.0), (9.7, 4.4), (18.1, 15.3), (29.6, 2.8), (33.0, 24.5), (12.4, 27.1)];
        for scale in [1.0, 1.5] {
            let (Ok(mut a), Ok(mut b)) = (Canvas::new(40, 30, scale), Canvas::new(40, 30, scale)) else { return };
            for c in [&mut a, &mut b] {
                c.fill_rect(Rect::new(0.0, 0.0, 40.0, 30.0), Color::rgb(0x202020));
            }
            a.fill_polygon(&pts, Color::rgb(0xc0d0e0));
            a.polyline(&pts, 1.6, Color::rgb(0x405060));
            b.fill_polygon_columns(&pts, &mut |_| Color::rgb(0xc0d0e0));
            b.polyline_columns(&pts, 1.6, &mut |_| Color::rgb(0x405060));
            assert!(a.pixmap.data() == b.pixmap.data(), "scale {scale}");
        }
        let Ok(mut c) = Canvas::new(40, 30, 1.0) else { return };
        c.fill_rect(Rect::new(0.0, 0.0, 40.0, 30.0), Color::rgb(0x000000));
        c.fill_polygon_columns(&[(0.0, 0.0), (40.0, 0.0), (40.0, 30.0), (0.0, 30.0)], &mut |x| {
            if x < 20.0 { Color::rgb(0xff0000) } else { Color::rgb(0x0000ff) }
        });
        let px = |x: usize| c.pixmap.pixels()[10 * 40 + x];
        assert_eq!((px(19).red(), px(19).blue(), px(20).red(), px(20).blue()), (255, 0, 0, 255));
    }

    #[test]
    fn moved_shapes_reuse_their_coverage() {
        // The same circle 7 px further right draws the same pixels 7 px further right, from the cache.
        let Ok(mut c) = Canvas::new(60, 30, 1.0) else { return };
        c.fill_rect(Rect::new(0.0, 0.0, 60.0, 30.0), Color::rgb(0x000000));
        c.fill_circle(10.3, 12.6, 6.0, Color::rgb(0xffffff));
        let n = c.shapes.len();
        c.fill_circle(17.3, 12.6, 6.0, Color::rgb(0xffffff));
        assert_eq!(c.shapes.len(), n, "found in the cache");
        let red = |c: &Canvas, x: usize, y: usize| c.pixmap.pixels()[y * 60 + x].red();
        let Ok(mut d) = Canvas::new(60, 30, 1.0) else { return };
        d.fill_rect(Rect::new(0.0, 0.0, 60.0, 30.0), Color::rgb(0x000000));
        d.fill_circle(17.3, 12.6, 6.0, Color::rgb(0xffffff));
        for y in 0..30 {
            for x in 18..60 {
                assert!(red(&c, x, y).abs_diff(red(&d, x, y)) <= 1, "pixel {x},{y}");
            }
        }
    }

    #[test]
    fn row_blend_matches_the_formula() {
        // Every alpha, rows of 0..11 pixels (whole SIMD chunks and leftovers), varied destinations.
        let mut seed = 12345u32;
        let mut next = || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (seed >> 16) as u8
        };
        for a in 0..=255u8 {
            let c = Color { r: next(), g: next(), b: next(), a };
            for n in 0..12 {
                let row: Vec<u32> = (0..n).map(|_| u32::from_ne_bytes([next(), next(), next(), 255])).collect();
                let mut got = row.clone();
                blend_row(&mut got, c);
                for (g, d) in got.iter().zip(&row) {
                    let d = d.to_ne_bytes();
                    let mix = |s: u8, d: u8| ((u32::from(s) * u32::from(a) + u32::from(d) * (255 - u32::from(a))) / 255) as u8;
                    assert_eq!(g.to_ne_bytes(), [mix(c.r, d[0]), mix(c.g, d[1]), mix(c.b, d[2]), 255], "alpha {a}");
                }
            }
        }
    }

    #[test]
    fn covered_rects_are_left_for_later() {
        // A fill under a cover leaves the covered pixels; the rest is filled as without it.
        let Ok(mut c) = Canvas::new(40, 30, 1.5) else { return };
        c.fill_rect(Rect::new(0.0, 0.0, 40.0, 30.0), Color::rgb(0x000000));
        c.set_cover(Some(Rect::new(5.0, 4.0, 10.0, 8.0)));
        c.fill_rect_gradient(Rect::new(2.0, 2.0, 20.0, 15.0), Color::rgb(0x204060), Color::rgb(0x6080a0), 0.0);
        c.set_cover(None);
        let Ok(mut d) = Canvas::new(40, 30, 1.5) else { return };
        d.fill_rect(Rect::new(0.0, 0.0, 40.0, 30.0), Color::rgb(0x000000));
        d.fill_rect_gradient(Rect::new(2.0, 2.0, 20.0, 15.0), Color::rgb(0x204060), Color::rgb(0x6080a0), 0.0);
        let w = 40;
        for (i, (p, q)) in c.pixmap.pixels().iter().zip(d.pixmap.pixels()).enumerate() {
            let (x, y) = (i % w, i / w);
            let covered = (8..23).contains(&x) && (6..18).contains(&y);
            if covered {
                assert_eq!(p.red(), 0, "covered pixel {x},{y} left alone");
            } else {
                assert_eq!(p, q, "pixel {x},{y} as without a cover");
            }
        }
    }

    #[test]
    fn regions_clip_to_each_rect_once() {
        // Two overlapping damage rects: an alpha fill blends once where they overlap, nothing outside them.
        let Ok(mut c) = Canvas::new(30, 10, 1.0) else { return };
        c.fill_rect(Rect::new(0.0, 0.0, 30.0, 10.0), Color::rgb(0x000000));
        c.begin_region(&[Rect::new(2.0, 0.0, 10.0, 10.0), Rect::new(8.0, 0.0, 10.0, 10.0), Rect::new(25.0, 0.0, 2.0, 10.0)]);
        c.fill_rect(Rect::new(0.0, 0.0, 30.0, 10.0), Color::rgb(0xffffff).with_alpha(128));
        let row: Vec<u8> = c.pixmap.pixels()[..30].iter().map(|p| p.red()).collect();
        for (x, &v) in row.iter().enumerate() {
            let inside = (2..18).contains(&x) || (25..27).contains(&x);
            assert_eq!(v, if inside { 128 } else { 0 }, "x {x}: {row:?}");
        }
        assert!(c.visible(Rect::new(20.0, 0.0, 6.0, 1.0)) && !c.visible(Rect::new(19.0, 0.0, 5.0, 1.0)));
    }

    #[test]
    fn region_rects_that_barely_touch_stay_apart() {
        // A wide strip and a tall bar sharing one device column (fractional edges): no bounding box, every pixel of
        // either painted once, nothing else.
        let Ok(mut c) = Canvas::new(120, 100, 1.0) else { return };
        c.fill_rect(Rect::new(0.0, 0.0, 120.0, 100.0), Color::rgb(0x000000));
        let (strip, bar) = (Rect::new(10.3, 5.0, 90.0, 10.0), Rect::new(100.3, 0.0, 12.0, 90.0));
        c.begin_region(&[strip, bar, Rect::new(40.0, 60.0, 4.0, 4.0)]);
        let reg = c.region();
        let area: f32 = reg.iter().map(|r| r.w * r.h).sum();
        assert!(area < 91.0 * 10.0 + 13.0 * 90.0 + 16.0 + 1.0, "{reg:?}");
        c.fill_rect(Rect::new(0.0, 0.0, 120.0, 100.0), Color::rgb(0xffffff).with_alpha(128));
        let k = |r: Rect, x: usize, y: usize| r.x.floor() as usize <= x && x < r.right().ceil() as usize && r.y as usize <= y && y < r.bottom() as usize;
        for y in 0..100 {
            for x in 0..120 {
                let inside = k(strip, x, y) || k(bar, x, y) || ((40..44).contains(&x) && (60..64).contains(&y));
                assert_eq!(c.pixmap.pixel(x as u32, y as u32).unwrap().red(), if inside { 128 } else { 0 }, "pixel {x},{y}");
            }
        }
    }

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
        // At a whole-pixel offset, cut by the canvas edges: still a copy.
        let Ok(mut c) = Canvas::new(8, 2, 1.0) else { return };
        let grid: Vec<u32> = (0..12).map(|i| i * 0x0a0b0c).collect();
        c.image(Rect::new(5.0, -1.0, 4.0, 3.0), 4, 3, &grid);
        let mut out = vec![0u32; 16];
        c.copy_to_xrgb(&mut out);
        assert_eq!((out[5], out[7], out[8 + 6]), (grid[4], grid[6], grid[9]));
        // Half size: each device pixel averages a black and a white source pixel.
        let Ok(mut c) = Canvas::new(8, 1, 0.5) else { return };
        c.image(Rect::new(0.0, 0.0, 16.0, 2.0), 16, 1, &px);
        assert!(c.pixmap.pixels().iter().all(|p| (127..=128).contains(&p.red())), "box average when shrinking");
        // 150 %: enlarged pixels blend, no uneven doubling.
        let Ok(mut c) = Canvas::new(24, 1, 1.5) else { return };
        c.image(Rect::new(0.0, 0.0, 16.0, 1.0), 16, 1, &px);
        assert!(c.pixmap.pixels().iter().any(|p| p.red() > 20 && p.red() < 235), "linear when enlarging");
    }

    /// `resample` is `image`'s filter into a buffer: the same size copies, half size averages pairs.
    #[test]
    fn resample_filters_like_image() {
        let px: Vec<u32> = (0..16).map(|i| if i % 2 == 0 { 0x000000 } else { 0xffffff }).collect();
        assert_eq!(resample(16, 1, &px, 16, 1), px);
        assert!(resample(16, 1, &px, 8, 1).iter().all(|&v| v == 0x808080 || v == 0x7f7f7f), "box average");
        assert_eq!(resample(1, 1, &[0x102030], 3, 2), vec![0x102030; 6]);
        assert_eq!(resample(0, 0, &[], 2, 1), vec![0, 0]);
    }

    #[test]
    fn paints_text_and_shapes() {
        let Ok(mut c) = Canvas::new(200, 50, 1.0) else { return }; // no system font: skip
        c.clear(Color::rgb(0x000000));
        c.fill_rrect(Rect::new(5.0, 5.0, 20.0, 20.0), 3.0, Color::rgb(0xff0000));
        let w = c.text(Font::new(14), 40.0, 10.0, Color::rgb(0xffffff), "Lemon Studio");
        assert!(w > 40.0);
        assert!(c.pixmap.data().chunks_exact(4).any(|p| p[0] > 200 && p[1] > 200));
    }
}
