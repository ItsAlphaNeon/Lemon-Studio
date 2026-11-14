// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon MIDI Out: the built-in generator that plays a "MIDI out" channel by sending its notes to a MIDI output port.
//! The channel keeps the file's plugin state (event 213) byte for byte; Lemon Studio reads its settings from it and
//! rewrites only the fields it edits. The instrument makes no sound: it puts its messages in a queue the MIDI output
//! thread (`midi_out`) sends to the output devices with the plugin's port.
//!
// spec: plugins/midi-out.md §2 (state), §3 (what it sends), §6 (Lemon MIDI Out)

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
#[cfg(not(any(test, feature = "test-util")))]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use crossbeam_queue::ArrayQueue;
use os_engine::{Instrument, TransportInfo};
use os_model::{Channel, ChannelKind};

/// What the ADD menu and a new channel call it.
pub const LEMON_MIDI_OUT: &str = "MIDI Out";
/// The plugin name project files store for it (event 201).
pub const FILE_NAME: &str = "MIDI out";

/// Whether a channel holds a MIDI Out.
pub fn is_midi_out(ch: &Channel) -> bool {
    ch.kind == ChannelKind::Plugin && ch.plugin.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(FILE_NAME))
}

// The state's fields (spec §2).
const VERSION: u32 = 7;
const CHANNEL: usize = 4;
const BANK: [usize; 2] = [12, 16];
const PORT: usize = 24;
const SEND_LEVELS: usize = 28;
const MAP_COLOR: usize = 29;
const PATCH: usize = 30;
/// The 72 knob values (8 pages of 9), then the controller records.
const KNOBS: usize = 34;
const RECORDS: usize = KNOBS + 4 * 72;

/// "On reset" bits: a set bit means the item is unticked.
pub const RESET_PITCH: u32 = 1;
pub const RESET_RANGE: u32 = 2;
pub const RESET_CUT: u32 = 4;
pub const RESET_PAN: u32 = 8;
pub const RESET_VOLUME: u32 = 16;
pub const RESET_PROGRAM: u32 = 32;

/// The default controller records: knob index, controller, full name, short name (spec §1).
const RECORDS_DEFAULT: [(u8, u16, &str, &str); 12] = [
    (0, 1, "Modulation wheel", "Mod"),
    (1, 2, "Breath control", "Breath"),
    (2, 4, "Foot controller", "Foot"),
    (3, 5, "Portamento time", "Port. Time"),
    (4, 91, "Reverb send level", "Reverb"),
    (5, 93, "Chorus send level", "Chorus"),
    (9, 64, "Damper pedal", "Damper"),
    (10, 65, "Portamento", "Porta."),
    (11, 66, "Sustenuto", "Sustenuto"),
    (12, 67, "Soft pedal", "Soft pedal"),
    (13, 68, "Legato Footswitch", "Legato"),
    (14, 69, "Hold 2", "Hold 2"),
];

/// The settings Lemon Studio edits (spec §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// MIDI channel, 0..=15.
    pub channel: u8,
    pub port: u8,
    /// 1..=128; 0 = none.
    pub patch: u8,
    /// The two bank boxes, 0..=127; -1 = none.
    pub bank: [i32; 2],
    /// Send pan, volume and pitch.
    pub send_levels: bool,
    /// Map note color to MIDI channel (kept; not played yet).
    pub map_color: bool,
    /// The "On reset" items unticked (`RESET_*` bits).
    pub unticked: u32,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { channel: 0, port: 0, patch: 1, bank: [-1, -1], send_levels: true, map_color: false, unticked: 0 }
    }
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn put_u32(b: &mut [u8], at: usize, v: u32) {
    if let Some(d) = b.get_mut(at..at + 4) {
        d.copy_from_slice(&v.to_le_bytes());
    }
}

/// Whether `b` is a state this page describes (version 7, the fixed part whole); others are kept but not edited.
pub fn known(b: &[u8]) -> bool {
    u32_at(b, 0) == Some(VERSION) && b.len() > RECORDS + 4
}

