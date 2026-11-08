// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The saved state (FLP event 213's bytes of a Wave Candy slot, version 9): 28 colour records, the title and its font,
//! then the display's settings. Every field is kept, the ones without a meaning too, so a state is written back as it
//! was read.
//!
// spec: plugins/wave-candy.md "State"

/// The version this layout is.
pub const VERSION: i32 = 9;

/// The colour targets, in their order in the state and in the colour menu.
pub const TARGETS: usize = 28;
pub const TARGET_NAMES: [&str; TARGETS] = [
    "Background",
    "Background punches",
    "Border",
    "Bevel top",
    "Bevel bottom",
    "Reflection top",
    "Reflection bottom",
    "Title",
    "Labels",
    "Hint",
    "Hint location",
    "Oscilloscope ruler",
    "Waveform",
    "Spectrum gradient color 1",
    "Spectrum gradient color 2",
    "Spectrum gradient color 3",
    "Spectrum gradient color 4",
    "Spectrum gradient color 5",
    "Meter background",
    "Meter gradient color -INFdB",
    "Meter gradient color -6dB",
    "Meter gradient color -0dB",
    "Meter gradient color +0dB",
    "Meter gradient color +6dB",
    "Vectorscope ruler",
    "Lissajou",
    "Glowing lights",
    "Glass effect",
];
/// The colour menu's groups: (heading, first target, targets).
pub const TARGET_GROUPS: [(&str, usize, usize); 9] = [
    ("Frame", 0, 3),
    ("Bevel", 3, 2),
    ("Glass", 5, 2),
    ("Text", 7, 3),
    ("Helpers", 10, 1),
    ("Oscilloscope", 11, 2),
    ("Spectrum", 13, 5),
    ("Peak meter", 18, 6),
    ("Vectorscope", 24, 2),
];
pub const EYE_CANDY: (&str, usize, usize) = ("Eye candy", 26, 2);

pub const BACKGROUND: usize = 0;
pub const PUNCHES: usize = 1;
pub const BORDER: usize = 2;
pub const BEVEL_TOP: usize = 3;
pub const BEVEL_BOTTOM: usize = 4;
pub const REFLECTION_TOP: usize = 5;
pub const REFLECTION_BOTTOM: usize = 6;
pub const TITLE: usize = 7;
pub const LABELS: usize = 8;
pub const HINT: usize = 9;
pub const HINT_LOCATION: usize = 10;
pub const SCOPE_RULER: usize = 11;
pub const WAVEFORM: usize = 12;
pub const SPECTRUM: usize = 13;
pub const METER_BACKGROUND: usize = 18;
pub const METER: usize = 19;
pub const VECTOR_RULER: usize = 24;
pub const LISSAJOU: usize = 25;
pub const GLOW: usize = 26;
pub const GLASS: usize = 27;

/// One colour target: object and colour opacity (0 .. 256), hue (0 .. 255, a wheel), lightness and saturation
/// (0 .. 256).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Paint {
    pub object: i32,
    pub color: i32,
    pub hue: i32,
    pub light: i32,
    pub sat: i32,
}

impl Paint {
    pub const fn new(object: i32, color: i32, hue: i32, light: i32, sat: i32) -> Self {
        Paint { object, color, hue, light, sat }
    }

    /// The colour as (r, g, b) in 0 .. 1, not premultiplied: hue 0 is blue and the wheel runs through green (80),
    /// yellow (128) and red (170) back to blue.
    pub fn rgb(&self) -> [f32; 3] {
        let h = (240.0 - self.hue as f32 * 360.0 / 256.0).rem_euclid(360.0);
        hls(h, (self.light as f32 / 256.0).clamp(0.0, 1.0), (self.sat as f32 / 256.0).clamp(0.0, 1.0))
    }

    /// The opacity a target is drawn with (0 .. 1): its object opacity times its colour opacity.
    pub fn alpha(&self) -> f32 {
        (self.object.clamp(0, 256) as f32 / 256.0) * (self.color.clamp(0, 256) as f32 / 256.0)
    }

