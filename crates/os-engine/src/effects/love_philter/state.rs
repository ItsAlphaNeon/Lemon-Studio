// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Fruity Love Philter's saved state: an int (the stored version), then a zlib stream with the global settings, the
//! switch block, the description and eight filters, each with its twelve controls, its type, order and switches, its
//! waveshaper curve and eight articulators of curves.
//!
// spec: plugins/fruity-love-philter.md ("The state", "The settings"), plugins/image-line-curves.md ("The stream")

use os_dsp::curve::{Curve, Point, ENVELOPE, LFO, MAP};
use os_dsp::num::Reader;

pub const FILTERS: usize = 8;
pub const TARGETS: usize = 8;
pub const PATTERNS: usize = 10;
pub const CONTROLS: usize = 12;
/// The global ints: master level, master LFO level, follower level, attack, release, pattern number.
pub const GLOBALS: usize = 6;
/// The automatable settings (FL's automation indices 0..=358).
pub const PARAMS: usize = 359;
/// The version FL 2024 writes, and Lemon Studio.
pub const VERSION: i32 = 0x000C_0003;
/// The switch block's length from version 11 on.
pub const SWITCHES: usize = 33;

// The global ints.
pub const MASTER: usize = 0;
pub const LFO_MASTER: usize = 1;
pub const FOLLOW_LEVEL: usize = 2;
pub const FOLLOW_ATTACK: usize = 3;
pub const FOLLOW_RELEASE: usize = 4;
pub const PATTERN: usize = 5;

// A filter's controls.
pub const C_ENV: usize = 0;
pub const C_CUT: usize = 1;
pub const C_RES: usize = 2;
pub const C_LOW: usize = 3;
pub const C_BAND: usize = 4;
pub const C_HIGH: usize = 5;
pub const C_PRE: usize = 6;
pub const C_WSMIX: usize = 7;
pub const C_PAN: usize = 8;
pub const C_VOL: usize = 9;
pub const C_IN: usize = 10;
pub const C_NEXT: usize = 11;

// The articulators' targets.
pub const T_PAN: usize = 0;
pub const T_VOL: usize = 1;
pub const T_CUT: usize = 2;
pub const T_RES: usize = 3;
pub const T_LOW: usize = 4;
pub const T_BAND: usize = 5;
pub const T_HIGH: usize = 6;
pub const T_WSMIX: usize = 7;

/// Pan, cutoff and resonance are additive targets (base 0, centred on 0.5); the others multiply (base 1, centre 0).
pub fn additive(t: usize) -> bool {
    matches!(t, T_PAN | T_CUT | T_RES)
}

/// A target's centre: what a level of its patterns and maps is measured from.
pub fn centre(t: usize) -> f64 {
    if additive(t) { 0.5 } else { 0.0 }
}

// Switch block offsets.
pub const SW_MODX: usize = 0;
pub const SW_MODY: usize = 4;
pub const SW_SOLO: usize = 16;
pub const SW_HQ_ENV: usize = 20;
pub const SW_OVERSAMPLING: usize = 24;
pub const SW_SMOOTH: usize = 28;
pub const SW_DC: usize = 29;
pub const SW_SYNC: usize = 30;
pub const SW_MIDI: usize = 32;

/// Filter types.
pub const CHOCOLATE: i32 = 0;
pub const LIME_LP: i32 = 1;
pub const LIME_HP: i32 = 2;
pub const VANILLA_LP: i32 = 3;
pub const BAND_PASS: i32 = 4;
pub const BAND_STOP: i32 = 5;
pub const VANILLA_HP: i32 = 6;
pub const ALL_PASS: i32 = 7;
pub const LOW_SHELF: i32 = 8;
pub const PEAKING: i32 = 9;
pub const HIGH_SHELF: i32 = 10;
pub const MANGO: i32 = 11;
pub const TYPES: i32 = 12;

/// One target's modulation: a base, the envelope and LFO amounts, ten patterns, the LFO and three maps (mod X, mod Y,
/// envelope follower).
#[derive(Debug, Clone, PartialEq)]
pub struct Articulator {
    pub version: i32,
    pub base: f32,
    pub amount: f32,
    pub lfo_amount: f32,
    /// Patterns 1..10; their time scales (in the curve stream) are kept as stored and not used.
    pub patterns: Vec<Curve>,
    pub lfo: Curve,
    pub maps: [Curve; 3],
}

