// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! What a controller's notes play (spec: ui/midi-settings.md §3, §4): a note plays the channel the interface points
//! notes at, through the velocity curve and links; the foot pedal holds them; the omni preview channel plays a channel
//! by its white key; pitch bend bends; controllers and other messages nobody links go to the hosted instrument. What a
//! note does besides sounding (recording, step input, a marker jump, Performance mode, the muting channel) is handed
//! back as [`Then`] for whoever runs the session to do.
// spec: ui/midi-settings.md

use std::sync::Arc;

use crate::midi_cfg::{self as cfg, CurveOf};
use crate::midi_in::{Ev, Msg};
use crate::prefs;
use crate::record::Extra;
use crate::session::{Live, Session};
use crate::settings_file::SettingsStore;
use crate::step_edit::Src;

/// A note held from an input device.
#[derive(Debug, Clone, Copy)]
pub struct Held {
    /// The device's key.
    pub key: u8,
    pub src: Src,
    /// The rack row sounding and the key it sounds at (key 60 for an omni preview).
    pub row: usize,
    pub play: u16,
    /// Released while the foot pedal holds it: its release velocity.
    pub up: Option<u8>,
}

/// The notes the devices hold, the pedal, the bent row and the muting keys held.
#[derive(Debug, Default)]
pub struct Play {
    pub held: Vec<Held>,
    /// The foot pedal is down.
    pub pedal: bool,
    /// The row bent by pitch bend, while off centre.
    pub bend_row: Option<usize>,
    /// Generator muting keys held with Toggle on release on: (key, row).
    pub muted: Vec<(u8, usize)>,
}

impl Play {
    /// Rows and keys sounding from the devices (the piano roll's keyboard lights them).
    pub fn lit(&self) -> impl Iterator<Item = (usize, u16)> + '_ {
        self.held.iter().filter(|h| h.up.is_none()).map(|h| (h.row, h.play))
    }
}

/// Where notes go: the rack row the interface sends them to (its selected channel), and whether that channel takes the
/// MIDI channel a note came on (Receive notes from ▸ MIDI channel through, spec: ui/channel-rack.md §18).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct To {
    pub row: usize,
    pub through: bool,
}

/// What a message does besides what it plays, for whoever runs the session.
#[derive(Debug, Clone, PartialEq)]
pub enum Then {
    /// A note started sounding (recording and step input take it).
    Down { src: Src, key: u16, velocity: u8, extra: Extra },
    /// A note stopped, with its release velocity.
    Up { src: Src, release: u8 },
    /// Song marker jump channel: the `i`-th time marker (spec §3).
    Marker(usize),
    /// A key of the Performance mode channel (spec: ui/performance-mode.md §3).
    Perform { key: u8, velocity: u8, down: bool },
    /// Generator muting channel: rack row `row` switches on or off (spec §3).
    Mute(usize),
    /// A controller: a controller link takes it, or else [`forward_in`] gives it to the instrument.
    Cc { dev: Arc<str>, ch: u8, n: u8, v: u8 },
    /// A transport message of a device (external sync, spec §8).
    Transport { dev: Arc<str>, msg: Msg },
}

/// The 0-based MIDI channel set in value box `n` (1..16; "---" = none).
fn channel_of(st: &SettingsStore, n: &prefs::Number) -> Option<u8> {
    match st.number(n) {
        0 => None,
        v => Some((v - 1) as u8),
    }
}

/// The white key's place from MIDI key 48 up: 48 → 0, 50 → 1, 52 → 2, 53 → 3... (spec §3); black keys and keys under
/// 48 have none.
pub fn white_index(key: u8) -> Option<usize> {
    let rel = key.checked_sub(48)?;
    let place = [0, 2, 4, 5, 7, 9, 11].iter().position(|&w| w == rel % 12)?;
    Some(usize::from(rel / 12) * 7 + place)
}

