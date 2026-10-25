// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! What the input devices' messages do (spec: ui/midi-settings.md §3, §4, §8): notes play the selected channel through
//! the velocity curve and links, the foot pedal holds them, the channels set on Settings > MIDI preview, jump and mute,
//! pitch bend bends, linked controllers drive their knobs (`crate::remote`), the rest goes to the hosted instrument,
//! and the external sync device's Start / Continue / Stop / position drive playback. Also what Settings > MIDI shows
//! of the devices (`Page`) and what master sync sends (`App::midi_out_tick`).
// spec: ui/midi-settings.md

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::app::App;
use crate::midi_cfg::{self as cfg, CurveOf, Offset, Switch};
use crate::midi_in::{Ev, InPort, Msg};
use crate::prefs;
use crate::record::Extra;
use crate::step_edit::Src;
use crate::views::midi_page::{CurveEdit, MAct};

/// What Settings > MIDI shows of the devices, kept up to date by `App::midi_tick`.
#[derive(Debug, Default, Clone)]
pub struct Page {
    pub inputs: Vec<InPort>,
    pub outputs: Vec<String>,
    /// The device each list has selected (its options show under it).
    pub input: usize,
    pub output: usize,
    /// External sync's status line ("Status: off").
    pub status: String,
    /// The mapping curve window, while open.
    pub curve: Option<crate::views::midi_page::CurveEdit>,
}

impl Page {
    pub fn selected_input(&self) -> Option<&InPort> {
        self.inputs.get(self.input)
    }

    pub fn selected_output(&self) -> Option<&String> {
        self.outputs.get(self.output)
    }
}

/// A note held from an input device.
#[derive(Debug, Clone, Copy)]
struct Held {
    /// The device's key.
    key: u8,
    src: Src,
    /// The rack row sounding and the key it sounds at (key 60 for an omni preview).
    row: usize,
    play: u16,
    /// Released while the foot pedal holds it: its release velocity.
    up: Option<u8>,
}

/// The input side's state.
#[derive(Debug, Default)]
pub struct Play {
    held: Vec<Held>,
    /// The foot pedal is down.
    pedal: bool,
    /// The row bent by pitch bend, while off centre.
    bend_row: Option<usize>,
    /// When the external clock's tempo was last applied.
    tempo_at: Option<Instant>,
    /// External sync messages waiting out a positive offset.
    pending: VecDeque<(Instant, Msg)>,
    /// Generator muting keys held with Toggle on release on: (key, row).
    muted: Vec<(u8, usize)>,
}

impl Play {
    /// Rows and keys sounding from the devices (the piano roll's keyboard lights them).
    pub fn lit(&self) -> impl Iterator<Item = (usize, u16)> + '_ {
        self.held.iter().filter(|h| h.up.is_none()).map(|h| (h.row, h.play))
    }

    /// Something waits for its time: the shell keeps ticking.
    pub fn busy(&self) -> bool {
        !self.pending.is_empty()
    }
}

