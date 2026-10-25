// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Patches from outside: the default patch, FL preset files (`.fst`) and the state inside them.
//
// spec: plugins/citrus.md, plugins/presets.md

use crate::state::State;

/// The patch of a fresh instance: operator 1, a sine at full volume with a pitch range of 1200 cents, straight to the
/// output.
pub fn default_patch() -> State {
    let mut st = State::default();
    st.name = "Init".into();
    st.matrix[8] = 256;
    st.modules[0].header[1] = 256;
    st.modules[0].header[2] = 1200;
    st.modules[0].op.levels[0] = 1.0;
    st
}

/// The Sytrus state inside an FL preset file (`FLhd` / `FLdt` events): the data of the last event 213.
pub fn state_from_fst(b: &[u8]) -> Option<Vec<u8>> {
    if b.len() < 22 || &b[0..4] != b"FLhd" {
        return None;
    }
    let hl = u32::from_le_bytes(b[4..8].try_into().ok()?) as usize;
    let mut i = 8 + hl;
    if b.get(i..i + 4)? != b"FLdt" {
        return None;
    }
    i += 8;
    let mut found = None;
    while i < b.len() {
        let id = b[i];
        i += 1;
        let n = match id {
            0..=63 => 1,
            64..=127 => 2,
            128..=191 => 4,
            _ => {
                let mut len = 0usize;
                let mut shift = 0;
                loop {
                    let c = *b.get(i)?;
                    i += 1;
                    len |= usize::from(c & 0x7F) << shift;
                    shift += 7;
                    if c & 0x80 == 0 {
                        break;
                    }
                }
                len
            }
        };
        let data = b.get(i..i + n)?;
        if id == 213 {
            found = Some(data.to_vec());
        }
        i += n;
    }
    found
}

/// A short Sytrus state for tests: version 25, operator 1 straight to the output with harmonic levels `levels` (its
/// first harmonics), the rest of the patch left out (a reader keeps the defaults for what a state lacks).
#[doc(hidden)]
pub fn test_state(levels: &[f32]) -> Vec<u8> {
    let mut b = Vec::new();
    let int = |b: &mut Vec<u8>, v: i32| b.extend_from_slice(&v.to_le_bytes());
    int(&mut b, 128); // main volume
    int(&mut b, 0); // master LFO
    int(&mut b, 0); // main pitch
    for k in 0..81 {
        int(&mut b, if k == 8 { 256 } else { 0 }); // operator 1's output level
    }
    for _ in 0..81 + 35 {
        int(&mut b, 0); // ring cells, settings
    }
    b.extend_from_slice(&[0; 162]); // cell off bytes
    int(&mut b, 0); // soften
    b.extend_from_slice(&crate::state::SWITCH_DEFAULTS);
    for _ in 0..3 {
        for v in [64, 144, 0, 64, 0] {
            int(&mut b, v);
        }
        b.extend_from_slice(&[1, 0, 0]);
    }
    for v in [75, 40, 0, 40, 100, 15, 40, 2, 25] {
        int(&mut b, v);
    }
    b.extend_from_slice(&[0, 0]); // reverb off, not tempo-based
    int(&mut b, 0); // equaliser off
    for band in [[0, 5777, 17500, 0], [0, 33145, 17500, 1], [0, 55825, 17500, 2]] {
        for v in band {
            int(&mut b, v);
        }
    }
    b.push(4);
    for c in "Test".encode_utf16() {
        b.extend_from_slice(&c.to_le_bytes());
    }
    // Module 1: its header (output level 256), then its operator block.
    for v in [0, 256, 1200, 0, 0, 0, 0, 0, 0, 0] {
        int(&mut b, v);
    }
    for v in [10000, 0, 0] {
        int(&mut b, v);
    }
    for k in 0..256 {
        b.extend_from_slice(&levels.get(k).copied().unwrap_or(0.0).to_le_bytes());
    }
    b.extend_from_slice(&[0; 256 * 4]); // phases
    for v in [0, 0, 0, 0, 128] {
        int(&mut b, v);
    }
    b.extend_from_slice(&[0, 0, 0, 0, 0, 1, 0, 0]);
    State::pack(25 << 16, &b)
}

/// An FL preset file holding Sytrus state `fl`, for tests.
#[doc(hidden)]
pub fn test_fst(fl: &[u8]) -> Vec<u8> {
    let mut ev = vec![199, 4, b'2', b'6', b'.', 0];
    ev.push(213);
    let mut n = fl.len();
    loop {
        let c = (n & 0x7F) as u8;
        n >>= 7;
        ev.push(if n > 0 { c | 0x80 } else { c });
        if n == 0 {
            break;
        }
    }
    ev.extend_from_slice(fl);
    let mut b = b"FLhd".to_vec();
    b.extend_from_slice(&6u32.to_le_bytes());
    b.extend_from_slice(&[0x10, 0, 1, 0, 96, 0]);
    b.extend_from_slice(b"FLdt");
    b.extend_from_slice(&(ev.len() as u32).to_le_bytes());
    b.extend(ev);
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The test state reads back as written, and an FL preset file around it gives it back.
    #[test]
    fn test_states_read_back() {
        let fl = test_state(&[1.0, 0.5]);
        let st = State::from_bytes(&fl).expect("state");
        assert_eq!((st.version, st.matrix[8], st.name.as_str()), (25, 256, "Test"));
        assert_eq!(&st.modules[0].op.levels[..3], &[1.0, 0.5, 0.0]);
        assert_eq!(st.modules[0].header[1], 256);
        assert_eq!(state_from_fst(&test_fst(&fl)), Some(fl));
        assert_eq!(state_from_fst(b"not a preset file at all"), None);
    }

    /// The default patch and a short state sound: what they lack takes no part.
    #[test]
    fn short_states_and_the_default_patch_sound() {
        use crate::engine::{Citrus, NoteOn};
        use crate::voice::Levels;
        for st in [default_patch(), State::from_bytes(&test_state(&[1.0])).unwrap()] {
            let mut c = Citrus::new(48000);
            c.set_state(st);
            let levels = Levels { vol: 1.0, pan: 0.0, pitch: 0.0, mod_x: 0.0, mod_y: 0.0 };
            c.note_on(NoteOn { levels, velocity: 100.0 / 128.0, release: 0.5 });
            let (mut l, mut r) = (vec![0.0f32; 4800], vec![0.0f32; 4800]);
            c.render(&mut l, &mut r);
            let peak = l.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(peak > 0.1 && peak < 1.5, "peak {peak}");
        }
    }
}
