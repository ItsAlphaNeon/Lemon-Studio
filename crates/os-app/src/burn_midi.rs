// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The channel menu's Burn MIDI to ▸ Current pattern / New pattern (spec: ui/channel-rack.md §18, Lemon Studio's
//! design): the notes a channel generates from its notes in the current pattern, written as notes. Two sources:
//!
//! - the channel's own echo delay and arpeggiator (the Misc tab), worked out from the pattern's notes as the original
//!   plays them (spec: audio/channel-misc.md). Lemon Studio's engine does not play them yet, so a burn is how they are
//!   heard; the burn switches them off on the channel, so the notes are not generated twice;
//! - the notes a hosted CLAP or VST 3 instrument puts out (an arpeggiator or sequencer plugin), captured while the
//!   pattern's notes (after the channel's own arpeggiator) are played into it offline.
//!
//! Current pattern replaces the channel's notes in the current pattern; New pattern puts them into the next empty
//! pattern, which becomes the current one. One undo step either way.
// spec: ui/channel-rack.md §18 "Burn MIDI to", audio/channel-misc.md

use std::sync::Arc;

use os_model::{Channel, ChannelKind, Note};

use crate::app::{App, State};

/// The undo step.
pub const STEP: &str = "Burn MIDI";

/// The rate the plugin pass runs at (frames a second); the notes come back in ticks, so it does not show.
const PASS_RATE: u32 = 48000;
/// The longest block handed to a plugin in the pass.
const PASS_BLOCK: usize = 1024;

/// The time law: a stored gate, time shift or arpeggio time `v` (1024 = one step) in ticks (spec:
/// audio/channel-misc.md "The time law").
pub fn law_ticks(v: i32, ppq: u16) -> u32 {
    let x = (f64::from(v) * (1.0 / 1024.0)) as f32;
    let e = ((f64::from(x) * 27f64.ln()).exp() - 1.0) as f32;
    let r = (f64::from(e * 48.0f32) * (1.0 / 26.0)).round_ties_even() as i64;
    (r * i64::from(ppq / 4) / 48).max(0) as u32
}

/// A stored time (450..=1447) of a knob kept as 0..=1 (the gate time, the arpeggio time).
fn time_stored(knob: f32) -> i32 {
    450 + (knob.clamp(0.0, 1.0) * 997.0).round() as i32
}

/// The value a stored time of 1447 or more means: the gate off, the arpeggio held.
const TIME_OFF: i32 = 1447;

/// The inverse of the level curve: the velocity (100 = 1) whose volume is `gain`.
fn uncurve(gain: f32) -> f32 {
    ((f64::from(gain.max(0.0)) * 10.0 + 1.0).ln() / 11f64.ln()) as f32
}

/// A note in play (spec: audio/channel-misc.md "What it is"), as far as a burn needs it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Play {
    start: u32,
    /// The (gated) end; for a plain step one step after its start.
    end: u32,
    /// A plain step whose end is its own (written back with no length).
    step: bool,
    /// Cents from key 0, the echoes' pitch steps included; the fine pitch apart.
    pitch: f32,
    /// The note's own levels: volume (after the level curve), pan, mod X and mod Y.
    vol: f32,
    pan: f32,
    mx: f32,
    my: f32,
    /// Where an arpeggio led by it starts in its list (-1: before entry 0).
    place: i32,
    /// The note it came from (its fine pitch, colour, flags and release).
    src: Note,
}

impl Play {
    fn of(n: &Note, step_ticks: u32) -> Play {
        let len = if n.length > 0 { n.length } else { step_ticks };
        let l = os_engine::NoteLevels::of_bytes(n.pan, n.mod_x, n.mod_y);
        Play {
            start: n.position,
            end: n.position + len,
            step: n.length == 0,
            pitch: f32::from(n.key) * 100.0,
            vol: os_dsp::level::curve(f32::from(n.velocity) * 0.01),
            pan: l.pan,
            mx: l.mod_x,
            my: l.mod_y,
            place: -1,
            src: *n,
        }
    }

    /// The note written for it; the velocity, pan and mods of a note whose levels did not change stay its bytes.
    fn note(&self) -> Note {
        let n = &self.src;
        let key = (self.pitch / 100.0).round();
        let rest = self.pitch - key * 100.0;
        let fine = (f32::from(n.fine_pitch) + (rest / 10.0).round()).clamp(0.0, 240.0) as u8;
        let same = |a: f32, b: f32| (a - b).abs() < 1e-6;
        let l = os_engine::NoteLevels::of_bytes(n.pan, n.mod_x, n.mod_y);
        let velocity = if same(self.vol, os_dsp::level::curve(f32::from(n.velocity) * 0.01)) {
            n.velocity
        } else {
            (uncurve(self.vol) * 100.0).round().clamp(0.0, 128.0) as u8
        };
        let byte = |v: f32, old: f32, b: u8, mid: f32, max: f32| if same(v, old) { b } else { (mid + v * mid).round().clamp(0.0, max) as u8 };
        Note {
            position: self.start,
            length: if self.step { 0 } else { (self.end - self.start).max(1) },
            key: key.clamp(0.0, 131.0) as u16,
            fine_pitch: fine,
            velocity,
            pan: byte(self.pan, l.pan, n.pan, 64.0, 128.0),
            mod_x: byte(self.mx, l.mod_x, n.mod_x, 128.0, 255.0),
            mod_y: byte(self.my, l.mod_y, n.mod_y, 128.0, 255.0),
            repeat: 0,
            ..*n
        }
    }
}

/// Whether channel `ch`'s echo delay or arpeggiator generates notes (what Burn MIDI to works out itself).
pub fn generates(ch: &Channel) -> bool {
    let m = &ch.misc;
    let echo = m.echo.feed > 0.0 && !m.mono && (m.echo.echoes > 0 || m.echo.fat);
    m.arp.direction != 0 || echo
}

