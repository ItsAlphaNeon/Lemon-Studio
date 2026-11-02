// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Audio tracks and instrument tracks: playlist tracks linked to a mixer track or to a channel. The link is the track
//! record's (`os_model::TrackLink`); this module makes and drops links (the mixer's Assign to new Audio track(s) and
//! Unassign from Audio track(s), the rack's Assign to new instrument track, the playlist track menu's Track mode ▸),
//! and keeps clips to their track's kind: an audio clip placed or moved onto an audio track plays through its mixer
//! track, and an instrument track takes no audio clips.
//!
// spec: ui/playlist.md §6c, ui/mixer-track-commands.md §8, ui/channel-rack.md §18, flp/events/playlist.md

use os_model::{ChannelKind, ClipSource, TrackLink};

use crate::app::State;
use crate::playlist::{arr_mut, clips, track_info, TRACKS};

/// Undo step names: the original's History entry for linking (spec: ui/one-click-recording.md §1), and Lemon Studio's
/// for the others.
pub const ASSIGN_AUDIO: &str = "Assign to audio track(s)";
pub const UNASSIGN_AUDIO: &str = "Unassign audio track(s)";
pub const ASSIGN_INSTRUMENT: &str = "Assign to instrument track";
pub const TRACK_MODE: &str = "Playlist track mode";

/// The original's message when an audio clip is dropped on an instrument track (it then offers the next empty track).
pub const NOT_FOR_AUDIO: &str = "Instrument tracks are not for audio clips";

/// Playlist track `t`'s link in the current arrangement.
pub fn link(s: &State, t: u16) -> TrackLink {
    track_info(s, t).map_or(TrackLink::NONE, |x| x.link)
}

/// The mixer track audio track `t` plays through.
pub fn audio_mixer(s: &State, t: u16) -> Option<u16> {
    link(s, t).audio_mixer()
}

pub fn is_instrument(s: &State, t: u16) -> bool {
    link(s, t).kind == TrackLink::INSTRUMENT
}

fn set_link(s: &mut State, t: u16, l: TrackLink) {
    if let Some(tr) = arr_mut(s).tracks.get_mut(usize::from(t)) {
        tr.link = l;
    }
}

/// The track a new audio or instrument track takes: the first one with no clips, no link and no name (measured: the
/// original skipped a track it had named, one with clips and linked ones).
pub fn new_track(s: &State) -> Option<u16> {
    (0..TRACKS).find(|&t| {
        let info = track_info(s, t);
        info.is_none_or(|x| x.link.is_none() && x.name.as_deref().is_none_or(str::is_empty)) && !clips(s).iter().any(|c| c.track == t)
    })
}

/// The audio tracks linked to mixer track `m`, top to bottom.
pub fn audio_tracks_of(s: &State, m: u16) -> Vec<u16> {
    (0..TRACKS).filter(|&t| audio_mixer(s, t) == Some(m)).collect()
}

/// The instrument track of channel `ch` (its index), the first when there are several.
pub fn instrument_track_of(s: &State, ch: u16) -> Option<u16> {
    (0..TRACKS).find(|&t| link(s, t).instrument_channel() == Some(ch))
}

/// Mixer tracks Assign to new Audio track(s) can take: inserts not yet linked (the master is left out).
fn assignable(s: &State, mixers: &[u16]) -> Vec<u16> {
    mixers.iter().copied().filter(|&m| m >= 1 && m <= crate::mixer::inserts(s) && audio_tracks_of(s, m).is_empty()).collect()
}

/// The mixer track menu's Assign to new Audio track(s) is greyed when none of `mixers` can take one (the original
/// greys it on a linked track).
pub fn can_assign_audio(s: &State, mixers: &[u16]) -> bool {
    !assignable(s, mixers).is_empty()
}

pub fn can_unassign_audio(s: &State, mixers: &[u16]) -> bool {
    mixers.iter().any(|&m| !audio_tracks_of(s, m).is_empty())
}

