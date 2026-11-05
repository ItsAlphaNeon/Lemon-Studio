// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Zest's engine. The signatures here are what the plugin (`crates/zest-plugin`) and the renderer
//! (`src/bin/zest-render.rs`) call: keep them. Everything behind them comes from `pages/`.

pub mod arp;
pub mod dsp;
pub mod envelope;
pub mod filter;
pub mod fx;
pub mod h2p;
pub mod hpf;
pub mod lfo;
mod notes;
pub mod osc;
pub mod params;
pub mod settings;
pub mod simd;
pub mod slop;
pub mod tables;
pub mod voice;

use dsp::{Lcg, MWC_SEED};
use filter::{FilterCtx, FilterTables};
use lfo::{Clock, SingleLfo};
use params::{COUNT, PARAMS};
use settings::Settings;
use tables::Tables;
use voice::{CallEnv, Shared, Spread, Voice, ROWS_LEN, VOICES};

/// One timed input event; `time` is the sample offset inside the next [`Engine::process`] call.
#[derive(Clone, Copy, Debug)]
pub enum Event {
    NoteOn { time: u32, key: u8, velocity: f32 },
    NoteOff { time: u32, key: u8 },
    /// Pitch bend, -1..1
    PitchBend { time: u32, value: f32 },
    /// MIDI controller `cc` (1 = mod wheel, ...), 0..1
    Controller { time: u32, cc: u8, value: f32 },
    /// Channel pressure, 0..1
    Pressure { time: u32, value: f32 },
    /// All notes off and voices reset
    Panic { time: u32 },
    /// The voices of `key` tuned `semitones` from it (a host's per-note tuning: Lemon Studio's slide notes and
    /// portamento glides arrive so; not a Diva input)
    Tuning { time: u32, key: u8, semitones: f32 },
}

#[derive(Clone, Copy, Debug)]
pub struct Transport {
    pub tempo: f64,
    pub playing: bool,
    /// Song position in quarter notes at the start of the block
    pub beats: f64,
}

impl Default for Transport {
    fn default() -> Self {
        Transport { tempo: 120.0, playing: false, beats: 0.0 }
    }
}

const QUEUE: usize = 1024;

#[derive(Clone, Copy)]
struct Queued {
    at: u64,
    ev: Event,
}

/// The engine. Everything is sized at construction: nothing in `process` or `set_params` allocates.
pub struct Engine {
    fs: f32,
    dt: f32,
    ft: Box<FilterTables>,
    tables: &'static Tables,
    values: [f32; COUNT],
    settings: Box<Settings>,
    lag_l: f32,
    q_inv: f32,
    offline: bool,
    avx: bool,

    voices: Vec<Box<Voice>>,
    sh: Box<Shared>,
    ch: notes::Channel,

    queue: Box<[Queued]>,
    q_head: usize,
    q_len: usize,

    t: u64,
    h: u64,
    left: [(f32, f32); 16],
    left_at: usize,
    left_n: usize,
    prev_playing: Option<bool>,
    tempo: f64,
    beats: f64,

    instance: Lcg,
    order: [u8; VOICES],
    order_n: usize,
    bus_l: Box<[f32; 64]>,
    bus_r: Box<[f32; 64]>,
    post: hpf::HpfPost,
    fx: Box<fx::Effects>,
    arp: Box<arp::Arp>,
}

const VOICE_SEEDS: &str = include_str!("../data/voice-seeds.tsv");
const STARTUP_SPREAD: &str = include_str!("../data/startup-spread.tsv");

fn startup_spread() -> Spread {
    let (_, rows) = dsp::parse_tsv(STARTUP_SPREAD);
    let mut sp = Spread { knobs: [16.0, 24.0, 33.0, 26.0], ..Spread::default() };
    for g in 0..4 {
        for v in 0..8 {
            sp.pots[g][v] = rows[g][v] as f32;
        }
        sp.recompute(g);
    }
    sp
}

