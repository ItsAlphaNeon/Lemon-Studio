// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! What a VST 2 instance saves: the state record FL Studio's plugin wrapper keeps for a VST 2 plugin, so a project's
//! VST 2 plugins move between the two programs (spec: flp/events/plugins.md §4, "FL 26's own VST2 wrapper").
//!
//! A 21-byte header: the 32-bit value -9, a kind byte, -2, the size of what follows, then two 32-bit values whose
//! meaning is not known (written 0). Kind 12 or 13: the plugin's own chunk (`op::GET_CHUNK`) follows. FL Studio 2026
//! writes both: 12 for Magical 8bit Plug 2, Krush and Dimension Expander, 13 for TruePianos and Kickstart 2 (in a
//! project FL Studio 26.1.3 saved). What tells the two apart is not known, so both are read as a chunk; Lemon Studio
//! writes 12, which FL Studio reads back. Kind 4, for plugins that keep no chunk: the size is 0, and the parameter count
//! and each value (`f32`) follow. FL Studio puts a part about programs after the values; its meaning is not known, so
//! it is not written, and it is skipped when read (FL Studio reads the values without it).
//!
//! Lemon Studio's first saves wrote kind 13, with the chunk, or for plugins without one their program and values in a
//! list of its own ([`LIST_TAG`]). Both are still read; FL Studio reads the chunk but not the list.

/// The header fields around the kind byte.
const HEAD: i32 = -9;
const HEAD_2: i32 = -2;
/// Kind bytes: a chunk (what Lemon Studio writes), a chunk too (FL Studio writes both; Lemon Studio's first saves wrote
/// 13), a parameter list.
const KIND_CHUNK: u8 = 12;
const KIND_CHUNK_13: u8 = 13;
const KIND_PARAMS: u8 = 4;
/// Header length; the kind is at byte 4, the size at byte 9.
const HEADER: usize = 21;

/// Starts the parameter list Lemon Studio's first saves wrote as a chunk: the program (`i32`), the count (`u32`), then
/// each value (`f32`), little-endian.
const LIST_TAG: &[u8; 4] = b"LSVP";

/// A VST 2 instance's saved state.
#[derive(Debug, Clone, PartialEq)]
pub enum Saved {
    /// The plugin's own chunk (a plugin with `flags::CHUNKS`).
    Chunk(Vec<u8>),
    /// Every parameter's value (any other plugin), and the program where the state tells it (only Lemon Studio's first
    /// list does).
    Params { program: Option<i32>, values: Vec<f32> },
}

impl Saved {
    /// The bytes FL Studio writes for this state. A parameter list's program is not written.
    pub fn to_bytes(&self) -> Vec<u8> {
        let (kind, size, body) = match self {
            Saved::Chunk(c) => (KIND_CHUNK, c.len() as u32, c.clone()),
            Saved::Params { values, .. } => {
                let mut b = (values.len() as u32).to_le_bytes().to_vec();
                values.iter().for_each(|v| b.extend(v.to_le_bytes()));
                (KIND_PARAMS, 0, b)
            }
        };
        let mut out = Vec::with_capacity(HEADER + body.len());
        out.extend(HEAD.to_le_bytes());
        out.push(kind);
        out.extend(HEAD_2.to_le_bytes());
        out.extend(size.to_le_bytes());
        out.extend([0; 8]);
        out.extend(body);
        out
    }