/// The notes channel `ch` plays from `notes` (its notes in a pattern, at `ppq`) through its gate time, echo delay and
/// arpeggiator; `None` when neither the echo delay nor the arpeggiator is on. Muted notes and slide notes stay as
/// they are; notes outside the key range, which make no sound, are left out.
pub fn generate(ch: &Channel, notes: &[Note], ppq: u16) -> Option<Vec<Note>> {
    if !generates(ch) {
        return None;
    }
    let m = &ch.misc;
    let step_ticks = u32::from(ppq / 4).max(1);
    let mut kept: Vec<Note> = notes.iter().filter(|n| n.muted() || n.slide()).copied().collect();
    // The notes as the engine sends them: a note repeat's retriggers one by one (spec: audio/pattern-controls.md).
    let mut plays = Vec::new();
    for n in notes.iter().filter(|n| !n.muted() && !n.slide()) {
        let key = i32::from(n.key);
        if key < m.key_range[0] || key > m.key_range[1] {
            continue;
        }
        match n.repeat_ticks(ppq) {
            Some(iv) => {
                let span = if n.length > 0 { n.length } else { step_ticks };
                let mut off = 0;
                while off < span {
                    let len = if n.length > 0 { n.length - off } else { 0 };
                    plays.push(Play::of(&Note { position: n.position + off, length: len, repeat: 0, ..*n }, step_ticks));
                    off += iv.max(1);
                }
            }
            None => plays.push(Play::of(n, step_ticks)),
        }
    }
    plays.sort_by_key(|p| p.start);
    let gate = time_stored(m.gate);
    let gate = (gate < TIME_OFF).then(|| law_ticks(gate, ppq).max(1));
    if let Some(g) = gate {
        for p in &mut plays {
            if p.start + g < p.end {
                p.end = p.start + g;
                p.step = false;
            }
        }
    }
    let plays = echoes(ch, plays, ppq);
    let mut out = if m.arp.direction != 0 { Arps::new(ch, ppq, gate).run(plays) } else { plays };
    out.sort_by_key(|p| p.start);
    let mut notes: Vec<Note> = out.iter().map(Play::note).collect();
    notes.append(&mut kept);
    notes.sort_by_key(|n| n.position);
    Some(notes)
}

/// The echo delay (spec: audio/channel-misc.md "Echo delay"): each note followed by its echoes. A mono channel has
/// none.
fn echoes(ch: &Channel, plays: Vec<Play>, ppq: u16) -> Vec<Play> {
    let e = &ch.misc.echo;
    let feed = (e.feed.clamp(0.0, 1.0) * 25600.0).round();
    if feed <= 0.0 || ch.misc.mono {
        return plays;
    }
    let s = feed * (1.0f32 / 12800.0);
    let n = e.echoes;
    let step0 = (e.pan.clamp(-1.0, 1.0) * 12800.0).round() * (1.0f32 / 6400.0);
    let pitch = (e.pitch.clamp(-1.0, 1.0) * 1200.0).round();
    let time = (e.time.clamp(0.0, 1.0) * 384.0).round() as u64;
    let mods = [e.mod_x, e.mod_y].map(|m| (128.0 + m.clamp(-1.0, 1.0) * 128.0).round());
    let chan_pan = (ch.pan as f32 + ch.misc.levels[0]).clamp(-2.0, 2.0);
    let delay = |k: u32| (u64::from(k) * time * u64::from(ppq / 4) / 48) as u32;
    let moved = |v: f32, m: f32| {
        if m < 128.0 {
            (v + 1.0) * m * (1.0f32 / 128.0) - 1.0
        } else if m > 128.0 {
            1.0 - (1.0 - v) * (256.0 - m) * (1.0f32 / 128.0)
        } else {
            v
        }
    };
    let mut out = Vec::with_capacity(plays.len() * (n as usize + 1));
    for p in plays {
        // One row of echoes from the note's levels `from`, stepping by `sign`; `vol` the volume of each (fat mode),
        // else the volume falls by the feed.
        let row = |from: Play, sign: f32, fat: Option<f32>, first_place: i32, out: &mut Vec<Play>| {
            let (mut e2, mut step) = (from, step0 * sign);
            for k in 1..=n {
                let x = (e2.pan + step + chan_pan).clamp(-1.0, 1.0);
                if e.ping_pong && x.abs() == 1.0 {
                    step = -step;
                }
                e2.pan = x - chan_pan;
                e2.vol = match fat {
                    Some(v) => v,
                    None => e2.vol * s,
                };
                e2.pitch += pitch * sign;
                e2.mx = moved(e2.mx, mods[0]);
                e2.my = moved(e2.my, mods[1]);
                let d = delay(k);
                let place = if fat.is_some() { first_place + 2 * k as i32 } else { from.place };
                out.push(Play { start: p.start + d, end: p.end + d, place, ..e2 });
            }
        };
        if e.fat {
            let s2 = (f64::from(s) / ((f64::from(n) * 1.5 + 1.0) * 2.0)) as f32;
            let vol = p.vol;
            let own = Play { vol: (f64::from(vol) * (1.5 - f64::from(s2 * n as f32 * 2.0))) as f32, ..p };
            out.push(own);
            let mut rows = Vec::new();
            row(p, 1.0, Some(vol * s2), -1, &mut rows);
            let mut second = Vec::new();
            row(p, -1.0, Some(vol * s2), -2, &mut second);
            // Of the two echoes of one tick the second row's is played first.
            for (a, b) in rows.into_iter().zip(second) {
                out.push(b);
                out.push(a);
            }
        } else {
            out.push(p);
            row(p, 1.0, None, -1, &mut out);
        }
    }
    out.sort_by_key(|p| p.start);
    out
}

/// One entry of an arpeggio's note list: its pitch (cents: an offset from the leading note for a chord, the held
/// key itself for chord -1) and, for chord -1, the held note's levels.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Entry {
    pitch: f32,
    held: Option<Play>,
}

