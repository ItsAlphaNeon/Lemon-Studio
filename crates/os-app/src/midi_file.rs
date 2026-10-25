// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! MIDI files: FILE ▸ Import ▸ MIDI file..., the piano roll's Import MIDI file... and a .mid dropped on the window read
//! a Standard MIDI File (`os_smf`) through the "Import MIDI data" form (`MidiForm`, painted by `views::midi_form`);
//! FILE ▸ Export MIDI... and the piano roll's Export as MIDI file... write one.
// spec: ui/midi-files.md

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::cell::RefCell;

use os_model::{ControlPoint, Note, Project, TimeMarker};
use os_smf::{Event, Kind as Ev, Smf, Track};
use winit::keyboard::{Key, NamedKey};

use crate::app::{App, State};
use crate::dialog::{spans, Answer, Body, Kind, Reply, Then};
use crate::history::{self, Snap};
use crate::project_forms::{FHit, Form};

/// The form's caption (spec §2).
pub const TITLE: &str = "Import MIDI data";
/// The undo step of an import (spec §10.7).
pub const IMPORT_STEP: &str = "Import MIDI file";
/// The tempo control point's target: channel 0x4000, parameter 5 (spec §3).
const TEMPO_CHANNEL: u16 = 0x4000;
const TEMPO_PARAM: u16 = 5;

/// The lamps of FILE's form, by their place: left column, then right (spec §2).
pub const CHECKS: [&str; 6] = [
    "Start new project",
    "Realign events",
    "Import time signatures",
    "Create one channel per track",
    "Set mixer tracks for new channels",
    "Import zero velocity notes",
];
/// The piano roll's form has this in place of Start new project, and neither lamp 3 nor 4.
pub const BLEND: &str = "Blend with existing data";
pub const CHECK_HINTS: [&str; 6] = [
    "Start a new project for the import, or add it to this one",
    "Remove the silence before the first note",
    "Put the file's time signatures into the pattern",
    "One channel per track, or one per MIDI channel",
    "Route the new channels to mixer tracks 1, 2, 3...",
    "Read a note-on of velocity 0 as a note, not as a note-off",
];
pub const BLEND_HINT: &str = "Add the notes to the channel's notes in the pattern, or replace them";
/// The combos: which tracks, the channel type.
pub const TRACKS_LABEL: &str = "Which tracks to import?";
pub const CHANNELS_LABEL: &str = "Which channels to import?";
pub const TYPE_LABEL: &str = "Channel type";

/// The instrument the import's new channels play (spec §10.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Instrument {
    Trio,
    Citrus,
    Sampler,
}

impl Instrument {
    pub const ALL: [Instrument; 3] = [Instrument::Trio, Instrument::Citrus, Instrument::Sampler];

    pub fn label(self) -> &'static str {
        match self {
            Instrument::Trio => crate::osc3::LEMON_TRIO,
            Instrument::Citrus => crate::citrus::CITRUS,
            Instrument::Sampler => crate::plugins::LEMON_SAMPLER,
        }
    }
}

/// The form's settings (spec §2).
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    /// The track to import, 0-based; `None` = all tracks.
    pub track: Option<usize>,
    /// MIDI channels 1 to 16.
    pub channels: [bool; 16],
    pub instrument: Instrument,
    /// FILE's form: Start new project.
    pub new_project: bool,
    /// The piano roll's form: Blend with existing data.
    pub blend: bool,
    pub realign: bool,
    pub signatures: bool,
    pub per_track: bool,
    pub mixer: bool,
    pub zero_velocity: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            track: None,
            channels: [true; 16],
            instrument: Instrument::Trio,
            new_project: true,
            blend: false,
            realign: false,
            signatures: true,
            per_track: true,
            mixer: true,
            zero_velocity: false,
        }
    }
}

impl Options {
    /// Lamp `k` of the form (`CHECKS`; lamp 0 is Blend on the piano roll's form).
    pub fn check(&self, k: u8, piano: bool) -> bool {
        match k {
            0 if piano => self.blend,
            0 => self.new_project,
            1 => self.realign,
            2 => self.signatures,
            3 => self.per_track,
            4 => self.mixer,
            _ => self.zero_velocity,
        }
    }

    fn toggle(&mut self, k: u8, piano: bool) {
        let v = match k {
            0 if piano => &mut self.blend,
            0 => &mut self.new_project,
            1 => &mut self.realign,
            2 => &mut self.signatures,
            3 => &mut self.per_track,
            4 => &mut self.mixer,
            _ => &mut self.zero_velocity,
        };
        *v = !*v;
    }
}

thread_local! {
    /// The settings of the last import this session (spec §10.2). Each form opens with every track and channel
    /// chosen, and the per-track lamp as the file's type sets it. The window's thread is the only one that imports
    /// (each test has its own).
    static LAST: RefCell<Option<Options>> = const { RefCell::new(None) };
    /// An import that waits for the save-changes question (Start new project): its file and settings.
    static PENDING: RefCell<Option<(PathBuf, Options)>> = const { RefCell::new(None) };
}

/// The settings a form for file `f` opens with: Realign events on for a file with a leading silence (spec §10.5).
fn options_for(f: &Smf) -> Options {
    let last = LAST.with_borrow(Clone::clone).unwrap_or_default();
    let realign = last.realign || leading_silence(f);
    Options { track: None, channels: [true; 16], per_track: f.format == 1, realign, ..last }
}

/// Whether file `f`'s first note starts at or after the end of its first bar, as the time signature at tick 0 sets
/// it (4/4 without one): a MIDI pack's loop placed hundreds of bars in (spec §10.5). A pickup is not one.
pub fn leading_silence(f: &Smf) -> bool {
    let events = || f.tracks.iter().flat_map(|t| &t.events);
    let first = events().filter(|e| matches!(e.kind, Ev::NoteOn { velocity, .. } if velocity > 0)).map(|e| e.tick).min();
    let (num, den) = events()
        .find_map(|e| match e.kind {
            Ev::TimeSignature { numerator, denominator } if e.tick == 0 && numerator > 0 && denominator > 0 => Some((numerator, denominator)),
            _ => None,
        })
        .unwrap_or((4, 4));
    let bar = (u64::from(f.ppq.max(1)) * 4 * u64::from(num) / u64::from(den)).max(1);
    first.is_some_and(|t| u64::from(t) >= bar)
}

/// The "Import MIDI data" form (spec §2): the file read, its tracks' names for the combo, the settings and which
/// combo's list is open (0 tracks, 1 channel type).
#[derive(Debug, Clone, PartialEq)]
pub struct MidiForm {
    pub path: PathBuf,
    pub smf: Smf,
    /// The piano roll's form (no channel type, Blend in place of Start new project).
    pub piano: bool,
    pub o: Options,
    pub list: Option<u8>,
}

impl MidiForm {
    /// The tracks combo's entries: All tracks, then "1. name" per track.
    pub fn track_items(&self) -> Vec<String> {
        std::iter::once("All tracks".to_string()).chain((0..self.smf.tracks.len()).map(|i| format!("{}. {}", i + 1, track_name(&self.smf, i)))).collect()
    }

    /// Combo `k`'s entries and the chosen one.
    pub fn list_items(&self, k: u8) -> (Vec<String>, usize) {
        if k == 0 {
            (self.track_items(), self.o.track.map_or(0, |t| t + 1))
        } else {
            let now = Instrument::ALL.iter().position(|&i| i == self.o.instrument).unwrap_or(0);
            (Instrument::ALL.iter().map(|i| i.label().to_string()).collect(), now)
        }
    }

    /// Combo `k`'s text.
    pub fn combo_text(&self, k: u8) -> String {
        let (items, now) = self.list_items(k);
        items.get(now).cloned().unwrap_or_default()
    }

