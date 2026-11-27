// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Chopping playlist clips: the clip menu's Chop into ▸ Bar / Beat / Beat Random, and Edit ▸ Slice and insert space.
//! Each is one undo step of the arrangement.
// spec: ui/playlist-chop.md

use crate::app::State;
use crate::playlist::{arr_edit, clips, split, Bars, ARRANGER_EDIT};

/// Where Chop cuts (§1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Bar,
    Beat,
    /// At every beat, then the beat-long pieces play in a random order (§1).
    BeatRandom,
}

/// The song's bar and beat lines strictly inside `a..b` (ticks), as the playlist's grid has them.
fn lines(s: &State, a: u32, b: u32, unit: Unit) -> Vec<u32> {
    let bars = Bars::new(&s.session);
    let ppq = crate::playlist::ppq(&s.session).max(1);
    let mut out = Vec::new();
    for (_, at, len) in bars.lines(0, b) {
        let step = if unit == Unit::Bar { len.max(1) } else { ppq };
        let mut t = at;
        while t < at + len {
            if t > a && t < b {
                out.push(t);
            }
            t += step;
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// A small deterministic random sequence (xorshift), so a test can follow it.
fn next(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}

/// The ticks Chop cuts clip `i` at (§1).
pub fn cuts(s: &State, i: usize, unit: Unit) -> Vec<u32> {
    let Some(c) = clips(&s.session).get(i) else { return Vec::new() };
    lines(s, c.position, c.end(), unit)
}

/// Chops clips `idx` (§1); Beat Random then shuffles the beat-long pieces' content with `seed`. Returns how many cuts
/// were made.
pub fn chop(s: &mut State, idx: &[usize], unit: Unit, seed: u64) -> usize {
    let mut seed = seed | 1;
    let plan: Vec<(usize, Vec<u32>)> = idx.iter().map(|&i| (i, cuts(s, i, unit))).collect();
    let ppq = crate::playlist::ppq(&s.session).max(1);
    arr_edit(s, ARRANGER_EDIT, |s| {
        let mut n = 0;
        let mut sel = s.playlist.sel.clone();
        for (i, ticks) in plan {
            let mut pieces = vec![i];
            // From the right: clip `i` stays the leftmost piece.
            for &t in ticks.iter().rev() {
                if let Some(j) = split(&mut s.session, i, t) {
                    n += 1;
                    pieces.push(j);
                    if sel.contains(&i) {
                        sel.push(j);
                    }
                }
            }
            if unit == Unit::BeatRandom {
                shuffle(s, &pieces, ppq, &mut seed);
            }
        }
        s.playlist.sel = sel;
        n
    })
}

/// Beat Random's reorder: the content (windows) of the beat-long pieces among `pieces` change places at random; a
/// shorter piece at either end keeps its own.
fn shuffle(s: &mut State, pieces: &[usize], beat: u32, seed: &mut u64) {
    let arr = crate::playlist::arr_mut(&mut s.session);
    let mut full: Vec<usize> = pieces.iter().copied().filter(|&j| arr.clips.get(j).is_some_and(|c| c.length == beat)).collect();
    full.sort_by_key(|&j| arr.clips[j].position);
    let mut windows: Vec<_> = full.iter().map(|&j| arr.clips[j].window).collect();
    for k in (1..windows.len()).rev() {
        let r = (next(seed) % (k as u64 + 1)) as usize;
        windows.swap(k, r);
    }
    for (&j, w) in full.iter().zip(windows) {
        arr.clips[j].window = w;
    }
}

/// The audio clip menu's Chop into ▸ for clip `i` (§1): Bar, Beat, Beat Random; Regions is off (no wave regions are
/// read).
pub fn menu_items(i: usize) -> Vec<crate::playlist::PItem> {
    use crate::playlist::{Cmd, PItem};
    vec![PItem::sub(
        "Chop into",
        vec![
            PItem::new("Bar", Cmd::Chop(i, Unit::Bar)),
            PItem::new("Beat", Cmd::Chop(i, Unit::Beat)),
            PItem::new("Beat Random", Cmd::Chop(i, Unit::BeatRandom)),
            PItem::new("Regions", Cmd::Nothing).off(),
        ],
    )]
}

/// The clips a Chop into entry on clip `i` acts on: the selection when `i` is in it, else `i` alone.
pub fn targets(s: &State, i: usize) -> Vec<usize> {
    if s.playlist.sel.contains(&i) { s.playlist.sel.clone() } else { vec![i] }
}

/// Edit ▸ Slice and insert space (Ctrl+Alt+Ins, §2): the clips crossing the time selection's start are sliced there,
/// and everything from the start on moves right by the selection's length. One undo step.
pub fn slice_and_insert_space(s: &mut State) -> (u32, u32) {
    arr_edit(s, ARRANGER_EDIT, crate::playlist::insert_space)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::playlist::arr_mut;
    use os_model::{ChannelKind, Clip, ClipSource};

    fn app() -> App {
        let mut app = App::new(None);
        app.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        app.state.session.project.get_mut().channels[0].kind = ChannelKind::AudioClip;
        let ch = app.state.session.project.channels[0].index;
        // 96 PPQ, 4/4: a clip from beat 1 of bar 1 to the middle of bar 3.
        arr_mut(&mut app.state.session).clips = vec![Clip::new(96, 384 * 2 + 96, 0, ClipSource::Channel(ch))];
        app.state.playlist.sel = vec![0];
        app
    }

    fn spans(app: &App) -> Vec<(u32, u32)> {
        let mut v: Vec<(u32, u32)> = clips(&app.state.session).iter().map(|c| (c.position, c.end())).collect();
        v.sort_unstable();
        v
    }

    /// By bar: cuts at the song's bar lines inside the clip; the pieces stay selected; one undo step.
    #[test]
    fn chop_by_bar() {
        let mut app = app();
        assert_eq!(chop(&mut app.state, &[0], Unit::Bar, 1), 2);
        assert_eq!(spans(&app), [(96, 384), (384, 768), (768, 960)]);
        assert_eq!(app.state.playlist.sel.len(), 3);
        crate::history_ui::undo(&mut app.state);
        assert_eq!(spans(&app), [(96, 960)]);
    }

    /// By beat: every beat line. Beat Random: the same cuts, then the pieces' content in another order: every
    /// window is still there once, and the same seed gives the same order.
    #[test]
    fn chop_by_beat_and_random() {
        let mut app = app();
        assert_eq!(chop(&mut app.state, &[0], Unit::Beat, 1), 8);
        assert!(spans(&app).iter().all(|(a, b)| b - a == 96));
        let in_order = windows(&app);
        let random = |seed| {
            let mut app = self::app();
            assert_eq!(chop(&mut app.state, &[0], Unit::BeatRandom, seed), 8);
            assert_eq!(spans(&app).len(), 9);
            windows(&app)
        };
        let (a, b) = (random(7), random(7));
        assert_eq!(a, b, "same seed, same order");
        assert_ne!(a, in_order, "reordered");
        let (mut x, mut y) = (a.clone(), in_order.clone());
        x.sort_by(|p, q| p.partial_cmp(q).unwrap());
        y.sort_by(|p, q| p.partial_cmp(q).unwrap());
        assert_eq!(x, y, "the same pieces");
    }

    /// Each clip's window by position.
    fn windows(app: &App) -> Vec<(f64, f64)> {
        let mut v: Vec<(u32, (f64, f64))> = clips(&app.state.session).iter().map(|c| (c.position, c.window.unwrap())).collect();
        v.sort_by_key(|p| p.0);
        v.into_iter().map(|p| p.1).collect()
    }

    /// The audio clip menu has Chop into ▸ with Bar, Beat, Beat Random and (off) Regions; Bar chops the clip.
    #[test]
    fn chop_into_menu() {
        let mut app = app();
        let m = crate::playlist::clip_menu(&app.state.session, 0, 0.0, 0.0);
        let sub = &m.items.iter().find(|i| i.label == "Chop into").expect("Chop into").children;
        assert_eq!(sub.iter().map(|i| i.label.as_ref()).collect::<Vec<_>>(), ["Bar", "Beat", "Beat Random", "Regions"]);
        assert!(!sub[3].enabled && sub[0].enabled);
        crate::playlist::run(&mut app, sub[0].cmd.clone());
        assert_eq!(spans(&app).len(), 3);
    }

    /// Ctrl+Alt+Ins: the clip crossing the selection's start is sliced there and its right part moves right by the
    /// selection's length; the Edit menu lists it.
    #[test]
    fn slice_and_insert_space_key_and_menu() {
        let mut app = app();
        app.state.playlist.range = Some((384, 576));
        let key = crate::keys::Key::Named(crate::keys::NamedKey::Insert);
        assert!(crate::playlist::key(&mut app, &key, true, true));
        assert_eq!(spans(&app), [(96, 384), (576, 1152)]);
        crate::history_ui::undo(&mut app.state);
        assert_eq!(spans(&app), [(96, 960)]);
        let pop = crate::playlist::options_menu(&app.state, 0.0, 0.0);
        let edit = &pop.items.iter().find(|i| i.label == "Edit").unwrap().children;
        assert!(edit.iter().any(|i| i.label == "Slice and insert space" && i.key == "Ctrl+Alt+Ins"));
    }
}