/// A leading voice (spec: audio/channel-misc.md "Arpeggiator").
#[derive(Debug, Clone)]
struct Lead {
    play: Play,
    /// Chord -1: the colour whose held list it plays.
    colour: Option<u8>,
    list: Vec<Entry>,
    index: i32,
    release: u32,
    next: u32,
    /// The list was rebuilt since this voice last chose.
    marked: bool,
    chosen: bool,
    /// The pitch it played last (cents, as the list holds them), and the one that would have come next.
    last: f32,
    upcoming: Option<f32>,
    /// The note it has sounding.
    sounding: Option<Play>,
    /// Hold: it chose once.
    held_once: bool,
}

/// The arpeggiator of one channel over a pattern's notes in play.
struct Arps {
    direction: u8,
    range: u32,
    repeat: u32,
    chord: i32,
    /// Arpeggio time in ticks, the note length (gate) in ticks; hold; slide.
    t: u32,
    g: u32,
    hold: bool,
    slide: bool,
    /// The channel's gate time, which the arpeggio's notes pass too.
    gate: Option<u32>,
    rng: os_engine::engine::Mt19937,
}

impl Arps {
    fn new(ch: &Channel, ppq: u16, gate: Option<u32>) -> Arps {
        let a = &ch.misc.arp;
        let time = time_stored(a.time);
        let t = law_ticks(time.min(TIME_OFF - 1), ppq);
        let gate_stored = (a.gate.clamp(0.0, 1.0) * 48.0).round() as u32;
        Arps {
            direction: a.direction,
            range: a.range,
            repeat: a.repeat.max(1),
            chord: a.chord,
            t,
            g: (t * gate_stored / 48).max(1),
            hold: time >= TIME_OFF,
            slide: a.slide,
            gate,
            rng: os_engine::engine::Mt19937::for_playback(),
        }
    }

    /// The chord's notes: offsets in cents from the note (chord 0 or more), or the held notes (chord -1).
    fn chord_notes(&self, held: Option<&[Play]>) -> Vec<Entry> {
        if let Some(h) = held.filter(|h| !h.is_empty()) {
            return h.iter().map(|p| Entry { pitch: p.pitch, held: Some(*p) }).collect();
        }
        let k = if held.is_some() { 1 } else { self.chord };
        let mut v = vec![Entry { pitch: 0.0, held: None }];
        if let Some(e) = usize::try_from(k).ok().filter(|&k| k >= 1).and_then(crate::chords::entry) {
            let mut at = 0.0;
            for &i in e.intervals {
                at += f32::from(i) * 100.0;
                v.push(Entry { pitch: at, held: None });
            }
        }
        v
    }

    /// The note list built from the chord's notes (spec: "The note list").
    fn list(&self, chord: Vec<Entry>) -> Vec<Entry> {
        let c = chord.len() as i64;
        let per = c * i64::from(self.repeat);
        let total = i64::from(self.range.max(1)) * per;
        let len = match self.direction {
            3 if total > 1 => 2 * total - 2,
            4 if total > 1 => 2 * total,
            _ => total,
        };
        (0..len)
            .map(|i| {
                let j = match self.direction {
                    2 => total - 1 - i,
                    3 if i >= total => 2 * total - i - 2,
                    4 if i >= total => 2 * total - i - 1,
                    _ => i,
                };
                let mut octave = j / per;
                let note = (j % per) % c;
                if self.range == 0 {
                    octave += note.min(5);
                }
                let e = chord[note as usize];
                Entry { pitch: e.pitch + octave as f32 * 1200.0, ..e }
            })
            .collect()
    }

    /// The next entry of `lead` (spec: "Choosing the next note").
    fn choose(&mut self, lead: &mut Lead) -> Entry {
        let count = lead.list.len() as i32;
        if lead.marked && lead.chosen {
            lead.index = anchor(&lead.list, lead.last, lead.upcoming);
        }
        lead.marked = false;
        lead.index = if self.direction == 5 {
            (f64::from(self.rng.next_u32()) * 2f64.powi(-32) * f64::from(count)) as i32
        } else {
            (lead.index + count + 1).rem_euclid(count.max(1))
        };
        let e = lead.list[lead.index as usize];
        lead.chosen = true;
        lead.last = e.pitch;
        lead.upcoming = (self.direction != 5).then(|| lead.list[((lead.index + 1) % count) as usize].pitch);
        e
    }

    /// The note `lead` plays for entry `e` from tick `at`.
    fn note_for(&self, lead: &Lead, e: Entry, at: u32) -> Play {
        let l = &lead.play;
        match e.held {
            // Chord -1: the held note's levels; its fine pitch is not played.
            Some(h) => Play { start: at, end: at, step: false, pitch: e.pitch, place: -1, src: Note { fine_pitch: 120, group: l.src.group, flags: l.src.flags, ..h.src }, ..h },
            None => Play { start: at, end: at, step: false, pitch: l.pitch + e.pitch, place: -1, ..*l },
        }
    }

    /// Ends the note `lead` has sounding at tick `at`, into `out`.
    fn close(&self, lead: &mut Lead, at: u32, out: &mut Vec<Play>) {
        if let Some(mut p) = lead.sounding.take() {
            p.end = at.max(p.start + 1);
            if let Some(g) = self.gate {
                p.end = p.end.min(p.start + g);
            }
            out.push(p);
        }
    }

