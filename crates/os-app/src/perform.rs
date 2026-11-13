// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Performance mode (spec: ui/performance-mode.md): the arranger's time before the Start marker is a grid of clips
//! launched while the song plays. A launched clip plays as copies of itself placed on its track from the time it
//! starts (`Live::passes`); `rack::play_project` adds them to what the engine plays (`apply`), so playback needs
//! nothing of its own. The zone itself is never played as part of the song.

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use std::collections::BTreeMap;

use os_model::{Clip, Project};

use crate::playlist::{arr_mut, clips, start_marker, to_src, window};

/// How far ahead of its start a launched clip's copies are placed, in bars of 4 beats (Lemon Studio's design: the
/// engine plays what it is given; a trigger, a stop or an edit places them again).
pub(crate) const AHEAD_BARS: u32 = 256;

/// What the zone is playing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Perform {
    /// Each playing track's (0-based) clip.
    pub live: BTreeMap<u16, Live>,
    /// Round robin (§2): the next clip of each slot (track, column).
    pub turn: BTreeMap<(u16, usize), usize>,
    /// The track whose slot is held with the mouse (Press Hold & stop, Hold & motion).
    pub held: Option<u16>,
    /// The MIDI grid's first (track, column) (notes 120–123 scroll it) and whether it scrolls a page at a time (124).
    pub origin: (u16, usize),
    pub paging: bool,
    /// Note 125 (Scene mode) or 126 (+Scene mode, `true`) held on the MIDI channel: notes trigger their column's scene.
    pub scene: Option<bool>,
    /// Note 127 (Global Snap) held on the MIDI channel: triggers and stops sync to the global snap.
    pub snap: bool,
    /// The velocity of the MIDI note triggering, while it does: it picks the slot's sub-clip (§3a).
    pub vel: Option<u8>,
}

/// The MIDI grid (§3): notes 0–119 are this many tracks of this many columns.
pub const GRID: (u16, usize) = (10, 12);

/// A track's launched clip.
#[derive(Debug, Clone, PartialEq)]
pub struct Live {
    /// The copies placed, in time order: one per pass, the first starting `Position sync` into its clip.
    pub passes: Vec<Clip>,
    /// The column of the first pass (the slot triggered).
    pub column: usize,
    /// Silent from this tick on (a stop at a sync point); `None` = until the copies end.
    pub end: Option<u32>,
    /// Triggered while stopped: it starts with playback (§3).
    pub armed: bool,
}

impl Live {
    pub fn start(&self) -> u32 {
        self.passes.first().map_or(0, |c| c.position)
    }

    /// Where it falls silent.
    pub fn until(&self) -> u32 {
        let last = self.passes.last().map_or(0, Clip::end);
        self.end.map_or(last, |e| e.min(last))
    }

    /// The pass sounding at `tick`, if any.
    pub(crate) fn pass_at(&self, tick: u32) -> Option<&Clip> {
        self.passes.iter().find(|c| c.position <= tick && tick < c.end()).filter(|_| tick < self.until())
    }
}

/// `zone` on the session.
pub fn zone_in(s: &crate::session::Session) -> Option<u32> {
    (s.project.performance == Some(true)).then(|| start_marker(s)).flatten().filter(|&z| z > 0)
}

pub(crate) fn column_of(cols: &[u32], tick: u32) -> usize {
    cols.partition_point(|&c| c <= tick).saturating_sub(1)
}

/// The next point of a grid of `unit` ticks that starts at `z`, at or after `now`. With Tolerant, a point passed by
/// less than a 16th of the unit is taken instead: the clip starts at once with its beginning skipped (§4).
pub(crate) fn sync_point(now: u32, z: u32, unit: u32, tolerant: bool) -> u32 {
    let unit = i64::from(unit.max(1));
    let rel = i64::from(now) - i64::from(z);
    let past = rel.rem_euclid(unit);
    let at = if past == 0 || (tolerant && past < unit / 16) { rel - past } else { rel - past + unit };
    (i64::from(z) + at).max(0) as u32
}

/// A small generator for Random and Exclusive random, seeded by the trigger.
pub(crate) struct Rng(pub(crate) u64);

impl Rng {
    pub(crate) fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n.max(1) as u64) as usize
    }
}

/// The column played after `col` by `motion` (§4); `None` = stop.
pub(crate) fn step(motion: u8, col: usize, used: &[usize], rng: &mut Rng) -> Option<usize> {
    match motion {
        1 => None,
        2 => used.iter().copied().find(|&k| k > col).or(used.first().copied()),
        3 => Some(used.iter().copied().find(|&k| k > col).unwrap_or(col)),
        4 => used.iter().copied().find(|&k| k > col),
        5 if !used.is_empty() => Some(used[rng.below(used.len())]),
        6 if used.len() > 1 => {
            let others: Vec<usize> = used.iter().copied().filter(|&k| k != col).collect();
            Some(others[rng.below(others.len())])
        }
        _ => Some(col),
    }
}