fn lag_coefficient(slew: f32, fs: f32) -> f32 {
    let w = 2.0f32.powf(slew * 0.1);
    if w - 1.0 == 0.0 {
        return 0.0;
    }
    let d = (w - 1.0) * 0.01 * fs * 0.25;
    ((0.01f32 as f64).ln() / d as f64).exp() as f32
}

impl Engine {
    /// `max_block`: the largest `left.len()` a call to `process` will have. No allocation after this.
    pub fn new(sample_rate: f32, _max_block: usize) -> Self {
        let fs = sample_rate;
        let values: [f32; COUNT] = core::array::from_fn(|i| PARAMS[i].default);
        let settings = Box::new(Settings::read(&values));
        let tables = Tables::get();
        let (_, seeds) = dsp::parse_tsv(VOICE_SEEDS);
        let voices = (0..VOICES).map(|v| Voice::new(v, (seeds[v][0] as u32, seeds[v][1] as u32))).collect();
        let mut eng = Engine {
            fs,
            dt: 1.0 / fs,
            ft: Box::new(FilterTables::new(fs)),
            tables,
            lag_l: lag_coefficient(settings.slew, fs),
            q_inv: 1.0 / settings.quant,
            values,
            settings,
            offline: false,
            avx: simd::has_avx(),
            voices,
            sh: Box::new(Shared {
                rng: MWC_SEED,
                random: Lcg::new(0, 0),
                alt: 1.0,
                last: [60.0, 60.0],
                last_voice: 0,
                current: 0,
                last_vel: 100,
                age_counter: 0,
                rows: [0.0; ROWS_LEN],
                single: [SingleLfo::default(); 2],
                spread: startup_spread(),
                key_table: core::array::from_fn(|i| i as f32),
                ctrl: [0.0; 4],
                press: 0.0,
            }),
            ch: notes::Channel::new(),
            queue: vec![Queued { at: 0, ev: Event::Panic { time: 0 } }; QUEUE].into_boxed_slice(),
            q_head: 0,
            q_len: 0,
            t: 0,
            h: 0,
            left: [(0.0, 0.0); 16],
            left_at: 0,
            left_n: 0,
            prev_playing: None,
            tempo: 120.0,
            beats: 0.0,
            instance: Lcg::new(2356542008, 3468471728),
            order: [0; VOICES],
            order_n: 0,
            bus_l: Box::new([0.0; 64]),
            bus_r: Box::new([0.0; 64]),
            post: hpf::HpfPost::new(fs),
            fx: Box::new(fx::Effects::new(fs)),
            arp: arp::Arp::new(),
        };
        eng.lfo_reset_run();
        eng.lfo_reset_run();
        eng
    }

    /// The samples the output lags its events (the event grid): the host compensates it.
    pub fn latency(&self) -> u32 {
        16
    }

    /// Rendering offline: the offline accuracy applies.
    pub fn set_offline(&mut self, offline: bool) {
        self.offline = offline;
    }

    /// All parameters, plain values in `params::PARAMS` order. Called before each `process`, so unchanged values must cost
    /// next to nothing.
    pub fn set_params(&mut self, values: &[f32; COUNT]) {
        if self.values == *values {
            return;
        }
        self.values = *values;
        *self.settings = Settings::read(values);
        self.lag_l = lag_coefficient(self.settings.slew, self.fs);
        self.q_inv = 1.0 / self.settings.quant;
        let knobs = self.settings.slop_knobs;
        let sp = &mut self.sh.spread;
        for g in 0..4 {
            if sp.knobs[g] != knobs[g] {
                sp.knobs[g] = knobs[g];
                sp.recompute(g);
            }
        }
    }

    /// What a preset's text does besides its parameters: its pots and re-rolls (see `slop`).
    pub fn load_preset_data(&mut self, h2p_text: &str) {
        self.apply_preset_slop(&slop::parse_preset(h2p_text));
    }

