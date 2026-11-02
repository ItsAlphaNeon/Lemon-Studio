// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The channel's levels: how Lemon Studio hands a Lemon instrument the channel's volume and pan, which the instrument
//! then works into its voices' final levels the way FL's sampler does (spec: audio/voice-levels.md "Final levels"),
//! instead of the engine putting them on the instrument's output.
//!
//! Transport: one MIDI SysEx message on the plugin's note port, at the start of the tick the levels apply from, sent
//! when they change (and again after the plugin is activated). The two levels travel as the bits of their singles, so
//! they arrive exactly. 0x7D is MIDI's manufacturer id for non-commercial use: a plugin that does not know the message
//! ignores it.
//!
//! Spec: `plugins/host-link.md` "Channel levels". This is a Lemon Studio design, not FL's protocol.
// spec: plugins/host-link.md

/// Bytes of the message: `F0 7D 4C 01`, the volume and the pan as 5 data bytes each, `F7`.
pub const LEN: usize = 15;

/// The message's first four bytes: SysEx, the non-commercial id, `L`, message 1 (channel levels).
const HEAD: [u8; 4] = [0xF0, 0x7D, b'L', 1];

/// The message for a channel's volume (a gain: its volume times its multiplier) and pan (its pan plus its offset).
pub fn encode(volume: f32, pan: f32) -> [u8; LEN] {
    let mut m = [0u8; LEN];
    m[..4].copy_from_slice(&HEAD);
    put(&mut m[4..9], volume.to_bits());
    put(&mut m[9..14], pan.to_bits());
    m[14] = 0xF7;
    m
}

/// The volume and pan of a channel-levels message; None for any other message.
pub fn decode(m: &[u8]) -> Option<(f32, f32)> {
    if m.len() != LEN || m[..4] != HEAD || m[14] != 0xF7 || m[4..14].iter().any(|&b| b > 0x7F) {
        return None;
    }
    Some((f32::from_bits(get(&m[4..9])), f32::from_bits(get(&m[9..14]))))
}

/// 32 bits as 5 data bytes of 7 bits, lowest first.
fn put(out: &mut [u8], bits: u32) {
    for (k, b) in out.iter_mut().enumerate() {
        *b = ((bits >> (7 * k)) & 0x7F) as u8;
    }
}

fn get(b: &[u8]) -> u32 {
    b.iter().enumerate().fold(0, |bits, (k, &x)| bits | u32::from(x) << (7 * k))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_cross_exactly() {
        for (v, p) in [(1.0, 0.0), (0.551_029_6, -0.375), (12.0, 2.0), (0.0, -1.0), (f32::MIN_POSITIVE, -0.0)] {
            let m = encode(v, p);
            assert!(m[1..14].iter().all(|&b| b <= 0x7F), "data bytes");
            let (a, b) = decode(&m).unwrap();
            assert_eq!((a.to_bits(), b.to_bits()), (v.to_bits(), p.to_bits()));
        }
        let mut other = encode(1.0, 0.0);
        other[3] = 2;
        assert_eq!(decode(&other), None);
        assert_eq!(decode(&encode(1.0, 0.0)[..14]), None);
    }
}
