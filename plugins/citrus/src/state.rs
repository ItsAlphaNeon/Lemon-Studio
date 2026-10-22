// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Sytrus's saved state (a project's event 213, the same bytes as a preset's): an int, the stored version, then the
//! patch, zlib-packed from version 5 on. Every version from 5 up is read as far as it goes; what a version lacks
//! keeps the defaults.
//
// spec: plugins/sytrus.md ("The state", "A module", "Articulators", "Older states"), plugins/sytrus-effects.md
// ("Settings")

use crate::curve::{Curve, ENVELOPE, LFO, MAP};
use crate::num::Reader;

/// Operators 1..6, filters 1..3, module 10 (the effects bus's own articulators).
pub const OPERATORS: usize = 6;
pub const FILTERS: usize = 3;
pub const MODULES: usize = 10;
/// The matrix's columns 7, 8, 9: the row's pan, effects send and output level.
pub const COL_PAN: usize = 6;
pub const COL_SEND: usize = 7;
pub const COL_OUT: usize = 8;

/// The operators' articulators, in order.
pub const OP_PAN: usize = 0;
pub const OP_VOLUME: usize = 1;
pub const OP_MOD: usize = 2;
pub const OP_PITCH: usize = 3;
pub const OP_PHASE: usize = 4;
pub const OP_DAMP: usize = 5;
/// The filters' articulators, in order.
pub const F_PAN: usize = 0;
pub const F_VOLUME: usize = 1;
pub const F_CUTOFF: usize = 2;
pub const F_RES: usize = 3;
pub const F_LOW: usize = 4;
pub const F_BAND: usize = 5;
pub const F_HIGH: usize = 6;
pub const F_SHAPER: usize = 7;

/// The six maps of an articulator.
pub const MAP_KEY: usize = 0;
pub const MAP_VEL: usize = 1;
pub const MAP_X: usize = 2;
pub const MAP_Y: usize = 3;
pub const MAP_RANDOM: usize = 4;
pub const MAP_UNISON: usize = 5;

/// Switches (offset 966 + n): their values when the state has none.
pub const SWITCH_DEFAULTS: [u8; 12] = [0, 0, 0, 0, 0, 1, 0, 0, 1, 0, 1, 0];

#[derive(Debug, Clone, PartialEq)]
pub struct Articulator {
    pub base: f32,
    pub env_amount: f32,
    pub env: Curve,
    pub lfo: Curve,
    pub maps: [Curve; 6],
}

impl Default for Articulator {
    fn default() -> Self {
        Articulator {
            base: 0.0,
            env_amount: 0.0,
            env: Curve::default_of(ENVELOPE),
            lfo: Curve::default_of(LFO),
            maps: std::array::from_fn(|_| Curve::default_of(MAP)),
        }
    }
}

impl Articulator {
    /// The articulator a state lacks (Lemon Studio choice; FL's own defaults were not measured): envelope and LFO off,
    /// every map flat at its default level (1 on product articulators, 0.5 on bipolar ones), so it takes no part.
    pub fn neutral(module: usize, art: usize) -> Self {
        let level = if crate::patch::bipolar(module, art) { 0.5 } else { 1.0 };
        let map = || {
            let mut c = Curve::default_of(MAP);
            c.points[0].level = level;
            c.prepare();
            c
        };
        Articulator { maps: std::array::from_fn(|_| map()), ..Articulator::default() }
    }
}

/// An operator's block: frequency, noise, 256 harmonics, the shape and the flags.
#[derive(Debug, Clone, PartialEq)]
pub struct OpBlock {
    /// 10000 = ratio 1.
    pub ratio: i32,
    /// 0.01 Hz.
    pub offset: i32,
    pub noise: i32,
    pub levels: Vec<f32>,
    pub phases: Vec<f32>,
    /// Shape (0..512), tension (-128..128), skew (0..128), sine shaper (0..128), pre-filter (0..128).
    pub shape: [i32; 5],
    /// Half wave, two-cycle gate, absolute value, free-running phase, DC removal, attack smoothing, plucked string,
    /// band limiting.
    pub flags: [u8; 8],
}

impl Default for OpBlock {
    fn default() -> Self {
        OpBlock { ratio: 10000, offset: 0, noise: 0, levels: vec![0.0; 256], phases: vec![0.0; 256], shape: [0; 5], flags: [0; 8] }
    }
}

