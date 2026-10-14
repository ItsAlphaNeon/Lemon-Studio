// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Text: the bundled Cuprum face (SIL OFL 1.1, see `fonts/Cuprum/OFL.txt`).
//!
//! On Windows, text goes through GDI with ClearType: the destination pixels are copied into a DIB,
//! GDI draws the string over them (so ClearType blends against the real background), and the
//! result is copied back. On Linux and macOS, text is drawn from outlines in the Barlow family (`text_sw.rs`;
//! `OPENSTUDIO_TEXT=cuprum` for the portable engine). Elsewhere (and on Windows with `OPENSTUDIO_TEXT=portable`) the portable
//! engine draws grayscale text with fontdue on GDI's metrics model: the same cell height and
//! baseline, condensed widths by squeezing glyphs, synthetic bold and italic.
// spec: ui/main-window.md §10 (typography)

use std::sync::atomic::{AtomicUsize, Ordering};

use os_theme::Color;
use tiny_skia::Pixmap;

/// Font objects the text engines hold (GDI fonts, loaded faces): the CPU panel's Misc reading.
static FONTS: AtomicUsize = AtomicUsize::new(0);

/// How many font objects the text engines hold now (spec: ui/main-window.md §13, Misc).
pub fn font_count() -> usize {
    FONTS.load(Ordering::Relaxed)
}

/// The UI face, embedded so every machine renders the same glyphs.
pub static CUPRUM: &[u8] = include_bytes!("../fonts/Cuprum/Cuprum.ttf");

/// Which typeface a `Font` uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Face {
    /// Cuprum (bundled): all UI labels.
    Ui,
    /// The system's light UI face (Segoe UI Light on Windows): the song position display.
    Light,
}

impl Face {
    #[cfg(windows)]
    fn gdi_name(self) -> &'static str {
        match self {
            Face::Ui => "Cuprum",
            Face::Light => "Segoe UI",
        }
    }
}

/// A font request, in logical pixels (GDI `LOGFONT` terms).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Font {
    pub face: Face,
    /// Em height (GDI: negative `lfHeight`).
    pub size: u16,
    /// Average character width (GDI `lfWidth`); 0 = the face's natural proportions.
    pub width: u16,
    pub bold: bool,
    pub italic: bool,
    /// ClearType "natural" advances; `false` = GDI-compatible (hinted, whole-pixel) advances.
    pub natural: bool,
}

impl Font {
    /// Cuprum at `size` px with natural ClearType advances.
    pub const fn new(size: u16) -> Font {
        Font { face: Face::Ui, size, width: 0, bold: false, italic: false, natural: true }
    }
    /// Horizontally condensed: `width` is the average character width in px.
    pub const fn condensed(size: u16, width: u16) -> Font {
        Font { face: Face::Ui, size, width, bold: false, italic: false, natural: false }
    }
    /// The light display face (song position digits).
    pub const fn light(size: u16, width: u16) -> Font {
        Font { face: Face::Light, size, width, bold: false, italic: false, natural: false }
    }
    pub const fn natural(self) -> Font {
        Font { natural: true, ..self }
    }
    pub const fn bold(self) -> Font {
        Font { bold: true, ..self }
    }
    pub const fn italic(self) -> Font {
        Font { italic: true, ..self }
    }
    /// GDI-compatible advances.
    pub const fn compat(self) -> Font {
        Font { natural: false, ..self }
    }
}

/// Device-pixel clip: (x0, y0, x1, y1), end-exclusive.
pub type ClipPx = (i32, i32, i32, i32);

pub struct TextEngine {
    /// GDI (Windows), unless the portable engine was asked for.
    #[cfg(windows)]
    gdi: Option<gdi::Gdi>,
    portable: portable::Fd,
    /// Linux and macOS: the outline engine with the Barlow family (`text_sw.rs`), unless Cuprum was asked for.
    #[cfg(unix)]
    sw: Option<sw::Sw>,
}

