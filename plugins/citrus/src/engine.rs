// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The synthesizer as a host sees it: a patch, notes in and out, and render calls of host-rate stereo.
//
// spec: plugins/sytrus.md ("Rates and control blocks", "Unison", "State and timing"), plugins/sytrus-effects.md
// ("Order", "Awake and asleep")

use crate::curve::{LfoShape, GLOBAL};
use crate::filter::SvfTables;
use crate::fx::{Bus, Section};
use crate::num::{round, s};
use crate::patch::Patch;
use crate::state::State;
use crate::voice::{unison, Ctx, Levels, Note, Random, Voice};

/// Where the sample counter stands when the song starts (FL runs each instance for a while before it; the spec
/// gives 0 to 10240 voice frames, fitted per instance).
pub const COUNTER_START: u64 = 8192;
/// Voices kept at most (the oldest note goes first beyond it).
pub const MAX_VOICES: usize = 512;

/// A note-on from the host.
#[derive(Debug, Clone, Copy)]
pub struct NoteOn {
    pub levels: Levels,
    /// Velocity 0..1 (velocity / 128).
    pub velocity: f32,
    /// Release velocity 0..1 (0.5 = FL's default of 64).
    pub release: f32,
}

struct Playing {
    id: u64,
    shared: bool,
    voices: Vec<Voice>,
}

pub struct Citrus {
    host_rate: u32,
    /// Rendering (offline) or playing live: picks the control block and the oversampling.
    offline: bool,
    tempo: f64,
    patch: Option<Patch>,
    v: usize,
    rate: u32,
    big_b: usize,
    step_free: f64,
    svf: SvfTables,
    section: Option<Section>,
    notes: Vec<Playing>,
    next_id: u64,
    counter: u64,
    global_lfo: Vec<Vec<Option<u32>>>,
    random: Random,
    previous: Option<f32>,
    ranks_dirty: bool,
    dry: Bus,
    fx: Bus,
}

impl Citrus {
    pub fn new(host_rate: u32) -> Citrus {
        Citrus {
            host_rate: host_rate.max(1),
            offline: false,
            tempo: 120.0,
            patch: None,
            v: 0,
            rate: host_rate.max(1),
            big_b: 8,
            step_free: 0.0,
            svf: SvfTables::new(1.0),
            section: None,
            notes: Vec::new(),
            next_id: 1,
            counter: COUNTER_START,
            global_lfo: Vec::new(),
            random: Random(0x1234_5678),
            previous: None,
            ranks_dirty: false,
            dry: [Vec::new(), Vec::new()],
            fx: [Vec::new(), Vec::new()],
        }
    }

    /// Sets the sample counter (voice-rate frames rendered so far), which free-running operator phases follow.
    pub fn set_counter(&mut self, frames: u64) {
        self.counter = frames;
    }

    pub fn patch(&self) -> Option<&Patch> {
        self.patch.as_ref()
    }

    pub fn state(&self) -> Option<&State> {
        self.patch.as_ref().map(|p| &p.st)
    }

    /// Loads a Sytrus state (event 213's bytes). False when it does not decode; the patch in use stays.
    pub fn load_state(&mut self, bytes: &[u8]) -> bool {
        let Some(st) = State::from_bytes(bytes) else { return false };
        self.set_state(st);
        true
    }

    pub fn set_state(&mut self, st: State) {
        self.notes.clear();
        self.patch = Some(Patch::new(st));
        self.setup();
    }

    /// Plays an edited patch (spec plugins/citrus.md §6, "Edits"). Notes keep playing on it, unless the edit changes
    /// which modules are used, how the operators run or the quality settings: then they stop, as when a state is
    /// loaded. The effects section is rebuilt only when an effects setting changes.
    pub fn edit(&mut self, p: Patch) {
        let Some(old) = &self.patch else { return self.set_state(p.st) };
        let keep = old.used == p.used && old.per_sample == p.per_sample && old.st.s[28..32] == p.st.s[28..32];
        let fx_same = fx_settings(&old.st) == fx_settings(&p.st);
        self.patch = Some(p);
        if !keep {
            self.notes.clear();
            self.setup();
            return;
        }
        if !fx_same {
            let st = &self.patch.as_ref().unwrap().st;
            self.section = Some(Section::new(st, self.host_rate, self.v, self.tempo as f32));
        }
        self.ranks_dirty = true;
    }

    pub fn set_offline(&mut self, offline: bool) {
        if offline != self.offline {
            self.offline = offline;
            self.setup();
        }
    }

    pub fn set_rate(&mut self, host_rate: u32) {
        if host_rate.max(1) != self.host_rate {
            self.host_rate = host_rate.max(1);
            self.setup();
        }
    }

    pub fn set_tempo(&mut self, bpm: f64) {
        if bpm > 0.0 && bpm != self.tempo {
            self.tempo = bpm;
            if let Some(p) = &self.patch {
                self.section = Some(Section::new(&p.st, self.host_rate, self.v, self.tempo as f32));
            }
        }
    }

