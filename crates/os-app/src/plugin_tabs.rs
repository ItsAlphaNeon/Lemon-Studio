// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Plugin windows drawn in the app's (`plugin_window::EmbedFrame`) of one plugin as tabs of one window: General ▸
//! Miscellaneous "Show windows of one plugin as tabs". A second window of a plugin joins the first, and the window shows
//! one of them at a time; a tab dragged out of the row stands alone, to compare side by side, until it is closed.
//!
//! Dragged back: a window held over another window of its plugin shows, after a moment, what letting go
//! gives, in its own row of tabs rather than on the window below (`Held`, `Preview`).
//!
//! Every window of a group keeps its frame, with the group's place and a tab row (`EmbedFrame::set_tabs`). Those not
//! showing leave `State::z`, so they are neither painted nor clicked.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use os_ui::Rect;

/// How far a tab's slanted edge leans over the row's height: well past a "/".
const SLANT: f32 = 18.0;
/// The widest a tab gets.
const TAB_W: f32 = 190.0;
/// Stacked: the least of each tab the next one lies over.
const MIN_OVERLAP: f32 = 72.0;
/// The least of a tab that has room for its close button.
const CLOSE_ROOM: f32 = 36.0;
const CLOSE: f32 = 14.0;

/// How long a window is held over another before it shows what letting go gives.
pub const MERGE_HOLD: Duration = Duration::from_millis(200);
/// How far past the other window the mouse may go once the window shows it.
pub const MERGE_KEEP: f32 = 10.0;

/// A window being dragged, over another window of its plugin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Held {
    pub row: usize,
    pub into: usize,
    pub since: Instant,
    /// The hold is over: the window shows what letting go gives.
    pub ripe: bool,
}

impl Held {
    /// The time runs while the mouse stays over window `into`, moving or not.
    pub fn new(row: usize, into: usize, since: Instant) -> Held {
        Held { row, into, since, ripe: false }
    }

    /// Whether the hold has just come to its end.
    pub fn ripen(&mut self, now: Instant) -> bool {
        let done = !self.ripe && now.duration_since(self.since) >= MERGE_HOLD;
        self.ripe |= done;
        done
    }
}

/// What the held window shows in its row of tabs: the group it would make, itself showing.
#[derive(Debug, Clone, PartialEq)]
pub struct Preview {
    pub row: usize,
    pub rows: Vec<usize>,
}

/// One tab of a row, in the row's coordinates (`row_shapes`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TabShape {
    /// Its place in `Group::rows`.
    pub index: usize,
    /// The row's rectangle, top and bottom.
    y: f32,
    h: f32,
    /// Where its left and right edges meet the bottom; they lean right by `SLANT` at the top.
    left: f32,
    right: f32,
    /// The left edge is upright (the row's first tab, against the window's edge).
    upright: bool,
    /// What shows of it halfway up, when others lie over it.
    pub shown: (f32, f32),
    /// How many tabs lie between it and the one showing, when stacked.
    pub depth: usize,
}

impl TabShape {
    /// Its outline.
    pub fn outline(&self) -> [(f32, f32); 4] {
        let top = self.y;
        let bottom = self.y + self.h;
        let top_left = if self.upright { self.left } else { self.left + SLANT };
        [(self.left, bottom), (top_left, top), (self.right + SLANT, top), (self.right, bottom)]
    }

    /// Its span at height `y`.
    fn span_at(&self, y: f32) -> (f32, f32) {
        let up = ((self.y + self.h - y) / self.h).clamp(0.0, 1.0) * SLANT;
        (if self.upright { self.left } else { self.left + up }, self.right + up)
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        let (a, b) = self.span_at(y);
        y >= self.y && y < self.y + self.h && x >= a && x < b
    }