    /// One tick of a leading voice (spec: "Timing").
    fn work(&mut self, lead: &mut Lead, tick: u32, out: &mut Vec<Play>) {
        if self.hold || self.slide {
            // The voice sounds itself and its pitch follows the arpeggio: written as one note per choice, lasting
            // until the next (Lemon Studio's form of the glide; a held arpeggio chooses once).
            let due = if self.hold { !lead.held_once } else { tick >= lead.next };
            if due {
                lead.held_once = true;
                let e = self.choose(lead);
                self.close(lead, tick, out);
                lead.sounding = Some(self.note_for(lead, e, tick));
                lead.next = tick + self.t;
            }
            return;
        }
        if tick < lead.release {
            return;
        }
        // Chord -1: one held key whose pitch is sounding is one held note.
        if lead.colour.is_some() && lead.list.len() == 1 && lead.sounding.is_some_and(|s| lead.list[0].held.is_some_and(|h| h.pitch == s.pitch)) {
            if lead.marked {
                lead.marked = false;
            }
            return;
        }
        self.close(lead, tick, out);
        if tick >= lead.next {
            let e = self.choose(lead);
            lead.sounding = Some(self.note_for(lead, e, tick));
            lead.release = tick + self.g;
            lead.next = tick + self.t;
        }
    }

    /// The notes the arpeggiator plays over `plays`, with the notes that lead no arpeggio as they are.
    fn run(mut self, plays: Vec<Play>) -> Vec<Play> {
        let mut out = Vec::new();
        let Some(first) = plays.iter().map(|p| p.start).min() else { return out };
        let last = plays.iter().map(|p| p.end).max().unwrap_or(first);
        let held_mode = self.chord < 0;
        let mut leads: Vec<Lead> = Vec::new();
        // Chord -1: the held notes of each colour, in order of pitch.
        let mut held: std::collections::BTreeMap<u8, Vec<Play>> = std::collections::BTreeMap::new();
        let mut at = 0;
        let mut tick = first;
        loop {
            // 1. Note ends.
            if held_mode {
                for (&colour, list) in held.iter_mut() {
                    let before = list.len();
                    list.retain(|p| p.end > tick);
                    if list.len() != before {
                        for l in leads.iter_mut().filter(|l| l.colour == Some(colour)) {
                            l.marked = true;
                        }
                    }
                }
                let mut i = 0;
                while i < leads.len() {
                    let colour = leads[i].colour.unwrap_or(0);
                    if held.get(&colour).is_none_or(Vec::is_empty) {
                        let mut l = leads.remove(i);
                        self.close(&mut l, tick, &mut out);
                    } else {
                        i += 1;
                    }
                }
            } else {
                let mut i = 0;
                while i < leads.len() {
                    if leads[i].play.end <= tick {
                        let mut l = leads.remove(i);
                        self.close(&mut l, tick, &mut out);
                    } else {
                        i += 1;
                    }
                }
            }
            // 2, 3. The notes that start.
            while at < plays.len() && plays[at].start == tick {
                let p = plays[at];
                at += 1;
                if held_mode {
                    let colour = p.src.group & 0x0F;
                    let list = held.entry(colour).or_default();
                    let lead_now = list.is_empty();
                    let i = list.partition_point(|h| h.pitch < p.pitch);
                    list.insert(i, p);
                    if lead_now {
                        leads.push(Lead::new(p, Some(colour)));
                    } else {
                        for l in leads.iter_mut().filter(|l| l.colour == Some(colour)) {
                            l.marked = true;
                        }
                    }
                } else {
                    leads.push(Lead::new(p, None));
                }
            }
            // 4. The arpeggios' work, in pool order; a marked list is built again first.
            for i in 0..leads.len() {
                let mut l = leads[i].clone();
                let rebuild = l.list.is_empty() || (held_mode && l.marked);
                if rebuild {
                    let chord = match l.colour {
                        Some(c) => self.chord_notes(Some(held.get(&c).map_or(&[][..], Vec::as_slice))),
                        None => self.chord_notes(None),
                    };
                    l.list = self.list(chord);
                }
                if !l.list.is_empty() {
                    self.work(&mut l, tick, &mut out);
                }
                leads[i] = l;
            }
            if leads.is_empty() {
                match plays.get(at) {
                    Some(p) => tick = p.start,
                    None => break,
                }
            } else {
                tick += 1;
                if tick > last.saturating_add(1) {
                    break;
                }
            }
        }
        for mut l in leads {
            self.close(&mut l, last, &mut out);
        }
        out
    }
}

impl Lead {
    fn new(play: Play, colour: Option<u8>) -> Lead {
        Lead {
            play,
            colour,
            list: Vec::new(),
            index: play.place,
            release: play.start,
            next: play.start,
            marked: false,
            chosen: false,
            last: 0.0,
            upcoming: None,
            sounding: None,
            held_once: false,
        }
    }
}

/// After the list was rebuilt: the place before the entry to go on from (spec: "Choosing the next note"). The entry
/// nearest the pitch played last (the later of equally near ones), or, when one is strictly nearer the pitch that would
/// have come next, the entry before that one.
fn anchor(list: &[Entry], last: f32, upcoming: Option<f32>) -> i32 {
    let nearest = |p: f32| {
        let mut best = (0usize, f32::INFINITY);
        for (i, e) in list.iter().enumerate() {
            let d = (e.pitch - p).round().abs();
            if d <= best.1 {
                best = (i, d);
            }
        }
        best
    };
    let (a, da) = nearest(last);
    if let Some(u) = upcoming {
        let (b, db) = nearest(u);
        if db < da {
            return b as i32 - 1;
        }
    }
    a as i32
}

// ---------------------------------------------------------------------------------------------------------
// The plugin pass

/// The notes the hosted plugin `plugin` puts out while `notes` (ticks at `ppq`, played at `tempo`) are played into it
/// from tick 0, on channel `channel`: note ons before tick `end`, each with its length up to its note off (or the
/// pass's end, one beat after `end`). The pass runs on a thread of its own named an audio thread, with the instance
/// locked (its channel is silent meanwhile), and resets the plugin before and after.
pub fn plugin_pass(plugin: &os_host::SharedPlugin, notes: &[Note], ppq: u16, tempo: f64, end: u32, channel: u16, sig: (u16, u16)) -> Vec<Note> {
    let plugin = Arc::clone(plugin);
    std::thread::scope(|sc| sc.spawn(|| {
        let _audio = os_engine::parallel::AudioScope::enter();
        let Ok(mut p) = plugin.lock() else { return Vec::new() };
        capture(&mut *p, notes, ppq, tempo, end, channel, sig)
    }).join().unwrap_or_default())
}

