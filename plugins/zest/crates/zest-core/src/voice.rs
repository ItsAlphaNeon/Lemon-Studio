// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! One voice: its note handling state, rows, modulators and the call (`pages/voice.md`, `pages/modulation.md`).

use crate::dsp::{Lcg, Mwc};
use crate::envelope::Envelope;
use crate::filter::bite::Bite;
use crate::filter::multimode::Multimode;
use crate::filter::solver::SolverFilter;
use crate::filter::uhbie::Uhbie;
use crate::filter::{FilterCtx, FilterIo, FilterTables, MatrixSums};
use crate::lfo::{Clock, LfoState, SingleLfo};
use crate::hpf::{HpfBite, HpfCtx, HpfPre};
use crate::osc::dco::Dco;
use crate::osc::digital::Digital;
use crate::osc::dual::Dual;
use crate::osc::eco::Eco;
use crate::osc::triple::Triple;
use crate::osc::OscCtx;
use crate::settings::Settings;
use crate::tables::Tables;

pub const VOICES: usize = 16;
pub const MAX_ACTIONS: usize = 16;
pub const ROWS_LEN: usize = 448;

/// Row numbers of the sources.
pub mod row {
    pub const GATE: usize = 5;
    pub const VELOCITY: usize = 6;
    pub const PRESSURE: usize = 7;
    pub const KEY1: usize = 8;
    pub const KEY2: usize = 9;
    pub const ENV1: usize = 14;
    pub const ENV2: usize = 15;
    pub const LFO1: usize = 16;
    pub const LFO2: usize = 17;
    pub const RECTIFY: usize = 18;
    pub const INVERT: usize = 19;
    pub const QUANT: usize = 20;
    pub const LAG: usize = 21;
    pub const ADDER: usize = 22;
    pub const MULTIPLY: usize = 23;
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ActKind {
    On,
    Off,
    Kill,
    Ctrl,
}

#[derive(Clone, Copy, Debug)]
pub struct Action {
    pub kind: ActKind,
    pub time: u64,
    pub key: i32,
    pub vel: i32,
    pub retrigger: bool,
    pub glide: bool,
    pub member: i32,
}

impl Action {
    pub const EMPTY: Action = Action {
        kind: ActKind::Ctrl,
        time: 0,
        key: 0,
        vel: 0,
        retrigger: false,
        glide: false,
        member: 0,
    };
}

/// The analog spread: pots, stored per-voice values and their knobs. Groups: cutoff, glide, pulse width, envelope.
#[derive(Clone, Copy, Debug, Default)]
pub struct Spread {
    pub pots: [[f32; 8]; 4],
    pub stored: [[f32; 8]; 4],
    pub knobs: [f32; 4],
}

impl Spread {
    pub fn recompute(&mut self, g: usize) {
        let k = self.knobs[g] * 0.01;
        for v in 0..8 {
            self.stored[g][v] = ((k * k) * self.pots[g][v]) * 0.01;
        }
    }
}

/// The channel's state that voices touch as they run, in processing order.
pub struct Shared {
    pub rng: Mwc,
    pub random: Lcg,
    pub alt: f32,
    pub last: [f32; 2],
    pub last_voice: usize,
    pub current: usize,
    pub last_vel: i32,
    pub age_counter: u32,
    /// The shared row buffer.
    pub rows: [f32; ROWS_LEN],
    pub single: [SingleLfo; 2],
    pub spread: Spread,
    /// The microtuning table: the pitch of a key after transpose.
    pub key_table: [f32; 128],
    /// The channel's controllers and pressure as they stand after the span's events: what note actions read.
    pub ctrl: [f32; 4],
    pub press: f32,
}

impl Shared {
    #[inline]
    pub fn date(&mut self) -> u32 {
        self.age_counter = self.age_counter.wrapping_add(1);
        self.age_counter
    }
}

/// What a call needs from the frame.
#[derive(Clone, Copy)]
pub struct CallEnv<'a> {
    pub s: &'a Settings,
    pub ft: &'a FilterTables,
    pub tables: &'static Tables,
    pub fs: f32,
    pub dt: f32,
    pub clock: Clock,
    pub acc: i32,
    pub lag_l: f32,
    pub q_inv: f32,
}

