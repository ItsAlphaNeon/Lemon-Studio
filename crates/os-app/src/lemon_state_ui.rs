// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::lemon_state` (step 9: the session's side stays there).

#![allow(unused_imports)]
#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use std::collections::BTreeSet;
use crate::app::State;
use crate::icon::ICONS;
use crate::playlist::IconOwner;
use crate::plugins::Plugins;
use os_flp::{Event, FlpFile, Value, lemon};
#[allow(unused_imports)]
use crate::lemon_state::*;

/// The `LMex` events holding the Lemon Studio state of `s` and `plugins`, in ID order. Set keyboard focus of all
/// plugins to ▸ is written only where it works (Windows and macOS); elsewhere the file's own event is kept.
pub fn events(s: &State, plugins: &Plugins) -> Vec<Event> {
    events_in(&s.session, plugins, &looks(s))
}

/// The interface's choices as `s` has them.
fn looks(s: &State) -> Looks<'_> {
    let pl = &s.playlist;
    let performance = [
        (pl.perf_clip_progress, CLIP_PROGRESS),
        (pl.perf_track_progress, TRACK_PROGRESS),
        (pl.perf_plus_scene, PLUS_SCENE),
        (pl.perf_velocity_subclips, VELOCITY_SUBCLIPS),
    ]
    .iter()
    .filter(|(on, _)| *on)
    .fold(0, |f, (_, bit)| f | bit);
    Looks { waveforms: s.mixer.waveforms, extra: s.mixer.extra, wave_preview: &s.rack.wave_preview, through_off: &s.rack.through_off, performance, note_lock: s.rack.note_lock }
}

/// The bytes [`events`] would write: a change to them is an unsaved change, as an edit of the project is. Set keyboard
/// focus of all plugins to ▸ is left out: it is a switch for the whole program, with no undo step, so it marks no
/// project changed.
pub fn saved_form(s: &State, plugins: &Plugins) -> Vec<u8> {
    saved_form_in(&s.session, plugins, &looks(s))
}

/// Puts the state a `.lemon` file holds into `s` and `plugins` (a project just opened, its ids given). An FLP has
/// none; its channels and mixer tracks show the icons their numbers stand for instead ([`from_flp`]).
pub fn apply(s: &mut State, file: &FlpFile) {
    apply_session(&mut s.session, file);
    if !lemon::is_lemon(file) {
        return;
    }
    let read = |id| lemon::data(file, id).filter(|d| d.first() == Some(&LAYOUT));
    if let Some(&flags) = read(lemon::MIXER).as_deref().and_then(|d| d.get(1)) {
        s.mixer.waveforms = flags & WAVEFORMS != 0;
        s.mixer.extra = flags & EXTRA_PROPS != 0;
    }
    if let Some(d) = read(lemon::CHANNEL_CHECKS) {
        let mut r = Reader(&d[1..]);
        for _ in 0..r.u32().unwrap_or(0) {
            let (Some(n), Some(flags)) = (r.u16(), r.u8()) else { break };
            let Some(id) = s.session.project.channels.get(usize::from(n)).map(|c| c.id.0) else { continue };
            if flags & WAVE_PREVIEW != 0 {
                s.rack.wave_preview.insert(id);
            }
            if flags & MIDI_THROUGH == 0 {
                s.rack.through_off.insert(id);
            }
        }
    }
    #[cfg(any(windows, target_os = "macos"))]
    if let Some(&on) = read(lemon::KEY_FOCUS).as_deref().and_then(|d| d.get(1)) {
        set_editor_keys(on != 0);
    }
    if let Some(d) = read(lemon::NOTE_LOCKS) {
        let mut r = Reader(&d[1..]);
        for _ in 0..r.u32().unwrap_or(0) {
            let (Some(src), Some(row)) = (r.u8(), r.u16()) else { break };
            let row = usize::from(row);
            if let Some(lock) = s.rack.note_lock.get_mut(usize::from(src)).filter(|_| row < s.session.project.channels.len()) {
                *lock = Some(row);
            }
        }
    }
    if let Some(&flags) = read(lemon::PERFORMANCE).as_deref().and_then(|d| d.get(1)) {
        let pl = &mut s.playlist;
        pl.perf_clip_progress = flags & CLIP_PROGRESS != 0;
        pl.perf_track_progress = flags & TRACK_PROGRESS != 0;
        pl.perf_plus_scene = flags & PLUS_SCENE != 0;
        pl.perf_velocity_subclips = flags & VELOCITY_SUBCLIPS != 0;
    }
}