/// One filter unit.
#[derive(Debug, Clone, PartialEq)]
pub struct Filter {
    pub controls: [i32; CONTROLS],
    pub order: i32,
    pub kind: i32,
    /// The alternate resonance / bandwidth switch.
    pub alt: bool,
    pub enabled: bool,
    pub ws: bool,
    pub ws_curve: Curve,
    /// The waveshaper curve's centre is this byte times 0.5: 0 unipolar, 1 bipolar.
    pub ws_bipolar: u8,
    pub arts: Vec<Articulator>,
}

/// A whole state.
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    /// The version as read (`value >> 16` of a stored value of 0x10000 or more).
    pub version: i32,
    pub globals: [i32; GLOBALS],
    /// The switch block, 33 bytes (bytes an older version does not carry are 1).
    pub switches: [u8; SWITCHES],
    /// The envelope follower listens to filter 1's output.
    pub follow_filter1: bool,
    /// An editor-only int.
    pub editor: i32,
    /// The description (the preset's RTF text), as UTF-16 units.
    pub description: Vec<u16>,
    pub filters: Vec<Filter>,
    /// Interpolated waveshaper look-ups.
    pub hq_ws: bool,
}

/// The version a stored int stands for.
pub fn version_of(stored: i32) -> i32 {
    if stored >= 0x10000 { stored >> 16 } else { stored }
}

impl State {
    pub fn switch_int(&self, at: usize) -> i32 {
        i32::from_le_bytes(self.switches[at..at + 4].try_into().unwrap())
    }

    pub fn set_switch_int(&mut self, at: usize, v: i32) {
        self.switches[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    pub fn mod_x(&self) -> i32 {
        self.switch_int(SW_MODX)
    }

    pub fn mod_y(&self) -> i32 {
        self.switch_int(SW_MODY)
    }

    /// The solo filter, or `None`.
    pub fn solo(&self) -> Option<usize> {
        usize::try_from(self.switch_int(SW_SOLO)).ok().filter(|&f| f < FILTERS)
    }

    pub fn oversampling(&self) -> u32 {
        self.switch_int(SW_OVERSAMPLING).clamp(0, 4) as u32
    }

    pub fn hq_envelopes(&self) -> bool {
        self.switch_int(SW_HQ_ENV) != 0
    }

    pub fn smooth(&self) -> bool {
        self.switches[SW_SMOOTH] != 0
    }

    pub fn dc(&self) -> bool {
        self.switches[SW_DC] != 0
    }

    /// Reads a state (event 213's bytes or a preset's); `None` when it is not one this version reads.
    pub fn from_bytes(b: &[u8]) -> Option<State> {
        let stored = i32::from_le_bytes(b.get(..4)?.try_into().ok()?);
        if stored > VERSION || stored == i32::MAX || stored < 0 {
            return None;
        }
        let version = version_of(stored);
        let data = miniz_oxide::inflate::decompress_to_vec_zlib(&b[4..]).ok()?;
        let mut r = Reader::new(&data);
        let mut globals = [0; GLOBALS];
        for g in globals.iter_mut() {
            *g = r.i32()?;
        }
        globals[PATTERN] = globals[PATTERN].clamp(1, PATTERNS as i32);
        let mut switches = [1u8; SWITCHES];
        let n = if version <= 9 {
            31
        } else if version == 10 {
            32
        } else {
            33
        };
        switches[..n].copy_from_slice(r.bytes(n)?);
        let (mut follow_filter1, mut editor) = (false, 0);
        if version >= 2 {
            follow_filter1 = r.u8()? == 1;
            editor = r.i32()?;
        }
        let description = if version < 6 {
            let len = usize::try_from(r.i32()?).ok()?;
            r.bytes(len)?.iter().map(|&c| u16::from(c)).collect()
        } else {
            let mut len = usize::from(r.u8()?);
            if len == 255 {
                len = usize::try_from(r.i32()?).ok()?;
            }
            r.bytes(len.checked_mul(2)?)?.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect()
        };
        let mut filters = Vec::with_capacity(FILTERS);
        for _ in 0..FILTERS {
            filters.push(read_filter(&mut r)?);
        }
        let hq_ws = if version >= 12 { r.u8().unwrap_or(0) != 0 } else { false };
        Some(State { version, globals, switches, follow_filter1, editor, description, filters, hq_ws })
    }

    /// The state as FL 2024 writes it (version 12, curves as curve version 3).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Vec::with_capacity(32 * 1024);
        for g in self.globals {
            int(&mut w, g);
        }
        w.extend_from_slice(&self.switches);
        w.push(u8::from(self.follow_filter1));
        int(&mut w, self.editor);
        let len = self.description.len();
        if len < 255 {
            w.push(len as u8);
        } else {
            w.push(255);
            int(&mut w, len as i32);
        }
        for c in &self.description {
            w.extend_from_slice(&c.to_le_bytes());
        }
        for f in &self.filters {
            write_filter(&mut w, f);
        }
        w.push(u8::from(self.hq_ws));
        let mut out = VERSION.to_le_bytes().to_vec();
        out.extend_from_slice(&miniz_oxide::deflate::compress_to_vec_zlib(&w, 6));
        out
    }

    /// The description as text (the RTF as stored).
    pub fn description_text(&self) -> String {
        String::from_utf16_lossy(&self.description)
    }

    /// Lemon Studio's own starting state (spec plugins/lemon-pulp-filter.md §5): filter 1 a single Chocolate stage, the
    /// other filters off, every pattern and LFO off and every map neutral, 2x oversampling, smoothing and the
    /// pattern-sync switch on.
    pub fn new_default() -> State {
        let mut sw = [0u8; SWITCHES];
        let mut st = State {
            version: version_of(VERSION),
            globals: [128, 128, 128, 4096, 8192, 1],
            switches: sw,
            follow_filter1: false,
            editor: 0,
            description: Vec::new(),
            filters: (0..FILTERS).map(|_| Filter::new_default()).collect(),
            hq_ws: false,
        };
        sw[SW_SMOOTH] = 1;
        sw[SW_SYNC] = 1;
        sw[31] = 1;
        sw[SW_MIDI] = 1;
        st.switches = sw;
        st.set_switch_int(SW_MODX, 512);
        st.set_switch_int(SW_MODY, 512);
        st.set_switch_int(SW_SOLO, -1);
        st.set_switch_int(SW_OVERSAMPLING, 1);
        st.filters[0].enabled = true;
        st.filters[0].order = 1;
        st
    }

    /// Setting `k`'s value by FL's automation index (spec "The settings").
    pub fn param(&self, k: usize) -> Option<i32> {
        Some(match k {
            0..=5 => self.globals[k],
            6 => self.mod_x(),
            7 => self.mod_y(),
            _ => {
                let (f, item) = filter_param(k)?;
                match item {
                    FilterParam::Control(n) => self.filters[f].controls[n],
                    FilterParam::Lfo(t, i) => self.filters[f].arts[t].lfo.lfo[i],
                }
            }
        })
    }

    /// Sets setting `k` (the stored value only; the effect applies it).
    pub fn set_param(&mut self, k: usize, v: i32) {
        match k {
            0..=5 => self.globals[k] = if k == PATTERN { v.clamp(1, PATTERNS as i32) } else { v },
            6 => self.set_switch_int(SW_MODX, v),
            7 => self.set_switch_int(SW_MODY, v),
            _ => match filter_param(k) {
                Some((f, FilterParam::Control(n))) => self.filters[f].controls[n] = v,
                Some((f, FilterParam::Lfo(t, i))) => self.filters[f].arts[t].lfo.lfo[i] = v,
                None => {}
            },
        }
    }
}

/// What a filter's automation index names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterParam {
    Control(usize),
    /// Target `t`'s LFO parameter `i`: 0 speed, 1 tension, 2 skew, 3 pulse width.
    Lfo(usize, usize),
}

