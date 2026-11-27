// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Merging pattern clips and working on whole playlist tracks: Edit ▸ Merge pattern clips (Ctrl+G) and Merge similar
//! pattern clips (Shift+Ctrl+G), the track menu's Merge pattern clips and Clone..., Tools ▸ Clone selected playlist
//! track(s) (Shift+Ctrl+B), and the ▸ menu's Collapse / Expand all grouped tracks. Each is one undo step.
// spec: ui/playlist-merge-tracks.md

use os_model::{Clip, ClipSource};

use crate::app::State;
use crate::control::command::{ClipIn, Command, NoteIn};
use crate::playlist::{clips, track_info, ARRANGER_EDIT};

/// What a merge works out before it applies: the clips it replaces, each group's start, end and track.
struct Plan {
    /// The merged clips' indexes, by group.
    groups: Vec<Vec<usize>>,
    /// The new pattern's notes (from the first group; every group plays the same).
    notes: Vec<NoteIn>,
    /// The clips' span and track, by group.
    spans: Vec<(u32, u32, u16)>,
}

/// The pattern clips of `idx`, sorted by position then track.
fn pattern_clips(s: &State, idx: &[usize]) -> Vec<usize> {
    let all = clips(&s.session);
    let mut v: Vec<usize> = idx
        .iter()
        .copied()
        .filter(|&i| all.get(i).is_some_and(|c| matches!(c.source, ClipSource::Pattern(_))))
        .collect();
    v.sort_by_key(|&i| (all[i].position, all[i].track, i));
    v.dedup();
    v
}

/// The window a pattern clip plays of its pattern, in pattern ticks.
fn window(c: &Clip) -> (f64, f64) {
    c.window.filter(|w| w.0 >= 0.0).unwrap_or((0.0, f64::from(c.length)))
}

/// The notes clips `group` play, as one pattern starting at the first clip's position (§1): each clip's notes inside
/// its window, cut at the window's end.
fn merged_notes(s: &State, group: &[usize]) -> Vec<NoteIn> {
    let p = &s.session.project;
    let all = clips(&s.session);
    let p0 = group.iter().map(|&i| all[i].position).min().unwrap_or(0);
    let mut out = Vec::new();
    for &i in group {
        let c = &all[i];
        let ClipSource::Pattern(n) = c.source else { continue };
        let Some(pat) = p.pattern(n) else { continue };
        let (w0, _) = window(c);
        let (from, to) = (w0.max(0.0) as u32, w0.max(0.0) as u32 + c.length);
        for note in pat.notes.iter().filter(|x| x.position >= from && x.position < to) {
            let Some(ch) = p.channels.iter().find(|x| x.index == note.channel) else { continue };
            let mut n = NoteIn::of(note, ch.id.0);
            n.id = None;
            n.position = c.position - p0 + note.position - from;
            if n.length > 0 {
                n.length = n.length.min(to - note.position);
            }
            out.push(n);
        }
    }
    out.sort_by_key(|n| (n.position, n.channel, n.key));
    out
}

/// How clip `c` sits in its group: pattern, position and track from the group's first clip, length and window.
fn shape(c: &Clip, first: &Clip) -> (ClipSource, i64, i32, u32, Option<(f64, f64)>) {
    (
        c.source,
        i64::from(c.position) - i64::from(first.position),
        i32::from(c.track) - i32::from(first.track),
        c.length,
        c.window,
    )
}

/// The groups elsewhere in the playlist laid out as `group` is (the same patterns, lengths and windows at the same
/// distances in time and tracks), none sharing a clip with another (§2).
fn similar(s: &State, group: &[usize]) -> Vec<Vec<usize>> {
    let all = clips(&s.session);
    let first = all[group[0]];
    let want: Vec<_> = group.iter().map(|&i| shape(&all[i], &first)).collect();
    let mut used: Vec<usize> = group.to_vec();
    let mut found = Vec::new();
    let mut anchors: Vec<usize> =
        (0..all.len()).filter(|&j| !used.contains(&j) && shape(&all[j], &all[j]) == want[0]).collect();
    anchors.sort_by_key(|&j| (all[j].position, all[j].track));
    for a in anchors {
        if used.contains(&a) {
            continue;
        }
        let anchor = all[a];
        let mut g = vec![a];
        for w in &want[1..] {
            let hit = (0..all.len()).find(|&j| !used.contains(&j) && !g.contains(&j) && shape(&all[j], &anchor) == *w);
            match hit {
                Some(j) => g.push(j),
                None => break,
            }
        }
        if g.len() == want.len() {
            used.extend(&g);
            found.push(g);
        }
    }
    found
}

