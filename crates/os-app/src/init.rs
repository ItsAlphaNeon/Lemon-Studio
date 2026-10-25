// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The project's initial values (`Project::init_controls`): controls that take a value when the song starts. The
//! Automation group of a control's menu adds one, "Init song with this position" (one undo step, "Initialized
//! control"), and takes it out, "Delete initial value" (no undo step, as in FL). With "Initialize controls
//! automatically" on, "Create automation clip" adds the control too, in its own step (`playlist::create_automation`).
//! In song mode, starting playback, moving the song position and playback looping back to the song's start set every
//! control in the list to its value (`apply`; the engine sets them itself where it loops), then automation clips act as
//! usual. The browser lists them under Current project ▸ Initialized controls (`views::browser`); the file holds them
//! in event 216 (`os_flp::project`).
// spec: ui/settings-choices.md "Advanced: Initialize controls automatically (2026-10-16)"

use os_model::target::{self, Target};
use os_model::InitControl;

use crate::app::State;
use crate::history::{self, Snap};
use crate::playlist::{Cmd, Mark, PItem};

/// The undo step of "Init song with this position" (FL's name).
pub const INITIALIZED: &str = "Initialized control";
pub const INIT_LABEL: &str = "Init song with this position";
pub const DELETE_LABEL: &str = "Delete initial value";

/// Control `t`'s entry in the list.
pub fn entry(s: &State, t: u32) -> Option<InitControl> {
    s.project.init_controls.iter().find(|c| c.target == t).copied()
}

/// Whether control `t` is in the list (the check mark of "Init song with this position").
pub fn listed(s: &State, t: u32) -> bool {
    entry(s, t).is_some()
}

/// Puts control `t`'s entry in the list (`None`: takes it out). An entry already there keeps its place.
pub(crate) fn put(s: &mut State, t: u32, e: Option<InitControl>) {
    let list = &mut s.project.init_controls;
    match e {
        Some(c) => match list.iter_mut().find(|x| x.target == t) {
            Some(x) => *x = c,
            None => list.push(c),
        },
        None => list.retain(|x| x.target != t),
    }
    // Playback gets the list (the engine sets it where a song loops).
    s.piano.dirty = true;
}

/// "Init song with this position": control `t`, at its value `v` (0..1), goes in the list, replacing an earlier value.
/// One undo step, "Initialized control". Nothing for a control whose stored units aren't known (a plugin parameter).
pub fn init(s: &mut State, t: u32, v: f64) {
    let Some(c) = InitControl::from_unit(t, v) else { return };
    history::record(s, INITIALIZED, Snap::Init(t, entry(s, t)));
    put(s, t, Some(c));
}

/// "Delete initial value": control `t` leaves the list. No undo step (FL makes none).
pub fn delete(s: &mut State, t: u32) {
    put(s, t, None);
}

/// What "Create automation clip" on control `t` (value `v`) puts in the list: its entry while "Initialize controls
/// automatically" is on, `None` while it is off or for a control whose stored units aren't known.
pub(crate) fn automatic(t: u32, v: f64) -> Option<InitControl> {
    crate::prefs::check(&crate::prefs::INIT_CONTROLS).then(|| InitControl::from_unit(t, v)).flatten()
}

/// "Init song with this position" and "Delete initial value" for a menu whose "Create automation clip" makes a clip on
/// `control` (its target and current value 0..1); both greyed without one. Init is checked while the control is in the
/// list, and greyed ("not yet") for a control whose stored units aren't known; Delete is greyed unless it is listed.
pub fn items(s: &State, control: Option<(u32, f64)>) -> [PItem; 2] {
    let Some((t, v)) = control else {
        return [PItem::new(INIT_LABEL, Cmd::Nothing).off(), PItem::new(DELETE_LABEL, Cmd::Nothing).off()];
    };
    let on = listed(s, t);
    let init = if InitControl::from_unit(t, v).is_some() { PItem::new(INIT_LABEL, Cmd::InitControl(t, v)) } else { PItem::not_yet(INIT_LABEL) };
    let delete = if on { PItem::new(DELETE_LABEL, Cmd::DeleteInit(t)) } else { PItem::new(DELETE_LABEL, Cmd::Nothing).off() };
    [init.mark(Mark::Check(on)), delete]
}