/// The settings a state holds; a state Lemon Studio doesn't know gives the defaults.
pub fn settings(b: &[u8]) -> Settings {
    let d = Settings::default();
    if !known(b) {
        return d;
    }
    let bank = |i: usize| u32_at(b, BANK[i]).map_or(-1, |v| (v as i32).clamp(-1, 127));
    Settings {
        channel: u32_at(b, CHANNEL).map_or(0, |v| v.min(15) as u8),
        port: u32_at(b, PORT).map_or(0, |v| v.min(255) as u8),
        patch: b[PATCH].min(128),
        bank: [bank(0), bank(1)],
        send_levels: b[SEND_LEVELS] != 0,
        map_color: b[MAP_COLOR] != 0,
        unticked: u32_at(b, b.len() - 4).unwrap_or(0) & 0x3f,
    }
}

/// Writes `s` into state `b` in place, leaving the other bytes (knobs, records, page names) as they are. A state Lemon
/// Studio doesn't know is replaced by the default one first.
pub fn write(b: &mut Vec<u8>, s: &Settings) {
    if !known(b) {
        *b = default_state();
    }
    put_u32(b, CHANNEL, u32::from(s.channel.min(15)));
    put_u32(b, PORT, u32::from(s.port));
    for i in 0..2 {
        put_u32(b, BANK[i], s.bank[i].clamp(-1, 127) as u32);
    }
    b[SEND_LEVELS] = u8::from(s.send_levels);
    b[MAP_COLOR] = u8::from(s.map_color);
    b[PATCH] = s.patch.min(128);
    let end = b.len() - 4;
    let rest = u32_at(b, end).unwrap_or(0) & !0x3f;
    put_u32(b, end, rest | (s.unticked & 0x3f));
}

fn pascal(b: &mut Vec<u8>, s: &str) {
    b.push(s.len() as u8);
    b.extend_from_slice(s.as_bytes());
}

/// The state of a new MIDI Out (spec §2): 967 bytes, the default controller records and page names.
pub fn default_state() -> Vec<u8> {
    let mut b = Vec::with_capacity(967);
    b.extend_from_slice(&VERSION.to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes());
    for _ in 0..3 {
        b.extend_from_slice(&(-1i32).to_le_bytes());
    }
    debug_assert_eq!(b.len(), 20);
    b.extend_from_slice(&0u32.to_le_bytes()); // the page shown
    b.extend_from_slice(&0u32.to_le_bytes()); // port
    b.extend_from_slice(&[1, 0, 1, 0, 0, 0]); // send levels, map colour, patch 1, 0, u16 0
    b.resize(RECORDS, 0);
    for (knob, cc, full, short) in RECORDS_DEFAULT {
        b.push(knob);
        b.extend_from_slice(&cc.to_le_bytes());
        b.extend_from_slice(&[0, 0x80]);
        b.extend_from_slice(&0u32.to_le_bytes()); // CC
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&127u32.to_le_bytes());
        b.extend_from_slice(&(-1i32).to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        pascal(&mut b, full);
        pascal(&mut b, short);
    }
    b.push(0xff);
    for name in ["Basic controllers", "Boolean controllers", "Page 3", "Page 4", "Page 5", "Page 6", "Page 7", "Page 8"] {
        pascal(&mut b, name);
    }
    b.extend_from_slice(&0u32.to_le_bytes());
    b
}

/// A default state with `s` written in.
pub fn state_with(s: &Settings) -> Vec<u8> {
    let mut b = default_state();
    write(&mut b, s);
    b
}