/// [`lemon::KEY_FOCUS`]: Set keyboard focus of all plugins to ▸ (`plugin_window::editor_keys`). The switch is one for
/// the whole program, so tests keep their own, one per test thread.
#[cfg(any(windows, target_os = "macos"))]
fn editor_keys() -> bool {
    #[cfg(test)]
    return tests::KEYS.with(|k| k.get());
    #[cfg(not(test))]
    crate::plugin_window::editor_keys()
}

#[cfg(any(windows, target_os = "macos"))]
fn set_editor_keys(on: bool) {
    #[cfg(test)]
    tests::KEYS.with(|k| k.set(on));
    #[cfg(not(test))]
    crate::plugin_window::set_editor_keys(on);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("os_lemon_state_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    use crate::app::App;
    use crate::icon::Icon;

    std::thread_local! {
        /// This test thread's Set keyboard focus of all plugins to ▸ (`editor_keys`).
        pub(super) static KEYS: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
    }

    /// Without an interface, a save writes the choices a new interface starts with.
    #[test]
    fn no_interface_looks_as_a_new_one() {
        let a = crate::app::App::new(None);
        let (l, n) = (looks(&a.state), Looks::none());
        assert_eq!((l.waveforms, l.extra, l.performance, l.note_lock), (n.waveforms, n.extra, n.performance, n.note_lock));
        assert_eq!((l.wave_preview, l.through_off), (n.wave_preview, n.through_off));
    }
    /// An app with every kind of state set: icons on the second channel, mixer track 2 and its slot 1; slot 1's name
    /// and slot 0's colour on track 2; Performance settings other than the defaults.
    fn with_state() -> App {
        let mut app = App::new(None);
        app.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        app.state.session.control.settle(app.state.session.project.get_mut());
        let ch = app.state.session.project.channels[1].id.0;
        let s = &mut app.state;
        crate::edit::playlist::put_icon(&mut s.session, IconOwner::Channel(ch), Some(Icon::Mic));
        crate::edit::playlist::put_icon(&mut s.session, IconOwner::Track(2), Some(Icon::Star));
        crate::edit::playlist::put_icon(&mut s.session, IconOwner::Slot(2, 1), Some(Icon::Plug));
        crate::edit::mixer::set_slot_label(&mut s.session, 2, 1, Some("Crunch ✓".into()));
        crate::edit::mixer::set_slot_color(&mut s.session, 2, 0, Some(os_model::Rgb::new(10, 200, 30)));
        s.playlist.perf_clip_progress = false;
        s.playlist.perf_track_progress = true;
        s.playlist.perf_plus_scene = true;
        s.playlist.perf_velocity_subclips = false;
        app
    }

    /// Saved to a .lemon file and opened again, each kind of state comes back: the channel's icon on the channel at
    /// the same rack position (ids are new), the track's and the slot's, the slot's name and colour, the Performance
    /// settings. Changing one of them is an unsaved change.
    #[test]
    fn state_round_trips_through_a_lemon_file() {
        let d = dir("round");
        let path = d.join("State.lemon");
        let mut app = with_state();
        app.save_to(path.clone());
        assert!(!app.unsaved());
        let file = os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap();
        let ids: Vec<u8> = lemon::events(&file).unwrap().iter().map(|e| e.id).collect();
        let mut want = vec![lemon::SAVED_BY, lemon::ICONS, lemon::SLOT_NAMES, lemon::SLOT_COLORS, lemon::PERFORMANCE, lemon::MIXER];
        want.extend([lemon::SCALING, lemon::CHANNEL_CHECKS]);
        #[cfg(any(windows, target_os = "macos"))]
        want.push(lemon::KEY_FOCUS);
        want.extend([lemon::TRACK_LOCKS, lemon::NOTE_LOCKS, lemon::FEATURES]);
        assert_eq!(ids, want);
        let mut back = App::new(None);
        back.open(path.clone());
        assert!(!back.unsaved(), "the state read back is the saved state");
        let s = &back.state;
        let ch = s.session.project.channels[1].id.0;
        assert_eq!(crate::playlist::icons(&s.session.project).get(&IconOwner::Channel(ch)), Some(&Icon::Mic));
        assert_eq!(crate::playlist::icons(&s.session.project).get(&IconOwner::Track(2)), Some(&Icon::Star));
        assert_eq!(crate::playlist::icons(&s.session.project).get(&IconOwner::Slot(2, 1)), Some(&Icon::Plug));
        assert_eq!(crate::playlist::icons(&s.session.project).len(), 3);
        assert_eq!(crate::mixer::slot_names_of(&s.session.project).get(&(2, 1)).map(String::as_str), Some("Crunch ✓"));
        assert_eq!(crate::mixer::slot_colors_of(&s.session.project).get(&(2, 0)), Some(&os_model::Rgb::new(10, 200, 30)));
        let pl = &s.playlist;
        assert_eq!((pl.perf_clip_progress, pl.perf_track_progress, pl.perf_plus_scene, pl.perf_velocity_subclips), (false, true, true, false));
        crate::edit::playlist::put_icon(&mut back.state.session, IconOwner::Track(2), None);
        assert!(back.unsaved(), "an icon taken off is an unsaved change");
        // Saved again, what was taken off stays off (the events are replaced, not kept from the opened file).
        back.save_to(path.clone());
        let mut again = App::new(None);
        again.open(path);
        assert_eq!(crate::playlist::icons(&again.state.session.project).len(), 2);
        assert!(!crate::playlist::icons(&again.state.session.project).contains_key(&IconOwner::Track(2)));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// `with_state`, plus the settings of events 197-201: Plugin delay compensation ▸ Automatic off, View ▸ Waveforms
    /// on and Extra properties off; Scaling ▸ 150 % on channel 0's window and 125 % on track 3's slot 2; Show waveform
    /// preview on channel 1 and MIDI channel through off on channel 2; tracks 3 and 4 of the first arrangement locked to
    /// pattern 1 and to channel 0.
    fn with_settings() -> App {
        let mut app = with_state();
        let s = &mut app.state;
        s.session.project.get_mut().pdc_off = true;
        s.mixer.waveforms = true;
        s.mixer.extra = false;
        s.rack.wave_preview.insert(s.session.project.channels[1].id.0);
        s.rack.through_off.insert(s.session.project.channels[2].id.0);
        if s.session.project.arrangements.is_empty() {
            s.session.project.get_mut().arrangements.push(os_flp::project::default_arrangement());
        }
        let tracks = &mut s.session.project.get_mut().arrangements[0].tracks;
        tracks[3].clip_lock = Some(os_model::ClipSource::Pattern(1));
        tracks[4].clip_lock = Some(os_model::ClipSource::Channel(0));
        app.state.session.plugins.editor_scales.insert(0, 1.5);
        app.state.session.plugins.editor_scales.insert(crate::plugins::fx_key(3, 2), 1.25);
        app
    }

    /// Saved to a .lemon file and opened again, the settings of events 197-201 come back (the checks on the channels
    /// at the same rack positions), Set keyboard focus of all plugins to ▸ too; a project opened from an FLP has the
    /// defaults (and a Scaling ▸ of the project opened before is gone). A changed Scaling ▸ is an unsaved change; the
    /// keyboard focus switch is not.
    #[test]
    fn settings_round_trip_through_a_lemon_file() {
        let d = dir("settings");
        let (lemon_path, flp_path) = (d.join("Settings.lemon"), d.join("Settings.flp"));
        let mut app = with_settings();
        #[cfg(any(windows, target_os = "macos"))]
        set_editor_keys(false);
        app.save_to(lemon_path.clone());
        app.save_to(flp_path.clone());
        assert!(app.state.message.as_deref().is_some_and(|m| m.starts_with("Saved")), "{:?}", app.state.message);
        #[cfg(any(windows, target_os = "macos"))]
        set_editor_keys(true);
        let mut back = App::new(None);
        back.open(lemon_path.clone());
        #[cfg(any(windows, target_os = "macos"))]
        assert!(!editor_keys(), "Disabled comes back");
        assert!(!back.unsaved());
        let s = &back.state;
        assert!(s.session.project.pdc_off && s.mixer.waveforms && !s.mixer.extra);
        let ids: Vec<u32> = s.session.project.channels.iter().map(|c| c.id.0).collect();
        assert_eq!(s.rack.wave_preview.iter().copied().collect::<Vec<_>>(), [ids[1]]);
        assert_eq!(s.rack.through_off.iter().copied().collect::<Vec<_>>(), [ids[2]]);
        let locks: Vec<_> = s.session.project.arrangements[0].tracks.iter().enumerate().filter_map(|(t, tr)| Some((t, tr.clip_lock?))).collect();
        assert_eq!(locks, [(3, os_model::ClipSource::Pattern(1)), (4, os_model::ClipSource::Channel(0))]);
        let scales: Vec<(usize, f32)> = back.state.session.plugins.editor_scales.iter().map(|(&k, &v)| (k, v)).collect();
        assert_eq!(scales, [(0, 1.5), (crate::plugins::fx_key(3, 2), 1.25)]);
        back.state.session.plugins.editor_scales.remove(&0);
        assert!(back.unsaved(), "a Scaling ▸ change is an unsaved change");
        #[cfg(any(windows, target_os = "macos"))]
        {
            back.plugins.editor_scales.insert(0, 1.5);
            set_editor_keys(true);
            assert!(!back.unsaved(), "the keyboard focus switch is not");
        }
        // The FLP saved from the same state: no LMex, so the defaults (opened over the .lemon project).
        back.open(flp_path);
        let s = &back.state;
        assert!(!s.session.project.pdc_off && !s.mixer.waveforms && s.mixer.extra);
        assert!(s.rack.wave_preview.is_empty() && s.rack.through_off.is_empty() && back.state.session.plugins.editor_scales.is_empty());
        assert!(s.session.project.arrangements.iter().flat_map(|a| &a.tracks).all(|t| t.clip_lock.is_none()));
        #[cfg(any(windows, target_os = "macos"))]
        assert!(editor_keys());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Receive notes from ▸ locks (event 202) come back on the channels at the same rack positions; one taken off is an
    /// unsaved change and stays off once saved; an FLP saved from the same state opens with none.
    #[test]
    fn note_locks_round_trip_through_a_lemon_file() {
        use crate::rack::NoteSrc;
        let d = dir("note_locks");
        let (lemon_path, flp_path) = (d.join("Locks.lemon"), d.join("Locks.flp"));
        let mut app = with_state();
        app.state.rack.note_lock[NoteSrc::Typing as usize] = Some(3);
        app.state.rack.note_lock[NoteSrc::Touch as usize] = Some(1);
        app.save_to(lemon_path.clone());
        app.save_to(flp_path.clone());
        let file = os_flp::FlpFile::parse(&std::fs::read(&lemon_path).unwrap()).unwrap();
        assert_eq!(lemon::data(&file, lemon::NOTE_LOCKS), Some(vec![LAYOUT, 2, 0, 0, 0, 0, 3, 0, 1, 1, 0]));
        let mut back = App::new(None);
        back.open(lemon_path.clone());
        assert!(!back.unsaved());
        assert_eq!(back.state.rack.note_lock, [Some(3), Some(1)]);
        back.state.rack.note_lock[NoteSrc::Typing as usize] = None;
        assert!(back.unsaved(), "an unlock is an unsaved change");
        back.save_to(lemon_path.clone());
        let mut again = App::new(None);
        again.open(lemon_path);
        assert_eq!(again.state.rack.note_lock, [None, Some(1)]);
        // A lock on a channel the file does not have is left out.
        let mut f = file.clone();
        lemon::to_lemon(&mut f, &[Event::new(lemon::NOTE_LOCKS, Value::var([LAYOUT, 1, 0, 0, 0, 0, 99, 0]))]).unwrap();
        let mut s = with_state();
        apply(&mut s.state, &f);
        assert_eq!(s.state.rack.note_lock, [None, None]);
        again.open(flp_path);
        assert_eq!(again.state.rack.note_lock, [None, None], "an FLP has no locks");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// An FLP's icon numbers (a channel's event 155, a mixer track's event 95) open as the icons with the same picture
    /// (`playlist::FLP_ICONS`), other numbers as none. Saved as .lemon, the FLP events keep the file's numbers and the
    /// icons come back from event 193, so one taken off in Lemon Studio stays off.
    #[test]
    fn flp_icon_numbers_open_as_icons() {
        let d = dir("flp_icons");
        let path = d.join("Icons.flp");
        let mut made = App::new(None);
        made.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        made.state.session.control.settle(made.state.session.project.get_mut());
        let m = &mut made.state.session.project.get_mut().mixer;
        m[2].icon = Some(75);
        m[3].icon = Some(74); // the automation clip's nodes: no picture of Lemon Studio's
        m[4].icon = Some(80);
        made.save_to(path.clone());
        let mut file = os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap();
        // The second channel's icon number, before its colour: 30, the microphone.
        let ev = file.chunks.iter_mut().find_map(|c| match &mut c.body {
            os_flp::ChunkBody::Events(v) => Some(v),
            _ => None,
        });
        let ev = ev.unwrap();
        let second = ev.iter().enumerate().filter(|(_, e)| e.id == 64).nth(1).unwrap().0;
        let color = second + ev[second..].iter().position(|e| e.id == 128).unwrap();
        ev.insert(color, Event::new(155, Value::U32(30)));
        std::fs::write(&path, file.to_bytes().unwrap()).unwrap();
        let mut app = App::new(None);
        app.open(path);
        let s = &app.state;
        let (first, second) = (s.session.project.channels[0].id.0, s.session.project.channels[1].id.0);
        assert_eq!(s.session.project.channels[1].rack.icon, 30);
        assert_eq!(crate::playlist::icons(&s.session.project).get(&IconOwner::Channel(second)), Some(&Icon::Mic));
        assert_eq!(crate::playlist::icons(&s.session.project).get(&IconOwner::Channel(first)), None);
        assert_eq!(crate::playlist::icons(&s.session.project).get(&IconOwner::Track(2)), Some(&Icon::Wave));
        assert_eq!(crate::playlist::icons(&s.session.project).get(&IconOwner::Track(4)), Some(&Icon::Kick));
        assert_eq!(crate::playlist::icons(&s.session.project).len(), 3, "74 has no picture here");
        assert_eq!(crate::playlist::window_icon(&s.session.project, 1), Some(Icon::Mic));
        assert!(!app.unsaved(), "the icons are the opened state");
        // Saved as .lemon with the track 2 icon taken off: the FLP events keep the original numbers.
        crate::edit::playlist::put_icon(&mut app.state.session, IconOwner::Track(2), None);
        let lemon_path = d.join("Icons.lemon");
        app.save_to(lemon_path.clone());
        let saved = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&lemon_path).unwrap()).unwrap()).unwrap();
        assert_eq!((saved.channels[1].rack.icon, saved.mixer[2].icon, saved.mixer[3].icon), (30, Some(75), Some(74)));
        let mut back = App::new(None);
        back.open(lemon_path);
        let s = &back.state;
        assert_eq!(crate::playlist::icons(&s.session.project).get(&IconOwner::Channel(s.session.project.channels[1].id.0)), Some(&Icon::Mic));
        assert_eq!(crate::playlist::icons(&s.session.project).get(&IconOwner::Track(2)), None, "event 193 holds the choice, not event 95");
        assert_eq!(crate::playlist::icons(&s.session.project).len(), 2);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A target named .flp has no place for the state: the save succeeds, the file is a plain FLP, and opening it gives
    /// the defaults.
    #[test]
    fn an_flp_target_drops_the_state() {
        let d = dir("flp");
        let path = d.join("State.flp");
        let mut app = with_state();
        app.save_to(path.clone());
        assert!(app.state.message.as_deref().is_some_and(|m| m.starts_with("Saved")), "{:?}", app.state.message);
        let file = os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap();
        assert!(!lemon::is_lemon(&file) && lemon::events(&file).unwrap().is_empty());
        let mut back = App::new(None);
        back.open(path);
        let s = &back.state;
        assert!(crate::playlist::icons(&s.session.project).is_empty() && crate::mixer::slot_names_of(&s.session.project).is_empty() && crate::mixer::slot_colors_of(&s.session.project).is_empty());
        let fresh = crate::playlist::PlaylistState::default();
        assert_eq!((s.playlist.perf_clip_progress, s.playlist.perf_velocity_subclips), (fresh.perf_clip_progress, fresh.perf_velocity_subclips));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A file naming a feature this version does not know opens saying that part does not play here; saved again,
    /// it still names it (its part is kept, as unknown events are). A file of this version names none.
    #[test]
    fn features_this_version_does_not_know() {
        let d = dir("features");
        let path = d.join("Newer.lemon");
        let mut app = with_state();
        app.save_to(path.clone());
        let file = os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(lemon::data(&file, lemon::FEATURES), Some(vec![LAYOUT, 0, 0, 0, 0]), "none used");
        let mut file = file;
        let mut names = start(2);
        for n in ["Tempo curves", "Clip stretching"] {
            names.extend_from_slice(&(n.len() as u16).to_le_bytes());
            names.extend_from_slice(n.as_bytes());
        }
        lemon::to_lemon(&mut file, &[Event::new(lemon::FEATURES, Value::var(names))]).unwrap();
        std::fs::write(&path, file.to_bytes().unwrap()).unwrap();
        let mut back = App::new(None);
        back.open(path.clone());
        let message = back.state.message.clone().unwrap_or_default();
        assert!(message.ends_with("This version doesn't play: Tempo curves, Clip stretching"), "{message}");
        back.save_to(path.clone());
        let file = os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(unknown_features(&file), ["Tempo curves", "Clip stretching"], "kept through a save");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// An `LMex` event this version does not know is kept through open and save; one of ours with a later layout is
    /// left unread, and a payload cut short keeps the entries before the cut.
    #[test]
    fn unknown_events_and_layouts() {
        let d = dir("unknown");
        let path = d.join("State.lemon");
        let mut app = with_state();
        app.save_to(path.clone());
        let mut file = os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap();
        let future = Event::new(240, Value::var([7, 7]));
        let later = Event::new(lemon::PERFORMANCE, Value::var([2, 0]));
        lemon::to_lemon(&mut file, &[future.clone(), later]).unwrap();
        std::fs::write(&path, file.to_bytes().unwrap()).unwrap();
        let mut back = App::new(None);
        back.open(path.clone());
        assert!(back.state.playlist.perf_clip_progress, "a layout 2 Performance event is not read: the defaults stay");
        assert_eq!(crate::playlist::icons(&back.state.session.project).len(), 3);
        back.save_to(path.clone());
        let file = os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(lemon::data(&file, 240), Some(vec![7, 7]), "the unknown event is kept");
        // Two colours, the second cut short: the first is read.
        let mut s = with_state();
        crate::edit::mixer::set_slot_color(&mut s.state.session, 3, 4, Some(os_model::Rgb::new(1, 2, 3)));
        let mut b = slot_colors(&s.state.session).data().unwrap().to_vec();
        b.truncate(b.len() - 1);
        for (t, k) in crate::mixer::slot_colors_of(&s.state.session.project).into_keys() {
            crate::edit::mixer::set_slot_color(&mut s.state.session, t, k, None);
        }
        let mut f = file.clone();
        lemon::to_lemon(&mut f, &[Event::new(lemon::SLOT_COLORS, Value::var(b))]).unwrap();
        apply(&mut s.state, &f);
        assert_eq!(crate::mixer::slot_colors_of(&s.state.session.project).len(), 1);
        assert_eq!(crate::mixer::slot_colors_of(&s.state.session.project).get(&(2, 0)), Some(&os_model::Rgb::new(10, 200, 30)));
        let _ = std::fs::remove_dir_all(&d);
    }
}
