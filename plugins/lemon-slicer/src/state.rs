// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The original Fruity Slicer's saved state (a project channel's plugin state): a 32-byte header (version, the loop's
//! beats and tempo, the settings), the sample's path, the slices (name, start, root key, a float, flags) and a block of
//! settings with the channel's envelope records after it. Parsing and writing give the same bytes back; the bytes
//! whose meaning is not known are kept as they were read.
//!
// spec: plugins/fruity-slicer.md §1 (saved state)

/// The version every measured state carries.
pub const VERSION: u32 = 16;

/// Byte offsets of the header's fields.
pub const H_BEATS: usize = 4;
pub const H_BPM: usize = 8;
/// Pitch shift (PS), cents; 0 = the slider's middle.
pub const H_PITCH: usize = 12;
/// Time stretch (TS); 0 = the middle (kept, not played).
pub const H_STRETCH: usize = 16;
/// Stretching method, the menu's order (kept, not played).
pub const H_METHOD: usize = 20;
/// Attack (ATT) and decay (DEC), ms; 0 = off.
pub const H_ATTACK: usize = 24;
pub const H_DECAY: usize = 28;
pub const HEADER: usize = 32;

/// A slice: its name, its first frame in the sample, its root key (−1: the slice's own key, C5 + its index), a float
/// kept as read (−1 in every measured state), and its flags (bit 0 = reversed).
#[derive(Clone, Debug, PartialEq)]
pub struct Slice {
    pub name: Vec<u8>,
    pub start: u32,
    pub root: i32,
    pub f: f32,
    pub flags: u8,
}

impl Slice {
    pub fn at(start: u32) -> Self {
        Slice { name: Vec::new(), start, root: -1, f: -1.0, flags: 0 }
    }
    pub fn reversed(&self) -> bool {
        self.flags & 1 != 0
    }
    pub fn name(&self) -> String {
        String::from_utf8_lossy(&self.name).into_owned()
    }
}

/// A parsed state.
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub header: [u8; HEADER],
    /// The sample's path as stored (bytes; FL's placeholders such as `%FLStudioFactoryData%` kept).
    pub path: Vec<u8>,
    pub slices: Vec<Slice>,
    /// Everything after the slices: settings, then the channel's five envelope records (kept as read).
    pub tail: Vec<u8>,
}

/// Offsets of the switches in the tail (one byte each, 1 = on).
pub const T_ANIMATE: usize = 0;
pub const T_TO_END: usize = 5;
pub const T_AUTO_DUMP: usize = 10;
pub const T_DECLICK: usize = 11;
pub const T_AUTO_FIT: usize = 12;

/// A new instance's tail (the bytes every measured state ends with).
pub const DEFAULT_TAIL: [u8; 381] = default_tail();

const fn default_tail() -> [u8; 381] {
    let mut t = [0u8; 381];
    let head: [u8; 13] = [0x01, 0x3c, 0, 0, 0, 0x00, 0x44, 0xac, 0, 0, 0x01, 0x01, 0x01];
    let mut i = 0;
    while i < 13 {
        t[i] = head[i];
        i += 1;
    }
    // Five envelope records of 68 bytes from offset 17: the volume one (second) differs from the others.
    let rec: [u32; 17] = [0, 0, 100, 20000, 20000, 30000, 50, 20000, 0, 100, 20000, 0, 0x80b6, 0, 0, 0, 0];
    let vol: [u32; 17] = [4, 0xffff_ffff, 100, 100, 100, 100, 128, 6500, 0, 100, 20000, 0, 0x80b6, 0, 0, 0, 0xffff_ff9b];
    let mut r = 0;
    while r < 5 {
        let vals = if r == 1 { vol } else { rec };
        let mut k = 0;
        while k < 17 {
            let b = vals[k].to_le_bytes();
            let at = 17 + 68 * r + 4 * k;
            t[at] = b[0];
            t[at + 1] = b[1];
            t[at + 2] = b[2];
            t[at + 3] = b[3];
            k += 1;
        }
        r += 1;
    }
    t[357 + 9] = 1;
    t
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4).map_or(0, |s| u32::from_le_bytes(s.try_into().unwrap()))
}

