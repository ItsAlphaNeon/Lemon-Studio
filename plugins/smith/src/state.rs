// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! An Ogun state (a project's event 213 or a preset's) read into one form, whichever of its versions it is, and
//! written back as the newest version. What was not edited stays as it was loaded: the plugin keeps the loaded bytes
//! until the first edit.
//
// spec: plugins/ogun.md ("The state")

use os_dsp::curve::{map_at, Curve, Point, ENVELOPE, LFO, MAP};
use os_dsp::num::Reader;

/// The version written.
pub const VERSION: u32 = 0x0013_0003;
/// From this version on the comment is UTF-16.
const VERSION_TEXT: u32 = 0x0012_0003;
pub const INTS: usize = 53;
pub const ARTS: usize = 13;
pub const FINALS: usize = 4;

// The int fields.
pub const LEVEL: usize = 0;
pub const COARSE: usize = 1;
pub const FINE: usize = 2;
pub const MOD_X: usize = 3;
pub const MOD_Y: usize = 4;
pub const PRE: usize = 5;
pub const DEC: usize = 6;
pub const REL: usize = 7;
pub const FULL: usize = 8;
pub const MORPH: usize = 9;
pub const SEED1: usize = 10;
pub const SEED2: usize = 11;
pub const U_ORD: usize = 12;
pub const U_PAN: usize = 13;
pub const U_VOL: usize = 14;
pub const U_PITCH: usize = 15;
pub const U_PHS: usize = 16;
pub const CUT: usize = 17;
pub const RES: usize = 18;
/// Chorus order, depth, speed, delay, spread, cross, volume.
pub const CHORUS: usize = 19;
/// Delay feedback, time, stereo offset, volume, mode.
pub const DELAY: usize = 26;
/// Reverb low cut, high cut, predelay, room, diffusion, decay, high damping, colour, wet.
pub const REVERB: usize = 31;
pub const EQ_LEVEL: usize = 40;
pub const EQ_FREQ: usize = 43;
pub const EQ_WIDTH: usize = 46;
pub const F_TYPE: usize = 49;
pub const F_MODE: usize = 50;
pub const FLAT: usize = 51;
pub const RICH: usize = 52;

// The articulators.
pub const A_VOLUME: usize = 0;
pub const A_CUT: usize = 1;
pub const A_RES: usize = 2;
pub const A_PITCH: usize = 3;
pub const A_U_PITCH: usize = 4;
pub const A_U_PHASE: usize = 5;
pub const A_PRE: usize = 6;
pub const A_DEC: usize = 7;
pub const A_REL: usize = 8;
pub const A_FULL: usize = 9;
pub const A_MORPH: usize = 10;
pub const A_ROT: usize = 11;
pub const A_SHIFT: usize = 12;

// The final maps.
pub const M_FORCED: usize = 0;
pub const M_IMPULSE: usize = 1;
pub const M_DECAY: usize = 2;
pub const M_RELEASE: usize = 3;

/// Whether comb setting `comb` lets line `k` take its map: 1..=4 are "-2".."-5", 5.. are "+3" on.
pub fn comb_gate(comb: i32, k: usize) -> bool {
    match comb {
        1..=4 => k % (comb as usize + 1) != 0,
        5.. => k % (comb as usize - 2) == 1,
        _ => true,
    }
}

pub const MAP_KEY: usize = 0;
pub const MAP_VEL: usize = 1;
pub const MAP_X: usize = 2;
pub const MAP_Y: usize = 3;

/// Whether articulator `a` is read around 0.5 (a sum) rather than as a factor.
pub fn bipolar(a: usize) -> bool {
    matches!(a, A_CUT | A_RES | A_PITCH | A_FULL | A_ROT | A_SHIFT)
}

/// A map's default level on articulator `a`.
fn map_default(a: usize) -> f64 {
    if bipolar(a) { 0.5 } else { 1.0 }
}

/// The default int fields.
pub const DEFAULT_INTS: [i32; INTS] = [
    160, 0, 0, 256, 256, 100, 4000, 2000, -500, 300, 1, 0, 1, 128, 128, 128, 64, 128, 128, 0, 512, 768, 8, 128, -32, 64, 64, 144, -260, 64, 2, 75, 40, 0, 40,
    100, 15, 40, 2, 55, 0, 0, 0, 8950, 33145, 49305, 17500, 17500, 17500, 0, 0, -1, 14,
];