    /// The same, already parsed: no allocation, for the audio thread.
    pub fn apply_preset_slop(&mut self, p: &slop::PresetSlop) {
        if let Some(t) = &p.tuning {
            self.sh.key_table = *t;
        }
        let sp = &mut self.sh.spread;
        for g in 0..4 {
            for v in 0..8 {
                if p.have[g][v] {
                    sp.pots[g][v] = p.pots[g][v];
                }
            }
        }
        for g in 0..4 {
            if p.rnd[g] {
                for part in 0..2 {
                    let at = part * 4;
                    let mut x = [0.0f32; 4];
                    let (mut mx, mut mn, mut sum) = (-1.0f32, 1.0f32, 0.0f32);
                    for xi in x.iter_mut() {
                        let u = self.instance.unit();
                        *xi = u;
                        sum = sum + u;
                        if !(mx > u) {
                            mx = u;
                        }
                        if !(u > mn) {
                            mn = u;
                        }
                    }
                    let mean = sum / 4.0;
                    let (a, b) = ((mn - mean).abs(), (mx - mean).abs());
                    let mut r = if a > b { a } else { b };
                    if r == 0.0 {
                        r = 1.0;
                    }
                    for (i, xi) in x.iter().enumerate() {
                        sp.pots[g][at + i] = ((*xi - mean) * 100.0) / r;
                    }
                }
                sp.recompute(g);
            }
        }
    }

    /// Renders `left.len()` samples (== `right.len()`), applying `events` (sorted by time) at their offsets. An empty
    /// slice only takes in the transport.
    pub fn process(&mut self, events: &[Event], transport: &Transport, left: &mut [f32], right: &mut [f32]) {
        let _guard = dsp::DenormalGuard::new();
        let n = left.len();
        if self.prev_playing == Some(false) && transport.playing {
            self.lfo_reset_run();
            for v in self.voices.iter_mut() {
                v.digital.transport_start();
            }
        }
        self.prev_playing = Some(transport.playing);
        self.tempo = if transport.tempo > 0.0 { transport.tempo } else { 120.0 };
        self.beats = transport.beats;
        for ev in events {
            if self.q_len == QUEUE {
                break;
            }
            let at = ((self.h + event_time(ev) as u64 + 7) / 16 + 1) * 16;
            let w = (self.q_head + self.q_len) % QUEUE;
            self.queue[w] = Queued { at, ev: *ev };
            self.q_len += 1;
        }
        let mut out = 0;
        while self.left_n > 0 && out < n {
            let (a, b) = self.left[self.left_at];
            left[out] = a;
            right[out] = b;
            self.left_at += 1;
            self.left_n -= 1;
            out += 1;
        }
        while out < n {
            let block_end = (self.t / 64 + 1) * 64;
            let frontier = (self.h + n as u64).div_ceil(16) * 16;
            let end = block_end.min(frontier);
            let ns = (end - self.t) as usize;
            self.render_span(ns, transport);
            let take = ns.min(n - out);
            left[out..out + take].copy_from_slice(&self.bus_l[..take]);
            right[out..out + take].copy_from_slice(&self.bus_r[..take]);
            out += take;
            self.left_at = 0;
            self.left_n = ns - take;
            for i in 0..self.left_n {
                self.left[i] = (self.bus_l[take + i], self.bus_r[take + i]);
            }
        }
        self.h += n as u64;
    }

    /// One run of the LFO modules' reset: LFO2 then LFO1, each over the 16 voices in index order (`lfo.md`, Reset).
    fn lfo_reset_run(&mut self) {
        for i in [1, 0] {
            for v in self.voices.iter_mut() {
                v.lfo[i].module_reset(&mut self.sh.rng);
            }
            self.sh.single[i].state = self.voices[0].lfo[i];
        }
    }