impl Default for State {
    /// A new channel's state (418 bytes): no sample, no slices, the stretching method Transient, decay 4 ms, ANIMATE,
    /// AUTO-DUMP, DECLICK and AUTO-FIT on.
    fn default() -> Self {
        let mut header = [0u8; HEADER];
        header[0..4].copy_from_slice(&VERSION.to_le_bytes());
        let mut s = State { header, path: Vec::new(), slices: Vec::new(), tail: DEFAULT_TAIL.to_vec() };
        s.set_i32(H_METHOD, 4);
        s.set_i32(H_DECAY, 4);
        s
    }
}

impl State {
    /// Parses a saved state; `None` when it is too short or its counts run past its end.
    pub fn parse(b: &[u8]) -> Option<State> {
        let header: [u8; HEADER] = b.get(..HEADER)?.try_into().ok()?;
        let mut at = HEADER;
        let n = usize::from(*b.get(at)?);
        let path = b.get(at + 1..at + 1 + n)?.to_vec();
        at += 1 + n;
        let count = u32_at(b, at) as usize;
        b.get(at..at + 4)?;
        at += 4;
        let mut slices = Vec::with_capacity(count.min(4096));
        for _ in 0..count {
            let n = usize::from(*b.get(at)?);
            let name = b.get(at + 1..at + 1 + n)?.to_vec();
            at += 1 + n;
            let f = b.get(at..at + 13)?;
            slices.push(Slice {
                name,
                start: u32::from_le_bytes(f[0..4].try_into().ok()?),
                root: i32::from_le_bytes(f[4..8].try_into().ok()?),
                f: f32::from_le_bytes(f[8..12].try_into().ok()?),
                flags: f[12],
            });
            at += 13;
        }
        Some(State { header, path, slices, tail: b[at..].to_vec() })
    }