/// `plugin_pass` on an instance at hand.
pub fn capture(p: &mut dyn os_host::Plugin, notes: &[Note], ppq: u16, tempo: f64, end: u32, channel: u16, sig: (u16, u16)) -> Vec<Note> {
    let ppq_ = u32::from(ppq.max(1));
    let step = (ppq_ / 4).max(1);
    let stop = end + ppq_;
    let per_tick = f64::from(PASS_RATE) * 60.0 / (tempo.max(1.0) * f64::from(ppq_));
    // The input as the engine sends it: ons and offs by tick.
    let mut events: Vec<(u32, bool, &Note)> = Vec::with_capacity(notes.len() * 2);
    for n in notes.iter().filter(|n| !n.muted() && !n.slide()) {
        events.push((n.position, true, n));
        events.push((n.position + if n.length > 0 { n.length } else { step }, false, n));
    }
    // Offs before ons on one tick.
    events.sort_by_key(|&(t, on, _)| (t, on));
    os_engine::Instrument::reset(p);
    p.capture_notes(true);
    let (mut l, mut r) = (vec![0.0f32; PASS_BLOCK], vec![0.0f32; PASS_BLOCK]);
    let mut open: Vec<(u32, u8, u8, u8)> = Vec::new();
    let mut out = Vec::new();
    let mut ev = 0;
    let mut frame = 0u64;
    let beats_per_bar = f64::from(sig.0.max(1)) * 4.0 / f64::from(sig.1.max(1));
    for tick in 0..stop {
        while ev < events.len() && events[ev].0 == tick {
            let (_, on, n) = events[ev];
            let key = n.key.min(127) as u8;
            if on {
                p.note_start(0, key, f32::from(n.velocity) / 128.0, 0.0, os_engine::NoteLevels::of_bytes(n.pan, n.mod_x, n.mod_y));
            } else {
                p.note_off(0, key);
            }
            ev += 1;
        }
        let to = ((f64::from(tick) + 1.0) * per_tick).round() as u64;
        let mut done = 0u64;
        while frame + done < to {
            let k = ((to - frame - done) as usize).min(PASS_BLOCK);
            let beats = f64::from(tick) / f64::from(ppq_) + done as f64 / (per_tick * f64::from(ppq_));
            let bar = (beats / beats_per_bar).floor();
            os_engine::Instrument::transport(p, &os_engine::TransportInfo {
                playing: true,
                tempo,
                frame: frame + done,
                beats,
                bar_start: bar * beats_per_bar,
                bar: bar as i32,
                numerator: sig.0,
                denominator: sig.1,
                looping: None,
            });
            os_engine::Instrument::process(p, PASS_RATE, &mut l[..k], &mut r[..k]);
            for o in p.take_notes_out() {
                let at = tick + ((done as f64 + f64::from(o.offset)) / per_tick) as u32;
                if o.on && o.velocity > 0.0 {
                    if at < end {
                        let vel = (o.velocity * 128.0).round().clamp(1.0, 128.0) as u8;
                        open.push((at, o.key, o.channel, vel));
                    }
                } else if let Some(i) = open.iter().position(|x| x.1 == o.key && x.2 == o.channel) {
                    let (start, key, _, vel) = open.remove(i);
                    out.push(Note { position: start, length: (at - start).max(1), key: u16::from(key), velocity: vel, ..Note::step(channel, start) });
                }
            }
            done += k as u64;
        }
        frame += done;
    }
    for (start, key, _, vel) in open {
        out.push(Note { position: start, length: (stop - start).max(1), key: u16::from(key), velocity: vel, ..Note::step(channel, start) });
    }
    p.capture_notes(false);
    os_engine::Instrument::reset(p);
    out.sort_by_key(|n| (n.position, n.key));
    out
}

// ---------------------------------------------------------------------------------------------------------
// The command

/// Burn MIDI to ▸ on rack row `row`: enabled on a channel that plays notes (not an automation clip's).
pub fn can_burn(s: &State, row: usize) -> bool {
    s.project.channels.get(row).is_some_and(|c| c.kind != ChannelKind::Automation)
}