    fn build_order(&mut self) {
        let cur = self.sh.current;
        let mut n = 0;
        for (i, v) in self.voices.iter().enumerate() {
            if v.playing && i != cur {
                self.order[n] = i as u8;
                n += 1;
            }
        }
        self.order[n] = cur as u8;
        n += 1;
        // idle voices with a note-on, the one whose note-on comes latest goes last
        let mut idle = [0u8; VOICES];
        let mut m = 0;
        let mut latest: Option<(u64, usize)> = None;
        for (i, v) in self.voices.iter().enumerate() {
            if !v.playing && i != cur {
                if let Some(t) = v.actions[..v.n_actions].iter().find(|a| a.kind == voice::ActKind::On).map(|a| a.time) {
                    idle[m] = i as u8;
                    m += 1;
                    if latest.is_none_or(|(lt, _)| t >= lt) {
                        latest = Some((t, i));
                    }
                }
            }
        }
        let last = latest.map(|(_, i)| i);
        for &i in &idle[..m] {
            if Some(i as usize) != last {
                self.order[n] = i;
                n += 1;
            }
        }
        if let Some(i) = last {
            self.order[n] = i as u8;
            n += 1;
        }
        self.order_n = n;
    }

    fn render_span(&mut self, ns: usize, transport: &Transport) {
        let t = self.t;
        let end = t + ns as u64;
        let c0 = ((t % 64) / 16) as usize;
        // 4. clock
        let beats = transport.beats + (t as f64 - self.h as f64) / self.fs as f64 * self.tempo / 60.0;
        let clock = Clock { tempo: self.tempo, beats, time: (t / 64) as i64 * 4 };
        // 1. take events
        {
            let settings: &Settings = &self.settings;
            let mut nh = notes::Notes { s: settings, ch: &mut self.ch, sh: &mut self.sh, voices: &mut self.voices };
            let arp = &mut self.arp;
            let play = |p: arp::Play, nh: &mut notes::Notes| match p {
                arp::Play::On(k, v) => nh.key_on(k, v, t),
                arp::Play::Off(k) => nh.key_off(k, t),
                arp::Play::ArmLfos => nh.arm_lfos(),
            };
            while self.q_len > 0 && self.queue[self.q_head].at < end {
                let q = self.queue[self.q_head];
                self.q_head = (self.q_head + 1) % QUEUE;
                self.q_len -= 1;
                if settings.arp_on {
                    match q.ev {
                        Event::NoteOn { key, velocity, .. } => {
                            arp.key_event(key, if velocity <= 0.0 { 0 } else { notes::velocity_of(velocity) });
                            continue;
                        }
                        Event::NoteOff { key, .. } => {
                            arp.key_event(key, 0);
                            continue;
                        }
                        Event::Controller { cc: 64, value, .. } => {
                            arp.pedal_event((value * 127.0).round() as i32);
                            continue;
                        }
                        Event::Controller { cc: 120 | 123, .. } | Event::Panic { .. } => {
                            arp.clear(&mut |p| play(p, &mut nh));
                        }
                        _ => {}
                    }
                }
                nh.take_event(&q.ev, q.at, t);
            }
            // the arpeggiator plays once per block, before any voice runs
            if t % 64 == 0 {
                if settings.arp_on {
                    arp.block(settings, beats, transport.playing, self.tempo, self.fs, &mut |p| play(p, &mut nh));
                } else {
                    arp.off(&mut |p| play(p, &mut nh));
                }
            }
        }
        self.sh.ctrl = self.ch.ctrl;
        self.sh.press = self.ch.press;
        // 2. held keys for Duo
        let duo = self.ch.duo_keys();
        // 3. snapshot: the settings stand for the whole of `process`
        let acc = self.settings.effective_accuracy(self.offline);
        // 5. processing order
        if t % 64 == 0 {
            self.build_order();
        }
        let mut list = [0u8; VOICES];
        let mut list_n = self.order_n;
        list[..list_n].copy_from_slice(&self.order[..list_n]);
        for (i, v) in self.voices.iter().enumerate() {
            if !list[..list_n].contains(&(i as u8)) && (v.playing || v.n_actions > 0 || i == self.sh.current) {
                list[list_n] = i as u8;
                list_n += 1;
            }
        }
        // 6. voices
        let settings: &Settings = &self.settings;
        let env = CallEnv { s: settings, ft: &self.ft, tables: self.tables, fs: self.fs, dt: self.dt, clock, acc, lag_l: self.lag_l, q_inv: self.q_inv };
        let fb_gate = settings.feedback_gate();
        let n_all = ns / 16;
        let mut waiting: Option<usize> = None;
        let span_current = self.sh.current;
        let pairable = Voice::pairs(settings, acc);
        for &vi in &list[..list_n] {
            let vi = vi as usize;
            self.voices[vi].ran = false;
            if !self.voices[vi].playing && self.voices[vi].n_actions == 0 && vi != self.sh.current {
                continue;
            }
            let mut pos = 0usize;
            while pos < ns {
                let sh = &mut *self.sh;
                let v = &mut *self.voices[vi];
                v.apply_due(t + pos as u64, &env, sh);
                let next = if v.n_actions > 0 { ((v.actions[0].time - t) as usize).min(ns) } else { ns };
                if next > pos {
                    let (c, n) = (c0 + pos / 16, (next - pos) / 16);
                    if v.playing {
                        if !v.ran {
                            v.ran = true;
                            if n != n_all {
                                v.out_l[..ns].fill(0.0);
                                v.out_r[..ns].fill(0.0);
                            }
                        }
                        v.call_pre(c, n, &env, sh, duo);
                        if v.finished(settings) {
                            v.playing = false;
                            v.age = sh.date();
                            v.stop_block = clock.time;
                        }
                        let stopped = !v.playing;
                        let ctx = FilterCtx { s: settings, ft: &self.ft, acc, c, n, fb_gate };
                        if pairable && n == n_all {
                            match waiting.take() {
                                None => waiting = Some(vi),
                                Some(w) => {
                                    let (a, b) = two_mut(&mut self.voices, w, vi);
                                    Voice::run_filter_pair(a, b, &ctx, &sh.spread.stored[0], self.avx);
                                }
                            }
                        } else {
                            v.run_filter(pos, &ctx, sh.spread.stored[0][vi & 7]);
                            // stopped with an action of this span still to come, as the span's current voice: those chunks
                            // count twice for the modulators (`index.md`, Rendering a span, step 6)
                            if stopped && next < ns && vi == span_current {
                                v.modulators(c, n, &env, sh, duo);
                            }
                        }
                    } else if vi == sh.current {
                        v.modulators(c, n, &env, sh, duo);
                    }
                }
                pos = next;
            }
            let v = &mut *self.voices[vi];
            if !v.playing && v.n_actions == 0 {
                v.busy = false;
            }
        }
        if let Some(w) = waiting {
            let ctx = FilterCtx { s: settings, ft: &self.ft, acc, c: c0, n: n_all, fb_gate };
            self.voices[w].run_filter(0, &ctx, self.sh.spread.stored[0][w & 7]);
        }
        // 7. the sum, in the list's order
        let (bl, br) = (&mut self.bus_l[..ns], &mut self.bus_r[..ns]);
        bl.fill(0.0);
        br.fill(0.0);
        for &vi in &list[..list_n] {
            let v = &*self.voices[vi as usize];
            if v.ran {
                for (b, x) in bl.iter_mut().zip(&v.out_l[..ns]) {
                    *b = *b + *x;
                }
                for (b, x) in br.iter_mut().zip(&v.out_r[..ns]) {
                    *b = *b + *x;
                }
            }
        }
        // 8. HPF post, 9. effects
        if settings.osc_model != 0 && settings.hpf.model == 1 {
            self.post.process(settings.hpf.post_freq, bl, br);
        }
        self.fx.process(settings, t, ns, clock.tempo, clock.beats, &self.sh.rows, bl, br);
        // 10. output gain
        let g = settings.output * 0.01;
        for x in bl.iter_mut() {
            *x = *x * g;
        }
        for x in br.iter_mut() {
            *x = *x * g;
        }
        // 11.
        self.t = end;
    }

