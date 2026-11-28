// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Stable ids (`os_model::Id`) for the project's channels, patterns, mixer tracks, arrangements, playlist clips, mixer
//! slots' plugins, notes, control points, time markers, playlist tracks and automation points. A project file stores none, and any code may add or copy one of these, so the session gives
//! out ids here: `settle` gives every thing without an id a new one, and the second of two things holding the same id
//! (a copy) a new one too. An id is never given out twice in a session, so one that is gone (a deleted channel) can
//! only come back with its thing (an undo).

use std::collections::HashMap;

use os_model::{Id, Project};

/// The session's ids: the next one to give out and, per kind, where each id was at the last `settle` (which of two
/// copies keeps the id: the one still where the id was).
#[derive(Debug)]
pub struct Ids {
    next: u32,
    /// The next id for a note, control point, time marker, playlist track or automation point: those count from
    /// `PARTS` up, so the other things keep small numbers however many notes are made.
    next_part: u32,
    channels: HashMap<u32, usize>,
    patterns: HashMap<u32, usize>,
    tracks: HashMap<u32, usize>,
    arrangements: HashMap<u32, usize>,
    /// Clips by arrangement, track and position: a copy pasted before its original moves the original in the list but
    /// not on the playlist.
    clips: HashMap<u32, (usize, u16, u32)>,
    slots: HashMap<u32, (usize, usize)>,
    /// Notes and control points by pattern and place in its list.
    notes: HashMap<u32, (usize, usize)>,
    controls: HashMap<u32, (usize, usize)>,
    /// Time markers and playlist tracks by arrangement and place in its list.
    markers: HashMap<u32, (usize, usize)>,
    playlist_tracks: HashMap<u32, (usize, usize)>,
    /// Automation points by automation and place.
    points: HashMap<u32, (usize, usize)>,
}

impl Default for Ids {
    fn default() -> Self {
        Ids {
            next: 1,
            next_part: PARTS,
            channels: HashMap::new(),
            patterns: HashMap::new(),
            tracks: HashMap::new(),
            arrangements: HashMap::new(),
            clips: HashMap::new(),
            slots: HashMap::new(),
            notes: HashMap::new(),
            controls: HashMap::new(),
            markers: HashMap::new(),
            playlist_tracks: HashMap::new(),
            points: HashMap::new(),
        }
    }
}

/// Where the ids of notes, control points, time markers, playlist tracks and automation points start (`Ids::part`).
pub const PARTS: u32 = 1 << 31;

impl Ids {
    /// A new id for a channel, pattern, mixer track, arrangement, clip or slot plugin, never given out before in this
    /// session.
    pub fn fresh(&mut self) -> Id {
        let id = Id(self.next);
        self.next += 1;
        id
    }

    /// The id `fresh` gives next.
    pub fn next(&self) -> u32 {
        self.next
    }

    /// A new id for a note, control point, time marker, playlist track or automation point (from `PARTS` up).
    pub fn part(&mut self) -> Id {
        let id = Id(self.next_part);
        self.next_part += 1;
        id
    }

    /// Advances `next` and `next_part` past every id already present in `p` (after restoring saved ids from a `.lemon`
    /// file) so newly created objects never collide with restored ones.
    pub fn advance_past(&mut self, p: &Project) {
        let mut bump = |id: Id| {
            if id.0 == 0 {
                return;
            }
            if id.0 < PARTS {
                self.next = self.next.max(id.0.saturating_add(1));
            } else {
                self.next_part = self.next_part.max(id.0.saturating_add(1));
            }
        };
        for c in &p.channels {
            bump(c.id);
        }
        for q in &p.patterns {
            bump(q.id);
            for n in &q.notes {
                bump(n.id);
            }
            for c in &q.controls {
                bump(c.id);
            }
        }
        for m in &p.mixer {
            bump(m.id);
            for s in m.slots.iter().flatten() {
                bump(s.id);
            }
        }
        for a in &p.arrangements {
            bump(a.id);
            for c in &a.clips {
                bump(c.id);
            }
            for mk in &a.markers {
                bump(mk.id);
            }
            for t in &a.tracks {
                bump(t.id);
            }
        }
        for au in &p.automations {
            for pt in &au.points {
                bump(pt.id);
            }
        }
    }
}

/// Gives ids to one kind's things (`ids` in place order, `at` their places): a thing without one gets a new id, and of
/// two holding the same id the one at the id's place from the last settle keeps it (else the first). Returns whether
/// an id changed, and records the places.
fn settle_kind<K: Copy + Eq + std::hash::Hash>(next: &mut Ids, ids: Vec<(K, &mut Id)>, which: fn(&mut Ids) -> &mut HashMap<u32, K>) -> bool {
    settle_with(next, ids, which, Ids::fresh)
}

