// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The tempo display's right-click menu: in a first column the groups every control's menu has (Automation, Remote
//! control, Value: Copy value, Paste value, Enter value..., Set...), in a second one Tap tempo (the Tempo tapper),
//! tempos from 80 to 160 BPM and Half speed. Half speed is a playback setting, not part of the project: it halves the
//! speed the song plays at and leaves the tempo as it is.
// spec: ui/tempo-menu.md

use crate::app::{App, State, Win};
use crate::playlist::{Cmd, Mark, PItem, Popup};
use crate::typein::Target;
use crate::typein_ui as typein;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// The tempos of the second column, in BPM.
pub const PRESETS: [u16; 9] = [80, 90, 100, 110, 120, 130, 140, 150, 160];

/// The menu's own commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TCmd {
    /// Open or close the Tempo tapper (as Ctrl+F5).
    Tapper,
    /// Set the tempo to this many BPM.
    Preset(u16),
    /// Switch Half speed.
    HalfSpeed,
}

/// Right click on the tempo display. `auto`: its automation target, clip name and value (`App::automation_target`).
pub fn menu(s: &State, auto: Option<(u32, String, f64)>, x: f32, y: f32) -> Popup {
    let mut items = crate::rack::value_groups(s, None, Some(Target::Tempo), auto);
    items.extend([
        PItem::col(),
        PItem::new("Tap tempo", Cmd::Tempo(TCmd::Tapper)).key("Ctrl+F5").mark(Mark::Check(s.shown(Win::Tapper))),
        PItem::sep(),
    ]);
    items.extend(PRESETS.map(|b| PItem::new(format!("{b} BPM"), Cmd::Tempo(TCmd::Preset(b)))));
    items.extend([PItem::sep(), PItem::new("Half speed", Cmd::Tempo(TCmd::HalfSpeed)).mark(Mark::Check(half_speed()))]);
    Popup { x, y, items, sub: None, scroll: [0; 2] }
}

/// Runs a command of the menu.
pub fn run(app: &mut App, c: TCmd) -> Option<String> {
    let msg = match c {
        TCmd::Tapper => {
            app.toggle_tapper();
            None
        }
        TCmd::Preset(b) => {
            typein::set_tempo(&mut app.state, f64::from(b));
            Some(format!("Tempo {:.3} BPM", app.state.tempo()))
        }
        TCmd::HalfSpeed => {
            let on = !half_speed();
            half_flag().store(on, Ordering::Relaxed);
            Some(if on { "Half speed on: the song plays at half its tempo" } else { "Half speed off" }.into())
        }
    };
    app.invalidate();
    msg
}

/// Half speed: while it is set, every playback engine runs at half the tempo (`crate::transport::half_flag`). It lasts
/// until switched off or the program ends; projects do not keep it.
pub fn half_flag() -> Arc<AtomicBool> {
    crate::transport::half_flag()
}