fn plan(s: &State, group: Vec<usize>, everywhere: bool) -> Option<Plan> {
    if group.len() < 2 {
        return None;
    }
    let all = clips(&s.session);
    let notes = merged_notes(s, &group);
    let mut groups = vec![group];
    if everywhere {
        groups.extend(similar(s, &groups[0]));
    }
    let spans = groups
        .iter()
        .map(|g| {
            let a = g.iter().map(|&i| all[i].position).min().unwrap_or(0);
            let b = g.iter().map(|&i| all[i].end()).max().unwrap_or(a);
            // The track of the earliest clip (the topmost of those starting first).
            (a, b, all[g[0]].track)
        })
        .collect();
    Some(Plan { groups, notes, spans })
}

/// Applies a merge: a new pattern numbered after the last holds the notes, and each group's clips give way to one
/// clip of it. One undo step. The new pattern's number, or what went wrong.
fn apply(s: &mut State, plan: Plan) -> Result<u16, String> {
    let p = &s.session.project;
    let top = p
        .patterns
        .iter()
        .map(|x| x.number)
        .chain(
            clips(&s.session).iter().filter_map(|c| if let ClipSource::Pattern(n) = c.source { Some(n) } else { None }),
        )
        .max()
        .unwrap_or(0);
    let number = top + 1;
    crate::playlist::clip_ids(s);
    let gone: Vec<usize> = plan.groups.iter().flatten().copied().collect();
    let Plan { notes, spans, .. } = plan;
    let then = move |s: &State, r: &[serde_json::Value]| -> Vec<Command> {
        let Some(id) = r.first().and_then(|v| v["pattern"].as_u64()) else { return Vec::new() };
        let p = &s.session.project;
        let mut list: Vec<ClipIn> = clips(&s.session)
            .iter()
            .enumerate()
            .filter(|(i, _)| !gone.contains(i))
            .map(|(_, c)| ClipIn::of(c, p))
            .collect();
        for (a, b, track) in spans {
            let mut c = ClipIn::of(&Clip::new(a, (b - a).max(1), track, ClipSource::Pattern(number)), p);
            c.id = None;
            list.push(c);
        }
        vec![Command::AddNotes { pattern: id as u32, notes }, Command::SetClips { clips: list }]
    };
    let first = vec![Command::AddPattern { name: None, color: None, number: Some(number) }];
    crate::control_ui::command_then(s, ARRANGER_EDIT, first, then)?;
    s.playlist.sel.clear();
    Ok(number)
}

/// Edit ▸ Merge pattern clips (Ctrl+G, §1): the selected pattern clips become one clip of a new pattern that plays
/// what they played. With `everywhere` (Merge similar pattern clips, Shift+Ctrl+G, §2) every other group laid out
/// the same way becomes a clip of that pattern too. The message to show.
pub fn merge_selected(s: &mut State, everywhere: bool) -> Option<String> {
    let group = pattern_clips(s, &s.playlist.sel.clone());
    let Some(plan) = plan(s, group, everywhere) else {
        return Some("Merge pattern clips: select two or more pattern clips".into());
    };
    let n = plan.groups.len();
    let number = match apply(s, plan) {
        Ok(n) => n,
        Err(e) => return Some(e),
    };
    Some(if n > 1 {
        format!("Merged {n} places into pattern {number}")
    } else {
        format!("Merged into pattern {number}")
    })
}

/// The track menu's Merge pattern clips (§1): the pattern clips on track `t` (0-based).
pub fn merge_track(s: &mut State, t: u16) -> Option<String> {
    let on: Vec<usize> = clips(&s.session).iter().enumerate().filter(|(_, c)| c.track == t).map(|(i, _)| i).collect();
    let group = pattern_clips(s, &on);
    let plan = plan(s, group, false)?;
    Some(match apply(s, plan) {
        Ok(number) => format!("Merged into pattern {number}"),
        Err(e) => e,
    })
}