/// Assign to new Audio track(s) (spec: ui/mixer-track-commands.md §8): each of `mixers` without an audio track gets
/// one on a new playlist track (`new_track`), named after the mixer track. The playlist selects the last one.
pub fn assign_audio(s: &mut State, mixers: &[u16]) -> Option<String> {
    let todo = assignable(s, mixers);
    let mut made = Vec::new();
    for m in todo {
        let Some(t) = new_track(s) else { break };
        let name = crate::mixer::name(s, m);
        set_link(s, t, TrackLink::audio(m));
        arr_mut(s).tracks[usize::from(t)].name = Some(name);
        made.push((m, t));
    }
    let &(m, t) = made.last()?;
    s.playlist.track = t;
    s.piano.dirty = true;
    Some(match made.len() {
        1 => format!("Assigned {} to {}", crate::mixer::name(s, m), crate::playlist::track_name(s, t)),
        n => format!("Assigned {n} mixer tracks to new audio tracks"),
    })
}

/// Unassign from Audio track(s): the audio tracks of `mixers` become plain tracks; names, colours and clips stay.
pub fn unassign_audio(s: &mut State, mixers: &[u16]) -> Option<String> {
    let tracks: Vec<u16> = mixers.iter().flat_map(|&m| audio_tracks_of(s, m)).collect();
    for &t in &tracks {
        set_link(s, t, TrackLink::NONE);
    }
    s.piano.dirty = true;
    match tracks.as_slice() {
        [] => Some("No audio track to unassign".into()),
        [t] => Some(format!("Unassigned {}", crate::playlist::track_name(s, *t))),
        v => Some(format!("Unassigned {} audio tracks", v.len())),
    }
}

/// A channel an instrument track can play: a sampler, plugin or layer channel (not an audio or automation clip's).
fn instrument_kind(k: ChannelKind) -> bool {
    !matches!(k, ChannelKind::AudioClip | ChannelKind::Automation)
}

/// Rack rows Assign to new instrument track can take: instrument channels without an instrument track.
fn instrument_rows(s: &State, rows: &[usize]) -> Vec<usize> {
    rows.iter()
        .copied()
        .filter(|&r| s.project.channels.get(r).is_some_and(|c| instrument_kind(c.kind) && instrument_track_of(s, c.index).is_none()))
        .collect()
}

pub fn can_assign_instrument(s: &State, rows: &[usize]) -> bool {
    !instrument_rows(s, rows).is_empty()
}

/// Assign to new instrument track (spec: ui/channel-rack.md §18): each channel of `rows` gets a new playlist track
/// (`new_track`) linked to it, named and coloured after it; the channel's mixer insert is named after it too (the
/// master keeps its name), as the original does.
pub fn assign_instrument(s: &mut State, rows: &[usize]) -> Option<String> {
    let todo = instrument_rows(s, rows);
    let mut made = Vec::new();
    for r in todo {
        let Some(t) = new_track(s) else { break };
        link_instrument(s, t, r);
        made.push((r, t));
    }
    let &(r, t) = made.last()?;
    s.playlist.track = t;
    s.piano.dirty = true;
    Some(match made.len() {
        1 => format!("Assigned {} to {}", crate::rack::channel_name(s, r), crate::playlist::track_name(s, t)),
        n => format!("Assigned {n} channels to new instrument tracks"),
    })
}

/// Track `t` becomes the instrument track of rack row `r`, named and coloured after the channel.
fn link_instrument(s: &mut State, t: u16, r: usize) {
    let Some(ch) = s.project.channels.get(r) else { return };
    let (index, color, mixer) = (ch.index, ch.color, ch.mixer_track);
    let name = crate::rack::channel_name(s, r);
    set_link(s, t, TrackLink::instrument(index));
    let tr = &mut arr_mut(s).tracks[usize::from(t)];
    tr.name = Some(name.clone());
    if color.is_some() {
        tr.color = color;
    }
    // The channel's insert takes its name and colour (measured: Assign to new instrument track on 808 Clap wrote both to
    // Insert 2, events 204 and 149); the master keeps its own.
    if mixer != 0 {
        if let Some(m) = crate::mixer::track_mut(s, mixer) {
            m.name = Some(name);
            if color.is_some() {
                m.color = color;
            }
        }
    }
}

/// The playlist track menu's Track mode ▸ (spec: ui/playlist.md §6c).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Unassigned,
    /// An audio track on this mixer track.
    Audio(u16),
    /// An audio track on a mixer track added for it.
    NewAudio,
    /// An instrument track of this rack row.
    Instrument(usize),
    /// An instrument track of a new channel playing generator `.0` (an index of the available plugins).
    NewInstrument(usize),
}