/// The filter and item of automation index `k` (8 and up): 44 indices per filter, filter 8's eleven controls without
/// "to next".
pub fn filter_param(k: usize) -> Option<(usize, FilterParam)> {
    let k = k.checked_sub(8)?;
    let f = (k / 44).min(FILTERS - 1);
    let n = k - 44 * f;
    let c = if f == FILTERS - 1 { 11 } else { 12 };
    if k >= PARAMS - 8 {
        return None;
    }
    Some(if n < c { (f, FilterParam::Control(n)) } else { (f, FilterParam::Lfo((n - c) / 4, (n - c) % 4)) })
}

/// The automation index of filter `f`'s control `n` (`None` for filter 8's "to next").
pub fn control_index(f: usize, n: usize) -> Option<usize> {
    (f < FILTERS - 1 || n < 11).then_some(8 + 44 * f + n)
}

/// The automation index of filter `f`'s target `t`'s LFO parameter `i`.
pub fn lfo_index(f: usize, t: usize, i: usize) -> usize {
    let c = if f == FILTERS - 1 { 11 } else { 12 };
    8 + 44 * f + c + 4 * t + i
}

/// A setting's range by automation index.
pub fn range(k: usize) -> (i32, i32) {
    match k {
        0 | 2 => (0, 160),
        1 => (-128, 128),
        3 | 4 => (0, 16384),
        5 => (1, 10),
        6 | 7 => (0, 1024),
        _ => match filter_param(k) {
            Some((_, FilterParam::Control(n))) => CONTROL_RANGES[n],
            Some((_, FilterParam::Lfo(_, i))) => LFO_RANGES[i],
            None => (0, 0),
        },
    }
}

