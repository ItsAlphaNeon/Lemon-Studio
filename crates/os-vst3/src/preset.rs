// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! A VST 3 instance's state bytes: the documented `.vstpreset` container (header, data chunks, chunk list), so the
//! bytes Lemon Studio stores for an instance are also a valid preset file.
// spec: plugins/vst3-hosting.md §8

/// Chunk ids.
pub const COMPONENT: [u8; 4] = *b"Comp";
pub const CONTROLLER: [u8; 4] = *b"Cont";

const HEADER: usize = 48;

/// The parts of a preset.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Preset {
    /// Class id as its 32-character FUID string.
    pub class_id: String,
    /// The component's (processor's) state.
    pub component: Vec<u8>,
    /// The edit controller's own state (often empty).
    pub controller: Vec<u8>,
}

impl Preset {
    /// The `.vstpreset` bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER + self.component.len() + self.controller.len() + 48);
        out.extend_from_slice(b"VST3");
        out.extend_from_slice(&1i32.to_le_bytes());
        let mut id = [b'0'; 32];
        for (d, s) in id.iter_mut().zip(self.class_id.bytes()) {
            *d = s;
        }
        out.extend_from_slice(&id);
        out.extend_from_slice(&0i64.to_le_bytes()); // list offset, patched below
        let mut entries: Vec<([u8; 4], i64, i64)> = Vec::new();
        for (id, data) in [(COMPONENT, &self.component), (CONTROLLER, &self.controller)] {
            if id == CONTROLLER && data.is_empty() {
                continue;
            }
            entries.push((id, out.len() as i64, data.len() as i64));
            out.extend_from_slice(data);
        }
        let list = out.len() as i64;
        out[40..48].copy_from_slice(&list.to_le_bytes());
        out.extend_from_slice(b"List");
        out.extend_from_slice(&(entries.len() as i32).to_le_bytes());
        for (id, off, size) in entries {
            out.extend_from_slice(&id);
            out.extend_from_slice(&off.to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
        }
        out
    }

    /// Reads `.vstpreset` bytes. The component chunk is required.
    pub fn parse(data: &[u8]) -> Option<Preset> {
        if data.len() < HEADER || &data[..4] != b"VST3" {
            return None;
        }
        let class_id = std::str::from_utf8(&data[8..40]).ok()?.to_owned();
        let list = usize::try_from(i64::from_le_bytes(data[40..48].try_into().ok()?)).ok()?;
        let head = data.get(list..list.checked_add(8)?)?;
        if &head[..4] != b"List" {
            return None;
        }
        let count = usize::try_from(i32::from_le_bytes(head[4..8].try_into().ok()?)).ok()?;
        let mut p = Preset { class_id, ..Preset::default() };
        let mut comp = false;
        for i in 0..count.min(64) {
            let at = list + 8 + i * 20;
            let e = data.get(at..at + 20)?;
            let off = usize::try_from(i64::from_le_bytes(e[4..12].try_into().ok()?)).ok()?;
            let size = usize::try_from(i64::from_le_bytes(e[12..20].try_into().ok()?)).ok()?;
            let chunk = data.get(off..off.checked_add(size)?)?;
            match [e[0], e[1], e[2], e[3]] {
                COMPONENT => {
                    p.component = chunk.to_vec();
                    comp = true;
                }
                CONTROLLER => p.controller = chunk.to_vec(),
                _ => {}
            }
        }
        comp.then_some(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_round_trip() {
        let p = Preset { class_id: "0123456789ABCDEF0123456789ABCDEF".into(), component: b"{\"a\":1}".to_vec(), controller: vec![] };
        let b = p.to_bytes();
        assert_eq!(&b[..4], b"VST3");
        assert_eq!(&b[8..40], b"0123456789ABCDEF0123456789ABCDEF");
        // Header, one chunk, list with one entry.
        assert_eq!(b.len(), 48 + 7 + 8 + 20);
        assert_eq!(i64::from_le_bytes(b[40..48].try_into().unwrap()), 55);
        assert_eq!(Preset::parse(&b), Some(p.clone()));
        let both = Preset { controller: vec![1, 2, 3], ..p };
        assert_eq!(Preset::parse(&both.to_bytes()), Some(both));
        assert_eq!(Preset::parse(b"VST3"), None);
        let mut bad = b.clone();
        bad[40] = 200;
        assert_eq!(Preset::parse(&bad), None);
    }
}