impl TextEngine {
    pub fn new() -> Result<TextEngine, String> {
        #[cfg(windows)]
        let gdi = match std::env::var("OPENSTUDIO_TEXT").as_deref() {
            Ok("portable") => None,
            _ => Some(gdi::Gdi::new()?),
        };
        Ok(TextEngine {
            #[cfg(windows)]
            gdi,
            portable: portable::Fd::new()?,
            #[cfg(unix)]
            sw: match std::env::var("OPENSTUDIO_TEXT").as_deref() {
                Ok("cuprum") => None,
                _ => Some(sw::Sw::new()?),
            },
        })
    }

    /// (width, cell height) of `s` in device pixels.
    pub fn extent(&mut self, f: Font, scale: f32, s: &str) -> (i32, i32) {
        #[cfg(windows)]
        if let Some(g) = &mut self.gdi {
            return g.extent(f, scale, s);
        }
        #[cfg(unix)]
        if let Some(e) = &mut self.sw {
            return e.extent(f, scale, s);
        }
        self.portable.extent(f, scale, s)
    }

    /// Cell height and ascent (device pixels).
    pub fn metrics(&mut self, f: Font, scale: f32) -> (i32, i32) {
        #[cfg(windows)]
        if let Some(g) = &mut self.gdi {
            return g.metrics(f, scale);
        }
        #[cfg(unix)]
        if let Some(e) = &mut self.sw {
            return e.metrics(f, scale);
        }
        self.portable.metrics(f, scale)
    }

    /// Draw `s` with its cell's top-left at device (x, y). `vertical` rotates it 90° counter-clockwise
    /// about that point (the string then reads upward from y, glyph tops facing left at x).
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, pix: &mut Pixmap, clip: ClipPx, f: Font, scale: f32, x: i32, y: i32, c: Color, s: &str, vertical: bool) {
        if s.is_empty() || c.a == 0 {
            return;
        }
        #[cfg(windows)]
        if let Some(g) = &mut self.gdi {
            return g.draw(pix, clip, f, scale, x, y, c, s, vertical);
        }
        #[cfg(unix)]
        if let Some(e) = &mut self.sw {
            return e.draw(pix, clip, f, scale, x, y, c, s, vertical);
        }
        self.portable.draw(pix, clip, f, scale, x, y, c, s, vertical);
    }
}

#[cfg(windows)]
mod gdi {
    use std::collections::HashMap;
    use std::hash::{BuildHasherDefault, Hash, Hasher};
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::SIZE;
    use windows_sys::Win32::Graphics::Gdi::*;

    use super::{ClipPx, Face, Font, CUPRUM};
    use crate::cache::{Fx, Id, Lru};
    use os_theme::Color;
    use tiny_skia::Pixmap;

    /// A drawn string: the pixels it produced over the pixels it was drawn on. ClearType blends each pixel with the
    /// destination pixel under it, so the same string, font, colour and offset over the same background pixels gives
    /// the same result; a hit is checked field by field and pixel by pixel, so it is exact.
    struct Drawn {
        s: Box<str>,
        f: Font,
        scale: u32,
        vertical: bool,
        c: Color,
        /// Text origin relative to the region, and the region size (device px).
        at: (i32, i32, i32, i32),
        bg: Vec<u8>,
        out: Vec<u8>,
    }

    /// Register the embedded face once per process (private to the process).
    fn register_face() -> bool {
        static DONE: OnceLock<bool> = OnceLock::new();
        *DONE.get_or_init(|| {
            let mut n = 0u32;
            // SAFETY: CUPRUM is 'static; GDI copies the font data.
            let h = unsafe { AddFontMemResourceEx(CUPRUM.as_ptr().cast(), CUPRUM.len() as u32, std::ptr::null(), &mut n) };
            !h.is_null() && n > 0
        })
    }

