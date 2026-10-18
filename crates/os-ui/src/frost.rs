// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! See-through drawing for the Etched skin: the backdrop (a small, already blurred and tinted picture enlarged under
//! the frame) and frosted panels (what is drawn behind a panel, blurred and tinted). Both work from small buffers:
//! the blur is done on a picture a few times smaller than the screen, then enlarged smoothly, with a little noise so
//! smooth gradients don't band.
// spec: ui/etched-design.md

use crate::{Canvas, Color, Rect};
use os_theme::etched::Frost;
use std::rc::Rc;
use tiny_skia::PremultipliedColorU8;

/// A picture drawn under the frame wherever it is see-through: `w` x `h` `0x00RRGGBB` pixels stretched over `at`
/// (logical px of the canvas; it may reach past the canvas, e.g. a whole monitor around the window).
#[derive(Clone, Debug, PartialEq)]
pub struct Backdrop {
    pub w: usize,
    pub h: usize,
    pub px: Rc<Vec<u32>>,
    pub at: Rect,
}

/// Noise for device pixel (x, y): a 64 x 64 tile of values in [0, 65536), the same every frame.
fn noise(x: i32, y: i32) -> u32 {
    static TILE: std::sync::OnceLock<Vec<u16>> = std::sync::OnceLock::new();
    let tile = TILE.get_or_init(|| {
        (0..64 * 64u32)
            .map(|i| {
                let mut h = (i % 64).wrapping_mul(0x8da6_b343) ^ (i / 64).wrapping_mul(0xd816_3841);
                h ^= h >> 13;
                h = h.wrapping_mul(0x5bd1_e995);
                h ^= h >> 15;
                (h & 0xffff) as u16
            })
            .collect()
    });
    u32::from(tile[((y & 63) * 64 + (x & 63)) as usize])
}

fn rgb(v: u32) -> [f32; 3] {
    [((v >> 16) & 0xff) as f32, ((v >> 8) & 0xff) as f32, (v & 0xff) as f32]
}

/// Box-blurs `px` (`w` x `h`, `0x00RRGGBB`) in place: `passes` passes of a (2r + 1)-wide box each way, which together
/// approach a Gaussian. Edges repeat the border pixels.
pub fn blur(px: &mut [u32], w: usize, h: usize, r: usize, passes: usize) {
    if w == 0 || h == 0 || r == 0 {
        return;
    }
    let mut buf: Vec<[f32; 3]> = px.iter().map(|&v| rgb(v)).collect();
    let mut line = vec![[0.0f32; 3]; w.max(h)];
    for _ in 0..passes {
        for y in 0..h {
            box_line(&mut buf, y * w, 1, w, r, &mut line);
        }
        for x in 0..w {
            box_line(&mut buf, x, w, h, r, &mut line);
        }
    }
    for (o, c) in px.iter_mut().zip(&buf) {
        let q = |v: f32| v.round().clamp(0.0, 255.0) as u32;
        *o = q(c[0]) << 16 | q(c[1]) << 8 | q(c[2]);
    }
}

/// One box pass along a line of `n` values from `start`, `step` apart.
fn box_line(buf: &mut [[f32; 3]], start: usize, step: usize, n: usize, r: usize, line: &mut [[f32; 3]]) {
    let at = |i: isize| -> [f32; 3] { buf[start + (i.clamp(0, n as isize - 1) as usize) * step] };
    let k = 1.0 / (2 * r + 1) as f32;
    let mut acc = [0.0f32; 3];
    for i in -(r as isize)..=(r as isize) {
        let v = at(i);
        for c in 0..3 {
            acc[c] += v[c];
        }
    }
    for (i, out) in line.iter_mut().enumerate().take(n) {
        *out = [acc[0] * k, acc[1] * k, acc[2] * k];
        let (add, sub) = (at(i as isize + r as isize + 1), at(i as isize - r as isize));
        for c in 0..3 {
            acc[c] += add[c] - sub[c];
        }
    }
    for (i, v) in line.iter().enumerate().take(n) {
        buf[start + i * step] = *v;
    }
}

