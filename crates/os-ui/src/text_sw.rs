// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Text on Linux and macOS: outlines through swash (TrueType hinting, quarter-pixel pen positions, blending in a
//! gamma of 1.8) in the Barlow family (SIL OFL 1.1, see `fonts/Barlow/OFL.txt`), which stays crisp at the
//! fractional scales Wayland and X11 desktops run at.
//!
//! It keeps the layout model of the portable engine (Cuprum's cell height and baseline, `Font::width` as
//! an average character width), so the layouts measured for Cuprum hold: Barlow is sized so that its
//! capitals are as tall as Cuprum's and fitted to Cuprum's average width. Bold text uses the family's
//! semibold, strongly condensed text its narrow cut, and digits are tabular so a counting number keeps
//! its width. `OPENSTUDIO_TEXT=cuprum` brings back the portable engine with Cuprum.

use std::collections::HashMap;

use os_theme::Color;
use swash::scale::{Render, ScaleContext, Source};
use swash::zeno::{Format, Transform, Vector};
use swash::FontRef;
use tiny_skia::Pixmap;

use super::{ClipPx, Face, Font, CUPRUM};

static REGULAR: &[u8] = include_bytes!("../fonts/Barlow/BarlowSemiCondensed-Regular.ttf");
static SEMIBOLD: &[u8] = include_bytes!("../fonts/Barlow/BarlowSemiCondensed-SemiBold.ttf");
static NARROW: &[u8] = include_bytes!("../fonts/Barlow/BarlowCondensed-Medium.ttf");
/// The space text is blended in.
const GAMMA: f32 = 1.8;

/// Segoe UI's baseline (see the portable engine).
const SEGOE_UI: (f32, f32) = (2210.0 / 2048.0, 514.0 / 2048.0);
/// Synthetic italic slant.
const SLANT: f32 = 0.21;
/// Positions per pixel a glyph is rendered at.
const PHASES: f32 = 4.0;

struct Glyph {
    cov: Vec<u8>,
    w: usize,
    h: usize,
    left: i32,
    top: i32,
}

struct FaceData {
    font: FontRef<'static>,
    /// Em factor that makes this face's capitals as tall as Cuprum's.
    size_k: f32,
    /// Average advance (em, after `size_k`) of the fitting sample at the default axis values.
    avg: f32,
}

pub struct Sw {
    ctx: ScaleContext,
    regular: FaceData,
    bold: FaceData,
    narrow: FaceData,
    /// Cuprum's win ascent / descent and average character width (em): the layout's reference.
    ui: (f32, f32),
    ref_avg: f32,
    /// Cuprum's average advance over the fitting sample (em).
    ref_sample: f32,
    to_lin: [u16; 256],
    from_lin: Vec<u8>,
    glyphs: HashMap<(char, Font, u32, u8), Glyph>,
    advances: HashMap<(char, Font, u32), f32>,
}

/// Text the widths are fitted on: the mix of letters and digits the UI's labels have.
const SAMPLE: &str = "Channel rack Mixer Playlist Insert 12 Master Volume 100% Pattern 3 Tempo 140.000";

fn face(data: &'static [u8], cap_ref: f32) -> Result<FaceData, String> {
    let font = FontRef::from_index(data, 0).ok_or("not a font swash can read")?;
    let m = font.metrics(&[]);
    let upem = f32::from(m.units_per_em).max(1.0);
    let cap = if m.cap_height > 0.0 { m.cap_height / upem } else { 0.7 };
    let size_k = if cap_ref > 0.0 { cap_ref / cap } else { 1.0 };
    let gm = font.glyph_metrics(&[]);
    let cm = font.charmap();
    let total: f32 = SAMPLE.chars().map(|c| gm.advance_width(cm.map(c))).sum();
    let avg = total / upem / SAMPLE.chars().count() as f32 * size_k;
    Ok(FaceData { font, size_k, avg })
}