/// Clone... (track menu) and Tools ▸ Clone selected playlist track(s) (Shift+Ctrl+B, §3): each track of `tracks`
/// (0-based) gets a copy right below it, with its name, colour and clips (the clips play the same sources). One undo
/// step.
pub fn clone_tracks(s: &mut State, tracks: &[u16]) -> Option<String> {
    let mut ts: Vec<u16> = tracks.iter().copied().filter(|&t| t + 1 < crate::playlist::TRACKS).collect();
    ts.sort_unstable();
    ts.dedup();
    if ts.is_empty() {
        return Some("Clone tracks: select a clip on each track to clone".into());
    }
    crate::playlist::clip_ids(s);
    // The tracks below each clone move down by the clones above them.
    let mut first = Vec::new();
    let mut moved = Vec::new();
    for (k, &t) in ts.iter().enumerate() {
        let at = t + k as u16 + 1;
        first.push(Command::InsertPlaylistTrack { at: at + 1 });
        let mut set = crate::playlist::track_cmd(at);
        if let (Command::SetPlaylistTrack { name, color, .. }, Some(info)) = (&mut set, track_info(&s.session, t)) {
            *name = info.name.clone();
            *color = info.color.and_then(|c| crate::control::command::color_text(Some(c)));
        }
        first.push(set);
        moved.push((t + k as u16, at));
    }
    let n = ts.len();
    let then = move |s: &State, _: &[serde_json::Value]| -> Vec<Command> {
        let p = &s.session.project;
        let all = clips(&s.session);
        let mut list: Vec<ClipIn> = all.iter().map(|c| ClipIn::of(c, p)).collect();
        for &(from, to) in &moved {
            for c in all.iter().filter(|c| c.track == from) {
                let mut copy = ClipIn::of(c, p);
                (copy.id, copy.track) = (None, to + 1);
                list.push(copy);
            }
        }
        vec![Command::SetClips { clips: list }]
    };
    if let Err(e) = crate::control_ui::command_then(s, ARRANGER_EDIT, first, then) {
        return Some(e);
    }
    Some(if n == 1 { "Cloned 1 track".into() } else { format!("Cloned {n} tracks") })
}

/// The tracks Shift+Ctrl+B clones: those of the selected clips.
pub fn selected_tracks(s: &State) -> Vec<u16> {
    let all = clips(&s.session);
    s.playlist.sel.iter().filter_map(|&i| all.get(i)).map(|c| c.track).collect()
}

