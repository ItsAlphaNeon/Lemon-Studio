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

/// The device px `[a, b)` of row `y` that `rrect_cover` covers fully (1.0): inside the edges, between the corners on
/// their rows. Those need no coverage worked out; the rest of the row does.
fn full_span(y: i32, (x0, y0, x1, y1): (f32, f32, f32, f32), r: f32) -> (i32, i32) {
    let py = y as f32 + 0.5;
    if py - y0 < 0.5 || y1 - py < 0.5 {
        return (0, 0);
    }
    let corner = r > 0.0 && (py < y0 + r || py > y1 - r);
    let (lo, hi) = if corner { ((x0 + r).max(x0 + 0.5), (x1 - r).min(x1 - 0.5)) } else { (x0 + 0.5, x1 - 0.5) };
    // Pixel x has its centre at x + 0.5.
    let (a, b) = ((lo - 0.5).ceil() as i32, (hi - 0.5).floor() as i32 + 1);
    if a < b { (a, b) } else { (0, 0) }
}

/// The backdrop enlarged over the whole canvas, in device px and dithered (`Canvas::backdrop_px`): the enlarging is
/// most of the see-through cost, and the backdrop only moves when the window does.
pub(crate) struct BackdropPx {
    src: Rc<Vec<u32>>,
    map: ((f32, f32), (f32, f32)),
    size: (u32, u32),
    grain: i64,
    px: Vec<PremultipliedColorU8>,
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

/// Rows `ys`, columns `xs.0..xs.1` of a small picture (`px`, `w` x `h`) enlarged by `map` (offset, factor, as
/// `Enlarge` takes them) and dithered with `grain`, into `out`, which holds device px from (`o.0`, `o.1`), `o.2` a row.
/// The grain is laid from device px `anchor` (the backdrop's grain moves with it).
#[allow(clippy::too_many_arguments)]
fn enlarge_rows(out: &mut [PremultipliedColorU8], o: (i32, i32, usize), (px, w, h): (&[u32], usize, usize), map: ((f32, f32), (f32, f32)), grain: i64, anchor: (i32, i32), xs: (i32, i32), ys: impl Iterator<Item = i32>) {
    let mut e = Enlarge::new(px, w, h, map.0, map.1, xs.0, xs.1);
    let mut row = Vec::with_capacity((xs.1 - xs.0) as usize);
    for y in ys {
        e.row(y, &mut row);
        let at = (y - o.1) as usize * o.2 + (xs.0 - o.0) as usize;
        let line = &mut out[at..at + (xs.1 - xs.0) as usize];
        for ((p, c), x) in line.iter_mut().zip(&row).zip(xs.0..) {
            let n = noise(x - anchor.0, y - anchor.1);
            *p = PremultipliedColorU8::from_rgba(dither(c[0], n, grain), dither(c[1], n, grain), dither(c[2], n, grain), 255).expect("opaque");
        }
    }
}

/// Lays `src` (device px from (`o.0`, `o.1`), `o.2` a row) over device rect `a` of `pixels` (`pw` a row), masked by the
/// rounded rect `rr` (corner radius `rad`): copied where it covers fully, blended by coverage at the edges.
fn lay_rrect(pixels: &mut [PremultipliedColorU8], pw: usize, (x0, y0, x1, y1): (i32, i32, i32, i32), src: &[PremultipliedColorU8], o: (i32, i32, usize), rr: (f32, f32, f32, f32), rad: f32) {
    for y in y0..y1 {
        let (a, b) = full_span(y, rr, rad);
        let (a, b) = (a.clamp(x0, x1), b.clamp(x0, x1));
        let (base, sbase) = (y as usize * pw, (y - o.1) as usize * o.2);
        let at = |x: i32| sbase + (x - o.0) as usize;
        if a < b {
            pixels[base + a as usize..base + b as usize].copy_from_slice(&src[at(a)..at(b)]);
        }
        let py = y as f32 + 0.5;
        for x in (x0..a).chain(b.max(a)..x1) {
            let cov = rrect_cover(x as f32 + 0.5, py, rr, rad);
            if cov > 0.0 {
                let (p, n) = (&mut pixels[base + x as usize], src[at(x)]);
                *p = blend(*p, [n.red(), n.green(), n.blue()], cov);
            }
        }
    }
}

/// How many frosted panels and shadows are kept (`Canvas::kept_frost`, `kept_shadow`): the few on screen at once.
const KEEP: usize = 6;

/// Device px rects: (x0, y0, x1, y1), whole and in floats.
type Px = (i32, i32, i32, i32);
type PxF = (f32, f32, f32, f32);

/// A frosted panel as last drawn (`Canvas::frost`): drawn the same way over the same picture, it comes out the same.
pub(crate) struct KeptFrost {
    /// Device rect, panel rect and radius, tuning, grain, shrink factor, source origin.
    key: (Px, PxF, f32, Frost, i64, i32, (i32, i32)),
    /// What it read: the canvas around it, shrunk.
    src: Vec<u32>,
    /// What it drew over its device rect (cut to the canvas): (x0, y0, w).
    at: (i32, i32, usize),
    px: Vec<PremultipliedColorU8>,
}

/// A shadow's strength (0..=255 of its colour's alpha) over its device rect, as last worked out (`Canvas::shadow`).
pub(crate) struct KeptShadow {
    /// Device rect, panel rect, radius, blur, offset.
    key: (Px, PxF, f32, f32, f32),
    w: usize,
    mask: Vec<u8>,
}

/// The kept `T` whose key `is` it, moved to the front; `make` makes one in its place (the oldest beyond `KEEP` go).
fn kept<T>(list: &mut Vec<T>, is: impl Fn(&T) -> bool, make: impl FnOnce() -> T) -> &T {
    match list.iter().position(is) {
        Some(i) => {
            let t = list.remove(i);
            list.insert(0, t);
        }
        None => {
            list.insert(0, make());
            list.truncate(KEEP);
        }
    }
    &list[0]
}

impl Canvas {
    /// The picture see-through clears show (`clear` with a colour that is not opaque); `None` = none.
    pub fn set_backdrop(&mut self, b: Option<Backdrop>) {
        self.backdrop = b;
    }