    /// The colour word the state keeps after the five settings: 0xAARRGGBB, premultiplied. The gradient colours
    /// (`is_gradient`) fold their object opacity into the alpha; the other targets keep only the colour opacity.
    pub fn word(&self, gradient: bool) -> u32 {
        let a = if gradient { self.object.clamp(0, 256) * self.color.clamp(0, 256) / 256 } else { self.color.clamp(0, 256) };
        let a = (a - 1).clamp(0, 255) as u32;
        let [r, g, b] = self.rgb();
        let q = |c: f32| ((c * 255.0).round() as u32 * a / 255).min(255);
        (a << 24) | (q(r) << 16) | (q(g) << 8) | q(b)
    }
}

/// HLS to RGB (hue in degrees).
pub fn hls(h: f32, l: f32, s: f32) -> [f32; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    [r + m, g + m, b + m]
}

/// The four display modes, in the state's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Oscilloscope,
    Spectrum,
    Meter,
    Vectorscope,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Oscilloscope, Mode::Spectrum, Mode::Meter, Mode::Vectorscope];
    pub fn from_i32(v: i32) -> Mode {
        Mode::ALL[v.clamp(0, 3) as usize]
    }
    pub fn name(self) -> &'static str {
        ["Oscilloscope", "Spectrum", "Meter", "Vectorscope"][self as usize]
    }
}

/// The meter's ballistics, by their stored value.
pub const METER_MODES: [(i32, &str); 6] = [(0, "Peak"), (1, "PPM"), (4, "RMS"), (2, "Vu"), (3, "Leq(A)"), (5, "ITU-R BS.1770")];
/// The vectorscope's drawing, by its stored value.
pub const VECTOR_MODES: [&str; 4] = ["String", "Spaghetti", "Tube", "Glowstick"];

/// "No fading" (fade out) and "Infinite hold" (meter hold): one past the knob's last millisecond.
pub const NEVER: i32 = 5001;

/// A window's place on the screen: left, top, right, bottom.
pub type Rect4 = [i32; 4];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    /// The int after the version (0 in every state seen).
    pub reserved: i32,
    pub paints: [Paint; TARGETS],
    /// The colour words as read (written back as they were when the paint did not change).
    pub words: [u32; TARGETS],
    /// The display's title and its font face (single-byte text), and the font's height (negative: character height
    /// in pixels, as Windows fonts are given).
    pub title: Vec<u8>,
    pub font: Vec<u8>,
    pub font_height: i32,
    /// Four bytes after the font height (1, 2, 0, 0 in a fresh instance).
    pub font_flags: [u8; 4],
    pub mode: Mode,
    /// Visual gain in tenths of a dB (-120 .. 120).
    pub gain: i32,
    /// Fade out after silence, ms (0 .. 5000, `NEVER`).
    pub fade: i32,
    /// Bevel / reflection: -256 (all bevel) .. 256 (all reflection).
    pub bevel: i32,
    /// Punches: negative rounded, positive square, size in px (-24 .. 24).
    pub punches: i32,
    /// Oscilloscope: block length and block repeat in hundredths of a ms (500 .. 5000, 1000 .. 10000).
    pub scope_length: i32,
    pub scope_repeat: i32,
    /// Spectrum: block length (500 .. 10000, hundredths of a ms), the maximum resolution (32 << n bands, n 0 .. 8),
    /// frequency curve (0 .. 256) and dB range (48 .. 144).
    pub spectrum_length: i32,
    pub spectrum_res: i32,
    pub spectrum_curve: i32,
    pub spectrum_range: i32,
    /// Meter: scale (0 .. 256), decay speed, hold duration (ms, 0 none .. `NEVER` infinite).
    pub meter_scale: i32,
    pub meter_decay: i32,
    pub meter_hold: i32,
    pub pinch: bool,
    pub interpolate: bool,
    pub visible: bool,
    pub on_desktop: bool,
    /// The byte before "keep in front" (0 seen).
    pub spare: u8,
    pub keep_in_front: bool,
    pub meter_mode: i32,
    pub auto_mode: bool,
    /// Rounded corners 0 .. 512 (0 .. 12 px).
    pub corners: i32,
    /// Oscilloscope stereo separation: -1 (none) or 0 .. 1000.
    pub separation: i32,
    /// Vectorscope: block length (500 .. 5000), thickness (100 .. 600, hundredths), drawing (`VECTOR_MODES`).
    pub vector_length: i32,
    pub vector_thickness: i32,
    pub vector_mode: i32,
    pub additive: bool,
    pub natural: bool,
    pub glow: bool,
    pub enhanced: bool,
    pub mini: bool,
    pub background: bool,
    pub solid: bool,
    pub desktop_flag: bool,
    /// The display window's place, and the mini view's.
    pub window: Rect4,
    pub mini_window: Rect4,
}

