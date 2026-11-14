// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::track_links` (step 9: the session's side stays there).

#![allow(unused_imports)]
#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use os_model::{ChannelKind, ClipSource, TrackLink};
use crate::app::State;
use crate::playlist::TRACKS;
use crate::edit::playlist::{arr_mut, clips, track_info};
#[allow(unused_imports)]
use crate::track_links::*;

/// Assign to new Audio track(s) (spec: ui/mixer-track-commands.md §8): each of `mixers` without an audio track gets
/// one on a new playlist track (`new_track`), named after the mixer track. The playlist selects the last one.
pub fn assign_audio(s: &mut State, mixers: &[u16]) -> Option<String> {
    let todo = assignable(&s.session, mixers);
    let mut made = Vec::new();
    for m in todo {
        let Some(t) = new_track(&s.session) else { break };
        let name = crate::edit::mixer::name(&s.session, m);
        set_link(&mut s.session, t, TrackLink::audio(m));
        arr_mut(&mut s.session).tracks[usize::from(t)].name = Some(name);
        made.push((m, t));
    }
    let &(m, t) = made.last()?;
    s.playlist.track = t;
    s.session.dirty = true;
    Some(match made.len() {
        1 => format!("Assigned {} to {}", crate::edit::mixer::name(&s.session, m), crate::edit::playlist::track_name(&s.session, t)),
        n => format!("Assigned {n} mixer tracks to new audio tracks"),
    })
}

/// Assign to new instrument track (spec: ui/channel-rack.md §18): each channel of `rows` gets a new playlist track
/// (`new_track`) linked to it, named and coloured after it; the channel's mixer insert is named after it too (the
/// master keeps its name), as the original does.
pub fn assign_instrument(s: &mut State, rows: &[usize]) -> Option<String> {
    let todo = instrument_rows(&s.session, rows);
    let mut made = Vec::new();
    for r in todo {
        let Some(t) = new_track(&s.session) else { break };
        link_instrument(&mut s.session, t, r);
        made.push((r, t));
    }
    let &(r, t) = made.last()?;
    s.playlist.track = t;
    s.session.dirty = true;
    Some(match made.len() {
        1 => format!("Assigned {} to {}", crate::edit::rack::channel_name(&s.session, r), crate::edit::playlist::track_name(&s.session, t)),
        n => format!("Assigned {n} channels to new instrument tracks"),
    })
}

/// Track mode ▸ on track `t`: an audio track takes its mixer track's name, an instrument track its channel's name and
/// colour; Unassigned keeps the name.
pub fn set_mode(s: &mut State, t: u16, mode: Mode) -> Option<String> {
    match mode {
        Mode::Unassigned => set_link(&mut s.session, t, TrackLink::NONE),
        Mode::Audio(_) | Mode::NewAudio => {
            let m = match mode {
                Mode::Audio(m) => m,
                _ => crate::mixer::add_track(s, None),
            };
            let name = crate::edit::mixer::name(&s.session, m);
            set_link(&mut s.session, t, TrackLink::audio(m));
            arr_mut(&mut s.session).tracks[usize::from(t)].name = Some(name);
        }
        Mode::Instrument(r) => link_instrument(&mut s.session, t, r),
        // Needs the app to host the plugin (`new_instrument`).
        Mode::NewInstrument(_) => return None,
    }
    s.session.dirty = true;
    None
}

/// Track mode ▸ Instrument track ▸ and a generator (spec: ui/playlist.md §6c, measured): a new channel with that plugin,
/// as ADD makes one (`plugin_menu::add_generator_now`), and track `t` becomes its instrument track, named and coloured
/// after it, its insert named after it. One undo step, "Playlist track mode".
pub fn new_instrument(app: &mut crate::app::App, t: u16, index: usize) -> Option<String> {
    crate::control_ui::edit_app(app, TRACK_MODE, crate::control::command::Region::Project, |app| {
        let rows = app.state.session.project.channels.len();
        let msg = crate::plugin_menu::add_generator_now(app, index, None);
        if app.state.session.project.channels.len() == rows {
            return msg;
        }
        let row = app.state.selected_channel;
        link_instrument(&mut app.state.session, t, row);
        app.state.playlist.track = t;
        app.state.session.dirty = true;
        Some(format!("Added {} on {}", crate::edit::rack::channel_name(&app.state.session, row), crate::edit::playlist::track_name(&app.state.session, t)))
    })
}