fn point(dist: f64, level: f64, tension: f64) -> Point {
    Point { dist: dist as f32, level: level as f32, tension: tension as f32, ..Point::default() }
}

/// The envelope default.
fn default_env() -> Curve {
    let mut c = Curve::default_of(ENVELOPE);
    c.points = vec![point(0.0, 0.0, 0.0), point(0.75, 1.0, 0.2), point(1.5, 0.5, 0.2), point(0.25, 0.0, 0.2)];
    c.decay = 1;
    c.sustain = 2;
    c.scales = [128, 128, 0, 128];
    c.keep_sytrus();
    c
}

/// The LFO default.
fn default_lfo() -> Curve {
    let mut c = Curve::default_of(LFO);
    c.points = vec![point(0.0, 0.5, 0.0), point(1.0, 0.5, 0.0), point(0.25, 1.0, 0.0), point(1.0, 0.5, 0.0)];
    c.lfo = [40000, 0, 0, 0, 0];
    c.keep_sytrus();
    c
}

/// A flat map at `level`.
pub fn flat_map(level: f64) -> Curve {
    let mut c = Curve::default_of(MAP);
    c.points = vec![point(0.0, level, 0.0), point(1.0, level, 0.0)];
    c.keep_sytrus();
    c
}

#[derive(Debug, Clone, PartialEq)]
pub struct Articulator {
    pub base: f32,
    pub env_amount: f32,
    pub lfo_amount: f32,
    pub env: Curve,
    pub lfo: Curve,
    pub maps: [Curve; 4],
}

impl Articulator {
    pub fn default_of(a: usize) -> Articulator {
        let l = map_default(a);
        Articulator { base: 0.0, env_amount: 1.0, lfo_amount: 1.0, env: default_env(), lfo: default_lfo(), maps: std::array::from_fn(|_| flat_map(l)) }
    }
}

/// The state, in the newest version's terms.
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub ints: [i32; INTS],
    pub reverb_on: bool,
    pub reverb_tempo: bool,
    pub quality: i32,
    pub smooth: bool,
    /// Link key to pitch, velocity to volume.
    pub key_pitch: u8,
    pub vel_vol: u8,
    /// Limit to one voice per key.
    pub one_voice: bool,
    /// Two bytes whose meaning was not found; kept.
    pub unknown_224: [u8; 2],
    pub declick: bool,
    pub link_release: bool,
    pub porta: bool,
    pub porta_variable: bool,
    pub porta_limit: u8,
    pub natural_scale: bool,
    /// The comb on each final map: forced, impulse, decay, release.
    pub combs: [i32; FINALS],
    pub alt_decay: bool,
    pub auto_loudness: bool,
    pub natural_weight: bool,
    /// An int and a byte whose meaning was not found; kept.
    pub unknown_252: i32,
    pub unknown_258: u8,
    pub delay_tempo: bool,
    pub delay_on: bool,
    pub eq_types: [i32; 3],
    /// "Title\rAuthor\rInfo\r" (the plain text of an old RTF comment).
    pub comment: String,
    pub arts: Vec<Articulator>,
    pub finals: [Curve; FINALS],
}

impl Default for State {
    fn default() -> Self {
        State {
            ints: DEFAULT_INTS,
            reverb_on: false,
            reverb_tempo: false,
            quality: 1,
            smooth: true,
            key_pitch: 1,
            vel_vol: 1,
            one_voice: true,
            unknown_224: [0, 0],
            declick: true,
            link_release: false,
            porta: false,
            porta_variable: true,
            porta_limit: 0,
            natural_scale: true,
            combs: [0; FINALS],
            alt_decay: false,
            auto_loudness: true,
            natural_weight: false,
            unknown_252: 430,
            unknown_258: 0,
            delay_tempo: true,
            delay_on: false,
            eq_types: [0, 1, 2],
            comment: "Title\rAuthor\rInfo\r".into(),
            arts: (0..ARTS).map(Articulator::default_of).collect(),
            finals: [flat_map(0.0), flat_map(0.5), flat_map(0.5), flat_map(0.5)],
        }
    }
}