    /// Lamp `k`'s label and hint on this form, `None` where the form has no such lamp.
    pub fn lamp(&self, k: u8) -> Option<(&'static str, &'static str)> {
        match k {
            0 if self.piano => Some((BLEND, BLEND_HINT)),
            3 | 4 if self.piano => None,
            k => Some((CHECKS[usize::from(k).min(5)], CHECK_HINTS[usize::from(k).min(5)])),
        }
    }

    fn pick(&mut self, k: u8, i: usize) {
        if k == 0 {
            self.o.track = i.checked_sub(1).filter(|&t| t < self.smf.tracks.len());
        } else if let Some(&ins) = Instrument::ALL.get(i) {
            self.o.instrument = ins;
        }
    }
}

/// Track `i`'s name: its first track-name event, else "Track n" (spec: open questions).
pub fn track_name(f: &Smf, i: usize) -> String {
    f.tracks
        .get(i)
        .and_then(|t| t.events.iter().find_map(|e| if let Ev::TrackName(n) = &e.kind { Some(n.trim().to_string()) } else { None }))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("Track {}", i + 1))
}

/// A channel the import makes: its name, its notes and channel control points (channel field 0 until placed).
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub name: String,
    pub notes: Vec<Note>,
    pub controls: Vec<ControlPoint>,
}

/// What a file gives at the project's PPQ (spec §3 to §6).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Imported {
    pub parts: Vec<Part>,
    /// Tempo control points, and the first tempo in BPM.
    pub tempos: Vec<ControlPoint>,
    pub tempo: Option<f64>,
    /// Time-signature markers (kind 8) and plain markers.
    pub signatures: Vec<TimeMarker>,
    pub markers: Vec<TimeMarker>,
}

/// A note read from the file, in its ticks.
struct Raw {
    start: u32,
    end: u32,
    key: u8,
    velocity: u8,
    release: u8,
    channel: u8,
    track: usize,
}

/// File `f` as notes, control points and markers at `ppq` ticks per quarter note, as `o` chooses (spec §3 to §6,
/// §10.4).
pub fn convert(f: &Smf, o: &Options, ppq: u16) -> Imported {
    let chosen = |t: usize| o.track.is_none_or(|k| k == t);
    let on = |ch: u8| o.channels[usize::from(ch & 15)];
    // Notes: a note-off (or a note-on of velocity 0, unless those are notes) ends the newest open note of its key and
    // channel; notes never ended are left out (§4).
    let mut raw = Vec::new();
    for (ti, t) in f.tracks.iter().enumerate().filter(|(i, _)| chosen(*i)) {
        let mut open: BTreeMap<(u8, u8), Vec<(u32, u8)>> = BTreeMap::new();
        for e in &t.events {
            let (channel, key, end_vel) = match e.kind {
                Ev::NoteOn { channel, key, velocity } if velocity > 0 || o.zero_velocity => {
                    open.entry((channel, key)).or_default().push((e.tick, velocity));
                    continue;
                }
                Ev::NoteOn { channel, key, .. } => (channel, key, 0),
                Ev::NoteOff { channel, key, velocity } => (channel, key, velocity),
                _ => continue,
            };
            if let Some((start, velocity)) = open.get_mut(&(channel, key)).and_then(Vec::pop)
                && on(channel)
            {
                raw.push(Raw { start, end: e.tick, key, velocity, release: end_vel, channel, track: ti });
            }
        }
    }
    raw.sort_by_key(|r| (r.start, r.track));
    let shift = if o.realign { raw.iter().map(|r| r.start).min().unwrap_or(0) } else { 0 };
    // Ticks round down; a note's start and end each, its length at least 1 (§4).
    let at = |t: u32| (u64::from(t.saturating_sub(shift)) * u64::from(ppq) / u64::from(f.ppq.max(1))) as u32;
    let part_of = |track: usize, channel: u8| if o.per_track { track } else { usize::from(channel) };
    let mut parts: BTreeMap<usize, Part> = BTreeMap::new();
    for r in &raw {
        let k = part_of(r.track, r.channel);
        let name = if o.per_track { track_name(f, r.track) } else { format!("Channel #{}", r.channel + 1) };
        let part = parts.entry(k).or_insert_with(|| Part { name, notes: Vec::new(), controls: Vec::new() });
        let (s, e) = (at(r.start), at(r.end));
        part.notes.push(Note {
            position: s,
            length: e.saturating_sub(s).max(1),
            key: u16::from(r.key),
            velocity: r.velocity,
            release: r.release,
            group: r.channel & 15,
            ..Note::step(0, 0)
        });
    }
    // Channel volume, pan and pitch from controllers 7 and 10 and the pitch bend, for the channels made (§3).
    for (ti, t) in f.tracks.iter().enumerate().filter(|(i, _)| chosen(*i)) {
        for e in &t.events {
            let Some(ch) = e.kind.channel().filter(|&c| on(c)) else { continue };
            let (param, value) = match e.kind {
                Ev::Controller { number: 7, value, .. } => (ControlPoint::VOLUME, i32::from(value) * 100),
                Ev::Controller { number: 10, value, .. } => (ControlPoint::PAN, i32::from(value) * 100),
                Ev::PitchBend { value, .. } => (ControlPoint::PITCH, i32::from(value) * 200 / 8192),
                _ => continue,
            };
            if let Some(p) = parts.get_mut(&part_of(ti, ch)) {
                p.controls.push(ControlPoint::new(at(e.tick), 0, param, value));
            }
        }
    }
    let mut imp = Imported { parts: parts.into_values().collect(), ..Imported::default() };
    // The tempo from every track, whichever are chosen (§5); signatures likewise; markers from the chosen tracks. Of
    // tempo changes that land on one tick (Realign puts those before the first note at 0), the last holds (§10.4).
    let mut tempos: Vec<(u32, f64)> = Vec::new();
    for (ti, t) in f.tracks.iter().enumerate() {
        for e in &t.events {
            match &e.kind {
                // In thousandths of a BPM, as the project and its tempo points keep it.
                Ev::Tempo(us) => tempos.push((e.tick, (os_smf::bpm_of_tempo(*us) * 1000.0).round() / 1000.0)),
                Ev::TimeSignature { numerator, denominator } if o.signatures && *numerator > 0 && *denominator > 0 => {
                    let pos = at(e.tick);
                    imp.signatures.retain(|m| m.position != pos);
                    imp.signatures.push(TimeMarker {
                        kind: TimeMarker::SIGNATURE,
                        numerator: *numerator,
                        denominator: *denominator,
                        ..TimeMarker::new(pos, &format!("{numerator}/{denominator}"))
                    });
                }
                Ev::Marker(text) if chosen(ti) => imp.markers.push(TimeMarker::new(at(e.tick), text)),
                _ => {}
            }
        }
    }
    tempos.sort_by_key(|t| t.0);
    let mut placed: Vec<(u32, f64)> = Vec::new();
    for (tick, bpm) in tempos {
        match placed.last_mut() {
            Some(l) if l.0 == at(tick) => l.1 = bpm,
            _ => placed.push((at(tick), bpm)),
        }
    }
    imp.tempo = placed.first().map(|t| t.1);
    imp.tempos = placed.iter().map(|&(pos, bpm)| ControlPoint::new(pos, TEMPO_CHANNEL, TEMPO_PARAM, (bpm * 1000.0).round() as i32)).collect();
    imp.signatures.sort_by_key(|m| m.position);
    imp.markers.sort_by_key(|m| m.position);
    imp
}