/// Voice state grouped with the model states of a voice.
#[repr(align(64))]
pub struct Voice {
    pub index: usize,

    // kept by the note handling
    pub busy: bool,
    pub gated: bool,
    pub vnote: i32,
    pub age: u32,
    pub actions: [Action; MAX_ACTIONS],
    pub n_actions: usize,

    // the voice's state
    pub note: i32,
    pub pitch: [f32; 2],
    pub duo_key: [i32; 2],
    pub vel: f32,
    pub level: f32,
    pub dec: f32,
    pub pressure: f32,
    pub cur: [f32; 4],
    pub tgt: [f32; 4],
    pub tgt_press: f32,
    pub prog: [f32; 2],
    pub start: [f32; 2],
    pub v_alt: f32,
    pub v_rnd: f32,
    pub v_stack: f32,
    pub playing: bool,
    pub gate_row: bool,
    pub countdown: i32,
    pub used: bool,
    pub ghost: bool,
    pub tune: f32,
    pub rows: [f32; ROWS_LEN],
    pub row_last: [f32; 24],

    pub env: [Envelope; 2],
    pub env1_active: bool,
    pub lfo: [LfoState; 2],
    pub mod_state: [f32; 6],

    pub needs_clear: bool,
    /// The block (its chunk counter) in which the voice last stopped.
    pub stop_block: i64,
    pub ran: bool,

    pub triple: Box<Triple>,
    pub dual: Box<Dual>,
    pub dco: Box<Dco>,
    pub eco: Box<Eco>,
    pub digital: Box<Digital>,
    pub solver: Box<SolverFilter>,
    pub multimode: Box<Multimode>,
    pub bite: Box<Bite>,
    pub uhbie: Box<Uhbie>,
    pub hpf_pre: Box<HpfPre>,
    pub hpf_bite: Box<HpfBite>,

    pub mx: MatrixSums,
    pub a: [f32; 64],
    pub b: [f32; 64],
    pub out_l: [f32; 64],
    pub out_r: [f32; 64],
}

impl Voice {
    pub fn new(index: usize, seed: (u32, u32)) -> Box<Voice> {
        Box::new(Voice {
            index,
            busy: false,
            gated: false,
            vnote: 0,
            age: 0,
            actions: [Action::EMPTY; MAX_ACTIONS],
            n_actions: 0,
            note: 0,
            pitch: [69.0, 69.0],
            duo_key: [0, 0],
            vel: 0.0,
            level: 0.0,
            dec: 0.0,
            pressure: 0.0,
            cur: [0.0; 4],
            tgt: [0.0; 4],
            tgt_press: 0.0,
            prog: [1.0, 1.0],
            start: [60.0, 60.0],
            v_alt: 0.0,
            v_rnd: 0.0,
            v_stack: 0.0,
            playing: false,
            gate_row: false,
            countdown: 0,
            used: false,
            ghost: false,
            tune: 0.0,
            rows: [0.0; ROWS_LEN],
            row_last: [0.0; 24],
            env: [Envelope::default(); 2],
            env1_active: false,
            lfo: [LfoState::default(); 2],
            mod_state: [0.0; 6],
            needs_clear: false,
            stop_block: -1,
            ran: false,
            triple: Triple::new(seed),
            dual: Dual::new(seed),
            dco: Dco::new(seed),
            eco: Eco::new(seed),
            digital: Digital::new(seed),
            solver: SolverFilter::new(seed),
            multimode: Multimode::new(seed),
            bite: Bite::new(seed),
            uhbie: Uhbie::new(seed),
            hpf_pre: HpfPre::new(),
            hpf_bite: HpfBite::new(),
            mx: MatrixSums::zero(),
            a: [0.0; 64],
            b: [0.0; 64],
            out_l: [0.0; 64],
            out_r: [0.0; 64],
        })
    }

    #[inline]
    pub fn has_on_waiting(&self) -> bool {
        self.actions[..self.n_actions].iter().any(|a| a.kind == ActKind::On)
    }

    #[inline]
    pub fn has_off_waiting(&self) -> bool {
        self.actions[..self.n_actions].iter().any(|a| a.kind == ActKind::Off)
    }