/// The General MIDI program names, patch 1 to 128.
pub const GM_NAMES: [&str; 128] = [
    "Acoustic Grand Piano", "Bright Acoustic Piano", "Electric Grand Piano", "Honky-tonk Piano", "Electric Piano 1", "Electric Piano 2", "Harpsichord", "Clavinet",
    "Celesta", "Glockenspiel", "Music Box", "Vibraphone", "Marimba", "Xylophone", "Tubular Bells", "Dulcimer",
    "Drawbar Organ", "Percussive Organ", "Rock Organ", "Church Organ", "Reed Organ", "Accordion", "Harmonica", "Tango Accordion",
    "Acoustic Guitar (nylon)", "Acoustic Guitar (steel)", "Electric Guitar (jazz)", "Electric Guitar (clean)", "Electric Guitar (muted)", "Overdriven Guitar", "Distortion Guitar", "Guitar Harmonics",
    "Acoustic Bass", "Electric Bass (finger)", "Electric Bass (pick)", "Fretless Bass", "Slap Bass 1", "Slap Bass 2", "Synth Bass 1", "Synth Bass 2",
    "Violin", "Viola", "Cello", "Contrabass", "Tremolo Strings", "Pizzicato Strings", "Orchestral Harp", "Timpani",
    "String Ensemble 1", "String Ensemble 2", "Synth Strings 1", "Synth Strings 2", "Choir Aahs", "Voice Oohs", "Synth Voice", "Orchestra Hit",
    "Trumpet", "Trombone", "Tuba", "Muted Trumpet", "French Horn", "Brass Section", "Synth Brass 1", "Synth Brass 2",
    "Soprano Sax", "Alto Sax", "Tenor Sax", "Baritone Sax", "Oboe", "English Horn", "Bassoon", "Clarinet",
    "Piccolo", "Flute", "Recorder", "Pan Flute", "Blown Bottle", "Shakuhachi", "Whistle", "Ocarina",
    "Lead 1 (square)", "Lead 2 (sawtooth)", "Lead 3 (calliope)", "Lead 4 (chiff)", "Lead 5 (charang)", "Lead 6 (voice)", "Lead 7 (fifths)", "Lead 8 (bass + lead)",
    "Pad 1 (new age)", "Pad 2 (warm)", "Pad 3 (polysynth)", "Pad 4 (choir)", "Pad 5 (bowed)", "Pad 6 (metallic)", "Pad 7 (halo)", "Pad 8 (sweep)",
    "FX 1 (rain)", "FX 2 (soundtrack)", "FX 3 (crystal)", "FX 4 (atmosphere)", "FX 5 (brightness)", "FX 6 (goblins)", "FX 7 (echoes)", "FX 8 (sci-fi)",
    "Sitar", "Banjo", "Shamisen", "Koto", "Kalimba", "Bagpipe", "Fiddle", "Shanai",
    "Tinkle Bell", "Agogo", "Steel Drums", "Woodblock", "Taiko Drum", "Melodic Tom", "Synth Drum", "Reverse Cymbal",
    "Guitar Fret Noise", "Breath Noise", "Seashore", "Bird Tweet", "Telephone Ring", "Helicopter", "Applause", "Gunshot",
];

/// "1 Acoustic Grand Piano", or "---" for none.
pub fn patch_text(patch: u8) -> String {
    match patch {
        1..=128 => format!("{patch} {}", GM_NAMES[usize::from(patch - 1)]),
        _ => "---".into(),
    }
}

/// A message for the output: the port it goes to and its bytes (`len` of them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Out {
    pub port: u8,
    pub bytes: [u8; 3],
    pub len: u8,
}

impl Out {
    pub fn data(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len.min(3))]
    }
}

/// Where the app's instruments put their messages; the MIDI output thread takes them.
pub fn queue() -> &'static Arc<ArrayQueue<Out>> {
    static Q: OnceLock<Arc<ArrayQueue<Out>>> = OnceLock::new();
    Q.get_or_init(|| Arc::new(ArrayQueue::new(8192)))
}

/// A render or export is running: the instruments send nothing (`Plugins::set_offline`).
#[cfg(not(any(test, feature = "test-util")))]
static OFFLINE: AtomicBool = AtomicBool::new(false);

#[cfg(any(test, feature = "test-util"))]
thread_local! {
    /// Tests: the switch for this thread, so tests running side by side don't see each other's exports.
    static OFFLINE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub fn set_offline(offline: bool) {
    #[cfg(not(any(test, feature = "test-util")))]
    OFFLINE.store(offline, Ordering::Relaxed);
    #[cfg(any(test, feature = "test-util"))]
    OFFLINE.with(|o| o.set(offline));
}

fn offline() -> bool {
    #[cfg(not(any(test, feature = "test-util")))]
    return OFFLINE.load(Ordering::Relaxed);
    #[cfg(any(test, feature = "test-util"))]
    OFFLINE.with(|o| o.get())
}

/// What the channel gives its instrument besides the settings: the channel volume knob (0..1), pan (-1..1) and the
/// pitch knob's range in semitones.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Live {
    pub settings: Settings,
    pub volume: f64,
    pub pan: f64,
    pub bend_range: u8,
}

impl Live {
    /// Channel `ch`'s (a MIDI Out channel).
    pub fn of(ch: &Channel) -> Live {
        let settings = ch.plugin_state.as_deref().map_or_else(Settings::default, settings);
        Live { settings, volume: ch.volume, pan: ch.pan, bend_range: ch.misc.pitch_range }
    }
}