impl Default for State {
    fn default() -> Self {
        State::new_default()
    }
}

/// A fresh instance's colours.
const DEFAULT_PAINTS: [Paint; TARGETS] = [
    Paint::new(256, 100, 0, 0, 0),
    Paint::new(256, 80, 0, 0, 0),
    Paint::new(256, 160, 0, 0, 0),
    Paint::new(64, 256, 0, 0, 0),
    Paint::new(64, 256, 0, 256, 0),
    Paint::new(64, 256, 0, 256, 0),
    Paint::new(64, 256, 0, 0, 0),
    Paint::new(128, 256, 0, 256, 0),
    Paint::new(128, 256, 0, 256, 0),
    Paint::new(192, 256, 0, 256, 0),
    Paint::new(96, 256, 0, 256, 0),
    Paint::new(64, 256, 0, 256, 0),
    Paint::new(256, 256, 0, 256, 128),
    Paint::new(0, 256, 0, 0, 0),
    Paint::new(120, 256, 0, 120, 128),
    Paint::new(200, 256, 168, 120, 256),
    Paint::new(256, 256, 128, 160, 256),
    Paint::new(256, 256, 0, 256, 0),
    Paint::new(90, 200, 0, 0, 0),
    Paint::new(256, 256, 60, 90, 200),
    Paint::new(256, 256, 80, 100, 220),
    Paint::new(256, 256, 80, 110, 256),
    Paint::new(256, 256, 128, 120, 256),
    Paint::new(256, 256, 160, 120, 256),
    Paint::new(128, 256, 0, 256, 0),
    Paint::new(0, 0, 0, 256, 0),
    Paint::new(24, 256, 0, 256, 0),
    Paint::new(32, 256, 0, 256, 0),
];

/// The spectrum's and the meter's gradient colours: their colour word folds the object opacity in.
pub fn is_gradient(i: usize) -> bool {
    (SPECTRUM..SPECTRUM + 5).contains(&i) || (METER..METER + 5).contains(&i)
}

fn words_of(p: &[Paint; TARGETS]) -> [u32; TARGETS] {
    std::array::from_fn(|i| p[i].word(is_gradient(i)))
}

impl State {
    /// A fresh instance (the original's, as it saves one).
    pub fn new_default() -> State {
        State {
            reserved: 0,
            paints: DEFAULT_PAINTS,
            words: words_of(&DEFAULT_PAINTS),
            title: b"Wave Candy".to_vec(),
            font: b"Trebuchet MS".to_vec(),
            font_height: -15,
            font_flags: [1, 2, 0, 0],
            mode: Mode::Oscilloscope,
            gain: 0,
            fade: NEVER,
            bevel: 64,
            punches: 0,
            scope_length: 1000,
            scope_repeat: 2000,
            spectrum_length: 1500,
            spectrum_res: 5,
            spectrum_curve: 0,
            spectrum_range: 96,
            meter_scale: 128,
            meter_decay: 750,
            meter_hold: 2500,
            pinch: true,
            interpolate: true,
            visible: true,
            on_desktop: false,
            spare: 0,
            keep_in_front: false,
            meter_mode: 0,
            auto_mode: false,
            corners: 256,
            separation: -1,
            vector_length: 1000,
            vector_thickness: 350,
            vector_mode: 2,
            additive: true,
            natural: true,
            glow: false,
            enhanced: true,
            mini: false,
            background: true,
            solid: true,
            desktop_flag: false,
            window: [600, 200, 901, 325],
            mini_window: [1189, 933, 1236, 980],
        }
    }