/// Whether the form is needed for file `f` while "Show when necessary" applies: notes in more than one track or on
/// more than one MIDI channel, or a leading silence (spec §10.5).
pub fn necessary(f: &Smf) -> bool {
    if leading_silence(f) {
        return true;
    }
    let mut tracks = 0;
    let mut channels = 0u16;
    for t in &f.tracks {
        let mut has = false;
        for e in &t.events {
            if let Ev::NoteOn { channel, velocity, .. } = e.kind
                && velocity > 0
            {
                has = true;
                channels |= 1 << (channel & 15);
            }
        }
        tracks += usize::from(has);
    }
    tracks > 1 || channels.count_ones() > 1
}

/// Which of the MIDI import window rows the held modifier picks (General ▸ Input ▸ Manage shortcuts): its row's
/// behaviour, "Show when necessary" when no row has that modifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Show {
    WhenNecessary,
    Always,
    Never,
}

pub fn show_for(alt: bool, shift: bool) -> Show {
    use crate::shortcuts::{MIDI_EVERY_TIME, MIDI_NEVER, MIDI_WHEN_NECESSARY};
    let held = if alt { "Alt" } else if shift { "Shift" } else { "(none)" };
    [(MIDI_WHEN_NECESSARY, Show::WhenNecessary), (MIDI_EVERY_TIME, Show::Always), (MIDI_NEVER, Show::Never)]
        .into_iter()
        .find(|(c, _)| crate::prefs::choice(c) == held)
        .map_or(Show::WhenNecessary, |(_, s)| s)
}

// ---------------------------------------------------------------------------------------------------------------
// Export

/// A note's MIDI channel: its colour group (spec §9).
fn midi_channel(n: &Note) -> u8 {
    n.group & 15
}

/// Note-on and note-off events for notes at (start, end); note-offs first at one tick (spec §9). A step (length 0)
/// lasts one step; muted notes are left out.
fn note_events<'a>(notes: impl Iterator<Item = (u32, u32, &'a Note)>) -> Vec<Event> {
    let mut v = Vec::new();
    for (s, e, n) in notes.filter(|(_, _, n)| !n.muted()) {
        let (channel, key) = (midi_channel(n), n.key.min(127) as u8);
        v.push(Event { tick: s, kind: Ev::NoteOn { channel, key, velocity: n.velocity.clamp(1, 127) } });
        v.push(Event { tick: e.max(s + 1), kind: Ev::NoteOff { channel, key, velocity: n.release.min(127) } });
    }
    v.sort_by_key(|e| (e.tick, matches!(e.kind, Ev::NoteOn { .. })));
    v
}

/// A note's end tick: its length, one step for a step.
fn end_of(n: &Note, ppq: u16) -> u32 {
    n.position + if n.length == 0 { (u32::from(ppq) / 4).max(1) } else { n.length }
}

/// The project's time signature at 0, when its bar is one.
fn start_signature(p: &Project) -> Option<Event> {
    let (num, den) = p.signature().unwrap_or((4, 4));
    p.bar_is_signature().then(|| Event { tick: 0, kind: Ev::TimeSignature { numerator: num.min(255) as u8, denominator: den.min(255) as u8 } })
}

fn signature_event(m: &TimeMarker, tick: u32) -> Event {
    Event { tick, kind: Ev::TimeSignature { numerator: m.numerator, denominator: m.denominator } }
}

/// Piano roll ▸ Export as MIDI file...: the channel with index `ch`'s notes in pattern `n` (spec §9). A type 1 file:
/// the tempo and the pattern's time signatures, then the notes in a track named "<channel> <MIDI channel>".
pub fn pattern_file(p: &Project, n: u16, ch: u16, tempo: f64) -> Smf {
    let pat = p.pattern(n);
    let mut conductor = vec![Event { tick: 0, kind: Ev::Tempo(os_smf::tempo_of_bpm(tempo)) }];
    let sigs: Vec<&TimeMarker> = pat.map(|q| q.markers.iter().filter(|m| m.kind == TimeMarker::SIGNATURE).collect()).unwrap_or_default();
    if !sigs.iter().any(|m| m.position == 0) {
        conductor.extend(start_signature(p));
    }
    conductor.extend(sigs.iter().map(|m| signature_event(m, m.position)));
    let notes: Vec<&Note> = pat.map(|q| q.notes.iter().filter(|x| x.channel == ch).collect()).unwrap_or_default();
    let name = p.channel(ch).and_then(|c| c.name.clone()).unwrap_or_else(|| format!("Channel {}", ch + 1));
    let midi = notes.iter().find(|x| !x.muted()).map_or(0, |x| midi_channel(x));
    let mut events = vec![Event { tick: 0, kind: Ev::TrackName(format!("{name} {}", midi + 1)) }];
    events.extend(note_events(notes.iter().map(|x| (x.position, end_of(x, p.ppq), *x))));
    Smf { format: 1, ppq: p.ppq, tracks: vec![Track { events: conductor }, Track { events }] }
}

/// FILE ▸ Export MIDI...: the project's notes as a type 1 file (spec §10.8): a conductor track (the tempo, tempo
/// control points, time signatures, markers), then a track per channel with notes. `song`: the current arrangement's
/// pattern clips, each cut at its end; else pattern `n`.
pub fn project_file(p: &Project, song: bool, n: u16, tempo: f64) -> Smf {
    // Notes and tempo points as placed in the song: (start, end, note) per channel index.
    let mut by_ch: BTreeMap<u16, Vec<(u32, u32, Note)>> = BTreeMap::new();
    let mut tempos: Vec<(u32, i32)> = Vec::new();
    let mut conductor: Vec<Event> = start_signature(p).into_iter().collect();
    let mut place = |pat: &os_model::Pattern, at: u32, from: u32, to: Option<u32>| {
        let inside = |t: u32| t >= from && to.is_none_or(|e| t < e);
        for x in pat.notes.iter().filter(|x| inside(x.position)) {
            let end = end_of(x, p.ppq).min(to.unwrap_or(u32::MAX));
            by_ch.entry(x.channel).or_default().push((at + x.position - from, at + end - from, *x));
        }
        for c in pat.controls.iter().filter(|c| c.channel() == TEMPO_CHANNEL && c.param() == TEMPO_PARAM && inside(c.position)) {
            tempos.push((at + c.position - from, c.value));
        }
    };
    if song {
        if let Some(a) = p.arrangement() {
            let track_on = |t: u16| a.tracks.iter().find(|x| x.number == u32::from(t) + 1).is_none_or(|x| x.enabled);
            for c in a.clips.iter().filter(|c| !c.muted && track_on(c.track)) {
                let os_model::ClipSource::Pattern(k) = c.source else { continue };
                let Some(pat) = p.pattern(k) else { continue };
                let from = c.window.map_or(0, |(s, _)| s.max(0.0) as u32);
                place(pat, c.position, from, Some(from + c.length));
            }
            for m in &a.markers {
                conductor.push(match m.kind {
                    TimeMarker::SIGNATURE => signature_event(m, m.position),
                    _ => Event { tick: m.position, kind: Ev::Marker(m.name.clone()) },
                });
            }
        }
    } else if let Some(pat) = p.pattern(n) {
        place(pat, 0, 0, None);
        for m in &pat.markers {
            conductor.push(match m.kind {
                TimeMarker::SIGNATURE => signature_event(m, m.position),
                _ => Event { tick: m.position, kind: Ev::Marker(m.name.clone()) },
            });
        }
    }
    tempos.sort_by_key(|t| t.0);
    // The project's tempo opens the song, unless a tempo point does.
    if tempos.first().is_none_or(|t| t.0 > 0) {
        conductor.insert(0, Event { tick: 0, kind: Ev::Tempo(os_smf::tempo_of_bpm(tempo)) });
    }
    conductor.extend(tempos.iter().map(|&(t, v)| Event { tick: t, kind: Ev::Tempo(os_smf::tempo_of_bpm(f64::from(v) / 1000.0)) }));
    let mut tracks = vec![Track { events: conductor }];
    for c in &p.channels {
        let Some(notes) = by_ch.get(&c.index).filter(|v| v.iter().any(|(_, _, x)| !x.muted())) else { continue };
        let name = c.name.clone().unwrap_or_else(|| format!("Channel {}", c.index + 1));
        let mut events = vec![Event { tick: 0, kind: Ev::TrackName(name) }];
        events.extend(note_events(notes.iter().map(|(s, e, x)| (*s, *e, x))));
        tracks.push(Track { events });
    }
    Smf { format: 1, ppq: p.ppq, tracks }
}