/// In song mode: every control in the list takes its value at once (where playback starts, the song position moves
/// or playback loops back to the song's start). Not an undo step. A record whose units aren't known, or whose control
/// the project doesn't have, is left alone. Returns whether a value changed; the project then goes to playback.
pub fn apply(s: &mut State) -> bool {
    if !s.song_mode {
        return false;
    }
    let mut changed = false;
    for c in s.project.init_controls.clone() {
        changed |= set_value(s, c);
    }
    if changed {
        s.piano.dirty = true;
    }
    changed
}

/// `*x = v`; whether it changed.
fn set<T: PartialEq>(x: &mut T, v: T) -> bool {
    let changed = *x != v;
    *x = v;
    changed
}

/// Control `c.target` set to `c.value` (stored units) in the model.
fn set_value(s: &mut State, c: InitControl) -> bool {
    let v = c.value;
    let f = f64::from(v);
    let p = &mut s.project;
    match c.target {
        InitControl::MAIN_VOLUME => return set(&mut p.main_volume, Some(v.clamp(0, 16000) as u16)),
        InitControl::MAIN_SWING => return set(&mut p.swing, Some(v.clamp(0, 128) as u8)),
        InitControl::MAIN_PITCH => return set(&mut p.main_pitch, Some(v.clamp(-1200, 1200) as i16)),
        _ => {}
    }
    match target::decode(c.target) {
        Target::Tempo => set(&mut s.tempo, (f / 1000.0).clamp(10.0, 522.0)),
        Target::ChannelVolume(ch) => p.channels.iter_mut().find(|x| x.index == ch).is_some_and(|x| set(&mut x.volume, (f / 12800.0).clamp(0.0, 1.0))),
        Target::ChannelPan(ch) => p.channels.iter_mut().find(|x| x.index == ch).is_some_and(|x| set(&mut x.pan, (f / 6400.0 - 1.0).clamp(-1.0, 1.0))),
        Target::ChannelPitch(ch) => p.channels.iter_mut().find(|x| x.index == ch).is_some_and(|x| set(&mut x.pitch_cents, v)),
        Target::MixerVolume(t) => p.mixer.iter_mut().find(|m| m.index == t).is_some_and(|m| set(&mut m.volume, Some((f / 16000.0).clamp(0.0, 1.0)))),
        Target::MixerPan(t) => p.mixer.iter_mut().find(|m| m.index == t).is_some_and(|m| set(&mut m.pan, Some((f / 6400.0).clamp(-1.0, 1.0)))),
        Target::MixerSeparation(t) => {
            p.mixer.iter_mut().find(|m| m.index == t).is_some_and(|m| set(&mut m.stereo_separation, Some((f / 64.0).clamp(-1.0, 1.0))))
        }
        Target::MixerEq(t, b, k) => p
            .mixer
            .iter_mut()
            .find(|m| m.index == t)
            .and_then(|m| m.eq.get_mut(usize::from(b))?.get_mut(usize::from(k)))
            .is_some_and(|x| set(x, v)),
        Target::MixerSend(t, d) => p.mixer.iter_mut().find(|m| m.index == t).is_some_and(|m| {
            let level = (f / 16000.0).clamp(0.0, 1.0);
            match m.send_levels.iter_mut().find(|x| x.0 == d) {
                Some(x) => set(&mut x.1, level),
                None => {
                    m.send_levels.push((d, level));
                    m.send_levels.sort_by_key(|x| x.0);
                    true
                }
            }
        }),
        Target::SlotOn(t, k) => p.mixer.iter_mut().find(|m| m.index == t).is_some_and(|m| set(&mut m.slot_on[usize::from(k).min(9)], v != 0)),
        Target::SlotMix(t, k) => {
            p.mixer.iter_mut().find(|m| m.index == t).is_some_and(|m| set(&mut m.slot_mix[usize::from(k).min(9)], (f / 12800.0).clamp(0.0, 1.0)))
        }
        _ => false,
    }
}