    /// Put an action on the list, kept sorted by time, equal times in order of arrival, with the queue limits of the page.
    pub fn put(&mut self, a: Action) {
        if a.kind == ActKind::Ctrl && self.n_actions > 0 {
            let last = &mut self.actions[self.n_actions - 1];
            if last.kind == ActKind::Ctrl && last.time == a.time {
                *last = a;
                return;
            }
        }
        if self.n_actions == MAX_ACTIONS {
            if matches!(a.kind, ActKind::Off | ActKind::Kill) {
                self.actions[MAX_ACTIONS - 1] = a;
            }
            return;
        }
        let mut i = self.n_actions;
        while i > 0 && self.actions[i - 1].time > a.time {
            self.actions[i] = self.actions[i - 1];
            i -= 1;
        }
        self.actions[i] = a;
        self.n_actions += 1;
    }

    /// Remove any On action waiting, returning it.
    pub fn take_on(&mut self) -> Option<Action> {
        let mut found = None;
        let mut w = 0;
        for r in 0..self.n_actions {
            if self.actions[r].kind == ActKind::On {
                found = Some(self.actions[r]);
            } else {
                self.actions[w] = self.actions[r];
                w += 1;
            }
        }
        self.n_actions = w;
        found
    }

    /// Applies the actions due at or before `time`, in list order.
    pub fn apply_due(&mut self, time: u64, e: &CallEnv, sh: &mut Shared) {
        let mut done = 0;
        while done < self.n_actions && self.actions[done].time <= time {
            let a = self.actions[done];
            self.apply(&a, e, sh);
            done += 1;
        }
        if done > 0 {
            self.actions.copy_within(done..self.n_actions, 0);
            self.n_actions -= done;
        }
    }

    fn apply(&mut self, a: &Action, e: &CallEnv, sh: &mut Shared) {
        match a.kind {
            ActKind::Ctrl => {
                self.tgt = sh.ctrl;
                self.tgt_press = sh.press;
            }
            ActKind::On => self.apply_on(a, e, sh),
            ActKind::Off => {
                if self.playing {
                    self.env[0].note_off();
                    self.env[1].note_off();
                    self.dec = 0.002232143;
                    self.countdown = 111;
                    self.age = sh.date();
                }
            }
            ActKind::Kill => {
                if self.playing {
                    self.env[0].fast_release();
                    self.env[1].fast_release();
                    self.dec = 0.008928572;
                    self.countdown = 28;
                }
            }
        }
    }

    fn apply_on(&mut self, a: &Action, e: &CallEnv, sh: &mut Shared) {
        let s = e.s;
        self.age = sh.date();
        self.tgt = sh.ctrl;
        self.tgt_press = sh.press;
        self.pressure = sh.press;
        if a.retrigger || !self.playing {
            self.cur = sh.ctrl;
        }
        // a note starting a voice that had stopped: rows 14..23 start from 0
        if !self.playing {
            self.row_last[row::ENV1..].fill(0.0);
        }
        let mono_type = matches!(s.mode, 1 | 2 | 3);
        if a.member == 0 || mono_type {
            sh.alt = -sh.alt;
        }
        self.v_alt = sh.alt;
        let u = sh.random.unit();
        self.v_rnd = (u + u) - 1.0;
        let sg = s.stack.clamp(1, s.voices.max(1));
        self.v_stack = if sg > 1 { 1.0 - a.member as f32 * (2.0 / (sg - 1) as f32) } else { 1.0 };
        sh.last_vel = a.vel;

        // the note start
        let restart = a.retrigger || !self.playing;
        self.note = a.key;
        self.tune = 0.0;
        let p = pitch_of(a.key, s, self.index, &sh.key_table);
        self.pitch = [p, p];
        if restart {
            self.vel = a.vel as f32 / 127.0;
        }
        let f = 1.0 - s.glide_range * 0.01;
        for i in 0..2 {
            self.start[i] = f * (p - sh.last[i]) + sh.last[i];
            if a.glide {
                self.prog[i] = 0.0;
            } else {
                self.prog[i] = 1.0;
                self.start[i] = p;
                sh.last[i] = p;
            }
        }
        if restart {
            self.duo_key = [self.start[0] as i32, self.start[1] as i32];
        }
        self.countdown = 1024;
        self.gate_row = true;
        if restart {
            self.level = 1.0;
            self.dec = 0.0;
            self.env[0].note_on();
            self.env[1].note_on();
            for i in 0..2 {
                let restart_kind = s.lfo[i].restart;
                if restart_kind == 2 {
                    if sh.single[i].armed {
                        sh.single[i].armed = false;
                        self.lfo[i].reset_gate(&mut sh.rng);
                        sh.single[i].reset_voice = self.index as i32;
                    }
                } else {
                    self.lfo[i].note_on(restart_kind, &mut sh.rng);
                }
            }
            self.needs_clear = !self.used;
            self.used = true;
        }
        // a ghost with no call between is still playing and skips all of this
        if !self.playing {
            if s.trans_m > 0 {
                match s.vcf.model {
                    0 | 1 => self.solver.reset_dc(),
                    2 => self.multimode.reset_dc(),
                    3 => self.bite.reset_dc(),
                    _ => self.uhbie.reset_dc(),
                }
            }
            if s.trans_m > 1 {
                self.needs_clear = true;
                self.hpf_pre.flag_clear();
                self.hpf_bite.flag_clear();
            }
            self.wake_osc();
            if self.ghost || self.stop_block == e.clock.time {
                self.force_osc_init();
            }
        }
        self.ghost = false;
        self.playing = true;
        sh.last_voice = self.index;
        sh.current = if mono_type { 0 } else { self.index };
    }