/// The 0-based MIDI channel set in value box `n` (1..16; "---" = none).
fn channel_of(n: &prefs::Number) -> Option<u8> {
    match prefs::number(n) {
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
fn curved(of: CurveOf, v: u8) -> u8 {
    let c = cfg::curve(of);
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
pub fn linked(v: u8) -> (u8, Extra) {
    let v = curved(CurveOf::Velocity, v);
    match prefs::index(&prefs::VELOCITY_LINK) {
        0 => (100, Extra::default()),
        2 => (100, Extra { mod_x: Some(mod_of(v)), ..Extra::default() }),
        3 => (100, Extra { mod_y: Some(mod_of(v)), ..Extra::default() }),
        _ => (v, Extra::default()),
    }
}

/// A note-off's release velocity (spec §3).
pub fn release_of(v: u8) -> u8 {
    match prefs::index(&prefs::RELEASE_LINK) {
        0 => 64,
        _ => curved(CurveOf::Release, v.max(1)),
    }
}

/// The external tempo is applied when it moved more than this, at most every `TEMPO_EVERY` (spec §8).
const TEMPO_STEP: f64 = 0.05;
const TEMPO_EVERY: Duration = Duration::from_millis(250);

impl App {
    /// The input devices' messages, the device lists, the external clock and what master sync sends: on every tick.
    pub(crate) fn midi_tick(&mut self) {
        if self.midi.take_changed() {
            self.midi_refresh(false);
        }
        let evs = self.midi.poll();
        self.midi_events(evs);
        self.midi_pending();
        self.ext_sync_tick();
        self.remote_tick();
        // The bent row returns to the centre when another is selected (spec §4).
        if let Some(row) = self.midi_play.bend_row.filter(|&r| r != self.state.selected_channel) {
            self.midi_play.bend_row = None;
            self.transport.midi(row, [0xe0, 0, 64], 200.0);
        }
        self.midi_out_tick();
    }

    /// Lists the devices again (Refresh device list, the watcher, a device switched on or off); `said` names what
    /// changed in the hint bar.
    pub(crate) fn midi_refresh(&mut self, said: bool) {
        let before: Vec<String> = self.midi_page().inputs.iter().map(|i| i.name.clone()).collect();
        let opened = self.midi.refresh();
        self.midi.set_ext_sync(cfg::ext_sync().as_deref());
        let p = &mut self.state.midi;
        p.inputs = self.midi.inputs.clone();
        p.outputs = self.midi.outputs.clone();
        p.input = p.input.min(p.inputs.len().saturating_sub(1));
        p.output = p.output.min(p.outputs.len().saturating_sub(1));
        let failed: Vec<&str> = p.inputs.iter().filter(|i| i.failed).map(|i| i.name.as_str()).collect();
        let added: Vec<&String> = opened.iter().filter(|n| !before.contains(n)).collect();
        let msg = if !failed.is_empty() {
            Some(format!("MIDI input in use by another program: {}", failed.join(", ")))
        } else if !added.is_empty() {
            Some(format!("MIDI input: {}", added.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")))
        } else if said {
            Some(format!("MIDI devices: {} input(s), {} output(s)", p.inputs.len(), p.outputs.len()))
        } else {
            None
        };
        if let Some(m) = msg {
            self.state.message = Some(m);
        }
        self.invalidate();
    }

    fn midi_page(&self) -> &Page {
        &self.state.midi
    }

    /// Messages received, oldest first (spec §3, §4, §8). With Enable MIDI remote control off, all are ignored.
    pub(crate) fn midi_events(&mut self, evs: Vec<Ev>) {
        if evs.is_empty() || !Switch::Remote.on() {
            return;
        }
        let ext = cfg::ext_sync();
        let mut notes = false;
        for ev in evs {
            match ev.msg {
                Msg::Start | Msg::Continue | Msg::Stop | Msg::Spp(_) => {
                    if ext.as_deref() == Some(&*ev.dev) {
                        self.ext_message(ev.msg);
                    }
                }
                Msg::On { ch, key, vel } => {
                    notes = true;
                    self.midi_note_on(ch, key, vel);
                }
                Msg::Off { ch, key, vel } => {
                    notes = true;
                    self.midi_note_off(ch, key, vel);
                }
                Msg::Cc { n: 64, v, .. } if prefs::check(&prefs::FOOT_PEDAL) => {
                    notes = true;
                    self.midi_pedal(v >= 64);
                }
                Msg::Cc { ch, n, v } => {
                    if !self.remote_cc(&ev.dev, ch, n, v) {
                        self.midi_forward(ev.msg);
                    }
                }
                Msg::Bend { .. } => {
                    let row = self.state.selected_channel;
                    if row < self.state.project.channels.len() {
                        self.sync_live();
                        if let Some(b) = ev.msg.bytes() {
                            self.transport.midi(row, b, 200.0);
                        }
                        self.midi_play.bend_row = Some(row);
                    }
                }
                Msg::Pressure { .. } | Msg::Poly { .. } | Msg::Program { .. } => self.midi_forward(ev.msg),
            }
        }
        if notes {
            self.typing_lit_changed();
        }
    }

    /// Tests: messages from a device named "Test".
    #[cfg(test)]
    pub(crate) fn midi_notes(&mut self, msgs: Vec<Msg>) {
        let dev: std::sync::Arc<str> = std::sync::Arc::from("Test");
        self.midi_events(msgs.into_iter().map(|msg| Ev { dev: dev.clone(), msg }).collect());
    }

    /// A message for the selected channel's hosted instrument, on MIDI channel 1 (spec §4).
    fn midi_forward(&mut self, m: Msg) {
        let row = self.state.selected_channel;
        let Some(b) = m.bytes().filter(|_| row < self.state.project.channels.len()) else { return };
        self.sync_live();
        self.transport.midi(row, b, 200.0);
    }

    fn midi_note_on(&mut self, ch: u8, key: u8, vel: u8) {
        self.key_tap();
        let n = self.state.project.channels.len();
        let (vel, mut extra) = linked(vel);
        // The channels set on Settings > MIDI (spec §3): their notes are not played on the selected channel.
        if Some(ch) == channel_of(&prefs::OMNI_CHANNEL) {
            let Some(row) = white_index(key).filter(|&r| r < n) else { return };
            extra.channel = Some(self.state.project.channels[row].index);
            return self.midi_start(Src::Omni(key), key, row, 60, vel, extra);
        }
        if Some(ch) == channel_of(&prefs::MARKER_CHANNEL) {
            if let Some(i) = white_index(key) {
                self.marker_jump(i);
            }
            return;
        }
        if Some(ch) == channel_of(&prefs::MUTING_CHANNEL) {
            if let Some(row) = white_index(key).filter(|&r| r < n) {
                crate::rack::toggle_mute(&mut self.state, row);
                if prefs::check(&prefs::TOGGLE_ON_RELEASE) {
                    self.midi_play.muted.push((key, row));
                }
                self.invalidate();
            }
            return;
        }
        let row = self.state.selected_channel;
        if row >= n {
            return;
        }
        self.midi_start(Src::Midi(key), key, row, u16::from(key), vel, extra);
    }

    /// Starts a note; a key the pedal still holds ends its earlier note first (spec §3).
    fn midi_start(&mut self, src: Src, key: u8, row: usize, play: u16, vel: u8, extra: Extra) {
        while let Some(i) = self.midi_play.held.iter().position(|h| h.src == src) {
            let h = self.midi_play.held.remove(i);
            self.midi_end(h, h.up.unwrap_or(64));
        }
        self.sync_live();
        self.transport.note_on(row, play, vel);
        self.midi_play.held.push(Held { key, src, row, play, up: None });
        self.step_edit_down_with(src, play, vel, extra);
    }

    fn midi_end(&mut self, h: Held, release: u8) {
        self.transport.note_off(h.row, h.play);
        self.step_edit_up_with(h.src, Some(release));
    }

    fn midi_note_off(&mut self, ch: u8, key: u8, vel: u8) {
        if Some(ch) == channel_of(&prefs::MUTING_CHANNEL) {
            while let Some(i) = self.midi_play.muted.iter().position(|m| m.0 == key) {
                let (_, row) = self.midi_play.muted.remove(i);
                crate::rack::toggle_mute(&mut self.state, row);
                self.invalidate();
            }
            return;
        }
        let src = if Some(ch) == channel_of(&prefs::OMNI_CHANNEL) { Src::Omni(key) } else { Src::Midi(key) };
        let release = release_of(vel);
        let pedal = self.midi_play.pedal && prefs::check(&prefs::FOOT_PEDAL);
        let Some(i) = self.midi_play.held.iter().position(|h| h.src == src && h.key == key && h.up.is_none()) else { return };
        if pedal {
            self.midi_play.held[i].up = Some(release);
        } else {
            let h = self.midi_play.held.remove(i);
            self.midi_end(h, release);
        }
    }

    /// The foot pedal (controller 64 with Foot pedal controls note off): up ends the notes it held (spec §3).
    fn midi_pedal(&mut self, down: bool) {
        self.midi_play.pedal = down;
        if down {
            return;
        }
        let (ended, kept): (Vec<Held>, Vec<Held>) = std::mem::take(&mut self.midi_play.held).into_iter().partition(|h| h.up.is_some());
        self.midi_play.held = kept;
        for h in ended {
            self.midi_end(h, h.up.unwrap_or(64));
        }
    }

    /// Every device note ends (the device list changed, remote control switched off).
    pub(crate) fn midi_release_all(&mut self) {
        self.midi_play.pedal = false;
        for h in std::mem::take(&mut self.midi_play.held) {
            self.midi_end(h, 64);
        }
        self.typing_lit_changed();
    }

    /// Song marker jump (spec §3): the `i`-th of the playlist's time markers, in SONG mode.
    fn marker_jump(&mut self, i: usize) {
        if !self.state.song_mode {
            self.state.message = Some("Song marker jump: in SONG mode".into());
            return self.invalidate();
        }
        let mut at: Vec<u32> = self
            .state
            .project
            .arrangement()
            .map(|a| a.markers.iter().filter(|m| m.kind != os_model::TimeMarker::SIGNATURE).map(|m| m.position).collect())
            .unwrap_or_default();
        at.sort_unstable();
        if let Some(&t) = at.get(i) {
            self.set_song_pos(t, true);
        }
    }

    /// The external sync device's transport message (spec §8): with a positive offset it waits that long.
    fn ext_message(&mut self, m: Msg) {
        let off = cfg::offset(Offset::External);
        if off > 0 {
            self.midi_play.pending.push_back((Instant::now() + Duration::from_millis(off as u64), m));
        } else {
            self.ext_apply(m, (-off) as u32);
        }
    }

    fn midi_pending(&mut self) {
        let now = Instant::now();
        while self.midi_play.pending.front().is_some_and(|p| p.0 <= now) {
            let Some((_, m)) = self.midi_play.pending.pop_front() else { break };
            self.ext_apply(m, 0);
        }
    }

    /// Start plays from the beginning, Continue resumes, Stop pauses, Song Position Pointer moves the position;
    /// `ahead_ms` starts that far into the song (a negative offset).
    fn ext_apply(&mut self, m: Msg, ahead_ms: u32) {
        use crate::transport::State as T;
        let ppq = u32::from(self.state.project.ppq.max(1));
        let ahead = (f64::from(ahead_ms) / 1000.0 * self.state.tempo / 60.0 * f64::from(ppq)).round() as u32;
        match m {
            Msg::Start => {
                self.transport.stop();
                self.ext_position(ahead);
                self.start_playback();
            }
            Msg::Continue => match self.transport.state() {
                T::Paused => self.transport.resume(),
                T::Stopped => {
                    if ahead > 0 {
                        let at = if self.state.song_mode { self.state.playlist.pos } else { self.state.piano.marker };
                        self.ext_position(at + ahead);
                    }
                    self.start_playback();
                }
                T::Playing => {}
            },
            Msg::Stop => {
                if self.transport.state() == T::Playing {
                    self.transport.pause();
                }
            }
            Msg::Spp(sixteenths) => {
                let t = u32::from(sixteenths) * ppq / 4;
                let running = self.transport.state() == T::Playing;
                // The position counts from a stop: a paused playback would resume where it paused.
                self.transport.stop();
                self.ext_position(t);
                if running {
                    self.start_playback();
                }
            }
            _ => {}
        }
        self.invalidate();
    }

    /// Puts the song position (SONG) or the play-start marker (PAT) at `t` without restarting.
    fn ext_position(&mut self, t: u32) {
        let s = &mut self.state;
        if s.song_mode {
            s.playlist.pos = t;
            crate::init::apply(s);
        } else {
            s.piano.marker = t;
        }
    }

    /// The external clock's tempo and the status line (spec §8).
    fn ext_sync_tick(&mut self) {
        let st = self.midi.ext_status();
        let text = st.text();
        if self.state.midi.status != text {
            self.state.midi.status = text;
            if self.state.settings.open && self.state.settings.tab == crate::views::settings::MIDI {
                self.invalidate();
            }
        }
        let Some(bpm) = st.tempo else { return };
        let bpm = (bpm * 100.0).round() / 100.0;
        let due = self.midi_play.tempo_at.is_none_or(|t| t.elapsed() >= TEMPO_EVERY);
        if due && (bpm - self.state.tempo).abs() > TEMPO_STEP {
            self.midi_play.tempo_at = Some(Instant::now());
            self.set_tempo(bpm);
        }
    }

    /// Master sync (spec §7): the outputs with Send master sync while Enable MIDI output and master sync are on.
    fn midi_out_tick(&mut self) {
        let send = Switch::Output.on() && Switch::MasterSync.on();
        let sync = if send {
            self.midi
                .outputs
                .iter()
                .filter_map(|name| {
                    let c = cfg::output(name);
                    c.sync.then(|| (name.clone(), c.notes_off))
                })
                .collect()
        } else {
            Vec::new()
        };
        let c = crate::midi_out::Config {
            sync,
            kind: crate::midi_out::SyncKind::of(prefs::index(&prefs::SYNC_TYPE)),
            offset_ms: cfg::offset(Offset::MasterSync),
            ppq: self.state.project.ppq.max(1),
            tempo: self.state.tempo,
        };
        self.midi_out.configure(c);
    }

    /// After an export: all notes off to the outputs that want it (spec §2).
    pub(crate) fn midi_after_export(&mut self) {
        if Switch::Output.on() {
            self.midi_out.notes_off();
        }
    }
}

/// Settings > MIDI's controls (`views::midi_page`) and the OPTIONS menu's MIDI switches.
impl App {
    /// Settings > MIDI opened: the device lists are read again (spec §1).
    pub(crate) fn midi_page_opened(&mut self) {
        self.midi_refresh(false);
    }

    pub(crate) fn midi_page_act(&mut self, m: MAct) {
        let p = &self.state.midi;
        let out = p.selected_output().cloned();
        let inp = p.selected_input().map(|i| i.name.clone());
        match m {
            MAct::Output(i) => self.state.midi.output = i,
            MAct::Input(i) => self.state.midi.input = i,
            MAct::Power(i) => {
                self.state.midi.input = i;
                let Some(name) = self.state.midi.inputs.get(i).map(|d| d.name.clone()) else { return };
                self.midi_enable(&name);
            }
            MAct::Enable => {
                if let Some(name) = inp {
                    self.midi_enable(&name);
                }
            }
            MAct::OutSync | MAct::OutNotesOff => {
                let Some(name) = out else { return };
                let mut c = cfg::output(&name);
                let (what, on) = if m == MAct::OutSync {
                    c.sync = !c.sync;
                    ("Send master sync", c.sync)
                } else {
                    c.notes_off = !c.notes_off;
                    ("Send all notes off", c.notes_off)
                };
                cfg::set_output(&name, c);
                let mut msg = format!("{name}: {what} {}", if on { "on" } else { "off" });
                if m == MAct::OutSync && on && !(Switch::MasterSync.on() && Switch::Output.on()) {
                    msg.push_str(" (OPTIONS > Enable MIDI master sync is off)");
                }
                self.state.message = Some(msg);
            }
            MAct::Curve(of) => self.state.midi.curve = Some(CurveEdit { of, curve: cfg::curve(of) }),
            MAct::CurveClose => self.state.midi.curve = None,
            MAct::CurveReset => {
                if let Some(e) = self.state.midi.curve.as_mut() {
                    e.curve = cfg::Curve::LINE;
                }
            }
            MAct::CurveAccept => {
                if let Some(e) = self.state.midi.curve.take() {
                    cfg::set_curve(e.of, e.curve);
                    self.state.message = Some(format!("{}: kept", e.of.title()));
                }
            }
            MAct::CurveHandle(i) => self.drag = Some(crate::app::Drag::MidiCurve(i)),
            MAct::ExtSync(x, y) => return self.ext_sync_menu(x, y),
            MAct::CurveBody | MAct::OutPort | MAct::InPort | MAct::Offset(_) => {
                if m.is_value() {
                    self.drag = Some(crate::app::Drag::MidiBox { part: m, y0: self.mouse.1, steps: 0 });
                }
            }
        }
        self.invalidate();
    }

    /// A curve handle dragged to `y` (`Drag::MidiCurve`).
    pub(crate) fn midi_curve_drag(&mut self, which: u8, y: f32) {
        let win = crate::views::settings::rect(&self.state);
        let (_, graph, ..) = crate::views::midi_page::curve_rects(win);
        if let Some(e) = self.state.midi.curve.as_mut() {
            e.curve = crate::views::midi_page::drag_handle(graph, e.curve, which, y);
            self.invalidate();
        }
    }

    /// Switches input `name` on or off; it opens or closes at once (spec §2).
    fn midi_enable(&mut self, name: &str) {
        let mut c = cfg::input(name);
        c.enabled = !c.enabled;
        cfg::set_input(name, c);
        if !c.enabled {
            self.midi_release_all();
        }
        self.midi_refresh(false);
        let failed = self.state.midi.inputs.iter().any(|i| i.name == name && i.failed);
        self.state.message = Some(match (c.enabled, failed) {
            (true, true) => format!("{name}: in use by another program"),
            (true, false) => format!("{name}: enabled"),
            (false, _) => format!("{name}: off"),
        });
    }

    /// The wheel (or a drag, or right-click with `reset`) on a value box of the page: `steps` up or down.
    pub(crate) fn midi_page_wheel(&mut self, m: MAct, steps: i32) {
        self.midi_page_step(m, Some(steps));
    }

    pub(crate) fn midi_page_reset(&mut self, m: MAct) {
        self.midi_page_step(m, None);
    }

    fn midi_page_step(&mut self, m: MAct, steps: Option<i32>) {
        // "---" sits below 0.
        let port = |p: Option<u8>| -> Option<u8> {
            let s = steps?;
            let n = p.map_or(-1, i32::from) + s;
            (n >= 0).then(|| n.min(255) as u8)
        };
        let p = &self.state.midi;
        let msg = match m {
            MAct::OutPort => {
                let Some(name) = p.selected_output().cloned() else { return };
                let mut c = cfg::output(&name);
                c.port = port(c.port);
                cfg::set_output(&name, c);
                format!("{name}: port {}", c.port.map_or_else(|| "---".into(), |p| p.to_string()))
            }
            MAct::InPort => {
                let Some(name) = p.selected_input().map(|i| i.name.clone()) else { return };
                let mut c = cfg::input(&name);
                c.port = port(c.port);
                cfg::set_input(&name, c);
                format!("{name}: port {}", c.port.map_or_else(|| "---".into(), |p| p.to_string()))
            }
            MAct::Offset(o) => {
                let v = steps.map_or(0, |s| cfg::offset(o) + s);
                cfg::set_offset(o, v);
                let what = if o == Offset::MasterSync { "Master sync offset" } else { "External sync offset" };
                format!("{what}: {} ms", cfg::offset(o))
            }
            _ => return,
        };
        self.state.message = Some(msg);
        self.invalidate();
    }

    /// External clock sync's menu: Off and the input devices (spec §8).
    fn ext_sync_menu(&mut self, x: f32, y: f32) {
        use crate::playlist::{Cmd, Mark, PItem, Popup};
        let now = cfg::ext_sync();
        let mut items = vec![PItem::new("Off", Cmd::MidiExt(None)).mark(Mark::Radio(now.is_none()))];
        for d in &self.state.midi.inputs {
            let on = now.as_deref() == Some(d.name.as_str());
            let it = PItem::new(d.name.clone(), Cmd::MidiExt(Some(d.name.clone()))).mark(Mark::Radio(on));
            items.push(if cfg::input(&d.name).enabled { it } else { it.off() });
        }
        self.state.playlist.popup = Some(Popup { x, y: y + 1.0, items, sub: None, scroll: [0; 2] });
        self.invalidate();
    }

    /// External clock sync chosen (`Cmd::MidiExt`).
    pub(crate) fn set_ext_sync(&mut self, device: Option<String>) -> Option<String> {
        cfg::set_ext_sync(device.as_deref());
        self.midi.set_ext_sync(device.as_deref());
        self.invalidate();
        Some(format!("External clock sync: {}", device.as_deref().unwrap_or("Off")))
    }

    /// An OPTIONS menu MIDI switch (spec §5).
    pub(crate) fn toggle_midi_switch(&mut self, sw: Switch) {
        let on = !sw.on();
        sw.set(on);
        if sw == Switch::Remote && !on {
            self.midi_release_all();
        }
        self.state.message = Some(format!("{}: {}", sw.label(), if on { "on" } else { "off" }));
        self.invalidate();
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
        assert_eq!([1, 32, 64, 100, 127].map(mod_of), [2, 64, 128, 199, 253]);
        assert_eq!(linked(90), (90, Extra::default()), "Velocity is the default");
        prefs::set_choice(&prefs::VELOCITY_LINK, 2);
        assert_eq!(linked(64), (100, Extra { mod_x: Some(128), ..Extra::default() }));
        prefs::set_choice(&prefs::VELOCITY_LINK, 0);
        assert_eq!(linked(20).0, 100);
        prefs::set_choice(&prefs::RELEASE_LINK, 0);
        assert_eq!(release_of(10), 64);
    }

    fn on(key: u8) -> Msg {
        Msg::On { ch: 0, key, vel: 100 }
    }

    fn off(key: u8) -> Msg {
        Msg::Off { ch: 0, key, vel: 64 }
    }

    fn cc(n: u8, v: u8) -> Msg {
        Msg::Cc { ch: 0, n, v }
    }

    fn app() -> App {
        let mut app = App::new(None);
        app.state.selected_channel = 0;
        assert!(app.state.project.channels.len() >= 2, "the new project has channels");
        app
    }

    /// spec §3 Foot pedal controls note off.
    #[test]
    fn the_foot_pedal_holds_released_keys() {
        let mut app = app();
        app.midi_notes(vec![on(60), off(60)]);
        assert!(app.midi_play.held.is_empty());
        app.midi_notes(vec![cc(64, 127), on(60), off(60)]);
        assert_eq!(app.midi_play.held.len(), 1, "held by the pedal");
        assert_eq!(app.midi_play.lit().count(), 0, "not lit: the key is up");
        app.midi_notes(vec![on(60)]);
        assert_eq!(app.midi_play.held.len(), 1, "struck again: the earlier note ended");
        app.midi_notes(vec![off(60), cc(64, 0)]);
        assert!(app.midi_play.held.is_empty(), "the pedal's release ends it");
        // Off: the pedal goes to the instrument and the keys end at once.
        prefs::set_check(&prefs::FOOT_PEDAL, false);
        app.midi_notes(vec![cc(64, 127), on(62), off(62)]);
        assert!(app.midi_play.held.is_empty());
    }

    /// spec §3: the omni preview and generator muting channels.
    #[test]
    fn special_channels() {
        let mut app = app();
        prefs::set_number(&prefs::OMNI_CHANNEL, 3);
        app.midi_notes(vec![Msg::On { ch: 2, key: 50, vel: 90 }]);
        let h = app.midi_play.held[0];
        assert_eq!((h.row, h.play), (1, 60), "white key 50 previews channel 2 at C5");
        app.midi_notes(vec![Msg::Off { ch: 2, key: 50, vel: 64 }, Msg::On { ch: 2, key: 49, vel: 90 }]);
        assert!(app.midi_play.held.is_empty(), "black keys do nothing");
        prefs::set_number(&prefs::MUTING_CHANNEL, 4);
        let was = app.state.project.channels[0].enabled;
        app.midi_notes(vec![Msg::On { ch: 3, key: 48, vel: 90 }, Msg::Off { ch: 3, key: 48, vel: 64 }]);
        assert_eq!(app.state.project.channels[0].enabled, !was, "toggled");
        prefs::set_check(&prefs::TOGGLE_ON_RELEASE, true);
        app.midi_notes(vec![Msg::On { ch: 3, key: 48, vel: 90 }]);
        assert_eq!(app.state.project.channels[0].enabled, was);
        app.midi_notes(vec![Msg::Off { ch: 3, key: 48, vel: 64 }]);
        assert_eq!(app.state.project.channels[0].enabled, !was, "momentary");
        assert!(app.midi_play.held.is_empty(), "nothing played on the selected channel");
    }

    /// spec §5: remote control off ignores every message.
    #[test]
    fn remote_control_off() {
        let mut app = app();
        app.toggle_midi_switch(Switch::Remote);
        app.midi_notes(vec![on(60)]);
        assert!(app.midi_play.held.is_empty());
        app.toggle_midi_switch(Switch::Remote);
        app.midi_notes(vec![on(60)]);
        assert_eq!(app.midi_play.held.len(), 1);
    }

    /// ui/remote-control.md: the box's auto accept, a link driving its knob, pickup, Multilink.
    #[test]
    fn controller_links() {
        use crate::knobs::KnobId;
        use crate::remote::RCmd;
        use crate::typein::Target;
        let mut app = app();
        let vol = Target::Knob(KnobId::ChanVol(0));
        app.remote_cmd(RCmd::Link(vol));
        assert!(app.state.link_box.is_some());
        app.midi_notes(vec![cc(21, 5)]);
        assert!(app.state.link_box.is_none(), "auto accepted");
        assert_eq!(crate::remote::link_of(vol).map(|l| (l.ch, l.cc)), Some((0, 21)));
        app.midi_notes(vec![cc(21, 127)]);
        assert!((app.state.knob(KnobId::ChanVol(0)) - 1.0).abs() < 1e-4);
        app.midi_notes(vec![cc(21, 0)]);
        assert!(app.state.knob(KnobId::ChanVol(0)).abs() < 1e-4);
        // Pickup: after a change elsewhere the controller waits until it crosses the knob.
        prefs::set_check(&prefs::PICKUP, true);
        app.state.set_knob(KnobId::ChanVol(0), 0.5);
        app.midi_notes(vec![cc(21, 10), cc(21, 30)]);
        assert!((app.state.knob(KnobId::ChanVol(0)) - 0.5).abs() < 1e-4, "not picked up yet");
        app.midi_notes(vec![cc(21, 80)]);
        assert!((app.state.knob(KnobId::ChanVol(0)) - 80.0 / 127.0).abs() < 1e-4, "crossed: taken over");
        // Multilink: knobs queued in order, then the next unlinked controllers.
        let pan = Target::Knob(KnobId::ChanPan(1));
        app.toggle_multilink();
        app.multilink_knob(pan);
        app.multilink_knob(Target::Knob(KnobId::MasterVol));
        app.midi_notes(vec![cc(30, 64), cc(31, 64)]);
        assert_eq!(crate::remote::link_of(pan).map(|l| l.cc), Some(30));
        assert_eq!(crate::remote::link_of(Target::Knob(KnobId::MasterVol)).map(|l| l.cc), Some(31));
        assert!(!app.state.multilink.on, "the queue is empty: off");
        // An unlinked controller goes to the instrument; Remove link drops the link.
        assert!(!app.remote_cc("Test", 0, 99, 1));
        app.remote_cmd(RCmd::Unlink(pan));
        assert!(crate::remote::link_of(pan).is_none());
    }
    /// Master sync to a real loopback port (loopMIDI's "LemonKeys" here): Start, then clocks at 24 a quarter, then Stop
    /// (needs the port and an audio device): `cargo test -p os-app master_clock_loopback -- --ignored`.
    #[test]
    #[ignore]
    fn master_clock_loopback() {
        use std::sync::{Arc, Mutex};
        let mut app = App::new(None);
        app.midi = crate::midi_in::MidiIn::open(|| {});
        app.midi_out = crate::midi_out::MidiOut::start(app.transport.watch());
        let name = app.midi.outputs.iter().find(|n| n.contains("LemonKeys")).cloned().expect("a LemonKeys output");
        let input = midir::MidiInput::new("Lemon Studio clock test").unwrap();
        let port = input.ports().into_iter().find(|p| input.port_name(p).is_ok_and(|n| n.contains("LemonKeys")));
        let got: Arc<Mutex<Vec<u8>>> = Arc::default();
        let sink = got.clone();
        let mut input = midir::MidiInput::new("Lemon Studio clock test").unwrap();
        input.ignore(midir::Ignore::None);
        let _conn = input
            .connect(&port.expect("a LemonKeys input"), "clock", move |_, b, _| sink.lock().unwrap().push(b[0]), ())
            .unwrap();
        cfg::set_output(&name, cfg::OutCfg { sync: true, ..cfg::OutCfg::default() });
        Switch::Output.set(true);
        Switch::MasterSync.set(true);
        app.midi_out_tick();
        std::thread::sleep(Duration::from_millis(300));
        app.act(crate::app::Action::Play);
        std::thread::sleep(Duration::from_millis(1000));
        app.act(crate::app::Action::Stop);
        std::thread::sleep(Duration::from_millis(300));
        let got = got.lock().unwrap().clone();
        let clocks = got.iter().filter(|b| **b == 0xf8).count();
        assert!(got.contains(&0xfa) && got.contains(&0xfc), "start and stop: {got:x?}");
        // 1 s at the tempo: 24 clocks a beat.
        let want = app.state.tempo / 60.0 * 24.0;
        assert!((clocks as f64 - want).abs() < want * 0.25, "{clocks} clocks, want about {want}");
    }
}
