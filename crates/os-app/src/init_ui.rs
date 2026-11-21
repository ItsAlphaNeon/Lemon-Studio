// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::init` (step 9: the session's side stays there).

#![allow(unused_imports)]
use os_model::target::{self, Target};
use os_model::InitControl;
use crate::app::State;
use crate::history::{self, Snap};
use crate::playlist::{Cmd, Mark, PItem};
#[allow(unused_imports)]
use crate::init::*;

/// "Set as song start value" and "Remove start value" for a menu whose "Create automation clip" makes a clip on
/// `control` (its target and current value 0..1); both greyed without one. Init is checked while the control is in the
/// list, and greyed ("not yet") for a control whose stored units aren't known; Delete is greyed unless it is listed.
pub fn items(s: &State, control: Option<(u32, f64)>) -> [PItem; 2] {
    let Some((t, v)) = control else {
        return [PItem::new(INIT_LABEL, Cmd::Nothing).off(), PItem::new(DELETE_LABEL, Cmd::Nothing).off()];
    };
    let on = listed(&s.session, t);
    let init = if InitControl::from_unit(t, v).is_some() { PItem::new(INIT_LABEL, Cmd::InitControl(t, v)) } else { PItem::not_yet(INIT_LABEL) };
    let delete = if on { PItem::new(DELETE_LABEL, Cmd::DeleteInit(t)) } else { PItem::new(DELETE_LABEL, Cmd::Nothing).off() };
    [init.mark(Mark::Check(on)), delete]
}