    pub struct Gdi {
        dc: HDC,
        dib: HBITMAP,
        old_bmp: HGDIOBJ,
        bits: *mut u8,
        cap: (i32, i32),
        fonts: HashMap<(Font, u32, bool), HFONT>,
        names: [Vec<u16>; 2],
        metrics: HashMap<(Font, u32), (i32, i32)>,
        /// Text extents by (font, scale, string) hash: views measure the same labels every frame.
        extents: HashMap<u64, (i32, i32), BuildHasherDefault<Id>>,
        drawn: Lru<Drawn>,
        /// Background pixels of the region being drawn (reused buffer).
        bg: Vec<u8>,
    }

    impl Gdi {
        pub fn new() -> Result<Gdi, String> {
            if !register_face() {
                return Err("could not register the UI font".into());
            }
            // SAFETY: plain GDI object creation; failures are checked.
            let dc = unsafe { CreateCompatibleDC(std::ptr::null_mut()) };
            if dc.is_null() {
                return Err("CreateCompatibleDC failed".into());
            }
            unsafe {
                SetBkMode(dc, TRANSPARENT as i32);
                SetTextAlign(dc, TA_TOP | TA_LEFT);
            }
            let mut g = Gdi {
                dc,
                dib: std::ptr::null_mut(),
                old_bmp: std::ptr::null_mut(),
                bits: std::ptr::null_mut(),
                cap: (0, 0),
                fonts: HashMap::new(),
                names: [Face::Ui, Face::Light].map(|f| f.gdi_name().encode_utf16().chain(Some(0)).collect()),
                metrics: HashMap::new(),
                extents: HashMap::default(),
                drawn: Lru::new(32 << 20),
                bg: Vec::new(),
            };
            g.ensure(256, 64)?;
            Ok(g)
        }

        fn ensure(&mut self, w: i32, h: i32) -> Result<(), String> {
            if w <= self.cap.0 && h <= self.cap.1 {
                return Ok(());
            }
            let (w, h) = (w.max(self.cap.0).max(64), h.max(self.cap.1).max(32));
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            // SAFETY: bmi is a valid header; `bits` receives the DIB memory owned by the bitmap.
            let dib = unsafe { CreateDIBSection(self.dc, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0) };
            if dib.is_null() || bits.is_null() {
                return Err("CreateDIBSection failed".into());
            }
            unsafe {
                let old = SelectObject(self.dc, dib);
                if self.dib.is_null() {
                    self.old_bmp = old;
                } else {
                    DeleteObject(self.dib);
                }
            }
            self.dib = dib;
            self.bits = bits.cast();
            self.cap = (w, h);
            Ok(())
        }

        fn font(&mut self, f: Font, scale: f32, vertical: bool) -> HFONT {
            let key = (f, scale.to_bits(), vertical);
            if let Some(h) = self.fonts.get(&key) {
                return *h;
            }
            let px = |v: u16| (f32::from(v) * scale).round() as i32;
            let esc = if vertical { 900 } else { 0 };
            let quality = if f.natural { CLEARTYPE_NATURAL_QUALITY } else { u32::from(CLEARTYPE_QUALITY) };
            // SAFETY: the face name is NUL-terminated UTF-16.
            let h = unsafe {
                CreateFontW(
                    -px(f.size),
                    px(f.width),
                    esc,
                    esc,
                    match (f.face, f.bold) {
                        (_, true) => 700,
                        (Face::Light, false) => 300,
                        (Face::Ui, false) => 400,
                    },
                    u32::from(f.italic),
                    0,
                    0,
                    u32::from(DEFAULT_CHARSET),
                    u32::from(OUT_DEFAULT_PRECIS),
                    u32::from(CLIP_DEFAULT_PRECIS),
                    quality,
                    u32::from(DEFAULT_PITCH),
                    self.names[f.face as usize].as_ptr(),
                )
            };
            if !h.is_null() {
                super::FONTS.fetch_add(1, super::Ordering::Relaxed);
            }
            self.fonts.insert(key, h);
            h
        }

        pub fn metrics(&mut self, f: Font, scale: f32) -> (i32, i32) {
            let key = (f, scale.to_bits());
            if let Some(m) = self.metrics.get(&key) {
                return *m;
            }
            let font = self.font(f, scale, false);
            let mut tm: TEXTMETRICW = unsafe { std::mem::zeroed() };
            // SAFETY: dc and font are valid; tm is a valid out-pointer.
            unsafe {
                SelectObject(self.dc, font);
                GetTextMetricsW(self.dc, &mut tm);
            }
            let m = (tm.tmHeight, tm.tmAscent);
            self.metrics.insert(key, m);
            m
        }

