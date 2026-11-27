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

/// The other selected tracks than `n`: what Track routing ▸ acts on. A selected master is a destination (the spec
/// leaves it out as a source only).
fn others(s: &State, n: u16) -> Vec<u16> {
    let mut v: Vec<u16> = selected(s).into_iter().filter(|&t| t != n).collect();
    v.sort_unstable();
    v
}

/// The other selected tracks than `n` that can feed it (the master never does).
fn sources(s: &State, n: u16) -> Vec<u16> {
    others(s, n).into_iter().filter(|&t| t != 0).collect()
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
        // The master can't be a submix, and a submix needs a track to feed it.
        if n == 0 || sources(s, n).is_empty() {
            PItem::new("Create submix...", Cmd::Mixer(MCmd::Submix(n))).off()
        } else {
            PItem::new("Create submix...", Cmd::Mixer(MCmd::Submix(n)))
        },
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
/// included. One undo step. When every destination would make a loop nothing changes, its sends included.
pub fn route_from(s: &mut State, src: u16, only: bool, side: bool) -> Option<String> {
    if src == 0 {
        return None;
    }
    track_ids(s);
    let mut msg = None;
    let mut dests = others(s, src);
    dests.retain(|&dst| {
        let ok = !makes_loop(&s.session, src, dst);
        if !ok {
            msg = Some(format!(
                "Can't route {} to {}: it would make a loop",
                name(&s.session, src),
                name(&s.session, dst)
            ));
        }
        ok
    });
    if dests.is_empty() {
        return msg;
    }
    let mut c = if only { unroute_all(s, src, None) } else { Vec::new() };
    for dst in dests {
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
    s.mixer.submix = None;
    let mut sources = sources(s, t);
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
    // No source routed: no step, and no name box.
    if sources.is_empty() {
        return msg;
    }
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
    let before = s.session.history.newest_id();
    if let Some(e) = apply(s, SUBMIX, c) {
        return Some(e);
    }
    // The routing's own step, none when it changed nothing (the names then make one of their own).
    let step = s.session.history.newest_id().filter(|&id| Some(id) != before);
    s.mixer.submix = Some(Submix { track: t, sources, step });
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

/// Left click on track `n`'s mute / solo LED with these modifiers (spec: ui/mixer-selection.md §4, §4.1): Ctrl solos
/// (again: all on), Alt alt-solos, Shift toggles the mute lock; a plain click mutes or turns on the track, or every
/// selected track when `n` is one of several selected (the selection stays). Ctrl+Shift adds the track to the
/// selection as on a strip (ui/mixer.md §2) and toggles it. No undo step, as the plain click.
pub fn led_click(s: &mut State, n: u16, ctrl: bool, shift: bool, alt: bool) -> Option<String> {
    let group = in_group(s, n);
    if !group {
        select(s, n, ctrl && shift);
    }
    track_ids(s);
    let m = track(&s.session, n)?;
    let (on, locked) = (!m.enabled, m.mute_lock);
    if alt {
        solo_with(s, n, true);
    } else if ctrl && !shift {
        solo_with(s, n, false);
    } else if shift && !ctrl {
        apply_quietly(s, vec![set_track!(s, n, mute_lock: !locked)?]);
    } else {
        let tracks = if group { selected(s) } else { vec![n] };
        let c = tracks.into_iter().filter_map(|t| set_track!(s, t, enabled: on)).collect();
        apply_quietly(s, c);
    }
    changed(&mut s.session);
    None
}

/// Right click on track `n`'s LED (spec: ui/mixer.md §3, ui/mixer-selection.md §4.1): solo, or back to all on; on one of
/// several selected tracks, the selected tracks are soloed together, and a second right click leaves them soloed.
pub fn led_right(s: &mut State, n: u16) {
    if in_group(s, n) {
        let tracks = selected(s);
        solo_set(s, &tracks, false, false);
    } else {
        solo(s, n);
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

    fn on(a: &App, t: u16) -> bool {
        track(&a.state.session, t).unwrap().enabled
    }

    fn led(a: &mut App, n: u16, ctrl: bool, shift: bool, alt: bool) {
        (a.ctrl, a.shift, a.alt) = (ctrl, shift, alt);
        hit(a, MHit::Led(n));
        (a.ctrl, a.shift, a.alt) = (false, false, false);
    }

    /// spec: ui/mixer-selection.md §4: Ctrl+click solos (again: all on), Alt+click keeps the tracks routed in, Shift+click
    /// locks: a locked track keeps its state through solo and unsolo, a plain click still toggles it.
    #[test]
    fn led_modifier_clicks_and_the_mute_lock() {
        let mut a = app();
        led(&mut a, 4, true, false, false);
        assert!(on(&a, 4) && !on(&a, 3) && !on(&a, 5), "Ctrl+click solos");
        led(&mut a, 4, true, false, false);
        assert!((1..=8).all(|t| on(&a, t)), "again: all on");

        // Alt: 2 and 3 route into 6 only; alt solo of 6 keeps them.
        select_all(&mut a, &[2, 3, 6]);
        let c = cmd(&entry(&a, 6), "Route selected to this track only");
        crate::playlist::run(&mut a, c);
        led(&mut a, 6, false, false, true);
        assert!(on(&a, 2) && on(&a, 3) && on(&a, 6) && !on(&a, 4), "alt solo");
        led(&mut a, 6, false, false, true);
        assert!((1..=8).all(|t| on(&a, t)));

        // Shift: the lock, saved in the track; locked on stays on under a solo, locked muted stays muted after it.
        led(&mut a, 7, false, true, false);
        assert!(track(&a.state.session, 7).unwrap().mute_lock && on(&a, 7));
        led(&mut a, 8, false, false, false);
        led(&mut a, 8, false, true, false);
        assert!(track(&a.state.session, 8).unwrap().mute_lock && !on(&a, 8));
        right(&mut a, MHit::Led(1));
        assert!(on(&a, 1) && !on(&a, 2) && on(&a, 7) && !on(&a, 8), "7 stays on, 8 stays muted");
        right(&mut a, MHit::Led(1));
        assert!(on(&a, 2) && on(&a, 7) && !on(&a, 8), "unsolo: 8 still muted");
        led(&mut a, 8, false, false, false);
        assert!(on(&a, 8), "a plain click still toggles a locked track");
        led(&mut a, 8, false, true, false);
        assert!(!track(&a.state.session, 8).unwrap().mute_lock, "Shift+click again unlocks");
    }

    /// spec: ui/mixer-selection.md §4.1: on one of several selected tracks, a click mutes them all and keeps the
    /// selection; a right click solos them together and a second one leaves them soloed.
    #[test]
    fn led_clicks_on_a_selection() {
        let mut a = app();
        select_all(&mut a, &[13, 14, 15]);
        led(&mut a, 14, false, false, false);
        assert!(!on(&a, 13) && !on(&a, 14) && !on(&a, 15) && on(&a, 12));
        assert_eq!(selected(&a.state).len(), 3, "the selection stays");
        led(&mut a, 13, false, false, false);
        assert!(on(&a, 13) && on(&a, 14) && on(&a, 15));
        right(&mut a, MHit::Led(14));
        assert!(on(&a, 13) && on(&a, 14) && on(&a, 15) && !on(&a, 12) && !on(&a, 1));
        right(&mut a, MHit::Led(14));
        assert!(on(&a, 13) && !on(&a, 12), "no toggle back");
        right(&mut a, MHit::Led(2));
        right(&mut a, MHit::Led(2));
        assert!((1..=16).all(|t| on(&a, t)), "a solo and unsolo elsewhere brings all back");
    }

    /// spec: ui/mixer.md §4: on selected faders, Ctrl drags at a quarter of the rate and Shift gives them the pressed
    /// one's value; after a Shift+drag a plain drag moves from the levels kept before it.
    #[test]
    fn fader_drags_on_a_selection() {
        let Ok(mut c) = os_ui::Canvas::new(1920, 1080, 1.0) else { return };
        let mut a = app();
        a.state.z = vec![crate::app::Win::Mixer];
        let set = |a: &mut App, t: u16, v: f64| crate::edit::mixer::track_mut(&mut a.state.session, t).unwrap().volume = Some(v);
        let vol = |a: &App, t: u16| volume(&a.state.session, t);
        for (t, v) in [(3, 0.5), (4, 0.7), (5, 0.6)] {
            set(&mut a, t, v);
        }
        select_all(&mut a, &[3, 5, 4]);
        a.invalidate();
        a.paint(&mut c, None);
        let r = a.hits.iter().rev().find(|h| h.action == crate::app::Action::Fader(4)).map(|h| h.rect).unwrap();
        let drag = |a: &mut App, dy: f32, ctrl: bool, shift: bool| {
            (a.ctrl, a.shift) = (ctrl, shift);
            a.mouse_move(r.cx(), r.y + 2.0);
            a.mouse_down();
            a.mouse_move(r.cx(), r.y + 2.0 + dy);
            a.mouse_up();
            (a.ctrl, a.shift) = (false, false);
        };
        drag(&mut a, 40.0, false, false);
        let plain = 0.7 - vol(&a, 4);
        crate::history_ui::undo(&mut a.state);
        assert_eq!(vol(&a, 4), 0.7);
        drag(&mut a, 40.0, true, false);
        assert!(((0.7 - vol(&a, 4)) - plain / 4.0).abs() < 1e-9, "a quarter: {} against {plain}", 0.7 - vol(&a, 4));
        assert!(vol(&a, 3) < 0.5 && vol(&a, 5) < 0.6, "the others follow");
        assert_eq!(selected(&a.state).len(), 3);
        crate::history_ui::undo(&mut a.state);
        let kept = (vol(&a, 3), vol(&a, 5));
        drag(&mut a, -40.0, false, true);
        let v = vol(&a, 4);
        assert!(v > 0.7 && vol(&a, 3) == v && vol(&a, 5) == v, "Shift: all at the pressed one's value");
        drag(&mut a, 10.0, false, false);
        assert!(vol(&a, 3) < vol(&a, 5) && vol(&a, 5) < vol(&a, 4), "the kept differences come back: {kept:?}");

        // A single fader outside the selection: Ctrl drags at the plain rate (the quarter is measured on a selection).
        select(&mut a.state, 9, false);
        set(&mut a, 4, 0.7);
        a.invalidate();
        a.paint(&mut c, None);
        drag(&mut a, 40.0, true, false);
        assert!(((0.7 - vol(&a, 4)) - plain).abs() < 1e-9, "the plain rate: {} against {plain}", 0.7 - vol(&a, 4));
    }

    /// Routes `src` to `dst` in a step of its own.
    fn route_one(a: &mut App, src: u16, dst: u16) {
        track_ids(&mut a.state);
        let c = Command::SetSend { from: id_of(&a.state, src).unwrap(), to: id_of(&a.state, dst).unwrap(), level: Some(0.8) };
        assert_eq!(apply(&mut a.state, ROUTING, vec![c]), None);
    }

    /// spec: ui/mixer-selection.md §4: locking is no undo step, so an undo of another step leaves the lock as it is now.
    #[test]
    fn undo_leaves_the_mute_lock_alone() {
        let lock = |a: &App, t: u16| track(&a.state.session, t).unwrap().mute_lock;
        let mut a = app();
        track_ids(&mut a.state);
        let c = set_track!(&a.state, 3, volume: 0.3).unwrap();
        assert_eq!(apply(&mut a.state, "Mixer track volume", vec![c]), None);
        led(&mut a, 3, false, true, false);
        assert!(lock(&a, 3));
        crate::history_ui::undo(&mut a.state);
        assert_eq!(volume(&a.state.session, 3), 0.8, "the fader move is undone");
        assert!(lock(&a, 3), "the lock stays");

        // Locked, a step, unlocked: the undo does not lock it again.
        led(&mut a, 4, false, true, false);
        let c = set_track!(&a.state, 4, volume: 0.3).unwrap();
        assert_eq!(apply(&mut a.state, "Mixer track volume", vec![c]), None);
        led(&mut a, 4, false, true, false);
        crate::history_ui::undo(&mut a.state);
        assert!(!lock(&a, 4), "stays unlocked");

        // A whole project put back (a track move) keeps it too.
        track_ids(&mut a.state);
        let c = Command::MoveTrack { track: id_of(&a.state, 9).unwrap(), to: 10 };
        assert_eq!(apply(&mut a.state, "Move mixer track", vec![c]), None);
        led(&mut a, 2, false, true, false);
        crate::history_ui::undo(&mut a.state);
        assert!(lock(&a, 2), "the lock stays through a project put back");
    }

    /// spec: ui/mixer-selection.md §1: Route this track to selected only, with every destination making a loop, leaves
    /// the track routed as it was, in no step.
    #[test]
    fn route_from_only_with_every_destination_refused_changes_nothing() {
        let mut a = app();
        route_one(&mut a, 3, 2);
        let steps = a.state.session.history.at();
        select_all(&mut a, &[3, 2]);
        let m = route_from(&mut a.state, 2, true, false);
        assert!(m.is_some_and(|m| m.contains("loop")));
        let s = &a.state.session;
        assert_eq!((send(s, 2, 0), send(s, 2, 3)), (Some(0.8), None), "still to the master");
        assert_eq!(s.history.at(), steps, "no step");
    }

    /// spec: ui/mixer-selection.md §1.1: Create submix... with no source routed opens no name box; when the routing is
    /// there already, the names make a step of their own, never joining an older one.
    #[test]
    fn create_submix_names_never_join_another_step() {
        let mut a = app();
        route_one(&mut a, 6, 2);
        let steps = a.state.session.history.at();
        select_all(&mut a, &[2, 6]);
        let c = cmd(&entry(&a, 6), "Create submix...");
        let m = crate::playlist::run(&mut a, c);
        assert!(a.state.playlist.edit.is_none() && a.state.mixer.submix.is_none(), "no name box: {m:?}");
        assert_eq!(a.state.session.history.at(), steps);

        let mut a = app();
        select_all(&mut a, &[2, 6]);
        let c = cmd(&entry(&a, 6), "Create submix...");
        crate::playlist::run(&mut a, c.clone());
        crate::playlist::commit_edit(&mut a.state);
        track_ids(&mut a.state);
        let r = set_track!(&a.state, 9, name: "Keys".to_string()).unwrap();
        assert_eq!(apply(&mut a.state, "Rename mixer track", vec![r]), None);
        let steps = a.state.session.history.at();
        crate::playlist::run(&mut a, c);
        assert_eq!(a.state.session.history.at(), steps, "routed already: no routing step");
        a.state.playlist.edit.as_mut().expect("the name box").text = "Bass".into();
        crate::playlist::commit_edit(&mut a.state);
        assert_eq!(name(&a.state.session, 6), "Bass master");
        assert_eq!(a.state.session.history.at(), steps + 1, "a step of their own");
        crate::history_ui::undo(&mut a.state);
        assert_eq!(
            (name(&a.state.session, 6), name(&a.state.session, 9)),
            ("Bus 6".to_string(), "Keys".to_string()),
            "the rename before stays"
        );
    }

    /// spec: ui/mixer-selection.md §1: a selected master is a destination of Route this track to selected; it is never
    /// a source, so Create submix... is greyed on it.
    #[test]
    fn a_selected_master_is_a_destination_only() {
        let mut a = app();
        select_all(&mut a, &[0, 2, 5]);
        let c = cmd(&entry(&a, 5), "Sidechain this track to selected");
        crate::playlist::run(&mut a, c);
        let s = &a.state.session;
        assert_eq!((send(s, 5, 2), send(s, 5, 0)), (Some(0.0), Some(0.0)));
        select_all(&mut a, &[2, 5, 0]);
        let e = entry(&a, 0);
        assert!(e.enabled);
        let sub = e.children.iter().find(|i| i.label == "Create submix...").unwrap();
        assert!(!sub.enabled, "greyed on the master");
        select_all(&mut a, &[0, 5]);
        let e = entry(&a, 5);
        assert!(!e.children.iter().find(|i| i.label == "Create submix...").unwrap().enabled, "no source but the master");
    }

    /// spec: ui/mixer-selection.md §4: a locked track's LED is a padlock, green while the track is on, grey muted.
    #[test]
    fn mute_lock_paints_a_padlock() {
        let Ok(mut c) = os_ui::Canvas::new(1920, 1080, 1.0) else { return };
        let mut a = app();
        a.state.z = vec![crate::app::Win::Mixer];
        let mut shot = |a: &mut App| {
            a.invalidate();
            a.paint(&mut c, None);
            let r = a.hits.iter().rev().find(|h| h.action == crate::app::Action::Mix(MHit::Led(7))).map(|h| h.rect).unwrap();
            let px: Vec<_> = (r.y as u32..r.bottom() as u32)
                .flat_map(|y| (r.x as u32..r.right() as u32).map(move |x| (x, y)))
                .filter_map(|(x, y)| c.pixmap.pixel(x, y).map(|p| (p.red(), p.green(), p.blue())))
                .collect();
            px
        };
        let plain = shot(&mut a);
        led(&mut a, 7, false, true, false);
        let locked = shot(&mut a);
        assert_ne!(plain, locked, "the padlock replaces the LED");
        assert!(locked.iter().any(|&(r, g, b)| g > r + 30 && g > b + 30), "green while on");
        led(&mut a, 7, false, false, false);
        let muted = shot(&mut a);
        assert!(!muted.iter().any(|&(r, g, b)| g > r + 30 && g > b + 30), "grey while muted");
        assert_ne!(muted, plain);
    }
}