    fn wake_osc(&mut self) {
        self.triple.wake();
        self.dual.wake();
        self.dco.wake();
        self.eco.wake();
        self.digital.wake();
    }

    fn force_osc_init(&mut self) {
        self.triple.force_init();
        self.dual.force_init();
        self.dco.force_init();
        self.eco.force_init();
        self.digital.force_init();
    }

    /// Steps for a ghost start (`voice.md`, Replaced notes).
    pub fn ghost_start(&mut self) {
        self.playing = true;
        self.ghost = true;
        if !self.used {
            self.needs_clear = true;
            self.used = true;
        }
    }

    /// The modulator part of a call: rows, envelopes, LFOs, modifiers, shared rows.
    pub fn modulators(&mut self, c: usize, n: usize, e: &CallEnv, sh: &mut Shared, duo: (i32, i32, bool)) {
        let s = e.s;
        let p0 = 4 * c;
        let k = 4 * n;
        for r in row::ENV1..24 {
            self.rows[r * 16 + p0..r * 16 + p0 + k].fill(self.row_last[r]);
        }
        // controllers, rows 1..4
        for i in 0..4 {
            let lo = if i == 1 { -1.0f32 } else { 0.0f32 };
            let d = self.tgt[i] - self.cur[i];
            let mut sl = 0.0f32;
            if d == 0.0 || d.abs() < 6.103888e-05 {
                self.cur[i] = self.tgt[i];
            } else {
                sl = ((d as f64 * 0.3f64) as f32) / (n as f32 * 4.0);
            }
            let moved = sl != 0.0;
            let at = (i + 1) * 16 + p0;
            if sl == 0.0 {
                self.rows[at..at + k].fill(self.cur[i]);
            } else {
                let mut cu = self.cur[i];
                for ch in 0..n {
                    if 1.0 < sl * 4.0 + cu {
                        cu = 1.0;
                        sl = 0.0;
                    }
                    if sl * 4.0 + cu < lo {
                        sl = 0.0;
                        cu = lo;
                    }
                    let v0 = cu + sl;
                    let v1 = (cu + sl) + sl;
                    let v2 = v1 + sl;
                    let v3 = v2 + sl;
                    let w = &mut self.rows[at + ch * 4..at + ch * 4 + 4];
                    w[0] = v0;
                    w[1] = v1;
                    w[2] = v2;
                    w[3] = v3;
                    cu = if v3 <= 1.0 { v3 } else { 1.0 };
                    if cu < lo {
                        cu = 1.0;
                    }
                }
                self.cur[i] = cu;
            }
            if moved && (self.tgt[i] - self.cur[i]).abs() < 6.103888e-05 {
                self.cur[i] = self.tgt[i];
            }
        }
        // Duo
        let f = 1.0 - s.glide_range * 0.01;
        if s.mode == 3 && self.playing {
            let (lo_key, hi_key, any) = duo;
            let mut moved = [false; 2];
            if any {
                if lo_key != self.duo_key[0] {
                    self.duo_key[0] = lo_key;
                    moved[0] = true;
                }
                if hi_key != self.duo_key[1] {
                    self.duo_key[1] = hi_key;
                    moved[1] = true;
                }
            }
            self.pitch[0] = pitch_of(self.duo_key[0], s, self.index, &sh.key_table);
            self.pitch[1] = pitch_of(self.duo_key[1], s, self.index, &sh.key_table);
            for i in 0..2 {
                if moved[i] {
                    self.prog[i] = 0.0;
                    self.start[i] = f * (self.pitch[i] - sh.last[i]) + sh.last[i];
                }
            }
        }
        // key rows and glide
        let b = self.cur[1];
        let range = (if b < 0.0 { s.bend_down } else { s.bend_up }) * 0.015625;
        let mono_type = matches!(s.mode, 1 | 2 | 3);
        let place = if mono_type { self.index } else { self.index % (s.stack.clamp(1, s.voices.max(1)) as usize) };
        let tn = s.vc[place.min(7)];
        let fine = s.fine * 0.01 * 0.015625 + self.tune * 0.015625;
        let mut k12 = [((self.pitch[0] + tn) - 64.0) * 0.015625, ((self.pitch[1] + tn) - 64.0) * 0.015625];
        let gv = sh.spread.stored[1][self.index & 7];
        let gs = gv * 0.9 + 1.0;
        let g = [s.glide * 0.01 * gs, (s.glide2 + s.glide) * 0.01 * gs];
        let mut sl12 = [0.0f32; 2];
        let kf = k as f32;
        for i in 0..2 {
            if self.prog[i] < 1.0 && self.playing {
                let gg = if g[i] >= 0.0 { g[i] } else { 0.0 };
                if gg == 0.0 {
                    self.prog[i] = 1.0;
                    sh.last[i] = self.pitch[i];
                    continue;
                }
                let mut rate = (e.dt * 4.0) / ((gg * gg) * gg);
                let st = ((self.start[i] + tn) - 64.0) * 0.015625;
                let t = k12[i];
                if s.glide_rate_mode && 0.0 < (t - st).abs() {
                    rate = rate / ((t - st).abs() * 5.3333335);
                }
                let d = t - st;
                let now = d * self.prog[i] + st;
                let mut pr = n as f32 * rate + self.prog[i];
                if pr > 1.0 {
                    pr = 1.0;
                }
                self.prog[i] = pr;
                sl12[i] = ((pr * d + st) - now) / kf;
                k12[i] = now;
                if self.index == sh.last_voice {
                    sh.last[i] = (self.pitch[i] - self.start[i]) * pr + self.start[i];
                }
            }
        }
        // pressure, gate and the rest, per step
        let mut ps = (self.tgt_press - self.pressure) / (k as f32);
        if ps.abs() > 0.0015 {
            ps = if ps <= 0.0 { -0.0015 } else { 0.0015 };
        }
        let w_row = self.rows[2 * 16 + p0];
        let vm = s.voice_map[self.index & 7] * 0.01;
        for j in 0..k {
            let p = p0 + j;
            self.pressure = self.pressure + ps;
            self.rows[row::PRESSURE * 16 + p] = self.pressure;
            self.rows[row::GATE * 16 + p] = self.level * self.level;
            self.level = self.level - self.dec;
            self.rows[row::KEY1 * 16 + p] = (range * w_row + k12[0]) + fine;
            self.rows[row::KEY2 * 16 + p] = (range * w_row + k12[1]) + fine;
            if self.level < 0.0 {
                self.level = 0.0;
                self.dec = 0.0;
            }
            self.rows[row::VELOCITY * 16 + p] = self.vel;
            k12[0] = k12[0] + sl12[0];
            k12[1] = k12[1] + sl12[1];
            self.rows[10 * 16 + p] = self.v_alt;
            self.rows[11 * 16 + p] = self.v_rnd;
            self.rows[12 * 16 + p] = self.v_stack;
            self.rows[13 * 16 + p] = vm;
        }
        let end = p0 + k - 1;
        for r in 1..14 {
            self.row_last[r] = self.rows[r * 16 + end];
        }
        if self.countdown < 1024 {
            let mut x = self.countdown - n as i32;
            if x < 1 {
                self.level = 0.0;
                self.dec = 0.0;
                self.gate_row = false;
                x = 0;
            }
            self.countdown = x;
        }
        // ENV1, ENV2
        let vel_v = self.rows[row::VELOCITY * 16 + p0];
        let key_v = self.rows[row::KEY1 * 16 + p0];
        let eslop = sh.spread.stored[3][self.index & 7];
        let now = e.clock.time;
        for i in 0..2 {
            let at = (row::ENV1 + i) * 16 + p0;
            let active = self.env[i].call(i, &s.env[i], &mut self.rows[at..at + k], vel_v, key_v, eslop, e.dt, now);
            if i == 0 {
                self.env1_active = active;
            }
            self.row_last[row::ENV1 + i] = self.rows[at + k - 1];
        }
        // LFO1, LFO2
        for i in 0..2 {
            let set = &s.lfo[i];
            let at = (row::LFO1 + i) * 16 + p0;
            let mut fm = self.rows[set.freq_src * 16 + end];
            let mut dm = self.rows[set.depth_src * 16 + end];
            if set.restart == 2 {
                let t = e.clock.time + c as i64;
                let sl = &mut sh.single[i];
                let me = self.index as i32;
                if set.freq_src == row::LFO1 + i {
                    fm = sl.last;
                }
                if set.depth_src == row::LFO1 + i {
                    dm = sl.last;
                }
                if t < sl.valid && sl.reset_voice != me {
                    self.rows[at..at + k].copy_from_slice(&sl.stored[p0..p0 + k]);
                } else {
                    if sl.reset_voice == me {
                        sl.state.reset_gate(&mut sh.rng);
                    }
                    sl.reset_voice = -1;
                    sl.state.call(set, fm, dm, n, e.fs, e.dt, &e.clock, &mut sh.rng, &mut self.rows[at..at + k]);
                    sl.stored[p0..p0 + k].copy_from_slice(&self.rows[at..at + k]);
                    sl.last = self.rows[at + k - 1];
                    sl.valid = t + n as i64;
                }
            } else {
                self.lfo[i].call(set, fm, dm, n, e.fs, e.dt, &e.clock, &mut sh.rng, &mut self.rows[at..at + k]);
            }
            self.row_last[row::LFO1 + i] = self.rows[at + k - 1];
        }
        self.modifiers(p0, k, s, e.lag_l, e.q_inv);
        // the shared row buffer
        for r in 0..24 {
            let at = r * 16 + p0;
            sh.rows[at..at + k].copy_from_slice(&self.rows[at..at + k]);
        }
    }