/// The reset bundle (spec §3, the order measured; bank select is Lemon Studio's): pan, volume and pitch bend when
/// Send pan, volume and pitch is on, the pitch-bend range, the bank and the program change, as the On reset checks
/// allow.
pub fn reset_messages(l: &Live) -> Vec<([u8; 3], u8)> {
    let s = &l.settings;
    let ch = s.channel & 0x0f;
    let on = |bit: u32| s.unticked & bit == 0;
    let cc = |n: u8, v: u8| ([0xb0 | ch, n, v.min(127)], 3u8);
    let mut v = Vec::new();
    if s.send_levels && on(RESET_PAN) {
        v.push(cc(10, (64.0 + l.pan * 64.0).round().clamp(0.0, 127.0) as u8));
    }
    if s.send_levels && on(RESET_VOLUME) {
        v.push(cc(7, (l.volume * 128.0).round().clamp(0.0, 127.0) as u8));
    }
    if s.send_levels && on(RESET_PITCH) {
        v.push(([0xe0 | ch, 0x00, 0x40], 3));
    }
    if on(RESET_RANGE) {
        v.extend([cc(101, 0), cc(100, 0), cc(6, l.bend_range.min(127))]);
    }
    if on(RESET_PROGRAM) {
        if s.bank[0] >= 0 {
            v.push(cc(0, s.bank[0] as u8));
        }
        if s.bank[1] >= 0 {
            v.push(cc(32, s.bank[1] as u8));
        }
        if s.patch > 0 {
            v.push(([0xc0 | ch, s.patch - 1, 0], 2));
        }
    }
    v
}

/// The instrument of one MIDI Out channel.
pub struct MidiOutInstrument {
    pub live: Live,
    sink: Arc<ArrayQueue<Out>>,
    /// The keys sounding (note on sent, no note off yet), with the channel and port they went to.
    held: Vec<(u8, u8, u8)>,
    /// The song is playing (the last `transport` said so) and where it was.
    playing: bool,
    last_frame: u64,
    /// `transport` was called since the last `process`.
    told: bool,
}

impl MidiOutInstrument {
    pub fn new(live: Live, sink: Arc<ArrayQueue<Out>>) -> MidiOutInstrument {
        MidiOutInstrument { live, sink, held: Vec::new(), playing: false, last_frame: 0, told: false }
    }

    fn send(&self, bytes: [u8; 3], len: u8) {
        self.push(self.live.settings.port, bytes, len);
    }

    fn push(&self, port: u8, bytes: [u8; 3], len: u8) {
        if !offline() {
            let _ = self.sink.push(Out { port, bytes, len });
        }
    }

    fn send_reset(&self) {
        for (b, n) in reset_messages(&self.live) {
            self.send(b, n);
        }
    }

    /// Note off for every key still sounding.
    fn cut(&mut self) {
        for (ch, key, port) in std::mem::take(&mut self.held) {
            self.push(port, [0x80 | ch, key, 64], 3);
        }
    }
}

impl Drop for MidiOutInstrument {
    fn drop(&mut self) {
        self.cut();
    }
}

impl Instrument for MidiOutInstrument {
    fn note_on(&mut self, _offset: u32, key: u8, velocity: f32) {
        let ch = self.live.settings.channel & 0x0f;
        let v = (velocity * 128.0).round().clamp(1.0, 127.0) as u8;
        if let Some(i) = self.held.iter().position(|h| h.1 == key) {
            let (c, k, p) = self.held.remove(i);
            self.push(p, [0x80 | c, k, 64], 3);
        }
        if offline() {
            return;
        }
        self.send([0x90 | ch, key & 0x7f, v], 3);
        self.held.push((ch, key & 0x7f, self.live.settings.port));
    }

    fn note_off(&mut self, _offset: u32, key: u8) {
        if let Some(i) = self.held.iter().position(|h| h.1 == key & 0x7f) {
            let (c, k, p) = self.held.remove(i);
            self.push(p, [0x80 | c, k, 64], 3);
        }
    }

    fn process(&mut self, _rate: u32, left: &mut [f32], right: &mut [f32]) {
        left.fill(0.0);
        right.fill(0.0);
        if !std::mem::take(&mut self.told) {
            self.playing = false;
        }
    }

    fn transport(&mut self, info: &TransportInfo) {
        // Playback starts, loops or jumps back: the reset bundle.
        if info.playing && (!self.playing || info.frame < self.last_frame) {
            self.send_reset();
        }
        self.playing = info.playing;
        self.last_frame = info.frame;
        self.told = true;
    }