/// Faces an engine holds (regular, semibold, narrow): its fonts in the CPU panel's Misc reading.
const FACES: usize = 3;

impl Drop for Sw {
    fn drop(&mut self) {
        super::FONTS.fetch_sub(FACES, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Sw {
    pub fn new() -> Result<Sw, String> {
        let (asc, desc, ref_avg) = super::portable::os2(CUPRUM).ok_or("the UI font has no OS/2 metrics")?;
        let cuprum = face(CUPRUM, 0.0)?;
        let cm = cuprum.font.metrics(&[]);
        let cap_ref = cm.cap_height / f32::from(cm.units_per_em);
        let mut to_lin = [0u16; 256];
        for (i, v) in to_lin.iter_mut().enumerate() {
            *v = ((i as f32 / 255.0).powf(GAMMA) * 4095.0).round() as u16;
        }
        let from_lin = (0..4096).map(|i| ((i as f32 / 4095.0).powf(1.0 / GAMMA) * 255.0).round() as u8).collect();
        let sw = Sw {
            ctx: ScaleContext::new(),
            ref_sample: cuprum.avg,
            regular: face(REGULAR, cap_ref)?,
            bold: face(SEMIBOLD, cap_ref)?,
            narrow: face(NARROW, cap_ref)?,
            ui: (asc, desc),
            ref_avg,
            to_lin,
            from_lin,
            glyphs: HashMap::new(),
            advances: HashMap::new(),
        };
        super::FONTS.fetch_add(FACES, std::sync::atomic::Ordering::Relaxed);
        Ok(sw)
    }

    /// The em height in device pixels (rounded, as the layouts expect).
    fn px(f: Font, scale: f32) -> f32 {
        (f32::from(f.size) * scale).round().max(1.0)
    }

    fn vm(&self, f: Font) -> (f32, f32) {
        match f.face {
            Face::Ui => self.ui,
            Face::Light => SEGOE_UI,
        }
    }

    /// How a font request is set up: which face (0 regular, 1 bold, 2 narrow) and the horizontal factor
    /// left for the outline transform.
    fn setup(&self, f: Font, scale: f32) -> (u8, f32) {
        let mut which = u8::from(f.bold);
        let fd = self.face(which);
        // Horizontal factor wanted against the face's own width.
        let mut k = if f.width != 0 {
            let want = (f32::from(f.width) * scale).round().max(1.0);
            (want / (self.ref_avg * Self::px(f, scale))).clamp(0.3, 2.0) * self.ref_sample / fd.avg
        } else {
            self.ref_sample / fd.avg
        };
        // A strong squeeze thins the stems with the letters: the narrow cut is drawn for that width.
        if !f.bold && k < 0.9 {
            k *= fd.avg / self.narrow.avg;
            which = 2;
        }
        (which, k)
    }

    fn face(&self, which: u8) -> &FaceData {
        match which {
            1 => &self.bold,
            2 => &self.narrow,
            _ => &self.regular,
        }
    }

    /// Digits are tabular: each takes the widest digit's advance and sits in its middle, so a number that
    /// counts does not change its width. Returns (advance, offset of the glyph inside it).
    fn advance(&mut self, c: char, f: Font, scale: f32) -> (f32, f32) {
        let own = self.own_advance(c, f, scale);
        if !c.is_ascii_digit() {
            return (own, 0.0);
        }
        let widest = ('0'..='9').map(|d| self.own_advance(d, f, scale)).fold(0.0, f32::max);
        (widest, (widest - own) * 0.5)
    }

    fn own_advance(&mut self, c: char, f: Font, scale: f32) -> f32 {
        let key = (c, f, scale.to_bits());
        if let Some(a) = self.advances.get(&key) {
            return *a;
        }
        let (which, k) = self.setup(f, scale);
        let fd = self.face(which);
        let gm = fd.font.glyph_metrics(&[]).scale(Self::px(f, scale) * fd.size_k);
        let a = gm.advance_width(fd.font.charmap().map(c)) * k;
        self.advances.insert(key, a);
        a
    }

    fn glyph(&mut self, c: char, f: Font, scale: f32, phase: u8) -> &Glyph {
        let key = (c, f, scale.to_bits(), phase);
        if !self.glyphs.contains_key(&key) {
            let (which, k) = self.setup(f, scale);
            let fd = match which {
                1 => &self.bold,
                2 => &self.narrow,
                _ => &self.regular,
            };
            let px = Self::px(f, scale) * fd.size_k;
            // The light face (the song position digits) has no cut of its own here: the regular stands in.
            let mut scaler = self.ctx.builder(fd.font).size(px).hint(true).build();
            let mut r = Render::new(&[Source::Outline]);
            r.format(Format::Alpha);
            r.offset(Vector::new(f32::from(phase) / PHASES, 0.0));
            let slant = if f.italic { SLANT } else { 0.0 };
            if (k - 1.0).abs() > 1e-3 || f.italic {
                r.transform(Some(Transform::new(k, 0.0, slant, 1.0, 0.0, 0.0)));
            }
            let g = match r.render(&mut scaler, fd.font.charmap().map(c)) {
                Some(img) => Glyph {
                    w: img.placement.width as usize,
                    h: img.placement.height as usize,
                    left: img.placement.left,
                    top: img.placement.top,
                    cov: img.data,
                },
                None => Glyph { cov: Vec::new(), w: 0, h: 0, left: 0, top: 0 },
            };
            self.glyphs.insert(key, g);
        }
        &self.glyphs[&key]
    }

    pub fn metrics(&mut self, f: Font, scale: f32) -> (i32, i32) {
        let px = Self::px(f, scale);
        let (a, d) = self.vm(f);
        let (asc, desc) = ((a * px).round() as i32, (d * px).round() as i32);
        (asc + desc, asc)
    }

    pub fn extent(&mut self, f: Font, scale: f32, s: &str) -> (i32, i32) {
        let w: f32 = s.chars().map(|c| self.advance(c, f, scale).0).sum();
        (w.round() as i32, self.metrics(f, scale).0)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, pix: &mut Pixmap, clip: ClipPx, f: Font, scale: f32, x: i32, y: i32, c: Color, s: &str, vertical: bool) {
        let asc = self.metrics(f, scale).1;
        let pw = pix.width() as i32;
        let col = [c.r, c.g, c.b].map(|v| u32::from(self.to_lin[usize::from(v)]));
        let mut pen = 0.0f32;
        for chr in s.chars() {
            let (adv, inset) = self.advance(chr, f, scale);
            let q = ((pen + inset) * PHASES).round() as i32;
            let (whole, phase) = (q.div_euclid(PHASES as i32), q.rem_euclid(PHASES as i32) as u8);
            self.glyph(chr, f, scale, phase);
            let g = &self.glyphs[&(chr, f, scale.to_bits(), phase)];
            let (gx0, gy0) = (whole + g.left, asc - g.top);
            let data = pix.data_mut();
            for j in 0..g.h {
                for i in 0..g.w {
                    let a = u32::from(g.cov[j * g.w + i]) * u32::from(c.a) / 255;
                    if a == 0 {
                        continue;
                    }
                    let (lx, ly) = (gx0 + i as i32, gy0 + j as i32);
                    let (dx, dy) = if vertical { (x + ly, y - lx) } else { (x + lx, y + ly) };
                    if dx < clip.0 || dy < clip.1 || dx >= clip.2 || dy >= clip.3 {
                        continue;
                    }
                    let kk = ((dy * pw + dx) * 4) as usize;
                    for (n, sv) in col.into_iter().enumerate() {
                        let dv = u32::from(self.to_lin[usize::from(data[kk + n])]);
                        data[kk + n] = self.from_lin[((sv * a + dv * (255 - a)) / 255) as usize];
                    }
                }
            }
            pen += adv;
        }
    }
}