impl App {
    /// Burn MIDI to ▸ Current pattern (`new` false) or New pattern on rack row `row`. Returns the hint bar's message.
    pub(crate) fn burn_midi(&mut self, row: usize, new: bool) -> Option<String> {
        if !can_burn(&self.state, row) {
            return None;
        }
        let s = &self.state;
        let ch = &s.project.channels[row];
        let name = crate::rack::channel_name(s, row);
        let (ppq, cur) = (s.project.ppq.max(4), s.pattern.max(1));
        let src: Vec<Note> = s.project.pattern(cur).map(|p| p.notes.iter().filter(|n| n.channel == ch.index).copied().collect()).unwrap_or_default();
        if !src.iter().any(|n| !n.muted() && !n.slide()) {
            return Some(format!("{name} has no notes in this pattern to burn"));
        }
        let own = generate(ch, &src, ppq);
        // A hosted instrument plays what the channel's own arpeggiator made, and what it puts out is what is burnt. One
        // that declares no note output (Lemon Sampler on every sampler channel, most instruments) is not played: the
        // pass would only keep the channel silent for nothing.
        let hosted = self.plugins.hosted.get(&row).map(|h| Arc::clone(&h.plugin)).filter(|h| h.lock().is_ok_and(|p| p.puts_out_notes()));
        let from_plugin = hosted.map(|h| {
            let input = own.as_deref().unwrap_or(&src);
            let end = s.project.pattern_length(cur).max(input.iter().map(|n| n.position + n.length.max(1)).max().unwrap_or(0));
            plugin_pass(&h, input, ppq, s.tempo, end, ch.index, s.project.signature().unwrap_or((4, 4)))
        });
        let notes = match (from_plugin, own) {
            (Some(p), _) if !p.is_empty() => p,
            (_, Some(o)) => o,
            (Some(_), None) => return Some(format!("{name} generates no notes: its arpeggiator and echo delay are off, and its plugin put out none")),
            (None, None) => return Some(format!("{name} generates no notes: its arpeggiator and echo delay are off")),
        };
        let index = ch.index;
        let count = notes.iter().filter(|n| !n.muted() && !n.slide()).count();
        let msg = crate::control::edit_app(self, STEP, crate::control::command::Region::Project, |app| {
            let s = &mut app.state;
            let to = if new { crate::patterns::next_empty(s, cur) } else { cur };
            let p = crate::patterns::pat_mut(s, to);
            if new {
                p.notes.extend(notes.iter().filter(|n| !n.muted() && !n.slide()).copied());
            } else {
                p.notes.retain(|n| n.channel != index);
                p.notes.extend(notes.iter().copied());
            }
            p.notes.sort_by_key(|n| n.position);
            // The channel's own generators are off now: the notes are not made twice.
            let m = &mut s.project.channels[row].misc;
            m.arp.direction = 0;
            m.echo.feed = 0.0;
            s.pattern = to;
            s.piano.dirty = true;
            if new { format!("{count} notes of {name} burnt to {}", crate::patterns::name_of(s, to)) } else { format!("{count} notes of {name} burnt") }
        });
        self.invalidate();
        Some(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use os_model::Arpeggio;

    /// A note of channel 0 at `pos`, `len` ticks long, on `key`.
    fn note(pos: u32, len: u32, key: u16) -> Note {
        Note { length: len, key, ..Note::step(0, pos) }
    }

    fn keys(v: &[Note]) -> Vec<(u32, u32, u16)> {
        v.iter().map(|n| (n.position, n.length, n.key)).collect()
    }

    fn arp_channel(f: impl FnOnce(&mut Arpeggio)) -> Channel {
        let mut ch = crate::app::sampler_channel(0, "Ch", 0);
        f(&mut ch.misc.arp);
        ch
    }

    /// The time law's table at 96 PPQ (spec: audio/channel-misc.md "The time law").
    #[test]
    fn the_time_law() {
        let want = [(100, 0), (200, 1), (300, 1), (400, 2), (450, 3), (512, 4), (600, 5), (700, 8), (800, 11), (900, 16), (1024, 24), (1100, 31), (1200, 43), (1446, 96)];
        for (v, t) in want {
            assert_eq!(law_ticks(v, 96), t, "{v}");
        }
        assert_eq!(law_ticks(700, 192), 16);
    }

    /// Off: nothing generated.
    #[test]
    fn nothing_without_the_arpeggiator_or_echoes() {
        assert_eq!(generate(&crate::app::sampler_channel(0, "Ch", 0), &[note(0, 96, 60)], 96), None);
    }

    /// A major chord upwards, one step a note: a note four steps long plays C E G C (spec: "The note list", "Timing").
    #[test]
    fn a_chord_arpeggio() {
        let ch = arp_channel(|a| (a.direction, a.chord) = (1, 1));
        let got = generate(&ch, &[note(0, 96, 60)], 96).unwrap();
        assert_eq!(keys(&got), [(0, 24, 60), (24, 24, 64), (48, 24, 67), (72, 24, 60)]);
        // Down, two octaves, half-step gate.
        let ch = arp_channel(|a| (a.direction, a.chord, a.range, a.gate) = (2, 1, 2, 0.5));
        let got = generate(&ch, &[note(0, 72, 60)], 96).unwrap();
        assert_eq!(keys(&got), [(0, 12, 79), (24, 12, 76), (48, 12, 72)]);
        // Bounce: no repeated ends.
        let ch = arp_channel(|a| (a.direction, a.chord) = (3, 1));
        let got = generate(&ch, &[note(0, 120, 60)], 96).unwrap();
        assert_eq!(got.iter().map(|n| n.key).collect::<Vec<_>>(), [60, 64, 67, 64, 60]);
    }

    /// Chord -1 plays the held notes in order of pitch, with their own levels, one held key as one note (spec: "Notes
    /// that start and end").
    #[test]
    fn an_arpeggio_over_the_held_notes() {
        let ch = arp_channel(|a| a.direction = 1);
        let mut e = note(0, 96, 64);
        e.velocity = 50;
        let got = generate(&ch, &[note(0, 96, 67), e, note(0, 96, 60)], 96).unwrap();
        assert_eq!(keys(&got), [(0, 24, 60), (24, 24, 64), (48, 24, 67), (72, 24, 60)]);
        assert_eq!(got[1].velocity, 50, "the held note's levels");
        // A note joining a held one: the arpeggio goes on from the pitch it played.
        let got = generate(&ch, &[note(0, 96, 60), note(24, 72, 72)], 96).unwrap();
        assert_eq!(got.iter().map(|n| n.key).collect::<Vec<_>>(), [60, 72, 60, 72]);
        // One held key is one held note.
        let got = generate(&ch, &[note(0, 96, 60)], 96).unwrap();
        assert_eq!(keys(&got), [(0, 96, 60)]);
    }

    /// The echo delay: each echo later by the time, quieter by the feed, its pitch stepped (spec: "Echo delay").
    #[test]
    fn echoes() {
        let mut ch = crate::app::sampler_channel(0, "Ch", 0);
        let e = &mut ch.misc.echo;
        (e.feed, e.echoes, e.time, e.pitch) = (0.25, 2, 48.0 / 384.0, 100.0 / 1200.0);
        let got = generate(&ch, &[note(0, 12, 60)], 96).unwrap();
        assert_eq!(keys(&got), [(0, 12, 60), (24, 12, 61), (48, 12, 62)]);
        // Volume times 0.5, then 0.25 (feed 6400): the velocity whose curve gives that.
        let v = |n: &Note| os_dsp::level::curve(f32::from(n.velocity) * 0.01);
        assert!((v(&got[1]) - 0.5).abs() < 0.02 && (v(&got[2]) - 0.25).abs() < 0.02, "{got:?}");
        // A mono channel has no echoes.
        ch.misc.mono = true;
        assert_eq!(generate(&ch, &[note(0, 12, 60)], 96), None);
    }

    /// Muted and slide notes stay; a note outside the key range goes.
    #[test]
    fn kept_and_dropped_notes() {
        let mut ch = arp_channel(|a| (a.direction, a.chord) = (1, 0));
        ch.misc.key_range = [48, 72];
        let mut muted = note(0, 24, 62);
        muted.set_muted(true);
        let got = generate(&ch, &[note(0, 24, 60), muted, note(0, 24, 90)], 96).unwrap();
        assert_eq!(got.len(), 2);
        assert!(got.iter().any(|n| n.muted() && n.key == 62) && got.iter().any(|n| !n.muted() && n.key == 60));
    }

    // -----------------------------------------------------------------------------------------------------
    // The command

    /// An app whose rack holds no hosted plugins (the plugin pass is tested on its own below), its current pattern
    /// holding `notes` of rack row 0's channel and a step of row 1's.
    fn app_with(notes: &[Note]) -> App {
        let mut app = App::new(None);
        app.plugins = crate::plugins::Plugins::default();
        let (a, b) = (app.state.project.channels[0].index, app.state.project.channels[1].index);
        let cur = app.state.pattern.max(1);
        app.state.pattern = cur;
        let p = crate::patterns::pat_mut(&mut app.state, cur);
        p.notes = notes.iter().map(|n| Note { channel: a, ..*n }).collect();
        p.notes.push(Note::step(b, 0));
        app
    }

    /// The notes of channel row `row` in pattern `pat`.
    fn notes_of(app: &App, pat: u16, row: usize) -> Vec<Note> {
        let ch = app.state.project.channels[row].index;
        app.state.project.pattern(pat).map(|p| p.notes.iter().filter(|n| n.channel == ch).copied().collect()).unwrap_or_default()
    }

    /// Current pattern: the arpeggio's notes replace the channel's notes in the pattern, another channel's stay, the
    /// arpeggiator is switched off, and it is one undo step.
    #[test]
    fn burning_an_arpeggio_into_the_current_pattern() {
        let mut app = app_with(&[note(0, 96, 60)]);
        assert_eq!(app.state.project.ppq, 96);
        (app.state.project.channels[0].misc.arp.direction, app.state.project.channels[0].misc.arp.chord) = (1, 1);
        let cur = app.state.pattern;
        let msg = crate::rack::run(&mut app, crate::rack::RCmd::BurnMidi(0, false));
        assert_eq!(keys(&notes_of(&app, cur, 0)), [(0, 24, 60), (24, 24, 64), (48, 24, 67), (72, 24, 60)]);
        assert!(msg.is_some_and(|m| m.starts_with("4 notes of ")), "the hint bar's message");
        assert_eq!(notes_of(&app, cur, 1).len(), 1, "another channel's notes stay");
        assert_eq!(app.state.project.channels[0].misc.arp.direction, 0, "the arpeggiator is off: not generated twice");
        assert_eq!(app.state.pattern, cur);
        crate::history::undo(&mut app.state);
        assert_eq!(keys(&notes_of(&app, cur, 0)), [(0, 96, 60)], "one undo step");
        assert_eq!(app.state.project.channels[0].misc.arp.direction, 1);
    }

    /// New pattern: the echoes go into the next empty pattern, which becomes the current one; the pattern burnt from
    /// keeps its notes. One undo step.
    #[test]
    fn burning_echoes_into_a_new_pattern() {
        let mut app = app_with(&[note(0, 12, 60)]);
        let e = &mut app.state.project.channels[0].misc.echo;
        (e.feed, e.echoes, e.time, e.pitch) = (0.25, 2, 48.0 / 384.0, 100.0 / 1200.0);
        let cur = app.state.pattern;
        let to = crate::patterns::next_empty(&app.state, cur);
        assert_ne!(to, cur);
        crate::rack::run(&mut app, crate::rack::RCmd::BurnMidi(0, true));
        assert_eq!(app.state.pattern, to, "the new pattern is the current one");
        assert_eq!(keys(&notes_of(&app, to, 0)), [(0, 12, 60), (24, 12, 61), (48, 12, 62)]);
        assert!(notes_of(&app, to, 1).is_empty(), "only the burnt channel's notes");
        assert_eq!(keys(&notes_of(&app, cur, 0)), [(0, 12, 60)], "the pattern burnt from keeps its notes");
        assert_eq!(app.state.project.channels[0].misc.echo.feed, 0.0, "the echo delay is off");
        crate::history::undo(&mut app.state);
        assert!(notes_of(&app, to, 0).is_empty(), "one undo step");
        assert_eq!(app.state.project.channels[0].misc.echo.feed, 0.25);
    }

    /// A channel with no notes in the pattern, or one that generates none, says so in the hint bar and changes
    /// nothing; an automation clip's channel has no burn.
    #[test]
    fn nothing_to_burn() {
        let mut app = app_with(&[note(0, 96, 60)]);
        let cur = app.state.pattern;
        let before = app.state.project.pattern(cur).unwrap().notes.clone();
        let steps = app.state.history.len();
        let msg = crate::rack::run(&mut app, crate::rack::RCmd::BurnMidi(0, false)).unwrap();
        assert!(msg.ends_with("its arpeggiator and echo delay are off"), "{msg}");
        let msg = crate::rack::run(&mut app, crate::rack::RCmd::BurnMidi(2, true)).unwrap();
        assert!(msg.ends_with("has no notes in this pattern to burn"), "{msg}");
        assert_eq!(app.state.project.pattern(cur).unwrap().notes, before);
        assert_eq!((app.state.pattern, app.state.history.len()), (cur, steps), "no undo step");
        assert!(can_burn(&app.state, 0) && !can_burn(&app.state, app.state.project.channels.len()));
        app.state.project.channels[0].kind = ChannelKind::Automation;
        assert!(!can_burn(&app.state, 0));
        assert_eq!(crate::rack::run(&mut app, crate::rack::RCmd::BurnMidi(0, false)), None);
    }

    /// A hosted instrument that puts out every note it is played a fifth up, with its velocity, and its note off.
    struct Fifths {
        desc: os_host::Descriptor,
        capture: bool,
        out: Vec<os_host::NoteOut>,
        resets: u32,
        /// Whether it declares a note output.
        outputs: bool,
    }

    impl Fifths {
        fn put(&mut self, key: u8, velocity: f32, on: bool) {
            if self.capture {
                self.out.push(os_host::NoteOut { offset: 0, key: key + 7, channel: 0, velocity, on });
            }
        }
    }

    impl os_engine::Instrument for Fifths {
        fn note_on(&mut self, _: u32, key: u8, velocity: f32) {
            self.put(key, velocity, true);
        }
        fn note_off(&mut self, _: u32, key: u8) {
            self.put(key, 0.0, false);
        }
        fn process(&mut self, _: u32, _: &mut [f32], _: &mut [f32]) {}
        fn reset(&mut self) {
            self.resets += 1;
        }
    }

    impl os_engine::HostedEffect for Fifths {
        fn process_fx(&mut self, _: u32, _: &mut [f32], _: &mut [f32]) {}
    }

    impl os_host::Plugin for Fifths {
        fn desc(&self) -> &os_host::Descriptor {
            &self.desc
        }
        fn activate(&mut self, _: u32) -> bool {
            true
        }
        fn idle(&mut self) {}
        fn has_editor(&self) -> bool {
            false
        }
        fn editor_open(&self) -> bool {
            false
        }
        fn open_editor(&mut self, _: *mut std::ffi::c_void, _: f32) -> Result<(u32, u32), String> {
            Err(String::new())
        }
        fn editor_size(&self) -> Option<(u32, u32)> {
            None
        }
        fn close_editor(&mut self) {}
        fn save_state(&self) -> Option<Vec<u8>> {
            None
        }
        fn load_state(&mut self, _: &[u8]) -> bool {
            false
        }
        fn capture_notes(&mut self, on: bool) {
            self.capture = on;
            self.out.clear();
        }
        fn take_notes_out(&mut self) -> Vec<os_host::NoteOut> {
            std::mem::take(&mut self.out)
        }
        fn puts_out_notes(&self) -> bool {
            self.outputs
        }
    }

    /// A hosted instrument's notes are what is burnt: it is played the pattern's notes after the channel's own
    /// arpeggiator, and its note ons and offs come back as notes at their ticks. The pass resets it before and after,
    /// and leaves it not capturing.
    #[test]
    fn burning_the_notes_a_plugin_puts_out() {
        let mut app = app_with(&[note(0, 48, 60), note(48, 0, 62)]);
        let desc = crate::plugins::builtin()[0].desc.clone();
        let fifths = Arc::new(std::sync::Mutex::new(Fifths { desc, capture: false, out: Vec::new(), resets: 0, outputs: true }));
        let mut h = crate::plugins::Hosted::new(fifths.clone());
        h.owner = app.state.project.channels[0].id;
        app.plugins.hosted.insert(0, h);
        let cur = app.state.pattern;
        crate::rack::run(&mut app, crate::rack::RCmd::BurnMidi(0, false));
        // A step plays a step long (24 ticks at 96 PPQ).
        assert_eq!(keys(&notes_of(&app, cur, 0)), [(0, 48, 67), (48, 24, 69)]);
        assert_eq!(notes_of(&app, cur, 0)[0].velocity, 100, "the velocity it put out");
        {
            let f = fifths.lock().unwrap();
            assert_eq!((f.capture, f.resets), (false, 2), "reset before and after, not capturing");
        }
        crate::history::undo(&mut app.state);
        // With the channel's arpeggiator on, the plugin is played the arpeggio.
        (app.state.project.channels[0].misc.arp.direction, app.state.project.channels[0].misc.arp.chord) = (1, 1);
        let ch = app.state.project.channels[0].index;
        crate::patterns::pat_mut(&mut app.state, cur).notes.retain(|n| n.position == 0 || n.channel != ch);
        crate::rack::run(&mut app, crate::rack::RCmd::BurnMidi(0, false));
        assert_eq!(keys(&notes_of(&app, cur, 0)), [(0, 24, 67), (24, 24, 71)]);
        assert_eq!(app.state.project.channels[0].misc.arp.direction, 0);
    }

    /// A hosted instrument that declares no note output (Lemon Sampler, most instruments) is not played: the channel's
    /// own arpeggio is what is burnt, and the instrument is never reset.
    #[test]
    fn an_instrument_without_note_outputs_is_not_played() {
        let mut app = app_with(&[note(0, 48, 60), note(0, 48, 64)]);
        let desc = crate::plugins::builtin()[0].desc.clone();
        let silent = Arc::new(std::sync::Mutex::new(Fifths { desc, capture: false, out: Vec::new(), resets: 0, outputs: false }));
        let mut h = crate::plugins::Hosted::new(silent.clone());
        h.owner = app.state.project.channels[0].id;
        app.plugins.hosted.insert(0, h);
        (app.state.project.channels[0].misc.arp.direction, app.state.project.channels[0].misc.arp.chord) = (1, 1);
        let cur = app.state.pattern;
        let own = generate(&app.state.project.channels[0], &notes_of(&app, cur, 0), app.state.project.ppq).expect("an arpeggio");
        crate::rack::run(&mut app, crate::rack::RCmd::BurnMidi(0, false));
        assert_eq!(keys(&notes_of(&app, cur, 0)), keys(&own), "the channel's own arpeggio, nothing a fifth up");
        assert_eq!(silent.lock().unwrap().resets, 0, "never played");
    }
}