pub const HALF_WAVE: usize = 0;
pub const GATE: usize = 1;
pub const ABSOLUTE: usize = 2;
pub const FREE_RUN: usize = 3;
pub const DC_REMOVAL: usize = 4;
pub const SMOOTHING: usize = 5;
pub const PLUCKED: usize = 6;
pub const BAND_LIMIT: usize = 7;

/// A filter's block: oversampling, track, wave shaper on, type; its shaper curve and whether it is bipolar.
#[derive(Debug, Clone, PartialEq)]
pub struct FilterBlock {
    pub oversample: bool,
    pub track: bool,
    pub shaper_on: bool,
    pub kind: u8,
    pub shaper: Curve,
    pub bipolar: bool,
}

impl Default for FilterBlock {
    fn default() -> Self {
        FilterBlock { oversample: false, track: false, shaper_on: false, kind: 0, shaper: Curve::default_of(MAP), bipolar: false }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Module {
    pub header: [i32; 10],
    pub op: OpBlock,
    pub filter: FilterBlock,
    pub arts: Vec<Articulator>,
}

/// A delay (23 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Delay {
    pub feedback: i32,
    pub time: i32,
    pub stereo: i32,
    pub level: i32,
    pub mode: i32,
    pub synced: bool,
    pub serial: bool,
    pub on: bool,
}

/// The reverb: low cut, high cut, pre-delay, room size, diffusion, decay, high damping, colour, wet; on; tempo-based
/// pre-delay.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReverbSettings {
    pub knobs: [i32; 9],
    pub on: bool,
    pub tempo: bool,
}

impl Default for ReverbSettings {
    fn default() -> Self {
        ReverbSettings { knobs: [75, 40, 0, 40, 100, 15, 40, 2, 25], on: false, tempo: false }
    }
}

/// The equaliser: mode (0 off, 1 the whole output, 2 the dry bus, 3 the effects bus), then per band gain (0.01 dB),
/// frequency, width, type (0 low shelf, 1 peaking, 2 high shelf).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EqSettings {
    pub mode: i32,
    pub bands: [[i32; 4]; 3],
}

impl Default for EqSettings {
    fn default() -> Self {
        EqSettings { mode: 0, bands: [[0, 5777, 17500, 0], [0, 33145, 17500, 1], [0, 55825, 17500, 2]] }
    }
}

/// Where a state's parts lie in its unpacked stream (byte offsets), for editing a state in place.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Layout {
    pub soften: Option<usize>,
    pub switches: usize,
    pub delays: usize,
    pub reverb: Option<usize>,
    pub eq: Option<usize>,
    pub name: usize,
    /// Per module: its header, its operator or filter block, and each articulator's start.
    pub modules: Vec<ModuleLayout>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModuleLayout {
    pub header: usize,
    pub block: usize,
    pub arts: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub version: i32,
    /// 128 = unity.
    pub main_volume: i32,
    pub master_lfo: i32,
    /// Semitones.
    pub main_pitch: i32,
    /// 9 rows (modules) of 9 columns, 256 = 100 %.
    pub matrix: [i32; 81],
    pub ring: [i32; 81],
    pub s: [i32; 35],
    pub cell_off: [u8; 162],
    pub soften: i32,
    pub switches: [u8; 12],
    pub delays: [Delay; 3],
    pub reverb: ReverbSettings,
    pub eq: EqSettings,
    pub name: String,
    pub modules: Vec<Module>,
    pub layout: Layout,
}

impl Default for State {
    fn default() -> Self {
        let modules = (0..MODULES)
            .map(|m| Module { arts: (0..arts_of(m)).map(|a| Articulator::neutral(m, a)).collect(), ..Module::default() })
            .collect();
        State {
            version: 25,
            main_volume: 128,
            master_lfo: 0,
            main_pitch: 0,
            matrix: [0; 81],
            ring: [0; 81],
            s: [0; 35],
            cell_off: [0; 162],
            soften: 0,
            switches: SWITCH_DEFAULTS,
            delays: [Delay::default(); 3],
            reverb: ReverbSettings::default(),
            eq: EqSettings::default(),
            name: String::new(),
            modules,
            layout: Layout::default(),
        }
    }
}

/// Articulators per module: six for an operator, eight for a filter, two for module 10.
pub fn arts_of(module: usize) -> usize {
    match module {
        0..=5 => 6,
        6..=8 => 8,
        _ => 2,
    }
}

/// Switches present by version: 0 from 6, 1 from 7, 2 from 13, 3 from 14, 4..7 from 15, 8..10 from 18, 11 from 19.
fn switch_count(v: i32) -> usize {
    match v {
        19.. => 12,
        18 => 11,
        15..=17 => 8,
        14 => 4,
        13 => 3,
        7..=12 => 2,
        6 => 1,
        _ => 0,
    }
}