/// A picture shrunk to `nw` x `nh` (each pixel the average of the source pixels it covers).
pub fn shrink(px: &[u32], w: usize, h: usize, nw: usize, nh: usize) -> Vec<u32> {
    let (nw, nh) = (nw.max(1), nh.max(1));
    let mut out = Vec::with_capacity(nw * nh);
    for y in 0..nh {
        let (y0, y1) = (y * h / nh, ((y + 1) * h / nh).max(y * h / nh + 1).min(h));
        for x in 0..nw {
            let (x0, x1) = (x * w / nw, ((x + 1) * w / nw).max(x * w / nw + 1).min(w));
            let mut acc = [0.0f32; 3];
            let mut n: f32 = 0.0;
            for sy in y0..y1 {
                for &v in &px[sy * w + x0..sy * w + x1] {
                    let c = rgb(v);
                    acc[0] += c[0];
                    acc[1] += c[1];
                    acc[2] += c[2];
                    n += 1.0;
                }
            }
            let q = |v: f32| (v / n.max(1.0)).round().clamp(0.0, 255.0) as u32;
            out.push(q(acc[0]) << 16 | q(acc[1]) << 8 | q(acc[2]));
        }
    }
    out
}

/// The frosted colour of a blurred pixel: its lightness pulled to the tint's by `luminosity` (its colour kept by
/// `chroma`), then the tint laid over it at `tint_opacity`.
pub fn frost_pixel(v: u32, f: &Frost) -> u32 {
    let c = rgb(v);
    let luma = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    let t = [f32::from(f.tint.r), f32::from(f.tint.g), f32::from(f.tint.b)];
    let (y, ty) = (luma(c), luma(t));
    let y2 = y + (ty - y) * f.luminosity;
    let mut out = [0.0f32; 3];
    let b = f.brightness.clamp(-1.0, 1.0);
    for i in 0..3 {
        let v = y2 + (c[i] - y) * f.chroma;
        let v = v + (t[i] - v) * f.tint_opacity;
        out[i] = if b >= 0.0 { v + (255.0 - v) * b } else { v * (1.0 + b) };
    }
    let q = |v: f32| v.round().clamp(0.0, 255.0) as u32;
    q(out[0]) << 16 | q(out[1]) << 8 | q(out[2])
}

/// Coverage (0..1) of device pixel centre (px, py) by the rounded rect (x0, y0, x1, y1) with corner radius `r`.
fn rrect_cover(px: f32, py: f32, (x0, y0, x1, y1): (f32, f32, f32, f32), r: f32) -> f32 {
    let edge = |d: f32| (d + 0.5).clamp(0.0, 1.0);
    let inside = edge(px - x0).min(edge(x1 - px)).min(edge(py - y0)).min(edge(y1 - py));
    if r <= 0.0 {
        return inside;
    }
    let cx = if px < x0 + r { x0 + r } else if px > x1 - r { x1 - r } else { return inside };
    let cy = if py < y0 + r { y0 + r } else if py > y1 - r { y1 - r } else { return inside };
    let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
    inside.min((r - d + 0.5).clamp(0.0, 1.0))
}

/// A small `w` x `h` picture enlarged smoothly (bilinear, in fixed point) onto device pixels `x0..x1` of each row:
/// device px (x, y) shows the picture at ((x + 0.5 - o.0) * f.0, (y + 0.5 - o.1) * f.1) (its pixel centres at +0.5).
struct Enlarge<'a> {
    px: &'a [u32],
    w: usize,
    h: usize,
    oy: f32,
    fy: f32,
    /// For each device px from `x0`: its left and right columns and the right one's weight (0..=256).
    cols: Vec<(u32, u32, u32)>,
    /// The columns the row uses.
    span: (usize, usize),
    /// The row blended between two picture rows, each channel x 256.
    line: Vec<[u32; 3]>,
}