/// Track mode ▸ on track `t`: an audio track takes its mixer track's name, an instrument track its channel's name and
/// colour; Unassigned keeps the name.
pub fn set_mode(s: &mut State, t: u16, mode: Mode) -> Option<String> {
    match mode {
        Mode::Unassigned => set_link(s, t, TrackLink::NONE),
        Mode::Audio(_) | Mode::NewAudio => {
            let m = match mode {
                Mode::Audio(m) => m,
                _ => crate::mixer::add_track(s, None),
            };
            let name = crate::mixer::name(s, m);
            set_link(s, t, TrackLink::audio(m));
            arr_mut(s).tracks[usize::from(t)].name = Some(name);
        }
        Mode::Instrument(r) => link_instrument(s, t, r),
        // Needs the app to host the plugin (`new_instrument`).
        Mode::NewInstrument(_) => return None,
    }
    s.piano.dirty = true;
    None
}

/// Track mode ▸ Instrument track ▸ and a generator (spec: ui/playlist.md §6c, measured): a new channel with that plugin,
/// as ADD makes one (`plugin_menu::add_generator_now`), and track `t` becomes its instrument track, named and coloured
/// after it, its insert named after it. One undo step, "Playlist track mode".
pub fn new_instrument(app: &mut crate::app::App, t: u16, index: usize) -> Option<String> {
    crate::control::edit_app(app, TRACK_MODE, crate::control::command::Region::Project, |app| {
        let rows = app.state.project.channels.len();
        let msg = crate::plugin_menu::add_generator_now(app, index, None);
        if app.state.project.channels.len() == rows {
            return msg;
        }
        let row = app.state.selected_channel;
        link_instrument(&mut app.state, t, row);
        app.state.playlist.track = t;
        app.state.piano.dirty = true;
        Some(format!("Added {} on {}", crate::rack::channel_name(&app.state, row), crate::playlist::track_name(&app.state, t)))
    })
}