    fn reset(&mut self) {
        if self.live.settings.unticked & RESET_CUT == 0 {
            self.cut();
        } else {
            self.held.clear();
        }
        self.playing = false;
    }
}

/// The settings the channel window shows as knobs (spec §6.3), each over its values: Channel 1..16, Port 0..255, Patch
/// none and 1..128, the two Bank boxes none and 0..127, and the two checks.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MoKnob {
    Channel,
    Port,
    Patch,
    Bank,
    Bank2,
    SendLevels,
    MapColor,
}

impl MoKnob {
    pub const ALL: [MoKnob; 7] = [MoKnob::Channel, MoKnob::Port, MoKnob::Patch, MoKnob::Bank, MoKnob::Bank2, MoKnob::SendLevels, MoKnob::MapColor];

    /// A check a click flips.
    pub fn switch(self) -> bool {
        matches!(self, MoKnob::SendLevels | MoKnob::MapColor)
    }

    /// The number of steps between its first and last value.
    fn steps(self) -> f32 {
        match self {
            MoKnob::Channel => 15.0,
            MoKnob::Port => 255.0,
            MoKnob::Patch | MoKnob::Bank | MoKnob::Bank2 => 128.0,
            MoKnob::SendLevels | MoKnob::MapColor => 1.0,
        }
    }

    /// One value per wheel notch.
    pub fn notch(self) -> f32 {
        1.0 / self.steps()
    }

    fn raw(self, s: &Settings) -> i32 {
        match self {
            MoKnob::Channel => i32::from(s.channel),
            MoKnob::Port => i32::from(s.port),
            MoKnob::Patch => i32::from(s.patch),
            MoKnob::Bank => s.bank[0] + 1,
            MoKnob::Bank2 => s.bank[1] + 1,
            MoKnob::SendLevels => i32::from(s.send_levels),
            MoKnob::MapColor => i32::from(s.map_color),
        }
    }

    /// The control's 0..1 position for `s`.
    pub fn pos(self, s: &Settings) -> f32 {
        (self.raw(s) as f32 / self.steps()).clamp(0.0, 1.0)
    }

    /// Stores position `v` (0..1) into `s`.
    pub fn store(self, s: &mut Settings, v: f32) {
        let n = (v.clamp(0.0, 1.0) * self.steps()).round() as i32;
        match self {
            MoKnob::Channel => s.channel = n as u8,
            MoKnob::Port => s.port = n as u8,
            MoKnob::Patch => s.patch = n as u8,
            MoKnob::Bank => s.bank[0] = n - 1,
            MoKnob::Bank2 => s.bank[1] = n - 1,
            MoKnob::SendLevels => s.send_levels = n != 0,
            MoKnob::MapColor => s.map_color = n != 0,
        }
    }

    /// Its row's caption in the channel window.
    pub fn caption(self) -> &'static str {
        match self {
            MoKnob::Channel => "Channel",
            MoKnob::Port => "Port",
            MoKnob::Patch => "Patch",
            MoKnob::Bank => "Bank",
            MoKnob::Bank2 => "Bank 2",
            MoKnob::SendLevels => "Send pan, volume and pitch",
            MoKnob::MapColor => "Map note color to MIDI channel",
        }
    }

    /// Its value as the window shows it: "10", "1 Acoustic Grand Piano", "---".
    pub fn value(self, s: &Settings) -> String {
        let bank = |b: i32| if b < 0 { "---".to_string() } else { b.to_string() };
        let on = |b: bool| if b { "on" } else { "off" }.to_string();
        match self {
            MoKnob::Channel => (s.channel + 1).to_string(),
            MoKnob::Port => s.port.to_string(),
            MoKnob::Patch => patch_text(s.patch),
            MoKnob::Bank => bank(s.bank[0]),
            MoKnob::Bank2 => bank(s.bank[1]),
            MoKnob::SendLevels => on(s.send_levels),
            MoKnob::MapColor => on(s.map_color),
        }
    }

    /// The hint text: "MIDI Out channel: 10", "MIDI Out patch: 1 Acoustic Grand Piano".
    pub fn text(self, s: &Settings) -> String {
        let name = match self {
            MoKnob::Channel => "MIDI Out channel",
            MoKnob::Port => "MIDI Out port",
            MoKnob::Patch => "MIDI Out patch",
            MoKnob::Bank => "MIDI Out bank",
            MoKnob::Bank2 => "MIDI Out bank 2",
            k => k.caption(),
        };
        format!("{name}: {}", self.value(s))
    }
}