    /// Everything silent, voices and effects cleared: the startup state, keeping the parameters, the pots, the knobs and the
    /// instance generator.
    pub fn reset(&mut self) {
        let (_, seeds) = dsp::parse_tsv(VOICE_SEEDS);
        self.voices = (0..VOICES).map(|v| Voice::new(v, (seeds[v][0] as u32, seeds[v][1] as u32))).collect();
        let spread = self.sh.spread;
        let last_vel = self.sh.last_vel;
        let key_table = self.sh.key_table;
        *self.sh = Shared {
            rng: MWC_SEED,
            random: Lcg::new(0, 0),
            alt: 1.0,
            last: [60.0, 60.0],
            last_voice: 0,
            current: 0,
            last_vel,
            age_counter: 0,
            rows: [0.0; ROWS_LEN],
            single: [SingleLfo::default(); 2],
            spread,
            key_table,
            ctrl: [0.0; 4],
            press: 0.0,
        };
        self.arp = arp::Arp::new();
        self.lfo_reset_run();
        self.lfo_reset_run();
        self.ch = notes::Channel::new();
        self.q_head = 0;
        self.q_len = 0;
        self.t = 0;
        self.h = 0;
        self.left_at = 0;
        self.left_n = 0;
        self.prev_playing = None;
        self.order_n = 0;
        self.post = hpf::HpfPost::new(self.fs);
        *self.fx = fx::Effects::new(self.fs);
    }
}