/// A list entry's name in the browser: "<owner> - <setting>", as the automation clip on the control is named
/// ("808 Kick - Channel volume", "Insert 1 - Volume", "Tempo").
pub fn name(s: &State, t: u32) -> String {
    let channel = |ch: u16| match s.project.channels.iter().position(|c| c.index == ch) {
        Some(row) => s.project.channels[row].name.clone().unwrap_or_else(|| format!("Channel {}", row + 1)),
        None => format!("Channel {}", ch + 1),
    };
    let mix = |t: u16| crate::mixer::name(s, t);
    match t {
        InitControl::MAIN_VOLUME => return "Main volume".into(),
        InitControl::MAIN_SWING => return "Main swing".into(),
        InitControl::MAIN_PITCH => return "Main pitch".into(),
        _ => {}
    }
    match target::decode(t) {
        Target::Tempo => "Tempo".into(),
        Target::ChannelVolume(ch) => format!("{} - Channel volume", channel(ch)),
        Target::ChannelPan(ch) => format!("{} - Channel panning", channel(ch)),
        Target::ChannelPitch(ch) => format!("{} - Channel pitch", channel(ch)),
        Target::ChannelLevel(ch, id) => {
            let setting = match id {
                target::CHANNEL_CUTOFF => "Filter cutoff",
                target::CHANNEL_RESONANCE => "Filter resonance",
                target::CHANNEL_PAN_OFFSET => "Pan offset",
                target::CHANNEL_VOLUME_MULT => "Volume multiplier",
                target::CHANNEL_MOD_X => "Mod X offset",
                _ => "Mod Y offset",
            };
            format!("{} - {setting}", channel(ch))
        }
        Target::MixerVolume(m) => format!("{} - Volume", mix(m)),
        Target::MixerPan(m) => format!("{} - Pan", mix(m)),
        Target::MixerSeparation(m) => format!("{} - Stereo separation", mix(m)),
        Target::MixerEq(m, b, k) => format!("{} - EQ band {} {}", mix(m), b + 1, ["gain", "frequency", "width"][usize::from(k).min(2)]),
        Target::MixerSend(m, d) => format!("{} - Send to {}", mix(m), mix(d)),
        Target::SlotOn(m, k) => format!("{} - Slot {} switch", mix(m), k + 1),
        Target::SlotMix(m, k) => format!("{} - Slot {} mix level", mix(m), k + 1),
        // A plugin that isn't hosted: its parameter by number.
        p @ (Target::ChannelParam(..) | Target::SlotParam(..)) => match (crate::params::key_of(s, p), p) {
            (Some((key, index)), _) => crate::params::clip_name(s, key, index),
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

    fn app() -> App {
        let mut app = App::new(None);
        app.state.project = test_project();
        app
    }

    fn rec(target: u32, value: i32) -> InitControl {
        InitControl { head: 0, target, value }
    }

    /// A menu's "Init song with this position" and "Delete initial value".
    fn pair(menu: &Popup) -> (PItem, PItem) {
        let i = menu.items.iter().position(|i| i.label == INIT_LABEL).expect("Init song with this position");
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
        assert_eq!(a.state.project.init_controls, vec![rec(vol, 10000)]);
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
        // No clip to make (the rack's mixer box): both greyed.
        let (init, delete) = pair(&crate::rack::mixer_menu(&a.state, 0, 0.0, 0.0));
        assert!(!init.enabled && !delete.enabled);
        // A plugin parameter: its stored units aren't known.
        let [init, delete] = items(&a.state, Some((target::channel_param(0, 3), 0.5)));
        assert!(!init.enabled && matches!(init.cmd, Cmd::NotYet(_)) && !delete.enabled);

        // "Delete initial value": the control leaves the list.
        let delete = pair(&rack_menu(&a)).1.cmd;
        pl::run(&mut a, delete);
        assert!(a.state.project.init_controls.is_empty());
        assert_eq!(pair(&rack_menu(&a)).0.mark, Mark::Check(false));
    }

    /// "Init song with this position" is one undo step, "Initialized control"; "Delete initial value" makes none.
    #[test]
    fn init_song_is_one_undo_step_and_delete_none() {
        let mut a = app();
        let vol = target::channel(0, target::CHANNEL_VOLUME);
        pl::run(&mut a, Cmd::InitControl(vol, 0.5));
        pl::run(&mut a, Cmd::InitControl(vol, 0.25));
        assert_eq!(a.state.history.len(), 2);
        assert_eq!(a.state.project.init_controls, vec![rec(vol, 3200)], "a new value replaces the old one");
        assert_eq!(history::undo(&mut a.state), Some(INITIALIZED));
        assert_eq!(a.state.project.init_controls, vec![rec(vol, 6400)]);
        assert_eq!(history::undo(&mut a.state), Some(INITIALIZED));
        assert!(a.state.project.init_controls.is_empty());
        assert_eq!(history::redo(&mut a.state), Some(INITIALIZED));
        assert_eq!(history::redo(&mut a.state), Some(INITIALIZED));
        assert_eq!(a.state.project.init_controls, vec![rec(vol, 3200)]);
        pl::run(&mut a, Cmd::DeleteInit(vol));
        assert!(a.state.project.init_controls.is_empty());
        assert_eq!(a.state.history.len(), 2, "no step");
        assert_eq!(history::undo(&mut a.state), Some(INITIALIZED), "undo goes back past the delete");
        assert_eq!(a.state.project.init_controls, vec![rec(vol, 6400)]);
    }

    /// "Initialize controls automatically" (on by default): "Create automation clip" also lists the control at its
    /// value, in the same undo step; off, only the clip is made.
    #[test]
    fn create_automation_clip_lists_the_control_while_the_check_is_on() {
        let mut a = app();
        assert!(prefs::check(&prefs::INIT_CONTROLS), "on by default");
        let vol = target::channel(0, target::CHANNEL_VOLUME);
        let (t, name, v) = a.automation_target(&Action::Knob(KnobId::ChanVol(0))).unwrap();
        pl::run(&mut a, Cmd::CreateAutomation(t, name, v));
        assert_eq!(a.state.project.init_controls, vec![rec(vol, 10000)]);
        assert_eq!((a.state.history.len(), pl::clips(&a.state).len()), (1, 1));
        assert_eq!(history::undo(&mut a.state), Some(history::CREATE_AUTOMATION));
        assert!(a.state.project.init_controls.is_empty() && pl::clips(&a.state).is_empty());
        assert_eq!(history::redo(&mut a.state), Some(history::CREATE_AUTOMATION));
        assert_eq!((a.state.project.init_controls.clone(), pl::clips(&a.state).len()), (vec![rec(vol, 10000)], 1));
        // A second clip on the moved knob: its new value replaces the old one; undo brings that back.
        a.state.project.channels[0].volume = 0.5;
        pl::run(&mut a, Cmd::CreateAutomation(vol, "Kick - Channel volume".into(), 0.5));
        assert_eq!(a.state.project.init_controls, vec![rec(vol, 6400)]);
        assert_eq!(history::undo(&mut a.state), Some(history::CREATE_AUTOMATION));
        assert_eq!(a.state.project.init_controls, vec![rec(vol, 10000)]);
        assert_eq!(history::redo(&mut a.state), Some(history::CREATE_AUTOMATION));

        // Off: the clip only.
        prefs::set_check(&prefs::INIT_CONTROLS, false);
        let (t, name, v) = a.automation_target(&Action::Tempo).unwrap();
        pl::run(&mut a, Cmd::CreateAutomation(t, name, v));
        assert_eq!((a.state.project.init_controls.clone(), pl::clips(&a.state).len()), (vec![rec(vol, 6400)], 3));
        assert_eq!(history::undo(&mut a.state), Some(history::CREATE_AUTOMATION));
        assert_eq!((a.state.project.init_controls.clone(), pl::clips(&a.state).len()), (vec![rec(vol, 6400)], 2));
        prefs::set_check(&prefs::INIT_CONTROLS, true);
    }

    /// A song (pattern 1, empty, on track 1) with channel 1's volume and the tempo listed.
    fn song_app() -> App {
        let mut a = app();
        pl::arr_mut(&mut a.state).clips.push(Clip::new(0, 4 * 384, 0, ClipSource::Pattern(1)));
        a.state.project.init_controls = vec![rec(target::channel(0, target::CHANNEL_VOLUME), 6400), rec(target::TEMPO, 140_000)];
        a
    }

    /// The listed controls moved away from their initial values.
    fn moved(a: &mut App) {
        a.state.project.channels[0].volume = 1.0;
        a.state.tempo = 90.0;
    }

    /// (channel 1's volume, tempo).
    fn controls(a: &App) -> (f64, f64) {
        (a.state.project.channels[0].volume, a.state.tempo)
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
        a.state.song_mode = false;
        moved(&mut a);
        a.set_song_pos(96, false);
        loop_back(&mut a, false);
        assert_eq!(controls(&a), MOVED);

        // Song mode: the position slider.
        a.state.song_mode = true;
        a.set_song_pos(192, false);
        assert_eq!(controls(&a), INIT);
        // Playback start, then Stop (the position goes back to where playback started).
        moved(&mut a);
        a.start_playback();
        assert_eq!(controls(&a), INIT);
        moved(&mut a);
        a.act(Action::Stop);
        assert_eq!((controls(&a), a.state.playlist.pos), (INIT, 192));
        // The song looping back to its start.
        moved(&mut a);
        a.show_playback(Shown { playing: true, song: true, tick: 900.0, ..Shown::default() });
        a.show_playback(Shown { playing: true, song: true, tick: 960.0, ..Shown::default() });
        assert_eq!(controls(&a), MOVED, "not while it plays on");
        a.show_playback(Shown::default());
        loop_back(&mut a, true);
        assert_eq!(controls(&a), INIT);
        assert_eq!(a.state.history.len(), 0, "no undo step");
        // An entry for a control the project doesn't have, or whose units aren't known, is left alone.
        a.state.project.init_controls.extend([rec(target::channel(40, target::CHANNEL_VOLUME), 0), rec(target::channel_param(0, 2), 5)]);
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
        a.state.song_mode = true;
        moved(&mut a);
        a.save_to(path.clone());
        assert_eq!(stored(&path), MOVED);
        let mut b = App::new(None);
        b.open(path.clone());
        assert!(b.state.song_mode);
        assert_eq!(controls(&b), INIT);
        assert!(!b.unsaved() && b.state.history.len() == 0, "opened, not changed");
        b.save_to(resaved.clone());
        assert_eq!(stored(&resaved), INIT, "saved as the stored values");

        // Saved in pattern mode, with the clips: opened in pattern mode, the stored values.
        a.state.song_mode = false;
        a.save_to(path.clone());
        let mut c = App::new(None);
        c.state.song_mode = true;
        c.open(path.clone());
        assert!(!c.state.song_mode && !pl::clips(&c.state).is_empty());
        assert_eq!(controls(&c), MOVED);
        // Without event 9 (a file from before Lemon Studio wrote it): song mode, even with no clips.
        pl::arr_mut(&mut a.state).clips.clear();
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
        assert!(d.state.song_mode);
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
        a.state.project.init_controls = vec![
            rec(target::channel(1, target::CHANNEL_VOLUME), 10000),
            rec(target::mixer(1, target::MIXER_VOLUME), 12800),
            rec(target::TEMPO, 140_000),
            rec(target::slot(2, 0, target::SLOT_MIX), 6400),
            rec(target::channel_param(0, 3), 5),
        ];
        let list = rows(&a.state);
        let at = list.iter().position(|r| r.key == folder).expect("the folder");
        assert_eq!(list[at - 1].label, "Generators");
        let inside = format!("{folder}/");
        let names: Vec<&str> = list.iter().filter(|r| r.key.starts_with(&inside)).map(|r| r.label.as_str()).collect();
        assert_eq!(names, ["Clap - Channel volume", "Insert 1 - Volume", "Tempo", "Insert 2 - Slot 1 mix level", "Kick - Parameter 4"]);
    }
}