    /// Reads a version 9 state; None for any other version or a short one.
    pub fn from_bytes(b: &[u8]) -> Option<State> {
        let mut r = Reader { b, at: 0 };
        if r.i32()? != VERSION {
            return None;
        }
        let reserved = r.i32()?;
        let mut paints = DEFAULT_PAINTS;
        let mut words = [0u32; TARGETS];
        for i in 0..TARGETS {
            paints[i] = Paint::new(r.i32()?, r.i32()?, r.i32()?, r.i32()?, r.i32()?);
            words[i] = r.i32()? as u32;
        }
        let title = r.text()?;
        let font = r.text()?;
        let font_height = r.i32()?;
        let font_flags = r.bytes4()?;
        let mode = r.i32()?;
        let mut s = State {
            reserved,
            paints,
            words,
            title,
            font,
            font_height,
            font_flags,
            mode: Mode::from_i32(mode),
            gain: r.i32()?,
            fade: r.i32()?,
            bevel: r.i32()?,
            punches: r.i32()?,
            scope_length: r.i32()?,
            scope_repeat: r.i32()?,
            spectrum_length: r.i32()?,
            spectrum_res: r.i32()?,
            spectrum_curve: r.i32()?,
            spectrum_range: r.i32()?,
            meter_scale: r.i32()?,
            meter_decay: r.i32()?,
            meter_hold: r.i32()?,
            ..State::new_default()
        };
        let [a, b2, c, d] = r.bytes4()?;
        (s.pinch, s.interpolate, s.visible, s.on_desktop) = (a != 0, b2 != 0, c != 0, d != 0);
        let [a, b2, c, d] = r.bytes4()?;
        (s.spare, s.keep_in_front, s.meter_mode, s.auto_mode) = (a, b2 != 0, i32::from(c), d != 0);
        s.corners = r.i32()?;
        s.separation = r.i32()?;
        s.vector_length = r.i32()?;
        s.vector_thickness = r.i32()?;
        s.vector_mode = r.i32()?;
        let [a, b2, c, d] = r.bytes4()?;
        (s.additive, s.natural, s.glow, s.enhanced) = (a != 0, b2 != 0, c != 0, d != 0);
        let [a, b2, c, d] = r.bytes4()?;
        (s.mini, s.background, s.solid, s.desktop_flag) = (a != 0, b2 != 0, c != 0, d != 0);
        s.window = [r.i32()?, r.i32()?, r.i32()?, r.i32()?];
        s.mini_window = [r.i32()?, r.i32()?, r.i32()?, r.i32()?];
        Some(s)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut o = Vec::with_capacity(840);
        let i = |o: &mut Vec<u8>, v: i32| o.extend_from_slice(&v.to_le_bytes());
        let flags = |o: &mut Vec<u8>, f: [bool; 4]| o.extend(f.map(u8::from));
        i(&mut o, VERSION);
        i(&mut o, self.reserved);
        for (k, p) in self.paints.iter().enumerate() {
            for v in [p.object, p.color, p.hue, p.light, p.sat] {
                i(&mut o, v);
            }
            i(&mut o, self.words[k] as i32);
        }
        for t in [&self.title, &self.font] {
            let t = &t[..t.len().min(255)];
            o.push(t.len() as u8);
            o.extend_from_slice(t);
        }
        i(&mut o, self.font_height);
        o.extend_from_slice(&self.font_flags);
        for v in [
            self.mode as i32,
            self.gain,
            self.fade,
            self.bevel,
            self.punches,
            self.scope_length,
            self.scope_repeat,
            self.spectrum_length,
            self.spectrum_res,
            self.spectrum_curve,
            self.spectrum_range,
            self.meter_scale,
            self.meter_decay,
            self.meter_hold,
        ] {
            i(&mut o, v);
        }
        flags(&mut o, [self.pinch, self.interpolate, self.visible, self.on_desktop]);
        o.extend_from_slice(&[self.spare, u8::from(self.keep_in_front), self.meter_mode.clamp(0, 255) as u8, u8::from(self.auto_mode)]);
        for v in [self.corners, self.separation, self.vector_length, self.vector_thickness, self.vector_mode] {
            i(&mut o, v);
        }
        flags(&mut o, [self.additive, self.natural, self.glow, self.enhanced]);
        flags(&mut o, [self.mini, self.background, self.solid, self.desktop_flag]);
        for v in self.window.iter().chain(&self.mini_window) {
            i(&mut o, *v);
        }
        o
    }

    /// Sets target `k`'s paint and its colour word.
    pub fn set_paint(&mut self, k: usize, p: Paint) {
        self.paints[k] = p;
        self.words[k] = p.word(is_gradient(k));
    }

    pub fn title_text(&self) -> String {
        self.title.iter().map(|&c| c as char).collect()
    }