    /// Rates, control block, tables and the effects section for the patch.
    fn setup(&mut self) {
        let Some(p) = &self.patch else { return };
        let st = &p.st;
        let (blk, over) = if self.offline { (st.s[29], st.s[31]) } else { (st.s[28], st.s[30]) };
        self.v = over.clamp(0, 5) as usize;
        self.rate = self.host_rate << self.v;
        let rate = f64::from(self.rate);
        let b = if blk == 0 { 96.0 } else { 8.0 };
        self.big_b = (round(b * rate / 44100.0 / 4.0) * 4).max(4) as usize;
        let seconds = ((self.big_b as f64 * b) / ((b * rate / 44100.0) * 96.0)) * 96.0 / 44100.0;
        self.step_free = seconds * 2.0;
        self.svf = SvfTables::new(s(44100.0 / rate));
        self.section = Some(Section::new(st, self.host_rate, self.v, self.tempo as f32));
        let ctx_step = |flags: u8| if flags & crate::curve::SYNCED != 0 { seconds * self.tempo / 60.0 } else { seconds * 2.0 };
        self.global_lfo = st
            .modules
            .iter()
            .map(|m| m.arts.iter().map(|a| (a.lfo.enabled && a.lfo.flags & GLOBAL != 0).then(|| LfoShape::new(&a.lfo, ctx_step(a.lfo.flags)).start)).collect())
            .collect();
    }

    fn ctx(&self) -> Option<Ctx<'_>> {
        let p = self.patch.as_ref()?;
        let k44 = 44100.0 / f64::from(self.rate);
        Some(Ctx {
            patch: p,
            rate: self.rate,
            k44,
            big_b: self.big_b,
            step_free: self.step_free,
            tempo: self.tempo,
            ramp_a: s(f64::from(s(f64::from(0.001f32) * f64::from(s(k44)))) * 10.0),
            ramp_o: s(f64::from(0.001f32) * f64::from(s(k44))),
            kf: s(k44),
            svf: &self.svf,
            counter: self.counter,
            knobs: [p.st.s[25], p.st.s[26]],
        })
    }

    /// Starts a note; returns its id.
    pub fn note_on(&mut self, n: NoteOn) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let Some(ctx) = self.ctx() else { return id };
        let st = &ctx.patch.st;
        let unis = unison(st);
        let o = st.s[8].max(1);
        let shared = st.switch(0) != 0 && (o > 1 || st.s[12] != 0);
        let note = Note { levels: n.levels, velocity: n.velocity, release: n.release, previous: self.previous };
        let mut random = Random(self.random.0);
        let mut voices = Vec::with_capacity(unis.len());
        for (j, u) in unis.into_iter().enumerate() {
            let leader = shared && j == 0;
            let follower = shared && j > 0;
            let mut v = Voice::new(&ctx, &note, u, id, leader, follower, &mut random, &self.global_lfo);
            // Global envelopes follow the first held voice's walks.
            if let Some(other) = self.notes.iter().flat_map(|p| p.voices.iter()).find(|v| !v.released) {
                v.adopt_globals(ctx.patch, other);
            }
            voices.push(v);
        }
        let sends = ctx.patch.sends;
        self.random = random;
        self.previous = Some(n.levels.pitch);
        if sends {
            if let Some(sec) = &mut self.section {
                sec.started_send = true;
            }
        }
        self.notes.push(Playing { id, shared, voices });
        let count: usize = self.notes.iter().map(|p| p.voices.len()).sum();
        if count > MAX_VOICES && self.notes.len() > 1 {
            self.notes.remove(0);
        }
        self.ranks_dirty = true;
        id
    }

    /// Releases a note.
    pub fn note_off(&mut self, id: u64) {
        let Some(p) = &self.patch else { return };
        for n in self.notes.iter_mut().filter(|n| n.id == id) {
            for v in n.voices.iter_mut() {
                v.release(p);
            }
        }
        self.ranks_dirty = true;
    }

    /// Releases every note.
    pub fn all_off(&mut self) {
        let ids: Vec<u64> = self.notes.iter().map(|n| n.id).collect();
        for id in ids {
            self.note_off(id);
        }
    }

    /// Stops every voice at once.
    pub fn panic(&mut self) {
        self.notes.clear();
    }

    /// The host's levels for a playing note (read at the next render call).
    pub fn set_levels(&mut self, id: u64, l: Levels) {
        for n in self.notes.iter_mut().filter(|n| n.id == id) {
            for v in n.voices.iter_mut() {
                v.set_levels(l);
            }
        }
    }

    /// A playing note's pitch (cents from key 60), for pitch bends.
    pub fn bend(&mut self, id: u64, pitch: f32) {
        for n in self.notes.iter_mut().filter(|n| n.id == id) {
            for v in n.voices.iter_mut() {
                let mut l = v.levels;
                l.pitch = pitch;
                v.set_levels(l);
            }
        }
    }

    pub fn voices(&self) -> usize {
        self.notes.iter().map(|n| n.voices.len()).sum()
    }

    fn ranks(&mut self) {
        let Some(p) = &self.patch else { return };
        if !p.has_steps {
            return;
        }
        let mut keys: Vec<f32> = self.notes.iter().flat_map(|n| n.voices.iter()).filter(|v| !v.released).map(|v| v.key).collect();
        keys.sort_by(f32::total_cmp);
        keys.dedup();
        if p.st.switch(3) != 0 {
            keys.reverse();
        }
        let count = keys.len() as i32;
        for v in self.notes.iter_mut().flat_map(|n| n.voices.iter_mut()) {
            let rank = keys.iter().position(|&k| k == v.key).map_or(-1, |r| r as i32);
            v.set_rank(p, rank, count);
        }
    }

    /// Renders one host block of `n` frames into `left` and `right` (overwritten).
    pub fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len());
        left[..n].fill(0.0);
        right[..n].fill(0.0);
        if self.patch.is_none() || n == 0 {
            return;
        }
        if self.ranks_dirty {
            self.ranks();
            self.ranks_dirty = false;
        }
        let vn = n << self.v;
        for b in self.dry.iter_mut().chain(self.fx.iter_mut()) {
            b.clear();
            b.resize(vn, 0.0);
        }
        let alive = !self.notes.is_empty();
        let mut dry = std::mem::take(&mut self.dry);
        let mut fx = std::mem::take(&mut self.fx);
        let mut notes = std::mem::take(&mut self.notes);
        {
            let ctx = self.ctx().unwrap();
            for note in notes.iter_mut().rev() {
                if note.shared {
                    Voice::render_group(&mut note.voices, &ctx, vn, &mut dry, &mut fx, 0);
                } else {
                    for v in note.voices.iter_mut().rev() {
                        v.render(&ctx, vn, &mut dry, &mut fx, 0);
                    }
                }
            }
        }
        let before = notes.iter().map(|n| n.voices.len()).sum::<usize>();
        for note in notes.iter_mut() {
            if note.shared {
                if note.voices.first().is_some_and(|v| v.done) {
                    note.voices.clear();
                }
            } else {
                note.voices.retain(|v| !v.done);
            }
        }
        notes.retain(|n| !n.voices.is_empty());
        if notes.iter().map(|n| n.voices.len()).sum::<usize>() != before {
            self.ranks_dirty = true;
        }
        self.notes = notes;
        if let Some(sec) = &mut self.section {
            sec.process(&mut dry, &mut fx, n, alive);
        }
        left[..n].copy_from_slice(&dry[0][..n]);
        right[..n].copy_from_slice(&dry[1][..n]);
        self.dry = dry;
        self.fx = fx;
        self.counter += vn as u64;
        // Global LFO counters.
        if let Some(p) = &self.patch {
            let seconds = self.step_free / 2.0;
            for (m, module) in p.st.modules.iter().enumerate() {
                for (a, art) in module.arts.iter().enumerate() {
                    if let Some(c) = self.global_lfo[m][a].as_mut() {
                        let step = if art.lfo.flags & crate::curve::SYNCED != 0 { seconds * self.tempo / 60.0 } else { seconds * 2.0 };
                        let dphi = LfoShape::new(&art.lfo, step).dphi;
                        *c = c.wrapping_add(round(vn as f64 / self.big_b as f64 * dphi as f64) as u32);
                    }
                }
            }
        }
    }
}