    /// The saved bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = self.header.to_vec();
        let path = &self.path[..self.path.len().min(255)];
        b.push(path.len() as u8);
        b.extend_from_slice(path);
        b.extend_from_slice(&(self.slices.len() as u32).to_le_bytes());
        for s in &self.slices {
            let name = &s.name[..s.name.len().min(255)];
            b.push(name.len() as u8);
            b.extend_from_slice(name);
            b.extend_from_slice(&s.start.to_le_bytes());
            b.extend_from_slice(&s.root.to_le_bytes());
            b.extend_from_slice(&s.f.to_le_bytes());
            b.push(s.flags);
        }
        b.extend_from_slice(&self.tail);
        b
    }

    pub fn i32(&self, at: usize) -> i32 {
        u32_at(&self.header, at) as i32
    }
    pub fn set_i32(&mut self, at: usize, v: i32) {
        self.header[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
    pub fn f32(&self, at: usize) -> f32 {
        f32::from_bits(u32_at(&self.header, at))
    }
    pub fn set_f32(&mut self, at: usize, v: f32) {
        self.header[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    /// The loop's length in beats.
    pub fn beats(&self) -> f64 {
        f64::from(self.f32(H_BEATS))
    }
    /// The loop's tempo.
    pub fn bpm(&self) -> f64 {
        f64::from(self.f32(H_BPM))
    }

    /// The sample's path as text (FL writes UTF-8; older bytes that are not are read as Latin-1).
    pub fn path(&self) -> String {
        String::from_utf8(self.path.clone()).unwrap_or_else(|_| self.path.iter().map(|&c| char::from(c)).collect())
    }
    pub fn set_path(&mut self, p: &str) {
        self.path = p.as_bytes().to_vec();
    }

    /// The key slice `i` plays on: its root key when one is set, else C5 (60) + its index.
    pub fn key_of(&self, i: usize) -> i32 {
        match self.slices.get(i) {
            Some(s) if s.root >= 0 => s.root,
            _ => 60 + i as i32,
        }
    }

    /// The slice key `key` plays (the last one whose key it is).
    pub fn slice_for(&self, key: i32) -> Option<usize> {
        (0..self.slices.len()).rev().find(|&i| self.key_of(i) == key)
    }

    /// Switch `at` of the tail (`T_*`).
    pub fn switch(&self, at: usize) -> bool {
        self.tail.get(at).is_some_and(|&b| b != 0)
    }
    pub fn set_switch(&mut self, at: usize, on: bool) {
        self.set_tail_u8(at, u8::from(on));
    }
    pub fn set_tail_u8(&mut self, at: usize, v: u8) {
        if self.tail.len() < DEFAULT_TAIL.len() {
            let n = self.tail.len();
            self.tail.extend_from_slice(&DEFAULT_TAIL[n..]);
        }
        self.tail[at] = v;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A state as a project stores it: a factory loop's path and three named slices.
    pub(crate) fn sample_state() -> Vec<u8> {
        let mut s = State::default();
        s.set_f32(H_BEATS, 8.0);
        s.set_f32(H_BPM, 60.0);
        s.set_path("%FLStudioFactoryData%\\Data\\Patches\\Packs\\Loops\\DL Breaker.wav");
        s.slices = [(b"Kick".to_vec(), 0), (b"Snare".to_vec(), 44100), (Vec::new(), 66150)]
            .into_iter()
            .map(|(name, start)| Slice { name, ..Slice::at(start) })
            .collect();
        s.to_bytes()
    }

    #[test]
    fn states_come_back_byte_for_byte() {
        let b = sample_state();
        let s = State::parse(&b).unwrap();
        assert_eq!(s.to_bytes(), b);
        assert_eq!((s.beats(), s.bpm()), (8.0, 60.0));
        assert_eq!(s.slices.len(), 3);
        assert_eq!((s.slices[1].name(), s.slices[1].start, s.slices[1].root), ("Snare".to_string(), 44100, -1));
        assert!(s.path().ends_with("DL Breaker.wav"));
        assert_eq!(s.tail.len(), 381);
        assert!(s.switch(T_DECLICK) && s.switch(T_ANIMATE) && !s.switch(T_TO_END));
    }

    /// A new channel's state as the original saved it (2026-10-29, fruity-slicer.md §1).
    #[test]
    fn a_new_instance_is_the_originals_new_channel() {
        let hex = "10000000000000000000000000000000000000000400000000000000040000000000000000013c0000000044ac000001010100000000000000000000000064000000204e0000204e00003075000032000000204e00000000000064000000204e000000000000b68000000000000000000000000000000000000004000000ffffffff6400000064000000640000006400000080000000641900000000000064000000204e000000000000b68000000000000000000000000000009bffffff000000000000000064000000204e0000204e00003075000032000000204e00000000000064000000204e000000000000b680000000000000000000000000000000000000000000000000000064000000204e0000204e00003075000032000000204e00000000000064000000204e000000000000b680000000000000000000000000000000000000000000000000000064000000204e0000204e00003075000032000000204e00000000000064000000204e000000000000b680000000000000000000000000000000000000000000000000000000010000000000000000000000000000";
        let b: Vec<u8> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect();
        assert_eq!(b.len(), 418);
        assert_eq!(State::default().to_bytes(), b);
    }

    #[test]
    fn slice_keys_start_at_c5() {
        let mut s = State::parse(&sample_state()).unwrap();
        assert_eq!((s.key_of(0), s.key_of(2)), (60, 62));
        assert_eq!(s.slice_for(61), Some(1));
        assert_eq!(s.slice_for(63), None);
        s.slices[2].root = 48;
        assert_eq!(s.slice_for(48), Some(2));
    }

    #[test]
    fn short_or_broken_states_are_refused() {
        let b = sample_state();
        assert!(State::parse(&b[..20]).is_none());
        // A count past the end.
        let mut c = b.clone();
        let at = HEADER + 1 + c[HEADER] as usize;
        c[at..at + 4].copy_from_slice(&1000u32.to_le_bytes());
        assert!(State::parse(&c).is_none());
    }
}
