// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The browser screen's visualizer: a square of gapped cells whose grid follows the pitch of the sound. A deep sound
//! divides the square into a few large cells, a higher one into more and smaller ones (2 x 2 up to 16 x 16). A change
//! of grid is a zoom: the cells grow or shrink over 250 ms, easing in and out and even per doubling (the size moves on a
//! log scale), cells sliding in or out at the top and right; the square is one even grid all the while.
//!
//! Each column is a frequency band (low to high), lit from the bottom by its level. A lit cell's colour is its heat,
//! as on a meter: lemon, orange where the band runs hot (its top 12 dB), red near the top (its last 3 dB). Each cell
//! takes the level at its centre, so a fine grid shows the colours in steps, a middle one only orange at the top, and
//! the coarsest grid none. The grid and the levels come from the same analysis (`step`), so neither costs anything
//! more.

use os_theme::design::lemon as brand;
use os_ui::{Canvas, Color, Rect};

/// The finest grid: the analysis gives this many bands, coarser grids merge them.
pub const BANDS: usize = 16;
/// The grids, coarse to fine (cells per side).
const GRIDS: [usize; 7] = [2, 3, 4, 6, 8, 12, 16];
/// The heat: levels (0..1 of the -60..0 dB scale) from which a cell is orange, and red.
const HOT: f32 = 0.8;
const HOTTEST: f32 = 0.95;
const ORANGE: Color = Color::rgb(0xff9a3c);
const RED: Color = Color::rgb(0xff4d3a);
/// The square's side (logical px).
const SIDE: f32 = 62.0;
/// How fast a column falls (its full height per second).
const FALL: f32 = 2.5;
/// How far past a step's edge the pitch must go before the grid changes (0..1 of the pitch range).
const HYSTERESIS: f32 = 0.04;
/// How long a zoom from one grid to another takes (s).
const ZOOM: f32 = 0.25;

#[derive(Clone, Debug, PartialEq)]
pub struct Viz {
    level: [f32; BANDS],
    /// The grid wanted (an index of `GRIDS`).
    grid: usize,
    /// The cells per side shown now: on its way from `from` to `GRIDS[grid]`, `t` (0..1) of the way.
    cells: f32,
    from: f32,
    t: f32,
}

impl Default for Viz {
    fn default() -> Self {
        Viz { level: [0.0; BANDS], grid: 4, cells: GRIDS[4] as f32, from: GRIDS[4] as f32, t: 1.0 }
    }
}

impl Viz {
    /// Moves on by `dt` seconds: the bands' levels now (`target`, 0..1, low to high) and the sound's pitch (`pitch`,
    /// 0..1 from low to high; `None` while silent, which keeps the grid). Returns whether anything shown changed
    /// (nothing does once all is dark, quiet and still).
    pub fn step(&mut self, target: &[f32; BANDS], pitch: Option<f32>, dt: f32) -> bool {
        let before = self.shown();
        for (l, &t) in self.level.iter_mut().zip(target) {
            let t = t.clamp(0.0, 1.0);
            *l = if t >= *l { t } else { (*l - FALL * dt).max(t) };
        }
        if let Some(p) = pitch {
            let steps = GRIDS.len() as f32;
            let want = ((p.clamp(0.0, 1.0) * steps) as usize).min(GRIDS.len() - 1);
            // change only once the pitch is clearly inside another step
            let edge = |g: usize| g as f32 / steps;
            let clear = if want > self.grid { p >= edge(want) + HYSTERESIS } else { p < edge(want + 1) - HYSTERESIS };
            if want != self.grid && clear {
                // a new zoom starts from wherever the grid is now (also halfway through another)
                self.grid = want;
                (self.from, self.t) = (self.cells, 0.0);
            }
        }
        if self.t < 1.0 {
            self.t = (self.t + dt / ZOOM).min(1.0);
            let e = self.t * self.t * (3.0 - 2.0 * self.t);
            let (a, b) = (self.from.log2(), (GRIDS[self.grid] as f32).log2());
            self.cells = (a + (b - a) * e).exp2();
            if self.t >= 1.0 {
                self.cells = GRIDS[self.grid] as f32;
            }
        }
        self.shown() != before
    }

    /// What painting draws: the cells per side (to a hundredth) and each column's lit height in cells.
    fn shown(&self) -> (i32, Vec<usize>) {
        let n = self.cells;
        let cols = n.ceil() as usize;
        ((n * 100.0).round() as i32, (0..cols).map(|c| (self.column(n, c) * n).round() as usize).collect())
    }