        pub fn extent(&mut self, f: Font, scale: f32, s: &str) -> (i32, i32) {
            let mut h = Fx::default();
            f.hash(&mut h);
            h.write_u32(scale.to_bits());
            h.write(s.as_bytes());
            let key = h.finish();
            if let Some(e) = self.extents.get(&key) {
                return *e;
            }
            let font = self.font(f, scale, false);
            let w: Vec<u16> = s.encode_utf16().collect();
            let mut sz = SIZE { cx: 0, cy: 0 };
            // SAFETY: valid dc, font and buffer.
            unsafe {
                SelectObject(self.dc, font);
                GetTextExtentPoint32W(self.dc, w.as_ptr(), w.len() as i32, &mut sz);
            }
            if self.extents.len() > 100_000 {
                self.extents.clear();
            }
            self.extents.insert(key, (sz.cx, sz.cy));
            (sz.cx, sz.cy)
        }

        #[allow(clippy::too_many_arguments)]
        pub fn draw(&mut self, pix: &mut Pixmap, clip: ClipPx, f: Font, scale: f32, x: i32, y: i32, c: Color, s: &str, vertical: bool) {
            let (tw, th) = self.extent(f, scale, s);
            // Glyphs may overhang their cell (italics, 'j', ClearType fringes).
            let pad = (f32::from(f.size) * scale * 0.35).ceil() as i32 + 2;
            let (rx0, ry0, rx1, ry1) =
                if vertical { (x - pad, y - tw - pad, x + th + pad, y + pad) } else { (x - pad, y - pad, x + tw + pad, y + th + pad) };
            let (pw, ph) = (pix.width() as i32, pix.height() as i32);
            let (x0, y0) = (rx0.max(clip.0).max(0), ry0.max(clip.1).max(0));
            let (x1, y1) = (rx1.min(clip.2).min(pw), ry1.min(clip.3).min(ph));
            if x0 >= x1 || y0 >= y1 {
                return;
            }
            let (w, h) = (x1 - x0, y1 - y0);
            let data = pix.data_mut();
            let pstride = pw as usize * 4;
            let row_bytes = w as usize * 4;
            // The pixels under the region, then a lookup of the same string drawn over the same pixels.
            let mut bg = std::mem::take(&mut self.bg);
            bg.clear();
            for row in 0..h as usize {
                bg.extend_from_slice(&data[(y0 as usize + row) * pstride + x0 as usize * 4..][..row_bytes]);
            }
            let at = (x - x0, y - y0, w, h);
            let mut hs = Fx::default();
            f.hash(&mut hs);
            hs.write_u32(scale.to_bits());
            hs.write_u8(u8::from(vertical));
            hs.write_u32(u32::from_le_bytes([c.r, c.g, c.b, c.a]));
            for v in [at.0, at.1, at.2, at.3] {
                hs.write_i32(v);
            }
            hs.write(s.as_bytes());
            hs.write(&bg);
            let key = hs.finish();
            if let Some(d) = self.drawn.get(key)
                && d.at == at
                && d.f == f
                && d.scale == scale.to_bits()
                && d.vertical == vertical
                && d.c == c
                && *d.s == *s
                && d.bg == bg
            {
                for row in 0..h as usize {
                    data[(y0 as usize + row) * pstride + x0 as usize * 4..][..row_bytes].copy_from_slice(&d.out[row * row_bytes..][..row_bytes]);
                }
                self.bg = bg;
                return;
            }
            if self.ensure(w, h).is_err() {
                self.bg = bg;
                return;
            }
            let stride = self.cap.0 as usize * 4;
            // SAFETY: the DIB holds cap.0 × cap.1 BGRA pixels and w ≤ cap.0, h ≤ cap.1.
            let dib = unsafe { std::slice::from_raw_parts_mut(self.bits, stride * self.cap.1 as usize) };
            for row in 0..h as usize {
                let src = &data[(y0 as usize + row) * pstride + x0 as usize * 4..][..w as usize * 4];
                let dst = &mut dib[row * stride..][..w as usize * 4];
                for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                    d[0] = s[2];
                    d[1] = s[1];
                    d[2] = s[0];
                    d[3] = 0;
                }
            }
            let font = self.font(f, scale, vertical);
            let ws: Vec<u16> = s.encode_utf16().collect();
            // SAFETY: valid dc/font; the string buffer outlives the call.
            unsafe {
                SelectObject(self.dc, font);
                SetTextColor(self.dc, u32::from(c.r) | (u32::from(c.g) << 8) | (u32::from(c.b) << 16));
                ExtTextOutW(self.dc, x - x0, y - y0, 0, std::ptr::null(), ws.as_ptr(), ws.len() as u32, std::ptr::null());
                GdiFlush();
            }
            let a = u32::from(c.a);
            for row in 0..h as usize {
                let dst = &mut data[(y0 as usize + row) * pstride + x0 as usize * 4..][..w as usize * 4];
                let src = &dib[row * stride..][..w as usize * 4];
                for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                    if a == 255 {
                        d[0] = s[2];
                        d[1] = s[1];
                        d[2] = s[0];
                    } else {
                        let mix = |n: u8, o: u8| ((u32::from(n) * a + u32::from(o) * (255 - a)) / 255) as u8;
                        d[0] = mix(s[2], d[0]);
                        d[1] = mix(s[1], d[1]);
                        d[2] = mix(s[0], d[2]);
                    }
                    d[3] = 255;
                }
            }
            let mut out = Vec::with_capacity(bg.len());
            for row in 0..h as usize {
                out.extend_from_slice(&data[(y0 as usize + row) * pstride + x0 as usize * 4..][..row_bytes]);
            }
            let bytes = bg.len() * 2 + s.len() + 96;
            let d = Drawn { s: s.into(), f, scale: scale.to_bits(), vertical, c, at, bg: bg.clone(), out };
            self.drawn.insert(key, d, bytes);
            self.bg = bg;
        }
    }

    impl Drop for Gdi {
        fn drop(&mut self) {
            // SAFETY: objects were created by this struct and are no longer selected when deleted.
            unsafe {
                if !self.old_bmp.is_null() {
                    SelectObject(self.dc, self.old_bmp);
                }
                for h in self.fonts.values() {
                    DeleteObject(*h);
                }
                let n = self.fonts.values().filter(|h| !h.is_null()).count();
                super::FONTS.fetch_sub(n, super::Ordering::Relaxed);
                if !self.dib.is_null() {
                    DeleteObject(self.dib);
                }
                DeleteDC(self.dc);
            }
        }
    }
}