    /// Its area as thin rectangles (a hit is a rectangle).
    pub fn bands(&self) -> impl Iterator<Item = Rect> + '_ {
        const BANDS: usize = 6;
        (0..BANDS).map(move |k| {
            let bh = self.h / BANDS as f32;
            let y = self.y + bh * k as f32;
            let (a, b) = self.span_at(y + bh / 2.0);
            Rect::new(a, y, b - a, bh)
        })
    }

    /// Its close button, when enough of it shows.
    pub fn close(&self) -> Option<Rect> {
        let (a, b) = self.shown;
        // The edge leans away from the middle by this much at the button's corners.
        let lean = SLANT * CLOSE / 2.0 / self.h;
        (b - a >= CLOSE_ROOM).then(|| Rect::new(b - lean - CLOSE - 3.0, self.y + (self.h - CLOSE) / 2.0, CLOSE, CLOSE))
    }

    /// Where its name may go, halfway up.
    pub fn label(&self) -> (f32, f32) {
        let start = self.shown.0 + 8.0;
        (start, self.close().map_or(self.shown.1 - 4.0, |c| c.x - 3.0))
    }
}

/// The tabs of a row of `n`, `active` showing, in painting order: the last lies over the others. Side by side, or
/// `stacked`: each lies partly under its neighbour towards the one showing.
pub fn row_shapes(row: Rect, n: usize, active: usize, stacked: bool) -> Vec<TabShape> {
    let room = (row.w - SLANT).max(1.0);
    let count = n.max(1) as f32;
    let (w, step) = if stacked {
        let w = room.min(TAB_W);
        let most = (w - MIN_OVERLAP).max(1.0);
        (w, if n > 1 { ((room - w) / (count - 1.0)).min(most) } else { most })
    } else {
        let w = (room / count).min(TAB_W);
        (w, w)
    };
    let edge = |i: usize| row.x + step * i as f32;
    let mid = SLANT / 2.0;
    let shape = |i: usize| {
        let left = edge(i);
        let right = left + w;
        let shown = if !stacked || i == active {
            (left + mid, right + mid)
        } else if i < active {
            (left + mid, edge(i + 1) + mid)
        } else {
            (edge(i - 1) + w + mid, right + mid)
        };
        TabShape { index: i, y: row.y, h: row.h, left, right, upright: i == 0, shown, depth: if stacked { i.abs_diff(active) } else { 0 } }
    };
    let mut order: Vec<usize> = (0..active.min(n)).collect();
    order.extend((active + 1..n).rev());
    if active < n {
        order.push(active);
    }
    order.into_iter().map(shape).collect()
}

/// The tab at (`x`, `y`) among `shapes`: the one lying on top.
pub fn tab_at(shapes: &[TabShape], x: f32, y: f32) -> Option<usize> {
    shapes.iter().rev().find(|s| s.contains(x, y)).map(|s| s.index)
}

/// Windows of one plugin shown as tabs: two or more.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    kind: String,
    pub rows: Vec<usize>,
    pub active: usize,
}

/// A window took the group's place: `row` goes where `from` is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Joined {
    pub row: usize,
    pub from: usize,
}

#[derive(Debug, Default)]
pub struct Tabs {
    groups: Vec<Group>,
    /// The open windows at the last look, by plugin.
    open: Vec<(usize, String)>,
    /// Dragged out of a group: they stand alone until closed.
    apart: BTreeSet<usize>,
}