/// `settle_kind` with the ids `fresh` gives.
fn settle_with<K: Copy + Eq + std::hash::Hash>(next: &mut Ids, ids: Vec<(K, &mut Id)>, which: fn(&mut Ids) -> &mut HashMap<u32, K>, fresh: fn(&mut Ids) -> Id) -> bool {
    let mut owner: HashMap<u32, K> = HashMap::new();
    {
        let prev = which(next);
        for (at, id) in &ids {
            if id.is_none() {
                continue;
            }
            match owner.get(&id.0) {
                Some(_) if prev.get(&id.0) == Some(at) => {
                    owner.insert(id.0, *at);
                }
                Some(_) => {}
                None => {
                    owner.insert(id.0, *at);
                }
            }
        }
    }
    let mut changed = false;
    let mut places = HashMap::with_capacity(ids.len());
    for (at, id) in ids {
        if id.is_none() || owner.get(&id.0) != Some(&at) {
            *id = fresh(next);
            changed = true;
        }
        places.insert(id.0, at);
    }
    *which(next) = places;
    changed
}

/// Gives every thing in `p` that has no id, or another's, a new one (see the module). Returns whether one changed.
pub fn settle(ids: &mut Ids, p: &mut Project) -> bool {
    let mut changed = false;
    changed |= settle_kind(ids, p.channels.iter_mut().enumerate().map(|(i, c)| (i, &mut c.id)).collect(), |x| &mut x.channels);
    changed |= settle_kind(ids, p.patterns.iter_mut().enumerate().map(|(i, c)| (i, &mut c.id)).collect(), |x| &mut x.patterns);
    changed |= settle_kind(ids, p.mixer.iter_mut().enumerate().map(|(i, c)| (i, &mut c.id)).collect(), |x| &mut x.tracks);
    changed |= settle_kind(ids, p.arrangements.iter_mut().enumerate().map(|(i, c)| (i, &mut c.id)).collect(), |x| &mut x.arrangements);
    let clips = p.arrangements.iter_mut().enumerate().flat_map(|(a, arr)| arr.clips.iter_mut().map(move |c| ((a, c.track, c.position), &mut c.id))).collect();
    changed |= settle_kind(ids, clips, |x| &mut x.clips);
    let slots = p
        .mixer
        .iter_mut()
        .enumerate()
        .flat_map(|(t, m)| m.slots.iter_mut().enumerate().filter_map(move |(k, s)| s.as_mut().map(|s| ((t, k), &mut s.id))))
        .collect();
    changed |= settle_kind(ids, slots, |x| &mut x.slots);
    let notes = p.patterns.iter_mut().enumerate().flat_map(|(i, q)| q.notes.iter_mut().enumerate().map(move |(k, n)| ((i, k), &mut n.id))).collect();
    changed |= settle_with(ids, notes, |x| &mut x.notes, Ids::part);
    let controls = p.patterns.iter_mut().enumerate().flat_map(|(i, q)| q.controls.iter_mut().enumerate().map(move |(k, c)| ((i, k), &mut c.id))).collect();
    changed |= settle_with(ids, controls, |x| &mut x.controls, Ids::part);
    let markers = p.arrangements.iter_mut().enumerate().flat_map(|(a, arr)| arr.markers.iter_mut().enumerate().map(move |(k, m)| ((a, k), &mut m.id))).collect();
    changed |= settle_with(ids, markers, |x| &mut x.markers, Ids::part);
    let tracks = p.arrangements.iter_mut().enumerate().flat_map(|(a, arr)| arr.tracks.iter_mut().enumerate().map(move |(k, t)| ((a, k), &mut t.id))).collect();
    changed |= settle_with(ids, tracks, |x| &mut x.playlist_tracks, Ids::part);
    let points = p.automations.iter_mut().enumerate().flat_map(|(a, au)| au.points.iter_mut().enumerate().map(move |(k, q)| ((a, k), &mut q.id))).collect();
    changed |= settle_with(ids, points, |x| &mut x.points, Ids::part);
    changed
}