/// The settings of rack row `row` when it is a MIDI Out channel.
pub fn row_settings(p: &os_model::Project, row: usize) -> Option<Settings> {
    let ch = p.channels.get(row).filter(|c| is_midi_out(c))?;
    Some(ch.plugin_state.as_deref().map_or_else(Settings::default, settings))
}

/// Writes `s` into rack row `row`'s state (a MIDI Out channel); whether anything changed.
pub fn set_row_settings(p: &mut os_model::Project, row: usize, s: &Settings) -> bool {
    let Some(ch) = p.channels.get_mut(row).filter(|c| is_midi_out(c)) else { return false };
    let st = ch.plugin_state.get_or_insert_with(default_state);
    let before = settings(st);
    write(st, s);
    before != *s
}

/// Every MIDI Out channel has its instrument, with the channel's settings and levels; rows that are no longer MIDI
/// Out channels lose theirs. Runs every tick.
pub fn sync_midi_outs(s: &mut crate::session::Session) {
    let rows: Vec<(usize, Live)> = s.project.channels.iter().enumerate().filter(|(_, c)| is_midi_out(c)).map(|(r, c)| (r, Live::of(c))).collect();
    let outs = &mut s.plugins.midi_outs;
    if rows.is_empty() && outs.is_empty() {
        return;
    }
    let before = outs.len();
    outs.retain(|r, _| rows.iter().any(|(x, _)| x == r));
    let mut changed = outs.len() != before;
    for (row, live) in rows {
        match outs.get(&row) {
            Some(i) => {
                if let Ok(mut i) = i.lock() {
                    if i.live != live {
                        i.live = live;
                    }
                }
            }
            None => {
                outs.insert(row, Arc::new(std::sync::Mutex::new(MidiOutInstrument::new(live, queue().clone()))));
                changed = true;
            }
        }
    }
    if changed {
        s.transport.set_instruments(s.plugins.instruments());
    }
}

/// TOOLS' entry and its undo step.
pub const PREP_LABEL: &str = "Set up for MIDI export";
/// The question it asks (the original's words, without "(no undo)": here it is one undo step).
pub const PREP_CONFIRM: &str = "All of the channels are about to be turned into MIDI Out channels in order to allow output to a MIDI file.";
/// A MIDI Out channel's colour (spec: ui/menus.md, event 128 = 0x0073725E).
pub const COLOR: os_model::Rgb = os_model::Rgb::new(0x5e, 0x72, 0x73);

/// Channel `ch` becomes a MIDI Out channel with settings `s`: the plugin and its state, no sample, the MIDI Out colour,
/// the master track.
pub fn make_midi_out(ch: &mut Channel, s: &Settings) {
    ch.kind = ChannelKind::Plugin;
    ch.plugin = Some(FILE_NAME.into());
    ch.plugin_state = Some(state_with(s));
    ch.clear_sample();
    ch.color = Some(COLOR);
    ch.mixer_track = 0;
}

