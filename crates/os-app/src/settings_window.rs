// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The settings window's side of the declared settings (`crate::prefs`): a click on a check, a combo's menu and the
//! choice made in it, a slider pressed, dragged or wheeled.
// spec: ui/settings.md, ui/options-coverage.md

use crate::app::App;
use crate::prefs::*;

/// The grey, unclickable heading rows of combo `c`'s menu: (the choice each stands above, its text).
fn headings(c: &Combo) -> &'static [(usize, &'static str)] {
    if c.key == RESAMPLING_QUALITY.key {
        &[(0, "Low quality / fastest"), (2, "Average quality"), (5, "High quality / slowest")]
    } else {
        &[]
    }
}

/// What a declared control does when clicked (`views::settings` draws them).
impl App {
    /// A declared check was clicked (`SAct::Check`): toggles and keeps it.
    pub(crate) fn pref_toggle(&mut self, label: &str) {
        let Some(c) = find_check(label) else { return };
        let on = toggle(c);
        if c.key == MULTITHREADED_MIXER.key || c.key == MULTITHREADED_GENERATORS.key || c.key == MULTITHREADED_AUTO.key {
            crate::mix_threads::apply();
        }
        // Recording ▸ Start on input off ends a wait for input, as its toolbar button does.
        if c.key == REC_WAIT.key && !on {
            self.state.rec.waiting = false;
        }
        self.state.message = Some(format!("{}: {}", c.label, if on { "on" } else { "off" }));
        self.invalidate();
    }

    /// A declared combo was clicked (`SAct::Combo`): the menu of its choices under its box (`x`, `y`), the current one
    /// marked.
    pub(crate) fn pref_menu(&mut self, label: &str, x: f32, y: f32) {
        use crate::playlist::{Cmd, Mark, PItem, Popup};
        let Some(c) = find_combo(label) else { return };
        let now = index(c);
        // Grey headings stand above the choices they group (`headings`).
        let mut items = Vec::new();
        for (i, &ch) in c.choices.iter().enumerate() {
            items.extend(headings(c).iter().filter(|h| h.0 == i).map(|h| PItem::head(h.1)));
            items.push(PItem::new(ch, Cmd::Pref(c, i)).mark(Mark::Radio(i == now)));
        }
        self.state.playlist.popup = Some(Popup { x, y: y + 1.0, items, sub: None, scroll: [0; 2] });
        self.invalidate();
    }

    /// Entry `i` of a declared combo's menu was chosen (`Cmd::Pref`): kept; returns the message.
    pub(crate) fn pref_choose(&mut self, c: &'static Combo, i: usize) -> Option<String> {
        let band = crate::views::band_h(&self.state);
        set_choice(c, i);
        // Toolbars scaling: the workspace's top follows the band's new height, the floating windows with it (spec:
        // ui/ui-scaling.md §1).
        if c.key == TOOLBARS_SCALING.key && !self.state.toolbar.hidden {
            let dy = crate::views::band_h(&self.state) - band;
            crate::app::lift_windows(&mut self.state, dy);
        }
        self.invalidate();
        Some(format!("{}: {}", c.label, choice(c)))
    }
}

/// The settings window's side of the declared sliders.
impl App {
    /// A declared slider was pressed (`SAct::Slider`, its track `w` wide from `x`): set from the mouse, then dragged
    /// (`Drag::PrefSlider`).
    pub(crate) fn pref_slider_press(&mut self, label: &'static str, x: f32, w: f32) {
        self.pref_slider_drag(label, x, w);
        self.drag = Some(crate::app::Drag::PrefSlider { label, x, w });
    }

    /// Sets the declared slider labelled `label` from the mouse along its track (`w` wide from `x`).
    pub(crate) fn pref_slider_drag(&mut self, label: &str, x: f32, w: f32) {
        let Some(s) = find_slider(label) else { return };
        let at = ((self.mouse.0 - x) / w.max(1.0)).clamp(0.0, 1.0);
        set_slider(s, (at * f32::from(s.max)).round() as u16);
        self.invalidate();
    }