/// The Track mode ▸ submenu for track `t`: Unassigned, Audio track ▸ (a new mixer track, then every insert), Instrument
/// track ▸ (every channel an instrument track can play); on an audio track also its options, which act on its mixer
/// track: Select audio input ▸ (as the mixer track's input menu, the current input marked), Arm recording (ticked while
/// armed) and Disarm recording (spec: ui/playlist.md §6c, measured).
pub fn mode_items(s: &State, t: u16, cmd: impl Fn(Mode) -> crate::playlist::Cmd) -> Vec<crate::playlist::PItem> {
    use crate::playlist::{Mark, PItem};
    let l = link(&s.session, t);
    let mut audio = vec![PItem::new("New mixer track", cmd(Mode::NewAudio)), PItem::sep()];
    audio.extend(
        (1..=crate::edit::mixer::inserts(&s.session)).map(|m| PItem::new(format!("{m}. {}", crate::edit::mixer::name(&s.session, m)), cmd(Mode::Audio(m))).mark(Mark::Check(l.audio_mixer() == Some(m)))),
    );
    let chans: Vec<PItem> = s
        .session.project
        .channels
        .iter()
        .enumerate()
        .filter(|(_, c)| instrument_kind(c.kind))
        .map(|(r, c)| PItem::new(crate::edit::rack::channel_name(&s.session, r), cmd(Mode::Instrument(r))).mark(Mark::Check(l.instrument_channel() == Some(c.index))))
        .collect();
    // As the original's: Use existing channel ▸, then the generators, each making a new channel (§6c, measured).
    let existing = if chans.is_empty() { PItem::new("Use existing channel", crate::playlist::Cmd::Nothing).off() } else { PItem::sub("Use existing channel", chans) };
    let mut chans = vec![existing, PItem::sep()];
    chans.extend(crate::plugin_menu::track_generators(s, t));
    let mut items = vec![
        PItem::new("Unassigned", cmd(Mode::Unassigned)).mark(Mark::Check(l.is_none())),
        PItem::sub("Audio track", audio).mark(Mark::Check(l.kind == TrackLink::AUDIO)),
        PItem::sub("Instrument track", chans).mark(Mark::Check(l.kind == TrackLink::INSTRUMENT)),
    ];
    if let Some(m) = l.audio_mixer() {
        use crate::mixer::MCmd;
        use crate::playlist::Cmd;
        let inputs = crate::mixer::input_items(&s.session, m, true);
        let armed = crate::edit::mixer::track(&s.session, m).is_some_and(|x| x.armed);
        items.extend([
            PItem::head("Audio track options"),
            match inputs {
                Some(i) => PItem::sub("Select audio input", i),
                None => PItem::new("Select audio input", Cmd::Nothing).off(),
            },
            PItem::new("Arm recording", Cmd::Mixer(MCmd::ArmTrack(m, true))).mark(Mark::Check(armed)),
            PItem::new("Disarm recording", Cmd::Mixer(MCmd::ArmTrack(m, false))),
        ]);
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::mixer::MCmd;
    use crate::playlist::Cmd;
    use crate::rack::RCmd;
    use os_model::{Clip, Id};

    fn clip(track: u16, position: u32, source: ClipSource) -> Clip {
        Clip { id: Id::NONE, position, length: 96, track, source, muted: false, window: None, raw: Clip::TEMPLATE }
    }

    /// Kick, Clap, Hat, Snare and Pluck on Inserts 1-5.
    fn app() -> App {
        let mut app = App::new(None);
        app.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        app
    }

    fn entry(items: &[crate::playlist::PItem], label: &str) -> crate::playlist::PItem {
        items.iter().find(|i| i.label == label).unwrap_or_else(|| panic!("{label}")).clone()
    }

    /// Assign to new Audio track(s) (ui/mixer-track-commands.md §8): the first free track (no clips, link or name) is
    /// linked to the insert and named after it; the entry is then greyed and Unassign enabled. Unassign keeps the name.
    #[test]
    fn assign_and_unassign_audio() {
        let mut app = app();
        let menu = |app: &App, n| crate::mixer::track_menu(&app.state, n, 0.0, 0.0).items;
        assert!(entry(&menu(&app, 3), "Assign to new Audio track(s)").enabled);
        assert!(!entry(&menu(&app, 3), "Unassign from Audio track(s)").enabled, "nothing linked");
        // Track 1 has a clip and track 2 a name: the measured run skipped both.
        crate::edit::playlist::arr_mut(&mut app.state.session).clips.push(clip(0, 0, ClipSource::Pattern(1)));
        crate::edit::playlist::arr_mut(&mut app.state.session).tracks[1].name = Some("Drums".into());
        assert_eq!(crate::mixer::run(&mut app, MCmd::AssignAudio(3)).as_deref(), Some(format!("Assigned {0} to {0}", crate::edit::mixer::name(&app.state.session, 3)).as_str()));
        assert_eq!(link(&app.state.session, 2), TrackLink::audio(3));
        assert_eq!(crate::edit::playlist::track_name(&app.state.session, 2), crate::edit::mixer::name(&app.state.session, 3));
        assert_eq!(app.state.playlist.track, 2);
        assert_eq!(audio_tracks_of(&app.state.session, 3), [2]);
        assert!(!entry(&menu(&app, 3), "Assign to new Audio track(s)").enabled);
        assert!(entry(&menu(&app, 3), "Unassign from Audio track(s)").enabled);
        // The next one goes below it.
        crate::mixer::run(&mut app, MCmd::AssignAudio(5));
        assert_eq!(link(&app.state.session, 3), TrackLink::audio(5));
        crate::mixer::run(&mut app, MCmd::UnassignAudio(3));
        assert!(link(&app.state.session, 2).is_none());
        assert_eq!(crate::edit::playlist::track_name(&app.state.session, 2), crate::edit::mixer::name(&app.state.session, 3), "the name stays");
        assert_eq!(link(&app.state.session, 3), TrackLink::audio(5), "only that insert's");
        // One undo step each.
        crate::history_ui::undo(&mut app.state);
        assert_eq!(link(&app.state.session, 2), TrackLink::audio(3));
        crate::history_ui::undo(&mut app.state);
        assert!(link(&app.state.session, 3).is_none());
        crate::history_ui::undo(&mut app.state);
        assert!(link(&app.state.session, 2).is_none());
    }

    /// Assign to new instrument track (ui/channel-rack.md §18): linked to the channel, named and coloured after it,
    /// the channel's insert renamed too; greyed once the channel has one.
    #[test]
    fn assign_instrument_track() {
        let mut app = app();
        app.state.session.project.get_mut().channels[1].color = Some(os_model::Rgb::new(0xaa, 0x33, 0x55));
        let insert2 = crate::edit::mixer::name(&app.state.session, 2);
        let menu = |app: &App| crate::rack::channel_menu(&app.state, 1, 0.0, 0.0).items;
        assert!(entry(&menu(&app), "Assign to new instrument track").enabled);
        crate::rack::run(&mut app, RCmd::AssignInstrument(1));
        assert_eq!(link(&app.state.session, 0), TrackLink::instrument(1));
        assert_eq!(instrument_track_of(&app.state.session, 1), Some(0));
        let tr = track_info(&app.state.session, 0).unwrap();
        assert_eq!((tr.name.as_deref(), tr.color), (Some("Clap"), Some(os_model::Rgb::new(0xaa, 0x33, 0x55))));
        assert_eq!(crate::edit::mixer::name(&app.state.session, 2), "Clap", "the channel's insert takes its name");
        let insert = crate::edit::mixer::track(&app.state.session, 2).unwrap().color;
        assert_eq!(insert, Some(os_model::Rgb::new(0xaa, 0x33, 0x55)), "and its colour");
        assert_eq!(app.state.session.project.channels[1].mixer_track, 2, "its routing stays");
        assert!(!entry(&menu(&app), "Assign to new instrument track").enabled);
        crate::history_ui::undo(&mut app.state);
        assert!(link(&app.state.session, 0).is_none());
        assert_eq!(crate::edit::mixer::name(&app.state.session, 2), insert2);
    }

    /// Track mode ▸ (ui/playlist.md §6c): an insert, a new mixer track, a channel, Unassigned; each one undo step.
    #[test]
    fn track_mode() {
        let mut app = app();
        let inserts = crate::edit::mixer::inserts(&app.state.session);
        crate::playlist::run(&mut app, Cmd::TrackMode(4, Mode::Audio(2)));
        assert_eq!(link(&app.state.session, 4), TrackLink::audio(2));
        let items = mode_items(&app.state, 4, |m| Cmd::TrackMode(4, m));
        assert_eq!(items[1].mark, crate::playlist::Mark::Check(true));
        assert_eq!(entry(&items[1].children, &format!("2. {}", crate::edit::mixer::name(&app.state.session, 2))).mark, crate::playlist::Mark::Check(true));
        crate::playlist::run(&mut app, Cmd::TrackMode(5, Mode::NewAudio));
        assert_eq!(link(&app.state.session, 5), TrackLink::audio(inserts + 1), "a mixer track added for it");
        assert_eq!(crate::edit::playlist::track_name(&app.state.session, 5), crate::edit::mixer::name(&app.state.session, inserts + 1));
        // Audio to instrument: the channel's insert takes its name.
        crate::playlist::run(&mut app, Cmd::TrackMode(4, Mode::Instrument(3)));
        assert_eq!(link(&app.state.session, 4), TrackLink::instrument(3));
        assert_eq!(crate::edit::playlist::track_name(&app.state.session, 4), "Snare");
        crate::playlist::run(&mut app, Cmd::TrackMode(4, Mode::Unassigned));
        assert!(link(&app.state.session, 4).is_none());
        assert_eq!(crate::edit::playlist::track_name(&app.state.session, 4), "Snare");
        crate::history_ui::undo(&mut app.state);
        assert_eq!(link(&app.state.session, 4), TrackLink::instrument(3));
    }

    /// Track mode ▸ Instrument track ▸ (ui/playlist.md §6c, measured): Use existing channel ▸ first, then the generators;
    /// a generator makes a new channel whose instrument track the track becomes, one undo step.
    #[test]
    fn instrument_track_with_a_new_channel() {
        let mut app = app();
        let items = mode_items(&app.state, 6, |m| Cmd::TrackMode(6, m));
        let inst = entry(&items, "Instrument track");
        assert_eq!(inst.children[0].label, "Use existing channel");
        assert!(entry(&inst.children[0].children, "Snare").enabled);
        app.state.session.plugins.available = crate::plugins::builtin();
        let i = app.state.session.plugins.index_of(crate::plugins::LEMON_SAMPLER).expect("the built-in sampler");
        let rows = app.state.session.project.channels.len();
        let steps = app.state.session.history.at();
        let msg = crate::playlist::run(&mut app, Cmd::TrackMode(6, Mode::NewInstrument(i)));
        assert_eq!(app.state.session.project.channels.len(), rows + 1);
        let ch = app.state.session.project.channels[rows].index;
        assert_eq!(link(&app.state.session, 6), TrackLink::instrument(ch));
        let name = crate::edit::rack::channel_name(&app.state.session, rows);
        assert_eq!(crate::edit::playlist::track_name(&app.state.session, 6), name);
        assert_eq!(msg, Some(format!("Added {name} on {name}")));
        assert_eq!((app.state.session.history.at(), app.state.session.history.newest_done()), (steps + 1, Some(TRACK_MODE)));
        crate::history_ui::undo(&mut app.state);
        assert_eq!(app.state.session.project.channels.len(), rows);
        assert!(link(&app.state.session, 6).is_none());
    }

    /// An audio track's options in Track mode ▸ (ui/playlist.md §6c, measured): only on an audio track; Arm recording and
    /// Disarm recording act on its mixer track alone, whatever the mixer's selection, and the entry is ticked while it is
    /// armed.
    #[test]
    fn audio_track_options() {
        let mut app = app();
        let labels = |app: &App, t| mode_items(&app.state, t, |m| Cmd::TrackMode(t, m)).iter().map(|i| i.label.to_string()).collect::<Vec<_>>();
        assert!(!labels(&app, 2).contains(&"Arm recording".to_string()), "not on a plain track");
        crate::edit::playlist::arr_mut(&mut app.state.session).tracks[2].link = TrackLink::audio(3);
        assert_eq!(labels(&app, 2)[3..], ["Audio track options", "Select audio input", "Arm recording", "Disarm recording"]);
        // The mixer has Inserts 3 and 4 selected: the audio track's option arms Insert 3 only.
        app.state.mixer.sel = [3, 4].into();
        let arm = entry(&mode_items(&app.state, 2, |m| Cmd::TrackMode(2, m)), "Arm recording");
        assert_eq!((arm.cmd.clone(), arm.mark), (Cmd::Mixer(MCmd::ArmTrack(3, true)), crate::playlist::Mark::Check(false)));
        crate::mixer::run(&mut app, MCmd::ArmTrack(3, true));
        let armed = |app: &App, m| crate::edit::mixer::track(&app.state.session, m).is_some_and(|x| x.armed);
        assert!(armed(&app, 3) && !armed(&app, 4));
        assert_eq!(entry(&mode_items(&app.state, 2, |m| Cmd::TrackMode(2, m)), "Arm recording").mark, crate::playlist::Mark::Check(true));
        crate::mixer::run(&mut app, MCmd::ArmTrack(3, false));
        assert!(!armed(&app, 3));
    }


    /// Audio clips (ui/playlist.md §6c): one landing on an audio track plays through its mixer track; an instrument
    /// track takes none (a drop goes to the next free track below it).
    #[test]
    fn audio_clips_follow_the_track() {
        let mut app = app();
        let s = &mut app.state;
        s.session.project.get_mut().channels[4].kind = ChannelKind::AudioClip;
        let audio = ClipSource::Channel(s.session.project.channels[4].index);
        crate::edit::playlist::arr_mut(&mut s.session).tracks[2].link = TrackLink::audio(7);
        crate::edit::playlist::arr_mut(&mut s.session).tracks[5].link = TrackLink::instrument(0);
        assert!(!fits(&s.session, audio, 5) && fits(&s.session, audio, 2) && fits(&s.session, ClipSource::Pattern(1), 5));
        crate::edit::playlist::arr_mut(&mut s.session).clips.push(clip(6, 0, ClipSource::Pattern(1)));
        assert_eq!(audio_drop_track(&s.session, 5, 0, 96), 7, "below the instrument track, past the busy track");
        assert_eq!(audio_drop_track(&s.session, 4, 0, 96), 4);
        crate::edit::playlist::arr_mut(&mut s.session).clips.push(clip(2, 0, audio));
        let i = clips(&s.session).len() - 1;
        route_clips(&mut s.session, &[i]);
        assert_eq!(s.session.project.channels[4].mixer_track, 7);
        // Off the audio track: the channel stays where it is.
        crate::edit::playlist::arr_mut(&mut s.session).clips[i].track = 3;
        route_clips(&mut s.session, &[i]);
        assert_eq!(s.session.project.channels[4].mixer_track, 7);
        // A recorded take goes to the mixer track's audio track while it is free there.
        assert_eq!(record_track(&s.session, 7, 200, 300), 2);
        assert_eq!(tracks_routed_to(&s.session, &[7]), [2]);
        assert_eq!(tracks_routed_to(&s.session, &[1]), [5], "the instrument track of a channel on Insert 1");
    }

    /// Deleting a linked mixer track or channel unlinks its tracks; the others follow the renumbering.
    #[test]
    fn links_follow_deletes() {
        let mut app = app();
        let s = &mut app.state;
        crate::edit::playlist::arr_mut(&mut s.session).tracks[0].link = TrackLink::audio(3);
        crate::edit::playlist::arr_mut(&mut s.session).tracks[1].link = TrackLink::audio(5);
        crate::edit::playlist::arr_mut(&mut s.session).tracks[2].link = TrackLink::instrument(1);
        crate::edit::playlist::arr_mut(&mut s.session).tracks[3].link = TrackLink::instrument(4);
        crate::mixer::delete_track(s, 3);
        assert!(link(&s.session, 0).is_none());
        assert_eq!(link(&s.session, 1), TrackLink::audio(4));
        crate::rack::remap_project(s.session.project.get_mut(), &[Some(0), None, Some(1), Some(2), Some(3)]);
        assert!(link(&s.session, 2).is_none());
        assert_eq!(link(&s.session, 3), TrackLink::instrument(3));
    }
}