impl State {
    /// A matrix cell's stored value, 0 when its "cell off" byte is set.
    pub fn cell(&self, row: usize, col: usize) -> i32 {
        if self.cell_off[row * 9 + col] != 0 { 0 } else { self.matrix[row * 9 + col] }
    }

    pub fn ring_cell(&self, row: usize, col: usize) -> i32 {
        if self.cell_off[81 + row * 9 + col] != 0 { 0 } else { self.ring[row * 9 + col] }
    }

    pub fn switch(&self, n: usize) -> u8 {
        self.switches[n]
    }

    /// `s[32]`'s switch byte `k` (0 mod X / Y smoothing, 1 random maps applied, 2 global pitch).
    pub fn s32(&self, k: usize) -> bool {
        (self.s[32] >> (8 * k)) & 0xFF != 0
    }

    /// A state's stored version int and its unpacked stream (what `layout` points into).
    pub fn unpack(b: &[u8]) -> Option<(i32, Vec<u8>)> {
        let stored = i32::from_le_bytes(b.get(0..4)?.try_into().ok()?);
        let version = if stored > 0xFFFF { stored >> 16 } else { stored };
        let body = if version >= 5 { miniz_oxide::inflate::decompress_to_vec_zlib(&b[4..]).ok()? } else { b[4..].to_vec() };
        Some((stored, body))
    }

    /// The stored bytes for a version int and an unpacked stream.
    pub fn pack(stored: i32, body: &[u8]) -> Vec<u8> {
        let version = if stored > 0xFFFF { stored >> 16 } else { stored };
        let mut out = stored.to_le_bytes().to_vec();
        if version >= 5 {
            out.extend(miniz_oxide::deflate::compress_to_vec_zlib(body, 6));
        } else {
            out.extend_from_slice(body);
        }
        out
    }

    /// Reads a state: the stored version, then the patch. `None` when the version is not read (`0x7FFFFFFF`, or
    /// `0x00190004` and above) or the packed stream does not unpack.
    pub fn from_bytes(b: &[u8]) -> Option<State> {
        let mut r = Reader::new(b);
        let stored = r.i32()?;
        if stored == 0x7FFF_FFFF || stored >= 0x0019_0004 || stored < 0 {
            return None;
        }
        let version = if stored > 0xFFFF { stored >> 16 } else { stored };
        let unpacked;
        let body: &[u8] = if version >= 5 {
            unpacked = miniz_oxide::inflate::decompress_to_vec_zlib(&b[4..]).ok()?;
            &unpacked
        } else {
            &b[4..]
        };
        let mut st = State { version, ..State::default() };
        st.read(&mut Reader::new(body));
        Some(st)
    }

