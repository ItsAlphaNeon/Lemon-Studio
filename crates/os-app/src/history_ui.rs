// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::history` (step 9: the session's side stays there).

#![allow(unused_imports)]
use std::borrow::Cow;
use os_model::{ControlPoint, Note};
use crate::app::State;
use crate::knobs::KnobId;
use crate::prefs;
#[allow(unused_imports)]
use crate::history::*;


/// Undoes the newest done step; its name, or `None` when nothing is done. The views follow.
pub fn undo(s: &mut State) -> Option<String> {
    if s.session.history.done > 0 {
        s.motion.borrow_mut().ripples_over();
    }
    crate::follow::around(s, |s| undo_in(&mut s.session))
}

/// Redoes the oldest undone step; its name, or `None` when nothing is undone. The views follow.
pub fn redo(s: &mut State) -> Option<String> {
    if s.session.history.done < s.session.history.steps.len() {
        s.motion.borrow_mut().ripples_over();
    }
    crate::follow::around(s, |s| redo_in(&mut s.session))
}

fn did(s: &mut State, redo_it: bool) -> String {
    let (verb, name) = if redo_it { ("Redo", redo(s)) } else { ("Undo", undo(s)) };
    match name {
        Some(n) => format!("{verb}: {}", lower(&n)),
        None => format!("Nothing to {}", verb.to_lowercase()),
    }
}

/// Ctrl+Z (and EDIT's first entry): with Alternate undo mode on, one more step back; off, it toggles the last move
/// (undo, redo, undo). With Show undo history in the browser when undoing changes on, the browser shows the history.
/// Returns the hint bar's message.
pub fn ctrl_z(s: &mut State) -> Option<String> {
    let m = did(s, s.session.history.ctrl_z_redoes(&s.session.settings));
    shown_in_browser(s);
    Some(m)
}

/// Ctrl+Alt+Z (and EDIT's Redo): with Alternate undo mode on, redo; off, one more step back.
pub fn ctrl_alt_z(s: &mut State) -> Option<String> {
    let m = did(s, prefs::check(&prefs::ALTERNATE_UNDO));
    shown_in_browser(s);
    Some(m)
}

/// Ctrl+Shift+Z (Command+Shift+Z, macOS's usual redo): redo, in either mode. A Lemon Studio addition: the reference
/// redoes with Ctrl+Alt+Z only.
pub fn ctrl_shift_z(s: &mut State) -> String {
    let m = did(s, true);
    shown_in_browser(s);
    m
}

fn shown_in_browser(s: &mut State) {
    if prefs::check(&prefs::UNDO_HISTORY_IN_BROWSER) {
        crate::browser::show_history(s);
    }
}

