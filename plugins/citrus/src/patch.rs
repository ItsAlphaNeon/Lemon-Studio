// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! A loaded patch: the state and what is made from it when it is loaded (wave tables, shaper tables, mod X / Y
//! tables, matrix gains, which modules are used and how operators run).
//
// spec: plugins/sytrus.md ("The matrix", "Maps", "The wave table", "The operators, per sample", "Filter modules")

use std::sync::Arc;

use crate::curve::{Curve, MapWalk};
use crate::filter::Shaper;
use crate::num::{cell_gain, s, signed_curve128};
use crate::state::*;
use crate::wave::{self, Table};

/// One articulator's place: module, index.
pub fn bipolar(module: usize, art: usize) -> bool {
    match module {
        0..=5 => matches!(art, OP_PAN | OP_PITCH | OP_PHASE),
        6..=8 => matches!(art, F_PAN | F_CUTOFF | F_RES),
        _ => art == 0,
    }
}

/// The 513-entry table of a mod X or Y map that takes part (`None` for a default map).
fn mod_table(c: &Curve, centre: f64) -> Option<Vec<f32>> {
    let default = s(1.0 - centre);
    if c.points.iter().all(|p| p.level == default) {
        return None;
    }
    let mut w = MapWalk::new(c, centre);
    let mut t = Vec::with_capacity(513);
    t.push(w.out);
    for _ in 1..513 {
        t.push(w.advance(0.001_953_125));
    }
    Some(t)
}

#[derive(Debug, Clone)]
pub struct Patch {
    pub st: State,
    /// Per operator: its table (operators that are used), shared with the editor's [`Tables`].
    pub tables: Vec<Option<Arc<Table>>>,
    pub shapers: Vec<Option<Shaper>>,
    /// Per module and articulator: the mod X and mod Y tables.
    pub mods: Vec<Vec<[Option<Vec<f32>>; 2]>>,
    /// Matrix gains `g(cell)` per row and column, ring gains.
    pub g: [[f32; 9]; 9],
    pub ring: [[f32; 9]; 9],
    pub used: [bool; 9],
    pub per_sample: [bool; 6],
    /// The master LFO level as every articulator's LFO amount.
    pub lfo_amount: f32,
    /// The patch has step points somewhere.
    pub has_steps: bool,
    /// Some module sends to the effects bus.
    pub sends: bool,
}

/// The matrix gains `g(cell)` and ring gains of a state, per row and column.
fn gains(st: &State) -> ([[f32; 9]; 9], [[f32; 9]; 9]) {
    let mut g = [[0f32; 9]; 9];
    let mut ring = [[0f32; 9]; 9];
    for r in 0..9 {
        for c in 0..9 {
            g[r][c] = cell_gain(st.cell(r, c));
            ring[r][c] = cell_gain(st.ring_cell(r, c));
        }
    }
    (g, ring)
}

/// Which modules a state uses, and which operators run per sample.
pub fn usage(st: &State) -> ([bool; 9], [bool; 6]) {
    let (g, ring) = gains(st);
    usage_of(st, &g, &ring)
}

fn usage_of(st: &State, g: &[[f32; 9]; 9], ring: &[[f32; 9]; 9]) -> ([bool; 9], [bool; 6]) {
    let mut used = [false; 9];
    let mut per_sample = [false; 6];
    for o in 0..OPERATORS {
        let sends_op = (0..OPERATORS).any(|d| d != o && (g[d][o] != 0.0 || ring[d][o] != 0.0));
        let sends_filter = (OPERATORS..9).any(|f| g[f][o] != 0.0);
        used[o] = g[o][COL_OUT] != 0.0 || g[o][COL_SEND] != 0.0 || sends_op || sends_filter;
        let modulated = (0..OPERATORS).any(|c| g[o][c] != 0.0 || ring[o][c] != 0.0);
        let modulates = (0..OPERATORS).any(|d| g[d][o] != 0.0 || ring[d][o] != 0.0);
        let op = &st.modules[o].op;
        per_sample[o] = modulated || modulates || op.noise != 0 || op.flags[PLUCKED] != 0;
    }
    for f in 0..FILTERS {
        let row = OPERATORS + f;
        let from_ops = (0..OPERATORS).any(|o| used[o] && g[row][o] != 0.0);
        let from_prev = f > 0 && used[row - 1] && st.modules[row - 1].header[9] != 0;
        used[row] = from_ops || from_prev;
    }
    (used, per_sample)
}

