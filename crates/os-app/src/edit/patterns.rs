// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The session's half of `crate::patterns`: what reads and changes the project, apart from the interface (specs
//! `roadmap/control-plane.md`, part 2, step 6).

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
#![allow(unused_imports)]

use os_model::{ClipSource, Pattern, Rgb};

pub fn name_of(s: &crate::session::Session, n: u16) -> String {
    s.project.pattern(n).and_then(|p| p.name.clone()).unwrap_or_else(|| format!("Pattern {n}"))
}

pub(crate) fn is_used(s: &crate::session::Session, n: u16) -> bool {
    s.project.pattern(n).is_some_and(used)
}

/// Lowest unused number.
pub fn first_empty(s: &crate::session::Session) -> u16 {
    (1..=MAX).find(|&n| !is_used(s, n)).unwrap_or(MAX)
}

/// Lowest unused number above `after`.
pub fn next_empty(s: &crate::session::Session, after: u16) -> u16 {
    (after.saturating_add(1)..=MAX).find(|&n| !is_used(s, n)).unwrap_or(MAX)
}

/// The pattern `n`, added when the project has none with that number.
pub fn pat_mut(s: &mut crate::session::Session, n: u16) -> &mut Pattern {
    let ps = &mut s.project.get_mut().patterns;
    let i = match ps.iter().position(|p| p.number == n) {
        Some(i) => i,
        None => {
            let at = ps.partition_point(|p| p.number < n);
            ps.insert(at, Pattern { number: n, ..Pattern::default() });
            at
        }
    };
    &mut ps[i]
}

/// `renumber` in the session; the views drop the clip and note selections.
pub(crate) fn renumber_in(s: &mut crate::session::Session, map: impl Fn(u16) -> Option<u16>) {
    let old: Vec<u16> = s.project.patterns.iter().map(|p| p.number).collect();
    let mut pats: Vec<Pattern> = std::mem::take(&mut s.project.get_mut().patterns)
        .into_iter()
        .filter_map(|mut p| {
            let n = map(p.number)?;
            p.number = n;
            Some(p)
        })
        .collect();
    for n in old {
        if !pats.iter().any(|p| p.number == n) {
            pats.push(Pattern { number: n, ..Pattern::default() });
        }
    }
    pats.sort_by_key(|p| p.number);
    s.project.get_mut().patterns = pats;
    if let Some(a) = s.project.get_mut().arrangement_mut() {
        a.clips.retain_mut(|c| match c.source {
            ClipSource::Pattern(n) => match map(n) {
                Some(m) => {
                    c.source = ClipSource::Pattern(m);
                    true
                }
                None => false,
            },
            _ => true,
        });
    }
    s.touched.views.push(crate::session::View::PatternsRenumbered);
    s.dirty = true;
}

/// "<name> #k" with the lowest free k >= 2 (§3 Clone).
pub fn clone_name(s: &crate::session::Session, n: u16) -> String {
    let name = name_of(s, n);
    let base = match name.rsplit_once(" #") {
        Some((b, k)) if k.parse::<u32>().is_ok() => b.to_string(),
        _ => name,
    };
    let taken = |cand: &str| s.project.patterns.iter().any(|p| p.name.as_deref() == Some(cand));
    (2..).map(|k| format!("{base} #{k}")).find(|c| !taken(c)).expect("unbounded")
}

/// Transpose every note of pattern `n` by `semis`.
pub fn transpose(s: &mut crate::session::Session, n: u16, semis: i32) {
    if let Some(p) = s.project.get_mut().patterns.iter_mut().find(|p| p.number == n) {
        for note in &mut p.notes {
            note.key = (i32::from(note.key) + semis).clamp(0, 131) as u16;
        }
        s.dirty = true;
    }
}

/// Highest pattern number (§1).
pub const MAX: u16 = 999;

/// A pattern with notes, control points or its own name (listed in menus, §2).
pub fn used(p: &Pattern) -> bool {
    !p.notes.is_empty() || !p.controls.is_empty() || p.name.is_some()
}