/// What the effects section is built from: the chorus, the send target, the delays, the reverb, the equaliser and the
/// DC removal.
fn fx_settings(st: &State) -> ([i32; 8], [crate::state::Delay; 3], crate::state::ReverbSettings, crate::state::EqSettings, u8) {
    let mut s = [0; 8];
    s[..7].copy_from_slice(&st.s[16..23]);
    s[7] = st.s[24];
    (s, st.delays, st.reverb, st.eq, st.switch(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preset::default_patch;

    fn note(c: &mut Citrus) {
        let levels = Levels { vol: 1.0, pan: 0.0, pitch: 0.0, mod_x: 0.0, mod_y: 0.0 };
        c.note_on(NoteOn { levels, velocity: 100.0 / 128.0, release: 0.5 });
    }

    fn peak(c: &mut Citrus) -> f32 {
        let (mut l, mut r) = (vec![0.0f32; 480], vec![0.0f32; 480]);
        c.render(&mut l, &mut r);
        l.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    /// An edit is heard on the playing note; one that uses another module stops it.
    #[test]
    fn edits_keep_notes_unless_the_modules_change() {
        let mut c = Citrus::new(48000);
        c.set_state(default_patch());
        note(&mut c);
        let loud = (0..10).map(|_| peak(&mut c)).fold(0.0, f32::max);
        let mut st = default_patch();
        st.main_volume = 32;
        c.edit(Patch::new(st.clone()));
        assert_eq!(c.voices(), 1);
        // The output gain glides to its new value.
        for _ in 0..40 {
            peak(&mut c);
        }
        let quiet = (0..10).map(|_| peak(&mut c)).fold(0.0, f32::max);
        assert!(quiet < loud * 0.5 && quiet > 0.0, "{quiet} {loud}");
        // Operator 2 to the output: another module is used.
        st.matrix[9 + 8] = 256;
        c.edit(Patch::new(st));
        assert_eq!(c.voices(), 0);
    }
}