#[cfg(unix)]
#[path = "text_sw.rs"]
mod sw;

mod portable {
    use std::collections::HashMap;

    use super::{ClipPx, Face, Font, CUPRUM};
    use os_theme::Color;
    use tiny_skia::Pixmap;

    /// Vertical metrics in em: (ascent, descent) as GDI takes them (the OS/2 table's win ascent / descent).
    type Vm = (f32, f32);

    /// The light face is Segoe UI Light on Windows; here Cuprum stands in for it, on Segoe UI's baseline (win ascent
    /// 2210, descent 514 of 2048 units) so the layouts measured against it line up.
    const SEGOE_UI: Vm = (2210.0 / 2048.0, 514.0 / 2048.0);

    /// Synthetic italic slant (horizontal shift per pixel of height), about GDI's 12 degrees.
    const SLANT: f32 = 0.21;

    /// A rasterized glyph: coverage rows, its box relative to the pen on the baseline, and the advance.
    struct Glyph {
        cov: Vec<u8>,
        w: usize,
        h: usize,
        xmin: i32,
        ymin: i32,
        advance: f32,
    }

    pub struct Fd {
        font: fontdue::Font,
        /// Cuprum's win ascent / descent (em).
        ui: Vm,
        /// Cuprum's average character width (em), the reference for `Font::width`.
        avg: f32,
        cache: HashMap<(char, u32, u32, bool, bool), Glyph>,
    }