impl Tabs {
    /// The windows open now (key and plugin), with the setting `on`. Returns those that took a group's place.
    pub fn sync(&mut self, open: &[(usize, String)], on: bool) -> Vec<Joined> {
        let mut joined = Vec::new();
        let is_open = |r: usize| open.iter().any(|o| o.0 == r);
        self.apart.retain(|&r| is_open(r));
        for g in &mut self.groups {
            if !is_open(g.active) {
                let at = g.rows.iter().position(|&r| r == g.active).unwrap_or(0);
                let next = g.rows.iter().skip(at + 1).chain(g.rows[..at].iter().rev()).find(|&&r| is_open(r));
                g.active = next.copied().unwrap_or(g.active);
            }
            g.rows.retain(|&r| is_open(r));
        }
        self.groups.retain(|g| g.rows.len() > 1);
        if !on {
            self.groups.clear();
            self.apart.clear();
            self.open = open.to_vec();
            return joined;
        }
        let was = std::mem::take(&mut self.open);
        for (row, kind) in open {
            if was.iter().any(|w| w.0 == *row) || self.apart.contains(row) {
                continue;
            }
            if let Some(g) = self.groups.iter_mut().find(|g| g.kind == *kind) {
                joined.push(Joined { row: *row, from: g.active });
                g.rows.push(*row);
                g.active = *row;
                continue;
            }
            let grouped = |r: usize, gs: &[Group]| gs.iter().any(|g| g.rows.contains(&r));
            let alone = open.iter().find(|(r, k)| r != row && k == kind && !self.apart.contains(r) && !grouped(*r, &self.groups) && (was.iter().any(|w| w.0 == *r) || r < row));
            if let Some(first) = alone.map(|o| o.0) {
                joined.push(Joined { row: *row, from: first });
                self.groups.push(Group { kind: kind.clone(), rows: vec![first, *row], active: *row });
            }
        }
        self.open = open.to_vec();
        joined
    }

    /// The group of window `row`.
    pub fn group_of(&self, row: usize) -> Option<&Group> {
        self.groups.iter().find(|g| g.rows.contains(&row))
    }

    /// Whether `row` is a tab that is not showing.
    pub fn hidden(&self, row: usize) -> bool {
        self.group_of(row).is_some_and(|g| g.active != row)
    }

    /// The groups' places: a tab not showing, and the one that is.
    pub fn followers(&self) -> Vec<(usize, usize)> {
        self.groups.iter().flat_map(|g| g.rows.iter().filter(|&&r| r != g.active).map(|&r| (r, g.active))).collect()
    }

    /// Shows tab `row`. Whether it was not showing.
    pub fn activate(&mut self, row: usize) -> bool {
        match self.groups.iter_mut().find(|g| g.rows.contains(&row)) {
            Some(g) if g.active != row => {
                g.active = row;
                true
            }
            _ => false,
        }
    }

    /// The windows `row` (with its tabs) and `into` (with its tabs) would make, in the order of their rows.
    pub fn merged(&self, row: usize, into: usize) -> Vec<usize> {
        let of = |r: usize| self.group_of(r).map_or(vec![r], |g| g.rows.clone());
        let mut rows = of(into);
        rows.extend(of(row).into_iter().filter(|r| !rows.contains(r)).collect::<Vec<_>>());
        rows
    }

    /// `row` (with its tabs) joins `into` (with its tabs), `row` showing.
    pub fn merge(&mut self, row: usize, into: usize, kind: &str) {
        let rows = self.merged(row, into);
        self.groups.retain(|g| !g.rows.contains(&row) && !g.rows.contains(&into));
        for r in &rows {
            self.apart.remove(r);
        }
        self.groups.push(Group { kind: kind.to_string(), rows, active: row });
    }