    #[inline(always)]
    fn modifiers(&mut self, p0: usize, k: usize, s: &Settings, l: f32, q_inv: f32) {
        // the modifiers' own rows hold the block fill until written
        #[inline(always)]
        fn read(rows: &[f32; ROWS_LEN], st: &[f32; 6], src: usize, own: usize, p: usize, zero_self: bool) -> f32 {
            if src > own {
                st[src - 18]
            } else if src == own {
                if zero_self {
                    0.0
                } else {
                    rows[own * 16 + p]
                }
            } else {
                rows[src * 16 + p]
            }
        }
        let q = s.quant;
        for j in 0..k {
            let p = p0 + j;
            let x = read(&self.rows, &self.mod_state, s.rect_src, row::RECTIFY, p, false);
            let v = x.abs();
            self.rows[row::RECTIFY * 16 + p] = v;
            self.mod_state[0] = v;
            let x = read(&self.rows, &self.mod_state, s.inv_src, row::INVERT, p, false);
            let v = -x;
            self.rows[row::INVERT * 16 + p] = v;
            self.mod_state[1] = v;
            let x = read(&self.rows, &self.mod_state, s.quant_src, row::QUANT, p, false);
            let v = (q * x).round() * q_inv;
            self.rows[row::QUANT * 16 + p] = v;
            self.mod_state[2] = v;
            let x = read(&self.rows, &self.mod_state, s.lag_src, row::LAG, p, false);
            let v = (1.0 - l) * x + l * self.mod_state[3];
            self.rows[row::LAG * 16 + p] = v;
            self.mod_state[3] = v;
            let x = read(&self.rows, &self.mod_state, s.add_src[0], row::ADDER, p, true);
            let y = read(&self.rows, &self.mod_state, s.add_src[1], row::ADDER, p, true);
            let v = x + y;
            self.rows[row::ADDER * 16 + p] = v;
            self.mod_state[4] = v;
            let x = read(&self.rows, &self.mod_state, s.mul_src[0], row::MULTIPLY, p, true);
            let y = read(&self.rows, &self.mod_state, s.mul_src[1], row::MULTIPLY, p, true);
            let v = x * y;
            self.rows[row::MULTIPLY * 16 + p] = v;
            self.mod_state[5] = v;
        }
        for i in 0..6 {
            self.row_last[row::RECTIFY + i] = self.mod_state[i];
        }
    }