/// The version among those measured that a state of version `v` is laid out like.
fn layout_version(v: u32) -> u32 {
    match v {
        0 => 0,
        1..=3 => 1,
        4..=6 => 4,
        7 => 7,
        8 => 8,
        9..=10 => 9,
        11 => 11,
        12 => 12,
        _ => 13,
    }
}

/// Where a layout keeps the EQ types (the delay's bytes are the three before).
fn eq_types_at(layout: u32) -> usize {
    match layout {
        0 => 226,
        1 => 227,
        4 => 230,
        7 => 231,
        8 => 235,
        9 => 251,
        11 => 252,
        12 => 253,
        _ => 259,
    }
}

fn read_i32(d: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_le_bytes(d.get(at..at + 4)?.try_into().ok()?))
}

/// An old comment's words: the RTF without its control words and its tables of fonts and colours.
fn rtf_text(b: &[u8]) -> String {
    let text: String = b.iter().map(|&c| c as char).collect();
    let mut out = String::new();
    let mut it = text.chars().peekable();
    let mut depth = 0i32;
    let mut skip = None;
    while let Some(c) = it.next() {
        match c {
            '{' => depth += 1,
            '}' => {
                if skip == Some(depth) {
                    skip = None;
                }
                depth -= 1;
            }
            '\\' => {
                let mut word = String::new();
                while it.peek().is_some_and(char::is_ascii_alphabetic) {
                    word.extend(it.next());
                }
                if word.is_empty() {
                    match it.next() {
                        Some('\u{27}') => {
                            let hex: String = it.by_ref().take(2).collect();
                            if skip.is_none()
                                && let Ok(v) = u8::from_str_radix(&hex, 16)
                            {
                                out.push(v as char);
                            }
                        }
                        Some(c) if skip.is_none() => out.push(c),
                        _ => {}
                    }
                    continue;
                }
                if it.peek().is_some_and(|c| *c == '-' || c.is_ascii_digit()) {
                    it.next();
                    while it.peek().is_some_and(char::is_ascii_digit) {
                        it.next();
                    }
                }
                if it.peek() == Some(&' ') {
                    it.next();
                }
                if skip.is_none() {
                    if matches!(word.as_str(), "fonttbl" | "colortbl" | "stylesheet" | "info") {
                        skip = Some(depth);
                    } else if matches!(word.as_str(), "par" | "line") {
                        out.push('\r');
                    }
                }
            }
            '\r' | '\n' | '\0' => {}
            c if skip.is_none() => out.push(c),
            _ => {}
        }
    }
    out
}

/// An old EQ map on the newest natural scale, as Ogun's own conversion has it: the old axis is harmonic / 511, read
/// at harmonics 1, 2, 4 .. 32768, one point per octave.
fn converted_map(old: &Curve) -> Curve {
    let mut c = Curve::default_of(MAP);
    c.points = (0..16)
        .map(|j| Point {
            dist: if j == 0 { 0.0 } else { 1.0 / 15.0 },
            level: map_at(old, (f64::from(1u32 << j) / 511.0).min(1.0), 0.0),
            ..Point::default()
        })
        .collect();
    c.keep_sytrus();
    c
}

impl State {
    /// A state from event 213's bytes: the version, then a zlib stream.
    pub fn from_bytes(b: &[u8]) -> Option<State> {
        let version = u32::from_le_bytes(b.get(..4)?.try_into().ok()?);
        let d = miniz_oxide::inflate::decompress_to_vec_zlib(b.get(4..)?).ok()?;
        State::from_stream(version, &d)
    }

