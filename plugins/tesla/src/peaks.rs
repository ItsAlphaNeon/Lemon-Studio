// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Minimum and maximum per block of frames, so the wave area and the overview draw long samples (and a recording as
//! it grows) without reading every frame each paint.
//!
// spec: plugins/tesla/README.md "The window" (wave area, scroll bar overview)

/// Frames per block.
pub const BLOCK: usize = 256;

#[derive(Clone, Debug, Default)]
pub struct Peaks {
    /// Per channel, (min, max) of each block; the last block may be partial.
    ch: Vec<Vec<(f32, f32)>>,
    /// Frames the blocks cover.
    frames: usize,
}

fn block_of(c: &[f32]) -> (f32, f32) {
    c.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &v| (lo.min(v), hi.max(v)))
}

impl Peaks {
    pub fn new(ch: &[&[f32]]) -> Peaks {
        let mut p = Peaks::default();
        p.extend(ch);
        p
    }

    pub fn frames(&self) -> usize {
        self.frames
    }

    /// Takes in frames added at the end of `ch` (the whole channels, of which the first `frames()` were seen).
    pub fn extend(&mut self, ch: &[&[f32]]) {
        let n = ch.first().map_or(0, |c| c.len());
        if self.ch.len() != ch.len() || n < self.frames {
            self.ch = vec![Vec::new(); ch.len()];
            self.frames = 0;
        }
        // The partial last block is made again.
        let first = self.frames / BLOCK;
        for (blocks, c) in self.ch.iter_mut().zip(ch) {
            blocks.truncate(first);
            let mut i = first * BLOCK;
            while i < n {
                let j = (i + BLOCK).min(n);
                blocks.push(block_of(&c[i..j]));
                i = j;
            }
        }
        self.frames = n;
    }

    /// (min, max) of channel `c` over frames `a..b`, from `src` (that channel) at the edges and the blocks between.
    pub fn minmax(&self, src: &[f32], c: usize, a: usize, b: usize) -> (f32, f32) {
        let b = b.min(src.len()).min(self.frames);
        if a >= b {
            return (0.0, 0.0);
        }
        let blocks = &self.ch[c.min(self.ch.len() - 1)];
        let (ba, bb) = (a.div_ceil(BLOCK), b / BLOCK);
        if bb <= ba {
            return block_of(&src[a..b]);
        }
        let mut r = block_of(&src[a..ba * BLOCK]);
        for &(lo, hi) in &blocks[ba..bb] {
            r = (r.0.min(lo), r.1.max(hi));
        }
        let t = block_of(&src[bb * BLOCK..b]);
        (r.0.min(t.0), r.1.max(t.1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_match_a_direct_scan() {
        let v: Vec<f32> = (0..5000).map(|i| ((i * 7919) % 1000) as f32 / 500.0 - 1.0).collect();
        let p = Peaks::new(&[&v]);
        for (a, b) in [(0, 5000), (13, 700), (255, 257), (1000, 1001), (4999, 5000)] {
            assert_eq!(p.minmax(&v, 0, a, b), block_of(&v[a..b]), "{a}..{b}");
        }
        // Growing: the partial block is redone.
        let mut g = Peaks::new(&[&v[..300]]);
        g.extend(&[&v[..4000]]);
        assert_eq!(g.minmax(&v[..4000], 0, 0, 4000), block_of(&v[..4000]));
    }
}