    /// The matrix sums of a call (slot order; rows of the voice).
    fn matrix(&mut self, c: usize, n: usize, s: &Settings) {
        let mx = &mut self.mx;
        mx.any_pitch = false;
        mx.any_shape = false;
        mx.any_cut = false;
        mx.any_vol = false;
        mx.any_pan = false;
        mx.res = 0.0;
        if s.matrix_n == 0 {
            return;
        }
        let p0 = 4 * c;
        for slot in &s.matrix[..s.matrix_n] {
            let at = slot.src * 16 + p0;
            match slot.dst {
                1 => {
                    if !mx.any_pitch {
                        mx.pitch = [0.0; 16];
                    }
                    mx.any_pitch = true;
                    let a = slot.amt * 0.24;
                    for j in 0..16 {
                        mx.pitch[j] = mx.pitch[j] + self.rows[at + j] * a;
                    }
                }
                2 => {
                    if !mx.any_cut {
                        mx.cut = [0.0; 16];
                    }
                    mx.any_cut = true;
                    let a = slot.amt * 1.2;
                    for j in 0..4 * n {
                        mx.cut[j] = mx.cut[j] + self.rows[at + j] * a;
                    }
                }
                3 => mx.res = mx.res + self.rows[at] * slot.amt,
                4 => {
                    if !mx.any_shape {
                        mx.shape = [0.0; 16];
                    }
                    mx.any_shape = true;
                    let a = slot.amt * 0.005;
                    for j in 0..16 {
                        mx.shape[j] = mx.shape[j] + self.rows[at + j] * a;
                    }
                }
                5 => {
                    if !mx.any_vol {
                        mx.vol = [0.0; 16];
                    }
                    mx.any_vol = true;
                    let a = slot.amt * 0.01;
                    for j in 0..4 * n {
                        mx.vol[j] = mx.vol[j] + self.rows[at + j] * a;
                    }
                }
                6 => {
                    if !mx.any_pan {
                        mx.pan = [0.0; 16];
                    }
                    mx.any_pan = true;
                    for j in 0..4 * n {
                        mx.pan[j] = mx.pan[j] + self.rows[at + j] * slot.amt;
                    }
                }
                _ => {}
            }
        }
    }