    pub fn font_text(&self) -> String {
        self.font.iter().map(|&c| c as char).collect()
    }

    /// The meter's stored mode as its menu index.
    pub fn meter_mode_index(&self) -> usize {
        METER_MODES.iter().position(|m| m.0 == self.meter_mode).unwrap_or(0)
    }
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.b.get(self.at..self.at + n)?;
        self.at += n;
        Some(s)
    }
    fn i32(&mut self) -> Option<i32> {
        Some(i32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn bytes4(&mut self) -> Option<[u8; 4]> {
        self.take(4)?.try_into().ok()
    }
    fn text(&mut self) -> Option<Vec<u8>> {
        let n = *self.take(1)?.first()? as usize;
        Some(self.take(n)?.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh instance of the original, saved (`plugins/wave-candy.md` "State").
    pub const FRESH: &str = concat!(
        "090000000000000000010000640000000000000000000000000000000000006300010000500000000000000000000000000000000000004f",
        "00010000a00000000000000000000000000000000000009f4000000000010000000000000000000000000000000000ff4000000000010000",
        "000000000001000000000000ffffffff4000000000010000000000000001000000000000ffffffff40000000000100000000000000000000",
        "00000000000000ff8000000000010000000000000001000000000000ffffffff8000000000010000000000000001000000000000ffffffff",
        "c000000000010000000000000001000000000000ffffffff6000000000010000000000000001000000000000ffffffff4000000000010000",
        "000000000001000000000000ffffffff0001000000010000000000000001000080000000ffffffff00000000000100000000000000000000",
        "00000000000000007800000000010000000000007800000080000000531c1c77c800000000010000a80000007800000000010000000bbac7",
        "000100000001000080000000a00000000001000040ffffff0001000000010000000000000001000000000000ffffffff5a000000c8000000",
        "000000000000000000000000000000c700010000000100003c0000005a000000c800000067a014ff00010000000100005000000064000000",
        "dc00000023b90eff0001000000010000500000006e000000000100001bdb00ff000100000001000080000000780000000001000000efefff",
        "0001000000010000a00000007800000000010000003cefff8000000000010000000000000001000000000000ffffffff0000000000000000",
        "000000000001000000000000000000001800000000010000000000000001000000000000ffffffff20000000000100000000000000010000",
        "00000000ffffffff0a576176652043616e64790c547265627563686574204d53f1ffffff0102000000000000000000008913000040000000",
        "00000000e8030000d0070000dc05000005000000000000006000000080000000ee020000c4090000010101000000000000010000ffffffff",
        "e80300005e01000002000000010100010001010058020000c80000008503000045010000a5040000a5030000d4040000d4030000",
    );

    fn unhex(s: &str) -> Vec<u8> {
        let s: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
        (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn a_fresh_state_reads_and_writes_back() {
        let b = unhex(FRESH);
        assert_eq!(b.len(), 836);
        let s = State::from_bytes(&b).unwrap();
        assert_eq!(s, State::new_default());
        assert_eq!(s.to_bytes(), b);
        assert_eq!(State::new_default().to_bytes(), b);
    }

    #[test]
    fn colour_words_are_the_originals() {
        for (k, p) in DEFAULT_PAINTS.iter().enumerate() {
            let want = State::from_bytes(&unhex(FRESH)).unwrap().words[k];
            let got = p.word(is_gradient(k));
            let close = |a: u32, b: u32| (0..4).all(|s| ((a >> (8 * s)) & 255).abs_diff((b >> (8 * s)) & 255) <= 1);
            assert!(close(got, want), "{k} {}: {got:08x} {want:08x}", TARGET_NAMES[k]);
        }
    }

    #[test]
    fn other_versions_and_short_states_are_refused() {
        let mut b = unhex(FRESH);
        assert!(State::from_bytes(&b[..800]).is_none());
        b[0] = 7;
        assert!(State::from_bytes(&b).is_none());
    }

    #[test]
    fn titles_of_any_length_shift_the_tail() {
        let mut s = State::new_default();
        s.title = b"Lemon".to_vec();
        s.gain = -37;
        s.meter_mode = 4;
        let b = s.to_bytes();
        assert_eq!(b.len(), 836 - 5);
        assert_eq!(State::from_bytes(&b).unwrap(), s);
    }
}
