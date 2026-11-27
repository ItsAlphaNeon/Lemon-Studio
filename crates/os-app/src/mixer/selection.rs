// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The mixer on a multi-selection: the track menu's Track routing ▸ with Create submix... (spec:
//! ui/mixer-selection.md §1).

use super::*;

/// Undo step name of Create submix... and its names (spec: ui/mixer-selection.md §1.1, FL's "Mixer track(s) routing").
pub const SUBMIX: &str = "Mixer track(s) routing";

/// Create submix... waiting for its name box: the submix track, its sources and the undo step that routed them.
#[derive(Debug, Clone, PartialEq)]
pub struct Submix {
    pub track: u16,
    pub sources: Vec<u16>,
    pub step: Option<u64>,
}

/// The other selected tracks than `n` (never the master): what Track routing ▸ acts on.
fn others(s: &State, n: u16) -> Vec<u16> {
    let mut v: Vec<u16> = selected(s).into_iter().filter(|&t| t != n && t != 0).collect();
    v.sort_unstable();
    v
}

/// Track routing ▸ (spec: ui/mixer-selection.md §1): two columns, "Selected tracks to this track" and "This track to
/// selected tracks". Greyed unless track `n` is selected with at least one other track.
pub fn track_routing(s: &State, n: u16) -> PItem {
    let into = |only, side| Cmd::Mixer(MCmd::Route { dst: n, only, side });
    let from = |only, side| Cmd::Mixer(MCmd::RouteFrom { src: n, only, side });
    let mut items = vec![
        PItem::head("Selected tracks to this track"),
        PItem::new("Route selected to this track", into(false, false)),
        PItem::new("Route selected to this track only", into(true, false)),
        PItem::sep(),
        PItem::new("Sidechain selected to this track", into(false, true)),
        PItem::new("Sidechain selected to this track only", into(true, true)),
        PItem::sep(),
        PItem::new("Create submix...", Cmd::Mixer(MCmd::Submix(n))),
        PItem::col(),
        PItem::head("This track to selected tracks"),
    ];
    // The master has no sends of its own to give.
    let master = |it: PItem| if n == 0 { it.off() } else { it };
    items.extend([
        master(PItem::new("Route this track to selected", from(false, false))),
        master(PItem::new("Route this track to selected only", from(true, false))),
        PItem::sep(),
        master(PItem::new("Sidechain this track to selected", from(false, true))),
        master(PItem::new("Sidechain this track to selected only", from(true, true))),
    ]);
    let it = PItem::sub("Track routing", items);
    if is_selected(s, n) && !others(s, n).is_empty() { it } else { it.off() }
}

/// Route this track to selected (only) (spec: ui/mixer-selection.md §1): track `src` sends to each other selected
/// track, at full level or (`side`) as a sidechain at level 0; `only` first drops its other sends, the master's
/// included. One undo step.
pub fn route_from(s: &mut State, src: u16, only: bool, side: bool) -> Option<String> {
    if src == 0 {
        return None;
    }
    track_ids(s);
    let mut msg = None;
    let mut c = if only { unroute_all(s, src, None) } else { Vec::new() };
    for dst in others(s, src) {
        if makes_loop(&s.session, src, dst) {
            msg = Some(format!(
                "Can't route {} to {}: it would make a loop",
                name(&s.session, src),
                name(&s.session, dst)
            ));
            continue;
        }
        c.push(Command::SetSend {
            from: id_of(s, src)?,
            to: id_of(s, dst)?,
            level: Some(if side { 0.0 } else { 0.8 }),
        });
    }
    apply(s, ROUTING, c).or(msg)
}

/// Where Create submix... puts separators for `sources` (sorted) feeding `t` (spec: ui/mixer-selection.md §1.1): at
/// the first source and after the run when the sources are one contiguous run (the run takes `t` in when `t`
/// directly follows it); none otherwise.
pub fn submix_separators(sources: &[u16], t: u16) -> Vec<u16> {
    let (Some(&a), Some(&b)) = (sources.first(), sources.last()) else { return Vec::new() };
    if usize::from(b - a) + 1 != sources.len() {
        return Vec::new();
    }
    let end = if b + 1 == t { t } else { b };
    vec![a, end + 1]
}