impl<'a> Enlarge<'a> {
    fn new(px: &'a [u32], w: usize, h: usize, o: (f32, f32), f: (f32, f32), x0: i32, x1: i32) -> Enlarge<'a> {
        let cols: Vec<_> = (x0..x1)
            .map(|x| {
                let sx = ((x as f32 + 0.5 - o.0) * f.0 - 0.5).clamp(0.0, (w - 1) as f32);
                let i = sx.floor() as u32;
                (i, (i + 1).min(w as u32 - 1), ((sx - i as f32) * 256.0) as u32)
            })
            .collect();
        let span = cols.first().zip(cols.last()).map_or((0, 0), |(a, b)| (a.0.min(b.0) as usize, a.1.max(b.1) as usize + 1));
        Enlarge { px, w, h, oy: o.1, fy: f.1, cols, span, line: vec![[0; 3]; w] }
    }

    /// Row `y`, each channel x 65536, into `out` (one per device px).
    fn row(&mut self, y: i32, out: &mut Vec<[u32; 3]>) {
        let sy = ((y as f32 + 0.5 - self.oy) * self.fy - 0.5).clamp(0.0, (self.h - 1) as f32);
        let iy = sy.floor() as usize;
        let iy1 = (iy + 1).min(self.h - 1);
        let ty = ((sy - iy as f32) * 256.0) as u32;
        let (top, bot) = (&self.px[iy * self.w..], &self.px[iy1 * self.w..]);
        for i in self.span.0..self.span.1 {
            let (a, b) = (top[i], bot[i]);
            for (c, sh) in [16, 8, 0].into_iter().enumerate() {
                self.line[i][c] = ((a >> sh) & 0xff) * (256 - ty) + ((b >> sh) & 0xff) * ty;
            }
        }
        out.clear();
        out.extend(self.cols.iter().map(|&(l, r, t)| {
            let (l, r) = (self.line[l as usize], self.line[r as usize]);
            [l[0] * (256 - t) + r[0] * t, l[1] * (256 - t) + r[1] * t, l[2] * (256 - t) + r[2] * t]
        }));
    }
}

/// A channel x 65536 to 8 bits, the fraction spread as noise `n` (0..65536) so smooth gradients don't band; `grain`
/// adds more of the same noise (x 1/65536 per channel step).
fn dither(v: u32, n: u32, grain: i64) -> u8 {
    ((i64::from(v) + i64::from(n) + (i64::from(n) - 32768) * grain) >> 16).clamp(0, 255) as u8
}

/// The backdrop's grain in `dither` units (its tuning, in the current dark or light).
fn backdrop_grain() -> i64 {
    grain_of(os_theme::etched::backdrop_frost(os_theme::etched::appearance().dark).noise)
}

/// Grain (0..1 of a level step) in `dither` units.
fn grain_of(noise: f32) -> i64 {
    (noise * 510.0).round() as i64
}

/// `new` over `old` by coverage `cov` (0..1).
fn blend(old: PremultipliedColorU8, new: [u8; 3], cov: f32) -> PremultipliedColorU8 {
    if cov >= 1.0 {
        return PremultipliedColorU8::from_rgba(new[0], new[1], new[2], 255).expect("opaque");
    }
    let q = |o: u8, n: u8| (f32::from(o) + (f32::from(n) - f32::from(o)) * cov).round() as u8;
    PremultipliedColorU8::from_rgba(q(old.red(), new[0]), q(old.green(), new[1]), q(old.blue(), new[2]), 255).expect("opaque")
}

impl Canvas {
    /// The picture see-through clears show (`clear` with a colour that is not opaque); `None` = none.
    pub fn set_backdrop(&mut self, b: Option<Backdrop>) {
        self.backdrop = b;
    }

    pub fn backdrop(&self) -> Option<&Backdrop> {
        self.backdrop.as_ref()
    }

    /// Draws the backdrop over `r` (clipped; outside the cover rect, which will be painted over anyway).
    pub fn fill_backdrop(&mut self, r: Rect) {
        let Some(b) = self.backdrop.clone() else { return };
        if b.w == 0 || b.h == 0 || b.px.len() < b.w * b.h {
            return;
        }
        let (dx, dy, dw, dh) = self.device_rect(r);
        let target = (dx as i32, dy as i32, (dx + dw) as i32, (dy + dh) as i32);
        for i in 0..self.clip_count() {
            let c = self.clip_at(i);
            let a = (c.0.max(target.0), c.1.max(target.1), c.2.min(target.2), c.3.min(target.3));
            if a.0 >= a.2 || a.1 >= a.3 {
                continue;
            }
            match self.cover {
                Some(v) if v.0 < a.2 && v.2 > a.0 && v.1 < a.3 && v.3 > a.1 => {
                    let (mx0, mx1, my0, my1) = (v.0.max(a.0), v.2.min(a.2), v.1.max(a.1), v.3.min(a.3));
                    for p in [(a.0, a.1, a.2, my0), (a.0, my1, a.2, a.3), (a.0, my0, mx0, my1), (mx1, my0, a.2, my1)] {
                        if p.0 < p.2 && p.1 < p.3 {
                            self.backdrop_in(&b, p);
                        }
                    }
                }
                _ => self.backdrop_in(&b, a),
            }
        }
    }