/// Velocity `v` (1..127) through curve `of`, 1..127.
fn curved(st: &SettingsStore, of: CurveOf, v: u8) -> u8 {
    let c = cfg::curve(st, of);
    if c.is_line() {
        return v;
    }
    (c.map(f32::from(v) / 127.0) * 127.0).round().clamp(1.0, 127.0) as u8
}

/// A Mod X / Mod Y value for velocity `v` (spec §3): twice the velocity up to 64 (the centre, 128), then the 63
/// velocities above it spread over 129..253.
pub fn mod_of(v: u8) -> u8 {
    let v = u32::from(v.min(127));
    if v <= 64 { (2 * v) as u8 } else { (128 + ((v - 64) * 125 + 31) / 63) as u8 }
}

/// What a note's velocity gives (spec §3): the velocity it plays at, and its Mod X / Mod Y.
pub fn linked(st: &SettingsStore, v: u8) -> (u8, Extra) {
    let v = curved(st, CurveOf::Velocity, v);
    match st.index(&prefs::VELOCITY_LINK) {
        0 => (100, Extra::default()),
        2 => (100, Extra { mod_x: Some(mod_of(v)), ..Extra::default() }),
        3 => (100, Extra { mod_y: Some(mod_of(v)), ..Extra::default() }),
        _ => (v, Extra::default()),
    }
}

/// A note-off's release velocity (spec §3).
pub fn release_of(st: &SettingsStore, v: u8) -> u8 {
    match st.index(&prefs::RELEASE_LINK) {
        0 => 64,
        _ => curved(st, CurveOf::Release, v.max(1)),
    }
}

/// Plays messages `evs`, oldest first, with notes going to `to`; returns what else they do, in order. With Enable MIDI
/// remote control off, all are ignored.
pub fn events_in(s: &mut Session, evs: Vec<Ev>, to: To) -> Vec<Then> {
    let mut then = Vec::new();
    if evs.is_empty() || !cfg::Switch::Remote.on(&s.settings) {
        return then;
    }
    for ev in evs {
        match ev.msg {
            Msg::Start | Msg::Continue | Msg::Stop | Msg::Spp(_) => then.push(Then::Transport { dev: ev.dev, msg: ev.msg }),
            Msg::On { ch, key, vel } => note_on(s, ch, key, vel, to, &mut then),
            Msg::Off { ch, key, vel } => note_off(s, ch, key, vel, &mut then),
            Msg::Cc { n: 64, v, .. } if s.settings.check(&prefs::FOOT_PEDAL) => pedal(s, v >= 64, &mut then),
            Msg::Cc { ch, n, v } => then.push(Then::Cc { dev: ev.dev, ch, n, v }),
            Msg::Bend { .. } => {
                if to.row < s.project.channels.len() {
                    s.sync_live();
                    if let Some(b) = ev.msg.bytes() {
                        s.live(Live::Midi { row: to.row, bytes: b, bend_cents: 200.0 });
                    }
                    s.midi_play.bend_row = Some(to.row);
                }
            }
            Msg::Pressure { .. } | Msg::Poly { .. } | Msg::Program { .. } => forward_in(s, ev.msg, to.row),
        }
    }
    then
}

/// A message for the hosted instrument of rack row `row`, on MIDI channel 1 (spec §4).
pub fn forward_in(s: &mut Session, m: Msg, row: usize) {
    let Some(b) = m.bytes().filter(|_| row < s.project.channels.len()) else { return };
    s.sync_live();
    s.live(Live::Midi { row, bytes: b, bend_cents: 200.0 });
}

/// The bent row returns to the centre when notes go to another (spec §4).
pub fn unbend_in(s: &mut Session, row: usize) {
    if let Some(bent) = s.midi_play.bend_row.filter(|&r| r != row) {
        s.midi_play.bend_row = None;
        s.live(Live::Midi { row: bent, bytes: [0xe0, 0, 64], bend_cents: 200.0 });
    }
}