    pub fn backdrop(&self) -> Option<&Backdrop> {
        self.backdrop.as_ref()
    }

    /// Draws the backdrop over `r` (clipped; outside the cover rect, which will be painted over anyway), with `veil` laid
    /// over it in the same pass.
    pub fn fill_backdrop(&mut self, r: Rect, veil: Color) {
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
                            self.backdrop_in(&b, p, veil);
                        }
                    }
                }
                _ => self.backdrop_in(&b, a, veil),
            }
        }
    }

    fn backdrop_in(&mut self, b: &Backdrop, (x0, y0, x1, y1): (i32, i32, i32, i32), veil: Color) {
        self.mark(x0, y0, x1, y1);
        self.make_backdrop_px(b);
        let Some(cache) = &self.backdrop_px else { return };
        let pw = self.pixmap.width() as usize;
        let pixels = self.pixmap.pixels_mut();
        // The veil, premultiplied, and what is left of the backdrop under it (x 255).
        let a = u32::from(veil.a);
        let v = [u32::from(veil.r) * a, u32::from(veil.g) * a, u32::from(veil.b) * a];
        let q = |p: u8, v: u32| ((u32::from(p) * (255 - a) + v + 127) / 255) as u8;
        for y in y0 as usize..y1 as usize {
            let at = y * pw + x0 as usize..y * pw + x1 as usize;
            if a == 0 {
                pixels[at.clone()].copy_from_slice(&cache.px[at]);
                continue;
            }
            for (o, p) in pixels[at.clone()].iter_mut().zip(&cache.px[at]) {
                *o = PremultipliedColorU8::from_rgba(q(p.red(), v[0]), q(p.green(), v[1]), q(p.blue(), v[2]), 255).expect("opaque");
            }
        }
    }

    /// Makes `backdrop_px` for `b` unless it is made for it already: `b` enlarged over the canvas, with its grain. When
    /// only its place moved by whole px (the window dragged), what was made is moved and only the uncovered strips made.
    fn make_backdrop_px(&mut self, b: &Backdrop) {
        let map = self.backdrop_map(b);
        let size = (self.pixmap.width(), self.pixmap.height());
        let grain = backdrop_grain();
        let (pw, ph) = (size.0 as i32, size.1 as i32);
        let (src, anchor) = ((&b.px[..], b.w, b.h), (map.0 .0.round() as i32, map.0 .1.round() as i32));
        let old = self.backdrop_px.take().filter(|c| c.size == size);
        let same = |c: &BackdropPx| Rc::ptr_eq(&c.src, &b.px) && c.map.1 == map.1 && c.grain == grain;
        let moved = old.as_ref().filter(|c| same(c)).map(|c| (map.0 .0 - c.map.0 .0, map.0 .1 - c.map.0 .1));
        let px = match (old, moved) {
            (Some(c), Some(_)) if c.map == map => c.px,
            (Some(c), Some((dx, dy))) if dx.fract() == 0.0 && dy.fract() == 0.0 && dx.abs() < pw as f32 && dy.abs() < ph as f32 => {
                let (dx, dy) = (dx as i32, dy as i32);
                let mut px = c.px;
                // What stays on the canvas, moved in place (rows taken before they are written over): x from
                // max(0, dx) to min(pw, pw + dx).
                let (x0, x1) = (dx.max(0), (pw + dx).min(pw));
                let rows = dy.max(0)..(ph + dy).min(ph);
                let rows: Box<dyn Iterator<Item = i32>> = if dy > 0 { Box::new(rows.rev()) } else { Box::new(rows) };
                for y in rows {
                    let (to, from) = ((y * pw) as usize, ((y - dy) * pw) as usize);
                    px.copy_within(from + (x0 - dx) as usize..from + (x1 - dx) as usize, to + x0 as usize);
                }
                let (y0, y1) = (dy.max(0), (ph + dy).min(ph));
                enlarge_rows(&mut px, (0, 0, pw as usize), src, map, grain, anchor, (0, pw), (0..y0).chain(y1..ph));
                for xs in [(0, x0), (x1, pw)] {
                    if xs.0 < xs.1 {
                        enlarge_rows(&mut px, (0, 0, pw as usize), src, map, grain, anchor, xs, y0..y1);
                    }
                }
                px
            }
            (old, _) => {
                // The old one's memory, when it is the same size.
                let mut px = old.map(|c| c.px).unwrap_or_default();
                px.resize(size.0 as usize * size.1 as usize, PremultipliedColorU8::TRANSPARENT);
                enlarge_rows(&mut px, (0, 0, pw as usize), src, map, grain, anchor, (0, pw), 0..ph);
                px
            }
        };
        self.backdrop_px = Some(BackdropPx { src: b.px.clone(), map, size, grain, px });
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
        let rad = radius * k;
        self.make_backdrop_px(&b);
        let pw = self.pixmap.width() as usize;
        for i in 0..self.clip_count() {
            let c = self.clip_at(i);
            let (x0, y0, x1, y1) = (c.0.max(target.0), c.1.max(target.1), c.2.min(target.2), c.3.min(target.3));
            if x0 >= x1 || y0 >= y1 {
                continue;
            }
            self.mark(x0, y0, x1, y1);
            let Some(cache) = &self.backdrop_px else { return };
            lay_rrect(self.pixmap.pixels_mut(), pw, (x0, y0, x1, y1), &cache.px, (0, 0, pw), rr, rad);
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
        let (pw, ph) = (self.pixmap.width() as i32, self.pixmap.height() as i32);
        // Its strength over the device rect (cut to the canvas), worked out once for this shape and place.
        let t = (target.0.max(0), target.1.max(0), target.2.min(pw), target.3.min(ph));
        if t.0 >= t.2 || t.1 >= t.3 {
            return;
        }
        let key = (t, panel, rad, b, off);
        let clips: Vec<_> = (0..self.clip_count()).map(|i| self.clip_at(i)).collect();
        let kept = kept(&mut self.kept_shadow, |k| k.key == key, || {
            let w = (t.2 - t.0) as usize;
            let mut mask = vec![0u8; w * (t.3 - t.1) as usize];
            for y in t.1..t.3 {
                let py = y as f32 + 0.5;
                let qy = (py - cy).abs() - hy;
                // Under the panel it darkens nothing: only the band around it is worked out.
                let (s0, s1) = full_span(y, panel, rad);
                let (s0, s1) = (s0.clamp(t.0, t.2), s1.clamp(t.0, t.2));
                let row = &mut mask[(y - t.1) as usize * w..(y - t.1 + 1) as usize * w];
                for x in (t.0..s0).chain(s1.max(s0)..t.2) {
                    let px = x as f32 + 0.5;
                    let outside = 1.0 - rrect_cover(px, py, panel, rad);
                    if outside <= 0.0 {
                        continue;
                    }
                    let qx = (px - cx).abs() - hx;
                    let d = (qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - rad).max(0.0);
                    let tt = (1.0 - d / b).clamp(0.0, 1.0);
                    row[(x - t.0) as usize] = (tt * tt * outside * 255.0).round() as u8;
                }
            }
            KeptShadow { key, w, mask }
        });
        let (w, mask) = (kept.w, &kept.mask);
        // 0..=255 strength -> 0..=65536 weights, in 16.16 fixed point.
        let weight = |m: u8| (a * f32::from(m) / 255.0 * 65536.0).round() as u32;
        let lut: Vec<u32> = (0..=255u8).map(weight).collect();
        let pixels = self.pixmap.pixels_mut();
        for cl in clips {
            let (x0, y0, x1, y1) = (cl.0.max(t.0), cl.1.max(t.1), cl.2.min(t.2), cl.3.min(t.3));
            if x0 >= x1 || y0 >= y1 {
                continue;
            }
            self.drawn = Some(match self.drawn {
                Some((p, q, r, u)) => (p.min(x0), q.min(y0), r.max(x1), u.max(y1)),
                None => (x0, y0, x1, y1),
            });
            for y in y0..y1 {
                let mrow = &mask[(y - t.1) as usize * w..];
                let prow = &mut pixels[y as usize * pw as usize..];
                for x in x0..x1 {
                    let m = mrow[(x - t.0) as usize];
                    if m == 0 {
                        continue;
                    }
                    let wt = lut[m as usize];
                    let p = &mut prow[x as usize];
                    let q = |o: u8, n: u8| ((u32::from(o) * (65536 - wt) + u32::from(n) * wt + 32768) >> 16) as u8;
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
        let rr = (r.x * k, r.y * k, r.right() * k, r.bottom() * k);
        let (rad, grain) = (radius * k, grain_of(f.noise));
        // What it draws over its device rect (cut to the canvas): made again only when what is behind it changed.
        let t = (target.0.max(0), target.1.max(0), target.2.min(cw), target.3.min(ch));
        if t.0 >= t.2 || t.1 >= t.3 {
            return;
        }
        let key = (t, rr, rad, *f, grain, d, (sx0, sy0));
        let clips: Vec<_> = (0..self.clip_count()).map(|i| self.clip_at(i)).collect();
        let kept = kept(&mut self.kept_frost, |k| k.key == key && k.src == small, || {
            let (src, mut small) = (small.clone(), small.clone());
            // Three box passes of radius r each approach a Gaussian of sigma ~ r (in small px): sigma = blur / 2.
            let rb = ((b / 2.0 / d as f32).round() as usize).max(1);
            self::blur(&mut small, sw, sh, rb, 3);
            for v in &mut small {
                *v = frost_pixel(*v, f);
            }
            let at = (t.0, t.1, (t.2 - t.0) as usize);
            let mut px = vec![PremultipliedColorU8::TRANSPARENT; at.2 * (t.3 - t.1) as usize];
            // The small picture's px `i` covers device px sx0 + i * d .. + d.
            let map = ((sx0 as f32, sy0 as f32), (1.0 / d as f32, 1.0 / d as f32));
            enlarge_rows(&mut px, at, (&small, sw, sh), map, grain, (0, 0), (t.0, t.2), t.1..t.3);
            KeptFrost { key, src, at, px }
        });
        let (px, at) = (&kept.px, kept.at);
        let pixels = self.pixmap.pixels_mut();
        let mut drawn = self.drawn;
        for c in clips {
            let a = (c.0.max(t.0), c.1.max(t.1), c.2.min(t.2), c.3.min(t.3));
            if a.0 >= a.2 || a.1 >= a.3 {
                continue;
            }
            drawn = Some(match drawn {
                Some((p, q, r, u)) => (p.min(a.0), q.min(a.1), r.max(a.2), u.max(a.3)),
                None => a,
            });
            lay_rrect(pixels, pw, a, px, at, rr, rad);
        }
        self.drawn = drawn;
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

    /// What `full_span` leaves out of the coverage sums is covered fully, and it leaves out the most of each row.
    #[test]
    fn full_spans_are_fully_covered() {
        for (rr, rad) in [((0.0, 0.0, 100.0, 50.0), 8.0), ((10.3, 4.7, 61.9, 40.2), 12.0), ((2.5, 2.5, 30.0, 9.0), 0.0), ((0.0, 0.0, 20.0, 20.0), 10.0)] {
            for y in -2..60 {
                let (a, b) = full_span(y, rr, rad);
                for x in a..b {
                    assert_eq!(rrect_cover(x as f32 + 0.5, y as f32 + 0.5, rr, rad), 1.0, "{rr:?} r {rad} at {x}, {y}");
                }
                if y as f32 > rr.1 + rad && (y as f32) + 1.0 < rr.3 - rad {
                    assert!(b - a >= (rr.2 - rr.0) as i32 - 2, "{rr:?} row {y}: {a}..{b}");
                }
            }
        }
    }

    /// The backdrop made once for the canvas follows the backdrop as it moves, and a window's rounded backdrop
    /// matches a fresh canvas's.
    #[test]
    fn the_kept_backdrop_follows_moves() {
        let px = Rc::new((0..64u32).map(|i| i.wrapping_mul(0x9e37_79b9) & 0xffffff).collect::<Vec<_>>());
        let paint = |c: &mut Canvas, x: f32, y: f32| {
            c.set_backdrop(Some(Backdrop { w: 8, h: 8, px: px.clone(), at: Rect::new(x, y, 80.0, 40.0) }));
            c.begin_frame(None);
            c.clear(os_theme::Color::rgb(0).with_alpha(0));
            c.backdrop_rrect(Rect::new(10.5, 6.0, 40.0, 20.0), 6.0, os_theme::Color::rgb(0));
        };
        let Ok(mut a) = Canvas::new(80, 40, 1.5) else { return };
        paint(&mut a, 0.0, 0.0);
        // A move by a fraction of a device px (made again), then by whole ones each way (moved).
        for (x, y) in [(-7.0, 0.0), (-11.0, 4.0), (6.0, -10.0), (6.0, 2.0)] {
            paint(&mut a, x, y);
            let mut b = Canvas::new(80, 40, 1.5).unwrap();
            paint(&mut b, x, y);
            assert!(a.pixmap.pixels() == b.pixmap.pixels(), "moved to {x}, {y}: the same as made afresh");
        }
    }

    /// Times of the see-through drawing (run by hand: `cargo test --release -p os-ui see_through_times -- --ignored
    /// --nocapture`), at 150 % over a 1920 x 1080 frame.
    #[test]
    #[ignore]
    fn see_through_times() {
        let mut c = Canvas::new(2880, 1620, 1.5).unwrap();
        let px: Vec<u32> = (0..240 * 135u32).map(|i| i.wrapping_mul(0x9e37_79b9) & 0xffffff).collect();
        c.set_backdrop(Some(Backdrop { w: 240, h: 135, px: Rc::new(px), at: Rect::new(0.0, 0.0, 1920.0, 1080.0) }));
        c.begin_frame(None);
        let f = os_theme::etched::panel_frost(true);
        let time = |what: &str, c: &mut Canvas, n: usize, g: &dyn Fn(&mut Canvas)| {
            let t0 = std::time::Instant::now();
            for _ in 0..n {
                g(c);
            }
            println!("{what:>22}: {:6.2} ms", t0.elapsed().as_secs_f64() * 1000.0 / n as f64);
        };
        time("clear (backdrop)", &mut c, 10, &|c| c.clear(os_theme::Color::rgb(0).with_alpha(40)));
        time("veil over all", &mut c, 10, &|c| c.fill_rect(Rect::new(0.0, 0.0, 1920.0, 1080.0), os_theme::Color::rgb(0x202020).with_alpha(160)));
        time("opaque over all", &mut c, 10, &|c| c.fill_rect(Rect::new(0.0, 0.0, 1920.0, 1080.0), os_theme::Color::rgb(0x202020)));
        time("window backdrop", &mut c, 10, &|c| c.backdrop_rrect(Rect::new(100.0, 100.0, 900.0, 600.0), 8.0, os_theme::Color::rgb(0)));
        time("window shadow", &mut c, 10, &|c| c.shadow(Rect::new(100.0, 100.0, 900.0, 600.0), 8.0, 3.0, 1.0, os_theme::Color::rgb(0).with_alpha(0x80)));
        time("menu frost", &mut c, 10, &|c| c.frost(Rect::new(10.0, 30.0, 260.0, 600.0), 8.0, 20.0, &f));
        time("menu shadow", &mut c, 10, &|c| c.shadow(Rect::new(10.0, 30.0, 260.0, 600.0), 8.0, 18.0, 6.0, os_theme::Color::rgb(0).with_alpha(0x70)));
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