    /// A state from its version and unpacked stream.
    pub fn from_stream(version: u32, d: &[u8]) -> Option<State> {
        let layout = layout_version(version);
        let mut st = State::default();
        // The ints: versions up to 7 lack fine pitch, and everything after sits 4 lower.
        let old = layout <= 7;
        for k in 0..INTS {
            let at = match (old, k) {
                (true, FINE) => {
                    st.ints[k] = 0;
                    continue;
                }
                (true, k) if k > FINE => 4 * (k - 1),
                (_, k) => 4 * k,
            };
            st.ints[k] = read_i32(d, at)?;
        }
        let shift = if old { 4 } else { 0 };
        st.reverb_on = *d.get(212 - shift)? != 0;
        st.reverb_tempo = *d.get(213 - shift)? != 0;
        st.quality = read_i32(d, 216 - shift)?;
        // The run of switches, 220 .. 232 in the newest version; each old version ends it sooner.
        let run_end = match layout {
            0 => 227,
            1 => 228,
            4 => 231,
            _ => 232,
        };
        let switch = |n: usize| -> Option<u8> { if 220 + n < run_end { d.get(220 + n - shift).copied() } else { None } };
        let def = State::default();
        st.smooth = switch(0).map_or(def.smooth, |v| v != 0);
        st.key_pitch = switch(1).unwrap_or(def.key_pitch);
        st.vel_vol = switch(2).unwrap_or(def.vel_vol);
        st.one_voice = switch(3).map_or(def.one_voice, |v| v != 0);
        st.unknown_224 = [switch(4).unwrap_or(0), switch(5).unwrap_or(0)];
        st.declick = switch(6).map_or(def.declick, |v| v != 0);
        st.link_release = switch(7).is_some_and(|v| v != 0);
        st.porta = switch(8).is_some_and(|v| v != 0);
        st.porta_variable = switch(9).map_or(def.porta_variable, |v| v != 0);
        st.porta_limit = switch(10).unwrap_or(0);
        st.natural_scale = switch(11).map_or(def.natural_scale, |v| v != 0);
        if layout >= 9 {
            st.combs = [read_i32(d, 232)?, read_i32(d, 236)?, read_i32(d, 240)?, read_i32(d, 244)?];
        }
        st.alt_decay = layout >= 11 && *d.get(248)? != 0;
        st.auto_loudness = layout >= 12 && *d.get(249)? != 0;
        st.natural_weight = layout >= 13 && *d.get(250)? != 0;
        if layout >= 13 {
            st.unknown_252 = read_i32(d, 252)?;
        }
        let t = eq_types_at(layout);
        st.delay_tempo = *d.get(t - 3)? != 0;
        st.delay_on = *d.get(t - 2)? != 0;
        st.unknown_258 = *d.get(t - 1)?;
        st.eq_types = [read_i32(d, t)?, read_i32(d, t + 4)?, read_i32(d, t + 8)?];
        // The comment.
        let mut r = Reader::new(d);
        r.bytes(t + 12)?;
        if version >= VERSION_TEXT {
            let n = usize::from(r.u8()?);
            let units: Vec<u16> = r.bytes(2 * n)?.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            st.comment = String::from_utf16_lossy(&units);
        } else {
            let n = r.i32()?.max(0) as usize;
            st.comment = rtf_text(r.bytes(n)?);
        }
        // The articulators and final maps.
        let order: Vec<usize> = match layout {
            0 | 1 => vec![0, 1, 2, 3, 6, 7, 8, 9, 10],
            4 | 7 | 8 | 9 => (0..11).collect(),
            _ => (0..ARTS).collect(),
        };
        for &a in &order {
            let _marker = r.i32()?;
            let (base, env_amount, lfo_amount) = (r.f32()?, r.f32()?, r.f32()?);
            let mut env = Curve::read(&mut r, ENVELOPE)?;
            let mut lfo = Curve::read(&mut r, LFO)?;
            env.keep_sytrus();
            lfo.keep_sytrus();
            let mut maps: [Curve; 4] = std::array::from_fn(|_| Curve::default_of(MAP));
            for m in maps.iter_mut() {
                *m = Curve::read(&mut r, MAP)?;
                m.keep_sytrus();
            }
            st.arts[a] = Articulator { base, env_amount, lfo_amount, env, lfo, maps };
        }
        if layout <= 4 {
            for f in [M_IMPULSE, M_DECAY, M_RELEASE] {
                let mut m = Curve::read(&mut r, MAP)?;
                m.keep_sytrus();
                st.finals[f] = converted_map(&m);
            }
            st.finals[M_FORCED] = flat_map(0.0);
        } else {
            for m in st.finals.iter_mut() {
                *m = Curve::read(&mut r, MAP)?;
                m.keep_sytrus();
            }
        }
        Some(st)
    }