    /// Column `c` of a grid of `n` cells per side (`n` fractional during a zoom): the loudest of the bands under it.
    fn column(&self, n: f32, c: usize) -> f32 {
        let at = |x: f32| ((x / n) * BANDS as f32).clamp(0.0, BANDS as f32);
        let a = (at(c as f32) as usize).min(BANDS - 1);
        let b = (at(c as f32 + 1.0).ceil() as usize).clamp(a + 1, BANDS);
        self.level[a..b].iter().copied().fold(0.0, f32::max)
    }

    /// Paints the square, its left edge at `x` and centred on `cy`; returns it.
    pub fn paint(&self, c: &mut Canvas, x: f32, cy: f32) -> Rect {
        let sq = Rect::new(x, (cy - SIDE / 2.0).round(), SIDE, SIDE);
        let n = self.cells;
        let (_, lit) = self.shown();
        // the gap shrinks as the cells do; a cell and its gap make one even step across the square
        let gap = (12.0 / n).clamp(1.0, 4.0);
        let step = (SIDE + gap) / n;
        let cell = step - gap;
        let off = Color::rgb(0x2b2b2b);
        // partly shown cells at the top and right while zooming
        c.push_clip(sq);
        for (col, &h) in lit.iter().enumerate() {
            for row in 0..n.ceil() as usize {
                let r = Rect::new(sq.x + col as f32 * step, sq.bottom() - row as f32 * step - cell, cell, cell);
                c.fill_rrect(r, (cell * 0.2).clamp(1.0, 3.0), if row < h { heat((row as f32 + 0.5) / n) } else { off });
            }
        }
        c.pop_clip();
        sq
    }
}

/// A lit cell's colour for the level at its centre.
fn heat(level: f32) -> Color {
    if level > HOTTEST {
        RED
    } else if level > HOT {
        ORANGE
    } else {
        brand::MAIN
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Steps until the zoom has settled (2 s), each 30 ms long.
    fn settle(v: &mut Viz, target: &[f32; BANDS], pitch: Option<f32>) {
        for _ in 0..66 {
            v.step(target, pitch, 0.03);
        }
    }

    /// A deep sound gives the coarsest grid, a high one the finest; near a step's edge it does not flip back and
    /// forth; silence keeps the grid and lets the columns fall until nothing changes.
    #[test]
    fn the_grid_follows_the_pitch() {
        let mut v = Viz::default();
        let loud = [1.0; BANDS];
        settle(&mut v, &loud, Some(0.05));
        assert_eq!(v.shown().0, 200);
        settle(&mut v, &loud, Some(0.2));
        assert_eq!(v.shown().0, 300);
        settle(&mut v, &loud, Some(0.95));
        assert_eq!(v.shown().0, 1600);
        // just below the 16 x 16 step's edge (6/7): stays
        settle(&mut v, &loud, Some(0.84));
        assert_eq!(v.shown().0, 1600);
        settle(&mut v, &loud, Some(0.75));
        assert_eq!(v.shown().0, 1200);
        let quiet = [0.0; BANDS];
        settle(&mut v, &quiet, None);
        assert_eq!(v.shown(), (1200, vec![0; 12]), "dark, on the last grid");
        assert!(!v.step(&quiet, None, 0.03), "nothing to repaint");
    }

    /// A change of grid is a zoom: the cells per side pass through the sizes between, and keep repainting until they
    /// arrive.
    #[test]
    fn grids_zoom() {
        let mut v = Viz::default();
        let loud = [0.5; BANDS];
        settle(&mut v, &loud, Some(0.05));
        assert_eq!(v.cells, 2.0);
        assert!(v.step(&loud, Some(0.95), 0.03));
        assert!(v.cells > 2.0 && v.cells < 16.0, "{}", v.cells);
        let mut frames = 1;
        while v.step(&loud, Some(0.95), 0.03) {
            frames += 1;
        }
        assert_eq!(v.cells, 16.0);
        assert_eq!(frames, 9, "250 ms at 30 ms a frame");
    }

    /// A coarse column shows the loudest of the bands it holds.
    #[test]
    fn coarse_columns_merge_bands() {
        let mut v = Viz::default();
        let mut t = [0.0; BANDS];
        t[3] = 1.0;
        settle(&mut v, &t, Some(0.3));
        assert_eq!(v.shown(), (400, vec![4, 0, 0, 0]));
    }

    /// Lemon below the hot range, orange in it, red near the top.
    #[test]
    fn heat_colours() {
        assert_eq!((heat(0.5), heat(0.875), heat(1.0)), (brand::MAIN, ORANGE, RED));
    }
}