/// The controls' ranges.
pub const CONTROL_RANGES: [(i32, i32); CONTROLS] =
    [(0, 256), (0, 256), (0, 256), (0, 128), (0, 128), (0, 128), (64, 192), (0, 128), (-128, 128), (0, 160), (-160, 160), (-160, 160)];
/// The controls' defaults (the Default preset's filter 1).
pub const CONTROL_DEFAULTS: [i32; CONTROLS] = [128, 128, 128, 128, 0, 0, 128, 128, 0, 128, 128, 0];
/// The LFO parameters' ranges: speed, tension, skew, pulse width.
pub const LFO_RANGES: [(i32, i32); 4] = [(512, 65536), (-128, 128), (-128, 128), (-112, 112)];
/// Lemon Studio's starting LFO parameters (spec plugins/lemon-pulp-filter.md §5).
pub const LFO_DEFAULTS: [i32; 4] = [40000, 0, 0, 0];

impl Filter {
    /// A filter of Lemon Studio's starting state: the default controls, Chocolate, order 1, off.
    pub fn new_default() -> Filter {
        let mut ws_curve = Curve::default_of(MAP);
        ws_curve.points = vec![Point { dist: 0.0, level: 0.0, ..Point::default() }, Point { dist: 1.0, level: 1.0, ..Point::default() }];
        ws_curve.enabled = true;
        ws_curve.keep_love_philter();
        Filter {
            controls: CONTROL_DEFAULTS,
            order: 1,
            kind: CHOCOLATE,
            alt: false,
            enabled: false,
            ws: false,
            ws_curve,
            ws_bipolar: 0,
            arts: (0..TARGETS).map(Articulator::new_default).collect(),
        }
    }
}

impl Articulator {
    /// A target's neutral articulator: patterns and LFO off, maps flat at the neutral level.
    pub fn new_default(t: usize) -> Articulator {
        let neutral = if additive(t) { 0.5 } else { 1.0 };
        let flat = |kind: i32, level: f32, n: usize| {
            let mut c = Curve::default_of(kind);
            c.points = (0..n).map(|i| Point { dist: if i == 0 { 0.0 } else { 1.0 }, level, ..Point::default() }).collect();
            c.enabled = true;
            c
        };
        let mut patterns: Vec<Curve> = (0..PATTERNS)
            .map(|_| {
                let mut c = flat(ENVELOPE, neutral as f32, 1);
                c.enabled = false;
                c.flags = os_dsp::curve::SYNCED;
                c
            })
            .collect();
        for c in patterns.iter_mut() {
            c.keep_love_philter();
        }
        let mut lfo = flat(LFO, 1.0, 1);
        lfo.enabled = false;
        lfo.flags = os_dsp::curve::SYNCED;
        lfo.lfo = [LFO_DEFAULTS[0], LFO_DEFAULTS[1], LFO_DEFAULTS[2], LFO_DEFAULTS[3], 0];
        lfo.keep_love_philter();
        let map = || {
            let mut c = flat(MAP, neutral as f32, 2);
            c.keep_love_philter();
            c
        };
        Articulator {
            version: 1,
            base: if additive(t) { 0.0 } else { 1.0 },
            amount: 1.0,
            lfo_amount: 1.0,
            patterns,
            lfo,
            maps: [map(), map(), map()],
        }
    }
}

fn int(w: &mut Vec<u8>, v: i32) {
    w.extend_from_slice(&v.to_le_bytes());
}

fn read_curve(r: &mut Reader, kind: i32) -> Option<Curve> {
    let mut c = Curve::read(r, kind)?;
    c.keep_love_philter();
    Some(c)
}

fn read_filter(r: &mut Reader) -> Option<Filter> {
    let mut controls = [0; CONTROLS];
    for c in controls.iter_mut() {
        *c = r.i32()?;
    }
    let order = r.i32()?;
    let kind = r.i32()?;
    let alt = r.u8()? != 0;
    let enabled = r.u8()? != 0;
    let ws = r.u8()? != 0;
    let ws_curve = read_curve(r, MAP)?;
    let ws_bipolar = r.u8()?;
    let mut arts = Vec::with_capacity(TARGETS);
    for _ in 0..TARGETS {
        let version = r.i32()?;
        let base = r.f32()?;
        let amount = r.f32()?;
        let lfo_amount = r.f32()?;
        let mut patterns = Vec::with_capacity(PATTERNS);
        for _ in 0..PATTERNS {
            patterns.push(read_curve(r, ENVELOPE)?);
        }
        let lfo = read_curve(r, LFO)?;
        let maps = [read_curve(r, MAP)?, read_curve(r, MAP)?, read_curve(r, MAP)?];
        arts.push(Articulator { version, base, amount, lfo_amount, patterns, lfo, maps });
    }
    Some(Filter { controls, order, kind, alt, enabled, ws, ws_curve, ws_bipolar, arts })
}