/// A session without a window, each tick: the device list kept up to date, and the devices' messages played on the
/// channel `Session::midi_channel` names (MIDI channel through ticked). Of what else they do, the muting channel
/// switches channels (an undo step, as the window's) and controllers go to the instrument; recording, step input,
/// marker jumps, Performance mode and external sync are the window's for now.
pub fn tick_headless(s: &mut Session) {
    if s.midi.take_changed() {
        let settings = s.settings.clone();
        s.midi.refresh(&settings);
        release_all_in(s);
    }
    let evs = s.midi.poll();
    let row = s.midi_channel.and_then(|id| s.project.channels.iter().position(|c| c.id.0 == id));
    let to = To { row: row.unwrap_or(usize::MAX), through: true };
    if let Some(r) = row {
        unbend_in(s, r);
    }
    for t in events_in(s, evs, to) {
        match t {
            Then::Mute(r) => {
                let Some(c) = s.project.channels.get(r).filter(|c| !c.id.is_none()) else { continue };
                let mut cmd = crate::control::Command::set_channel(c.id.0);
                if let crate::control::Command::SetChannel { enabled, .. } = &mut cmd {
                    *enabled = Some(!c.enabled);
                }
                if let Err(e) = crate::control::apply_in(s, &crate::control::By::User, Some(MUTE_SOLO), vec![cmd]) {
                    eprintln!("Lemon Studio: {e}");
                }
            }
            Then::Cc { ch, n, v, .. } => forward_in(s, Msg::Cc { ch, n, v }, to.row),
            _ => {}
        }
    }
}

/// The undo step of a channel's switch (the rack's, the muting channel's).
pub const MUTE_SOLO: &str = "Channel mute / solo";

/// Every device note ends (the device list changed, remote control switched off).
pub fn release_all_in(s: &mut Session) -> Vec<Then> {
    s.midi_play.pedal = false;
    let mut then = Vec::new();
    for h in std::mem::take(&mut s.midi_play.held) {
        end(s, h, 64, &mut then);
    }
    then
}

fn note_on(s: &mut Session, ch: u8, key: u8, vel: u8, to: To, then: &mut Vec<Then>) {
    let n = s.project.channels.len();
    let (vel, mut extra) = linked(&s.settings, vel);
    // The channels set on Settings > MIDI (spec §3): their notes are not played on the selected channel.
    if Some(ch) == channel_of(&s.settings, &prefs::OMNI_CHANNEL) {
        let Some(row) = white_index(key).filter(|&r| r < n) else { return };
        extra.channel = Some(s.project.channels[row].index);
        return start(s, Src::Omni(key), key, row, 60, vel, extra, 0, then);
    }
    if Some(ch) == channel_of(&s.settings, &prefs::MARKER_CHANNEL) {
        if let Some(i) = white_index(key) {
            then.push(Then::Marker(i));
        }
        return;
    }
    if Some(ch) == channel_of(&s.settings, &prefs::PERFORMANCE_CHANNEL) {
        return then.push(Then::Perform { key, velocity: vel, down: true });
    }
    if Some(ch) == channel_of(&s.settings, &prefs::MUTING_CHANNEL) {
        if let Some(row) = white_index(key).filter(|&r| r < n) {
            then.push(Then::Mute(row));
            if s.settings.check(&prefs::TOGGLE_ON_RELEASE) {
                s.midi_play.muted.push((key, row));
            }
        }
        return;
    }
    if to.row >= n {
        return;
    }
    // MIDI channel through (spec: ui/channel-rack.md §18): the plugin gets the note on the channel it came on, and a
    // recorded note takes that channel as its colour; unticked, channel 1.
    let midi = if to.through { ch & 15 } else { 0 };
    if midi > 0 {
        extra.group = Some(midi);
    }
    start(s, Src::Midi(key), key, to.row, u16::from(key), vel, extra, midi, then);
}