    /// Tab `row` leaves its group, to stand alone.
    pub fn pull_out(&mut self, row: usize) {
        for g in &mut self.groups {
            if g.rows.contains(&row) {
                let at = g.rows.iter().position(|&r| r == row).unwrap_or(0);
                g.rows.retain(|&r| r != row);
                if g.active == row {
                    g.active = g.rows[at.min(g.rows.len() - 1)];
                }
            }
        }
        // A group of one was two: both stand alone now.
        self.groups.retain(|g| g.rows.len() > 1);
        self.apart.insert(row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROW: Rect = Rect { x: 100.0, y: 50.0, w: 500.0, h: 26.0 };

    #[test]
    fn side_by_side_tabs_touch_along_their_slices() {
        let s = row_shapes(ROW, 3, 1, false);
        let by = |i: usize| s.iter().find(|t| t.index == i).unwrap();
        assert_eq!(by(0).outline()[3], by(1).outline()[0], "no gap at the bottom");
        assert_eq!(by(0).outline()[2], by(1).outline()[1], "no gap at the top");
        assert!(by(1).outline()[1].0 - by(1).outline()[0].0 > ROW.h / 2.0, "leans well past a slash");
        assert_eq!(by(0).outline()[0].0, by(0).outline()[1].0, "the first stands against the window's edge");
        // A point just either side of the slice, high up and low down.
        for y in [ROW.y + 2.0, ROW.y + 24.0] {
            let (_, b) = by(0).span_at(y);
            assert_eq!(tab_at(&s, b - 1.0, y), Some(0));
            assert_eq!(tab_at(&s, b + 1.0, y), Some(1));
        }
    }

    #[test]
    fn stacked_tabs_can_all_be_clicked_and_the_showing_one_lies_on_top() {
        for n in 2..=9 {
            for active in 0..n {
                let s = row_shapes(ROW, n, active, true);
                assert_eq!(s.last().unwrap().index, active);
                assert_eq!(s.len(), n);
                for t in &s {
                    let (a, b) = t.shown;
                    assert!(b > a, "{n} tabs, {active} showing: tab {} hidden", t.index);
                    assert_eq!(tab_at(&s, (a + b) / 2.0, ROW.cy()), Some(t.index), "{n} tabs, {active} showing");
                    if let Some(c) = t.close() {
                        assert_eq!(tab_at(&s, c.cx(), c.cy()), Some(t.index), "{n} tabs, {active} showing: its close button");
                        assert_eq!(tab_at(&s, c.x, c.y), Some(t.index), "{n} tabs, {active} showing: the button's corner");
                    }
                }
            }
        }
        let few = row_shapes(ROW, 2, 0, true);
        let (first, second) = (few[1], few[0]);
        assert!(second.left < first.right, "they overlap even with room to spare");
    }

    #[test]
    fn every_tab_has_a_close_button_with_room_for_a_name() {
        for stacked in [false, true] {
            for t in row_shapes(ROW, 4, 2, stacked) {
                let c = t.close().unwrap();
                assert!(t.contains(c.x, c.y) && t.contains(c.right(), c.bottom() - 0.5), "stacked {stacked}: tab {}", t.index);
                let (a, b) = t.label();
                assert!(b - a > 10.0, "stacked {stacked}: tab {}", t.index);
            }
        }
    }

    #[test]
    fn the_hit_bands_cover_the_shape() {
        let s = row_shapes(ROW, 3, 1, false)[0];
        let bands: Vec<Rect> = s.bands().collect();
        assert_eq!(bands.first().unwrap().y, ROW.y);
        assert!((bands.last().unwrap().bottom() - ROW.bottom()).abs() < 0.01);
    }

    fn open(rows: &[(usize, &str)]) -> Vec<(usize, String)> {
        rows.iter().map(|&(r, k)| (r, k.to_string())).collect()
    }

    #[test]
    fn a_second_window_of_a_plugin_joins_the_first() {
        let mut t = Tabs::default();
        assert!(t.sync(&open(&[(1, "Zest"), (2, "Citrus")]), true).is_empty());
        assert!(t.group_of(1).is_none(), "one window is no group");
        let j = t.sync(&open(&[(1, "Zest"), (2, "Citrus"), (5, "Zest")]), true);
        assert_eq!(j, [Joined { row: 5, from: 1 }]);
        let g = t.group_of(5).unwrap();
        assert_eq!((g.rows.as_slice(), g.active), (&[1, 5][..], 5), "the new one shows");
        assert!(t.hidden(1) && !t.hidden(5) && !t.hidden(2));
        assert_eq!(t.sync(&open(&[(1, "Zest"), (2, "Citrus"), (5, "Zest"), (7, "Zest")]), true), [Joined { row: 7, from: 5 }]);
        assert_eq!(t.followers(), [(1, 7), (5, 7)]);
    }

    #[test]
    fn windows_opened_together_group_at_once() {
        let mut t = Tabs::default();
        let j = t.sync(&open(&[(1, "Zest"), (5, "Zest"), (7, "Zest")]), true);
        assert_eq!(j, [Joined { row: 5, from: 1 }, Joined { row: 7, from: 5 }]);
        assert_eq!(t.group_of(1).unwrap().rows, [1, 5, 7]);
    }

    #[test]
    fn closing_shows_the_next_and_the_last_stands_alone() {
        let mut t = Tabs::default();
        t.sync(&open(&[(1, "Zest"), (5, "Zest"), (7, "Zest")]), true);
        assert!(t.activate(5));
        assert!(!t.activate(5), "already showing");
        t.sync(&open(&[(1, "Zest"), (7, "Zest")]), true);
        assert_eq!(t.group_of(1).unwrap().active, 7, "the next one");
        t.sync(&open(&[(1, "Zest")]), true);
        assert!(t.group_of(1).is_none() && !t.hidden(1));
    }

    #[test]
    fn a_tab_dragged_out_stands_alone_until_closed() {
        let mut t = Tabs::default();
        let all = open(&[(1, "Zest"), (5, "Zest"), (7, "Zest")]);
        t.sync(&all, true);
        t.pull_out(7);
        assert!(t.group_of(7).is_none());
        assert_eq!(t.group_of(1).unwrap().active, 5);
        assert!(t.sync(&all, true).is_empty(), "it stays out");
        t.pull_out(5);
        assert!(t.group_of(1).is_none(), "two apart, one left: no group");
        // Closed and opened again, it joins again.
        t.sync(&open(&[(1, "Zest"), (5, "Zest")]), true);
        assert!(t.sync(&open(&[(1, "Zest")]), true).is_empty());
        let j = t.sync(&open(&[(1, "Zest"), (5, "Zest")]), true);
        assert_eq!(j, [Joined { row: 5, from: 1 }]);
    }

    #[test]
    fn windows_apart_go_back_together() {
        let mut t = Tabs::default();
        let all = open(&[(1, "Zest"), (5, "Zest"), (7, "Zest"), (9, "Zest")]);
        t.sync(&all, true);
        t.pull_out(7);
        t.pull_out(9);
        assert_eq!(t.merged(9, 7), [7, 9]);
        t.merge(9, 7, "Zest");
        let g = t.group_of(7).unwrap();
        assert_eq!((g.rows.as_slice(), g.active), (&[7, 9][..], 9), "the one let go shows");
        assert!(t.sync(&all, true).is_empty(), "and they stay");
        assert_eq!(t.group_of(9).unwrap().rows, [7, 9]);
        // A whole group onto another: all its tabs go with it.
        assert_eq!(t.merged(5, 7), [7, 9, 1, 5]);
        t.merge(5, 7, "Zest");
        let g = t.group_of(1).unwrap();
        assert_eq!((g.rows.as_slice(), g.active), (&[7, 9, 1, 5][..], 5));
        assert_eq!(t.followers().len(), 3);
    }

    #[test]
    fn the_hold_shows_the_group_after_a_moment() {
        let t0 = Instant::now();
        let later = |ms: u64| t0 + Duration::from_millis(ms);
        let mut h = Held::new(1, 5, t0);
        assert!(!h.ripen(later(199)));
        assert!(h.ripen(later(200)), "a moment over the other window");
        assert!(!h.ripen(later(400)) && h.ripe, "once, and it stays");
    }

    #[test]
    fn the_setting_off_shows_them_all() {
        let mut t = Tabs::default();
        let all = open(&[(1, "Zest"), (5, "Zest")]);
        t.sync(&all, true);
        assert!(t.hidden(1));
        t.sync(&all, false);
        assert!(!t.hidden(1) && t.group_of(1).is_none());
        // They do not regroup by themselves when it comes back on, but the next window joins.
        assert!(t.sync(&all, true).is_empty());
    }
}
