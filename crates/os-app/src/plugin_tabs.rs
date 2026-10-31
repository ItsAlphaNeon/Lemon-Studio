// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Plugin windows drawn in the app's (`plugin_window::EmbedFrame`) of one plugin as tabs of one window: General ▸
//! Miscellaneous "Show windows of one plugin as tabs". A second window of a plugin joins the first, and the window shows
//! one of them at a time; a tab dragged out of the row stands alone, to compare side by side, until it is closed.
//!
//! Every window of a group keeps its frame, with the group's place and a tab row (`EmbedFrame::set_tabs`). Those not
//! showing leave `State::z`, so they are neither painted nor clicked.

use std::collections::BTreeSet;

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