/// Starts a note, on MIDI channel `midi` for a hosted plugin; a key the pedal still holds ends its earlier note first
/// (spec §3).
#[allow(clippy::too_many_arguments)]
fn start(s: &mut Session, src: Src, key: u8, row: usize, play: u16, vel: u8, extra: Extra, midi: u8, then: &mut Vec<Then>) {
    while let Some(i) = s.midi_play.held.iter().position(|h| h.src == src) {
        let h = s.midi_play.held.remove(i);
        end(s, h, h.up.unwrap_or(64), then);
    }
    s.sync_live();
    s.live(Live::NoteOn { row, key: play, velocity: vel, midi });
    s.midi_play.held.push(Held { key, src, row, play, up: None });
    then.push(Then::Down { src, key: play, velocity: vel, extra });
}

fn end(s: &mut Session, h: Held, release: u8, then: &mut Vec<Then>) {
    s.live(Live::NoteOff { row: h.row, key: h.play });
    then.push(Then::Up { src: h.src, release });
}

fn note_off(s: &mut Session, ch: u8, key: u8, vel: u8, then: &mut Vec<Then>) {
    if Some(ch) == channel_of(&s.settings, &prefs::PERFORMANCE_CHANNEL) {
        return then.push(Then::Perform { key, velocity: vel, down: false });
    }
    if Some(ch) == channel_of(&s.settings, &prefs::MUTING_CHANNEL) {
        while let Some(i) = s.midi_play.muted.iter().position(|m| m.0 == key) {
            let (_, row) = s.midi_play.muted.remove(i);
            then.push(Then::Mute(row));
        }
        return;
    }
    let src = if Some(ch) == channel_of(&s.settings, &prefs::OMNI_CHANNEL) { Src::Omni(key) } else { Src::Midi(key) };
    let release = release_of(&s.settings, vel);
    let pedal = s.midi_play.pedal && s.settings.check(&prefs::FOOT_PEDAL);
    let Some(i) = s.midi_play.held.iter().position(|h| h.src == src && h.key == key && h.up.is_none()) else { return };
    if pedal {
        s.midi_play.held[i].up = Some(release);
    } else {
        let h = s.midi_play.held.remove(i);
        end(s, h, release, then);
    }
}

/// The foot pedal (controller 64 with Foot pedal controls note off): up ends the notes it held (spec §3).
fn pedal(s: &mut Session, down: bool, then: &mut Vec<Then>) {
    s.midi_play.pedal = down;
    if down {
        return;
    }
    let (ended, kept): (Vec<Held>, Vec<Held>) = std::mem::take(&mut s.midi_play.held).into_iter().partition(|h| h.up.is_some());
    s.midi_play.held = kept;
    for h in ended {
        end(s, h, h.up.unwrap_or(64), then);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_keys_from_48() {
        assert_eq!([48, 50, 52, 53, 55, 57, 59, 60].map(white_index), [0, 1, 2, 3, 4, 5, 6, 7].map(Some));
        assert_eq!((white_index(49), white_index(47), white_index(62)), (None, None, Some(8)));
    }

    #[test]
    fn velocity_links() {
        let st = SettingsStore::memory();
        assert_eq!([1, 32, 64, 100, 127].map(mod_of), [2, 64, 128, 199, 253]);
        assert_eq!(linked(&st, 90), (90, Extra::default()), "Velocity is the default");
        st.set_choice(&prefs::VELOCITY_LINK, 2);
        assert_eq!(linked(&st, 64), (100, Extra { mod_x: Some(128), ..Extra::default() }));
        st.set_choice(&prefs::VELOCITY_LINK, 0);
        assert_eq!(linked(&st, 20).0, 100);
        st.set_choice(&prefs::RELEASE_LINK, 0);
        assert_eq!(release_of(&st, 10), 64);
    }
}