    /// Reads the patch as far as it goes.
    fn read(&mut self, r: &mut Reader) -> Option<()> {
        let v = self.version;
        self.main_volume = r.i32()?;
        self.master_lfo = r.i32()?;
        self.main_pitch = r.i32()?;
        for x in self.matrix.iter_mut() {
            *x = r.i32()?;
        }
        for x in self.ring.iter_mut() {
            *x = r.i32()?;
        }
        for x in self.s.iter_mut() {
            *x = r.i32()?;
        }
        if v >= 2 {
            self.cell_off.copy_from_slice(r.bytes(162)?);
        }
        if v >= 4 {
            self.layout.soften = Some(r.at);
            self.soften = r.i32()?;
        }
        self.layout.switches = r.at;
        for k in 0..switch_count(v) {
            self.switches[k] = r.u8()?;
        }
        self.layout.delays = r.at;
        for d in self.delays.iter_mut() {
            let i = r.ints(5)?;
            let b = r.bytes(3)?;
            *d = Delay {
                feedback: i[0],
                time: i[1],
                stereo: i[2],
                level: i[3],
                mode: i[4],
                synced: b[0] != 0,
                serial: b[1] != 0,
                on: b[2] != 0,
            };
        }
        if v >= 10 {
            self.layout.reverb = Some(r.at);
            let k = r.ints(9)?;
            self.reverb.knobs.copy_from_slice(&k);
            self.reverb.on = r.u8()? != 0;
            if v >= 11 {
                self.reverb.tempo = r.u8()? != 0;
            }
        }
        if v >= 3 {
            self.layout.eq = Some(r.at);
            self.eq.mode = r.i32()?;
            for (k, band) in self.eq.bands.iter_mut().enumerate() {
                let n = if v >= 14 { 4 } else { 3 };
                let i = r.ints(n)?;
                band[..3].copy_from_slice(&i[..3]);
                band[3] = if v >= 14 { i[3] } else { k as i32 };
            }
        }
        self.layout.name = r.at;
        self.name = if v >= 23 {
            let mut n = usize::from(r.u8()?);
            if n == 255 {
                n = r.i32()?.max(0) as usize;
            }
            let b = r.bytes(n * 2)?;
            let u: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            String::from_utf16_lossy(&u)
        } else {
            let n = r.i32()?.max(0) as usize;
            r.bytes(n)?.iter().map(|&c| char::from(c)).collect()
        };
        for m in 0..MODULES {
            let mut module = Module { arts: Vec::new(), ..Module::default() };
            let mut ml = ModuleLayout { header: r.at, ..ModuleLayout::default() };
            let h = r.ints(10)?;
            ml.block = r.at;
            module.header.copy_from_slice(&h);
            if m < OPERATORS {
                module.op = read_op(r, v)?;
            } else if m < OPERATORS + FILTERS {
                let n = if v >= 9 { 48 } else { 41 };
                let b = r.bytes(n)?;
                let f = &mut module.filter;
                f.oversample = b[0] != 0;
                f.track = b[1] != 0;
                f.shaper_on = b[40] != 0;
                f.kind = if v >= 9 { b[44] } else { 0 };
                let mut c = Curve::read(r, MAP)?;
                c.keep_sytrus();
                f.shaper = c;
                if v >= 12 {
                    f.bipolar = r.u8()? != 0;
                }
            }
            for _ in 0..arts_of(m) {
                ml.arts.push(r.at);
                match read_articulator(r) {
                    Some(a) => module.arts.push(a),
                    None => {
                        while module.arts.len() < arts_of(m) {
                            module.arts.push(Articulator::neutral(m, module.arts.len()));
                        }
                        self.modules[m] = module;
                        self.layout.modules.push(ml);
                        return None;
                    }
                }
            }
            self.modules[m] = module;
            self.layout.modules.push(ml);
        }
        Some(())
    }
}

fn read_op(r: &mut Reader, v: i32) -> Option<OpBlock> {
    let mut o = OpBlock::default();
    if v < 8 {
        o.ratio = r.i32()?;
        o.offset = r.i32()?;
        for k in 0..24 {
            o.levels[k] = r.f32()?;
        }
        for k in 0..24 {
            o.phases[k] = r.f32()?;
        }
        for x in o.shape.iter_mut() {
            *x = r.i32()?;
        }
        o.noise = r.i32()?;
        o.flags[..7].copy_from_slice(r.bytes(7)?);
    } else {
        let h = if v >= 17 { 256 } else { 128 };
        o.ratio = r.i32()?;
        o.offset = r.i32()?;
        o.noise = r.i32()?;
        for k in 0..h {
            o.levels[k] = r.f32()?;
        }
        for k in 0..h {
            o.phases[k] = r.f32()?;
        }
        for x in o.shape.iter_mut() {
            *x = r.i32()?;
        }
        o.flags.copy_from_slice(r.bytes(8)?);
    }
    Some(o)
}

fn read_articulator(r: &mut Reader) -> Option<Articulator> {
    let _kind = r.i32()?;
    let base = r.f32()?;
    let env_amount = r.f32()?;
    let _lfo_amount = r.f32()?;
    let mut env = Curve::read(r, ENVELOPE)?;
    env.keep_sytrus();
    let mut lfo = Curve::read(r, LFO)?;
    lfo.keep_sytrus();
    let mut maps: [Curve; 6] = std::array::from_fn(|_| Curve::default_of(MAP));
    for m in maps.iter_mut() {
        let mut c = Curve::read(r, MAP)?;
        c.keep_sytrus();
        *m = c;
    }
    Some(Articulator { base, env_amount, env, lfo, maps })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The factory Default preset (FL's install, when present) reads to its last module.
    #[test]
    fn reads_the_default_preset() {
        let path = r"C:\Program Files\Image-Line\FL Studio 2026\Plugins\Fruity\Generators\Sytrus\Data\Default.dat";
        let Ok(b) = std::fs::read(path) else { return };
        let st = State::from_bytes(&b).expect("state");
        assert!(st.version >= 5);
        assert_eq!(st.modules.len(), MODULES);
        assert!(st.modules.iter().enumerate().all(|(m, x)| x.arts.len() == arts_of(m)));
    }
}