/// The ▸ menu's Collapse all grouped tracks / Expand all grouped tracks (§4): every group's collapsed state. One undo
/// step when one changes.
pub fn collapse_all(s: &mut State, collapse: bool) -> bool {
    let leads: Vec<u16> = (0..crate::playlist::TRACKS)
        .filter(|&t| {
            crate::playlist::is_group_track(&s.session, t)
                && track_info(&s.session, t).is_some_and(|x| x.collapsed != collapse)
        })
        .collect();
    if leads.is_empty() {
        return false;
    }
    crate::playlist::snapshot(&mut s.session);
    let arr = crate::playlist::arr_mut(&mut s.session);
    for t in leads {
        arr.tracks[usize::from(t)].collapsed = collapse;
    }
    s.session.dirty = true;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::playlist::{arr_mut, options_menu, run, track_menu};
    use os_model::{Note, Pattern};

    /// Patterns 1 (a note at 0 and 96) and 2 (a note at 48), clips: 1 at 0 and 2 at 384 on track 0, the same pair
    /// again at 1536 on track 3, and a lone pattern 1 clip at 3072.
    fn app() -> App {
        let mut app = App::new(None);
        let mut p = crate::projects::test_project();
        let ch = p.channels[0].index;
        p.patterns = vec![
            Pattern { number: 1, notes: vec![Note::step(ch, 0), Note::step(ch, 96)], ..Pattern::default() },
            Pattern { number: 2, notes: vec![Note::step(ch, 48)], ..Pattern::default() },
        ];
        app.state.session.project = crate::locked::Locked::new(p);
        app.state.session.control.settle(app.state.session.project.get_mut());
        arr_mut(&mut app.state.session).clips = vec![
            Clip::new(0, 384, 0, ClipSource::Pattern(1)),
            Clip::new(384, 384, 0, ClipSource::Pattern(2)),
            Clip::new(1536, 384, 3, ClipSource::Pattern(1)),
            Clip::new(1920, 384, 3, ClipSource::Pattern(2)),
            Clip::new(3072, 384, 5, ClipSource::Pattern(1)),
        ];
        app.state.playlist.sel = vec![0, 1];
        app
    }

    fn layout(app: &App) -> Vec<(u32, u32, u16, ClipSource)> {
        let mut v: Vec<_> =
            clips(&app.state.session).iter().map(|c| (c.position, c.length, c.track, c.source)).collect();
        v.sort_by_key(|x| (x.0, x.2));
        v
    }

    /// Ctrl+G: the two selected clips become one clip of new pattern 3, holding both patterns' notes where they
    /// played; one undo step.
    #[test]
    fn merge_pattern_clips() {
        let mut app = app();
        let key = crate::keys::Key::Character("g".into());
        assert!(crate::playlist::key(&mut app, &key, true, false));
        let l = layout(&app);
        assert_eq!(app.state.message, Some("Merged into pattern 3".into()));
        assert!(l.contains(&(0, 768, 0, ClipSource::Pattern(3))), "{l:?}");
        assert_eq!(l.len(), 4);
        let pat = app.state.session.project.pattern(3).expect("pattern 3");
        assert_eq!(pat.notes.iter().map(|n| n.position).collect::<Vec<_>>(), [0, 96, 384 + 48]);
        crate::history_ui::undo(&mut app.state);
        assert_eq!(layout(&app).len(), 5);
        assert!(app.state.session.project.pattern(3).is_none());
    }

    /// Shift+Ctrl+G: the same pair laid out alike on another track becomes a clip of the merged pattern too; the lone
    /// clip stays.
    #[test]
    fn merge_similar_pattern_clips() {
        let mut app = app();
        app.shift = true;
        assert!(crate::playlist::key(&mut app, &crate::keys::Key::Character("G".into()), true, false));
        let l = layout(&app);
        assert_eq!(
            l,
            [
                (0, 768, 0, ClipSource::Pattern(3)),
                (1536, 768, 3, ClipSource::Pattern(3)),
                (3072, 384, 5, ClipSource::Pattern(1))
            ]
        );
    }

    /// The Edit menu lists both merges with FL's keys; the track menu merges the track's pattern clips.
    #[test]
    fn menus() {
        let mut app = app();
        let pop = options_menu(&app.state, 0.0, 0.0);
        let edit = &pop.items.iter().find(|i| i.label == "Edit").unwrap().children;
        let keys: Vec<(&str, &str)> =
            edit.iter().filter(|i| i.label.starts_with("Merge")).map(|i| (i.label.as_ref(), i.key)).collect();
        assert_eq!(
            keys,
            [
                ("Merge pattern clips", "Ctrl+G"),
                ("Merge similar pattern clips", "Shift+Ctrl+G"),
                ("Merge automation clips", "Ctrl+Alt+G")
            ]
        );
        let m = track_menu(&app.state, 3, 0.0, 0.0);
        let merge = m.items.iter().find(|i| i.label == "Merge pattern clips").expect("track entry").cmd.clone();
        run(&mut app, merge);
        assert!(layout(&app).contains(&(1536, 768, 3, ClipSource::Pattern(3))));
    }

    /// Shift+Ctrl+B clones the selected clips' track right below it, with its name and clips; the tracks below move
    /// down. One undo step. The track menu has Clone....
    #[test]
    fn clone_track() {
        let mut app = app();
        arr_mut(&mut app.state.session).tracks[0].name = Some("Drums".into());
        app.state.playlist.sel = vec![0];
        app.shift = true;
        assert!(crate::playlist::key(&mut app, &crate::keys::Key::Character("B".into()), true, false));
        let l = layout(&app);
        assert!(
            l.contains(&(0, 384, 1, ClipSource::Pattern(1))) && l.contains(&(384, 384, 1, ClipSource::Pattern(2))),
            "{l:?}"
        );
        assert!(l.contains(&(1536, 384, 4, ClipSource::Pattern(1))), "track 3 moved down");
        assert_eq!(track_info(&app.state.session, 1).and_then(|t| t.name.clone()).as_deref(), Some("Drums"));
        crate::history_ui::undo(&mut app.state);
        assert_eq!(layout(&app).len(), 5);
        assert!(track_menu(&app.state, 0, 0.0, 0.0).items.iter().any(|i| i.label == "Clone..."));
    }

    /// The ▸ menu's Collapse all / Expand all grouped tracks set every group; undo puts them back.
    #[test]
    fn collapse_and_expand_all() {
        let mut app = app();
        {
            let arr = arr_mut(&mut app.state.session);
            arr.tracks[1].grouped = true;
            arr.tracks[4].grouped = true;
        }
        let pop = options_menu(&app.state, 0.0, 0.0);
        let item = |l: &str| pop.items.iter().find(|i| i.label == l).expect(l).cmd.clone();
        run(&mut app, item("Collapse all grouped tracks"));
        assert!(
            track_info(&app.state.session, 0).unwrap().collapsed
                && track_info(&app.state.session, 3).unwrap().collapsed
        );
        run(&mut app, item("Expand all grouped tracks"));
        assert!(!track_info(&app.state.session, 0).unwrap().collapsed);
        crate::history_ui::undo(&mut app.state);
        assert!(track_info(&app.state.session, 3).unwrap().collapsed);
    }
}