/// Forgets where things were: the project was replaced (opened, made). Ids already given out are not given again.
pub fn forget(ids: &mut Ids) {
    let (next, next_part) = (ids.next, ids.next_part);
    *ids = Ids { next, next_part, ..Ids::default() };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(st: &crate::settings_file::SettingsStore, channels: usize) -> Project {
        let mut p = Project::default();
        for i in 0..channels {
            p.channels.push(crate::projects::default_project(st).channels[0].clone());
            p.channels[i].index = i as u16;
        }
        p
    }

    #[test]
    fn every_thing_gets_its_own_id() {
        let st = &crate::settings_file::SettingsStore::memory();
        let mut ids = Ids::default();
        let mut p = project(st, 3);
        assert!(settle(&mut ids, &mut p));
        let got: Vec<u32> = p.channels.iter().map(|c| c.id.0).collect();
        assert!(got.iter().all(|&i| i != 0));
        assert_eq!(got.iter().collect::<std::collections::HashSet<_>>().len(), 3);
        assert!(!settle(&mut ids, &mut p), "a settled project keeps its ids");
    }

    #[test]
    fn ids_follow_moves_and_deletes() {
        let st = &crate::settings_file::SettingsStore::memory();
        let mut ids = Ids::default();
        let mut p = project(st, 3);
        settle(&mut ids, &mut p);
        let (a, c) = (p.channels[0].id.0, p.channels[2].id.0);
        p.channels.remove(1);
        p.channels.reverse();
        assert!(!settle(&mut ids, &mut p));
        assert_eq!((p.channels[0].id.0, p.channels[1].id.0), (c, a));
    }

    #[test]
    fn a_copy_gets_a_new_id_and_the_original_keeps_its_own() {
        let st = &crate::settings_file::SettingsStore::memory();
        // A copy below its original (Clone channel, a pasted clip): the one still at the id's place keeps it.
        let mut ids = Ids::default();
        let mut p = project(st, 2);
        settle(&mut ids, &mut p);
        let a = p.channels[0].id.0;
        let copy = p.channels[0].clone();
        p.channels.push(copy);
        settle(&mut ids, &mut p);
        assert_eq!(p.channels[0].id.0, a);
        assert_ne!(p.channels[2].id.0, a);
    }

    /// Notes, control points, time markers, playlist tracks and automation points get ids too: a note keeps its own when
    /// the pattern's notes are sorted, and a pasted copy gets a new one.
    #[test]
    fn notes_markers_tracks_and_points_get_ids() {
        use os_model::{Arrangement, AutoPoint, Automation, ControlPoint, Note, Pattern, PlaylistTrack, TimeMarker};
        let mut ids = Ids::default();
        let mut p = Project::default();
        p.patterns.push(Pattern { number: 1, notes: vec![Note::step(0, 96), Note::step(0, 0)], controls: vec![ControlPoint::new(0, 0, 0, 100)], ..Pattern::default() });
        let marker = TimeMarker { position: 0, kind: 0, name: "Verse".into(), numerator: 4, denominator: 4, scale: 0, root: 0, flag: 0, scale_index: 0, id: Id::NONE };
        p.arrangements.push(Arrangement { markers: vec![marker], tracks: vec![PlaylistTrack::new(1), PlaylistTrack::new(2)], ..Arrangement::default() });
        p.automations.push(Automation { channel: 0, targets: Vec::new(), points: vec![AutoPoint::new(0.0, 0.5), AutoPoint::new(1.0, 1.0)], header: vec![0; 21], tail: Vec::new(), lfo: Default::default() });
        assert!(settle(&mut ids, &mut p));
        let q = &p.patterns[0];
        let all = [q.notes[0].id, q.notes[1].id, q.controls[0].id, p.arrangements[0].markers[0].id, p.arrangements[0].tracks[0].id, p.arrangements[0].tracks[1].id, p.automations[0].points[0].id, p.automations[0].points[1].id];
        assert!(all.iter().all(|i| !i.is_none()));
        assert_eq!(all.iter().map(|i| i.0).collect::<std::collections::HashSet<_>>().len(), all.len(), "no two share one");
        assert!(all.iter().all(|i| i.0 >= PARTS), "they count from PARTS up, apart from the channels' and patterns'");
        assert!(!settle(&mut ids, &mut p), "a settled project keeps its ids");
        let late = p.patterns[0].notes[0].id.0;
        p.patterns[0].notes.sort_by_key(|n| n.position);
        let copy = p.patterns[0].notes[1];
        p.patterns[0].notes.push(copy);
        settle(&mut ids, &mut p);
        let notes = &p.patterns[0].notes;
        assert_eq!(notes[1].id.0, late, "the note at 96 kept its id after the sort");
        assert!(notes[2].id.0 != late && !notes[2].id.is_none(), "its copy got a new one");
    }

    /// `OS_IDS_FLP=<a.flp>;<b.flp> cargo test -p os-session ids_settle_time -- --ignored --nocapture`: how long a first
    /// settle (every thing new) and a settled one take on each project, by its notes.
    #[test]
    #[ignore]
    fn ids_settle_time() {
        let Ok(list) = std::env::var("OS_IDS_FLP") else { return };
        for path in list.split(';').filter(|p| !p.is_empty()) {
            let Some(mut p) = std::fs::read(path).ok().and_then(|b| os_flp::FlpFile::parse(&b).ok()).and_then(|f| os_flp::project::load(&f).ok()) else { continue };
            let notes: usize = p.patterns.iter().map(|q| q.notes.len()).sum();
            let mut ids = Ids::default();
            let t = std::time::Instant::now();
            settle(&mut ids, &mut p);
            let first = t.elapsed();
            let t = std::time::Instant::now();
            settle(&mut ids, &mut p);
            println!("{notes:>7} notes  first {first:>10.2?}  again {:>10.2?}  {path}", t.elapsed());
        }
    }

    #[test]
    fn ids_are_not_given_twice() {
        let st = &crate::settings_file::SettingsStore::memory();
        let mut ids = Ids::default();
        let mut p = project(st, 2);
        settle(&mut ids, &mut p);
        let gone = p.channels.pop().unwrap().id.0;
        p.channels.push(crate::projects::default_project(st).channels[0].clone());
        p.channels[1].id = Id::NONE;
        settle(&mut ids, &mut p);
        assert_ne!(p.channels[1].id.0, gone);
        forget(&mut ids);
        let mut q = project(st, 1);
        settle(&mut ids, &mut q);
        assert!(q.channels[0].id.0 > gone, "ids after a new project are new too");
    }
}