fn write_filter(w: &mut Vec<u8>, f: &Filter) {
    for c in f.controls {
        int(w, c);
    }
    int(w, f.order);
    int(w, f.kind);
    w.extend_from_slice(&[u8::from(f.alt), u8::from(f.enabled), u8::from(f.ws)]);
    f.ws_curve.write(w);
    w.push(f.ws_bipolar);
    for a in &f.arts {
        int(w, a.version);
        for v in [a.base, a.amount, a.lfo_amount] {
            w.extend_from_slice(&v.to_le_bytes());
        }
        for c in &a.patterns {
            c.write(w);
        }
        a.lfo.write(w);
        for m in &a.maps {
            m.write(w);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_state_round_trips() {
        let st = State::new_default();
        let b = st.to_bytes();
        assert_eq!(i32::from_le_bytes(b[..4].try_into().unwrap()), VERSION);
        let back = State::from_bytes(&b).expect("reads its own state");
        assert_eq!(back, st);
        assert_eq!(back.to_bytes(), b);
        assert_eq!(back.oversampling(), 1);
        assert_eq!(back.solo(), None);
        assert_eq!((back.mod_x(), back.mod_y()), (512, 512));
    }

    #[test]
    fn automation_indices_name_the_settings() {
        assert_eq!(filter_param(8), Some((0, FilterParam::Control(0))));
        assert_eq!(filter_param(19), Some((0, FilterParam::Control(11))));
        assert_eq!(filter_param(20), Some((0, FilterParam::Lfo(0, 0))));
        assert_eq!(filter_param(51), Some((0, FilterParam::Lfo(7, 3))));
        assert_eq!(filter_param(52), Some((1, FilterParam::Control(0))));
        // Filter 8 has eleven controls: index 8 + 44 * 7 + 11 is its first LFO parameter, 358 the last.
        assert_eq!(filter_param(8 + 308 + 10), Some((7, FilterParam::Control(10))));
        assert_eq!(filter_param(8 + 308 + 11), Some((7, FilterParam::Lfo(0, 0))));
        assert_eq!(filter_param(358), Some((7, FilterParam::Lfo(7, 3))));
        assert_eq!(filter_param(359), None);
        assert_eq!(lfo_index(7, 7, 3), 358);
        assert_eq!(control_index(7, 11), None);
        assert_eq!(range(5), (1, 10));
        assert_eq!(range(control_index(2, C_PRE).unwrap()), (64, 192));
        let mut st = State::new_default();
        st.set_param(control_index(3, C_CUT).unwrap(), 200);
        st.set_param(6, 100);
        st.set_param(lfo_index(1, T_CUT, 0), 4000);
        assert_eq!(st.filters[3].controls[C_CUT], 200);
        assert_eq!(st.mod_x(), 100);
        assert_eq!(st.param(lfo_index(1, T_CUT, 0)), Some(4000));
    }

    #[test]
    fn unreadable_states_are_refused() {
        assert!(State::from_bytes(&[]).is_none());
        let mut b = State::new_default().to_bytes();
        b[..4].copy_from_slice(&0x000C_0004i32.to_le_bytes());
        assert!(State::from_bytes(&b).is_none());
        b[..4].copy_from_slice(&i32::MAX.to_le_bytes());
        assert!(State::from_bytes(&b).is_none());
    }

    /// FL's own Default.dat, when FL is installed here.
    #[test]
    fn reads_fls_default_preset_when_present() {
        let p = r"C:\Program Files\Image-Line\FL Studio 2026\Plugins\Fruity\Effects\Fruity Love Philter\Data\Default.dat";
        let Ok(b) = std::fs::read(p) else { return };
        let st = State::from_bytes(&b).expect("FL's default reads");
        assert_eq!(st.filters.len(), FILTERS);
        let again = State::from_bytes(&st.to_bytes()).expect("and reads back");
        assert_eq!(again.filters[0].controls, st.filters[0].controls);
        assert_eq!(again.globals, st.globals);
    }
}