    /// The state as event 213's bytes, the newest version.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = VERSION.to_le_bytes().to_vec();
        out.extend_from_slice(&miniz_oxide::deflate::compress_to_vec_zlib(&self.stream(), 6));
        out
    }

    /// The unpacked stream of the newest version.
    pub fn stream(&self) -> Vec<u8> {
        let mut w = Vec::with_capacity(16384);
        let int = |w: &mut Vec<u8>, v: i32| w.extend_from_slice(&v.to_le_bytes());
        for &v in &self.ints {
            int(&mut w, v);
        }
        w.extend_from_slice(&[u8::from(self.reverb_on), u8::from(self.reverb_tempo), 0, 0]);
        int(&mut w, self.quality);
        w.extend_from_slice(&[
            u8::from(self.smooth),
            self.key_pitch,
            self.vel_vol,
            u8::from(self.one_voice),
            self.unknown_224[0],
            self.unknown_224[1],
            u8::from(self.declick),
            u8::from(self.link_release),
            u8::from(self.porta),
            u8::from(self.porta_variable),
            self.porta_limit,
            u8::from(self.natural_scale),
        ]);
        for &v in &self.combs {
            int(&mut w, v);
        }
        w.extend_from_slice(&[u8::from(self.alt_decay), u8::from(self.auto_loudness), u8::from(self.natural_weight), 0]);
        int(&mut w, self.unknown_252);
        w.extend_from_slice(&[u8::from(self.delay_tempo), u8::from(self.delay_on), self.unknown_258]);
        for &v in &self.eq_types {
            int(&mut w, v);
        }
        let units: Vec<u16> = self.comment.encode_utf16().take(255).collect();
        w.push(units.len() as u8);
        for u in units {
            w.extend_from_slice(&u.to_le_bytes());
        }
        for art in &self.arts {
            int(&mut w, 3);
            for v in [art.base, art.env_amount, art.lfo_amount] {
                w.extend_from_slice(&v.to_le_bytes());
            }
            art.env.write(&mut w);
            art.lfo.write(&mut w);
            for m in &art.maps {
                m.write(&mut w);
            }
        }
        for m in &self.finals {
            m.write(&mut w);
        }
        w
    }

    /// The comment's title, author and info lines.
    pub fn comment_lines(&self) -> [String; 3] {
        let mut it = self.comment.split('\r').map(str::to_owned);
        [it.next().unwrap_or_default(), it.next().unwrap_or_default(), it.next().unwrap_or_default()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default's layout: the offsets of the spec's tables.
    #[test]
    fn the_newest_layout_has_its_offsets() {
        let st = State::default();
        let d = st.stream();
        assert_eq!(read_i32(&d, 4 * RICH), Some(14));
        assert_eq!(read_i32(&d, 216), Some(1), "quality");
        assert_eq!(d[220], 1, "smooth");
        assert_eq!(d[226], 1, "declick");
        assert_eq!(d[231], 1, "natural scale");
        assert_eq!(d[249], 1, "auto loudness");
        assert_eq!(read_i32(&d, 252), Some(430));
        assert_eq!(d[256], 1, "delay tempo");
        assert_eq!(read_i32(&d, 263), Some(1));
        assert_eq!(d[271], 18, "the comment's length");
        assert_eq!(read_i32(&d, 308), Some(3), "the first articulator's marker");
        assert_eq!(d.len(), 308 + 13 * 628 + 4 * 80);
    }

    #[test]
    fn a_comb_follows_its_law() {
        let on = |comb: i32| -> Vec<usize> { (1..=12).filter(|&k| comb_gate(comb, k)).collect() };
        assert_eq!(on(0), (1..=12).collect::<Vec<_>>());
        assert_eq!(on(1), [1, 3, 5, 7, 9, 11]);
        assert_eq!(on(4), [1, 2, 3, 4, 6, 7, 8, 9, 11, 12]);
        assert_eq!(on(5), [1, 4, 7, 10]);
        assert_eq!(on(7), [1, 6, 11]);
    }

    #[test]
    fn a_state_comes_back_as_it_was_written() {
        let mut st = State::default();
        st.ints[RICH] = 9;
        st.combs = [3, 1, 5, 7];
        st.alt_decay = true;
        st.comment = "A\rB\rC\r".into();
        st.arts[A_CUT].env.enabled = true;
        let back = State::from_bytes(&st.to_bytes()).expect("a state");
        assert_eq!(back.stream(), st.stream());
        assert_eq!(back.ints[RICH], 9);
        assert!(back.alt_decay && back.arts[A_CUT].env.enabled);
        assert_eq!(back.combs, [3, 1, 5, 7]);
        assert_eq!(back.comment_lines(), ["A", "B", "C"]);
    }

    #[test]
    fn broken_states_are_refused() {
        assert_eq!(State::from_bytes(&[1, 2, 3]), None);
        let mut b = VERSION.to_le_bytes().to_vec();
        b.extend_from_slice(&miniz_oxide::deflate::compress_to_vec_zlib(&[0u8; 100], 6));
        assert_eq!(State::from_bytes(&b), None);
    }

    /// Version 7's ints have no fine pitch, so the rest sit one lower; it has 11 articulators and an RTF comment.
    #[test]
    fn an_old_version_reads_into_the_new_form() {
        let st = State::default();
        let mut d = Vec::new();
        for k in 0..INTS {
            if k != FINE {
                d.extend_from_slice(&st.ints[k].to_le_bytes());
            }
        }
        d.extend_from_slice(&[1, 0, 0, 0]); // reverb on, tempo
        d.extend_from_slice(&2i32.to_le_bytes()); // quality
        d.extend_from_slice(&[0, 1, 1, 0, 0, 0, 1, 0, 0, 1, 0, 1]); // the switches
        d.extend_from_slice(&[1, 1, 0]); // delay tempo, on, unknown
        for t in [0i32, 1, 2] {
            d.extend_from_slice(&t.to_le_bytes());
        }
        let rtf = b"{\\rtf1\\ansi{\\fonttbl{\\f0 Arial;}}\\f0 Hello\\par World}\r\n\0";
        d.extend_from_slice(&(rtf.len() as i32).to_le_bytes());
        d.extend_from_slice(rtf);
        for a in 0..11 {
            let art = Articulator::default_of(a);
            d.extend_from_slice(&2i32.to_le_bytes());
            for v in [0f32, 1.0, 1.0] {
                d.extend_from_slice(&v.to_le_bytes());
            }
            art.env.write(&mut d);
            art.lfo.write(&mut d);
            for m in &art.maps {
                m.write(&mut d);
            }
        }
        flat_map(0.25).write(&mut d);
        for _ in 0..3 {
            flat_map(0.75).write(&mut d);
        }
        let got = State::from_stream(7, &d).expect("a state");
        assert_eq!(got.ints[FINE], 0);
        assert_eq!(got.ints[RICH], 14);
        assert_eq!(got.quality, 2);
        assert!(got.reverb_on && got.delay_on && !got.smooth && got.declick && !got.auto_loudness);
        assert_eq!(got.comment.trim_end(), "Hello\rWorld");
        assert_eq!(got.finals[M_FORCED].points[0].level, 0.25);
        // What the version lacks stays at its default.
        assert_eq!(got.arts[A_ROT].maps[MAP_KEY].points[0].level, 0.5);
    }

    /// Versions up to 4: three EQ maps on the old axis, resampled per octave.
    #[test]
    fn old_eq_maps_are_resampled() {
        let mut old = Curve::default_of(MAP);
        old.points = vec![point(0.0, 0.5, 0.0), point(1.0, 0.1875, 0.0)];
        old.keep_sytrus();
        let c = converted_map(&old);
        assert_eq!(c.points.len(), 16);
        let f = |x: f64| 0.5 - (0.5 - 0.1875) * x;
        for (j, want) in [(0, f(1.0 / 511.0)), (4, f(16.0 / 511.0)), (8, f(256.0 / 511.0)), (9, f(1.0)), (15, f(1.0))] {
            assert!((f64::from(c.points[j].level) - want).abs() < 1e-3, "{j}: {} vs {want}", c.points[j].level);
        }
    }

    #[test]
    fn rtf_comments_read_as_words() {
        assert_eq!(rtf_text(b"{\\rtf1{\\colortbl;\\red0;}Some \\'41 text\\par}"), "Some A text\r");
    }
}