    /// Wheel notches over the declared slider labelled `label`: up (`notches` > 0) moves it by its step toward its
    /// `max`.
    pub(crate) fn pref_slider_wheel(&mut self, label: &str, notches: i32) {
        let Some(s) = find_slider(label) else { return };
        let v = i32::from(slider(s)) + notches * i32::from(s.step);
        set_slider(s, v.clamp(0, i32::from(s.max)) as u16);
        self.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings_file::Store;
    use crate::app::Action;
    use crate::views::settings::{self, Row, SAct};
    use os_ui::{Canvas, Rect};

    /// Every declaration has its own key and label, a default among its choices, and a control of its kind with that
    /// label on a settings page (checks: an enabled check; combos: a combo; value boxes: a value box). The combos of
    /// `OFF_PAGE_COMBOS` are on no page.
    #[test]
    fn declarations_match_the_settings_pages() {
        let app = App::new(None);
        let mut checks_shown = Vec::new();
        let mut combos_shown = Vec::new();
        let mut numbers_shown = Vec::new();
        fn walk(r: &Row, checks: &mut Vec<&'static str>, combos: &mut Vec<&'static str>, numbers: &mut Vec<&'static str>) {
            match r {
                Row::Check(k, _) => checks.push(*k),
                Row::Check2(a, _, b, _) => checks.extend([*a, *b]),
                Row::Combo(_, _, l) => combos.push(*l),
                Row::Value(_, l) | Row::ValueR(l, _) => numbers.push(*l),
                Row::Midi(m) => combos.extend(m.combo()),
                Row::Pair(a, b) => {
                    walk(a, checks, combos, numbers);
                    walk(b, checks, combos, numbers);
                }
                _ => {}
            }
        }
        for tab in 0..settings::TABS.len() {
            for g in settings::page(tab, &app.state) {
                for r in &g.rows {
                    walk(r, &mut checks_shown, &mut combos_shown, &mut numbers_shown);
                }
            }
        }
        let mut keys = Vec::new();
        let mut labels = Vec::new();
        for c in checks() {
            assert!(checks_shown.contains(&c.label), "{} is a check on a settings page", c.label);
            keys.push(c.key);
            labels.push(c.label);
        }
        for c in COMBOS {
            assert!(combos_shown.contains(&c.label), "{} is a combo on a settings page", c.label);
            assert!(c.default < c.choices.len(), "{}: default among its choices", c.label);
            keys.push(c.key);
            labels.push(c.label);
        }
        for c in OFF_PAGE_COMBOS {
            assert!(!combos_shown.contains(&c.label), "{} is not on a settings page", c.label);
            assert!(c.default < c.choices.len(), "{}: default among its choices", c.label);
            keys.push(c.key);
            labels.push(c.label);
        }
        for n in NUMBERS {
            assert!(numbers_shown.contains(&n.label), "{} is a value box on a settings page", n.label);
            assert!((n.min..=n.max).contains(&n.default), "{}: default in its range", n.label);
            keys.push(n.key);
            labels.push(n.label);
        }
        for (i, k) in keys.iter().enumerate() {
            assert!(!k.is_empty() && !k.contains('=') && k.trim() == *k, "key {k:?}");
            assert!(!keys[..i].contains(k), "key {k} declared twice");
            assert!(!labels[..i].contains(&labels[i]), "label {} declared twice (TEST_CHECK may need another label)", labels[i]);
        }
    }

    fn hit_rect(app: &App, a: &Action) -> Rect {
        app.hits.iter().rev().find(|h| &h.action == a).map(|h| h.rect).unwrap_or_else(|| panic!("no hit for {a:?}"))
    }

    fn click(app: &mut App, c: &mut Canvas, r: Rect) {
        app.mouse_move(r.cx(), r.cy());
        app.mouse_down();
        app.mouse_up();
        app.paint(c, None);
    }

    /// The page draws declared controls live: a check toggles and is kept, a combo lists its choices and shows the
    /// chosen one; undeclared controls stay greyed out.
    #[test]
    fn declared_controls_work_on_the_settings_page() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.open_settings(settings::GENERAL);
        app.paint(&mut c, None);
        // Main GUI scaling opens its list under its box, System marked; Language is not available yet.
        let combo = |app: &App| {
            app.hits.iter().rev().find(|h| matches!(&h.action, Action::Settings(SAct::Combo(l, ..)) if *l == MAIN_GUI_SCALING.label)).map(|h| h.rect).expect("the combo works")
        };
        assert!(app.hits.iter().any(|h| h.action == Action::Placeholder("Language")));
        let r = combo(&app);
        click(&mut app, &mut c, r);
        let p = app.state.playlist.popup.as_ref().expect("its menu");
        let labels: Vec<&str> = p.items.iter().map(|it| it.label.as_ref()).collect();
        assert_eq!(labels, MAIN_GUI_SCALING.choices);
        assert_eq!(p.items[0].mark, crate::playlist::Mark::Radio(true));
        assert!(p.items[1..].iter().all(|it| it.mark == crate::playlist::Mark::Radio(false)));
        assert!((p.y - r.bottom()).abs() < 2.0, "under its box");
        let item = hit_rect(&app, &Action::PlPopup(2));
        click(&mut app, &mut c, item);
        assert!(app.state.playlist.popup.is_none());
        assert_eq!(choice(&MAIN_GUI_SCALING), "125%");
        assert_eq!(app.state.message.as_deref(), Some("Main GUI scaling: 125%"));
        // The menu marks the new choice.
        let r = combo(&app);
        click(&mut app, &mut c, r);
        assert_eq!(app.state.playlist.popup.as_ref().unwrap().items[2].mark, crate::playlist::Mark::Radio(true));
        app.state.playlist.popup = None;
        // The test check (Advanced, further down the page: the groups above it folded) toggles.
        app.state.settings.folded.extend(["Display", "Language", "Input", "Undo history", "Hints and warnings", "System", "Updates", "Miscellaneous"]);
        app.paint(&mut c, None);
        let key = Action::Settings(SAct::Check(TEST_CHECK.label));
        let r = hit_rect(&app, &key);
        click(&mut app, &mut c, r);
        assert!(!check(&TEST_CHECK));
        assert_eq!(app.state.message.as_deref(), Some("Use the high-performance power plan: off"));
        let r = hit_rect(&app, &key);
        click(&mut app, &mut c, r);
        assert!(check(&TEST_CHECK));
        // The update checks above it are real settings (`crate::update`), not greyed out.
        app.state.settings.folded.retain(|g| *g != "Updates");
        app.paint(&mut c, None);
        let download = AUTO_DOWNLOAD_UPDATES.label;
        assert!(app.hits.iter().any(|h| h.action == Action::Settings(SAct::Check(download))));
        assert!(!app.hits.iter().any(|h| h.action == Action::Placeholder(download)));
        // Nothing about usage data.
        assert!(!app.hits.iter().any(|h| matches!(h.action, Action::Placeholder(l) if l.contains("usage"))));
    }