/// A list entry's name in the browser: "<owner> - <setting>", as the automation clip on the control is named
/// ("808 Kick - Channel volume", "Bus 1 - Volume", "Tempo").
pub fn name(s: &State, t: u32) -> String {
    let channel = |ch: u16| match s.session.project.channels.iter().position(|c| c.index == ch) {
        Some(row) => s.session.project.channels[row].name.clone().unwrap_or_else(|| format!("Channel {}", row + 1)),
        None => format!("Channel {}", ch + 1),
    };
    let mix = |t: u16| crate::edit::mixer::name(&s.session, t);
    match t {
        InitControl::MAIN_VOLUME => return "Main volume".into(),
        InitControl::MAIN_SWING => return "Main swing".into(),
        InitControl::MAIN_PITCH => return "Main pitch".into(),
        _ => {}
    }
    // A container member's wet or dry level (spec: plugins/patcher/README.md §4.2).
    if let Some((m, k, id, wet)) = crate::edit::container::level_of(&s.session, target::decode(t)) {
        return crate::edit::container::level_name(&s.session, m, k, id, wet);
    }
    match target::decode(t) {
        Target::Tempo => "Tempo".into(),
        Target::MainSwing => "Main swing".into(),
        Target::MainPitch => "Main pitch".into(),
        Target::ChannelVolume(ch) => format!("{} - Channel volume", channel(ch)),
        // As the original names the clips on the rack's mixer box and LED (spec: ui/channel-rack.md §19).
        Target::ChannelMixerTrack(ch) => format!("{} - Target mixer track", channel(ch)),
        Target::ChannelOn(ch) => format!("{} - Mute / solo", channel(ch)),
        Target::ChannelPan(ch) => format!("{} - Channel panning", channel(ch)),
        Target::ChannelPitch(ch) => format!("{} - Channel pitch", channel(ch)),
        Target::ChannelLevel(ch, id) => {
            let setting = match id {
                target::CHANNEL_CUTOFF => "Filter cutoff",
                target::CHANNEL_RESONANCE => "Filter resonance",
                target::CHANNEL_PAN_OFFSET => "Pan offset",
                target::CHANNEL_VOLUME_MULT => "Volume multiplier",
                target::CHANNEL_MOD_X => "Mod A offset",
                _ => "Mod B offset",
            };
            format!("{} - {setting}", channel(ch))
        }
        Target::MixerVolume(m) => format!("{} - Volume", mix(m)),
        Target::MixerPan(m) => format!("{} - Pan", mix(m)),
        Target::MixerSeparation(m) => format!("{} - Stereo separation", mix(m)),
        Target::MixerEq(m, b, k) => format!("{} - EQ band {} {}", mix(m), b + 1, ["gain", "frequency", "width"][usize::from(k).min(2)]),
        // As the original names the clips on a send knob and a slot's mix knob: "Delay - To Master", "Fruity delay
        // bank (Delay) - Mix level", an empty slot "Slot 2 (Insert 17) - Mix level" (measured 2026-11-21).
        Target::MixerSend(m, d) => format!("{} - To {}", mix(m), mix(d)),
        Target::SlotOn(m, k) => format!("{} - Slot {} switch", mix(m), k + 1),
        Target::SlotMix(m, k) => {
            let slot = crate::mixer::slot_name(&s.session, m, k).unwrap_or_else(|| format!("Slot {}", k + 1));
            format!("{slot} ({}) - Mix level", mix(m))
        }
        // A plugin that isn't hosted: its parameter by number.
        p @ (Target::ChannelParam(..) | Target::SlotParam(..)) => match (crate::params::key_of(&s.session, p), p) {
            (Some((key, index)), _) => crate::params_ui::clip_name(s, key, index),
            (None, Target::SlotParam(m, k, i)) => format!("{} - Slot {} - Parameter {}", mix(m), k + 1, i + 1),
            (None, _) => format!("{} - Parameter {}", channel((t >> 16) as u16), (t & 0x7FFF) + 1),
        },
        Target::Other(_) => format!("Control {:#06X} at {:#06X}", t & 0xFFFF, t >> 16),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{test_project, Action, App, Shown};
    use crate::knobs::KnobId;
    use crate::playlist::{self as pl, Popup};
    use crate::prefs;
    use os_model::{Clip, ClipSource};

    fn rec(target: u32, value: i32) -> InitControl {
        InitControl { head: 0, target, value }
    }

    fn app() -> App {
        let mut app = App::new(None);
        app.state.session.project = crate::locked::Locked::new(test_project());
        app
    }

    /// A menu's "Set as song start value" and "Remove start value".
    fn pair(menu: &Popup) -> (PItem, PItem) {
        let i = menu.items.iter().position(|i| i.label == INIT_LABEL).expect("Set as song start value");
        assert_eq!(menu.items[i + 1].label, DELETE_LABEL);
        (menu.items[i].clone(), menu.items[i + 1].clone())
    }

    /// The channel rack's menu of channel 1's volume knob.
    fn rack_menu(a: &App) -> Popup {
        let vol = Action::Knob(KnobId::ChanVol(0));
        crate::rack::knob_menu(&a.state, KnobId::ChanVol(0), a.automation_target(&vol), 0.0, 0.0)
    }

    /// The two items go wherever "Create automation clip" makes a clip; Init is checked while the control is listed and
    /// Delete can be chosen only then; both greyed without a control, Init greyed ("not yet") for a plugin parameter.
    #[test]
    fn menus_offer_the_initial_value_with_a_check_mark() {
        let mut a = app();
        let vol = target::channel(0, target::CHANNEL_VOLUME);
        let (init, delete) = pair(&rack_menu(&a));
        assert_eq!((init.enabled, init.mark, &init.cmd), (true, Mark::Check(false), &Cmd::InitControl(vol, 10000.0 / 12800.0)));
        assert!(!delete.enabled);
        pl::run(&mut a, init.cmd);
        assert_eq!(a.state.session.project.init_controls, vec![rec(vol, 10000)]);
        let (init, delete) = pair(&rack_menu(&a));
        assert_eq!((init.enabled, init.mark), (true, Mark::Check(true)));
        assert_eq!((delete.enabled, &delete.cmd), (true, &Cmd::DeleteInit(vol)));

        // The toolbar's and the mixer's knob menus (tempo, a fader): not listed yet.
        for action in [Action::Tempo, Action::Fader(1)] {
            let menu = pl::knob_menu(&a.state, a.automation_target(&action), None, 0.0, 0.0);
            assert!(menu.items.iter().any(|i| i.label == "Create automation clip" && i.enabled));
            let (init, delete) = pair(&menu);
            assert_eq!((init.enabled, init.mark, delete.enabled), (true, Mark::Check(false), false), "{action:?}");
        }
        // The rack's mixer box and LED: their track number and switch (spec: flp/engine-events.md "Channel rack
        // records").
        let mixer = a.automation_target(&Action::RackMixer(0));
        let (init, delete) = pair(&crate::rack::mixer_menu(&a.state, 0, mixer, 0.0, 0.0));
        assert!(init.enabled && !delete.enabled);
        let led = a.automation_target(&Action::ChannelEnable(0));
        let (init, _) = pair(&crate::rack::led_menu(&a.state, 0, led, 0.0, 0.0));
        assert_eq!(init.cmd, Cmd::InitControl(target::channel(0, target::CHANNEL_ON), 1.0));
        // No control: both greyed.
        let [init, delete] = items(&a.state, None);
        assert!(!init.enabled && !delete.enabled);
        // A plugin parameter: its stored units aren't known.
        let [init, delete] = items(&a.state, Some((target::channel_param(0, 3), 0.5)));
        assert!(!init.enabled && matches!(init.cmd, Cmd::NotYet(_)) && !delete.enabled);

        // "Remove start value": the control leaves the list.
        let delete = pair(&rack_menu(&a)).1.cmd;
        pl::run(&mut a, delete);
        assert!(a.state.session.project.init_controls.is_empty());
        assert_eq!(pair(&rack_menu(&a)).0.mark, Mark::Check(false));
    }

    /// "Set as song start value" is one undo step, "Initialized control"; "Remove start value" makes none.
    #[test]
    fn init_song_is_one_undo_step_and_delete_none() {
        let mut a = app();
        let vol = target::channel(0, target::CHANNEL_VOLUME);
        pl::run(&mut a, Cmd::InitControl(vol, 0.5));
        pl::run(&mut a, Cmd::InitControl(vol, 0.25));
        assert_eq!(a.state.session.history.len(), 2);
        assert_eq!(a.state.session.project.init_controls, vec![rec(vol, 3200)], "a new value replaces the old one");
        assert_eq!(crate::history_ui::undo(&mut a.state).as_deref(), Some(INITIALIZED));
        assert_eq!(a.state.session.project.init_controls, vec![rec(vol, 6400)]);
        assert_eq!(crate::history_ui::undo(&mut a.state).as_deref(), Some(INITIALIZED));
        assert!(a.state.session.project.init_controls.is_empty());
        assert_eq!(crate::history_ui::redo(&mut a.state).as_deref(), Some(INITIALIZED));
        assert_eq!(crate::history_ui::redo(&mut a.state).as_deref(), Some(INITIALIZED));
        assert_eq!(a.state.session.project.init_controls, vec![rec(vol, 3200)]);
        pl::run(&mut a, Cmd::DeleteInit(vol));
        assert!(a.state.session.project.init_controls.is_empty());
        assert_eq!(a.state.session.history.len(), 2, "no step");
        assert_eq!(crate::history_ui::undo(&mut a.state).as_deref(), Some(INITIALIZED), "undo goes back past the delete");
        assert_eq!(a.state.session.project.init_controls, vec![rec(vol, 6400)]);
    }

    /// "Set start values automatically" (on by default): "Create automation clip" also lists the control at its
    /// value, in the same undo step; off, only the clip is made.
    #[test]
    fn create_automation_clip_lists_the_control_while_the_check_is_on() {
        let mut a = app();
        assert!(prefs::check(&prefs::INIT_CONTROLS), "on by default");
        let vol = target::channel(0, target::CHANNEL_VOLUME);
        let (t, name, v) = a.automation_target(&Action::Knob(KnobId::ChanVol(0))).unwrap();
        pl::run(&mut a, Cmd::CreateAutomation(t, name, v));
        assert_eq!(a.state.session.project.init_controls, vec![rec(vol, 10000)]);
        assert_eq!((a.state.session.history.len(), pl::clips(&a.state.session).len()), (1, 1));
        // The clip's channel: the only selected one, in the "Automation" group with the filter on it, as the original's
        // rack shows after every Create automation clip.
        let row = a.state.session.project.channels.len() - 1;
        let g = crate::edit::rack::groups(&a.state.session.project).iter().position(|g| g.name == "Automation").expect("the group");
        assert_eq!((a.state.selected_channel, crate::edit::rack::filter(&a.state.session.project)), (row, crate::edit::rack::Filter::Group(g)));
        assert!(crate::edit::rack::groups(&a.state.session.project)[g].members.contains(&row) && a.state.rack.sel.iter().eq([&row]));
        let channels = a.state.session.project.channels.len();
        assert_eq!(crate::history_ui::undo(&mut a.state).as_deref(), Some(history::CREATE_AUTOMATION));
        assert!(a.state.session.project.init_controls.is_empty() && pl::clips(&a.state.session).is_empty());
        assert_eq!(a.state.session.project.channels.len(), channels - 1, "the clip's channel goes with the undo");
        assert!(crate::edit::rack::groups(&a.state.session.project).iter().all(|g| g.name != "Automation") && crate::edit::rack::filter(&a.state.session.project) == crate::edit::rack::Filter::All);
        assert_eq!(crate::history_ui::redo(&mut a.state).as_deref(), Some(history::CREATE_AUTOMATION));
        assert_eq!((a.state.session.project.channels.len(), crate::edit::rack::filter(&a.state.session.project)), (channels, crate::edit::rack::Filter::Group(g)));
        assert_eq!((a.state.session.project.init_controls.clone(), pl::clips(&a.state.session).len()), (vec![rec(vol, 10000)], 1));
        // A second clip on the moved knob: its new value replaces the old one; undo brings that back.
        a.state.session.project.get_mut().channels[0].volume = 0.5;
        pl::run(&mut a, Cmd::CreateAutomation(vol, "Kick - Channel volume".into(), 0.5));
        assert_eq!(a.state.session.project.init_controls, vec![rec(vol, 6400)]);
        assert_eq!(crate::history_ui::undo(&mut a.state).as_deref(), Some(history::CREATE_AUTOMATION));
        assert_eq!(a.state.session.project.init_controls, vec![rec(vol, 10000)]);
        assert_eq!(crate::history_ui::redo(&mut a.state).as_deref(), Some(history::CREATE_AUTOMATION));

        // Off: the clip only.
        prefs::set_check(&prefs::INIT_CONTROLS, false);
        let (t, name, v) = a.automation_target(&Action::Tempo).unwrap();
        pl::run(&mut a, Cmd::CreateAutomation(t, name, v));
        assert_eq!((a.state.session.project.init_controls.clone(), pl::clips(&a.state.session).len()), (vec![rec(vol, 6400)], 3));
        assert_eq!(crate::history_ui::undo(&mut a.state).as_deref(), Some(history::CREATE_AUTOMATION));
        assert_eq!((a.state.session.project.init_controls.clone(), pl::clips(&a.state.session).len()), (vec![rec(vol, 6400)], 2));
        prefs::set_check(&prefs::INIT_CONTROLS, true);
    }

    /// A song (pattern 1, empty, on track 1) with channel 1's volume and the tempo listed.
    fn song_app() -> App {
        let mut a = app();
        pl::arr_mut(&mut a.state.session).clips.push(Clip::new(0, 4 * 384, 0, ClipSource::Pattern(1)));
        a.state.session.project.get_mut().init_controls = vec![rec(target::channel(0, target::CHANNEL_VOLUME), 6400), rec(target::TEMPO, 140_000)];
        a
    }

    /// The listed controls moved away from their initial values.
    fn moved(a: &mut App) {
        a.state.session.project.get_mut().channels[0].volume = 1.0;
        a.state.set_tempo(90.0);
    }

    /// (channel 1's volume, tempo).
    fn controls(a: &App) -> (f64, f64) {
        (a.state.session.project.channels[0].volume, a.state.tempo())
    }

    /// Playback at tick 900, then back at tick 12 (the song looped), then stopped.
    fn loop_back(a: &mut App, song: bool) {
        a.show_playback(Shown { playing: true, song, tick: 900.0, ..Shown::default() });
        a.show_playback(Shown { playing: true, song, tick: 12.0, ..Shown::default() });
        a.show_playback(Shown::default());
    }

    /// In song mode, playback start, song position moves (the toolbar's position slider, Stop's rewind) and the song
    /// looping back to its start set the listed controls, as no undo step; pattern mode leaves them alone.
    #[test]
    fn the_song_sets_the_initial_values() {
        const INIT: (f64, f64) = (0.5, 140.0);
        const MOVED: (f64, f64) = (1.0, 90.0);
        let mut a = song_app();
        // Pattern mode: nothing.
        a.state.set_song_mode(false);
        moved(&mut a);
        a.set_song_pos(96, false);
        loop_back(&mut a, false);
        assert_eq!(controls(&a), MOVED);

        // Song mode: the position slider.
        a.state.set_song_mode(true);
        a.set_song_pos(192, false);
        assert_eq!(controls(&a), INIT);
        // Playback start, then Stop (the position goes back to where playback started).
        moved(&mut a);
        a.start_playback();
        assert_eq!(controls(&a), INIT);
        moved(&mut a);
        a.act(Action::Stop);
        assert_eq!((controls(&a), a.state.session.song_pos), (INIT, 192));
        // The song looping back to its start.
        moved(&mut a);
        a.show_playback(Shown { playing: true, song: true, tick: 900.0, ..Shown::default() });
        a.show_playback(Shown { playing: true, song: true, tick: 960.0, ..Shown::default() });
        assert_eq!(controls(&a), MOVED, "not while it plays on");
        a.show_playback(Shown::default());
        loop_back(&mut a, true);
        assert_eq!(controls(&a), INIT);
        assert_eq!(a.state.session.history.len(), 0, "no undo step");
        // An entry for a control the project doesn't have, or whose units aren't known, is left alone.
        a.state.session.project.get_mut().init_controls.extend([rec(target::channel(40, target::CHANNEL_VOLUME), 0), rec(target::channel_param(0, 2), 5)]);
        moved(&mut a);
        a.set_song_pos(0, false);
        assert_eq!(controls(&a), INIT);
    }

    /// Opening a project in song mode sets the listed controls, as FL 26 does (spec: flp/engine-events.md "Event
    /// 216"): the file keeps the stored values, the opened project has the initial ones and is not changed, and saving
    /// it stores them. Opened in pattern mode, the stored values stay. The mode is the one saved (event 9), clips or
    /// not; a file without it opens in song mode, as in FL.
    #[test]
    fn opening_a_song_sets_the_initial_values() {
        const INIT: (f64, f64) = (0.5, 140.0);
        const MOVED: (f64, f64) = (1.0, 90.0);
        let dir = std::env::temp_dir();
        let path = dir.join(format!("lemon-init-open-{}.flp", std::process::id()));
        let resaved = dir.join(format!("lemon-init-open-{}-resaved.flp", std::process::id()));
        let stored = |p: &std::path::Path| {
            let f = os_flp::FlpFile::parse(&std::fs::read(p).unwrap()).unwrap();
            let q = os_flp::project::load(&f).unwrap();
            (q.channels[0].volume, q.tempo.unwrap())
        };
        let mut a = song_app();
        a.state.set_song_mode(true);
        moved(&mut a);
        a.save_to(path.clone());
        assert_eq!(stored(&path), MOVED);
        let mut b = App::new(None);
        b.open(path.clone());
        assert!(b.state.song_mode());
        assert_eq!(controls(&b), INIT);
        assert!(!b.unsaved() && b.state.session.history.len() == 0, "opened, not changed");
        b.save_to(resaved.clone());
        assert_eq!(stored(&resaved), INIT, "saved as the stored values");

        // Saved in pattern mode, with the clips: opened in pattern mode, the stored values.
        a.state.set_song_mode(false);
        a.save_to(path.clone());
        let mut c = App::new(None);
        c.state.set_song_mode(true);
        c.open(path.clone());
        assert!(!c.state.song_mode() && !pl::clips(&c.state.session).is_empty());
        assert_eq!(controls(&c), MOVED);
        // Without event 9 (a file from before Lemon Studio wrote it): song mode, even with no clips.
        pl::arr_mut(&mut a.state.session).clips.clear();
        a.save_to(path.clone());
        let mut f = os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap();
        for c in &mut f.chunks {
            if let os_flp::ChunkBody::Events(ev) = &mut c.body {
                ev.retain(|e| e.id != 9);
            }
        }
        std::fs::write(&path, f.to_bytes().unwrap()).unwrap();
        let mut d = App::new(None);
        d.open(path.clone());
        assert!(d.state.song_mode());
        assert_eq!(controls(&d), INIT);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&resaved);
    }

    /// The browser lists the controls under Current project ▸ Initialized controls ("<owner> - <setting>"), a folder
    /// shown only while there are some.
    #[test]
    fn browser_lists_the_initialized_controls() {
        use crate::views::browser::{rows, INITIALIZED_CONTROLS};
        let mut a = app();
        a.state.browser_tab = 1;
        let folder = format!("1:Current project/{INITIALIZED_CONTROLS}");
        a.state.browser_open.extend(["1:Current project".to_string(), folder.clone()]);
        assert!(!rows(&a.state).iter().any(|r| r.key == folder), "no folder while the list is empty");
        a.state.session.project.get_mut().init_controls = vec![
            rec(target::channel(1, target::CHANNEL_VOLUME), 10000),
            rec(target::mixer(1, target::MIXER_VOLUME), 12800),
            rec(target::TEMPO, 140_000),
            rec(target::slot(2, 0, target::SLOT_MIX), 6400),
            rec(target::channel_param(0, 3), 5),
        ];
        let list = rows(&a.state);
        let at = list.iter().position(|r| r.key == folder).expect("the folder");
        assert_eq!(list[at - 1].label, crate::views::browser::REMOTE_CONTROL, "after Remote control (spec: ui/browser.md)");
        let inside = format!("{folder}/");
        let names: Vec<&str> = list.iter().filter(|r| r.key.starts_with(&inside)).map(|r| r.label.as_str()).collect();
        assert_eq!(names, ["Clap - Channel volume", "Bus 1 - Volume", "Tempo", "Slot 1 (Bus 2) - Mix level", "Kick - Parameter 4"]);
    }
}