    /// The part of a call before the filter: modulators, matrix, oscillator, HPF pre or BITE.
    pub fn call_pre(&mut self, c: usize, n: usize, e: &CallEnv, sh: &mut Shared, duo: (i32, i32, bool)) {
        self.modulators(c, n, e, sh, duo);
        self.matrix(c, n, e.s);
        let s = e.s;
        let ctx = OscCtx {
            s,
            tables: e.tables,
            rows: &sh.rows,
            mx_pitch: if self.mx.any_pitch { Some(&self.mx.pitch) } else { None },
            mx_shape: if self.mx.any_shape { Some(&self.mx.shape) } else { None },
            fs: e.fs,
            dt: e.dt,
            voice: self.index,
            c,
            n,
            time: e.clock.time,
            pw_slop: sh.spread.stored[2][self.index & 7],
            acc: e.acc,
        };
        match s.osc_model {
            0 => self.triple.run(&ctx, &mut self.a, &mut self.b),
            1 => self.dual.run(&ctx, &mut self.a, &mut self.b),
            2 => self.dco.run(&ctx, &mut self.a, &mut self.b),
            3 => self.eco.run(&ctx, &mut self.a, &mut self.b),
            _ => self.digital.run(&ctx, &mut self.a, &mut self.b),
        }
        if s.osc_model != 0 && s.hpf.model >= 2 {
            let hx = HpfCtx { s, ft: e.ft, acc: e.acc, rows: &self.rows, c, n };
            if s.hpf.model == 2 {
                self.hpf_bite.note_other_model();
                self.hpf_pre.run(&hx, &mut self.a, &mut self.b);
            } else {
                self.hpf_pre.note_other_model();
                self.hpf_bite.run(&hx, &mut self.a, &mut self.b);
            }
        } else if s.osc_model != 0 {
            self.hpf_pre.note_other_model();
            self.hpf_bite.note_other_model();
        }
    }

