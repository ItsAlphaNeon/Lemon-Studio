// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! From events to actions (`pages/voice.md`): held keys, the pedal, allocation, the actions put on the voices' lists.

use crate::settings::Settings;
use crate::voice::{ActKind, Action, Shared, Voice, VOICES};
use crate::Event;

/// The channel's note state that lives outside the voices.
pub struct Channel {
    /// Held keys in order of arrival.
    pub held: [u8; 128],
    pub n_held: usize,
    pub ctrl: [f32; 4],
    pub press: f32,
    /// The sustain pedal.
    pub pedal: bool,
    /// Times each key is down.
    pub key_count: [i32; 128],
    /// Note-offs the pedal keeps, per key.
    pub pedal_held: [i32; 128],
    /// The velocity each key was last pressed with.
    pub key_vel: [i32; 128],
}

impl Channel {
    pub fn new() -> Channel {
        Channel { held: [0; 128], n_held: 0, ctrl: [0.0; 4], press: 0.0, pedal: false, key_count: [0; 128], pedal_held: [0; 128], key_vel: [100; 128] }
    }

    fn held_remove(&mut self, key: u8) -> bool {
        if let Some(i) = self.held[..self.n_held].iter().position(|k| *k == key) {
            self.held.copy_within(i + 1..self.n_held, i);
            self.n_held -= 1;
            true
        } else {
            false
        }
    }

    fn held_push(&mut self, key: u8) {
        self.held_remove(key);
        if self.n_held < 128 {
            self.held[self.n_held] = key;
            self.n_held += 1;
        }
    }

    fn priority_key(&self, prio: i32) -> i32 {
        let h = &self.held[..self.n_held];
        match prio {
            1 => *h.iter().min().unwrap_or(&0) as i32,
            2 => *h.iter().max().unwrap_or(&0) as i32,
            _ => h.last().copied().unwrap_or(0) as i32,
        }
    }

