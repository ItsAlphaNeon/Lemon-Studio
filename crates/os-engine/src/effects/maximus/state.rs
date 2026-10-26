// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Maximus's saved state: an int (the stored version), a block with the 59 parameters, their A/B twin, the display
//! settings and options, from version 14 an extension with flags, from version 15 four more bytes, then eight maps
//! (per band the map in use and the twin's). Versions 1 and 2 are laid out differently and are read into the same
//! form; they are written back in version 14's layout.
//!
// spec: plugins/maximus.md ("Parameters", "The state")

use os_dsp::num::Reader;

pub const PARAMS: usize = 59;
pub const BANDS: usize = 4;
pub const LOW: usize = 0;
pub const MID: usize = 1;
pub const HIGH: usize = 2;
pub const MASTER: usize = 3;

// The global parameters.
pub const P_LOW_FREQ: usize = 0;
pub const P_LOW_SLOPE: usize = 1;
pub const P_HIGH_FREQ: usize = 2;
pub const P_HIGH_SLOPE: usize = 3;
pub const P_LMH_DELAY: usize = 4;
pub const P_LMH_MIX: usize = 5;
pub const P_LOW_CUT: usize = 6;

// A band's parameters, at `band(b, k)`.
pub const B_MODE: usize = 0;
pub const B_PRE: usize = 1;
pub const B_POST: usize = 2;
pub const B_SEP: usize = 3;
pub const B_THRES: usize = 4;
pub const B_CEIL: usize = 5;
pub const B_ATT: usize = 6;
pub const B_REL2: usize = 7;
pub const B_ATT_STAGES: usize = 8;
pub const B_REL: usize = 9;
pub const B_REL_STAGES: usize = 10;
pub const B_WINDOW: usize = 11;
pub const B_RMS: usize = 12;

/// Band modes: compressor on, compressor off, muted, not split off.
pub const MODE_ON: i32 = 0;
pub const MODE_COMP_OFF: i32 = 1;
pub const MODE_MUTED: i32 = 2;
pub const MODE_OFF: i32 = 3;

/// Parameter `k` of band `b`.
pub const fn band(b: usize, k: usize) -> usize {
    7 + 13 * b + k
}

/// Each parameter's range.
pub fn range(k: usize) -> (i32, i32) {
    match k {
        P_LOW_FREQ | P_HIGH_FREQ => (0, 65536),
        P_LOW_SLOPE | P_HIGH_SLOPE => (0, 1),
        P_LMH_DELAY => (0, 10000),
        P_LMH_MIX => (0, 1000),
        P_LOW_CUT => (-1, 65536),
        _ => match (k - 7) % 13 {
            B_MODE => (0, 3),
            B_PRE | B_POST | B_CEIL => (0, 2000),
            B_SEP | B_THRES => (-1000, 1000),
            B_ATT_STAGES | B_REL_STAGES => (1, 8),
            B_RMS => (0, 1),
            _ => (0, 10000),
        },
    }
}

/// The version Lemon Studio writes for new states (the version of the factory default, stored as 0xE0003).
pub const VERSION: i32 = 0x000E_0003;

// Offsets in the block that follows the 59 parameters and their twin (the block's own offsets 0x1DC.. less 472).
pub const BL_SHOWN: usize = 4;
pub const BL_SOLO: usize = 19;
pub const BL_OVERSAMPLING: usize = 24;
pub const BL_MID_TRIGGER: usize = 32;
pub const BL_FIR: usize = 33;
/// The block's length past the parameters from version 7 on.
pub const BLOCK_REST: usize = 34;
/// Bit 6 of the extension's byte 8: the maps' 0 dB comes out at 1.0 instead of 0.975.
pub const FLAG_FULL_CEILING: u8 = 0x40;
/// Bit 0 of the extension's byte 8: version 14's linear-phase mode (its editor's LIN button; not played).
pub const FLAG_LINEAR_PHASE: u8 = 0x01;

/// A map point: its distance from the point before and its level (doubles, the level used as a single), the
/// tension of the segment that ends on it, that segment's mode and three more bytes kept as stored.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapPoint {
    pub dist: f64,
    pub level: f64,
    pub tension: f32,
    pub mode: u8,
    pub rest: [u8; 3],
}

impl MapPoint {
    pub fn new(dist: f64, level: f64) -> MapPoint {
        MapPoint { dist, level, tension: 0.0, mode: 0, rest: [0; 3] }
    }
}