    fn backdrop_in(&mut self, b: &Backdrop, (x0, y0, x1, y1): (i32, i32, i32, i32)) {
        self.mark(x0, y0, x1, y1);
        let (o, f) = self.backdrop_map(b);
        let mut e = Enlarge::new(&b.px, b.w, b.h, o, f, x0, x1);
        let mut row = Vec::with_capacity((x1 - x0) as usize);
        let pw = self.pixmap.width() as usize;
        let g = backdrop_grain();
        let pixels = self.pixmap.pixels_mut();
        for y in y0..y1 {
            e.row(y, &mut row);
            let line = &mut pixels[y as usize * pw + x0 as usize..y as usize * pw + x1 as usize];
            for ((p, c), x) in line.iter_mut().zip(&row).zip(x0..) {
                let n = noise(x, y);
                *p = PremultipliedColorU8::from_rgba(dither(c[0], n, g), dither(c[1], n, g), dither(c[2], n, g), 255).expect("opaque");
            }
        }
    }

    /// Device px -> backdrop px: (offset, factor), as `Enlarge` takes them.
    fn backdrop_map(&self, b: &Backdrop) -> ((f32, f32), (f32, f32)) {
        let k = self.scale;
        ((b.at.x * k, b.at.y * k), (b.w as f32 / (b.at.w * k).max(1.0), b.h as f32 / (b.at.h * k).max(1.0)))
    }

    /// The backdrop in the rounded rect `r` (a window of its own on the desktop, not over the app); `fallback` where
    /// there is none.
    pub fn backdrop_rrect(&mut self, r: Rect, radius: f32, fallback: Color) {
        let Some(b) = self.backdrop.clone().filter(|b| b.w > 0 && b.h > 0 && b.px.len() >= b.w * b.h) else {
            return self.fill_rrect(r, radius, fallback.with_alpha(255));
        };
        let k = self.scale;
        let (dx, dy, dw, dh) = self.device_rect(r);
        let target = (dx as i32, dy as i32, (dx + dw) as i32, (dy + dh) as i32);
        let rr = (r.x * k, r.y * k, r.right() * k, r.bottom() * k);
        let (o, f) = self.backdrop_map(&b);
        self.enlarge_in(target, rr, radius * k, (&b.px, b.w, b.h), o, f, backdrop_grain());
    }

    /// Draws a small picture enlarged (`Enlarge`) over device rect `target`, clipped and masked by the rounded rect
    /// `rr` (device px, corner radius `rad`), with `grain` (see `dither`).
    #[allow(clippy::too_many_arguments)]
    fn enlarge_in(&mut self, target: (i32, i32, i32, i32), rr: (f32, f32, f32, f32), rad: f32, (px, w, h): (&[u32], usize, usize), o: (f32, f32), f: (f32, f32), grain: i64) {
        let pw = self.pixmap.width() as usize;
        let mut row = Vec::new();
        for i in 0..self.clip_count() {
            let c = self.clip_at(i);
            let (x0, y0, x1, y1) = (c.0.max(target.0), c.1.max(target.1), c.2.min(target.2), c.3.min(target.3));
            if x0 >= x1 || y0 >= y1 {
                continue;
            }
            self.mark(x0, y0, x1, y1);
            let mut e = Enlarge::new(px, w, h, o, f, x0, x1);
            let pixels = self.pixmap.pixels_mut();
            for y in y0..y1 {
                e.row(y, &mut row);
                let py = y as f32 + 0.5;
                let line = &mut pixels[y as usize * pw + x0 as usize..y as usize * pw + x1 as usize];
                for ((p, c), x) in line.iter_mut().zip(&row).zip(x0..) {
                    let cov = rrect_cover(x as f32 + 0.5, py, rr, rad);
                    if cov <= 0.0 {
                        continue;
                    }
                    let n = noise(x, y);
                    *p = blend(*p, [dither(c[0], n, grain), dither(c[1], n, grain), dither(c[2], n, grain)], cov);
                }
            }
        }
    }

    /// Where frosted panels and their shadows were drawn since the last call (logical, each with its reach: what it
    /// reads or darkens around it). A repaint that touches one must repaint all of it, and what lies under it first.
    pub fn take_frosted(&mut self) -> Vec<Rect> {
        std::mem::take(&mut self.frosted)
    }

