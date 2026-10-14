// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Icon glyphs drawn with subpixel (ClearType-style) coverage, so vector icons sit next to the
//! GDI-rendered text with the same crisp, color-fringed edges.
//!
//! A `Glyph` is a list of shapes in logical coordinates; shapes either add coverage or cut it
//! away. `Canvas::glyph` rasterizes them at three times the horizontal resolution, runs the
//! usual five-tap subpixel filter and blends each color channel separately.
// spec: ui/main-window.md §10 (icons are drawn like text glyphs)

use tiny_skia::{FillRule, LineCap, LineJoin, Mask, PathBuilder, Stroke, Transform};

use crate::Rect;

#[derive(Default, Clone)]
pub struct Glyph {
    pub(crate) shapes: Vec<(tiny_skia::Path, bool)>,
    erase: bool,
    /// Round caps and joins for strokes added from now on (default butt caps, miter joins).
    pub round_caps: bool,
}

impl Glyph {
    pub fn new() -> Glyph {
        Glyph::default()
    }

    /// Following shapes remove coverage (`true`) or add it (`false`).
    pub fn erase(&mut self, on: bool) -> &mut Self {
        self.erase = on;
        self
    }

    pub fn path(&mut self, p: tiny_skia::Path) -> &mut Self {
        self.shapes.push((p, self.erase));
        self
    }

    /// Stroke an arbitrary path (round caps also give round joins).
    pub fn stroke(&mut self, p: tiny_skia::Path, width: f32) -> &mut Self {
        self.stroke_path(p, width)
    }

    fn stroke_path(&mut self, p: tiny_skia::Path, width: f32) -> &mut Self {
        let s = Stroke {
            width,
            line_cap: if self.round_caps { LineCap::Round } else { LineCap::Butt },
            line_join: if self.round_caps { LineJoin::Round } else { LineJoin::Miter },
            ..Stroke::default()
        };
        if let Some(o) = p.stroke(&s, 4.0) {
            self.path(o);
        }
        self
    }

    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32) -> &mut Self {
        if let Some(r) = tiny_skia::Rect::from_xywh(x, y, w, h) {
            self.path(PathBuilder::from_rect(r));
        }
        self
    }

    pub fn rrect(&mut self, r: Rect, radius: f32) -> &mut Self {
        if let Some(p) = crate::rrect_path(r, radius) {
            self.path(p);
        }
        self
    }

    /// Outline of a rect: a band of `width` just inside `r`.
    pub fn frame(&mut self, r: Rect, width: f32) -> &mut Self {
        let erase = self.erase;
        self.rect(r.x, r.y, r.w, width).rect(r.x, r.bottom() - width, r.w, width);
        self.rect(r.x, r.y + width, width, r.h - 2.0 * width).rect(r.right() - width, r.y + width, width, r.h - 2.0 * width);
        self.erase = erase;
        self
    }

    pub fn circle(&mut self, cx: f32, cy: f32, r: f32) -> &mut Self {
        if let Some(p) = PathBuilder::from_circle(cx, cy, r) {
            self.path(p);
        }
        self
    }

    /// A ring whose outer radius is `r`.
    pub fn ring(&mut self, cx: f32, cy: f32, r: f32, width: f32) -> &mut Self {
        if let Some(p) = PathBuilder::from_circle(cx, cy, r - width / 2.0) {
            self.stroke_path(p, width);
        }
        self
    }

    pub fn polygon(&mut self, pts: &[(f32, f32)]) -> &mut Self {
        if let Some(p) = crate::poly_path(pts, true) {
            self.path(p);
        }
        self
    }

    pub fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32) -> &mut Self {
        self.polyline(&[(x0, y0), (x1, y1)], width)
    }

    pub fn polyline(&mut self, pts: &[(f32, f32)], width: f32) -> &mut Self {
        if let Some(p) = crate::poly_path(pts, false) {
            self.stroke_path(p, width);
        }
        self
    }

    /// Arc from `start` sweeping `sweep` radians (0 = 3 o'clock, positive = clockwise on screen),
    /// centred on radius `r`.
    pub fn arc(&mut self, cx: f32, cy: f32, r: f32, start: f32, sweep: f32, width: f32) -> &mut Self {
        let steps = ((sweep.abs() * r).ceil() as usize).clamp(6, 96);
        let pts: Vec<(f32, f32)> = (0..=steps)
            .map(|i| {
                let a = start + sweep * i as f32 / steps as f32;
                (cx + r * a.cos(), cy + r * a.sin())
            })
            .collect();
        self.polyline(&pts, width)
    }

    /// Add path data made of absolute `M x y`, `L x y`, `Q x1 y1 x y`, `C x1 y1 x2 y2 x y` and `Z`
    /// commands (space separated), mapped by `(x, y) -> (ox + x * k, oy + y * k)`. Filled when
    /// `stroke` is `None`, otherwise stroked with that width.
    pub fn commands(&mut self, d: &str, ox: f32, oy: f32, k: f32, stroke: Option<f32>) -> &mut Self {
        let mut pb = PathBuilder::new();
        let mut it = d.split_ascii_whitespace();
        let pt = |it: &mut std::str::SplitAsciiWhitespace| -> Option<(f32, f32)> {
            let x: f32 = it.next()?.parse().ok()?;
            let y: f32 = it.next()?.parse().ok()?;
            Some((ox + x * k, oy + y * k))
        };
        while let Some(c) = it.next() {
            match c {
                "M" => {
                    let Some((x, y)) = pt(&mut it) else { break };
                    pb.move_to(x, y);
                }
                "L" => {
                    let Some((x, y)) = pt(&mut it) else { break };
                    pb.line_to(x, y);
                }
                "Q" => {
                    let (Some(a), Some(b)) = (pt(&mut it), pt(&mut it)) else { break };
                    pb.quad_to(a.0, a.1, b.0, b.1);
                }
                "C" => {
                    let (Some(a), Some(b), Some(e)) = (pt(&mut it), pt(&mut it), pt(&mut it)) else { break };
                    pb.cubic_to(a.0, a.1, b.0, b.1, e.0, e.1);
                }
                "Z" => pb.close(),
                _ => break,
            }
        }
        if let Some(p) = pb.finish() {
            match stroke {
                Some(w) => self.stroke_path(p, w),
                None => self.path(p),
            };
        }
        self
    }

    /// Bounds of all shapes (logical).
    pub(crate) fn bounds(&self) -> Option<Rect> {
        let mut it = self.shapes.iter().filter(|(_, e)| !e).map(|(p, _)| p.bounds());
        let first = it.next()?;
        let b = it.fold(first, |a, b| {
            tiny_skia::Rect::from_ltrb(a.left().min(b.left()), a.top().min(b.top()), a.right().max(b.right()), a.bottom().max(b.bottom()))
                .unwrap_or(a)
        });
        Some(Rect::new(b.left(), b.top(), b.width(), b.height()))
    }
}