/// A compression map: levels on the scale `log3(1 + 2 level) / 2` (0.5 = 0 dB) against the same scale.
#[derive(Debug, Clone, PartialEq)]
pub struct Map {
    pub kind: i32,
    pub sub: i32,
    pub points: Vec<MapPoint>,
    pub tail: [u8; 20],
}

impl Map {
    /// The factory map: rising one to one up to 0 dB, then flat.
    pub fn default_map() -> Map {
        let mut p = vec![MapPoint::new(0.0, 0.0), MapPoint::new(0.5, 0.5), MapPoint::new(0.5, 0.5)];
        p[1].rest[2] = 2;
        p[2].rest[2] = 2;
        let mut tail = [0xff; 20];
        tail[..4].fill(0);
        Map { kind: 3, sub: 3, points: p, tail }
    }

    fn read(r: &mut Reader) -> Option<Map> {
        let kind = r.i32()?;
        let sub = r.i32()?;
        let n = r.i32()?;
        if !(0..=100_000).contains(&n) {
            return None;
        }
        let mut points = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let dist = r.f64()?;
            let level = r.f64()?;
            let tension = r.f32()?;
            let mode = r.u8()?;
            let rest = r.bytes(3)?.try_into().ok()?;
            points.push(MapPoint { dist, level, tension, mode, rest });
        }
        let tail = r.bytes(20)?.try_into().ok()?;
        Some(Map { kind, sub, points, tail })
    }

    fn write(&self, w: &mut Vec<u8>) {
        w.extend_from_slice(&self.kind.to_le_bytes());
        w.extend_from_slice(&self.sub.to_le_bytes());
        w.extend_from_slice(&(self.points.len() as i32).to_le_bytes());
        for p in &self.points {
            w.extend_from_slice(&p.dist.to_le_bytes());
            w.extend_from_slice(&p.level.to_le_bytes());
            w.extend_from_slice(&p.tension.to_le_bytes());
            w.push(p.mode);
            w.extend_from_slice(&p.rest);
        }
        w.extend_from_slice(&self.tail);
    }

    /// Each point's place along the input axis (0..1, the distances scaled to add up to 1) and its level.
    pub fn places(&self) -> Vec<(f64, f64)> {
        let total: f64 = self.points.iter().map(|p| p.dist.max(0.0)).sum();
        let k = if total > 0.0 && total != 1.0 { 1.0 / total } else { 1.0 };
        let mut x = 0.0;
        self.points.iter().map(|p| {
            x += p.dist.max(0.0) * k;
            (x, f64::from(p.level as f32))
        })
        .collect()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct State {
    /// The version int as stored.
    pub stored_version: i32,
    pub params: [i32; PARAMS],
    /// The A/B twin ("spare state").
    pub twin: [i32; PARAMS],
    /// The block past the parameters and their twin, as long as the version writes it.
    pub rest: Vec<u8>,
    /// From version 14: 29 bytes, flags at byte 8.
    pub ext: Option<[u8; 29]>,
    /// From version 15: 4 more bytes.
    pub ext15: Option<[u8; 4]>,
    /// Per band: the map in use, then the twin's (index `2 b` and `2 b + 1`).
    pub maps: Vec<Map>,
}

/// A band's factory settings: on, 0 dB gains, no separation or saturation, 2 ms attack, release 2 as the attack, two
/// attack stages, about 86 ms release with three stages, a 10 ms peak window.
const BAND_DEFAULT: [i32; 13] = [0, 1000, 1000, 0, 0, 1000, 1115, 0, 2, 5000, 3, 2882, 0];

impl State {
    /// The factory default: 200 Hz and 3 kHz at 24 dB, 2 ms band look-ahead, full mix, low cut at 20 Hz, every band
    /// on with the 0 dB limiting map; version 14's layout.
    pub fn new_default() -> State {
        let mut p = [0; PARAMS];
        p[..7].copy_from_slice(&[19800, 1, 48065, 1, 1115, 1000, 32768]);
        for b in 0..BANDS {
            p[band(b, 0)..band(b, 0) + 13].copy_from_slice(&BAND_DEFAULT);
        }
        // The display settings as the factory default stores them (band shown: MASTER).
        let mut rest = vec![0; BLOCK_REST];
        rest[0] = 0x32;
        rest[BL_SHOWN] = 3;
        for k in [13, 14, 16, 18, 21] {
            rest[k] = 1;
        }
        let mut ext = [0; 29];
        ext[0] = 2;
        ext[8..12].copy_from_slice(&[0x14, 3, 0, 0]);
        ext[13] = 1;
        ext[17] = 2;
        ext[21] = 2;
        ext[25] = 2;
        State { stored_version: VERSION, params: p, twin: p, rest, ext: Some(ext), ext15: None, maps: vec![Map::default_map(); 2 * BANDS] }
    }

    /// The version number: the upper half of a stored value of 0x10000 or more.
    pub fn version(&self) -> i32 {
        version_of(self.stored_version)
    }

    pub fn from_bytes(b: &[u8]) -> Option<State> {
        let mut r = Reader::new(b);
        let stored = r.i32()?;
        let v = version_of(stored);
        if v < 1 {
            return None;
        }
        let mut st = State { stored_version: stored, params: [0; PARAMS], twin: [0; PARAMS], rest: Vec::new(), ext: None, ext15: None, maps: Vec::new() };
        if v <= 2 {
            // Parameters 0..5 are the ints 1..6, no low cut (on at 32768), parameters 7..58 are the ints 7..58; the
            // display block starts at byte 468 and the maps 21 or 22 bytes after it.
            let ints = r.ints(58)?;
            st.params[..6].copy_from_slice(&ints[..6]);
            st.params[P_LOW_CUT] = 32768;
            st.params[7..].copy_from_slice(&ints[6..]);
            let disp = b.get(468..468 + if v == 1 { 21 } else { 22 })?;
            st.rest = vec![0; BLOCK_REST];
            st.rest[BL_SHOWN..BL_SHOWN + 4].copy_from_slice(&disp[4..8]);
            st.rest[BL_SOLO] = disp[0x13];
            st.twin = st.params;
            st.stored_version = VERSION;
            st.ext = State::new_default().ext;
            let mut r = Reader::new(&b[468 + disp.len()..]);
            st.maps = read_maps(&mut r)?;
            return Some(st);
        }
        let len = block_len(v) - 472;
        let ints = r.ints(2 * PARAMS)?;
        st.params.copy_from_slice(&ints[..PARAMS]);
        st.twin.copy_from_slice(&ints[PARAMS..]);
        st.rest = r.bytes(len)?.to_vec();
        if v >= 14 {
            st.ext = Some(r.bytes(29)?.try_into().ok()?);
        }
        if v >= 15 {
            st.ext15 = Some(r.bytes(4)?.try_into().ok()?);
        }
        st.maps = read_maps(&mut r)?;
        Some(st)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Vec::with_capacity(1400);
        w.extend_from_slice(&self.stored_version.to_le_bytes());
        for v in self.params.iter().chain(&self.twin) {
            w.extend_from_slice(&v.to_le_bytes());
        }
        let len = block_len(self.version().max(3)) - 472;
        let mut rest = self.rest.clone();
        rest.resize(len, 0);
        w.extend_from_slice(&rest);
        if self.version() >= 14 {
            w.extend_from_slice(&self.ext.unwrap_or_default());
        }
        if self.version() >= 15 {
            w.extend_from_slice(&self.ext15.unwrap_or_default());
        }
        for m in &self.maps {
            m.write(&mut w);
        }
        w
    }

    fn rest_i32(&self, o: usize) -> i32 {
        let mut b = [0; 4];
        for (k, x) in b.iter_mut().enumerate() {
            *x = self.rest.get(o + k).copied().unwrap_or(0);
        }
        i32::from_le_bytes(b)
    }

    fn set_rest_i32(&mut self, o: usize, v: i32) {
        if self.rest.len() < o + 4 {
            self.rest.resize(o + 4, 0);
        }
        self.rest[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }

    fn rest_u8(&self, o: usize) -> u8 {
        self.rest.get(o).copied().unwrap_or(0)
    }

    fn set_rest_u8(&mut self, o: usize, v: u8) {
        if self.rest.len() <= o {
            self.rest.resize(o + 1, 0);
        }
        self.rest[o] = v;
    }

    /// The band shown in the editor (0..3).
    pub fn shown(&self) -> usize {
        self.rest_i32(BL_SHOWN).clamp(0, 3) as usize
    }

    pub fn set_shown(&mut self, b: usize) {
        self.set_rest_i32(BL_SHOWN, b as i32);
    }

    pub fn solo(&self) -> bool {
        self.rest_u8(BL_SOLO) != 0
    }

    pub fn set_solo(&mut self, on: bool) {
        self.set_rest_u8(BL_SOLO, u8::from(on));
    }

    /// Oversampling stages, 0..4 (2 to 16 times).
    pub fn oversampling(&self) -> u32 {
        self.rest_i32(BL_OVERSAMPLING).clamp(0, 4) as u32
    }

    pub fn set_oversampling(&mut self, stages: u32) {
        self.set_rest_i32(BL_OVERSAMPLING, stages.min(4) as i32);
    }

    pub fn mid_trigger(&self) -> bool {
        self.rest_u8(BL_MID_TRIGGER) != 0
    }

    pub fn set_mid_trigger(&mut self, on: bool) {
        self.set_rest_u8(BL_MID_TRIGGER, u8::from(on));
    }

    /// The linear-phase (FIR) crossover.
    pub fn fir(&self) -> bool {
        self.rest_u8(BL_FIR) != 0
    }

    pub fn set_fir(&mut self, on: bool) {
        self.set_rest_u8(BL_FIR, u8::from(on));
    }

    /// The maps' 0 dB comes out at 1.0 (version 14's flag) instead of 0.975 (the "0.2 dB compressor safety").
    pub fn full_ceiling(&self) -> bool {
        self.ext.is_some_and(|e| e[8] & FLAG_FULL_CEILING != 0)
    }

    /// Version 14's linear-phase mode (stored, not played).
    pub fn linear_phase(&self) -> bool {
        self.ext.is_some_and(|e| e[8] & FLAG_LINEAR_PHASE != 0)
    }

    /// Sets the full ceiling; a state older than version 14 has no place for it and stays at 0.975.
    pub fn set_full_ceiling(&mut self, on: bool) {
        if let Some(e) = self.ext.as_mut() {
            e[8] = if on { e[8] | FLAG_FULL_CEILING } else { e[8] & !FLAG_FULL_CEILING };
        }
    }

    /// The map band `b` uses.
    pub fn map(&self, b: usize) -> &Map {
        &self.maps[2 * b]
    }

    pub fn map_mut(&mut self, b: usize) -> &mut Map {
        &mut self.maps[2 * b]
    }

    /// Parameter `k` set to `v`, kept in its range.
    pub fn set_param(&mut self, k: usize, v: i32) {
        let (lo, hi) = range(k);
        self.params[k] = v.clamp(lo, hi);
    }

    /// Stores the settings in use as the spare state (the parameters and the maps).
    pub fn store_spare(&mut self) {
        self.twin = self.params;
        for b in 0..BANDS {
            self.maps[2 * b + 1] = self.maps[2 * b].clone();
        }
    }

    /// Swaps the settings in use with the spare state (A/B compare).
    pub fn flip_spare(&mut self) {
        std::mem::swap(&mut self.params, &mut self.twin);
        for b in 0..BANDS {
            self.maps.swap(2 * b, 2 * b + 1);
        }
    }
}

fn version_of(stored: i32) -> i32 {
    if stored >= 0x10000 { stored >> 16 } else { stored }
}

/// The block's length (parameters, twin and the rest) in version `v` (3 and later).
fn block_len(v: i32) -> usize {
    match v {
        ..=3 => 494,
        4 => 497,
        5 => 501,
        6 => 505,
        _ => 506,
    }
}

fn read_maps(r: &mut Reader) -> Option<Vec<Map>> {
    (0..2 * BANDS).map(|_| Map::read(r)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_round_trips() {
        let st = State::new_default();
        let b = st.to_bytes();
        assert_eq!(b.len(), 1371);
        assert_eq!(State::from_bytes(&b), Some(st.clone()));
        assert_eq!(st.version(), 14);
        assert_eq!(st.shown(), MASTER);
        assert!(!st.full_ceiling());
        assert_eq!(st.map(LOW).places(), vec![(0.0, 0.0), (0.5, 0.5), (1.0, 0.5)]);
    }

    #[test]
    fn spare_states_flip() {
        let mut st = State::new_default();
        st.store_spare();
        st.set_param(band(MID, B_PRE), 1500);
        st.map_mut(MID).points[1].level = 0.4;
        st.flip_spare();
        assert_eq!(st.params[band(MID, B_PRE)], 1000);
        assert_eq!(st.twin[band(MID, B_PRE)], 1500);
        assert_eq!(st.map(MID).points[1].level, 0.5);
        st.flip_spare();
        assert_eq!(st.map(MID).points[1].level, 0.4);
        st.set_param(band(MID, B_MODE), 9);
        assert_eq!(st.params[band(MID, B_MODE)], 3);
    }
}