#[inline]
fn event_time(ev: &Event) -> u32 {
    match *ev {
        Event::NoteOn { time, .. }
        | Event::NoteOff { time, .. }
        | Event::PitchBend { time, .. }
        | Event::Controller { time, .. }
        | Event::Pressure { time, .. }
        | Event::Panic { time }
        | Event::Tuning { time, .. } => time,
    }
}

/// Two different voices at once.
fn two_mut(v: &mut [Box<Voice>], a: usize, b: usize) -> (&mut Voice, &mut Voice) {
    if a < b {
        let (x, y) = v.split_at_mut(b);
        (&mut x[a], &mut y[0])
    } else {
        let (x, y) = v.split_at_mut(a);
        (&mut y[0], &mut x[b])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use params::id;

    fn engine(set: &[(usize, f32)]) -> (Engine, [f32; COUNT]) {
        let mut e = Engine::new(48000.0, 512);
        let mut v: [f32; COUNT] = core::array::from_fn(|i| PARAMS[i].default);
        for &(k, x) in set {
            v[k] = x;
        }
        e.set_params(&v);
        (e, v)
    }

    fn run(e: &mut Engine, events: &[Event], samples: usize) {
        let mut l = vec![0.0; samples];
        let mut r = vec![0.0; samples];
        e.process(events, &Transport::default(), &mut l, &mut r);
    }

    fn on(key: u8) -> Event {
        Event::NoteOn { time: 0, key, velocity: 0.8 }
    }

    fn off(key: u8) -> Event {
        Event::NoteOff { time: 0, key }
    }

    #[test]
    fn lag_coefficient_page_values() {
        for (slew, l) in [(0.0, 0.0), (10.0, 0.96235061), (20.0, 0.98728931), (50.0, 0.99876285), (75.0, 0.99978685), (100.0, 0.99996251)] {
            assert!((lag_coefficient(slew, 48000.0) - l).abs() < 1e-6, "slew {slew}");
        }
    }

    #[test]
    fn event_grid_page_table() {
        let (mut e, _) = engine(&[]);
        for (time, at) in [(0u32, 16u64), (1, 16), (8, 16), (9, 32), (16, 32), (24, 32), (25, 48)] {
            e.q_len = 0;
            e.q_head = 0;
            e.process(&[Event::NoteOn { time, key: 60, velocity: 0.5 }], &Transport::default(), &mut [], &mut []);
            assert_eq!(e.queue[0].at, at, "time {time}");
        }
    }

    #[test]
    fn host_block_100_spans() {
        let (mut e, _) = engine(&[]);
        run(&mut e, &[], 100);
        assert_eq!((e.t, e.h, e.left_n), (112, 100, 12));
        run(&mut e, &[], 100);
        assert_eq!((e.t, e.h, e.left_n), (208, 200, 8));
    }

    #[test]
    fn first_notes_walk_the_voices_upward() {
        let (mut e, _) = engine(&[]);
        run(&mut e, &[on(60), on(62), on(64)], 64);
        run(&mut e, &[], 64);
        assert!(e.voices[0].playing && e.voices[1].playing && e.voices[2].playing && !e.voices[3].playing);
        assert_eq!((e.voices[0].vnote, e.voices[1].vnote, e.voices[2].vnote), (60, 62, 64));
        assert_eq!(e.sh.current, 2);
    }

    #[test]
    fn all_playing_the_nearest_note_gives_its_voice() {
        let (mut e, _) = engine(&[(id::VCC_VOICES, 0.0)]); // 2 voices
        run(&mut e, &[on(40), on(70)], 128);
        run(&mut e, &[on(68)], 128);
        assert_eq!(e.voices[1].vnote, 68);
        assert_eq!(e.voices[0].vnote, 40);
    }

    #[test]
    fn poly2_reuses_the_voice_just_released() {
        let (mut e, _) = engine(&[(id::VCC_MODE, 4.0)]);
        run(&mut e, &[on(60)], 128);
        run(&mut e, &[off(60)], 128);
        run(&mut e, &[on(62)], 128);
        assert_eq!(e.voices[0].vnote, 62);
        run(&mut e, &[on(64)], 128);
        assert_eq!(e.voices[1].vnote, 64);
    }

    #[test]
    fn two_mono_note_ons_in_one_block_start_one_note() {
        let (mut e, _) = engine(&[(id::VCC_MODE, 1.0)]);
        run(&mut e, &[on(60), on(63)], 128);
        assert_eq!(e.voices[0].note, 63);
        assert!(!e.voices[1].playing);
    }

    #[test]
    fn a_tuning_moves_the_key_row() {
        let (mut e, _) = engine(&[(id::VCC_GLIDE, 0.0)]);
        run(&mut e, &[on(60)], 128);
        let before = e.voices[0].rows[voice::row::KEY1 * 16 + 15];
        run(&mut e, &[Event::Tuning { time: 0, key: 60, semitones: 2.0 }], 128);
        let after = e.voices[0].rows[voice::row::KEY1 * 16 + 15];
        assert!((after - before - 2.0 / 64.0).abs() < 1e-6);
    }

    #[test]
    fn a_burst_of_bends_cannot_push_out_a_note_off() {
        let (mut e, _) = engine(&[]);
        run(&mut e, &[on(60)], 128);
        let mut evs: Vec<Event> = (0..40).map(|i| Event::PitchBend { time: 0, value: i as f32 / 40.0 }).collect();
        evs.push(off(60));
        run(&mut e, &evs, 64);
        assert!(!e.voices[0].gated);
        assert!(e.voices[0].env[0].stage == 3);
    }

    #[test]
    fn duo_rows_follow_lowest_and_highest() {
        let (mut e, _) = engine(&[(id::VCC_MODE, 3.0), (id::VCC_GLIDE, 0.0), (id::VCC_TRANSPOSE, 0.0)]);
        run(&mut e, &[on(60), on(72)], 256);
        let (r8, r9) = (e.voices[0].rows[voice::row::KEY1 * 16 + 15], e.voices[0].rows[voice::row::KEY2 * 16 + 15]);
        assert!((r8 - ((60.0 + 12.0 - 64.0) / 64.0)).abs() < 1e-5, "{r8}");
        assert!((r9 - ((72.0 + 12.0 - 64.0) / 64.0)).abs() < 1e-5, "{r9}");
    }

    #[test]
    fn a_mode_change_with_keys_down_releases_on_key_up() {
        let (mut e, mut v) = engine(&[]);
        run(&mut e, &[on(60), on(64)], 128);
        v[id::VCC_MODE] = 1.0;
        e.set_params(&v);
        run(&mut e, &[off(64)], 128);
        run(&mut e, &[off(60)], 128);
        assert!(!e.voices[0].gated && !e.voices[1].gated);
    }

    #[test]
    fn a_voice_ending_in_mid_glide_leaves_last_where_it_stopped() {
        let (mut e, _) = engine(&[(id::VCC_GLIDE, 100.0), (id::VCA1_VCA, 0.0)]);
        run(&mut e, &[on(40)], 128);
        run(&mut e, &[off(40)], 64);
        run(&mut e, &[on(80)], 64);
        run(&mut e, &[off(80)], 64);
        for _ in 0..40 {
            run(&mut e, &[], 64);
        }
        assert!(!e.voices[1].playing);
        let l = e.sh.last[0];
        assert!(l > 40.0 && l < 80.0, "{l}");
        let keep = l;
        run(&mut e, &[], 640);
        assert_eq!(e.sh.last[0], keep);
    }

    #[test]
    fn instance_generator_first_re_roll() {
        let (mut e, _) = engine(&[]);
        let p = slop::PresetSlop { rnd: [true, false, false, false], ..Default::default() };
        e.apply_preset_slop(&p);
        let pots = e.sh.spread.pots[0];
        for (a, b) in pots[..4].iter().zip([100.0, -18.123484, -53.027763, -28.848753]) {
            assert!((*a - b).abs() < 1e-4, "{a} {b}");
        }
    }

    #[test]
    fn preset_pots_change_stored_values_only_with_their_knob() {
        let (mut e, mut v) = engine(&[]);
        let old = e.sh.spread.stored[3];
        let mut p = slop::PresetSlop::default();
        p.pots[3] = [50.0; 8];
        p.have[3] = [true; 8];
        e.apply_preset_slop(&p);
        assert_eq!(e.sh.spread.stored[3], old);
        v[id::OPT_ENVRATESLOP] = 50.0;
        e.set_params(&v);
        assert!((e.sh.spread.stored[3][0] - 0.125).abs() < 1e-6);
    }

    #[test]
    fn startup_preset_chunk_holds_the_startup_pots() {
        // only where the preset library is present (bench/presets is not in the repository)
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../bench/presets");
        let Ok(text) = std::fs::read(format!("{root}/HS Bass Nine.h2p")) else { return };
        let text = String::from_utf8_lossy(&text);
        let p = slop::parse_preset(&text);
        assert!(p.have[0].iter().all(|h| *h));
        let v = h2p::load(&text, &core::array::from_fn(|i| PARAMS[i].default)).unwrap();
        assert!(v[id::VCC_MODE] >= 0.0);
    }

    #[test]
    fn no_events_stays_silent() {
        let (mut e, _) = engine(&[]);
        let mut l = vec![1.0; 512];
        let mut r = vec![1.0; 512];
        e.process(&[], &Transport::default(), &mut l, &mut r);
        assert!(l.iter().chain(r.iter()).all(|x| *x == 0.0));
    }

    #[test]
    fn reset_is_the_startup_state() {
        let (mut e, _) = engine(&[]);
        let mut a = (vec![0.0; 2048], vec![0.0; 2048]);
        e.process(&[on(60)], &Transport::default(), &mut a.0, &mut a.1);
        e.reset();
        let mut b = (vec![0.0; 2048], vec![0.0; 2048]);
        e.process(&[on(60)], &Transport::default(), &mut b.0, &mut b.1);
        assert_eq!(a, b);
    }

    #[test]
    fn pairing_gives_the_samples_of_plain_calls() {
        // two voices in lockstep against the same two run alone
        let render = |avx: bool| {
            let (mut e, _) = engine(&[(id::VCF1_RESONANCE, 60.0)]);
            e.avx = avx;
            let (mut l, mut r) = (vec![0.0; 4096], vec![0.0; 4096]);
            e.process(&[on(48), on(60)], &Transport::default(), &mut l, &mut r);
            (l, r)
        };
        if simd::has_avx() {
            assert_eq!(render(true), render(false));
        }
    }
}