    /// Pop-ups scaling: menus and combo lists are painted at its scale, rows as ui-scaling.md §2 has them, from where they
    /// were opened; clicks find their entries. Main follows the window.
    #[test]
    fn pop_ups_scaling_scales_menus() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.open_settings(settings::GENERAL);
        app.paint(&mut c, None);
        let combo = |app: &App| {
            app.hits.iter().rev().find(|h| matches!(&h.action, Action::Settings(SAct::Combo(l, ..)) if *l == POPUPS_SCALING.label)).map(|h| h.rect).expect("the combo works")
        };
        let r = combo(&app);
        click(&mut app, &mut c, r);
        let main = hit_rect(&app, &Action::PlPopup(1));
        assert_eq!(main.h, 19.0, "Main at 100 %");
        let item = hit_rect(&app, &Action::PlPopup(5));
        click(&mut app, &mut c, item);
        assert_eq!((choice(&POPUPS_SCALING), app.state.message.as_deref()), ("200%", Some("Pop-ups scaling: 200%")));
        // At 200 % on a 100 % window: item rows 12k + 7 = 31 px, the panel's border 2 px, text twice as wide.
        let r = combo(&app);
        click(&mut app, &mut c, r);
        let p = app.state.playlist.popup.as_ref().expect("its menu");
        let (px, py) = (p.x, p.y);
        let (first, big) = (hit_rect(&app, &Action::PlPopup(0)), hit_rect(&app, &Action::PlPopup(1)));
        assert_eq!((big.h, first.y - py, first.x - px), (31.0, 2.0, 2.0));
        assert!((1.8..2.2).contains(&(big.w / main.w)), "{} -> {}", main.w, big.w);
        click(&mut app, &mut c, first);
        assert_eq!(choice(&POPUPS_SCALING), "Main", "the click finds the scaled entry");
        // Menu bar dropdowns too.
        set_choice(&POPUPS_SCALING, 3);
        app.state.settings.open = false;
        app.state.menu_open = Some(0);
        app.paint(&mut c, None);
        let save = app.hits.iter().rev().find(|h| h.action == crate::views::menus::pick(0, crate::views::menus::FILE_SAVE)).map(|h| h.rect).expect("FILE > Save");
        assert_eq!(save.h, 25.0, "150 %: 12k + 7");
    }

    /// "Small scrollbars in editors" (spec: ui/settings-choices.md "Miscellaneous"): the piano roll's and the
    /// playlist's scroll bars get about half as thick (the playlist's as FL measured: 9 px across, 10 px down) and
    /// lose their arrow buttons; the time rulers grow by the room freed above them, the grids by the room at the side.
    #[test]
    fn small_scrollbars_in_editors() {
        use crate::app::{Scroll, Win};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut seen = Vec::new();
        for on in [false, true] {
            set_check(&SMALL_SCROLLBARS, on);
            let mut app = App::new(None);
            let mut measure = |app: &mut App, win: Win| {
                app.state.z = vec![win];
                app.paint(&mut c, None);
                let bar = |t: Scroll| app.bars.iter().find(|b| b.target == t).map(|b| b.track).unwrap_or_else(|| panic!("{t:?} bar"));
                let arrows = app.hits.iter().filter(|h| matches!(h.action, Action::ScrollStep(..) | Action::PitchStep(_))).count();
                let ruler = |a: Action| hit_rect(app, &a);
                match win {
                    Win::PianoRoll => (bar(Scroll::PianoX).h, bar(Scroll::PianoY).w, arrows, ruler(Action::PianoRuler), app.piano_grid.expect("grid")),
                    _ => (bar(Scroll::PlaylistX).h, bar(Scroll::PlaylistY).w, arrows, ruler(Action::PlRuler), app.pl_grid().expect("grid")),
                }
            };
            seen.push((measure(&mut app, Win::PianoRoll), measure(&mut app, Win::Playlist)));
        }
        let ((piano, playlist), (piano_small, playlist_small)) = (seen[0], seen[1]);
        assert_eq!((piano.0, piano.1, playlist.0, playlist.1), (22.0, 20.0, 18.0, 18.0), "off: as before");
        assert_eq!((piano_small.0, piano_small.1, playlist_small.0, playlist_small.1), (11.0, 10.0, 9.0, 10.0), "on: thin");
        assert_eq!((piano.2 > 0, playlist.2), (true, 0), "arrow buttons: the piano roll's");
        assert_eq!((piano_small.2, playlist_small.2), (0, 0), "on: no arrow buttons");
        // The rulers end where they did and start higher; the grids keep their top and widen.
        for ((r0, g0), (r1, g1), freed_h, freed_w) in [((piano.3, piano.4), (piano_small.3, piano_small.4), 11.0, 10.0), ((playlist.3, playlist.4), (playlist_small.3, playlist_small.4), 9.0, 8.0)] {
            assert_eq!((r1.bottom(), r1.h), (r0.bottom(), r0.h + freed_h), "ruler");
            assert_eq!((g1.x, g1.y, g1.h, g1.w), (g0.x, g0.y, g0.h, g0.w + freed_w), "grid");
        }
        set_check(&SMALL_SCROLLBARS, false);
    }

    // spec: ui/settings-choices.md "Display: Colorful waveforms (2026-10-16)"
    /// "Colorful waveforms" is Off by default and found by its label; the General page's combo (Display) lists Off,
    /// Color map and Multiband and switches it.
    #[test]
    fn colorful_waveforms_setting() {
        assert_eq!((index(&COLORFUL_WAVEFORMS), choice(&COLORFUL_WAVEFORMS)), (0, "Off"));
        assert_eq!(find_combo("Colorful waveforms"), Some(&COLORFUL_WAVEFORMS));
        assert_eq!(COLORFUL_WAVEFORMS.choices, ["Off", "Color map", "Multiband"]);
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.open_settings(settings::GENERAL);
        app.paint(&mut c, None);
        let combo = |app: &App| {
            app.hits.iter().rev().find(|h| matches!(&h.action, Action::Settings(SAct::Combo(l, ..)) if *l == COLORFUL_WAVEFORMS.label)).map(|h| h.rect).expect("the combo works")
        };
        let r = combo(&app);
        click(&mut app, &mut c, r);
        let p = app.state.playlist.popup.as_ref().expect("its menu");
        let labels: Vec<&str> = p.items.iter().map(|it| it.label.as_ref()).collect();
        assert_eq!(labels, COLORFUL_WAVEFORMS.choices);
        assert_eq!(p.items[0].mark, crate::playlist::Mark::Radio(true), "Off marked");
        let item = hit_rect(&app, &Action::PlPopup(2));
        click(&mut app, &mut c, item);
        assert_eq!(choice(&COLORFUL_WAVEFORMS), "Multiband");
        assert_eq!(app.state.message.as_deref(), Some("Colorful waveforms: Multiband"));
        set_choice(&COLORFUL_WAVEFORMS, 0);
    }

    /// Every declared slider is a slider on a settings page, with a key and a label of its own among all the
    /// declarations, a default in range and a step.
    #[test]
    fn sliders_match_the_settings_pages() {
        let app = App::new(None);
        let mut shown = Vec::new();
        fn walk(r: &Row, shown: &mut Vec<&'static str>) {
            match r {
                Row::Slider(l, _) => shown.push(*l),
                Row::Pair(a, b) => {
                    walk(a, shown);
                    walk(b, shown);
                }
                _ => {}
            }
        }
        for tab in 0..settings::TABS.len() {
            for g in settings::page(tab, &app.state) {
                for r in &g.rows {
                    walk(r, &mut shown);
                }
            }
        }
        let mut keys: Vec<&str> = checks().map(|c| c.key).chain(COMBOS.iter().map(|c| c.key)).chain(NUMBERS.iter().map(|n| n.key)).collect();
        let mut labels: Vec<&str> = checks().map(|c| c.label).chain(COMBOS.iter().map(|c| c.label)).chain(NUMBERS.iter().map(|n| n.label)).collect();
        for s in SLIDERS {
            assert!(shown.contains(&s.label), "{} is a slider on a settings page", s.label);
            assert!(s.default <= s.max && s.step > 0, "{}: default and step", s.label);
            assert!(!s.key.is_empty() && !s.key.contains('=') && s.key.trim() == s.key, "key {:?}", s.key);
            assert!(!keys.contains(&s.key), "key {} declared twice", s.key);
            assert!(!s.label.is_empty() && !labels.contains(&s.label), "label {} declared twice", s.label);
            keys.push(s.key);
            labels.push(s.label);
        }
    }

    // spec: ui/settings-choices.md "Advanced: Fast sample preview, Auto keep long audio on disk, Threaded project
    // sample loading (2026-10-16)", "Advanced: Cursors scaling and Legacy scaling (2026-10-16)"
    /// General ▸ Advanced: the sample checks and the two scaling lists have the defaults as found and are kept in the
    /// settings file; on the page each is live: a click toggles a check, a combo lists its choices (Cursors scaling
    /// those of Main GUI scaling) and keeps the one picked.
    #[test]
    fn advanced_sample_and_scaling_settings() {
        let checks = |s: &Store| [&FAST_SAMPLE_PREVIEW, &AUTO_KEEP_ON_DISK, &THREADED_SAMPLE_LOADING].map(|c| check_in(s, c));
        let combos = |s: &Store| (index_in(s, &CURSORS_SCALING), index_in(s, &LEGACY_SCALING));
        let s = Store::parse("");
        assert_eq!((checks(&s), combos(&s)), ([true, true, false], (0, 0)), "as found");
        let s = Store::parse(
            "fast_sample_preview=0\nauto_keep_long_audio_on_disk=0\nthreaded_project_sample_loading=1\ncursors_scaling=250%\nlegacy_scaling=300%\n",
        );
        assert_eq!((checks(&s), combos(&s)), ([false, false, true], (6, 3)));
        assert_eq!(CURSORS_SCALING.choices, MAIN_GUI_SCALING.choices);
        assert_eq!(LEGACY_SCALING.choices, ["Main", "100%", "200%", "300%", "400%"]);

        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.open_settings(settings::GENERAL);
        app.state.settings.folded.extend(["Display", "Language", "Input", "Undo history", "Hints and warnings", "Miscellaneous"]);
        app.paint(&mut c, None);
        for k in [&FAST_SAMPLE_PREVIEW, &AUTO_KEEP_ON_DISK, &THREADED_SAMPLE_LOADING] {
            let was = check(k);
            let r = hit_rect(&app, &Action::Settings(SAct::Check(k.label)));
            click(&mut app, &mut c, r);
            assert_eq!(check(k), !was, "{} toggles", k.label);
            assert!(!app.hits.iter().any(|h| h.action == Action::Placeholder(k.label)), "{} is not greyed out", k.label);
        }
        for (k, pick) in [(&CURSORS_SCALING, 5), (&LEGACY_SCALING, 2)] {
            let combo = |app: &App| {
                app.hits.iter().rev().find(|h| matches!(&h.action, Action::Settings(SAct::Combo(l, ..)) if *l == k.label)).map(|h| h.rect)
            };
            let r = combo(&app).unwrap_or_else(|| panic!("{} works", k.label));
            click(&mut app, &mut c, r);
            let labels: Vec<String> = app.state.playlist.popup.as_ref().expect("its menu").items.iter().map(|it| it.label.to_string()).collect();
            assert_eq!(labels, k.choices, "{}", k.label);
            let item = hit_rect(&app, &Action::PlPopup(pick));
            click(&mut app, &mut c, item);
            assert_eq!(index(k), pick, "{}", k.label);
        }
        for k in [&FAST_SAMPLE_PREVIEW, &AUTO_KEEP_ON_DISK, &THREADED_SAMPLE_LOADING] {
            set_check(k, k.default);
        }
        set_choice(&CURSORS_SCALING, 0);
        set_choice(&LEGACY_SCALING, 0);
    }

    // spec: ui/settings-choices.md "Display: PPI ruler, Force refreshes, Thick lines, High visibility (2026-10-16)",
    // "Input: Optimize for (2026-10-16)"
    /// Force refreshes (on), Thick lines (off) and Optimize for (Multi-button mouse, of three) have their defaults and
    /// are kept in the settings file; on the General page Force refreshes toggles and Optimize for lists its choices
    /// and keeps the one picked.
    #[test]
    fn display_and_input_settings() {
        let s = Store::parse("");
        assert_eq!((check_in(&s, &FORCE_REFRESHES), check_in(&s, &THICK_LINES), index_in(&s, &OPTIMIZE_FOR)), (true, false, 0));
        let s = Store::parse("force_refreshes=0\nthick_lines=1\noptimize_for=Multi-touch monitor\n");
        assert_eq!((check_in(&s, &FORCE_REFRESHES), check_in(&s, &THICK_LINES), index_in(&s, &OPTIMIZE_FOR)), (false, true, 2));
        assert_eq!(OPTIMIZE_FOR.choices, ["Multi-button mouse", "Pencil", "Multi-touch monitor"]);
        assert_eq!((find_check("Force refreshes"), find_check("Thick lines")), (Some(&FORCE_REFRESHES), Some(&THICK_LINES)));
        assert_eq!(find_combo("Optimize for"), Some(&OPTIMIZE_FOR));

        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.open_settings(settings::GENERAL);
        app.paint(&mut c, None);
        let r = hit_rect(&app, &Action::Settings(SAct::Check(FORCE_REFRESHES.label)));
        assert!(!app.hits.iter().any(|h| h.action == Action::Placeholder(FORCE_REFRESHES.label)), "not greyed out");
        click(&mut app, &mut c, r);
        assert!(!check(&FORCE_REFRESHES));
        assert_eq!(app.state.message.as_deref(), Some("Force refreshes: off"));
        let combo = |app: &App| {
            app.hits.iter().rev().find(|h| matches!(&h.action, Action::Settings(SAct::Combo(l, ..)) if *l == OPTIMIZE_FOR.label)).map(|h| h.rect).expect("the combo works")
        };
        let r = combo(&app);
        click(&mut app, &mut c, r);
        let labels: Vec<String> = app.state.playlist.popup.as_ref().expect("its menu").items.iter().map(|it| it.label.to_string()).collect();
        assert_eq!(labels, OPTIMIZE_FOR.choices);
        let item = hit_rect(&app, &Action::PlPopup(1));
        click(&mut app, &mut c, item);
        assert_eq!((choice(&OPTIMIZE_FOR), app.state.message.as_deref()), ("Pencil", Some("Optimize for: Pencil")));
        set_check(&FORCE_REFRESHES, true);
        set_choice(&OPTIMIZE_FOR, 0);
    }

    // spec: ui/settings-choices.md "Display: Smoothing, Animation refresh rate, Transparent menus and windows
    // (2026-10-16)"
    /// Transparent menus (a check, on) and Transparent windows (Off or On, Off) have the defaults as found and are kept
    /// in the settings file; on the General page both work: the check toggles, the combo lists its two choices and keeps
    /// the one picked.
    #[test]
    fn transparency_settings() {
        let s = Store::parse("");
        assert_eq!((check_in(&s, &TRANSPARENT_MENUS), index_in(&s, &TRANSPARENT_WINDOWS)), (true, 0));
        let s = Store::parse("transparent_menus=0\ntransparent_windows=On\n");
        assert_eq!((check_in(&s, &TRANSPARENT_MENUS), index_in(&s, &TRANSPARENT_WINDOWS)), (false, 1));
        assert_eq!(TRANSPARENT_WINDOWS.choices, ["Off", "On"]);
        assert_eq!((find_check("Transparent menus"), find_combo("Transparent windows")), (Some(&TRANSPARENT_MENUS), Some(&TRANSPARENT_WINDOWS)));

        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.open_settings(settings::GENERAL);
        app.paint(&mut c, None);
        assert!(!app.hits.iter().any(|h| h.action == Action::Placeholder(TRANSPARENT_MENUS.label)), "not greyed out");
        let r = hit_rect(&app, &Action::Settings(SAct::Check(TRANSPARENT_MENUS.label)));
        click(&mut app, &mut c, r);
        assert!(!check(&TRANSPARENT_MENUS));
        assert_eq!(app.state.message.as_deref(), Some("Transparent menus: off"));
        let r = app
            .hits
            .iter()
            .rev()
            .find(|h| matches!(&h.action, Action::Settings(SAct::Combo(l, ..)) if *l == TRANSPARENT_WINDOWS.label))
            .map(|h| h.rect)
            .expect("the combo works");
        click(&mut app, &mut c, r);
        let labels: Vec<String> = app.state.playlist.popup.as_ref().expect("its menu").items.iter().map(|it| it.label.to_string()).collect();
        assert_eq!(labels, TRANSPARENT_WINDOWS.choices);
        let item = hit_rect(&app, &Action::PlPopup(1));
        click(&mut app, &mut c, item);
        assert_eq!((choice(&TRANSPARENT_WINDOWS), app.state.message.as_deref()), ("On", Some("Transparent windows: On")));
        set_check(&TRANSPARENT_MENUS, true);
        set_choice(&TRANSPARENT_WINDOWS, 0);
    }

    // spec: ui/settings-choices.md "Display: PPI ruler, Force refreshes, Thick lines, High visibility (2026-10-16)"
    /// Thick lines is greyed out while the effective UI scale is under 200 % (a click changes nothing, the hint says
    /// why) and works from 200 %, the Main GUI scaling chosen or the display's.
    #[test]
    fn thick_lines_from_200_percent() {
        assert!(!settings::thick_lines_live(1.0) && !settings::thick_lines_live(1.75) && !settings::thick_lines_live(1.994));
        assert!(settings::thick_lines_live(2.0) && settings::thick_lines_live(2.5) && settings::thick_lines_live(4.0));
        let key = Action::Settings(SAct::Check(THICK_LINES.label));
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.open_settings(settings::GENERAL);
        app.paint(&mut c, None);
        assert!(!app.hits.iter().any(|h| h.action == key), "greyed out at 100 %");
        let greyed = app.hits.iter().rev().find(|h| h.hint == "Thick lines: from 200 % scaling").map(|h| h.rect).expect("its hint");
        click(&mut app, &mut c, greyed);
        assert!(!check(&THICK_LINES), "a click changes nothing");
        let Ok(mut c) = Canvas::new(3840, 2160, 2.0) else { return };
        app.invalidate();
        app.paint(&mut c, None);
        let r = hit_rect(&app, &key);
        click(&mut app, &mut c, r);
        assert!(check(&THICK_LINES), "live at 200 %");
        assert_eq!(app.state.message.as_deref(), Some("Thick lines: on"));
        set_check(&THICK_LINES, false);
    }

    // spec: ui/settings-choices.md "Input / output: Auto close; CPU and Mixer: Smart disable, Align tick lengths,
    // Resampling quality (2026-10-16)"
    /// Auto close (off), Smart disable (on), Align tick lengths (off) and Resampling quality (24-point sinc, of nine,
    /// under three grey headings) have the defaults as found and are kept in the settings file; on the Audio page
    /// each is live: a click toggles a check, the combo lists its choices and keeps the one picked.
    #[test]
    fn audio_cpu_and_mixer_settings() {
        let checks = |s: &Store| [&AUTO_CLOSE, &SMART_DISABLE, &ALIGN_TICK_LENGTHS].map(|c| check_in(s, c));
        let s = Store::parse("");
        assert_eq!((checks(&s), index_in(&s, &RESAMPLING_QUALITY)), ([false, true, false], 3), "as found");
        let s = Store::parse("auto_close=1\nsmart_disable=0\nalign_tick_lengths=1\nresampling_quality=512-point sinc\n");
        assert_eq!((checks(&s), index_in(&s, &RESAMPLING_QUALITY)), ([true, false, true], 8));
        assert_eq!(
            RESAMPLING_QUALITY.choices,
            ["2-point linear", "6-point Hermite", "16-point sinc", "24-point sinc", "32-point sinc", "64-point sinc", "128-point sinc", "256-point sinc", "512-point sinc"]
        );
        assert_eq!(choice(&RESAMPLING_QUALITY), "24-point sinc");

        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.open_settings(settings::AUDIO);
        app.paint(&mut c, None);
        for k in [&AUTO_CLOSE, &SMART_DISABLE, &ALIGN_TICK_LENGTHS] {
            let was = check(k);
            let r = hit_rect(&app, &Action::Settings(SAct::Check(k.label)));
            click(&mut app, &mut c, r);
            assert_eq!(check(k), !was, "{} toggles", k.label);
            assert!(!app.hits.iter().any(|h| h.action == Action::Placeholder(k.label)), "{} is not greyed out", k.label);
        }
        let combo = |app: &App| {
            app.hits.iter().rev().find(|h| matches!(&h.action, Action::Settings(SAct::Combo(l, ..)) if *l == RESAMPLING_QUALITY.label)).map(|h| h.rect).expect("the combo works")
        };
        let r = combo(&app);
        click(&mut app, &mut c, r);
        let items = &app.state.playlist.popup.as_ref().expect("its menu").items;
        let rows: Vec<(String, bool)> = items.iter().map(|it| (it.label.to_string(), it.head)).collect();
        let mut want: Vec<(String, bool)> = RESAMPLING_QUALITY.choices.iter().map(|ch| (ch.to_string(), false)).collect();
        want.insert(5, ("High quality / slowest".into(), true));
        want.insert(2, ("Average quality".into(), true));
        want.insert(0, ("Low quality / fastest".into(), true));
        assert_eq!(rows, want);
        assert_eq!(items[5].mark, crate::playlist::Mark::Radio(true), "24-point sinc marked");
        let item = hit_rect(&app, &Action::PlPopup(11));
        click(&mut app, &mut c, item);
        assert_eq!((choice(&RESAMPLING_QUALITY), app.state.message.as_deref()), ("512-point sinc", Some("Resampling quality: 512-point sinc")));
        for k in [&AUTO_CLOSE, &SMART_DISABLE, &ALIGN_TICK_LENGTHS] {
            set_check(k, k.default);
        }
        set_choice(&RESAMPLING_QUALITY, 3);
    }

    // spec: ui/settings-choices.md "Input: Vertical and Horizontal scrolling speed (2026-10-16)"
    /// General ▸ Input's scrolling speeds are live sliders: a click sets one from where it lands on the track, a drag
    /// follows the mouse, the wheel moves it by 32 a notch; the handle shows the value.
    #[test]
    fn scrolling_speed_sliders_work() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.open_settings(settings::GENERAL);
        app.state.settings.folded.extend(["Display", "Language"]);
        app.paint(&mut c, None);
        let track = |app: &App, s: &Slider| {
            app.hits
                .iter()
                .rev()
                .find_map(|h| match h.action {
                    Action::Settings(SAct::Slider(l, x, w)) if l == s.label => Some((h.rect, x, w)),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{} works", s.label))
        };
        for s in SLIDERS {
            assert!(!app.hits.iter().any(|h| h.action == Action::Placeholder(s.label)), "{} is not greyed out", s.label);
        }
        let (r, x, w) = track(&app, &VERTICAL_SCROLLING_SPEED);
        // A click three quarters along: 384; a drag past the left end: 0.
        app.mouse_move(x + w * 0.75, r.cy());
        app.mouse_down();
        assert_eq!(slider(&VERTICAL_SCROLLING_SPEED), 384);
        app.mouse_move(x - 40.0, r.cy() + 30.0);
        assert_eq!(slider(&VERTICAL_SCROLLING_SPEED), 0);
        app.mouse_move(x + w * 0.25, r.cy() + 30.0);
        app.mouse_up();
        assert_eq!(slider(&VERTICAL_SCROLLING_SPEED), 128);
        app.mouse_move(x + w + 40.0, r.cy());
        assert_eq!(slider(&VERTICAL_SCROLLING_SPEED), 128, "released: the mouse no longer moves it");
        assert_eq!(slider(&HORIZONTAL_SCROLLING_SPEED), 256, "the other one stays");
        app.paint(&mut c, None);
        // The wheel: 32 a notch, kept in 0..=512.
        let (r, _, _) = track(&app, &HORIZONTAL_SCROLLING_SPEED);
        app.mouse_move(r.cx(), r.cy());
        let scroll = app.state.settings.scroll;
        app.wheel(0.0, 1.0, false, false);
        assert_eq!(slider(&HORIZONTAL_SCROLLING_SPEED), 288);
        app.wheel(0.0, -1.0, false, false);
        app.wheel(0.0, -1.0, false, false);
        assert_eq!(slider(&HORIZONTAL_SCROLLING_SPEED), 224);
        assert_eq!(app.state.settings.scroll, scroll, "the page does not scroll");
        set_slider(&HORIZONTAL_SCROLLING_SPEED, 500);
        app.wheel(0.0, 1.0, false, false);
        assert_eq!(slider(&HORIZONTAL_SCROLLING_SPEED), 512);
        set_slider(&HORIZONTAL_SCROLLING_SPEED, 10);
        app.wheel(0.0, -1.0, false, false);
        assert_eq!(slider(&HORIZONTAL_SCROLLING_SPEED), 0);
        // The handle is drawn where the value is: the canvas changes with it.
        set_slider(&HORIZONTAL_SCROLLING_SPEED, 0);
        app.invalidate();
        app.paint(&mut c, None);
        let at0 = shot(&c);
        set_slider(&HORIZONTAL_SCROLLING_SPEED, 512);
        app.invalidate();
        app.paint(&mut c, None);
        assert_ne!(shot(&c), at0, "the handle moved");
        // A middle click, and Reset in the right-click menu, put a slider back to its default.
        let (r, _, _) = track(&app, &VERTICAL_SCROLLING_SPEED);
        app.mouse_move(r.cx(), r.cy());
        set_slider(&VERTICAL_SCROLLING_SPEED, 32);
        app.mouse_down_middle();
        assert_eq!(slider(&VERTICAL_SCROLLING_SPEED), VERTICAL_SCROLLING_SPEED.default, "middle click");
        set_slider(&VERTICAL_SCROLLING_SPEED, 32);
        app.mouse_down_right();
        app.mouse_up();
        let menu = app.state.playlist.popup.clone().expect("the slider's menu");
        assert_eq!(menu.items.iter().map(|i| i.label.to_string()).collect::<Vec<_>>(), ["Reset"]);
        app.act(Action::PlPopup(0));
        assert_eq!(slider(&VERTICAL_SCROLLING_SPEED), VERTICAL_SCROLLING_SPEED.default, "Reset");
        set_slider(&VERTICAL_SCROLLING_SPEED, 256);
        set_slider(&HORIZONTAL_SCROLLING_SPEED, 256);
    }

    // spec: ui/settings-choices.md "Mixer: Browser preview track (2026-10-16)"
    /// Audio ▸ Mixer's Browser preview track box shows "---" or a track; the wheel steps it from "---" through the
    /// open project's insert tracks, one a notch, and a click says so.
    #[test]
    fn browser_preview_track_box() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.open_settings(settings::AUDIO);
        app.paint(&mut c, None);
        let shown = |app: &App| {
            let rows = settings::page(settings::AUDIO, &app.state).into_iter().flat_map(|g| g.rows);
            rows.into_iter().find_map(|r| match r {
                Row::Value(v, l) if l == BROWSER_PREVIEW_TRACK.label => Some(v),
                _ => None,
            })
        };
        assert_eq!(shown(&app).as_deref(), Some("---"), "the box shows --- for none");
        let key = Action::Settings(SAct::Number(BROWSER_PREVIEW_TRACK.label));
        assert!(!app.hits.iter().any(|h| h.action == Action::Placeholder(BROWSER_PREVIEW_TRACK.label)));
        let r = hit_rect(&app, &key);
        click(&mut app, &mut c, r);
        assert_eq!(app.state.message.as_deref(), Some("Browser preview track: the mouse wheel changes it"));
        app.mouse_move(r.cx(), r.cy());
        app.wheel(0.0, 1.0, false, false);
        assert_eq!(number(&BROWSER_PREVIEW_TRACK), 1);
        assert_eq!(app.state.message.as_deref(), Some("Browser preview track: 1"));
        app.wheel(0.0, 2.0, false, false);
        assert_eq!(number(&BROWSER_PREVIEW_TRACK), 2, "one a notch");
        app.wheel(0.0, -1.0, false, false);
        app.wheel(0.0, -1.0, false, false);
        app.wheel(0.0, -1.0, false, false);
        assert_eq!((number(&BROWSER_PREVIEW_TRACK), app.state.message.as_deref()), (0, Some("Browser preview track: ---")));
        // Up to the open project's last insert track.
        let inserts = crate::mixer::inserts(&app.state.session);
        assert!(inserts > 0);
        set_number(&BROWSER_PREVIEW_TRACK, inserts);
        app.wheel(0.0, 1.0, false, false);
        assert_eq!(number(&BROWSER_PREVIEW_TRACK), inserts);
        // A track another project had comes back in range.
        set_number(&BROWSER_PREVIEW_TRACK, inserts + 10);
        app.wheel(0.0, -1.0, false, false);
        assert_eq!(number(&BROWSER_PREVIEW_TRACK), inserts);
        // The box shows the value.
        set_number(&BROWSER_PREVIEW_TRACK, 0);
        app.invalidate();
        app.paint(&mut c, None);
        let before = shot(&c);
        set_number(&BROWSER_PREVIEW_TRACK, 7);
        app.invalidate();
        app.paint(&mut c, None);
        assert_ne!(shot(&c), before, "the box shows 7");
        assert_eq!(shown(&app).as_deref(), Some("7"));
        set_number(&BROWSER_PREVIEW_TRACK, 0);
    }

    // spec: ui/metronome.md §4
    /// Audio ▸ Mixer's Metronome track box steps from "---" through the open project's insert tracks like Browser
    /// preview track, and the metronome's switch (shared with the playing engines) follows it.
    #[test]
    fn metronome_track_box() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.open_settings(settings::AUDIO);
        app.paint(&mut c, None);
        let r = hit_rect(&app, &Action::Settings(SAct::Number(METRONOME_TRACK.label)));
        assert!(!app.hits.iter().any(|h| h.action == Action::Placeholder(METRONOME_TRACK.label)));
        assert_eq!((number(&METRONOME_TRACK), crate::metronome::switch().track()), (0, 0), "the master by default");
        app.mouse_move(r.cx(), r.cy());
        app.wheel(0.0, 1.0, false, false);
        assert_eq!((number(&METRONOME_TRACK), crate::metronome::switch().track()), (1, 1));
        assert_eq!(app.state.message.as_deref(), Some("Metronome track: 1"));
        let inserts = crate::mixer::inserts(&app.state.session);
        set_number(&METRONOME_TRACK, inserts);
        app.wheel(0.0, 1.0, false, false);
        assert_eq!(number(&METRONOME_TRACK), inserts, "up to the last insert track");
        set_number(&METRONOME_TRACK, 1);
        app.wheel(0.0, -1.0, false, false);
        assert_eq!((number(&METRONOME_TRACK), crate::metronome::switch().track()), (0, 0));
        assert_eq!(app.state.message.as_deref(), Some("Metronome track: ---"));
    }

    fn shot(c: &Canvas) -> Vec<u32> {
        let mut v = vec![0u32; 1920 * 1080];
        c.copy_to_xrgb(&mut v);
        v
    }

    /// "Swap mouse buttons for drawing" (General ▸ Input; spec: ui/settings-choices.md "Input: Optimize for"): off by default,
    /// ticked and unticked on the page, kept for the next start; nothing reads it yet (pens arrive as the mouse).
    #[test]
    fn invert_pencil_buttons_is_stored_and_shown() {
        assert!(!check(&INVERT_PENCIL_BUTTONS));
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let key = Action::Settings(SAct::Check(INVERT_PENCIL_BUTTONS.label));
        let mut app = App::new(None);
        app.open_settings(settings::GENERAL);
        app.state.settings.folded.extend(["Display", "Language"]);
        app.paint(&mut c, None);
        let r = hit_rect(&app, &key);
        click(&mut app, &mut c, r);
        assert!(check(&INVERT_PENCIL_BUTTONS));
        assert_eq!(app.state.message.as_deref(), Some("Swap mouse buttons for drawing: on"));
        // Kept: a new start shows it ticked; a click unticks it.
        let mut app = App::new(None);
        app.open_settings(settings::GENERAL);
        app.state.settings.folded.extend(["Display", "Language"]);
        app.paint(&mut c, None);
        assert!(check(&INVERT_PENCIL_BUTTONS));
        let r = hit_rect(&app, &key);
        click(&mut app, &mut c, r);
        assert!(!check(&INVERT_PENCIL_BUTTONS));
        assert_eq!(app.state.message.as_deref(), Some("Swap mouse buttons for drawing: off"));
    }

    /// Settings ▸ Recording (spec: ui/settings.md "Recording"): every control works, and each is the setting the Record
    /// button's menus, the toolbar's option buttons and the mixer's Disk recording menu set, so a change from either
    /// side shows on the other and takes effect.
    #[test]
    fn recording_page_shares_the_record_settings() {
        use crate::record::Opt;
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.open_settings(settings::RECORDING);
        app.paint(&mut c, None);
        assert_eq!(settings::TABS[settings::RECORDING].1, "Recording options");
        // Nothing on the page is greyed out.
        let win = settings::rect(&app.state);
        let inside = |r: Rect| r.x >= win.x && r.right() <= win.right() && r.y >= win.y && r.bottom() <= win.bottom();
        let greyed: Vec<&str> = app.hits.iter().filter(|h| inside(h.rect)).filter_map(|h| match h.action { Action::Placeholder(p) => Some(p), _ => None }).collect();
        assert!(greyed.is_empty(), "greyed: {greyed:?}");
        // Eleven tabs still fit: About ends inside the window.
        let about = hit_rect(&app, &Action::Settings(SAct::Tab(settings::TABS.len() - 1)));
        assert!(about.right() + 7.0 <= win.right(), "{about:?} in {win:?}");
        let opts = [
            Opt::Notes,
            Opt::Automation,
            Opt::Audio,
            Opt::Ask,
            Opt::StartsPlayback,
            Opt::DisarmOnStop,
            Opt::Loop,
            Opt::Blend,
            Opt::QuantizeStart,
            Opt::QuantizeEnd,
            Opt::Wait,
            Opt::WaitFilter,
            Opt::WaitArmed,
            Opt::Precount,
        ];
        for o in opts {
            let label = o.check().label;
            let was = o.on();
            let r = hit_rect(&app, &Action::Settings(SAct::Check(label)));
            click(&mut app, &mut c, r);
            assert_eq!(o.on(), !was, "{label}: the page's click changes what recording reads");
            assert_eq!(app.state.message.as_deref(), Some(format!("{label}: {}", if was { "off" } else { "on" }).as_str()));
            o.toggle();
            assert_eq!(check(o.check()), was, "{label}: the menus' toggle is the page's check");
        }
        // Start on input off from the page ends a wait for input, as its button does.
        Opt::Wait.toggle();
        app.state.rec.waiting = true;
        let r = hit_rect(&app, &Action::Settings(SAct::Check(REC_WAIT.label)));
        click(&mut app, &mut c, r);
        assert!(!Opt::Wait.on() && !app.state.rec.waiting);
        // Loop record from the page reaches the playback at the next tick: armed with it off, playback runs on.
        app.state.rec.armed = true;
        app.record_tick();
        assert!(crate::transport::run_on_flag().load(std::sync::atomic::Ordering::Relaxed));
        let r = hit_rect(&app, &Action::Settings(SAct::Check(REC_LOOP.label)));
        click(&mut app, &mut c, r);
        app.record_tick();
        assert!(!crate::transport::run_on_flag().load(std::sync::atomic::Ordering::Relaxed));
        set_check(&REC_LOOP, false);
        app.state.rec.armed = false;
        // Count-in length: the page's combo is the count-in button's right-click choice.
        let combo = |app: &App, l: &str| app.hits.iter().rev().find(|h| matches!(&h.action, Action::Settings(SAct::Combo(k, ..)) if *k == l)).map(|h| h.rect).expect("the combo works");
        let r = combo(&app, COUNT_IN_LENGTH.label);
        click(&mut app, &mut c, r);
        let labels: Vec<String> = app.state.playlist.popup.as_ref().expect("its menu").items.iter().map(|it| it.label.to_string()).collect();
        assert_eq!(labels, ["1 bar", "2 bars"]);
        let item = hit_rect(&app, &Action::PlPopup(1));
        click(&mut app, &mut c, item);
        assert_eq!(crate::record::precount_bars(), 2);
        crate::record::set_precount_bars(1);
        assert_eq!(choice(&COUNT_IN_LENGTH), "1 bar");
        // Auto-create audio clips is the mixer menu's.
        let r = hit_rect(&app, &Action::Settings(SAct::Check(AUTO_AUDIO_CLIPS.label)));
        click(&mut app, &mut c, r);
        assert!(!check(&AUTO_AUDIO_CLIPS));
        set_check(&AUTO_AUDIO_CLIPS, true);
        // The External input question's answer: chosen here, the mixer stops asking (`App::ask_external`).
        let r = combo(&app, EXTERNAL_INPUT.label);
        click(&mut app, &mut c, r);
        let item = hit_rect(&app, &Action::PlPopup(2));
        click(&mut app, &mut c, item);
        assert_eq!(choice(&EXTERNAL_INPUT), "Keep the record source");
        app.ask_external(1);
        assert!(app.state.dialogs.is_empty(), "remembered: not asked");
        set_choice(&EXTERNAL_INPUT, 0);
        app.ask_external(1);
        assert!(!app.state.dialogs.is_empty(), "Ask asks");
    }
}