    /// Lowest and highest held keys, and whether any is held.
    pub fn duo_keys(&self) -> (i32, i32, bool) {
        let h = &self.held[..self.n_held];
        match (h.iter().min(), h.iter().max()) {
            (Some(a), Some(b)) => (*a as i32, *b as i32, true),
            _ => (0, 0, false),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum VState {
    Waiting,
    Idle,
    OffWaiting,
    Releasing,
    Playing,
}

fn state_of(v: &Voice) -> VState {
    if v.has_on_waiting() {
        VState::Waiting
    } else if !v.busy {
        VState::Idle
    } else if !v.gated && v.has_off_waiting() {
        VState::OffWaiting
    } else if !v.gated && v.playing {
        VState::Releasing
    } else if !v.gated {
        VState::Idle
    } else {
        VState::Playing
    }
}

#[inline]
fn is_mono_type(s: &Settings) -> bool {
    matches!(s.mode, 1 | 2 | 3)
}

/// What the note handling needs besides the channel: everything the events change.
pub struct Notes<'a> {
    pub s: &'a Settings,
    pub ch: &'a mut Channel,
    pub sh: &'a mut Shared,
    pub voices: &'a mut [Box<Voice>],
}

/// A velocity from the event's 0..1 form.
#[inline]
pub fn velocity_of(velocity: f32) -> i32 {
    ((velocity * 127.0).round() as i32).clamp(1, 127)
}

impl Notes<'_> {
    /// Handles one host event that goes straight to the note handling. `at` is its time, `span_t` the span's start.
    pub fn take_event(&mut self, ev: &Event, at: u64, span_t: u64) {
        match *ev {
            Event::NoteOn { key, velocity, .. } => {
                if velocity <= 0.0 {
                    self.key_off(key, at);
                } else {
                    self.key_on(key, velocity_of(velocity), at);
                }
            }
            Event::NoteOff { key, .. } => self.key_off(key, at),
            Event::PitchBend { value, .. } => {
                self.ch.ctrl[1] = if value < -1.0 {
                    -1.0
                } else if value > 1.0 {
                    1.0
                } else {
                    value
                };
                self.controller_changed(span_t);
            }
            Event::Controller { cc, value, .. } => match cc {
                1 => {
                    self.ch.ctrl[0] = value;
                    self.controller_changed(span_t);
                }
                2 => {
                    self.ch.ctrl[2] = value;
                    self.controller_changed(span_t);
                }
                11 => {
                    self.ch.ctrl[3] = value;
                    self.controller_changed(span_t);
                }
                64 => self.pedal(value * 127.0 >= 64.0, at),
                120 | 123 => self.all_notes_off(at),
                _ => {}
            },
            Event::Pressure { value, .. } => {
                self.ch.press = value;
                self.controller_changed(span_t);
            }
            Event::Panic { .. } => self.panic(at),
            Event::Tuning { key, semitones, .. } => {
                for v in self.voices.iter_mut() {
                    if v.note == key as i32 {
                        v.tune = semitones;
                    }
                }
            }
        }
    }

    pub fn all_notes_off(&mut self, at: u64) {
        for i in 0..VOICES {
            let v = &mut self.voices[i];
            if v.busy && v.gated {
                v.gated = false;
                put_off(v, at);
            }
        }
        self.clear_keys();
    }

    pub fn panic(&mut self, at: u64) {
        for v in self.voices.iter_mut() {
            if v.busy {
                v.gated = false;
                v.put(action(ActKind::Kill, at));
            }
        }
        self.clear_keys();
    }

    fn clear_keys(&mut self) {
        self.ch.n_held = 0;
        self.ch.key_count = [0; 128];
        self.ch.pedal_held = [0; 128];
        self.arm_lfos();
    }

    pub fn arm_lfos(&mut self) {
        self.sh.single[0].armed = true;
        self.sh.single[1].armed = true;
    }

    /// Ctrl goes on every busy voice and on the current voice, busy or not.
    fn controller_changed(&mut self, span_t: u64) {
        let current = self.sh.current;
        for v in self.voices.iter_mut() {
            if v.busy || v.index == current {
                v.put(action(ActKind::Ctrl, span_t));
            }
        }
    }

    /// Pedal up: every note-off it kept goes through step 2, by key from 0 up, the key's kept count times.
    fn pedal(&mut self, down: bool, at: u64) {
        let was = self.ch.pedal;
        self.ch.pedal = down;
        if was && !down {
            self.release_kept(at);
        }
    }

    fn release_kept(&mut self, at: u64) {
        for k in 0..128u8 {
            let n = core::mem::take(&mut self.ch.pedal_held[k as usize]);
            for _ in 0..n {
                self.key_off_counted(k, at);
            }
        }
    }

    /// A key down, under the sustain pedal's rules.
    pub fn key_on(&mut self, key: u8, vel: i32, at: u64) {
        if key == 0 {
            return;
        }
        self.ch.key_count[key as usize] += 1;
        self.ch.key_vel[key as usize] = vel;
        if is_mono_type(self.s) {
            self.note_on_mono(key, vel, at);
        } else {
            self.note_on_poly(key, vel, at);
        }
        if self.ch.pedal && matches!(self.s.mode, 1 | 2) {
            self.release_kept(at);
        }
    }

    /// A key up, under the sustain pedal's rules.
    pub fn key_off(&mut self, key: u8, at: u64) {
        if key == 0 {
            return;
        }
        let k = key as usize;
        if self.ch.pedal && (!matches!(self.s.mode, 1 | 2) || self.ch.key_count.iter().sum::<i32>() < 2) {
            self.ch.pedal_held[k] += 1;
            return;
        }
        self.key_off_counted(key, at);
    }

    /// Step 2 of the pedal's note-off: the key's count goes down, and at 0 the note-off happens.
    fn key_off_counted(&mut self, key: u8, at: u64) {
        let k = key as usize;
        self.ch.key_count[k] = (self.ch.key_count[k] - 1).max(0);
        if self.ch.key_count[k] > 0 {
            if !is_mono_type(self.s) {
                self.release_extra_group(key, at);
            }
            return;
        }
        self.note_off(key, at);
    }

    /// In poly types a key still down: when more groups play it than it is down, the oldest of them is released.
    fn release_extra_group(&mut self, key: u8, at: u64) {
        let n = self.s.voices.max(1) as usize;
        let st = (self.s.stack.max(1) as usize).min(n);
        let groups = n / st;
        let mut count = 0;
        let mut oldest: Option<usize> = None;
        for g in 0..groups {
            let v = &self.voices[g * st];
            if v.gated && v.vnote == key as i32 {
                count += 1;
                if oldest.is_none_or(|o| v.age < self.voices[o * st].age) {
                    oldest = Some(g);
                }
            }
        }
        if count > self.ch.key_count[key as usize] {
            if let Some(g) = oldest {
                for i in g * st..(g + 1) * st {
                    let v = &mut self.voices[i];
                    if v.busy && v.gated {
                        v.gated = false;
                        if !v.has_on_waiting() {
                            v.age = self.sh.date();
                        }
                        put_off(v, at);
                    }
                }
            }
        }
    }

    fn note_on_poly(&mut self, key: u8, vel: i32, at: u64) {
        let s = self.s;
        self.ch.held_push(key);
        let n = s.voices.max(1) as usize;
        let st = (s.stack.max(1) as usize).min(n);
        let groups = n / st;
        let g = choose_group(key as i32, groups, st, s.mode == 4, self.ch, self.voices);
        for k in 0..st {
            let v = &mut self.voices[g * st + k];
            v.take_on();
            v.busy = true;
            v.gated = true;
            v.vnote = key as i32;
            v.age = self.sh.date();
            v.put(Action { kind: ActKind::On, time: at, key: key as i32, vel, retrigger: true, glide: true, member: k as i32 });
        }
    }

    fn note_on_mono(&mut self, key: u8, vel: i32, at: u64) {
        let s = self.s;
        self.ch.held_push(key);
        let legato_mode = matches!(s.mode, 2 | 3);
        let gated0 = self.voices[0].busy && self.voices[0].gated;
        let legato = legato_mode && gated0;
        let target = self.ch.priority_key(s.priority);
        if s.mode != 3 && gated0 && target == self.voices[0].vnote && target != key as i32 {
            return;
        }
        let mut retrigger = !legato;
        let mut glide = s.mode != 2 || legato;
        let st = s.stack.clamp(1, s.voices.max(1)) as usize;
        for k in 0..st {
            let v = &mut self.voices[k];
            if let Some(old) = v.take_on() {
                retrigger = old.retrigger;
                glide = old.glide;
                if !v.playing {
                    v.ghost_start();
                }
            }
            v.busy = true;
            v.gated = true;
            v.vnote = target;
            v.age = self.sh.date();
            v.put(Action { kind: ActKind::On, time: at, key: target, vel, retrigger, glide, member: k as i32 });
        }
    }

    fn note_off(&mut self, key: u8, at: u64) {
        let s = self.s;
        let was_held = self.ch.held_remove(key);
        let empty = self.ch.n_held == 0;
        if empty {
            self.arm_lfos();
        }
        let k = key as i32;
        if !is_mono_type(s) {
            for i in 0..VOICES {
                let v = &mut self.voices[i];
                if v.busy && v.gated && (v.vnote == k || empty) {
                    v.gated = false;
                    if !v.has_on_waiting() {
                        v.age = self.sh.date();
                    }
                    put_off(v, at);
                }
            }
            return;
        }
        if !was_held {
            return;
        }
        let st = s.stack.clamp(1, s.voices.max(1)) as usize;
        for i in st..VOICES {
            let v = &mut self.voices[i];
            if v.busy && v.gated && (v.vnote == k || empty) {
                v.gated = false;
                put_off(v, at);
            }
        }
        if empty {
            for i in 0..st {
                let v = &mut self.voices[i];
                if v.gated {
                    v.gated = false;
                    put_off(v, at);
                }
            }
            return;
        }
        let target = self.ch.priority_key(s.priority);
        if target != self.voices[0].vnote {
            let mut retrigger = s.mode == 1;
            let mut glide = true;
            let vel = self.ch.key_vel[target.clamp(0, 127) as usize];
            for m in 0..st {
                let v = &mut self.voices[m];
                if let Some(old) = v.take_on() {
                    retrigger = old.retrigger;
                    glide = old.glide;
                    if !v.playing {
                        v.ghost_start();
                    }
                }
                v.vnote = target;
                v.put(Action { kind: ActKind::On, time: at, key: target, vel, retrigger, glide, member: m as i32 });
            }
        }
    }
}

fn action(kind: ActKind, at: u64) -> Action {
    Action { kind, time: at, ..Action::EMPTY }
}

/// An Off. With an On still waiting on the voice (a note let go before it started) its time is the start of the second block
/// after the On's block, or the event's own time if that is later.
fn put_off(v: &mut Voice, at: u64) {
    let mut time = at;
    if let Some(on) = v.actions[..v.n_actions].iter().find(|a| a.kind == ActKind::On) {
        time = time.max((on.time / 64 + 2) * 64);
    }
    v.put(action(ActKind::Off, time));
}

fn choose_group(key: i32, groups: usize, stack: usize, poly2: bool, ch: &Channel, voices: &[Box<Voice>]) -> usize {
    let st = |g: usize| state_of(&voices[g * stack]);
    let age = |g: usize| voices[g * stack].age;
    let smallest = |want: VState| -> Option<usize> {
        let mut best: Option<usize> = None;
        for g in 0..groups {
            if st(g) == want && best.is_none_or(|b| age(g) < age(b)) {
                best = Some(g);
            }
        }
        best
    };
    let largest = |want: VState| -> Option<usize> {
        let mut best: Option<usize> = None;
        for g in 0..groups {
            if st(g) == want && best.is_none_or(|b| age(g) > age(b)) {
                best = Some(g);
            }
        }
        best
    };
    if poly2 {
        if let Some(g) = largest(VState::OffWaiting).or_else(|| largest(VState::Releasing)) {
            return g;
        }
    }
    if let Some(g) = smallest(VState::Idle) {
        return g;
    }
    if let Some(g) = smallest(VState::Releasing) {
        return g;
    }
    if let Some(g) = smallest(VState::OffWaiting) {
        return g;
    }
    // the holding candidate: a key sounding on several groups goes first (its oldest), then the oldest note held only by
    // the pedal
    let key_of = |g: usize| voices[g * stack].note;
    let age0 = voices[0].age;
    let mut candidate: Option<usize> = None;
    let mut best = 0i32;
    for g in 0..groups {
        if st(g) != VState::Playing {
            continue;
        }
        let k = key_of(g);
        let mut oldest = g;
        let mut count = 0i32;
        for h in g + 1..groups {
            if key_of(h) == k {
                if age(h) < age(oldest) {
                    oldest = h;
                }
                count += 1;
                if count > best || (count == best && age(oldest) < age0) {
                    candidate = Some(oldest);
                    best = count;
                }
            }
        }
        let ki = k.clamp(0, 127) as usize;
        if ch.pedal_held[ki] == ch.key_count[ki] && best < 1 && candidate.is_none_or(|c| age(g) < age(c)) {
            candidate = Some(g);
        }
    }
    if let Some(g) = candidate {
        return g;
    }
    // among the playing groups: the keys held by the most groups; of those the nearest to the new key
    let mut counts = [0u8; 128];
    let mut order: [i32; VOICES] = [-1; VOICES];
    let mut n_keys = 0;
    let mut any = false;
    for g in 0..groups {
        if st(g) == VState::Playing {
            any = true;
            let kv = voices[g * stack].vnote;
            let k = kv.clamp(0, 127) as usize;
            if counts[k] == 0 {
                order[n_keys] = k as i32;
                n_keys += 1;
            }
            counts[k] += 1;
        }
    }
    if any {
        let most = order[..n_keys].iter().map(|k| counts[*k as usize]).max().unwrap_or(0);
        let mut best = -1i32;
        let mut best_d = i32::MAX;
        for &k in &order[..n_keys] {
            if counts[k as usize] == most {
                let d = (k - key).abs();
                if d < best_d {
                    best_d = d;
                    best = k;
                }
            }
        }
        let mut pick: Option<usize> = None;
        for g in 0..groups {
            if st(g) == VState::Playing && voices[g * stack].vnote.clamp(0, 127) == best && pick.is_none_or(|b| age(g) < age(b)) {
                pick = Some(g);
            }
        }
        if let Some(g) = pick {
            return g;
        }
    }
    smallest(VState::Waiting).unwrap_or(0)
}