    /// A soft shadow around the rounded rect `r` (corner radius `radius`), `blur` logical px wide and `dy` lower, only
    /// outside it (the panel drawn over it may be see-through). What it darkens outside the panel is not repainted by
    /// itself when the panel goes: drawn by a frosted panel, whose reach covers it, or kept inside a frame's own rect.
    pub fn shadow(&mut self, r: Rect, radius: f32, blur: f32, dy: f32, c: Color) {
        let k = self.scale;
        let reach = Rect::new(r.x - blur, r.y - blur + dy.min(0.0), r.w + 2.0 * blur, r.h + 2.0 * blur + dy.abs());
        let (dx, dy0, dw, dh) = self.device_rect(reach);
        let target = (dx as i32, dy0 as i32, (dx + dw) as i32, (dy0 + dh) as i32);
        let panel = (r.x * k, r.y * k, r.right() * k, r.bottom() * k);
        let (rad, b, off) = (radius * k, (blur * k).max(1.0), dy * k);
        let (cx, cy) = ((panel.0 + panel.2) / 2.0, (panel.1 + panel.3) / 2.0 + off);
        let (hx, hy) = ((panel.2 - panel.0) / 2.0 - rad, (panel.3 - panel.1) / 2.0 - rad);
        let a = f32::from(c.a) / 255.0;
        let pw = self.pixmap.width() as usize;
        for i in 0..self.clip_count() {
            let cl = self.clip_at(i);
            let (x0, y0, x1, y1) = (cl.0.max(target.0), cl.1.max(target.1), cl.2.min(target.2), cl.3.min(target.3));
            if x0 >= x1 || y0 >= y1 {
                continue;
            }
            self.mark(x0, y0, x1, y1);
            let pixels = self.pixmap.pixels_mut();
            for y in y0..y1 {
                let py = y as f32 + 0.5;
                let qy = (py - cy).abs() - hy;
                for x in x0..x1 {
                    let px = x as f32 + 0.5;
                    let outside = 1.0 - rrect_cover(px, py, panel, rad);
                    if outside <= 0.0 {
                        continue;
                    }
                    let qx = (px - cx).abs() - hx;
                    let d = (qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - rad).max(0.0);
                    let t = (1.0 - d / b).clamp(0.0, 1.0);
                    let w = a * t * t * outside;
                    if w <= 0.0 {
                        continue;
                    }
                    let p = &mut pixels[y as usize * pw + x as usize];
                    let q = |o: u8, n: u8| (f32::from(o) + (f32::from(n) - f32::from(o)) * w).round().clamp(0.0, 255.0) as u8;
                    *p = PremultipliedColorU8::from_rgba(q(p.red(), c.r), q(p.green(), c.g), q(p.blue(), c.b), 255).expect("opaque");
                }
            }
        }
    }