/// Create submix... (spec: ui/mixer-selection.md §1.1): the other selected tracks are routed to track `t` only (their
/// master send dropped), grouped when they are a contiguous run, as one undo step; then the name box opens on `t`
/// (`name_submix`).
pub fn create_submix(s: &mut State, t: u16) -> Option<String> {
    let mut sources = others(s, t);
    if t == 0 || sources.is_empty() {
        return None;
    }
    track_ids(s);
    let mut msg = None;
    let mut c = Vec::new();
    sources.retain(|&src| {
        let ok = !makes_loop(&s.session, src, t);
        if !ok {
            msg =
                Some(format!("Can't route {} to {}: it would make a loop", name(&s.session, src), name(&s.session, t)));
        }
        ok
    });
    for &src in &sources {
        c.extend(unroute_all(s, src, None));
        c.push(Command::SetSend { from: id_of(s, src)?, to: id_of(s, t)?, level: Some(0.8) });
    }
    let last = inserts(&s.session);
    c.extend(
        submix_separators(&sources, t)
            .into_iter()
            .filter(|&n| n <= last)
            .filter_map(|n| set_track!(s, n, separator: true)),
    );
    if let Some(e) = apply(s, SUBMIX, c) {
        return Some(e);
    }
    s.mixer.submix = Some(Submix { track: t, sources, step: s.session.history.newest_id() });
    msg
}