    /// Whether this model's filter can wait for a partner.
    #[inline]
    pub fn pairs(s: &Settings, acc: i32) -> bool {
        // the Uhbie's draft is still solved
        s.vcf.model == 4 || acc != 0
    }

    /// The filter of a call, alone.
    pub fn run_filter(&mut self, pos: usize, ctx: &FilterCtx, spread: f32) {
        let ns = 16 * ctx.n;
        let clear = core::mem::take(&mut self.needs_clear);
        let mut io = FilterIo {
            rows: &self.rows,
            mx: &self.mx,
            a: &self.a,
            b: &self.b,
            l: &mut self.out_l[pos..pos + ns],
            r: &mut self.out_r[pos..pos + ns],
            spread,
            clear,
        };
        match ctx.s.vcf.model {
            0 => self.solver.run(ctx, &mut io, false),
            1 => self.solver.run(ctx, &mut io, true),
            2 => self.multimode.run(ctx, &mut io),
            3 => self.bite.run(ctx, &mut io),
            _ => self.uhbie.run(ctx, &mut io),
        }
    }

    /// Two filters with a whole call each, in step.
    pub fn run_filter_pair(p: &mut Voice, q: &mut Voice, ctx: &FilterCtx, spread: &[f32; 8], avx: bool) {
        let ns = 16 * ctx.n;
        let (cp, cq) = (core::mem::take(&mut p.needs_clear), core::mem::take(&mut q.needs_clear));
        let (sp, sq) = (spread[p.index & 7], spread[q.index & 7]);
        let mut iop = FilterIo { rows: &p.rows, mx: &p.mx, a: &p.a, b: &p.b, l: &mut p.out_l[..ns], r: &mut p.out_r[..ns], spread: sp, clear: cp };
        let mut ioq = FilterIo { rows: &q.rows, mx: &q.mx, a: &q.a, b: &q.b, l: &mut q.out_l[..ns], r: &mut q.out_r[..ns], spread: sq, clear: cq };
        match ctx.s.vcf.model {
            2 => Multimode::run_two(&mut p.multimode, &mut q.multimode, ctx, &mut iop, &mut ioq),
            3 => Bite::run_two(&mut p.bite, &mut q.bite, ctx, &mut iop, &mut ioq),
            4 => Uhbie::run_two(&mut p.uhbie, &mut q.uhbie, ctx, &mut iop, &mut ioq),
            m => crate::filter::solver::run_pair(&mut p.solver, &mut q.solver, ctx, &mut iop, &mut ioq, m == 1, avx),
        }
    }

    /// After a call: the voice has finished when the VCA's source row went inactive.
    #[inline]
    pub fn finished(&self, s: &Settings) -> bool {
        if s.vca.env1 {
            !self.env1_active
        } else {
            !self.gate_row
        }
    }
}

const DETUNE: [f32; 16] = [0.0, 0.01, -0.01, 0.03, -0.032, 0.024, -0.022, 0.008, -0.006, 0.02, 0.012, -0.012, 0.032, -0.0322, 0.0242, -0.0222];

/// The pitch of a key on a voice (`modulation.md`, Key rows).
#[inline]
pub fn pitch_of(key: i32, s: &Settings, voice: usize, table: &[f32; 128]) -> f32 {
    let k = key + s.transpose;
    let mut x = k as f32;
    if s.mtun_on {
        x = table[k.clamp(0, 127) as usize];
    }
    if x > 160.0 {
        x = 160.0;
    }
    if x < -32.0 {
        x = -32.0;
    }
    if s.tuning_mode {
        x = x + DETUNE[voice & 15];
    }
    x + 12.0
}