/// Rasterize `g` into per-device-pixel RGB coverage over the device rect (x0, y0, w, h).
pub(crate) fn coverage(g: &Glyph, scale: f32, x0: i32, y0: i32, w: i32, h: i32) -> Option<Vec<[u8; 3]>> {
    let sw = (w * 3) as u32;
    let mut acc = Mask::new(sw, h as u32)?;
    let mut tmp = Mask::new(sw, h as u32)?;
    let t = Transform::from_row(3.0 * scale, 0.0, 0.0, scale, -3.0 * x0 as f32, -(y0 as f32));
    for (p, erase) in &g.shapes {
        tmp.clear();
        tmp.fill_path(p, FillRule::Winding, true, t);
        let a = acc.data_mut();
        for (d, s) in a.iter_mut().zip(tmp.data()) {
            let (dv, sv) = (u32::from(*d), u32::from(*s));
            *d = if *erase { (dv * (255 - sv) / 255) as u8 } else { (dv + sv - dv * sv / 255) as u8 };
        }
    }
    // Five-tap subpixel filter (1 2 3 2 1) / 9 around each colour channel's subpixel.
    let m = acc.data();
    let sw = sw as i32;
    let mut out = vec![[0u8; 3]; (w * h) as usize];
    for y in 0..h {
        let row = &m[(y * sw) as usize..((y + 1) * sw) as usize];
        for x in 0..w {
            let mut px = [0u8; 3];
            for (c, v) in px.iter_mut().enumerate() {
                let s = x * 3 + c as i32;
                let mut sum = 0u32;
                for (k, wgt) in [(-2, 1u32), (-1, 2), (0, 3), (1, 2), (2, 1)] {
                    let i = s + k;
                    if i >= 0 && i < sw {
                        sum += u32::from(row[i as usize]) * wgt;
                    }
                }
                *v = (sum / 9).min(255) as u8;
            }
            out[(y * w + x) as usize] = px;
        }
    }
    Some(out)
}