// ---------------------------------------------------------------------------------------------------------------
// The app's side

/// The top dialog's MIDI form.
fn top(app: &mut App) -> Option<&mut MidiForm> {
    match &mut app.state.dialogs.last_mut()?.body {
        Body::Project(Form::Midi(f)) => Some(f),
        _ => None,
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

fn stem(path: &Path) -> String {
    path.file_stem().map_or_else(|| "MIDI file".to_string(), |n| n.to_string_lossy().into_owned())
}

/// The channels' notes go into pattern `n` with the import's control points and signature markers, the markers into
/// the playlist; the new channels into a filter group named after the file, which the rack then shows (spec §3, §7).
fn place(s: &mut State, imp: Imported, o: &Options, name: &str, n: u16) {
    let base = s.project.channels.len();
    let max = crate::mixer::max_insert(&s.project);
    for (i, part) in imp.parts.iter().enumerate() {
        let row = match o.instrument {
            Instrument::Sampler => crate::app::add_channel(&mut s.project),
            k => crate::app::add_plugin_channel(&mut s.project, k.label()),
        };
        let ch = &mut s.project.channels[row];
        ch.name = Some(part.name.clone());
        ch.mixer_track = if o.mixer { (i as u16 + 1).min(max) } else { 0 };
    }
    let index: Vec<u16> = s.project.channels[base..].iter().map(|c| c.index).collect();
    let old = s.project.pattern_length(n);
    let pat = crate::patterns::pat_mut(s, n);
    if pat.name.is_none() {
        pat.name = Some(name.to_string());
    }
    for (part, &ch) in imp.parts.iter().zip(&index) {
        pat.notes.extend(part.notes.iter().map(|x| Note { channel: ch, ..*x }));
        pat.controls.extend(part.controls.iter().map(|c| ControlPoint::new(c.position, ch, c.param(), c.value)));
    }
    pat.controls.extend(imp.tempos.iter().copied());
    pat.notes.sort_by_key(|x| x.position);
    pat.controls.sort_by_key(|c| c.position);
    if o.signatures && !imp.signatures.is_empty() {
        pat.markers.retain(|m| !(m.kind == TimeMarker::SIGNATURE && imp.signatures.iter().any(|x| x.position == m.position)));
        pat.markers.extend(imp.signatures.iter().cloned());
        pat.markers.sort_by_key(|m| m.position);
        crate::playlist::refit_pattern_clips(s, n, old);
    }
    if !imp.markers.is_empty() {
        crate::playlist::arr_mut(s).markers.extend(imp.markers.iter().cloned());
    }
    let rack = &mut s.rack;
    rack.groups.insert(0, crate::rack::Group { name: name.to_string(), members: (base..base + index.len()).collect() });
    rack.unsorted_at = None;
    rack.filter = crate::rack::Filter::Group(0);
    s.selected_channel = base;
    s.rack.select_only(base);
    crate::rack::autosize(s);
    s.piano.dirty = true;
}

impl App {
    /// FILE ▸ Import ▸ MIDI file... (`piano` = false) and the piano roll's Import MIDI file...: the open dialog (spec §1).
    pub(crate) fn import_midi_dialog(&mut self, piano: bool) {
        use crate::filedlg::{FileDlg, Mode};
        let dlg = FileDlg::new(Mode::Open, "Import MIDI file", "midi", Then::MidiImport(piano)).filter("MIDI file", &["mid", "midi"]);
        self.file_dialog(dlg);
    }

    /// A MIDI file chosen in the open dialog or dropped on the window: the form, or the import with the last settings
    /// as the MIDI import window setting says (spec §10.5).
    pub(crate) fn midi_file_chosen(&mut self, path: PathBuf, piano: bool) {
        let smf = match std::fs::read(&path).map_err(|e| e.to_string()).and_then(|b| os_smf::read(&b).map_err(|e| e.to_string())) {
            Ok(f) => f,
            Err(e) => return self.tell(Kind::Warning, TITLE, spans(&["Could not import ", &file_name(&path), &format!(": {e}.")])),
        };
        let o = options_for(&smf);
        let show = match show_for(self.alt, self.shift) {
            Show::Always => true,
            Show::Never => false,
            Show::WhenNecessary => necessary(&smf),
        };
        if show {
            self.show_form(Body::Project(Form::Midi(Box::new(MidiForm { path, smf, piano, o, list: None }))));
        } else {
            self.midi_accepted(path, smf, o, piano);
        }
    }

    /// The settings are chosen: the piano roll's import at once; FILE's into this project at once, into a new one
    /// once unsaved changes are answered.
    fn midi_accepted(&mut self, path: PathBuf, smf: Smf, o: Options, piano: bool) {
        // Realign for a file with a leading silence is not remembered: the next file opens as the last other import
        // left it (spec §10.5).
        let realign = if leading_silence(&smf) { LAST.with_borrow(|l| l.as_ref().is_some_and(|l| l.realign)) } else { o.realign };
        LAST.set(Some(Options { realign, ..o.clone() }));
        if piano {
            return self.import_into_piano(&path, &smf, &o);
        }
        if !o.new_project {
            return self.import_into_project(&path, &smf, &o);
        }
        PENDING.set(Some((path.clone(), o)));
        self.ask_discard("importing a MIDI file", crate::dialog::Next::ImportMidi(path));
    }

    /// The save-changes question was answered: the import that waited for it (`Next::ImportMidi`).
    pub(crate) fn import_pending(&mut self, path: PathBuf) {
        let Some((p, o)) = PENDING.take().filter(|(p, _)| *p == path) else { return };
        match std::fs::read(&p).map_err(|e| e.to_string()).and_then(|b| os_smf::read(&b).map_err(|e| e.to_string())) {
            Ok(f) => self.import_into_project(&p, &f, &o),
            Err(e) => self.tell(Kind::Warning, TITLE, spans(&["Could not import ", &file_name(&p), &format!(": {e}.")])),
        }
    }

    /// FILE's import (spec §3 to §7): into a new project in pattern mode at PPQ 96, or added to this one as one undo
    /// step.
    fn import_into_project(&mut self, path: &Path, f: &Smf, o: &Options) {
        let ppq = if o.new_project { 96 } else { self.state.project.ppq.max(1) };
        let imp = convert(f, o, ppq);
        let name = stem(path);
        if imp.parts.is_empty() {
            return self.tell(Kind::Info, TITLE, spans(&["", &file_name(path), " has no notes in the tracks and channels chosen."]));
        }
        let parts = imp.parts.len();
        if o.new_project {
            self.transport.stop();
            self.reset_hosted();
            let tempo = imp.tempo.unwrap_or(120.0);
            let s = &mut self.state;
            s.project = crate::app::empty_project();
            s.project.ppq = ppq;
            s.project.tempo = Some(tempo);
            crate::project_info::created(s);
            s.file = None;
            s.path = None;
            s.tempo = tempo;
            s.pattern = 1;
            s.song_mode = false;
            s.piano.reset();
            s.playlist.reset();
            s.history.reset();
            crate::playlist::load_view(s);
            s.rack.reset();
            crate::rack::from_model(s);
            // A project with nothing in it counts as saved; the import is its first change.
            self.mark_saved();
            place(&mut self.state, imp, o, &name, 1);
        } else {
            let s = &mut self.state;
            // As every playlist step does (`playlist::snapshot_as`), the arrangement exists before the step.
            crate::playlist::arr_mut(s);
            let snap = Snap::Several(vec![
                Snap::Patterns { patterns: s.project.patterns.clone(), current: s.pattern },
                Snap::Playlist(Box::new(crate::playlist::capture(s))),
                Snap::Tail(Box::new(history::Tail::now(s, s.project.channels.len()))),
            ]);
            history::record(s, IMPORT_STEP, snap);
            let n = s.pattern;
            place(s, imp, o, &name, n);
        }
        let s = &mut self.state;
        crate::piano::center_keys(s, self.piano_grid.map_or(700.0, |g| g.h));
        s.message = Some(format!("Imported {} ({parts} channel{})", file_name(path), if parts == 1 { "" } else { "s" }));
        self.invalidate();
    }

    /// The piano roll's import (spec §9): every chosen track's notes into its channel in the current pattern, replacing
    /// the channel's notes there unless Blend is on; the imported notes selected. One undo step.
    fn import_into_piano(&mut self, path: &Path, f: &Smf, o: &Options) {
        let s = &mut self.state;
        let Some(ch) = crate::piano::channel(s) else { return };
        let imp = convert(f, &Options { per_track: true, ..o.clone() }, s.project.ppq.max(1));
        let new: Vec<Note> = imp.parts.iter().flat_map(|p| p.notes.iter()).map(|x| Note { channel: ch, ..*x }).collect();
        if new.is_empty() {
            return self.tell(Kind::Info, TITLE, spans(&["", &file_name(path), " has no notes in the tracks and channels chosen."]));
        }
        let n = s.pattern;
        history::record(s, IMPORT_STEP, Snap::Patterns { patterns: s.project.patterns.clone(), current: n });
        let old = s.project.pattern_length(n);
        let pat = crate::patterns::pat_mut(s, n);
        let mut all: Vec<(Note, bool)> = std::mem::take(&mut pat.notes).into_iter().filter(|x| o.blend || x.channel != ch).map(|x| (x, false)).collect();
        all.extend(new.iter().map(|x| (*x, true)));
        all.sort_by_key(|(x, _)| x.position);
        let sel = all.iter().enumerate().filter(|(_, (_, new))| *new).map(|(i, _)| i).collect();
        pat.notes = all.into_iter().map(|(x, _)| x).collect();
        if o.signatures && !imp.signatures.is_empty() {
            pat.markers.retain(|m| !(m.kind == TimeMarker::SIGNATURE && imp.signatures.iter().any(|x| x.position == m.position)));
            pat.markers.extend(imp.signatures.iter().cloned());
            pat.markers.sort_by_key(|m| m.position);
            crate::playlist::refit_pattern_clips(s, n, old);
        }
        s.piano.select(n, ch, sel);
        s.piano.dirty = true;
        s.message = Some(format!("Imported {} into the piano roll ({} notes)", file_name(path), new.len()));
        self.invalidate();
    }

    /// A click on a part of the MIDI form; `Some` closes it. With a list open, a click picks from it or only closes it.
    pub(crate) fn midi_form_click(&mut self, h: FHit) -> Option<Reply> {
        let f = top(self)?;
        if let Some(k) = f.list.take() {
            if let FHit::Item(i) = h {
                f.pick(k, usize::from(i));
            }
            self.invalidate();
            return None;
        }
        match h {
            FHit::Combo(k) if k == 0 || !f.piano => f.list = Some(k),
            FHit::Tile(c) => {
                let c = usize::from(c).min(15);
                f.o.channels[c] = !f.o.channels[c];
            }
            FHit::Check(k) if f.lamp(k).is_some() => f.o.toggle(k, f.piano),
            FHit::Main => return Some(Reply::Button(Answer::Ok)),
            _ => {}
        }
        self.invalidate();
        None
    }

    /// Keys on the MIDI form: Esc closes an open list, then the form; Enter accepts.
    pub(crate) fn midi_form_key(&mut self, key: &Key) -> Option<Reply> {
        let f = top(self)?;
        if f.list.is_some() {
            if matches!(key, Key::Named(NamedKey::Escape)) {
                f.list = None;
                self.invalidate();
            }
            return None;
        }
        match key {
            Key::Named(NamedKey::Escape) => Some(Reply::Button(Answer::Cancel)),
            Key::Named(NamedKey::Enter) => Some(Reply::Button(Answer::Ok)),
            _ => None,
        }
    }

    /// The form closed: Accept imports; Esc and the caption's × import nothing (spec §2).
    pub(crate) fn midi_form_closed(&mut self, f: MidiForm, reply: Reply) {
        if reply == Reply::Button(Answer::Ok) {
            self.midi_accepted(f.path, f.smf, f.o, f.piano);
        }
    }

    /// FILE ▸ Export MIDI... (`piano` = false) and the piano roll's Export as MIDI file...: the save dialog, unless
    /// there is nothing to export (spec §9, §10.8).
    pub(crate) fn export_midi_dialog(&mut self, piano: bool) {
        use crate::filedlg::{FileDlg, Mode};
        let s = &self.state;
        let file = if piano { self.piano_midi() } else { self.project_midi() };
        if !file.tracks.iter().any(|t| t.events.iter().any(|e| matches!(e.kind, Ev::NoteOn { .. }))) {
            let what = if piano { "The piano roll's channel has no notes in this pattern." } else { "There are no notes to export." };
            return self.tell(Kind::Info, "Export MIDI file", spans(&[what]));
        }
        let name = if piano { "untitled.mid".to_string() } else { format!("{}.mid", s.path.as_deref().map_or_else(|| "Untitled".to_string(), stem)) };
        let dlg = FileDlg::new(Mode::Save, "Export MIDI file", "midi", Then::MidiExport(piano))
            .filter("MIDI file", &["mid"])
            .start(s.path.as_deref().and_then(Path::parent))
            .name(&name);
        self.file_dialog(dlg);
    }

    fn piano_midi(&self) -> Smf {
        let s = &self.state;
        pattern_file(&s.project, s.pattern, crate::piano::channel(s).unwrap_or(0), s.tempo)
    }

    fn project_midi(&self) -> Smf {
        let s = &self.state;
        project_file(&s.project, s.song_mode, s.pattern, s.tempo)
    }

    /// The save dialog's file: written.
    pub(crate) fn midi_export_chosen(&mut self, path: PathBuf, piano: bool) {
        let path = if path.extension().is_none() { path.with_extension("mid") } else { path };
        let file = if piano { self.piano_midi() } else { self.project_midi() };
        self.state.message = Some(match std::fs::write(&path, os_smf::write(&file)) {
            Ok(()) => format!("Exported {}", path.display()),
            Err(e) => format!("Could not write {}: {e}", path.display()),
        });
        self.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(tick: u32, kind: Ev) -> Event {
        Event { tick, kind }
    }

    fn on(tick: u32, channel: u8, key: u8, velocity: u8) -> Event {
        ev(tick, Ev::NoteOn { channel, key, velocity })
    }

    fn off(tick: u32, channel: u8, key: u8, velocity: u8) -> Event {
        ev(tick, Ev::NoteOff { channel, key, velocity })
    }

    fn name(n: &str) -> Event {
        ev(0, Ev::TrackName(n.into()))
    }

    /// The measured file (spec §3): a conductor track, Piano on channel 1, Bass on 2, Drums on 10, at `ppq`.
    fn three(ppq: u16) -> Smf {
        let b = |beats: f64| (beats * f64::from(ppq)).round() as u32;
        let conductor = vec![
            name("Song title"),
            ev(0, Ev::Tempo(500_000)),
            ev(0, Ev::TimeSignature { numerator: 4, denominator: 4 }),
            ev(b(2.0), Ev::Marker("Verse".into())),
            ev(b(8.0), Ev::Tempo(os_smf::tempo_of_bpm(140.0))),
            ev(b(16.0), Ev::TimeSignature { numerator: 3, denominator: 4 }),
        ];
        let piano = vec![
            name("Piano"),
            ev(0, Ev::Controller { channel: 0, number: 7, value: 100 }),
            ev(0, Ev::Controller { channel: 0, number: 10, value: 32 }),
            on(0, 0, 60, 100),
            ev(b(1.0), Ev::PitchBend { channel: 0, value: 4096 }),
            off(b(1.0), 0, 60, 64),
            on(b(1.0), 0, 64, 127),
            off(b(2.0), 0, 64, 10),
        ];
        let bass = vec![name("Bass"), on(0, 1, 36, 110), off(b(0.5), 1, 36, 64)];
        let drums = vec![name("Drums"), on(0, 9, 36, 120), off(b(0.25), 9, 36, 64)];
        Smf { format: 1, ppq, tracks: [conductor, piano, bass, drums].into_iter().map(|events| Track { events }).collect() }
    }

    #[test]
    fn the_default_import_makes_a_channel_per_track_with_notes() {
        let imp = convert(&three(96), &Options::default(), 96);
        assert_eq!(imp.parts.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Piano", "Bass", "Drums"]);
        let piano = &imp.parts[0];
        let n = piano.notes[1];
        assert_eq!((n.position, n.length, n.key, n.velocity, n.release, n.group, n.pan, n.fine_pitch), (96, 96, 64, 127, 10, 0, 64, 120));
        assert_eq!(imp.parts[2].notes[0].group, 9, "the colour is the MIDI channel minus one");
        let c: Vec<(u32, u16, i32)> = piano.controls.iter().map(|c| (c.position, c.param(), c.value)).collect();
        assert_eq!(c, [(0, ControlPoint::VOLUME, 10000), (0, ControlPoint::PAN, 3200), (96, ControlPoint::PITCH, 100)]);
        let t: Vec<(u32, u32, i32)> = imp.tempos.iter().map(|c| (c.position, c.kind, c.value)).collect();
        assert_eq!(t, [(0, 0x4000_0005, 120_000), (768, 0x4000_0005, 140_000)]);
        assert_eq!(imp.tempo, Some(120.0));
        let sigs: Vec<(u32, &str, u8)> = imp.signatures.iter().map(|m| (m.position, m.name.as_str(), m.kind)).collect();
        assert_eq!(sigs, [(0, "4/4", 8), (1536, "3/4", 8)]);
        assert_eq!(imp.markers.iter().map(|m| (m.position, m.name.as_str(), m.kind)).collect::<Vec<_>>(), [(192, "Verse", 0)]);
        // At 480 ticks per quarter the same project.
        assert_eq!(convert(&three(480), &Options::default(), 96), imp);
    }

    /// §4: starts and ends round down, length at least 1; note-offs end the newest note of their key; notes never
    /// ended are left out; Realign shifts before rounding.
    #[test]
    fn timing_and_pairing() {
        let odd = Smf {
            format: 1,
            ppq: 480,
            tracks: vec![Track {
                events: vec![
                    name("Odd"),
                    on(1, 0, 0, 1),
                    on(3, 0, 0, 0),
                    on(241, 0, 60, 100),
                    on(720, 0, 60, 0),
                    on(962, 0, 62, 50),
                    on(1000, 0, 62, 70),
                    off(1500, 0, 62, 0),
                    off(1700, 0, 62, 0),
                    on(1920, 0, 127, 127),
                    off(3840, 0, 127, 0),
                    on(4000, 0, 50, 100),
                ],
            }],
        };
        let notes = |o: &Options| {
            let mut v: Vec<(u32, u32, u16, u8)> = convert(&odd, o, 96).parts[0].notes.iter().map(|n| (n.position, n.length, n.key, n.velocity)).collect();
            v.sort();
            v
        };
        assert_eq!(notes(&Options::default()), [(0, 1, 0, 1), (48, 96, 60, 100), (192, 148, 62, 50), (200, 100, 62, 70), (384, 384, 127, 127)]);
        let realign = Options { realign: true, ..Options::default() };
        assert_eq!(notes(&realign), [(0, 1, 0, 1), (48, 95, 60, 100), (192, 147, 62, 50), (199, 100, 62, 70), (383, 384, 127, 127)]);
        // Realign moves everything by the same shift; a tempo change before the first note holds at 0.
        let mut late = three(96);
        for t in &mut late.tracks[1..] {
            t.events.iter_mut().filter(|e| e.kind.channel().is_some()).for_each(|e| e.tick += 9 * 96);
        }
        let imp = convert(&late, &realign, 96);
        assert_eq!(imp.parts[0].notes[0].position, 0);
        assert_eq!(imp.tempos.iter().map(|c| (c.position, c.value)).collect::<Vec<_>>(), [(0, 140_000)]);
        assert_eq!(imp.tempo, Some(140.0));
        assert_eq!(imp.signatures.iter().map(|m| (m.position, m.name.as_str())).collect::<Vec<_>>(), [(0, "4/4"), (7 * 96, "3/4")]);
        // With zero-velocity note-ons as notes, the two notes they ended never end.
        let zero = Options { zero_velocity: true, ..Options::default() };
        assert_eq!(notes(&zero), [(192, 148, 62, 50), (200, 100, 62, 70), (384, 384, 127, 127)]);
    }

    /// §5, §6: one track or some channels; a type 0 file split per MIDI channel keeps every note (FL loses some).
    #[test]
    fn tracks_channels_and_type_0() {
        let only_bass = convert(&three(96), &Options { track: Some(2), ..Options::default() }, 96);
        assert_eq!(only_bass.parts.len(), 1);
        assert_eq!(only_bass.parts[0].name, "Bass");
        assert_eq!(only_bass.tempos.len(), 2, "the tempo comes whichever track is chosen");
        assert!(only_bass.markers.is_empty());
        let mut channels = [true; 16];
        channels[9] = false;
        let no_drums = convert(&three(96), &Options { channels, ..Options::default() }, 96);
        assert_eq!(no_drums.parts.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Piano", "Bass"]);
        let t0 = Smf {
            format: 0,
            ppq: 96,
            tracks: vec![Track {
                events: vec![
                    name("Type zero"),
                    on(0, 0, 60, 100),
                    on(0, 1, 40, 100),
                    on(0, 9, 36, 100),
                    off(24, 9, 36, 64),
                    off(96, 0, 60, 64),
                    on(96, 0, 62, 100),
                    off(192, 0, 62, 64),
                    off(192, 1, 40, 64),
                ],
            }],
        };
        let per_channel = convert(&t0, &options_for(&t0), 96);
        let got: Vec<(&str, usize)> = per_channel.parts.iter().map(|p| (p.name.as_str(), p.notes.len())).collect();
        assert_eq!(got, [("Channel #1", 2), ("Channel #2", 1), ("Channel #10", 1)]);
        let per_track = convert(&t0, &Options { per_track: true, ..Options::default() }, 96);
        assert_eq!(per_track.parts.len(), 1);
        assert_eq!(per_track.parts[0].notes.len(), 4);
        assert!(necessary(&t0) && necessary(&three(96)));
        let single = Smf { format: 1, ppq: 96, tracks: vec![Track { events: vec![on(0, 0, 60, 100), off(96, 0, 60, 0)] }] };
        assert!(!necessary(&single));
    }

    /// Issue #159's file: type 0, 480 PPQ, 110 BPM, 4/4, one note at tick 384000 (bar 201), as a MIDI pack places a
    /// loop.
    const LATE_NOTE: [u8; 71] = [
        0x4d, 0x54, 0x68, 0x64, 0, 0, 0, 6, 0, 0, 0, 1, 0x01, 0xe0, 0x4d, 0x54, 0x72, 0x6b, 0, 0, 0, 0x31, 0, 0xff, 3, 0x0f, b'L',
        b'a', b't', b'e', b' ', b'M', b'I', b'D', b'I', b' ', b'r', b'e', b'p', b'r', b'o', 0, 0xff, 0x51, 3, 0x08, 0x52, 0x2f,
        0, 0xff, 0x58, 4, 4, 2, 0x18, 8, 0x97, 0xb8, 0, 0x90, 0x3c, 0x64, 0x81, 0x70, 0x80, 0x3c, 0x40, 0, 0xff, 0x2f, 0,
    ];

    /// One note on channel 1 at `tick`, after a time signature at 0 when given.
    fn one_note(tick: u32, signature: Option<(u8, u8)>) -> Smf {
        let mut events: Vec<Event> =
            signature.map(|(numerator, denominator)| ev(0, Ev::TimeSignature { numerator, denominator })).into_iter().collect();
        events.extend([on(tick, 0, 60, 100), off(tick + 240, 0, 60, 0)]);
        Smf { format: 0, ppq: 480, tracks: vec![Track { events }] }
    }

    /// §10.5: a first note a bar or more in is a leading silence; the form shows for it with Realign on, and the
    /// import starts at 0. A pickup inside the first bar is not one.
    #[test]
    fn leading_silence_shows_the_form_with_realign_on() {
        let late = os_smf::read(&LATE_NOTE).unwrap();
        assert!(leading_silence(&late));
        assert!(necessary(&late), "one track and one channel, but the silence is a choice to make");
        let o = options_for(&late);
        assert!(o.realign);
        assert_eq!(convert(&late, &Options::default(), 96).parts[0].notes[0].position, 76_800, "bar 201 with Realign off");
        assert_eq!(convert(&late, &o, 96).parts[0].notes[0].position, 0);
        for (tick, signature, silent) in [
            (3 * 480, None, false),
            (4 * 480 - 1, None, false),
            (4 * 480, None, true),
            (3 * 480, Some((3, 4)), true),
            (3 * 480, Some((6, 8)), true),
            (5 * 480, Some((7, 4)), false),
        ] {
            let f = one_note(tick, signature);
            assert_eq!(leading_silence(&f), silent, "first note at {tick}, signature {signature:?}");
            assert_eq!(necessary(&f), silent);
            assert_eq!(options_for(&f).realign, silent);
        }
        // A velocity-0 note-on is a note-off, not a first note.
        let mut f = one_note(8 * 480, None);
        f.tracks[0].events.insert(0, on(0, 0, 61, 0));
        assert!(leading_silence(&f));
    }

    #[test]
    fn pitch_bends_round_toward_zero() {
        let f = Smf {
            format: 1,
            ppq: 96,
            tracks: vec![Track {
                events: vec![
                    on(0, 2, 60, 100),
                    ev(1, Ev::PitchBend { channel: 2, value: 8191 }),
                    ev(2, Ev::PitchBend { channel: 2, value: -8192 }),
                    ev(3, Ev::Controller { channel: 2, number: 10, value: 127 }),
                    ev(4, Ev::Controller { channel: 2, number: 1, value: 64 }),
                    off(96, 2, 60, 0),
                ],
            }],
        };
        let c: Vec<i32> = convert(&f, &Options::default(), 96).parts[0].controls.iter().map(|c| c.value).collect();
        assert_eq!(c, [199, -200, 12700]);
    }

    fn test_app() -> App {
        let mut app = App::new(None);
        app.state.project = crate::app::test_project();
        app.state.song_mode = false;
        app
    }

    fn write_three(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("os_midi_{tag}_{}.mid", std::process::id()));
        std::fs::write(&path, os_smf::write(&three(96))).unwrap();
        path
    }

    fn form(app: &App) -> Option<&MidiForm> {
        match &app.state.dialogs.last()?.body {
            Body::Project(Form::Midi(f)) => Some(f),
            _ => None,
        }
    }

    /// FILE ▸ Import ▸ MIDI file: the form for a file of several tracks; Accept starts a new project of one pattern
    /// (spec §3).
    #[test]
    fn import_into_a_new_project() {
        let path = write_three("new");
        let mut app = test_app();
        crate::dialog::fake::answer(Some(path.clone()));
        app.import_midi_dialog(false);
        let f = form(&app).expect("the form shows for a file of three tracks with notes");
        assert_eq!(f.track_items(), ["All tracks", "1. Song title", "2. Piano", "3. Bass", "4. Drums"]);
        app.dialog_click(crate::dialog::DHit::Form(FHit::Main));
        let p = &app.state.project;
        assert_eq!(p.ppq, 96);
        assert_eq!(p.channels.iter().map(|c| c.name.as_deref().unwrap_or("")).collect::<Vec<_>>(), ["Piano", "Bass", "Drums"]);
        assert_eq!(p.channels.iter().map(|c| c.mixer_track).collect::<Vec<_>>(), [1, 2, 3]);
        assert!(p.channels.iter().all(|c| c.plugin.as_deref() == Some(crate::osc3::FILE_NAME)), "Lemon Trio plays them");
        assert_eq!(p.patterns.len(), 1);
        assert_eq!(p.patterns[0].name.as_deref(), Some(stem(&path).as_str()));
        assert_eq!(p.patterns[0].notes.len(), 4);
        assert!(p.arrangement().is_none_or(|a| a.clips.is_empty()), "nothing on the playlist");
        assert_eq!(app.state.tempo, 120.0);
        assert!(!app.state.song_mode);
        assert_eq!(app.state.rack.groups[0].name, stem(&path));
        assert_eq!(app.state.rack.filter, crate::rack::Filter::Group(0));
        assert!(app.unsaved());
        let _ = std::fs::remove_file(path);
    }

    /// Start new project off: the channels are added, the notes go into the current pattern; one undo step takes all
    /// of it back and redo brings it again (spec §7, §10.7).
    #[test]
    fn import_into_the_open_project_undoes_in_one_step() {
        let path = write_three("append");
        let mut app = test_app();
        crate::playlist::arr_mut(&mut app.state);
        let before = app.state.project.clone();
        let rows = before.channels.len();
        crate::dialog::fake::answer(Some(path.clone()));
        app.import_midi_dialog(false);
        app.dialog_click(crate::dialog::DHit::Form(FHit::Check(0)));
        assert!(!form(&app).unwrap().o.new_project);
        app.dialog_click(crate::dialog::DHit::Form(FHit::Main));
        assert_eq!(app.state.project.channels.len(), rows + 3);
        assert_eq!(app.state.project.channels[rows].name.as_deref(), Some("Piano"));
        assert_eq!(app.state.project.channels[rows].mixer_track, 1, "routing starts again at track 1");
        let notes = app.state.project.pattern(1).unwrap().notes.len();
        assert_eq!(notes, before.pattern(1).map_or(0, |p| p.notes.len()) + 4);
        let after = app.state.project.clone();
        assert_eq!(crate::history::undo(&mut app.state), Some(IMPORT_STEP));
        assert_eq!(app.state.project, before);
        crate::history::redo(&mut app.state);
        assert_eq!(app.state.project.channels.len(), after.channels.len());
        assert_eq!(app.state.project.pattern(1), after.pattern(1));
        let _ = std::fs::remove_file(path);
    }

    /// The piano roll's import replaces the channel's notes unless Blend is on, and selects what it brought.
    #[test]
    fn piano_roll_import() {
        let path = write_three("piano");
        let mut app = test_app();
        app.state.selected_channel = 1;
        let ch = app.state.project.channels[1].index;
        crate::patterns::pat_mut(&mut app.state, 1).notes = vec![Note::step(ch, 0), Note::step(0, 0)];
        crate::dialog::fake::answer(Some(path.clone()));
        app.import_midi_dialog(true);
        let f = form(&app).unwrap();
        assert_eq!(f.lamp(0).map(|l| l.0), Some(BLEND));
        assert!(f.lamp(3).is_none());
        app.dialog_click(crate::dialog::DHit::Form(FHit::Main));
        let pat = app.state.project.pattern(1).unwrap();
        assert_eq!(pat.notes.iter().filter(|n| n.channel == ch).count(), 4, "the step was replaced");
        assert_eq!(pat.notes.iter().filter(|n| n.channel == 0).count(), 1, "other channels keep theirs");
        assert_eq!(app.state.piano.selection(1, Some(ch)).len(), 4);
        let _ = std::fs::remove_file(path);
    }

    /// Shift+Ctrl+M is FILE ▸ Export MIDI...: the save dialog, the file written with the .mid it was not given. A
    /// dropped .mid opens the form; Esc closes it and imports nothing.
    #[test]
    fn shortcut_and_drop() {
        let mut app = test_app();
        crate::patterns::pat_mut(&mut app.state, 1).notes = vec![Note { length: 48, ..Note::step(0, 0) }];
        let out = std::env::temp_dir().join(format!("os_midi_export_{}", std::process::id()));
        crate::dialog::fake::answer(Some(out.clone()));
        app.shift = true;
        app.key(&Key::Character("M".into()), true, false);
        app.shift = false;
        let written = out.with_extension("mid");
        let f = os_smf::read(&std::fs::read(&written).expect("Export MIDI wrote the file")).unwrap();
        assert_eq!(f.tracks.len(), 2);
        let _ = std::fs::remove_file(written);
        // The piano roll's channel without notes: a message, no save dialog.
        app.state.selected_channel = 1;
        app.export_midi_dialog(true);
        assert!(matches!(app.state.dialogs.last().map(|d| &d.body), Some(crate::dialog::Body::Msg(_))));
        app.key(&Key::Named(NamedKey::Escape), false, false);
        let path = write_three("drop");
        let before = app.state.project.clone();
        app.drop_file(path.clone());
        assert!(form(&app).is_some());
        app.key(&Key::Named(NamedKey::Escape), false, false);
        assert!(!app.modal_open());
        assert_eq!(app.state.project, before);
        let _ = std::fs::remove_file(path);
    }

    /// Issue #159: a dropped file with a leading silence shows the form with Realign on and lands at bar 1; that
    /// Realign is not remembered for the next file (§10.5).
    #[test]
    fn dropped_late_file_lands_at_the_start() {
        let path = std::env::temp_dir().join(format!("os_midi_late_{}.mid", std::process::id()));
        std::fs::write(&path, LATE_NOTE).unwrap();
        let mut app = test_app();
        app.drop_file(path.clone());
        assert!(form(&app).expect("the form shows for the late file").o.realign);
        app.dialog_click(crate::dialog::DHit::Form(FHit::Main));
        let notes = &app.state.project.patterns[0].notes;
        assert_eq!(notes.iter().map(|n| (n.position, n.length, n.key)).collect::<Vec<_>>(), [(0, 48, 60)]);
        assert_eq!(app.state.tempo, 110.026);
        assert_eq!(LAST.with_borrow(|l| l.as_ref().map(|o| o.realign)), Some(false));
        assert!(!options_for(&one_note(3 * 480, None)).realign, "a pickup imports where it is");
        // Realign chosen for an ordinary file is remembered, as the other settings are.
        LAST.set(Some(Options { realign: true, ..Options::default() }));
        let into_this = Options { realign: false, new_project: false, ..Options::default() };
        app.midi_accepted(path.clone(), os_smf::read(&LATE_NOTE).unwrap(), into_this, false);
        assert_eq!(LAST.with_borrow(|l| l.as_ref().map(|o| o.realign)), Some(true));
        let _ = std::fs::remove_file(path);
    }

    /// §10.6: a .mid opened as a project (the command line) goes through the import, not the project reader.
    #[test]
    fn a_midi_file_opened_as_a_project_is_imported() {
        let path = write_three("open");
        let mut app = test_app();
        app.open(path.clone());
        assert!(form(&app).is_some(), "the form shows for the three-track file");
        assert!(app.state.message.as_deref().is_none_or(|m| !m.starts_with("Could not open")));
        let _ = std::fs::remove_file(path);
    }

    /// The MIDI import window rows (General ▸ Input ▸ Manage shortcuts) as they come: no modifier shows the form when
    /// necessary, Alt every time, Shift never. A file without notes says so and changes nothing.
    #[test]
    fn modifiers_and_empty_files() {
        assert_eq!(show_for(false, false), Show::WhenNecessary);
        assert_eq!(show_for(true, false), Show::Always);
        assert_eq!(show_for(false, true), Show::Never);
        let path = std::env::temp_dir().join(format!("os_midi_empty_{}.mid", std::process::id()));
        let f = Smf { format: 1, ppq: 96, tracks: vec![Track { events: vec![ev(0, Ev::Tempo(500_000))] }] };
        std::fs::write(&path, os_smf::write(&f)).unwrap();
        let mut app = test_app();
        let before = app.state.project.clone();
        app.midi_file_chosen(path.clone(), false);
        let Some(crate::dialog::Body::Msg(m)) = app.state.dialogs.last().map(|d| &d.body) else { panic!("a message") };
        assert!(m.plain().contains("has no notes"), "{}", m.plain());
        assert_eq!(app.state.project, before);
        let _ = std::fs::remove_file(path);
    }

    /// Export: the piano roll's file follows §9; FILE's export brings back what an import made.
    #[test]
    fn exports_read_back() {
        let imp = convert(&three(96), &Options::default(), 96);
        let mut p = crate::app::empty_project();
        p.channels = (0..3).map(|i| {
            let mut c = crate::app::test_project().channels[0].clone();
            c.index = i;
            c.name = Some(imp.parts[usize::from(i)].name.clone());
            c
        }).collect();
        let pat = &mut p.patterns[0];
        for (i, part) in imp.parts.iter().enumerate() {
            pat.notes.extend(part.notes.iter().map(|x| Note { channel: i as u16, ..*x }));
        }
        pat.markers = imp.signatures.clone();
        pat.controls = imp.tempos.clone();
        let drums = pattern_file(&p, 1, 2, 120.0);
        assert_eq!(drums.tracks[1].events[0].kind, Ev::TrackName("Drums 10".into()));
        let sigs: Vec<&Ev> = drums.tracks[0].events.iter().map(|e| &e.kind).filter(|k| matches!(k, Ev::TimeSignature { .. })).collect();
        assert_eq!(sigs.len(), 2);
        let back = os_smf::read(&os_smf::write(&project_file(&p, false, 1, 120.0))).unwrap();
        assert_eq!(back.tracks.len(), 4);
        let again = convert(&back, &Options::default(), 96);
        assert_eq!(again.parts.iter().map(|q| q.name.as_str()).collect::<Vec<_>>(), ["Piano", "Bass", "Drums"]);
        for (a, b) in again.parts.iter().zip(&imp.parts) {
            assert_eq!(a.notes, b.notes);
        }
        assert_eq!(again.tempos, imp.tempos);
    }
}