/// A click on entry `p` of the browser's History folder: undoes or redoes up to it.
pub fn go_to(s: &mut State, p: usize) -> Option<String> {
    let p = p.min(s.session.history.steps.len());
    while s.session.history.done > p {
        undo(s);
    }
    while s.session.history.done < p {
        redo(s);
    }
    let name = if p == 0 { LAST_RESET } else { &s.session.history.steps[p - 1].name };
    Some(format!("History: {}", lower(name)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;

    /// Step `k` of row `r` on or off in the shown pattern with no step of its own: the edit a step recorded before it
    /// holds.
    fn raw_step(s: &mut crate::app::State, r: usize, k: u32, on: bool) {
        let (n, ch) = (s.pattern().max(1), s.session.project.channels[r].index);
        crate::edit::rack::set_step_in(&mut s.session, n, ch, k, on, false);
    }

    /// A fader's value in dB.
    fn db(v: f64) -> f64 {
        20.0 * os_engine::laws::fader_gain(v).log10()
    }

    fn entry(label: &str, shortcut: &'static str, enabled: bool) -> (String, &'static str, bool) {
        (label.into(), shortcut, enabled)
    }

    use crate::app::Action;
    use crate::icons::Icon;
    use crate::views::browser::Item;
    use crate::views::menus::{bar_model, EDIT_MENU};
    use crate::keys::Key;

    fn note_count(s: &State) -> usize {
        s.session.project.pattern(s.pattern()).map_or(0, |p| p.notes.len())
    }

    /// Two recorded edits: undo and redo swap them back and forth; a new action drops the undone branch; past
    /// Maximum undo levels the oldest step goes (spec: ui/settings-choices.md "Undo history: the four checks").
    #[test]
    fn steps_swap_branch_and_limit() {
        let mut app = App::new(None);
        let s = &mut app.state;
        let start = note_count(s);
        for k in 0..2 {
            let snap = Snap::pattern(&s.session, s.pattern());
            record(&mut s.session, STEP_SEQ_EDIT, snap);
            raw_step(s, 0, 10 + k, true);
        }
        assert_eq!(note_count(s), start + 2);
        assert_eq!((undo(s).as_deref(), note_count(s)), (Some(STEP_SEQ_EDIT), start + 1));
        assert_eq!((undo(s).as_deref(), note_count(s)), (Some(STEP_SEQ_EDIT), start));
        assert_eq!(undo(s), None);
        assert_eq!((redo(s).as_deref(), note_count(s)), (Some(STEP_SEQ_EDIT), start + 1));
        assert!(s.session.history.undone(2) && s.session.history.current(1) && !s.session.history.undone(1));
        // A new action: the undone step is gone.
        let snap = Snap::pattern(&s.session, s.pattern());
        record(&mut s.session, "Piano roll add note", snap);
        raw_step(s, 0, 14, true);
        assert_eq!(s.session.history.entries(&crate::settings_file::store()).iter().map(|e| e.1).collect::<Vec<_>>(), [LAST_RESET, STEP_SEQ_EDIT, "Piano roll add note"]);
        assert_eq!(redo(s), None);
        // Three levels kept: the oldest go.
        prefs::set_number(&prefs::MAX_UNDO_LEVELS, 3);
        for k in 0..3 {
            let snap = Snap::pattern(&s.session, s.pattern());
            record(&mut s.session, "Piano roll edit", snap);
            raw_step(s, 0, 20 + k, true);
        }
        assert_eq!(s.session.history.len(), 3);
        assert!(s.session.history.entries(&crate::settings_file::store()).iter().skip(1).all(|e| e.1 == "Piano roll edit"), "{:?}", s.session.history.entries(&crate::settings_file::store()));
        prefs::set_number(&prefs::MAX_UNDO_LEVELS, 2);
        trim(&mut s.session);
        assert_eq!((s.session.history.len(), s.session.history.done), (2, 2));
        prefs::set_number(&prefs::MAX_UNDO_LEVELS, prefs::MAX_UNDO_LEVELS.default);
    }

    /// A step whose edit changed nothing is dropped when the button comes up (a step of an earlier press stays); a
    /// knob's wheel notches make one step.
    #[test]
    fn settle_and_wheel_runs() {
        let mut app = App::new(None);
        let s = &mut app.state;
        let snap = Snap::pattern(&s.session, s.pattern());
        record(&mut s.session, "Piano roll edit", snap);
        settle(&mut s.session);
        assert_eq!(s.session.history.len(), 0, "nothing changed");
        let snap = Snap::pattern(&s.session, s.pattern());
        record(&mut s.session, "Piano roll edit", snap);
        raw_step(s, 0, 9, !crate::rack::row_steps(&s.session, 0, 10)[9]);
        settle(&mut s.session);
        raw_step(s, 0, 9, !crate::rack::row_steps(&s.session, 0, 10)[9]);
        settle(&mut s.session);
        assert_eq!(s.session.history.len(), 1, "an earlier press's step stays");
        s.session.history.reset();
        let id = KnobId::ChanVol(0);
        let v0 = s.session.project.channels[0].volume;
        for _ in 0..3 {
            knob(&mut s.session, id, true);
            let v = s.knob(id) - 0.03;
            s.set_knob(id, v);
        }
        assert_eq!(s.session.history.len(), 1, "one step for the notches");
        assert_eq!(undo(s).as_deref(), Some("Channel volume"));
        assert_eq!(s.session.project.channels[0].volume, v0, "exactly back");
    }

    // ---------------------------------------------------------------------------------------------------------------
    // Through the app, as in the spec: two step-sequencer clicks (row 0 step 1, row 1 step 2), Ctrl+Z,
    // Ctrl+Alt+Z, the first channel's volume knob and the EDIT menu (spec: ui/settings-choices.md "Undo history: the
    // four checks (2026-10-16)").

    /// A click on step `k` of rack row `row`: press and release.
    fn click_step(app: &mut App, row: usize, k: u32) {
        app.act(Action::Step(row, k));
        app.mouse_up();
    }

    /// Rack rows 0 and 1, their first steps.
    fn steps(app: &App) -> [Vec<bool>; 2] {
        [0, 1].map(|r| crate::rack::row_steps(&app.state.session, r, 4))
    }

    fn ctrl_z(app: &mut App) {
        app.key(&Key::Character("z".into()), true, false);
    }

    fn ctrl_alt_z(app: &mut App) {
        app.key(&Key::Character("z".into()), true, true);
    }

    fn ctrl_shift_z(app: &mut App) {
        app.shift = true;
        app.key(&Key::Character("Z".into()), true, false);
        app.shift = false;
    }

    /// A drag on knob `id`, `dy` pixels down.
    fn turn(app: &mut App, id: KnobId, dy: f32) {
        app.mouse_move(100.0, 300.0);
        app.act(Action::Knob(id));
        app.mouse_move(100.0, 300.0 + dy);
        app.mouse_up();
    }

    /// EDIT as it shows now: (entry, shortcut, can be chosen).
    fn edit_menu(app: &App) -> Vec<(String, &'static str, bool)> {
        bar_model(&app.state)[EDIT_MENU].rows.iter().map(|r| (r.label.clone(), r.shortcut, r.enabled)).collect()
    }

    /// The browser's Current project ▸ History, opened: (name, mirrored arrow, drawn grey, where the project is).
    fn history_rows(app: &mut App) -> Vec<(String, bool, bool, bool)> {
        app.state.browser_tab = 1;
        app.state.browser_open.extend(["1:Current project".to_string(), "1:Current project/History".to_string()]);
        let h = &app.state.session.history;
        crate::views::browser::rows(&app.state)
            .into_iter()
            .filter_map(|r| match r.item {
                Item::History(e) => Some((r.label, r.icon == Icon::Redo, h.undone(e), r.bright)),
                _ => None,
            })
            .collect()
    }

    /// Alternate undo mode on (classic): each Ctrl+Z steps one further back, the third does nothing; Ctrl+Alt+Z
    /// redoes; EDIT shows "Undo step seq edit  Ctrl+Z" and "Redo step seq edit  Ctrl+Alt+Z" (spec:
    /// ui/settings-choices.md "Undo history: the four checks (2026-10-16)", Alternate undo mode On).
    #[test]
    fn alternate_mode_on_steps_back_and_redoes() {
        let mut app = App::new(None);
        assert!(prefs::check(&prefs::ALTERNATE_UNDO), "on by default");
        let start = steps(&app);
        click_step(&mut app, 0, 0);
        let one = steps(&app);
        click_step(&mut app, 1, 1);
        let two = steps(&app);
        assert!(start != one && one != two);
        assert_eq!(edit_menu(&app), [entry("Undo step seq edit", "Ctrl+Z", true), entry("Redo", "Ctrl+Alt+Z", false)]);
        ctrl_z(&mut app);
        assert_eq!(steps(&app), one, "the second click undone");
        assert_eq!(edit_menu(&app), [entry("Undo step seq edit", "Ctrl+Z", true), entry("Redo step seq edit", "Ctrl+Alt+Z", true)]);
        ctrl_z(&mut app);
        assert_eq!(steps(&app), start, "then the first");
        assert_eq!(edit_menu(&app), [entry("Undo", "Ctrl+Z", false), entry("Redo step seq edit", "Ctrl+Alt+Z", true)]);
        ctrl_z(&mut app);
        assert_eq!((steps(&app), app.state.message.as_deref()), (start.clone(), Some("Nothing to undo")), "a third does nothing");
        ctrl_alt_z(&mut app);
        assert_eq!(steps(&app), one, "Ctrl+Alt+Z redoes");
        // EDIT's Redo does what Ctrl+Alt+Z does.
        app.menu_item(EDIT_MENU, 1);
        assert_eq!(steps(&app), two);
        app.menu_item(EDIT_MENU, 0);
        assert_eq!(steps(&app), one);
    }

    /// Alternate undo mode off: Ctrl+Z three times undoes the second click, redoes it and undoes it again; Ctrl+Alt+Z
    /// then undoes the first. EDIT has one entry naming what Ctrl+Z does next, "Redo step seq edit  Ctrl+Z" after an
    /// undo (spec: ui/settings-choices.md "Undo history: the four checks (2026-10-16)", Alternate undo mode Off).
    #[test]
    fn alternate_mode_off_toggles_the_last_step() {
        let mut app = App::new(None);
        prefs::set_check(&prefs::ALTERNATE_UNDO, false);
        let start = steps(&app);
        click_step(&mut app, 0, 0);
        let one = steps(&app);
        click_step(&mut app, 1, 1);
        let two = steps(&app);
        assert_eq!(edit_menu(&app), [entry("Undo step seq edit", "Ctrl+Z", true)]);
        ctrl_z(&mut app);
        assert_eq!(steps(&app), one, "undid the second click");
        assert_eq!(edit_menu(&app), [entry("Redo step seq edit", "Ctrl+Z", true)]);
        ctrl_z(&mut app);
        assert_eq!(steps(&app), two, "redid it");
        assert_eq!(edit_menu(&app), [entry("Undo step seq edit", "Ctrl+Z", true)]);
        ctrl_z(&mut app);
        assert_eq!(steps(&app), one, "undid it again");
        ctrl_alt_z(&mut app);
        assert_eq!(steps(&app), start, "Ctrl+Alt+Z steps back further: the first click undone");
        ctrl_z(&mut app);
        assert_eq!(steps(&app), one, "Ctrl+Z toggles that back");
        // EDIT's one entry does what Ctrl+Z does.
        app.menu_item(EDIT_MENU, 0);
        assert_eq!(steps(&app), start);
        prefs::set_check(&prefs::ALTERNATE_UNDO, true);
    }

    /// Ctrl+Shift+Z (Command+Shift+Z on macOS) redoes in either mode, where Ctrl+Z alone would undo or toggle (a Lemon
    /// Studio addition; spec: ui/settings-choices.md "Undo history: the four checks (2026-10-16)").
    #[test]
    fn ctrl_shift_z_redoes_in_either_mode() {
        for alternate in [true, false] {
            let mut app = App::new(None);
            prefs::set_check(&prefs::ALTERNATE_UNDO, alternate);
            click_step(&mut app, 0, 0);
            let one = steps(&app);
            click_step(&mut app, 1, 1);
            let two = steps(&app);
            ctrl_z(&mut app);
            assert_eq!(steps(&app), one, "alternate undo mode {alternate}: undone");
            ctrl_shift_z(&mut app);
            assert_eq!(steps(&app), two, "alternate undo mode {alternate}: redone");
            ctrl_shift_z(&mut app);
            let nothing = (two.clone(), Some("Nothing to redo"));
            assert_eq!((steps(&app), app.state.message.as_deref()), nothing, "alternate undo mode {alternate}");
        }
        prefs::set_check(&prefs::ALTERNATE_UNDO, true);
    }

    /// EDIT with an empty history: only Undo, greyed out (spec: ui/menus.md "EDIT"; ui/settings-choices.md "Undo
    /// history: the four checks (2026-10-16)").
    #[test]
    fn edit_menu_with_no_history() {
        let app = App::new(None);
        assert_eq!(edit_menu(&app), [entry("Undo", "Ctrl+Z", false)]);
    }

    #[test]
    fn undoing_a_send_level_leaves_no_level_record() {
        // A route with no level record plays at unity; turning its send knob writes one, and undo takes it away again
        // instead of leaving one at unity (equivalence harness, System/Render tests/Hardcore.flp).
        let mut app = App::new(None);
        let s = &mut app.state;
        let t = s.session.project.mixer.iter().position(|m| m.index == 1).expect("track 1");
        s.session.project.get_mut().mixer[t].routes = vec![0];
        s.session.project.get_mut().mixer[t].send_levels.clear();
        let id = KnobId::Send(1, 0);
        knob(&mut s.session, id, true);
        s.set_knob(id, 0.5);
        assert_eq!(s.session.project.mixer[t].send_levels, vec![(0, 0.5)]);
        assert!(undo(s).is_some());
        assert_eq!(s.session.project.mixer[t].send_levels, vec![], "no record left behind");
        assert!(redo(s).is_some());
        assert_eq!(s.session.project.mixer[t].send_levels, vec![(0, 0.5)]);
    }

    #[test]
    fn undo_and_redo_of_a_pattern_edit_leave_the_pattern_shown() {
        // As in FL 24.1.1 (2026-11-09): a note added in pattern 1, pattern 2 shown, undo and redo change pattern 1's
        // notes and leave pattern 2 shown (equivalence harness, redo to the end showed the pattern edited).
        let mut app = App::new(None);
        let s = &mut app.state;
        s.set_pattern(1);
        crate::piano::snapshot_as(&mut s.session, "Piano roll edit");
        crate::patterns::pat_mut(&mut s.session, 1).notes.push(os_model::Note::step(0, 0));
        let edited = s.session.project.pattern(1).map(|p| p.notes.clone());
        s.set_pattern(2);
        assert!(undo(s).is_some());
        assert_eq!((s.pattern(), s.session.project.pattern(1).map(|p| p.notes.len())), (2, Some(0)));
        assert!(redo(s).is_some());
        assert_eq!((s.pattern(), s.session.project.pattern(1).map(|p| p.notes.clone())), (2, edited));
    }

    #[test]
    fn a_knob_turn_stays_a_step_when_a_track_before_it_is_deleted() {
        // Track 3's separation wheeled, then track 2 deleted: track 3's place now holds track 4's separation, the
        // value track 3 had before the wheel. The wheel's step was judged on that and dropped as a no-op; both steps
        // stay, and undoing them gives the project back (equivalence harness, nucleon - Fruity Delay 3).
        let mut app = App::new(None);
        let id = KnobId::MixSep(3);
        let start = app.state.session.project.clone();
        knob(&mut app.state.session, id, true);
        app.state.set_knob(id, 0.25);
        let steps = app.state.session.history.len();
        crate::mixer::run(&mut app, crate::mixer::MCmd::Delete(2));
        assert_eq!(app.state.session.history.len(), steps + 1, "the wheel's step stays");
        while undo(&mut app.state).is_some() {}
        assert_eq!(app.state.session.project.mixer, start.mixer);
    }

    /// Undo knob tweaks: on, a turn of a channel knob is a step and Ctrl+Z puts the knob back, the click before it
    /// staying; off, the turn makes no step and Ctrl+Z leaves the knob, undoing the click made before it (spec:
    /// ui/settings-choices.md "Undo history: the four checks (2026-10-16)", Undo knob tweaks).
    #[test]
    fn knob_tweaks_on_and_off() {
        let id = KnobId::ChanVol(0);
        for on in [true, false] {
            let mut app = App::new(None);
            prefs::set_check(&prefs::UNDO_KNOB_TWEAKS, on);
            let start = steps(&app);
            click_step(&mut app, 0, 0);
            let clicked = steps(&app);
            let v0 = app.state.session.project.channels[0].volume;
            turn(&mut app, id, 40.0);
            let v1 = app.state.session.project.channels[0].volume;
            assert_ne!(v1, v0, "the knob turned");
            assert_eq!(app.state.session.history.len(), if on { 2 } else { 1 });
            ctrl_z(&mut app);
            let vol = app.state.session.project.channels[0].volume;
            if on {
                assert_eq!((vol, steps(&app)), (v0, clicked), "the knob back, the click kept");
            } else {
                assert_eq!((vol, steps(&app)), (v1, start), "the knob kept, the click undone");
            }
        }
        prefs::set_check(&prefs::UNDO_KNOB_TWEAKS, true);
    }

    /// The browser's History: "Last reset" then the steps, oldest first; undone steps grey with the mirrored arrow; a
    /// new action drops them; newest first with Show recent changes at the top; a click on an entry goes there (spec:
    /// ui/settings-choices.md "Undo history: the four checks (2026-10-16)", Show undo history in the browser, Show
    /// recent changes at the top).
    #[test]
    fn history_folder_lists_greys_and_drops() {
        let mut app = App::new(None);
        let start = steps(&app);
        assert_eq!(history_rows(&mut app), [(LAST_RESET.into(), false, false, true)]);
        click_step(&mut app, 0, 0);
        click_step(&mut app, 1, 1);
        ctrl_z(&mut app);
        let seq = |undone: bool, now: bool| (STEP_SEQ_EDIT.to_string(), undone, undone, now);
        assert_eq!(history_rows(&mut app), [(LAST_RESET.into(), false, false, false), seq(false, true), seq(true, false)]);
        // Newest first: "Last reset" at the bottom.
        prefs::set_check(&prefs::RECENT_CHANGES_TOP, true);
        assert_eq!(history_rows(&mut app), [seq(true, false), seq(false, true), (LAST_RESET.into(), false, false, false)]);
        prefs::set_check(&prefs::RECENT_CHANGES_TOP, false);
        // A new action (a knob turn) drops the undone step: nothing to redo.
        turn(&mut app, KnobId::ChanVol(0), 40.0);
        let names: Vec<String> = history_rows(&mut app).into_iter().map(|r| r.0).collect();
        assert_eq!(names, [LAST_RESET, STEP_SEQ_EDIT, "Channel volume"]);
        assert!(history_rows(&mut app).iter().all(|r| !r.2), "none undone");
        ctrl_alt_z(&mut app);
        assert_eq!(app.state.message.as_deref(), Some("Nothing to redo"));
        // A click on "Last reset" undoes everything; on the newest entry, redoes it all.
        let rows = crate::views::browser::rows(&app.state);
        let at = |e: usize| rows.iter().position(|r| r.item == Item::History(e)).expect("history row");
        let (first, last) = (at(0), at(2));
        app.act(Action::BrowserRow(first));
        app.mouse_up();
        assert_eq!(steps(&app), start);
        assert!(app.state.session.history.undone(1) && app.state.session.history.undone(2) && app.state.session.history.current(0));
        assert!(app.drag.is_none(), "a history entry is not dragged");
        app.act(Action::BrowserRow(last));
        app.mouse_up();
        assert!(app.state.session.history.current(2));
    }

    /// Show undo history in the browser when undoing changes: off, Ctrl+Z leaves the browser where it is; on, the
    /// browser shows Current project ▸ History, opened (spec: ui/settings-choices.md "Undo history: the four checks
    /// (2026-10-16)").
    #[test]
    fn ctrl_z_shows_the_history_in_the_browser() {
        let mut app = App::new(None);
        click_step(&mut app, 0, 0);
        click_step(&mut app, 1, 1);
        app.state.show_browser = false;
        app.state.browser_tab = 2;
        let open = app.state.browser_open.clone();
        ctrl_z(&mut app);
        assert_eq!((app.state.show_browser, app.state.browser_tab, &app.state.browser_open), (false, 2, &open), "off by default");
        prefs::set_check(&prefs::UNDO_HISTORY_IN_BROWSER, true);
        ctrl_z(&mut app);
        assert!(app.state.show_browser);
        assert_eq!(app.state.browser_tab, 1);
        assert!(app.state.browser_open.contains("1:Current project/History"));
        assert_eq!(app.state.browser_sel.as_deref(), Some("1:Current project/History/0"), "where the project now is");
        prefs::set_check(&prefs::UNDO_HISTORY_IN_BROWSER, false);
    }

    /// Maximum undo levels: past it the oldest steps go, so those edits stay; the General page shows the value (spec:
    /// ui/settings-choices.md "Undo history: the four checks (2026-10-16)").
    #[test]
    fn max_levels_drop_the_oldest() {
        let mut app = App::new(None);
        assert_eq!(prefs::number(&prefs::MAX_UNDO_LEVELS), 100, "100 by default");
        prefs::set_number(&prefs::MAX_UNDO_LEVELS, 3);
        let shown = crate::views::settings::page(crate::views::settings::GENERAL, &app.state)
            .into_iter()
            .flat_map(|g| g.rows)
            .any(|r| matches!(r, crate::views::settings::Row::Value(v, "Maximum undo levels") if v == "3"));
        assert!(shown, "the General page shows the value");
        let start = crate::rack::row_steps(&app.state.session, 0, 5);
        for k in 0..5 {
            click_step(&mut app, 0, k);
        }
        assert_eq!(app.state.session.history.len(), 3);
        for _ in 0..4 {
            ctrl_z(&mut app);
        }
        let now = crate::rack::row_steps(&app.state.session, 0, 5);
        assert_eq!(now[..2], start[..2].iter().map(|b| !b).collect::<Vec<_>>()[..], "the two oldest clicks can no longer be undone");
        assert_eq!(now[2..], start[2..]);
        prefs::set_number(&prefs::MAX_UNDO_LEVELS, prefs::MAX_UNDO_LEVELS.default);
    }

    /// A drag on mixer track `t`'s volume fader, `dy` pixels down.
    fn fade(app: &mut App, t: u16, dy: f32) {
        app.mouse_move(100.0, 300.0);
        app.act(Action::Fader(t));
        app.mouse_move(100.0, 300.0 + dy);
        app.mouse_up();
    }

    fn names(s: &State) -> Vec<&str> {
        s.session.history.entries(&s.session.settings).into_iter().map(|e| e.1).collect()
    }

    /// A mixer fader drag is one step, "Mixer track volume", as EDIT names it; Ctrl+Z puts the fader back exactly and
    /// Ctrl+Alt+Z redoes it. A click without a move makes none, and with Undo knob tweaks off nor does a drag (OS).
    #[test]
    fn mixer_fader_drag_is_one_step() {
        let mut app = App::new(None);
        let vol = |app: &App| crate::mixer::track(&app.state.session, 1).map(|m| m.volume);
        let v0 = vol(&app);
        assert!(v0.is_some(), "insert 1 exists");
        fade(&mut app, 1, -30.0);
        let v1 = vol(&app);
        assert_ne!(v1, v0, "the fader moved");
        assert_eq!(names(&app.state), [LAST_RESET, MIXER_VOLUME]);
        assert_eq!(edit_menu(&app)[0], entry("Undo mixer track volume", "Ctrl+Z", true));
        ctrl_z(&mut app);
        assert_eq!(vol(&app), v0, "exactly back");
        ctrl_alt_z(&mut app);
        assert_eq!(vol(&app), v1, "redone");
        fade(&mut app, 1, 0.0);
        assert_eq!(app.state.session.history.len(), 1, "a click without a move");
        prefs::set_check(&prefs::UNDO_KNOB_TWEAKS, false);
        fade(&mut app, 1, 20.0);
        assert_ne!(vol(&app), v1);
        assert_eq!(app.state.session.history.len(), 1, "Undo knob tweaks off");
        prefs::set_check(&prefs::UNDO_KNOB_TWEAKS, true);
    }

    /// The mixer's faders `ts`, as values.
    fn volumes<const N: usize>(app: &App, ts: [u16; N]) -> [f64; N] {
        ts.map(|t| crate::mixer::track(&app.state.session, t).and_then(|m| m.volume).unwrap())
    }

    /// Puts tracks 1, 2, 3 at 0.2, 0.5 and 0.7, with 4 left alone, and selects `sel` with `current` as the current one.
    fn spread(app: &mut App, current: u16, sel: &[u16]) {
        group(app, [0.2, 0.5, 0.7], current, sel);
    }

    /// Puts tracks 1, 2, 3 at `levels` and selects `sel` with `current` as the current one.
    fn group(app: &mut App, levels: [f64; 3], current: u16, sel: &[u16]) {
        for (t, v) in [1, 2, 3].into_iter().zip(levels) {
            crate::mixer::track_mut(&mut app.state.session, t).unwrap().volume = Some(v);
        }
        app.state.selected_mixer = current;
        app.state.mixer.sel = sel.to_vec();
    }

    /// Tracks 1, 2 and 3 are `want` dB apart, each measured from track 2.
    fn apart(app: &App, want: [f64; 3]) {
        let [a, b, c] = volumes(app, [1, 2, 3]).map(db);
        for (got, want) in [a - b, 0.0, c - b].into_iter().zip(want) {
            assert!((got - want).abs() < 1e-6, "{got} dB apart, want {want} ({a}, {b}, {c})");
        }
    }

    /// Each fader stops at the top on its own while the others go on, and when the pressed one reaches -INF they all do.
    /// Back at the press point in the same drag, they are all exactly where they were. Spec: ui/mixer.md §4.
    #[test]
    fn mixer_group_fader_at_the_ends_of_the_range() {
        let mut app = App::new(None);
        let before = [0.712, 0.592, 0.48];
        group(&mut app, before, 1, &[1, 2, 3]);
        app.mouse_move(100.0, 300.0);
        app.act(Action::Fader(2));
        app.mouse_move(100.0, -700.0);
        let [a, b, c] = volumes(&app, [1, 2, 3]);
        assert_eq!((a, b), (1.0, 1.0), "at the top");
        assert!((db(c) - db(before[2]) - (db(1.0) - db(before[1]))).abs() < 1e-6, "the lowest went on by the same dB: {c}");
        app.mouse_move(100.0, 1300.0);
        assert_eq!(volumes(&app, [1, 2, 3]), [0.0; 3], "the pressed one at -INF takes them all there");
        app.mouse_move(100.0, 300.0);
        assert_eq!(volumes(&app, [1, 2, 3]), before, "back at the press point");
        app.mouse_up();
    }

    /// A fader at -INF stays there while the others move, and a press on it sends them to the top. Spec: ui/mixer.md §4.
    #[test]
    fn mixer_group_fader_at_minus_inf() {
        let mut app = App::new(None);
        let before = [0.6, 0.5, 0.0];
        group(&mut app, before, 1, &[1, 2, 3]);
        fade(&mut app, 2, 20.0);
        let [a, b, c] = volumes(&app, [1, 2, 3]);
        assert!(a < before[0] && b < before[1] && c == 0.0, "{a}, {b}, {c}");
        fade(&mut app, 2, -40.0);
        let [a, b, c] = volumes(&app, [1, 2, 3]);
        assert!(a > before[0] && b > before[1] && c == 0.0, "{a}, {b}, {c}");
        ctrl_z(&mut app);
        ctrl_z(&mut app);
        assert_eq!(volumes(&app, [1, 2, 3]), before);
        fade(&mut app, 3, -40.0);
        let [a, b, c] = volumes(&app, [1, 2, 3]);
        assert_eq!((a, b), (1.0, 1.0), "the others at the top");
        assert!(c > 0.0 && c < 0.2, "the pressed one follows the mouse: {c}");
        ctrl_z(&mut app);
        assert_eq!(volumes(&app, [1, 2, 3]), before, "one step");
    }

    /// A fader stopped at the top stays there through new drags until the others have come down far enough, then they are
    /// as far apart as they were. A press on it brings the others back at once. Spec: ui/mixer.md §4.
    #[test]
    fn mixer_group_fader_keeps_the_differences_between_drags() {
        let before = [0.712, 0.592, 0.48];
        let want = [db(before[0]) - db(before[1]), 0.0, db(before[2]) - db(before[1])];
        let mut app = App::new(None);
        group(&mut app, before, 1, &[1, 2, 3]);
        fade(&mut app, 2, -1000.0);
        assert_eq!(volumes(&app, [1, 2])[..], [1.0, 1.0]);
        let mut stopped = false;
        while volumes(&app, [1])[0] == 1.0 {
            fade(&mut app, 2, 10.0);
            let [a, b, c] = volumes(&app, [1, 2, 3]);
            assert!(b < 1.0);
            assert!((db(c) - db(b) - want[2]).abs() < 1e-6, "the lowest stays as far from the pressed one");
            stopped |= a == 1.0;
        }
        assert!(stopped, "it stayed at the top for a drag at least");
        apart(&app, want);

        group(&mut app, before, 1, &[1, 2, 3]);
        app.state.mixer.faders = Default::default();
        fade(&mut app, 2, -1000.0);
        fade(&mut app, 1, 10.0);
        let a = volumes(&app, [1])[0];
        assert!(a < 1.0);
        apart(&app, want);
    }

    /// A group taken to -INF and released comes back as it was on the next drag. Spec: ui/mixer.md §4.
    #[test]
    fn mixer_group_fader_comes_back_from_minus_inf() {
        let before = [0.712, 0.592, 0.48];
        let want = [db(before[0]) - db(before[1]), 0.0, db(before[2]) - db(before[1])];
        let mut app = App::new(None);
        group(&mut app, before, 1, &[1, 2, 3]);
        fade(&mut app, 3, 1000.0);
        assert_eq!(volumes(&app, [1, 2, 3]), [0.0; 3]);
        fade(&mut app, 3, -60.0);
        assert!(volumes(&app, [3])[0] > 0.0);
        apart(&app, want);
    }

    /// Making the selection again starts from the faders as they are, even with the same tracks. So does anything else
    /// that moved a fader since the last drag. Spec: ui/mixer.md §4.
    #[test]
    fn mixer_group_fader_forgets_the_differences() {
        let before = [0.712, 0.592, 0.48];
        let mut app = App::new(None);
        group(&mut app, before, 1, &[1, 2, 3]);
        fade(&mut app, 2, -1000.0);
        crate::mixer::select(&mut app.state, 1, false);
        for t in [2, 3] {
            crate::mixer::select(&mut app.state, t, true);
        }
        let [_, _, c] = volumes(&app, [1, 2, 3]);
        fade(&mut app, 2, 10.0);
        let [a, b, c2] = volumes(&app, [1, 2, 3]);
        assert!(b < 1.0);
        assert!((a - b).abs() < 1e-9, "both were at the top, and that is what counts now: {a}, {b}");
        assert!((db(c2) - db(b) - (db(c) - db(1.0))).abs() < 1e-6);

        group(&mut app, before, 1, &[1, 2, 3]);
        fade(&mut app, 2, -1000.0);
        let c = volumes(&app, [3])[0];
        crate::mixer::track_mut(&mut app.state.session, 3).unwrap().volume = Some(c / 2.0);
        fade(&mut app, 2, 10.0);
        let [a, b, c2] = volumes(&app, [1, 2, 3]);
        assert!((a - b).abs() < 1e-9, "the fader moved by something else: they start from what they show: {a}, {b}");
        assert!((db(c2) - db(b) - (db(c / 2.0) - db(1.0))).abs() < 1e-6);

        group(&mut app, before, 1, &[1, 2, 3]);
        app.state.mixer.faders = Default::default();
        fade(&mut app, 2, -1000.0);
        ctrl_z(&mut app);
        assert_eq!(volumes(&app, [1, 2, 3]), before);
        fade(&mut app, 2, 10.0);
        apart(&app, [db(before[0]) - db(before[1]), 0.0, db(before[2]) - db(before[1])]);
    }

    /// The Master moves with the others when selected, and a press on its fader moves them all. Spec: ui/mixer.md §4.
    #[test]
    fn mixer_group_fader_with_the_master() {
        let mut app = App::new(None);
        spread(&mut app, 1, &[0, 1, 2]);
        let before = volumes(&app, [0, 1, 2, 3]);
        fade(&mut app, 1, 20.0);
        let after = volumes(&app, [0, 1, 2, 3]);
        assert!(after[0] < before[0] && after[2] < before[2], "{after:?}");
        assert_eq!(after[3], before[3]);
        fade(&mut app, 0, 20.0);
        let again = volumes(&app, [0, 1, 2, 3]);
        assert!(again[0] < after[0] && again[1] < after[1] && again[2] < after[2], "{again:?}");
        assert_eq!(app.state.selected_mixer, 0);
        assert_eq!(crate::mixer::selected(&app.state).len(), 3, "the selection stays");
    }

    /// A plain drag on the fader of a selected track moves every selected one by the same number of dB, and the others
    /// stay. The press keeps the selection and makes that track current. All of it is one step: Ctrl+Z puts every
    /// fader back exactly and Ctrl+Alt+Z redoes them. Spec: ui/mixer.md §2, §4.
    #[test]
    fn mixer_group_fader_drag_is_one_step() {
        let mut app = App::new(None);
        spread(&mut app, 1, &[1, 2, 3]);
        let (before, steps) = (volumes(&app, [1, 2, 3, 4]), app.state.session.history.len());
        fade(&mut app, 2, 40.0);
        let after = volumes(&app, [1, 2, 3, 4]);
        assert!(after[1] < before[1], "the pressed fader moved");
        assert_eq!(after[3], before[3], "a track outside the selection stays");
        let db = |v: f64| 20.0 * os_engine::laws::fader_gain(v).log10();
        let moved = db(after[1]) - db(before[1]);
        for i in [0, 2] {
            assert!((db(after[i]) - db(before[i]) - moved).abs() < 1e-9, "fader {i} moved {moved} dB too");
        }
        assert_eq!(app.state.selected_mixer, 2, "the pressed track is current");
        assert_eq!(crate::mixer::selected(&app.state), [2, 1, 3], "the selection stays");
        assert_eq!(app.state.session.history.len(), steps + 1, "one step");
        assert_eq!(edit_menu(&app)[0], entry("Undo mixer track volume", "Ctrl+Z", true));
        ctrl_z(&mut app);
        assert_eq!(volumes(&app, [1, 2, 3, 4]), before, "exactly back");
        ctrl_alt_z(&mut app);
        assert_eq!(volumes(&app, [1, 2, 3, 4]), after, "redone");
    }

    /// A click on the fader of a selected track moves nothing and makes no step, and the selection stays. A drag there
    /// with Undo knob tweaks off moves them all without one (OS, as for one fader).
    #[test]
    fn mixer_group_fader_click_and_no_undo_knob_tweaks() {
        let mut app = App::new(None);
        spread(&mut app, 1, &[1, 2, 3]);
        let (before, steps) = (volumes(&app, [1, 2, 3]), app.state.session.history.len());
        fade(&mut app, 3, 0.0);
        assert_eq!(volumes(&app, [1, 2, 3]), before, "nothing moved");
        assert_eq!(app.state.session.history.len(), steps, "no step");
        assert_eq!((app.state.selected_mixer, crate::mixer::selected(&app.state)), (3, vec![3, 1, 2]));
        prefs::set_check(&prefs::UNDO_KNOB_TWEAKS, false);
        fade(&mut app, 3, 20.0);
        prefs::set_check(&prefs::UNDO_KNOB_TWEAKS, true);
        let after = volumes(&app, [1, 2, 3]);
        assert!((0..3).all(|i| after[i] < before[i]), "all moved");
        assert_eq!(app.state.session.history.len(), steps, "Undo knob tweaks off");
    }

    /// A drag on a fader outside the selection moves that one alone, and it alone is selected now.
    #[test]
    fn mixer_fader_drag_outside_the_selection_moves_one() {
        let mut app = App::new(None);
        spread(&mut app, 1, &[1, 2]);
        let before = volumes(&app, [1, 2, 3]);
        fade(&mut app, 3, 20.0);
        let after = volumes(&app, [1, 2, 3]);
        assert_eq!(after[..2], before[..2], "the selection's faders stay");
        assert!(after[2] < before[2]);
        assert_eq!(crate::mixer::selected(&app.state), [3], "it alone is selected now");
    }

    /// Press and release on fader `t` with Alt held, moving the mouse in between. Returns the hint bar's text right after
    /// the press.
    fn alt_click_fader(app: &mut App, t: u16) -> String {
        app.alt = true;
        app.mouse_move(100.0, 300.0);
        app.act(Action::Fader(t));
        let hint = app.hint.to_string();
        app.mouse_move(100.0, 360.0);
        app.mouse_up();
        app.alt = false;
        hint
    }

    /// Alt+click puts a fader back at unity, from below or above it, as a step of its own: Ctrl+Z brings back the value
    /// before the click, a second one undoes the drag before that, and Ctrl+Alt+Z redoes them. No drag follows the press.
    /// A fader already at unity makes no step, and with Undo knob tweaks off it resets without one (OS). Spec:
    /// ui/mixer.md §4.
    #[test]
    fn mixer_fader_alt_click_is_one_step() {
        const UNITY: Option<f64> = Some(0.8);
        let mut app = App::new(None);
        let vol = |app: &App, t| crate::mixer::track(&app.state.session, t).and_then(|m| m.volume);
        let v0 = vol(&app, 1);
        assert_eq!(v0, UNITY, "a new track is at unity");
        fade(&mut app, 1, 40.0);
        let v1 = vol(&app, 1);
        assert_ne!(v1, UNITY, "the fader moved");
        assert_eq!(alt_click_fader(&mut app, 1), "Volume: 100%  0.0dB  1.00");
        assert_eq!(vol(&app, 1), UNITY, "at unity, and the mouse moving after the press did not move it");
        assert!(app.state.mixer.sel.is_empty());
        assert_eq!(app.state.selected_mixer, 1, "the strip is selected");
        assert_eq!(names(&app.state), [LAST_RESET, MIXER_VOLUME, MIXER_VOLUME], "a step of its own");
        assert_eq!(edit_menu(&app)[0], entry("Undo mixer track volume", "Ctrl+Z", true));
        ctrl_z(&mut app);
        assert_eq!(vol(&app, 1), v1, "the value before the click");
        ctrl_z(&mut app);
        assert_eq!(vol(&app, 1), v0, "the drag undone");
        ctrl_alt_z(&mut app);
        assert_eq!(vol(&app, 1), v1);
        ctrl_alt_z(&mut app);
        assert_eq!(vol(&app, 1), UNITY, "redone");
        let steps = app.state.session.history.len();
        alt_click_fader(&mut app, 1);
        assert_eq!(app.state.session.history.len(), steps, "nothing changed");
        fade(&mut app, 2, -30.0);
        assert!(vol(&app, 2) > UNITY, "above unity");
        alt_click_fader(&mut app, 2);
        assert_eq!(vol(&app, 2), UNITY, "from above");
        fade(&mut app, 0, 30.0);
        assert_ne!(vol(&app, 0), UNITY, "the Master's fader moved");
        alt_click_fader(&mut app, 0);
        assert_eq!(vol(&app, 0), UNITY, "the Master's too");
        fade(&mut app, 3, 40.0);
        let steps = app.state.session.history.len();
        prefs::set_check(&prefs::UNDO_KNOB_TWEAKS, false);
        alt_click_fader(&mut app, 3);
        assert_eq!(vol(&app, 3), UNITY);
        assert_eq!(app.state.session.history.len(), steps, "Undo knob tweaks off");
        prefs::set_check(&prefs::UNDO_KNOB_TWEAKS, true);
    }

    /// With several tracks selected, Alt+click on the fader of a selected one resets nothing: the selection stays and
    /// that track becomes current. On any other fader it resets that one, which becomes the only one selected. No undo
    /// step where nothing moved (OS: the original adds one). Spec: ui/mixer.md §4.
    #[test]
    fn mixer_fader_alt_click_with_several_selected() {
        let mut app = App::new(None);
        for (t, v) in [(1, 0.2), (2, 0.3), (3, 0.4)] {
            crate::mixer::track_mut(&mut app.state.session, t).unwrap().volume = Some(v);
        }
        let vols = |app: &App| [1, 2, 3].map(|t| crate::mixer::track(&app.state.session, t).and_then(|m| m.volume).unwrap());
        app.state.selected_mixer = 1;
        app.state.mixer.sel = vec![1, 2];
        let steps = app.state.session.history.len();
        alt_click_fader(&mut app, 2);
        assert_eq!(vols(&app), [0.2, 0.3, 0.4], "nothing resets");
        assert_eq!(app.state.session.history.len(), steps, "no step");
        assert_eq!(app.state.selected_mixer, 2, "the pressed track is current");
        assert_eq!(crate::mixer::selected(&app.state), [2, 1], "the selection stays");
        alt_click_fader(&mut app, 3);
        assert_eq!(vols(&app), [0.2, 0.3, 0.8], "only the one outside the selection");
        assert_eq!(app.state.session.history.len(), steps + 1);
        assert_eq!(crate::mixer::selected(&app.state), [3], "it alone is selected now");
    }

    /// Press and release on knob `id` with Alt held, moving the mouse in between. Returns the hint bar's text right after
    /// the press.
    fn alt_click_knob(app: &mut App, id: KnobId) -> String {
        app.alt = true;
        app.mouse_move(100.0, 300.0);
        app.act(Action::Knob(id));
        let hint = app.hint.to_string();
        app.mouse_move(100.0, 360.0);
        app.mouse_up();
        app.alt = false;
        hint
    }

    /// Alt+click puts a mixer track's pan or stereo separation knob back at its default, from either side, as a step of
    /// its own: Ctrl+Z brings back the value before the click, a second one undoes the drag before that, and Ctrl+Alt+Z
    /// redoes them. No drag follows the press. A knob already at its default makes no step, and with Undo knob tweaks off
    /// it resets without one (OS). Spec: ui/mixer.md §4.
    #[test]
    fn mixer_knob_alt_click_is_one_step() {
        let knobs: [(fn(u16) -> KnobId, &str, &str, &str); 2] = [
            (KnobId::MixPan, "Mixer track panning", "Undo mixer track panning", "Panning: Centered"),
            (KnobId::MixSep, "Mixer track stereo separation", "Undo mixer track stereo separation", "Stereo separation: Original"),
        ];
        for (knob, name, undo_entry, default) in knobs {
            let mut app = App::new(None);
            let text = |app: &App, id| app.state.knob_text(id);
            let k = knob(1);
            assert_eq!(text(&app, k), default, "a new track's");
            turn(&mut app, k, 40.0);
            let moved = text(&app, k);
            assert_ne!(moved, default, "the knob moved");
            assert_eq!(alt_click_knob(&mut app, k), default);
            assert_eq!(app.state.knob(k), 0.5, "at its default, and the mouse moving after the press did not move it");
            assert_eq!(names(&app.state), [LAST_RESET, name, name], "a step of its own");
            assert_eq!(edit_menu(&app)[0], entry(undo_entry, "Ctrl+Z", true));
            ctrl_z(&mut app);
            assert_eq!(text(&app, k), moved, "the value before the click");
            ctrl_z(&mut app);
            assert_eq!(text(&app, k), default, "the drag undone");
            ctrl_alt_z(&mut app);
            assert_eq!(text(&app, k), moved);
            ctrl_alt_z(&mut app);
            assert_eq!(text(&app, k), default, "redone");
            let steps = app.state.session.history.len();
            alt_click_knob(&mut app, k);
            assert_eq!(app.state.session.history.len(), steps, "nothing changed");
            turn(&mut app, knob(2), -30.0);
            assert_ne!(text(&app, knob(2)), default, "the other side");
            alt_click_knob(&mut app, knob(2));
            assert_eq!(app.state.knob(knob(2)), 0.5, "from the other side");
            turn(&mut app, knob(0), 30.0);
            assert_ne!(text(&app, knob(0)), default, "the Master's knob moved");
            alt_click_knob(&mut app, knob(0));
            assert_eq!(app.state.knob(knob(0)), 0.5, "the Master's too");
            turn(&mut app, knob(3), 40.0);
            let steps = app.state.session.history.len();
            prefs::set_check(&prefs::UNDO_KNOB_TWEAKS, false);
            alt_click_knob(&mut app, knob(3));
            assert_eq!(app.state.knob(knob(3)), 0.5);
            assert_eq!(app.state.session.history.len(), steps, "Undo knob tweaks off");
            prefs::set_check(&prefs::UNDO_KNOB_TWEAKS, true);
        }
    }

    /// With several tracks selected, Alt+click on a pan or stereo separation knob resets only that knob, unlike a fader's,
    /// and the selection stays, in it or outside it. The step undoes only that knob. Spec: ui/mixer.md §4.
    #[test]
    fn mixer_knob_alt_click_with_several_selected() {
        const OFF: f32 = 0.25;
        const DEFAULT: f32 = 0.5;
        let mut app = App::new(None);
        for t in 1..=4 {
            app.state.set_knob(KnobId::MixPan(t), OFF);
            app.state.set_knob(KnobId::MixSep(t), OFF);
        }
        let at = |app: &App| [1, 2, 3, 4].map(|t| [app.state.knob(KnobId::MixPan(t)), app.state.knob(KnobId::MixSep(t))]);
        app.state.selected_mixer = 1;
        app.state.mixer.sel = vec![1, 2, 3];
        let steps = app.state.session.history.len();
        assert_eq!(alt_click_knob(&mut app, KnobId::MixPan(2)), "Panning: Centered");
        assert_eq!(at(&app), [[OFF, OFF], [DEFAULT, OFF], [OFF, OFF], [OFF, OFF]], "a selected track's pan, only that one");
        assert_eq!(app.state.session.history.len(), steps + 1);
        assert_eq!(crate::mixer::selected(&app.state), [1, 2, 3], "the selection stays");
        assert_eq!(alt_click_knob(&mut app, KnobId::MixSep(1)), "Stereo separation: Original");
        assert_eq!(at(&app), [[OFF, DEFAULT], [DEFAULT, OFF], [OFF, OFF], [OFF, OFF]], "the current track's separation, only that one");
        assert_eq!(app.state.session.history.len(), steps + 2);
        assert_eq!(crate::mixer::selected(&app.state), [1, 2, 3], "the selection stays");
        alt_click_knob(&mut app, KnobId::MixPan(4));
        assert_eq!(at(&app), [[OFF, DEFAULT], [DEFAULT, OFF], [OFF, OFF], [DEFAULT, OFF]], "a knob outside the selection");
        assert_eq!(crate::mixer::selected(&app.state), [1, 2, 3], "the selection stays");
        ctrl_z(&mut app);
        assert_eq!(at(&app), [[OFF, DEFAULT], [DEFAULT, OFF], [OFF, OFF], [OFF, OFF]], "the step undoes that knob alone");
    }

    /// A generator added to the rack is one step, "Add channel" (`control::edit_app`): undone, the channel goes and its
    /// instance is kept; redone, the channel comes back with it (`control::host`).
    #[test]
    fn an_added_generator_is_one_step() {
        let mut a = App::new(None);
        a.state.session.plugins = crate::plugins::Plugins::default();
        a.state.session.plugins.available = crate::plugins::builtin();
        let n = a.state.session.project.channels.len();
        let sampler = a.state.session.plugins.index_of(crate::plugins::LEMON_SAMPLER).unwrap();
        let trio = a.state.session.plugins.index_of(crate::osc3::LEMON_TRIO).unwrap();
        crate::plugin_menu::add_generator(&mut a, sampler, None);
        crate::plugin_menu::add_generator(&mut a, trio, None);
        assert_eq!(a.state.session.project.channels.len(), n + 2);
        assert_eq!(names(&a.state), [LAST_RESET, ADD_CHANNEL, ADD_CHANNEL]);
        a.plugin_tick();
        assert!(a.state.session.plugins.hosted.contains_key(&(n + 1)), "the Trio is hosted");
        assert_eq!(undo(&mut a.state).as_deref(), Some(ADD_CHANNEL));
        a.plugin_tick();
        assert_eq!((a.state.session.project.channels.len(), a.state.selected_channel), (n + 1, n), "the Trio channel went");
        assert!(!a.state.session.plugins.hosted.contains_key(&(n + 1)), "and its instance");
        assert_eq!(redo(&mut a.state).as_deref(), Some(ADD_CHANNEL));
        a.plugin_tick();
        assert_eq!(a.state.session.project.channels.len(), n + 2);
        assert!(a.state.session.plugins.hosted.contains_key(&(n + 1)), "its instance is back");
        assert_eq!(a.state.session.project.channels[n + 1].plugin, Some(crate::osc3::file_name(crate::osc3::LEMON_TRIO).to_string()));
        undo(&mut a.state);
        undo(&mut a.state);
        assert_eq!(a.state.session.project.channels.len(), n, "both gone");
    }

    /// A channel delete is one step, "Delete channel": undone, the channel is back in its row with its notes and the
    /// rows below move back down; redone, it goes again (OS). Its instrument follows (`control::host`).
    #[test]
    fn a_channel_delete_is_one_step() {
        let mut app = App::new(None);
        assert!(app.state.session.project.channels.len() >= 3);
        app.state.session.project.get_mut().patterns[0].notes = vec![os_model::Note::step(1, 0), os_model::Note::step(2, 4)];
        let (chans, pats) = (app.state.session.project.channels.clone(), app.state.session.project.patterns.clone());
        crate::rack::delete_step(&mut app, &[1]);
        let (gone_chans, gone_pats) = (app.state.session.project.channels.clone(), app.state.session.project.patterns.clone());
        assert_eq!(gone_chans.len(), chans.len() - 1);
        assert_eq!(names(&app.state), [LAST_RESET, DELETE_CHANNEL]);
        ctrl_z(&mut app);
        assert_eq!((&app.state.session.project.channels, &app.state.session.project.patterns), (&chans, &pats), "all back");
        ctrl_alt_z(&mut app);
        assert_eq!((&app.state.session.project.channels, &app.state.session.project.patterns), (&gone_chans, &gone_pats), "deleted again");
    }

    /// Settings > Project: the bar, its mode and the panning law are steps, named as the control plane's commands
    /// name them ("Time signature"), put back by Ctrl+Z (OS).
    #[test]
    fn project_settings_are_steps() {
        let mut app = App::new(None);
        let now = |app: &App| {
            let p = &app.state.session.project;
            (p.bar, p.bar_as_signature, p.panning_law)
        };
        let start = now(&app);
        app.set_panning_law(2);
        app.set_bar(3, 4);
        app.set_time_mode(!app.state.session.project.bar_is_signature());
        let sig = crate::control::command::TIME_SIGNATURE;
        assert_eq!(names(&app.state), [LAST_RESET, "Panning law", sig, sig]);
        app.set_panning_law(2);
        assert_eq!(app.state.session.history.len(), 3, "the same value again makes no step");
        assert_eq!(edit_menu(&app)[0], entry("Undo time signature", "Ctrl+Z", true));
        ctrl_z(&mut app);
        assert_eq!(app.state.session.project.bar(), (3, 4), "the mode back");
        ctrl_z(&mut app);
        ctrl_z(&mut app);
        assert_eq!(now(&app), start);
        ctrl_alt_z(&mut app);
        assert_eq!(app.state.session.project.panning_law, Some(2));
    }

    /// The timebase is a "Project settings" step: undone, the ticks a lower timebase rounded come back exactly; redone,
    /// the rounded ones; the same timebase again makes no step (OS).
    #[test]
    fn a_timebase_change_is_a_step() {
        let mut app = App::new(None);
        app.set_timebase(96);
        app.state.session.project.get_mut().patterns[0].notes = vec![os_model::Note { length: 7, ..os_model::Note::step(0, 5) }];
        let before = app.state.session.project.clone();
        app.set_timebase(24);
        app.set_timebase(24);
        assert_eq!(names(&app.state), [LAST_RESET, PROJECT_SETTINGS]);
        let n = app.state.session.project.patterns[0].notes[0];
        assert_eq!((n.position, n.length), (1, 2), "rounded to the new ticks");
        let after = app.state.session.project.clone();
        ctrl_z(&mut app);
        assert_eq!(app.state.session.project, before, "every tick back");
        assert!(app.state.session.retimed, "playback stops");
        ctrl_alt_z(&mut app);
        assert_eq!(app.state.session.project, after);
        app.plugin_tick();
        assert!(!app.state.session.retimed);
    }

    /// Settings > Info: an accepted box edit and a genre picked are each one step (`control::edit`); an edit that changes
    /// nothing and one dropped with Escape make none (OS).
    #[test]
    fn project_info_edits_are_steps() {
        use crate::views::settings::InfoField;
        use crate::keys::{Key, NamedKey};
        let mut app = App::new(None);
        let start = app.state.session.project.info.clone();
        let type_in = |app: &mut App, field: InfoField, text: &str| {
            app.info_act(field);
            app.state.settings.info_edit.as_mut().unwrap().text.set(text);
        };
        type_in(&mut app, InfoField::Title, "  Song ");
        crate::project_info_ui::accept_info(&mut app.state);
        type_in(&mut app, InfoField::Title, "Song");
        crate::project_info_ui::accept_info(&mut app.state);
        type_in(&mut app, InfoField::Author, "Someone");
        app.state.settings.info_edit = None;
        app.info_act(InfoField::Author);
        app.state.settings.open = true;
        app.state.settings.tab = crate::views::settings::INFO;
        app.info_key(&Key::Named(NamedKey::Escape), false, false);
        app.set_genre(Some(crate::project_info::GENRES[0]));
        let info = crate::control::command::SONG_INFO;
        assert_eq!(names(&app.state), [LAST_RESET, info, info]);
        assert_eq!(app.state.session.project.info.title.as_deref(), Some("Song"));
        ctrl_z(&mut app);
        assert_eq!(app.state.session.project.info.genre, start.genre, "the genre back");
        ctrl_z(&mut app);
        assert_eq!(app.state.session.project.info, start);
        ctrl_alt_z(&mut app);
        assert_eq!(app.state.session.project.info.title.as_deref(), Some("Song"));
    }

    /// A generator inserted higher up the rack is one step, "Add channel": undone, it goes and the rows under it move
    /// back up with their notes; redone, it comes back in its row with its instance (`control::host`).
    #[test]
    fn an_inserted_generator_is_one_step() {
        let mut a = App::new(None);
        a.state.session.plugins = crate::plugins::Plugins::default();
        a.state.session.plugins.available = crate::plugins::builtin();
        let trio = a.state.session.plugins.index_of(crate::osc3::LEMON_TRIO).unwrap();
        assert!(a.state.session.project.channels.len() >= 3);
        a.state.session.project.get_mut().patterns[0].notes = vec![os_model::Note::step(1, 0), os_model::Note::step(2, 4)];
        let (chans, pats) = (a.state.session.project.channels.clone(), a.state.session.project.patterns.clone());
        crate::plugin_menu::add_generator(&mut a, trio, Some(0));
        a.plugin_tick();
        let trio_name = Some(crate::osc3::LEMON_TRIO.to_string());
        assert_eq!(a.state.session.plugins.descriptor(1).map(|d| d.name), trio_name, "hosted in its row");
        let (more, moved) = (a.state.session.project.channels.clone(), a.state.session.project.patterns.clone());
        assert_eq!((more.len(), a.state.selected_channel), (chans.len() + 1, 1));
        assert_ne!(moved, pats, "the notes follow their rows");
        assert_eq!(names(&a.state), [LAST_RESET, ADD_CHANNEL]);
        ctrl_z(&mut a);
        a.plugin_tick();
        assert_eq!((&a.state.session.project.channels, &a.state.session.project.patterns), (&chans, &pats), "all as before");
        assert_ne!(a.state.session.plugins.descriptor(1).map(|d| d.name), trio_name, "the instance goes");
        ctrl_alt_z(&mut a);
        a.plugin_tick();
        assert_eq!((&a.state.session.project.channels, &a.state.session.project.patterns), (&more, &moved), "inserted again");
        assert_eq!(a.state.session.plugins.descriptor(1).map(|d| d.name), trio_name, "with its instance");
    }

    /// Replace is one step, "Replace channel": undone, the channel and its old instrument come back; redone, the new
    /// one (`control::host`: the channel takes a new id, the old instrument is kept by the old one's).
    #[test]
    fn replace_is_one_step() {
        let mut a = App::new(None);
        a.state.session.plugins = crate::plugins::Plugins::default();
        a.state.session.plugins.available = crate::plugins::builtin();
        a.plugin_tick();
        a.plugin_tick();
        let ticks = |a: &mut App| (0..3).for_each(|_| a.plugin_tick());
        let name = |a: &App| a.state.session.plugins.descriptor(0).map(|d| d.name);
        let (old, old_name) = (a.state.session.project.channels[0].clone(), name(&a));
        assert!(old_name.is_some());
        let trio = a.state.session.plugins.index_of(crate::osc3::LEMON_TRIO).unwrap();
        crate::plugin_menu::replace_generator(&mut a, trio, 0);
        ticks(&mut a);
        let new = a.state.session.project.channels[0].clone();
        assert_ne!(new, old);
        assert_eq!(names(&a.state), [LAST_RESET, REPLACE_CHANNEL]);
        assert_eq!(name(&a), Some(crate::osc3::LEMON_TRIO.to_string()));
        for _ in 0..2 {
            ctrl_z(&mut a);
            ticks(&mut a);
            assert_eq!((&a.state.session.project.channels[0], name(&a)), (&old, old_name.clone()), "the old instrument back");
            ctrl_alt_z(&mut a);
            ticks(&mut a);
            assert_eq!((&a.state.session.project.channels[0], name(&a)), (&new, Some(crate::osc3::LEMON_TRIO.to_string())));
            assert_eq!(a.state.session.hosted_rows.iter().filter(|&&r| r == 0).count(), 1);
        }
    }

}