/// Enter in Create submix...'s name box (spec: ui/mixer-selection.md §1.1): the sources are named `text`, the submix
/// track "`text` master", in the routing's undo step while it is the newest one. An empty box names nothing.
pub fn name_submix(s: &mut State, t: u16, text: &str) {
    let Some(sub) = s.mixer.submix.take().filter(|m| m.track == t) else { return };
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    track_ids(s);
    let mut c: Vec<Command> = sub.sources.iter().filter_map(|&n| set_track!(s, n, name: text.to_string())).collect();
    c.extend(set_track!(s, t, name: format!("{text} master")));
    // The routing's step is still the newest and done (not undone while the box was open).
    let h = &s.session.history;
    if sub.step.is_some() && h.newest_id() == sub.step && h.done() == h.steps.len() && h.newest_is_commands() {
        if let Ok((_, mut undo)) = crate::control_ui::run(s, c) {
            s.session.dirty = true;
            crate::history::extend_newest(&mut s.session, |old| {
                undo.append(old);
                *old = undo;
            });
        }
    } else if let Some(e) = apply(s, SUBMIX, c) {
        s.message = Some(e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn submix_separators_follow_the_measured_runs() {
        // spec: ui/mixer-selection.md §1.1, the table.
        assert_eq!(submix_separators(&[2, 3, 4], 6), vec![2, 5]);
        assert_eq!(submix_separators(&[13, 14], 15), vec![13, 16]);
        assert_eq!(submix_separators(&[9], 11), vec![9, 10]);
        assert_eq!(submix_separators(&[8, 10], 12), Vec::<u16>::new());
        assert_eq!(submix_separators(&[], 3), Vec::<u16>::new());
    }

    fn app() -> App {
        let mut app = App::new(None);
        app.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        app
    }

    /// Selects `tracks`, the last one current (the track the menu is opened on).
    fn select_all(a: &mut App, tracks: &[u16]) {
        select(&mut a.state, tracks[0], false);
        for &t in &tracks[1..] {
            select(&mut a.state, t, true);
        }
    }

    fn entry(a: &App, n: u16) -> PItem {
        track_items(&a.state, n).into_iter().find(|i| i.label == "Track routing").unwrap()
    }

    fn cmd(e: &PItem, label: &str) -> Cmd {
        e.children
            .iter()
            .find(|i| i.label == label && !i.head)
            .unwrap_or_else(|| panic!("no entry {label}"))
            .cmd
            .clone()
    }

    /// spec: ui/mixer-selection.md §1: greyed unless the track is selected with another; two columns with their headers.
    #[test]
    fn track_routing_menu_has_fl_columns() {
        let mut a = app();
        select(&mut a.state, 5, false);
        assert!(!entry(&a, 5).enabled, "only this track selected");
        select_all(&mut a, &[2, 3, 5]);
        let e = entry(&a, 5);
        assert!(e.enabled);
        assert!(!entry(&a, 9).enabled, "a track outside the selection");
        let labels: Vec<&str> = e.children.iter().filter(|i| !i.sep && !i.col).map(|i| i.label.as_ref()).collect();
        assert_eq!(
            labels,
            [
                "Selected tracks to this track",
                "Route selected to this track",
                "Route selected to this track only",
                "Sidechain selected to this track",
                "Sidechain selected to this track only",
                "Create submix...",
                "This track to selected tracks",
                "Route this track to selected",
                "Route this track to selected only",
                "Sidechain this track to selected",
                "Sidechain this track to selected only",
            ]
        );
        assert!(e.children.iter().filter(|i| i.head).count() == 2);
    }

    /// spec: ui/mixer-selection.md §1: painted, the submenu's second column ("This track to selected tracks") stands to
    /// the right of the first, its rows starting at the top again.
    #[test]
    fn track_routing_paints_two_columns() {
        use crate::app::Action;
        let mut a = app();
        a.state.raise(crate::app::Win::Mixer);
        select_all(&mut a, &[2, 3, 5]);
        let items = track_items(&a.state, 5);
        let at = items.iter().position(|i| i.label == "Track routing").unwrap();
        let e = items[at].clone();
        a.state.playlist.popup =
            Some(crate::playlist::Popup { x: 300.0, y: 100.0, items, sub: Some(at), scroll: [0; 2] });
        let Ok(mut c) = os_ui::Canvas::new(1600, 900, 1.0) else { return };
        a.invalidate();
        a.paint(&mut c, None);
        let row = |label: &str| {
            let i = e.children.iter().position(|i| i.label == label && !i.head).unwrap();
            a.hits
                .iter()
                .rev()
                .find(|h| h.action == Action::PlPopupSub(i))
                .map(|h| h.rect)
                .unwrap_or_else(|| panic!("{label} not drawn"))
        };
        let (first, second) = (row("Route selected to this track"), row("Route this track to selected"));
        assert!(second.x >= first.right() - 1.0, "a column of its own: {first:?} {second:?}");
        assert!((second.y - first.y).abs() < 1.0, "both columns start at the top: {first:?} {second:?}");
        assert!(row("Create submix...").y > first.y);
    }

    /// spec: ui/mixer-selection.md §1: the selected tracks into this one, and this one into the selected tracks; "only"
    /// drops the master send; a sidechain is level 0; one undo step each.
    #[test]
    fn track_routing_routes_both_ways() {
        let mut a = app();
        select_all(&mut a, &[2, 3, 5]);
        let e = entry(&a, 5);
        let steps = a.state.session.history.at();
        crate::playlist::run(&mut a, cmd(&e, "Route selected to this track"));
        assert_eq!(
            (send(&a.state.session, 2, 5), send(&a.state.session, 3, 5), send(&a.state.session, 2, 0)),
            (Some(0.8), Some(0.8), Some(0.8))
        );
        assert_eq!(a.state.session.history.at(), steps + 1);
        crate::playlist::run(&mut a, cmd(&e, "Sidechain selected to this track only"));
        assert_eq!(
            (send(&a.state.session, 2, 5), send(&a.state.session, 2, 0)),
            (Some(0.0), None),
            "sidechained only: the master route goes"
        );
        assert_eq!(send(&a.state.session, 5, 5), None, "this track does not route to itself");

        // Measured: Insert 1 with 5 and 7 selected, Route this track to selected only: 1 sends to 5 and 7, not the master.
        let mut a = app();
        select_all(&mut a, &[5, 7, 1]);
        let e = entry(&a, 1);
        crate::playlist::run(&mut a, cmd(&e, "Route this track to selected only"));
        let s = &a.state.session;
        assert_eq!((send(s, 1, 5), send(s, 1, 7), send(s, 1, 0)), (Some(0.8), Some(0.8), None));
        crate::playlist::run(&mut a, cmd(&e, "Sidechain this track to selected"));
        assert_eq!(send(&a.state.session, 1, 5), Some(0.0));
    }

    /// spec: ui/mixer-selection.md §1.1: the sources route to the submix only at once, a contiguous run is grouped, the
    /// name box names them "N" and the submix "N master", all in one undo step "Mixer track(s) routing".
    #[test]
    fn create_submix_routes_groups_and_names_in_one_step() {
        let mut a = app();
        select_all(&mut a, &[2, 3, 4, 6]);
        let steps = a.state.session.history.at();
        let c = cmd(&entry(&a, 6), "Create submix...");
        crate::playlist::run(&mut a, c);
        let s = &a.state.session;
        for t in [2, 3, 4] {
            assert_eq!((send(s, t, 6), send(s, t, 0)), (Some(0.8), None), "track {t} to the submix only");
        }
        assert_eq!(send(s, 6, 0), Some(0.8), "the submix keeps its master send");
        let seps: Vec<u16> = (1..=inserts(s)).filter(|&t| track(s, t).unwrap().separator).collect();
        assert_eq!(seps, vec![2, 5]);
        let e = a.state.playlist.edit.as_ref().expect("the name box");
        assert_eq!((e.target, e.text.as_str()), (EditTarget::MixerSubmix(6), ""));
        a.state.playlist.edit.as_mut().unwrap().text = "Drums".into();
        crate::playlist::commit_edit(&mut a.state);
        let s = &a.state.session;
        assert_eq!(
            (name(s, 2), name(s, 4), name(s, 6)),
            ("Drums".to_string(), "Drums".to_string(), "Drums master".to_string())
        );
        assert_eq!(a.state.session.history.at(), steps + 1, "one step");
        assert_eq!(a.state.session.history.newest_done(), Some(SUBMIX));

        crate::history_ui::undo(&mut a.state);
        let s = &a.state.session;
        assert_eq!(
            (send(s, 2, 0), send(s, 2, 6), name(s, 6)),
            (Some(0.8), None, "Bus 6".to_string()),
            "one undo takes all back"
        );
        assert!(!track(s, 2).unwrap().separator);
        crate::history_ui::redo(&mut a.state);
        let s = &a.state.session;
        assert_eq!(
            (send(s, 2, 6), name(s, 3), name(s, 6)),
            (Some(0.8), "Drums".to_string(), "Drums master".to_string()),
            "redo brings the names"
        );
    }

    /// spec: ui/mixer-selection.md §1.1: Esc (or an empty box) keeps the names; the routing stays; sources apart are
    /// not grouped.
    #[test]
    fn create_submix_without_a_name_keeps_names() {
        let mut a = app();
        select_all(&mut a, &[8, 10, 12]);
        let c = cmd(&entry(&a, 12), "Create submix...");
        crate::playlist::run(&mut a, c);
        crate::playlist::commit_edit(&mut a.state);
        let s = &a.state.session;
        assert_eq!(
            (send(s, 8, 12), send(s, 10, 12), name(s, 8), name(s, 12)),
            (Some(0.8), Some(0.8), "Bus 8".to_string(), "Bus 12".to_string())
        );
        assert!((1..=inserts(s)).all(|t| !track(s, t).unwrap().separator), "not contiguous: no group");
    }
}