/// The operators' tables an editor made last, kept so an edit that leaves an operator's waveform settings alone
/// does not build its table again (a table is 16384 points and its spectrum; six of them took up to 7 ms an edit).
#[derive(Default)]
pub struct Tables([Option<(OpBlock, bool, Arc<Table>)>; OPERATORS]);

impl Tables {
    /// Operator `o`'s table for its settings `op` and smoothing `smooth`: the kept one when they are the same.
    pub fn get(&mut self, o: usize, op: &OpBlock, smooth: bool) -> Arc<Table> {
        match &self.0[o] {
            Some((kept, s, t)) if kept == op && *s == smooth => t.clone(),
            _ => {
                let t = Arc::new(wave::build(op, smooth));
                self.0[o] = Some((op.clone(), smooth, t.clone()));
                t
            }
        }
    }
}

impl Patch {
    pub fn new(st: State) -> Patch {
        Patch::with_tables(st, &mut Tables::default())
    }

    /// The patch of `st`, its operators' tables taken from `kept` where their settings did not change.
    pub fn with_tables(st: State, kept: &mut Tables) -> Patch {
        let (g, ring) = gains(&st);
        let (used, per_sample) = usage_of(&st, &g, &ring);
        let smooth = st.switch(2) != 0;
        let tables = (0..OPERATORS).map(|o| used[o].then(|| kept.get(o, &st.modules[o].op, smooth))).collect();
        let shapers = (0..FILTERS)
            .map(|f| {
                let m = &st.modules[OPERATORS + f];
                (used[OPERATORS + f] && m.filter.shaper_on).then(|| Shaper::new(&m.filter.shaper, m.filter.bipolar))
            })
            .collect();
        let mods = st
            .modules
            .iter()
            .enumerate()
            .map(|(m, module)| {
                module
                    .arts
                    .iter()
                    .enumerate()
                    .map(|(a, art)| {
                        let centre = if bipolar(m, a) { 0.5 } else { 0.0 };
                        [mod_table(&art.maps[MAP_X], centre), mod_table(&art.maps[MAP_Y], centre)]
                    })
                    .collect()
            })
            .collect();
        let has_steps = st.modules.iter().any(|m| m.arts.iter().any(|a| (a.env.enabled && a.env.has_steps) || (a.lfo.enabled && a.lfo.has_steps)));
        let sends = (0..9).any(|r| used[r] && g[r][COL_SEND] != 0.0);
        let lfo_amount = signed_curve128(st.master_lfo);
        Patch { st, tables, shapers, mods, g, ring, used, per_sample, lfo_amount, has_steps, sends }
    }

    /// Whether an operator plays a plucked string.
    pub fn plucked(&self, o: usize) -> bool {
        self.st.modules[o].op.flags[PLUCKED] != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An edit reuses the tables of operators whose waveform settings stayed and builds the changed one's again.
    #[test]
    fn edits_reuse_unchanged_tables() {
        let mut st = crate::preset::default_patch();
        st.matrix[9 + COL_OUT] = 256;
        let mut kept = Tables::default();
        let a = Patch::with_tables(st.clone(), &mut kept);
        assert!(a.used[0] && a.used[1]);
        st.main_volume += 1;
        let b = Patch::with_tables(st.clone(), &mut kept);
        let same = |x: &Patch, y: &Patch, o: usize| Arc::ptr_eq(x.tables[o].as_ref().unwrap(), y.tables[o].as_ref().unwrap());
        assert!(same(&a, &b, 0) && same(&a, &b, 1));
        st.modules[1].op.shape[0] += 64;
        let c = Patch::with_tables(st.clone(), &mut kept);
        assert!(same(&b, &c, 0) && !same(&b, &c, 1));
        assert_eq!(c.tables[1].as_ref().unwrap().t, wave::build(&st.modules[1].op, st.switch(2) != 0).t);
    }
}
