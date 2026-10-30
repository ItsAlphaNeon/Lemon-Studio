// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! What a VST 3 instance saves: the state record FL Studio's plugin wrapper keeps for a VST 3 plugin, so a project's
//! VST 3 plugins move between the two programs (spec: flp/events/plugins.md §4, "FL 26's own VST3 wrapper" and
//! "Third-party plugins in producers' projects").
//!
//! The 32-bit value 1, then chunks of a 32-bit kind, a 64-bit size and that many bytes: kind 1 (the value 1 and 60
//! zero bytes, its meaning not known), kind 3 the component's state, kind 2 the edit controller's (only a plugin with
//! a separate controller has one), kind 4 the 32-bit count and ids of the plugin's parameters in its own order.
// spec: flp/events/plugins.md §4

const VERSION: u32 = 1;
/// Chunk kinds, in the order FL Studio writes them: 1, 3, 2, 4.
const KIND_HEAD: u32 = 1;
const KIND_CONTROLLER: u32 = 2;
const KIND_COMPONENT: u32 = 3;
const KIND_PARAMS: u32 = 4;
/// Kind 1's bytes: the value 1, then zeros (in every state FL Studio wrote that has been seen).
const HEAD_LEN: usize = 64;

/// The parts of a saved state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FlState {
    /// The component's (processor's) state.
    pub component: Vec<u8>,
    /// The edit controller's own state: `None` for a plugin whose controller is its component (no kind 2 chunk).
    pub controller: Option<Vec<u8>>,
    /// The plugin's parameter ids in its own order.
    pub params: Vec<u32>,
}

impl FlState {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = VERSION.to_le_bytes().to_vec();
        let mut chunk = |kind: u32, body: &[u8]| {
            out.extend(kind.to_le_bytes());
            out.extend((body.len() as u64).to_le_bytes());
            out.extend(body);
        };
        let mut head = vec![0; HEAD_LEN];
        head[..4].copy_from_slice(&1u32.to_le_bytes());
        chunk(KIND_HEAD, &head);
        chunk(KIND_COMPONENT, &self.component);
        if let Some(c) = &self.controller {
            chunk(KIND_CONTROLLER, c);
        }
        let mut ids = (self.params.len() as u32).to_le_bytes().to_vec();
        self.params.iter().for_each(|id| ids.extend(id.to_le_bytes()));
        chunk(KIND_PARAMS, &ids);
        out
    }

    /// Reads a saved state. `None` unless `data` is exactly the version and chunks of known kinds, one of them the
    /// component's (so a plugin's own state that happens to start with the value 1 is not taken for one).
    pub fn parse(data: &[u8]) -> Option<FlState> {
        let u32_at = |p: usize| Some(u32::from_le_bytes(data.get(p..p.checked_add(4)?)?.try_into().ok()?));
        if u32_at(0)? != VERSION {
            return None;
        }
        let (mut component, mut controller, mut params) = (None, None, Vec::new());
        let mut p = 4;
        while p < data.len() {
            let kind = u32_at(p)?;
            let size = usize::try_from(u64::from_le_bytes(data.get(p + 4..p + 12)?.try_into().ok()?)).ok()?;
            let body = data.get(p + 12..(p + 12).checked_add(size)?)?;
            match kind {
                KIND_HEAD => {}
                KIND_COMPONENT => component = Some(body.to_vec()),
                KIND_CONTROLLER => controller = Some(body.to_vec()),
                KIND_PARAMS => {
                    let count = usize::try_from(u32::from_le_bytes(body.get(..4)?.try_into().ok()?)).ok()?;
                    let ids = body.get(4..)?;
                    if ids.len() != count.checked_mul(4)? {
                        return None;
                    }
                    params = ids.chunks_exact(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
                }
                _ => return None,
            }
            p += 12 + size;
        }
        Some(FlState { component: component?, controller, params })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The chunk kinds of a saved state, in order.
    fn kinds(b: &[u8]) -> Vec<u32> {
        let mut p = 4;
        let mut v = Vec::new();
        while p < b.len() {
            v.push(u32::from_le_bytes(b[p..p + 4].try_into().unwrap()));
            p += 12 + u64::from_le_bytes(b[p + 4..p + 12].try_into().unwrap()) as usize;
        }
        v
    }

    #[test]
    fn states_round_trip_in_fl_studios_order() {
        let s = FlState { component: vec![1, 2, 3], controller: Some(Vec::new()), params: vec![0, 7, 0x4000_081F] };
        let b = s.to_bytes();
        assert_eq!(&b[..4], &[1, 0, 0, 0]);
        // Kind 1 with its 64 bytes, then 3, 2 and 4.
        assert_eq!(&b[4..16], &[1, 0, 0, 0, 64, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&b[16..20], &[1, 0, 0, 0]);
        assert!(b[20..80].iter().all(|&x| x == 0));
        assert_eq!(&b[80..92], &[3, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&b[95..107], &[2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&b[107..119], &[4, 0, 0, 0, 16, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&b[119..123], &[3, 0, 0, 0]);
        assert_eq!(b.len(), 119 + 16);
        assert_eq!(FlState::parse(&b), Some(s));
        assert_eq!(kinds(&b), [1, 3, 2, 4]);
        // A plugin without a separate controller has no kind 2 chunk.
        let single = FlState { component: vec![9; 10], controller: None, params: vec![] };
        let b = single.to_bytes();
        assert_eq!(kinds(&b), [1, 3, 4]);
        assert_eq!(FlState::parse(&b), Some(single));
    }

    #[test]
    fn only_whole_fl_states_are_taken() {
        let b = FlState { component: vec![5; 8], controller: None, params: vec![1, 2] }.to_bytes();
        // Cut inside a chunk. (Cut after the component's chunk, the state reads without its parameter ids.)
        for cut in (1..b.len()).filter(|&c| c != 4 + 76 + 20) {
            assert_eq!(FlState::parse(&b[..cut]), None, "cut at {cut}");
        }
        assert_eq!(FlState::parse(&b[..100]).map(|s| s.component), Some(vec![5; 8]));
        // No component chunk.
        let mut no_comp = 1u32.to_le_bytes().to_vec();
        no_comp.extend([1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(FlState::parse(&no_comp), None);
        // A plugin's own state that starts with the value 1 (as the Kilohearts plugins' do): the next "chunk" does not fit.
        let own = [1, 0, 0, 0, 0x21, 2, 0, 0, b'P', b'K', 3, 4, 20, 0, 8, 0];
        assert_eq!(FlState::parse(&own), None);
        // An unknown chunk kind.
        let mut odd = b.clone();
        odd.extend([9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(FlState::parse(&odd), None);
    }
}