    /// A frosted panel over `r` (corner radius `radius`): what is drawn behind it, blurred by `blur` logical px and
    /// tinted (`f`), with a little grain. Reads the canvas around `r` (out to `blur`), so what is behind must be drawn
    /// there first, in this frame.
    pub fn frost(&mut self, r: Rect, radius: f32, blur: f32, f: &Frost) {
        self.frosted.push(r.outset(blur));
        let k = self.scale;
        let (cw, ch) = (self.pixmap.width() as i32, self.pixmap.height() as i32);
        let (dx, dy, dw, dh) = self.device_rect(r);
        let target = (dx as i32, dy as i32, (dx + dw) as i32, (dy + dh) as i32);
        if !self.reaches(target.0, target.1, target.2, target.3) {
            return;
        }
        // The source: the panel and the blur's reach around it, shrunk by `d` so the blur is cheap.
        let b = (blur * k).max(1.0);
        let d = ((b / 6.0).round() as i32).clamp(2, 16);
        let pad = b.ceil() as i32;
        let (sx0, sy0) = ((target.0 - pad).max(0), (target.1 - pad).max(0));
        let (sx1, sy1) = ((target.2 + pad).min(cw), (target.3 + pad).min(ch));
        if sx0 >= sx1 || sy0 >= sy1 {
            return;
        }
        let (sw, sh) = (((sx1 - sx0 + d - 1) / d) as usize, ((sy1 - sy0 + d - 1) / d) as usize);
        let pw = cw as usize;
        let mut small = vec![0u32; sw * sh];
        {
            let pixels = self.pixmap.pixels();
            for (j, row) in small.chunks_mut(sw).enumerate() {
                let y0 = sy0 + j as i32 * d;
                let y1 = (y0 + d).min(sy1);
                for (i, o) in row.iter_mut().enumerate() {
                    let x0 = sx0 + i as i32 * d;
                    let x1 = (x0 + d).min(sx1);
                    let mut acc = [0u32; 3];
                    let mut n = 0;
                    for y in y0..y1 {
                        for p in &pixels[y as usize * pw + x0 as usize..y as usize * pw + x1 as usize] {
                            acc[0] += u32::from(p.red());
                            acc[1] += u32::from(p.green());
                            acc[2] += u32::from(p.blue());
                            n += 1;
                        }
                    }
                    let n = n.max(1);
                    *o = (acc[0] / n) << 16 | (acc[1] / n) << 8 | (acc[2] / n);
                }
            }
        }
        // Three box passes of radius r each approach a Gaussian of sigma ~ r (in small px): sigma = blur / 2.
        let rb = ((b / 2.0 / d as f32).round() as usize).max(1);
        self::blur(&mut small, sw, sh, rb, 3);
        for v in &mut small {
            *v = frost_pixel(*v, f);
        }
        let rr = (r.x * k, r.y * k, r.right() * k, r.bottom() * k);
        // The small picture's px `i` covers device px sx0 + i * d .. + d.
        let (o, fxy) = ((sx0 as f32, sy0 as f32), (1.0 / d as f32, 1.0 / d as f32));
        self.enlarge_in(target, rr, radius * k, (&small, sw, sh), o, fxy, grain_of(f.noise));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blur_keeps_flat_colour_and_spreads_an_edge() {
        let mut flat = vec![0x336699; 64];
        blur(&mut flat, 8, 8, 2, 3);
        assert!(flat.iter().all(|&v| v == 0x336699));
        let mut edge: Vec<u32> = (0..64).map(|i| if i % 8 < 4 { 0 } else { 0xffffff }).collect();
        blur(&mut edge, 8, 8, 1, 3);
        let mid = (edge[3] >> 16) & 0xff;
        assert!(mid > 40 && mid < 215, "{mid}");
    }

    #[test]
    fn frost_pulls_towards_the_tint() {
        let f = Frost { tint: os_theme::Color::rgb(0x202020), tint_opacity: 0.8, luminosity: 1.0, chroma: 0.7, noise: 0.0, brightness: 0.0 };
        let v = frost_pixel(0xffffff, &f);
        assert_eq!(v, 0x202020, "white: lightness to the tint's, then the tint");
        let blue = frost_pixel(0x2060ff, &f);
        let c = rgb(blue);
        assert!(c[2] > c[0] && c[0] < 0x40 as f32, "a hint of the colour stays: {blue:06x}");
    }

    #[test]
    fn rounded_corners_cover_partly() {
        let r = (0.0, 0.0, 100.0, 50.0);
        assert_eq!(rrect_cover(50.0, 25.0, r, 8.0), 1.0);
        assert_eq!(rrect_cover(0.5, 0.5, r, 8.0), 0.0);
        assert!(rrect_cover(0.5, 25.0, r, 8.0) > 0.9);
    }

    /// The backdrop fills a see-through clear; a frosted panel blurs what is behind it.
    #[test]
    fn backdrop_and_frost_draw() {
        let Ok(mut c) = Canvas::new(64, 32, 1.0) else { return };
        let px: Vec<u32> = (0..16).map(|i| if i % 4 < 2 { 0x000000 } else { 0xffffff }).collect();
        c.set_backdrop(Some(Backdrop { w: 4, h: 4, px: Rc::new(px), at: Rect::new(0.0, 0.0, 64.0, 32.0) }));
        c.begin_frame(None);
        c.clear(os_theme::Color::rgb(0).with_alpha(0));
        let at = |c: &Canvas, x: u32, y: u32| c.pixmap.pixel(x, y).map(|p| p.red()).unwrap_or(0);
        assert!(at(&c, 2, 16) < 20 && at(&c, 60, 16) > 235, "left dark, right light");
        let f = Frost { tint: os_theme::Color::rgb(0x808080), tint_opacity: 0.0, luminosity: 0.0, chroma: 1.0, noise: 0.0, brightness: 0.0 };
        c.frost(Rect::new(16.0, 4.0, 32.0, 24.0), 4.0, 12.0, &f);
        let mid = at(&c, 32, 16);
        assert!(mid > 60 && mid < 200, "blurred across the edge: {mid}");
    }
}