    /// (win ascent, win descent, average char width) in em from a TrueType font's OS/2 table.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(super) fn os2(data: &[u8]) -> Option<(f32, f32, f32)> {
        let u16_at = |o: usize| data.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
        let u32_at = |o: usize| data.get(o..o + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
        let table = |tag: &[u8; 4]| {
            (0..usize::from(u16_at(4)?)).map(|i| 12 + 16 * i).find(|&r| data.get(r..r + 4) == Some(tag)).and_then(|r| u32_at(r + 8)).map(|o| o as usize)
        };
        let upem = f32::from(u16_at(table(b"head")? + 18)?);
        let o = table(b"OS/2")?;
        let em = |v: u16| f32::from(v) / upem;
        Some((em(u16_at(o + 74)?), em(u16_at(o + 76)?), em(u16_at(o + 2)?)))
    }

    impl Drop for Fd {
        fn drop(&mut self) {
            super::FONTS.fetch_sub(1, super::Ordering::Relaxed);
        }
    }

    impl Fd {
        pub fn new() -> Result<Fd, String> {
            let font = fontdue::Font::from_bytes(CUPRUM, fontdue::FontSettings::default()).map_err(|e| e.to_string())?;
            let (asc, desc, avg) = os2(CUPRUM).ok_or("the UI font has no OS/2 metrics")?;
            super::FONTS.fetch_add(1, super::Ordering::Relaxed);
            Ok(Fd { font, ui: (asc, desc), avg, cache: HashMap::new() })
        }

        /// The em height in device pixels (GDI rounds the requested height).
        fn px(f: Font, scale: f32) -> f32 {
            (f32::from(f.size) * scale).round().max(1.0)
        }

        fn vm(&self, f: Font) -> Vm {
            match f.face {
                Face::Ui => self.ui,
                Face::Light => SEGOE_UI,
            }
        }

        /// Horizontal squeeze for a requested average character width (GDI's `lfWidth`).
        fn squeeze(&self, f: Font, scale: f32) -> f32 {
            if f.width == 0 {
                return 1.0;
            }
            let want = (f32::from(f.width) * scale).round().max(1.0);
            (want / (self.avg * Self::px(f, scale))).clamp(0.3, 2.0)
        }

        /// Synthetic bold: glyphs widen by this many pixels (and advance as much further).
        fn embolden(px: f32) -> usize {
            (px / 24.0).round().max(1.0) as usize
        }

        fn glyph(&mut self, c: char, f: Font, scale: f32) -> &Glyph {
            let (px, k) = (Self::px(f, scale), self.squeeze(f, scale));
            let key = (c, px.to_bits(), k.to_bits(), f.bold, f.italic);
            let font = &self.font;
            self.cache.entry(key).or_insert_with(|| {
                let (m, cov) = font.rasterize(c, px);
                // Condense.
                let (mut w, h) = (m.width, m.height);
                let mut cov = cov;
                let mut xmin = m.xmin;
                if (k - 1.0).abs() > 1e-3 && w > 0 {
                    let nw = ((w as f32 * k).round() as usize).max(1);
                    let mut out = vec![0u8; nw * h];
                    for y in 0..h {
                        for x in 0..nw {
                            let sx = (((x as f32 + 0.5) / k) as usize).min(w - 1);
                            out[y * nw + x] = cov[y * w + sx];
                        }
                    }
                    xmin = (xmin as f32 * k).round() as i32;
                    (cov, w) = (out, nw);
                }
                let mut advance = m.advance_width * k;
                // Bold: each pixel spreads `b` to the right.
                if f.bold && w > 0 {
                    let b = Self::embolden(px);
                    let nw = w + b;
                    let mut out = vec![0u8; nw * h];
                    for y in 0..h {
                        for x in 0..nw {
                            let lo = x.saturating_sub(b);
                            out[y * nw + x] = (lo..=x.min(w - 1)).map(|sx| cov[y * w + sx]).max().unwrap_or(0);
                        }
                    }
                    (cov, w) = (out, nw);
                    advance += b as f32;
                }
                // Italic: rows above the baseline shift right in proportion to their height.
                if f.italic && w > 0 {
                    let top = m.ymin + h as i32;
                    let shift = |y: usize| ((top - y as i32) as f32 * SLANT).round().max(0.0) as usize;
                    let extra = shift(0).max(shift(h.saturating_sub(1)));
                    let nw = w + extra;
                    let mut out = vec![0u8; nw * h];
                    for y in 0..h {
                        let s = shift(y);
                        out[y * nw + s..y * nw + s + w].copy_from_slice(&cov[y * w..(y + 1) * w]);
                    }
                    (cov, w) = (out, nw);
                }
                Glyph { cov, w, h, xmin, ymin: m.ymin, advance }
            })
        }