/// The cut groups of the channels Set up for MIDI export converts, from their groups now (spec §4): a group from 1 to
/// the number of channels stays with the first channel that has it, the others get the lowest numbers not taken, in
/// order.
pub fn cut_groups(now: &[u32]) -> Vec<u32> {
    let n = now.len() as u32;
    let mut taken = std::collections::BTreeSet::new();
    let mut out: Vec<Option<u32>> = now.iter().map(|&g| ((1..=n).contains(&g) && taken.insert(g)).then_some(g)).collect();
    let mut free = (1..).filter(|g| !taken.contains(g));
    for g in out.iter_mut().filter(|g| g.is_none()) {
        *g = free.next();
    }
    out.into_iter().map(|g| g.unwrap_or(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;


    /// The default state as the original writes it (spec §2): the fixed part, the first record and the end.
    #[test]
    fn default_state_layout() {
        let b = default_state();
        assert_eq!(b.len(), 967);
        assert_eq!(&b[..32], &[7, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 0]);
        assert!(b[32..RECORDS].iter().all(|&x| x == 0));
        assert_eq!(&b[RECORDS..RECORDS + 28], &[0, 1, 0, 0, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0x7f, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0, 0x10, b'M', b'o']);
        assert_eq!(&b[b.len() - 11..], b"\x06Page 8\0\0\0\0");
        assert_eq!(settings(&b), Settings::default());
    }

    #[test]
    fn settings_round_trip_in_place() {
        let mut b = default_state();
        // A knob value and a record flag the editor doesn't touch.
        b[KNOBS] = 9;
        let s = Settings { channel: 2, port: 5, patch: 126, bank: [1, 2], send_levels: false, map_color: true, unticked: RESET_PAN | RESET_PROGRAM };
        write(&mut b, &s);
        assert_eq!(settings(&b), s);
        assert_eq!(b.len(), 967);
        assert_eq!(b[KNOBS], 9);
        assert_eq!(&b[..32], &[7, 0, 0, 0, 2, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 1, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 5, 0, 0, 0, 0, 1, 126, 0]);
        assert_eq!(u32_at(&b, b.len() - 4), Some(40));
        // A state of another version is not read.
        assert_eq!(settings(&[1, 2, 3]), Settings::default());
    }

    fn live() -> Live {
        Live { settings: Settings::default(), volume: 0.78125, pan: -0.5, bend_range: 12 }
    }

    /// The bundle of the measured export (spec §3): pan 32 for -50 %, volume 100 for 78 %, centre bend, RPN 0 = 12,
    /// program 0.
    #[test]
    fn reset_bundle() {
        let v: Vec<Vec<u8>> = reset_messages(&live()).into_iter().map(|(b, n)| b[..usize::from(n)].to_vec()).collect();
        assert_eq!(v, vec![vec![0xb0, 10, 32], vec![0xb0, 7, 100], vec![0xe0, 0, 0x40], vec![0xb0, 101, 0], vec![0xb0, 100, 0], vec![0xb0, 6, 12], vec![0xc0, 0]]);
        let mut l = live();
        l.settings = Settings { channel: 9, patch: 34, bank: [1, -1], send_levels: false, unticked: RESET_RANGE, ..Settings::default() };
        let v: Vec<Vec<u8>> = reset_messages(&l).into_iter().map(|(b, n)| b[..usize::from(n)].to_vec()).collect();
        assert_eq!(v, vec![vec![0xb9, 0, 1], vec![0xc9, 33]]);
    }

    #[test]
    fn notes_reach_the_queue() {
        let q = Arc::new(ArrayQueue::new(64));
        let mut l = live();
        l.settings.channel = 3;
        l.settings.port = 7;
        let mut i = MidiOutInstrument::new(l, q.clone());
        let info = TransportInfo { playing: true, tempo: 120.0, frame: 0, beats: 0.0, bar_start: 0.0, bar: 0, numerator: 4, denominator: 4, looping: None };
        i.transport(&info);
        let bundle: Vec<Out> = std::iter::from_fn(|| q.pop()).collect();
        assert_eq!(bundle.len(), 7);
        assert!(bundle.iter().all(|o| o.port == 7 && o.bytes[0] & 0x0f == 3));
        i.note_on(0, 60, 100.0 / 128.0);
        i.note_on(0, 64, 1.0);
        i.note_off(0, 60);
        let got: Vec<Out> = std::iter::from_fn(|| q.pop()).collect();
        assert_eq!(got.iter().map(|o| o.data().to_vec()).collect::<Vec<_>>(), vec![vec![0x93, 60, 100], vec![0x93, 64, 127], vec![0x83, 60, 64]]);
        // Stop cuts the note still sounding; the next start sends the bundle again.
        i.reset();
        assert_eq!(q.pop().map(|o| o.data().to_vec()), Some(vec![0x83, 64, 64]));
        assert!(q.pop().is_none());
        i.transport(&info);
        assert_eq!(std::iter::from_fn(|| q.pop()).count(), 7);
        // A render sends nothing.
        set_offline(true);
        i.note_on(0, 60, 0.5);
        set_offline(false);
        assert!(q.pop().is_none());
    }

    /// The groups of the measured conversion (spec §4): 1, 2 and 4 kept, none, 15 and 19 given 3, 5, 6, 7, 8.
    #[test]
    fn cut_groups_as_measured() {
        assert_eq!(cut_groups(&[0, 4, 15, 0, 1, 0, 19, 2]), vec![3, 4, 5, 6, 1, 7, 8, 2]);
        assert_eq!(cut_groups(&[2, 2]), vec![2, 1]);
    }


}