/// Whether Half speed is on.
pub fn half_speed() -> bool {
    half_flag().load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::command::TEMPO;
    use crate::playlist::run as pl_run;
    use crate::rack::RCmd;

    fn labels(p: &Popup) -> Vec<String> {
        p.items.iter().map(|i| if i.col { "|".to_string() } else { i.label.to_string() }).collect()
    }

    fn item<'a>(p: &'a Popup, label: &str) -> &'a PItem {
        p.items.iter().find(|i| i.label == label).unwrap_or_else(|| panic!("no {label}"))
    }

    // spec: ui/tempo-menu.md §1
    #[test]
    fn the_menu_has_two_columns() {
        let app = App::new(None);
        let m = menu(&app.state, app.automation_target(&crate::app::Action::Tempo), 0.0, 0.0);
        let l = labels(&m);
        let col = l.iter().position(|x| x == "|").expect("a second column");
        for left in ["Create automation clip", "MIDI learn...", "Copy value", "Paste value", "Enter value...", "Set..."] {
            assert!(l[..col].iter().any(|x| x == left), "{left} in {l:?}");
        }
        let right: Vec<&str> = l[col + 1..].iter().map(String::as_str).filter(|x| !x.is_empty()).collect();
        assert_eq!(right, ["Tap tempo", "80 BPM", "90 BPM", "100 BPM", "110 BPM", "120 BPM", "130 BPM", "140 BPM", "150 BPM", "160 BPM", "Half speed"]);
        assert_eq!(item(&m, "Tap tempo").key, "Ctrl+F5");
        assert!(!item(&m, "Paste value").enabled, "nothing copied yet");
    }

    // spec: ui/tempo-menu.md §2
    #[test]
    fn a_preset_sets_the_tempo_in_one_step() {
        let mut app = App::new(None);
        app.state.set_tempo(97.25);
        let steps = app.state.session.history.at();
        pl_run(&mut app, Cmd::Tempo(TCmd::Preset(120)));
        assert_eq!((app.state.tempo(), app.state.session.history.at()), (120.0, steps + 1));
        // The same tempo again: no step.
        pl_run(&mut app, Cmd::Tempo(TCmd::Preset(120)));
        assert_eq!(app.state.session.history.at(), steps + 1);
        assert_eq!(crate::history_ui::undo(&mut app.state).as_deref(), Some(TEMPO));
        assert_eq!(app.state.tempo(), 97.25);
    }

    // spec: ui/tempo-menu.md §3
    #[test]
    fn copy_and_paste_value_carry_the_tempo() {
        let mut app = App::new(None);
        app.state.set_tempo(97.5);
        pl_run(&mut app, Cmd::Rack(RCmd::CopyBox(Target::Tempo)));
        let m = menu(&app.state, None, 0.0, 0.0);
        let paste = m.items.iter().find(|i| i.label.starts_with("Paste value")).unwrap();
        assert_eq!((paste.label.as_ref(), paste.enabled), ("Paste value (17%)", true), "(97.5 - 10) / 512");
        pl_run(&mut app, Cmd::Tempo(TCmd::Preset(140)));
        pl_run(&mut app, Cmd::Rack(RCmd::PasteBox(Target::Tempo)));
        assert_eq!(app.state.tempo(), 97.5);
    }

    // spec: ui/tempo-menu.md §3
    #[test]
    fn the_value_box_takes_bpm() {
        use crate::typein_ui::{accept, caption, parse, text};
        let mut app = App::new(None);
        app.state.set_tempo(160.0);
        assert_eq!((caption(&app.state, Target::Tempo), text(&app.state, Target::Tempo, false)), ("Tempo (BPM)".to_string(), "160".to_string()));
        assert_eq!(parse(Target::Tempo, false, "97.5"), Some(97.5));
        assert_eq!(parse(Target::Tempo, false, "128 bpm"), Some(128.0));
        assert_eq!(parse(Target::Tempo, false, "999"), Some(522.0));
        assert_eq!(parse(Target::Tempo, false, "50%"), None);
        assert_eq!(parse(Target::Tempo, false, "fast"), None);
        let steps = app.state.session.history.at();
        accept(&mut app.state, Target::Tempo, false, "97.5");
        assert_eq!((app.state.tempo(), app.state.session.history.at()), (97.5, steps + 1));
        assert_eq!(text(&app.state, Target::Tempo, false), "97.5");
    }

    // spec: ui/tempo-menu.md §3 "Set..."
    #[test]
    fn set_offers_the_default_and_the_range() {
        let mut app = App::new(None);
        app.state.set_tempo(10.0);
        let m = menu(&app.state, None, 0.0, 0.0);
        let set: Vec<(String, Mark)> = item(&m, "Set...").children.iter().filter(|i| !i.label.is_empty()).map(|i| (i.label.to_string(), i.mark)).collect();
        assert_eq!(set[0].0, "Default (130.000 BPM)");
        assert_eq!((set[1].0.as_str(), set[1].1), ("Minimum (10.000 BPM)", Mark::Radio(true)));
        assert_eq!(set[3].0, "50% (266.000 BPM)");
        assert_eq!(set.last().unwrap().0, "Maximum (522.000 BPM)");
        pl_run(&mut app, Cmd::Rack(RCmd::SetBox(Target::Tempo, 266.0)));
        assert_eq!(app.state.tempo(), 266.0);
    }

    // spec: ui/tempo-menu.md §4, §5
    #[test]
    fn tap_tempo_and_half_speed_switch() {
        let mut app = App::new(None);
        assert!(!app.state.shown(Win::Tapper));
        pl_run(&mut app, Cmd::Tempo(TCmd::Tapper));
        assert!(app.state.shown(Win::Tapper));
        assert_eq!(item(&menu(&app.state, None, 0.0, 0.0), "Tap tempo").mark, Mark::Check(true));
        pl_run(&mut app, Cmd::Tempo(TCmd::Tapper));
        assert!(!app.state.shown(Win::Tapper), "a second time closes it");
        let (tempo, steps) = (app.state.tempo(), app.state.session.history.at());
        pl_run(&mut app, Cmd::Tempo(TCmd::HalfSpeed));
        assert!(half_speed());
        assert_eq!(item(&menu(&app.state, None, 0.0, 0.0), "Half speed").mark, Mark::Check(true));
        assert_eq!((app.state.tempo(), app.state.session.history.at()), (tempo, steps), "the tempo stays; no undo step");
        pl_run(&mut app, Cmd::Tempo(TCmd::HalfSpeed));
        assert!(!half_speed());
    }
}