        pub fn metrics(&mut self, f: Font, scale: f32) -> (i32, i32) {
            let px = Self::px(f, scale);
            let (a, d) = self.vm(f);
            let (asc, desc) = ((a * px).round() as i32, (d * px).round() as i32);
            (asc + desc, asc)
        }

        pub fn extent(&mut self, f: Font, scale: f32, s: &str) -> (i32, i32) {
            let w: f32 = s.chars().map(|c| self.glyph(c, f, scale).advance).sum();
            (w.round() as i32, self.metrics(f, scale).0)
        }

        #[allow(clippy::too_many_arguments)]
        pub fn draw(&mut self, pix: &mut Pixmap, clip: ClipPx, f: Font, scale: f32, x: i32, y: i32, c: Color, s: &str, vertical: bool) {
            let asc = self.metrics(f, scale).1;
            let pw = pix.width() as i32;
            let mut pen = 0.0f32;
            for ch in s.chars() {
                let g = self.glyph(ch, f, scale);
                // Glyph box relative to the cell's top-left (whole pixels).
                let gx0 = pen.round() as i32 + g.xmin;
                let gy0 = asc - g.ymin - g.h as i32;
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
                        for (n, v) in [c.r, c.g, c.b].into_iter().enumerate() {
                            let d = u32::from(data[kk + n]);
                            data[kk + n] = ((u32::from(v) * a + d * (255 - a)) / 255) as u8;
                        }
                    }
                }
                pen += g.advance;
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn cell_and_baseline_follow_the_win_metrics() {
            let mut fd = Fd::new().unwrap();
            // Cuprum: win ascent 910, descent 210 of 1000 units.
            assert_eq!(fd.metrics(Font::new(20), 1.0), (22, 18));
            assert_eq!(fd.metrics(Font::new(20), 1.5), (33, 27));
            // The light face sits on Segoe UI's baseline.
            assert_eq!(fd.metrics(Font::light(32, 13), 1.0).1, 35);
        }

        #[test]
        fn bold_italic_and_condensed_widths() {
            let mut fd = Fd::new().unwrap();
            let f = Font::new(20);
            let plain = fd.extent(f, 1.0, "Pattern").0;
            assert!(fd.extent(f.bold(), 1.0, "Pattern").0 > plain);
            assert_eq!(fd.extent(f.italic(), 1.0, "Pattern").0, plain, "italic keeps the advances");
            assert!(fd.extent(Font::condensed(20, 6), 1.0, "Pattern").0 < plain);
            // Drawing stays inside the pixmap whatever the style.
            let mut pix = Pixmap::new(64, 32).unwrap();
            for f in [f, f.bold(), f.italic(), f.bold().italic()] {
                fd.draw(&mut pix, (0, 0, 64, 32), f, 1.0, 60, 20, Color::rgb(0xffffff), "Wg", false);
                fd.draw(&mut pix, (0, 0, 64, 32), f, 1.0, 2, 30, Color::rgb(0xffffff), "Wg", true);
            }
        }
    }
}