/// The Track mode ▸ submenu for track `t`: Unassigned, Audio track ▸ (a new mixer track, then every insert), Instrument
/// track ▸ (every channel an instrument track can play); on an audio track also its options, which act on its mixer
/// track: Select audio input ▸ (as the mixer track's input menu, the current input marked), Arm recording (ticked while
/// armed) and Disarm recording (spec: ui/playlist.md §6c, measured).
pub fn mode_items(s: &State, t: u16, cmd: impl Fn(Mode) -> crate::playlist::Cmd) -> Vec<crate::playlist::PItem> {
    use crate::playlist::{Mark, PItem};
    let l = link(s, t);
    let mut audio = vec![PItem::new("New mixer track", cmd(Mode::NewAudio)), PItem::sep()];
    audio.extend(
        (1..=crate::mixer::inserts(s)).map(|m| PItem::new(format!("{m}. {}", crate::mixer::name(s, m)), cmd(Mode::Audio(m))).mark(Mark::Check(l.audio_mixer() == Some(m)))),
    );
    let chans: Vec<PItem> = s
        .project
        .channels
        .iter()
        .enumerate()
        .filter(|(_, c)| instrument_kind(c.kind))
        .map(|(r, c)| PItem::new(crate::rack::channel_name(s, r), cmd(Mode::Instrument(r))).mark(Mark::Check(l.instrument_channel() == Some(c.index))))
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
        let inputs = crate::mixer::input_items(s, m, true);
        let armed = crate::mixer::track(s, m).is_some_and(|x| x.armed);
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

/// Whether a clip of `src` may sit on track `t`: an instrument track takes no audio clips.
pub fn fits(s: &State, src: ClipSource, t: u16) -> bool {
    !(is_instrument(s, t) && is_audio_source(s, src))
}

fn is_audio_source(s: &State, src: ClipSource) -> bool {
    matches!(src, ClipSource::Channel(ch) if s.project.channel(ch).is_some_and(|c| c.kind == ChannelKind::AudioClip))
}

/// The track an audio clip dropped on `t` over ticks `a..b` goes to: `t`, or for an instrument track the first track
/// below it that is free over the span and not an instrument track (the original asks first, then does this).
pub fn audio_drop_track(s: &State, t: u16, a: u32, b: u32) -> u16 {
    if !is_instrument(s, t) {
        return t;
    }
    (t + 1..TRACKS)
        .find(|&k| !is_instrument(s, k) && !clips(s).iter().any(|c| c.track == k && c.position < b && c.end() > a))
        .unwrap_or(t)
}

/// Audio clips `idx` (of the current arrangement) on audio tracks: their channels go to the track's mixer track, as
/// the original reroutes a clip's channel when it lands on an audio track (moving it off again leaves the channel
/// where it is).
pub fn route_clips(s: &mut State, idx: &[usize]) {
    let moves: Vec<(u16, u16)> = idx
        .iter()
        .filter_map(|&i| clips(s).get(i))
        .filter_map(|c| match c.source {
            ClipSource::Channel(ch) if is_audio_source(s, c.source) => audio_mixer(s, c.track).map(|m| (ch, m)),
            _ => None,
        })
        .collect();
    let max = crate::mixer::max_insert(&s.project);
    for (ch, m) in moves {
        if let Some(c) = s.project.channels.iter_mut().find(|c| c.index == ch) {
            if c.mixer_track != m && m <= max {
                c.mixer_track = m;
                s.piano.dirty = true;
            }
        }
    }
}

/// The playlist track a take recorded on mixer track `m` over ticks `a..b` goes to: `m`'s first audio track when it
/// is free over the span (recording into an audio track), else the first track free over it.
pub fn record_track(s: &State, m: u16, a: u32, b: u32) -> u16 {
    audio_tracks_of(s, m)
        .into_iter()
        .find(|&t| !clips(s).iter().any(|c| c.track == t && c.position < b && c.end() > a))
        .unwrap_or_else(|| crate::playlist::free_track(s, 0, a, b))
}

/// The track of a recorded take (clip `i`) becomes an audio track on the take channel's mixer track, unless it is
/// linked already.
pub fn bind_take(s: &mut State, i: usize) {
    let Some(c) = clips(s).get(i).copied() else { return };
    let ClipSource::Channel(ch) = c.source else { return };
    let Some(m) = s.project.channel(ch).map(|x| x.mixer_track) else { return };
    if link(s, c.track).is_none() && m != 0 {
        set_link(s, c.track, TrackLink::audio(m));
    }
}

/// Select ▸ Channels and playlist tracks routed to this track: the playlist tracks linked to `mixers` (audio tracks)
/// or to a channel using them (instrument tracks), top to bottom.
pub fn tracks_routed_to(s: &State, mixers: &[u16]) -> Vec<u16> {
    (0..TRACKS)
        .filter(|&t| {
            let l = link(s, t);
            l.audio_mixer().is_some_and(|m| mixers.contains(&m))
                || l.instrument_channel().and_then(|ch| s.project.channel(ch)).is_some_and(|c| mixers.contains(&c.mixer_track))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::mixer::MCmd;
    use crate::playlist::Cmd;
    use crate::rack::RCmd;
    use os_model::{Clip, Id};

    /// Kick, Clap, Hat, Snare and Pluck on Inserts 1-5.
    fn app() -> App {
        let mut app = App::new(None);
        app.state.project = crate::app::test_project();
        app
    }

    fn clip(track: u16, position: u32, source: ClipSource) -> Clip {
        Clip { id: Id::NONE, position, length: 96, track, source, muted: false, window: None, raw: Clip::TEMPLATE }
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
        crate::playlist::arr_mut(&mut app.state).clips.push(clip(0, 0, ClipSource::Pattern(1)));
        crate::playlist::arr_mut(&mut app.state).tracks[1].name = Some("Drums".into());
        assert_eq!(crate::mixer::run(&mut app, MCmd::AssignAudio(3)).as_deref(), Some(format!("Assigned {0} to {0}", crate::mixer::name(&app.state, 3)).as_str()));
        assert_eq!(link(&app.state, 2), TrackLink::audio(3));
        assert_eq!(crate::playlist::track_name(&app.state, 2), crate::mixer::name(&app.state, 3));
        assert_eq!(app.state.playlist.track, 2);
        assert_eq!(audio_tracks_of(&app.state, 3), [2]);
        assert!(!entry(&menu(&app, 3), "Assign to new Audio track(s)").enabled);
        assert!(entry(&menu(&app, 3), "Unassign from Audio track(s)").enabled);
        // The next one goes below it.
        crate::mixer::run(&mut app, MCmd::AssignAudio(5));
        assert_eq!(link(&app.state, 3), TrackLink::audio(5));
        crate::mixer::run(&mut app, MCmd::UnassignAudio(3));
        assert!(link(&app.state, 2).is_none());
        assert_eq!(crate::playlist::track_name(&app.state, 2), crate::mixer::name(&app.state, 3), "the name stays");
        assert_eq!(link(&app.state, 3), TrackLink::audio(5), "only that insert's");
        // One undo step each.
        crate::history::undo(&mut app.state);
        assert_eq!(link(&app.state, 2), TrackLink::audio(3));
        crate::history::undo(&mut app.state);
        assert!(link(&app.state, 3).is_none());
        crate::history::undo(&mut app.state);
        assert!(link(&app.state, 2).is_none());
    }

    /// Assign to new instrument track (ui/channel-rack.md §18): linked to the channel, named and coloured after it,
    /// the channel's insert renamed too; greyed once the channel has one.
    #[test]
    fn assign_instrument_track() {
        let mut app = app();
        app.state.project.channels[1].color = Some(os_model::Rgb::new(0xaa, 0x33, 0x55));
        let insert2 = crate::mixer::name(&app.state, 2);
        let menu = |app: &App| crate::rack::channel_menu(&app.state, 1, 0.0, 0.0).items;
        assert!(entry(&menu(&app), "Assign to new instrument track").enabled);
        crate::rack::run(&mut app, RCmd::AssignInstrument(1));
        assert_eq!(link(&app.state, 0), TrackLink::instrument(1));
        assert_eq!(instrument_track_of(&app.state, 1), Some(0));
        let tr = track_info(&app.state, 0).unwrap();
        assert_eq!((tr.name.as_deref(), tr.color), (Some("Clap"), Some(os_model::Rgb::new(0xaa, 0x33, 0x55))));
        assert_eq!(crate::mixer::name(&app.state, 2), "Clap", "the channel's insert takes its name");
        let insert = crate::mixer::track(&app.state, 2).unwrap().color;
        assert_eq!(insert, Some(os_model::Rgb::new(0xaa, 0x33, 0x55)), "and its colour");
        assert_eq!(app.state.project.channels[1].mixer_track, 2, "its routing stays");
        assert!(!entry(&menu(&app), "Assign to new instrument track").enabled);
        crate::history::undo(&mut app.state);
        assert!(link(&app.state, 0).is_none());
        assert_eq!(crate::mixer::name(&app.state, 2), insert2);
    }

    /// Track mode ▸ (ui/playlist.md §6c): an insert, a new mixer track, a channel, Unassigned; each one undo step.
    #[test]
    fn track_mode() {
        let mut app = app();
        let inserts = crate::mixer::inserts(&app.state);
        crate::playlist::run(&mut app, Cmd::TrackMode(4, Mode::Audio(2)));
        assert_eq!(link(&app.state, 4), TrackLink::audio(2));
        let items = mode_items(&app.state, 4, |m| Cmd::TrackMode(4, m));
        assert_eq!(items[1].mark, crate::playlist::Mark::Check(true));
        assert_eq!(entry(&items[1].children, &format!("2. {}", crate::mixer::name(&app.state, 2))).mark, crate::playlist::Mark::Check(true));
        crate::playlist::run(&mut app, Cmd::TrackMode(5, Mode::NewAudio));
        assert_eq!(link(&app.state, 5), TrackLink::audio(inserts + 1), "a mixer track added for it");
        assert_eq!(crate::playlist::track_name(&app.state, 5), crate::mixer::name(&app.state, inserts + 1));
        // Audio to instrument: the channel's insert takes its name.
        crate::playlist::run(&mut app, Cmd::TrackMode(4, Mode::Instrument(3)));
        assert_eq!(link(&app.state, 4), TrackLink::instrument(3));
        assert_eq!(crate::playlist::track_name(&app.state, 4), "Snare");
        crate::playlist::run(&mut app, Cmd::TrackMode(4, Mode::Unassigned));
        assert!(link(&app.state, 4).is_none());
        assert_eq!(crate::playlist::track_name(&app.state, 4), "Snare");
        crate::history::undo(&mut app.state);
        assert_eq!(link(&app.state, 4), TrackLink::instrument(3));
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
        app.plugins.available = crate::plugins::builtin();
        let i = app.plugins.index_of(crate::plugins::LEMON_SAMPLER).expect("the built-in sampler");
        let rows = app.state.project.channels.len();
        let steps = app.state.history.at();
        let msg = crate::playlist::run(&mut app, Cmd::TrackMode(6, Mode::NewInstrument(i)));
        assert_eq!(app.state.project.channels.len(), rows + 1);
        let ch = app.state.project.channels[rows].index;
        assert_eq!(link(&app.state, 6), TrackLink::instrument(ch));
        let name = crate::rack::channel_name(&app.state, rows);
        assert_eq!(crate::playlist::track_name(&app.state, 6), name);
        assert_eq!(msg, Some(format!("Added {name} on {name}")));
        assert_eq!((app.state.history.at(), app.state.history.newest_done()), (steps + 1, Some(TRACK_MODE)));
        crate::history::undo(&mut app.state);
        assert_eq!(app.state.project.channels.len(), rows);
        assert!(link(&app.state, 6).is_none());
    }

    /// An audio track's options in Track mode ▸ (ui/playlist.md §6c, measured): only on an audio track; Arm recording and
    /// Disarm recording act on its mixer track alone, whatever the mixer's selection, and the entry is ticked while it is
    /// armed.
    #[test]
    fn audio_track_options() {
        let mut app = app();
        let labels = |app: &App, t| mode_items(&app.state, t, |m| Cmd::TrackMode(t, m)).iter().map(|i| i.label.to_string()).collect::<Vec<_>>();
        assert!(!labels(&app, 2).contains(&"Arm recording".to_string()), "not on a plain track");
        crate::playlist::arr_mut(&mut app.state).tracks[2].link = TrackLink::audio(3);
        assert_eq!(labels(&app, 2)[3..], ["Audio track options", "Select audio input", "Arm recording", "Disarm recording"]);
        // The mixer has Inserts 3 and 4 selected: the audio track's option arms Insert 3 only.
        app.state.mixer.sel = [3, 4].into();
        let arm = entry(&mode_items(&app.state, 2, |m| Cmd::TrackMode(2, m)), "Arm recording");
        assert_eq!((arm.cmd.clone(), arm.mark), (Cmd::Mixer(MCmd::ArmTrack(3, true)), crate::playlist::Mark::Check(false)));
        crate::mixer::run(&mut app, MCmd::ArmTrack(3, true));
        let armed = |app: &App, m| crate::mixer::track(&app.state, m).is_some_and(|x| x.armed);
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
        s.project.channels[4].kind = ChannelKind::AudioClip;
        let audio = ClipSource::Channel(s.project.channels[4].index);
        crate::playlist::arr_mut(s).tracks[2].link = TrackLink::audio(7);
        crate::playlist::arr_mut(s).tracks[5].link = TrackLink::instrument(0);
        assert!(!fits(s, audio, 5) && fits(s, audio, 2) && fits(s, ClipSource::Pattern(1), 5));
        crate::playlist::arr_mut(s).clips.push(clip(6, 0, ClipSource::Pattern(1)));
        assert_eq!(audio_drop_track(s, 5, 0, 96), 7, "below the instrument track, past the busy track");
        assert_eq!(audio_drop_track(s, 4, 0, 96), 4);
        crate::playlist::arr_mut(s).clips.push(clip(2, 0, audio));
        let i = clips(s).len() - 1;
        route_clips(s, &[i]);
        assert_eq!(s.project.channels[4].mixer_track, 7);
        // Off the audio track: the channel stays where it is.
        crate::playlist::arr_mut(s).clips[i].track = 3;
        route_clips(s, &[i]);
        assert_eq!(s.project.channels[4].mixer_track, 7);
        // A recorded take goes to the mixer track's audio track while it is free there.
        assert_eq!(record_track(s, 7, 200, 300), 2);
        assert_eq!(tracks_routed_to(s, &[7]), [2]);
        assert_eq!(tracks_routed_to(s, &[1]), [5], "the instrument track of a channel on Insert 1");
    }

    /// Deleting a linked mixer track or channel unlinks its tracks; the others follow the renumbering.
    #[test]
    fn links_follow_deletes() {
        let mut app = app();
        let s = &mut app.state;
        crate::playlist::arr_mut(s).tracks[0].link = TrackLink::audio(3);
        crate::playlist::arr_mut(s).tracks[1].link = TrackLink::audio(5);
        crate::playlist::arr_mut(s).tracks[2].link = TrackLink::instrument(1);
        crate::playlist::arr_mut(s).tracks[3].link = TrackLink::instrument(4);
        crate::mixer::delete_track(s, 3);
        assert!(link(s, 0).is_none());
        assert_eq!(link(s, 1), TrackLink::audio(4));
        crate::rack::remap_project(&mut s.project, &[Some(0), None, Some(1), Some(2), Some(3)]);
        assert!(link(s, 2).is_none());
        assert_eq!(link(s, 3), TrackLink::instrument(3));
    }
}
