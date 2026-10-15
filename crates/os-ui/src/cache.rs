// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Caches for repeated drawing: most frames repaint what the last one painted, so the expensive steps (path
//! rasterization, icon coverage, GDI text) are looked up by a hash of their inputs instead of being redone.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

/// A fast 64-bit hasher (folded multiply). Keys built with it are either verified on a hit (text) or cover every
/// input exactly (shapes, glyphs), where a 64-bit collision is not a practical concern.
#[derive(Clone, Copy)]
pub(crate) struct Fx(u64);

impl Default for Fx {
    fn default() -> Self {
        Fx(0x243f_6a88_85a3_08d3)
    }
}

impl Fx {
    #[inline]
    fn add(&mut self, w: u64) {
        let m = u128::from(self.0 ^ w) * 0x9e37_79b9_7f4a_7c15;
        self.0 = (m as u64) ^ ((m >> 64) as u64);
    }

    pub(crate) fn f32(&mut self, v: f32) {
        self.add(u64::from(v.to_bits()));
    }
}

impl Hasher for Fx {
    fn finish(&self) -> u64 {
        // Final avalanche, so keys that differ in a few low bits spread over the table.
        let mut h = self.0;
        h ^= h >> 33;
        h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
        h ^= h >> 33;
        h
    }

    fn write(&mut self, bytes: &[u8]) {
        let (words, rest) = bytes.as_chunks::<8>();
        for w in words {
            self.add(u64::from_le_bytes(*w));
        }
        if !rest.is_empty() {
            let mut b = [0u8; 8];
            b[..rest.len()].copy_from_slice(rest);
            self.add(u64::from_le_bytes(b) ^ ((rest.len() as u64) << 56));
        }
    }

    fn write_u8(&mut self, v: u8) {
        self.add(u64::from(v));
    }
    fn write_u16(&mut self, v: u16) {
        self.add(u64::from(v));
    }
    fn write_u32(&mut self, v: u32) {
        self.add(u64::from(v));
    }
    fn write_u64(&mut self, v: u64) {
        self.add(v);
    }
    fn write_i32(&mut self, v: i32) {
        self.add(u64::from(v as u32));
    }
    fn write_usize(&mut self, v: usize) {
        self.add(v as u64);
    }
}

/// Map keys that already are hashes pass through unchanged.
#[derive(Default)]
pub(crate) struct Id(u64);

impl Hasher for Id {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, _: &[u8]) {
        unreachable!("only u64 keys")
    }
    fn write_u64(&mut self, v: u64) {
        self.0 = v;
    }
}

/// A byte-budgeted cache keyed by a 64-bit hash. Past the budget, the less recently used half is dropped.
pub(crate) struct Lru<V> {
    map: HashMap<u64, (u64, usize, V), BuildHasherDefault<Id>>,
    clock: u64,
    bytes: usize,
    budget: usize,
}

impl<V> Lru<V> {
    pub(crate) fn new(budget: usize) -> Self {
        Lru { map: HashMap::default(), clock: 0, bytes: 0, budget }
    }

    pub(crate) fn get(&mut self, k: u64) -> Option<&V> {
        self.clock += 1;
        let now = self.clock;
        self.map.get_mut(&k).map(|e| {
            e.0 = now;
            &e.2
        })
    }

    pub(crate) fn insert(&mut self, k: u64, v: V, bytes: usize) {
        self.clock += 1;
        if let Some(old) = self.map.insert(k, (self.clock, bytes, v)) {
            self.bytes -= old.1;
        }
        self.bytes += bytes;
        if self.bytes > self.budget {
            let mut stamps: Vec<u64> = self.map.values().map(|e| e.0).collect();
            let mid = stamps.len() / 2;
            let (_, &mut cut, _) = stamps.select_nth_unstable(mid);
            self.map.retain(|_, e| e.0 >= cut);
            self.bytes = self.map.values().map(|e| e.1).sum();
        }
    }

    pub(crate) fn clear(&mut self) {
        self.map.clear();
        self.bytes = 0;
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.map.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lru_keeps_recent_entries_within_budget() {
        let mut c = Lru::new(100);
        for k in 0..10u64 {
            c.insert(k, k, 10);
        }
        assert_eq!(c.len(), 10);
        // Touch the oldest: it survives the next eviction.
        assert_eq!(c.get(0), Some(&0));
        c.insert(10, 10, 10);
        assert!(c.get(0).is_some() && c.get(10).is_some());
        assert!(c.get(1).is_none(), "old, unused entries go first");
        assert!(c.len() <= 7);
    }

    #[test]
    fn hasher_separates_lengths_and_values() {
        let h = |b: &[u8]| {
            let mut f = Fx::default();
            f.write(b);
            f.finish()
        };
        assert_ne!(h(b"a"), h(b"a\0"));
        assert_ne!(h(b"abcdefgh"), h(b"abcdefgi"));
        assert_ne!(h(b""), h(b"\0"));
    }
}