    /// Reads a saved state. `chunks`: the plugin keeps a chunk (another plugin's chunk is never taken as a parameter
    /// list; a list is taken by any plugin). `None` when `data` is not this layout.
    pub fn parse(data: &[u8], chunks: bool) -> Option<Saved> {
        let i32_at = |p: usize| Some(i32::from_le_bytes(data.get(p..p + 4)?.try_into().ok()?));
        let len_at = |p: usize| usize::try_from(u32::from_le_bytes(data.get(p..p + 4)?.try_into().ok()?)).ok();
        let floats = |from: usize, count: usize| -> Option<Vec<f32>> {
            let b = data.get(from..from.checked_add(count.checked_mul(4)?)?)?;
            Some(b.chunks_exact(4).map(|v| f32::from_le_bytes(v.try_into().unwrap_or_default())).collect())
        };
        if i32_at(0)? != HEAD || i32_at(5)? != HEAD_2 {
            return None;
        }
        match data[4] {
            KIND_PARAMS => Some(Saved::Params { program: None, values: floats(HEADER + 4, len_at(HEADER)?)? }),
            KIND_CHUNK | KIND_CHUNK_13 => {
                let size = len_at(9)?;
                let body = data.get(HEADER..HEADER.checked_add(size)?)?;
                if chunks {
                    return Some(Saved::Chunk(body.to_vec()));
                }
                body.strip_prefix(LIST_TAG)?;
                let at = HEADER + LIST_TAG.len();
                Some(Saved::Params { program: Some(i32_at(at)?), values: floats(at + 8, len_at(at + 4)?)? })
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_read_back_with_the_header_fl_studio_writes() {
        let s = Saved::Chunk(b"#zip#\0data".to_vec());
        let b = s.to_bytes();
        assert_eq!(&b[..21], &[0xF7, 0xFF, 0xFF, 0xFF, 12, 0xFE, 0xFF, 0xFF, 0xFF, 10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&b[21..], b"#zip#\0data");
        assert_eq!(Saved::parse(&b, true), Some(s.clone()));
        // A plugin without chunks does not take another plugin's chunk as its parameters.
        assert_eq!(Saved::parse(&b, false), None);
        // Kind 13, as FL Studio also writes it (and Lemon Studio's first saves did); and FL Studio's other header values
        // (-1 seen in the last one).
        let mut first = b.clone();
        first[4] = 13;
        first[17..21].copy_from_slice(&(-1i32).to_le_bytes());
        assert_eq!(Saved::parse(&first, true), Some(s));
    }

    #[test]
    fn parameter_lists_are_written_as_fl_studio_writes_them() {
        let s = Saved::Params { program: Some(3), values: vec![1.0, 0.8, 0.3] };
        let b = s.to_bytes();
        let mut want = vec![0xF7, 0xFF, 0xFF, 0xFF, 4, 0xFE, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0];
        [1.0f32, 0.8, 0.3].iter().for_each(|v| want.extend(v.to_le_bytes()));
        assert_eq!(b, want);
        // Read back by plugins with or without chunks; the program is not kept.
        let back = Saved::Params { program: None, values: vec![1.0, 0.8, 0.3] };
        assert_eq!(Saved::parse(&b, false), Some(back.clone()));
        assert_eq!(Saved::parse(&b, true), Some(back.clone()));
        // FL Studio's part about programs after the values is skipped.
        let mut fl = b.clone();
        fl.extend(1u32.to_le_bytes());
        fl.extend(b"Init\0\xe0\x68\x5f\x10\0\0\0\0\x10\x4c\xbc\0\0\0\0\0\x01\0\0\0");
        assert_eq!(Saved::parse(&fl, false), Some(back));
    }

    #[test]
    fn lemon_studios_first_parameter_lists_read_back() {
        // Kind 13, the size of the list, then `LSVP`, the program, the count and the values.
        let mut b = vec![0xF7, 0xFF, 0xFF, 0xFF, 13, 0xFE, 0xFF, 0xFF, 0xFF, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        b.extend(b"LSVP");
        b.extend(2i32.to_le_bytes());
        b.extend(2u32.to_le_bytes());
        b.extend(0.25f32.to_le_bytes());
        b.extend(1.0f32.to_le_bytes());
        assert_eq!(Saved::parse(&b, false), Some(Saved::Params { program: Some(2), values: vec![0.25, 1.0] }));
        // A chunk plugin takes it as its chunk, as it took it before.
        assert_eq!(Saved::parse(&b, true), Some(Saved::Chunk(b[21..].to_vec())));
    }

    #[test]
    fn other_bytes_are_refused() {
        assert_eq!(Saved::parse(b"", true), None);
        assert_eq!(Saved::parse(b"VST3 preset", true), None);
        // A size running past the end.
        let mut b = Saved::Chunk(vec![1, 2, 3]).to_bytes();
        b.truncate(b.len() - 1);
        assert_eq!(Saved::parse(&b, true), None);
        // A list shorter than its count.
        let mut b = Saved::Params { program: None, values: vec![0.5; 4] }.to_bytes();
        b[21] = 9;
        assert_eq!(Saved::parse(&b, false), None);
        // A kind that has not been seen.
        let mut b = Saved::Chunk(vec![1, 2, 3]).to_bytes();
        b[4] = 8;
        assert_eq!(Saved::parse(&b, true), None);
    }
}