pub(crate) fn rng(at: u32, track: u16) -> Rng {
    Rng(u64::from(at) << 16 ^ u64::from(track) ^ 0x9e37_79b9_7f4a_7c15)
}

/// `l`'s passes before `at` (or before it falls silent), the one sounding there cut short.
pub(crate) fn cut_at(s: &crate::session::Session, l: &Live, at: u32) -> Vec<Clip> {
    let at = at.min(l.until());
    let mut v: Vec<Clip> = l.passes.iter().filter(|c| c.position < at).copied().collect();
    if let Some(c) = v.last_mut().filter(|c| c.end() > at) {
        let keep = at - c.position;
        let (w0, _) = window(s, c);
        c.window = Some((w0, w0 + to_src(s, c.source, f64::from(keep))));
        c.length = keep;
    }
    v
}

/// Recording a performance (§3b, Lemon Studio's design): with Record armed, when the song stops, what each launched track
/// played after the Start marker up to `stop` becomes ordinary clips on its track, a clip a pass, the last cut where
/// it fell silent. On such a track the clips starting in the recorded stretch go and one running into it ends where it
/// starts. One undo step ("Recorded items"); what was launched is over. Returns the clips made.
pub fn record(s: &mut crate::session::Session, stop: u32) -> usize {
    let Some(z) = zone_in(s) else { return 0 };
    let takes: Vec<(u16, Vec<Clip>)> = s
        .perform
        .live
        .iter()
        .filter(|(_, l)| !l.armed)
        .map(|(&t, l)| (t, cut_at(s, l, stop).into_iter().filter(|c| c.position >= z && c.length > 0).collect::<Vec<_>>()))
        .filter(|(_, v)| !v.is_empty())
        .collect();
    if takes.is_empty() {
        return 0;
    }
    let mut all = clips(s).to_vec();
    let mut made = 0;
    for (t, v) in takes {
        let (from, to) = (v[0].position, v.last().map_or(0, Clip::end));
        all.retain(|c| c.track != t || c.position < from || c.position >= to);
        for c in all.iter_mut().filter(|c| c.track == t && c.position >= z && c.position < from && c.end() > from) {
            let keep = from - c.position;
            let (w0, _) = window(s, c);
            c.window = Some((w0, w0 + to_src(s, c.source, f64::from(keep))));
            c.length = keep;
        }
        made += v.len();
        all.extend(v.into_iter().map(|mut c| {
            c.set_group(0);
            c
        }));
    }
    crate::playlist::snapshot_as(s, crate::record::RECORDED);
    arr_mut(s).clips = all;
    s.perform.live.retain(|_, l| l.armed);
    s.dirty = true;
    made
}

/// Playback starts (§3): clips triggered while stopped start with it; what played before is over.
pub fn starting(s: &mut crate::session::Session) {
    let p = &mut s.perform;
    p.live.retain(|_, l| l.armed);
    for l in p.live.values_mut() {
        l.armed = false;
    }
    p.held = None;
}

/// The Marker settings dialog (§1): how many columns and how many bars apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarkerForm {
    pub count: u8,
    pub spacing: u8,
}

impl Default for MarkerForm {
    fn default() -> Self {
        MarkerForm { count: 16, spacing: 4 }
    }
}

impl MarkerForm {
    /// The values the boxes' lists offer (Lemon Studio's ranges; the original's were not measured).
    pub const COUNTS: std::ops::RangeInclusive<u8> = 1..=64;
    pub const SPACINGS: std::ops::RangeInclusive<u8> = 1..=16;
}

/// The text of the question Prepare for performance mode asks first (§1).
pub const CONFIRM: &str = "The project will be tweaked for best results in a live performance situation (no undo).";

/// The project as played (§3): the zone silent, each launched clip's passes on its track, and that track's own clips
/// silent while they play (Lemon Studio's design).
pub fn apply(s: &crate::session::Session, p: &mut Project) {
    let Some(z) = zone_in(s) else { return };
    let Some(a) = p.arrangement_mut() else { return };
    for c in a.clips.iter_mut().filter(|c| c.position < z) {
        c.muted = true;
    }
    let mut add = Vec::new();
    for (&t, l) in &s.perform.live {
        let (from, to) = (l.start(), l.until());
        if from >= to {
            continue;
        }
        for c in a.clips.iter_mut().filter(|c| c.track == t && c.position >= z && c.position < to && c.end() > from) {
            if c.position < from {
                let (w0, _) = window(s, c);
                c.window = Some((w0, w0 + to_src(s, c.source, f64::from(from - c.position))));
                c.length = from - c.position;
            } else {
                c.muted = true;
            }
        }
        add.extend(l.passes.iter().filter(|c| c.position < to).map(|c| {
            let mut c = *c;
            if c.end() > to {
                c.length = to - c.position;
            }
            c
        }));
    }
    a.clips.extend(add);
    a.clips.sort_by_key(|c| c.position);
}
