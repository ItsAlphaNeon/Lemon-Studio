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

pub fn is_used(s: &crate::session::Session, n: u16) -> bool {
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
pub fn renumber_in(s: &mut crate::session::Session, map: impl Fn(u16) -> Option<u16>) {
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

/// Transpose every note of pattern `n` by `semis`; with channels in `only` (their indexes), only their notes.
pub fn transpose(s: &mut crate::session::Session, n: u16, semis: i32, only: &[u16]) {
    if let Some(p) = s.project.get_mut().patterns.iter_mut().find(|p| p.number == n) {
        for note in p.notes.iter_mut().filter(|x| only.is_empty() || only.contains(&x.channel)) {
            note.key = (i32::from(note.key) + semis).clamp(0, 131) as u16;
        }
        s.dirty = true;
    }
}

/// PATTERNS > Split by channel (spec: ui/split-by-channel.md §2) on pattern `cur`. Channels with notes in it, in rack
/// order: the pattern keeps the first one's notes and takes its name; each further one gets a pattern of its own
/// named after it, right after it in channel order (later patterns move up with their clips, as Insert one), with the
/// original's colour, markers and other settings but only that channel's notes. Every playlist clip of the original
/// gets one clip per new pattern at the same place, under it. No notes, or no room: nothing happens.
pub fn split_by_channel(s: &mut crate::session::Session, cur: u16) {
    let Some(orig) = s.project.pattern(cur).cloned() else { return };
    let chans: Vec<(u16, String)> = s
        .project
        .channels
        .iter()
        .enumerate()
        .filter(|(_, c)| orig.notes.iter().any(|n| n.channel == c.index))
        .map(|(r, c)| (c.index, crate::edit::rack::channel_name(s, r)))
        .collect();
    let Some(((first, name), rest)) = chans.split_first() else { return };
    let k = rest.len() as u16;
    // Room for the new numbers: no used pattern may move past the last one.
    let last = s.project.patterns.iter().filter(|p| used(p)).map(|p| p.number).max().unwrap_or(cur).max(cur);
    if usize::from(last) + rest.len() > usize::from(MAX) {
        return;
    }
    // Notes of a channel no longer in the rack stay with the original.
    let p = pat_mut(s, cur);
    p.notes.retain(|n| n.channel == *first || !chans.iter().any(|c| c.0 == n.channel));
    p.name = Some(name.clone());
    s.dirty = true;
    if k == 0 {
        return;
    }
    renumber_in(s, |n| Some(if n > cur { n + k } else { n }));
    for (j, (ch, name)) in (1..).zip(rest) {
        // Control points stay with the original (only notes are divided).
        let notes = orig.notes.iter().filter(|n| n.channel == *ch).copied().collect();
        *pat_mut(s, cur + j) = Pattern { number: cur + j, name: Some(name.clone()), notes, controls: Vec::new(), id: os_model::Id::NONE, ..orig.clone() };
    }
    if let Some(a) = s.project.get_mut().arrangement_mut() {
        // Clips are drawn in record order: the new ones go just before their original, which stays on top.
        let mut clips = Vec::with_capacity(a.clips.len());
        for c in std::mem::take(&mut a.clips) {
            if c.source == ClipSource::Pattern(cur) {
                clips.extend((1..=k).map(|j| os_model::Clip { source: ClipSource::Pattern(cur + j), ..c }));
            }
            clips.push(c);
        }
        a.clips = clips;
    }
}

/// Pattern `n`'s time signature at its start (spec: ui/pattern-time-signature.md §3-5): `Some((num, den))` puts it
/// first among its markers, named as FL names it ("3/8"); `None` takes it away. The pattern's bars follow, and so do
/// its clips that kept their natural length.
pub fn set_signature(s: &mut crate::session::Session, n: u16, sig: Option<(u8, u8)>) {
    use os_model::TimeMarker;
    let old = s.project.pattern_length(n);
    let markers = &mut pat_mut(s, n).markers;
    markers.retain(|m| !(m.kind == TimeMarker::SIGNATURE && m.position == 0));
    if let Some((num, den)) = sig {
        markers.insert(0, TimeMarker { kind: TimeMarker::SIGNATURE, numerator: num, denominator: den, ..TimeMarker::new(0, &format!("{num}/{den}")) });
    }
    crate::edit::playlist::refit_pattern_clips(s, n, old);
}

/// Highest pattern number (§1).
pub const MAX: u16 = 999;

/// A pattern with notes, control points or its own name (listed in menus, §2).
pub fn used(p: &Pattern) -> bool {
    !p.notes.is_empty() || !p.controls.is_empty() || p.name.is_some()
}

pub fn random_color(seed: u64) -> Rgb {
    // A random hue at a muted saturation / lightness like the default pattern colours.
    let mut x = seed ^ 0x9e37_79b9_7f4a_7c15;
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    let h = (x % 360) as f32;
    let (sat, l) = (0.35f32, 0.45f32);
    let c = (1.0 - (2.0 * l - 1.0).abs()) * sat;
    let hp = h / 60.0;
    let xx = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, xx, 0.0),
        1 => (xx, c, 0.0),
        2 => (0.0, c, xx),
        3 => (0.0, xx, c),
        4 => (xx, 0.0, c),
        _ => (c, 0.0, xx),
    };
    let m = l - c / 2.0;
    let u = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Rgb::new(u(r), u(g), u(b))
}


