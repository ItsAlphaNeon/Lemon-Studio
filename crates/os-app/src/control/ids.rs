// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Stable ids (`os_model::Id`) for the project's channels, patterns, mixer tracks, arrangements, playlist clips and
//! mixer slots' plugins. A project file stores none, and any code may add or copy one of these, so the session gives
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
    channels: HashMap<u32, usize>,
    patterns: HashMap<u32, usize>,
    tracks: HashMap<u32, usize>,
    arrangements: HashMap<u32, usize>,
    /// Clips by arrangement, track and position: a copy pasted before its original moves the original in the list but
    /// not on the playlist.
    clips: HashMap<u32, (usize, u16, u32)>,
    slots: HashMap<u32, (usize, usize)>,
}

impl Default for Ids {
    fn default() -> Self {
        Ids {
            next: 1,
            channels: HashMap::new(),
            patterns: HashMap::new(),
            tracks: HashMap::new(),
            arrangements: HashMap::new(),
            clips: HashMap::new(),
            slots: HashMap::new(),
        }
    }
}

impl Ids {
    fn fresh(&mut self) -> Id {
        let id = Id(self.next);
        self.next += 1;
        id
    }
}

/// Gives ids to one kind's things (`ids` in place order, `at` their places): a thing without one gets a new id, and of
/// two holding the same id the one at the id's place from the last settle keeps it (else the first). Returns whether
/// an id changed, and records the places.
fn settle_kind<K: Copy + Eq + std::hash::Hash>(next: &mut Ids, ids: Vec<(K, &mut Id)>, which: fn(&mut Ids) -> &mut HashMap<u32, K>) -> bool {
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
            *id = next.fresh();
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
    changed
}

/// Forgets where things were: the project was replaced (opened, made). Ids already given out are not given again.
pub fn forget(ids: &mut Ids) {
    let next = ids.next;
    *ids = Ids { next, ..Ids::default() };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(channels: usize) -> Project {
        let mut p = Project::default();
        for i in 0..channels {
            p.channels.push(crate::app::default_project().channels[0].clone());
            p.channels[i].index = i as u16;
        }
        p
    }

    #[test]
    fn every_thing_gets_its_own_id() {
        let mut ids = Ids::default();
        let mut p = project(3);
        assert!(settle(&mut ids, &mut p));
        let got: Vec<u32> = p.channels.iter().map(|c| c.id.0).collect();
        assert!(got.iter().all(|&i| i != 0));
        assert_eq!(got.iter().collect::<std::collections::HashSet<_>>().len(), 3);
        assert!(!settle(&mut ids, &mut p), "a settled project keeps its ids");
    }

    #[test]
    fn ids_follow_moves_and_deletes() {
        let mut ids = Ids::default();
        let mut p = project(3);
        settle(&mut ids, &mut p);
        let (a, c) = (p.channels[0].id.0, p.channels[2].id.0);
        p.channels.remove(1);
        p.channels.reverse();
        assert!(!settle(&mut ids, &mut p));
        assert_eq!((p.channels[0].id.0, p.channels[1].id.0), (c, a));
    }

    #[test]
    fn a_copy_gets_a_new_id_and_the_original_keeps_its_own() {
        // A copy below its original (Clone channel, a pasted clip): the one still at the id's place keeps it.
        let mut ids = Ids::default();
        let mut p = project(2);
        settle(&mut ids, &mut p);
        let a = p.channels[0].id.0;
        let copy = p.channels[0].clone();
        p.channels.push(copy);
        settle(&mut ids, &mut p);
        assert_eq!(p.channels[0].id.0, a);
        assert_ne!(p.channels[2].id.0, a);
    }

    #[test]
    fn ids_are_not_given_twice() {
        let mut ids = Ids::default();
        let mut p = project(2);
        settle(&mut ids, &mut p);
        let gone = p.channels.pop().unwrap().id.0;
        p.channels.push(crate::app::default_project().channels[0].clone());
        p.channels[1].id = Id::NONE;
        settle(&mut ids, &mut p);
        assert_ne!(p.channels[1].id.0, gone);
        forget(&mut ids);
        let mut q = project(1);
        settle(&mut ids, &mut q);
        assert!(q.channels[0].id.0 > gone, "ids after a new project are new too");
    }
}
