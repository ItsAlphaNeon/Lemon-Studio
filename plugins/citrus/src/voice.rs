// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! One voice of a note (one unison voice): its articulators, six operators, three filter modules and output gains.
//! It renders a stretch at a time (the rest of a control block, cut short where the host's render call ends) into
//! the dry bus and the effects bus.
//
// spec: plugins/sytrus.md ("Rates and control blocks", "Curves", "Maps", "An articulator's value", "Operators",
// "Filter modules", "Output", "Unison", "Portamento", "The end of a voice")

use std::f64::consts::PI;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::curve::{map_at, Lfo, Walk, GLOBAL, SYNCED};
use crate::filter::{FilterRun, Glide, Inputs, Ramp, Shaper, SvfTables};
use crate::num::{curve, flush, flush_le, lcg, round, s, TINY};
use crate::patch::{bipolar, Patch};
use crate::state::*;
use crate::wave::{self, N};

/// The noise seed every operator of every voice of every instance draws from (one per plugin library).
pub static NOISE_SEED: AtomicU32 = AtomicU32::new(0);

/// `(ln 2 / 12) / 100`: cents to the natural log of a frequency ratio.
pub const L2: f64 = std::f64::consts::LN_2 / 12.0 / 100.0;

/// What the voices of an instance share while they render.
pub struct Ctx<'a> {
    pub patch: &'a Patch,
    /// The voice rate.
    pub rate: u32,
    pub k44: f64,
    /// The control block in samples and in the curves' free unit (half seconds).
    pub big_b: usize,
    pub step_free: f64,
    pub tempo: f64,
    pub ramp_a: f32,
    pub ramp_o: f32,
    pub kf: f32,
    pub svf: &'a SvfTables,
    /// The plugin's sample counter at the voice's start (free-running phases).
    pub counter: u64,
    /// Mod X / Y knobs.
    pub knobs: [i32; 2],
}

impl Ctx<'_> {
    /// A curve's step: the block in half seconds, or in beats when tempo-synced.
    pub fn step_of(&self, flags: u8) -> f64 {
        if flags & SYNCED != 0 { self.step_free / 2.0 * self.tempo / 60.0 } else { self.step_free }
    }
}

/// A voice's final levels from the host: volume, pan (-1..1), pitch (cents from key 60), mod X / Y.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Levels {
    pub vol: f32,
    pub pan: f32,
    pub pitch: f32,
    pub mod_x: f32,
    pub mod_y: f32,
}

/// One unison voice's own values (spec "Unison").
#[derive(Debug, Clone, Copy)]
pub struct Uni {
    pub phase: f32,
    pub pan: f32,
    pub volume: f32,
    pub detune: f32,
    pub position: f64,
    pub variation: f32,
    /// Layer 1: an octave down.
    pub sub: bool,
}

impl Default for Uni {
    fn default() -> Self {
        Uni { phase: 0.0, pan: 0.5, volume: 1.0, detune: 1.0, position: 0.5, variation: 1.0 / 128.0, sub: false }
    }
}

/// What a note brings to its voices.
#[derive(Debug, Clone, Copy)]
pub struct Note {
    pub levels: Levels,
    /// Velocity 0..1 (velocity / 128).
    pub velocity: f32,
    /// Release velocity 0..1.
    pub release: f32,
    /// The previous note's pitch (cents), for portamento.
    pub previous: Option<f32>,
}

#[derive(Debug, Clone)]
struct ArtRun {
    env: Walk,
    lfo: Lfo,
    env_on: bool,
    lfo_on: bool,
    amount: f32,
    lfo_amount: f32,
    bip: bool,
    fixed: f32,
    value: f32,
    prev: f32,
    runs: bool,
}

#[derive(Debug, Clone, Default)]
struct Pluck {
    ring: Vec<f32>,
    pos: usize,
    damp: f32,
    keep: f32,
    tune: f32,
    whole: u64,
    frac: u32,
    acc: u32,
    prev: f64,
    cur: f64,
    last: f64,
    first: bool,
    damp_value: f32,
    inc: f32,
}

#[derive(Debug, Clone, Default)]
struct OpRun {
    used: bool,
    per_sample: bool,
    started: bool,
    /// Cycles before the first stretch, table entries after it.
    phase: f64,
    table: Option<Vec<f32>>,
    freq: f64,
    factor: f32,
    inc_cycles: f32,
    inc: f32,
    dphase: f32,
    depth: f32,
    depth_step: f32,
    vol: f32,
    vol_step: f32,
    ring: f32,
    mod_in: f32,
    noise_keep: f32,
    noise_scale: f32,
    dc: (f64, f64),
    smooth_n: usize,
    smooth_done: usize,
    smooth_c: f32,
    smooth_y: f32,
    pluck: Option<Pluck>,
    buf: Vec<f32>,
}

/// An output gain gliding in extended precision (spec "Output").
#[derive(Debug, Clone, Copy, Default)]
struct OutGain {
    g: [f32; 4],
}

pub struct Voice {
    pub tag: u64,
    pub levels: Levels,
    pub velocity: f32,
    pub release_velocity: f32,
    pub uni: Uni,
    pub key: f32,
    /// The first voice of a note whose unison voices share the filters, and the others.
    pub leader: bool,
    pub follower: bool,
    arts: Vec<Vec<ArtRun>>,
    ops: Vec<OpRun>,
    filters: Vec<Option<FilterRun>>,
    fbuf: Vec<Vec<f32>>,
    out: [OutGain; 9],
    block_left: usize,
    first_block: bool,
    first_stretch: bool,
    pub released: bool,
    pub finished: bool,
    pub done: bool,
    last_pitch: f32,
    mod_m: [f32; 2],
    pub rank: i32,
    pub count: i32,
}

/// The unison voices of a note (spec "Unison"): order `O` and the sub-octave layer.
pub fn unison(st: &State) -> Vec<Uni> {
    let o = st.s[8].max(1) as usize;
    let sub = st.s[12];
    let layers = if sub != 0 { 2 } else { 1 };
    let mode = st.switch(11).min(3) as usize;
    let shared = st.switch(0) != 0 && (o > 1 || sub != 0);
    let mut g = 1.0 / o as f32;
    if o > 1 {
        g = 1.0 - (s(((1.0 - f64::from(g)) * 3f64.ln()).exp() - 1.0)) * 0.5;
    }
    const D01: [f32; 9] = [0.877, 1.069, 0.929, 1.181, 1.031, 1.129, 0.887, 1.117, 0.977];
    const D3: [f32; 9] = [0.890_471_9, 1.076_426_3, 0.911_577_05, 1.102_535_8, 1.034_126_2, 1.166_861_2, 0.853_970_9, 1.206_272_6, 1.0];
    const V: [f64; 9] = [0.9877, 1.0069, 0.9929, 1.0181, 1.0031, 1.0129, 0.9887, 1.0117, 0.9977];
    let mut out = Vec::new();
    for l in 0..layers {
        let sl = sub as f32 / 128.0;
        let lg = if layers == 2 { if l == 0 { g * (1.0 - sl) } else { g * sl } } else { g };
        for j in 0..o {
            let mut u = Uni { volume: lg, sub: l == 1, ..Uni::default() };
            if o > 1 {
                let pos = j as f32 / (o - 1) as f32;
                let f = pos * 2.0 - 1.0;
                u.position = f64::from(pos);
                u.phase = (j as f32 / o as f32) * (st.s[13] as f32 / 128.0);
                u.pan = if shared { 0.5 } else { f * (1.0 / 256.0) * st.s[9] as f32 + 0.5 };
                let side = if mode >= 1 && o == 2 { lg } else { lg * 2.0 * (1.0 - f.abs()) };
                let mix = st.s[10] as f32 / 128.0;
                u.volume = lg * mix + side * (1.0 - mix);
                let mut sign = f.abs();
                if j % 2 == 1 {
                    sign = -sign;
                }
                if mode >= 1 && o % 2 == 1 && 2 * j > o {
                    sign = -sign;
                }
                let x = s11(st, mode);
                let d = match mode {
                    0 | 1 => D01[(j + l) % 9],
                    2 => 1.0,
                    _ => D3[(j + l) % 9],
                };
                u.detune = s(f64::from(s(f64::from(sign * d) * f64::from(x))).exp());
                if mode == 3 {
                    let ph = st.s[13] as f32 / 128.0;
                    let a = f64::from(ph * ph).powi(2);
                    let mut q = a * (f64::from(s(f64::from(u.detune) * 100000.0)) * a);
                    if l == 1 {
                        q *= 0.5;
                    }
                    let q = f64::from(s(q));
                    u.phase = s(q - q.floor());
                }
                u.variation = s(((f64::from(f.abs()) - 0.5) * f64::from(st.s[14]) * V[(j + l) % 9] * (1.0 / 128.0) + 1.0) * (1.0 / 128.0));
            }
            if l == 1 {
                u.detune *= 0.5;
            }
            out.push(u);
        }
    }
    out
}

/// The detune range `R`.
fn s11(st: &State, mode: usize) -> f32 {
    let x = f64::from(st.s[11] as f32 * (1.0 / 256.0));
    if mode == 0 {
        s(f64::from(s((x * 11f64.ln()).exp() - 1.0)) * 5.0 * L2)
    } else {
        s(f64::from(s((x * 41f64.ln()).exp() - 1.0)) * 1.875 * L2)
    }
}

/// The plugin's random generator for random maps.
pub struct Random(pub u32);

impl Random {
    pub fn next(&mut self) -> f64 {
        self.0 = lcg(self.0);
        f64::from(self.0) / 4_294_967_296.0
    }
}

impl Voice {
    /// A voice at note-on.
    #[allow(clippy::too_many_arguments)]
    pub fn new(ctx: &Ctx, note: &Note, uni: Uni, tag: u64, leader: bool, follower: bool, rnd: &mut Random, global_lfo: &[Vec<Option<u32>>]) -> Voice {
        let p = ctx.patch;
        let st = &p.st;
        let cents = f64::from(note.levels.pitch) + 100.0 * f64::from(st.main_pitch);
        let mut key = s(f64::from(s(cents * 0.01)) + 60.0).max(0.0);
        if uni.sub {
            key = (key - 12.0).max(0.0);
        }
        let key_cents = if st.switch(6) != 0 { note.previous.map(|pp| f64::from(pp) + 100.0 * f64::from(st.main_pitch)).unwrap_or(cents) } else { cents };
        let mut kmap = s(f64::from(s(key_cents * 0.01)) + 60.0).max(0.0);
        if uni.sub {
            kmap = (kmap - 12.0).max(0.0);
        }
        let random_on = st.s32(1);
        let lfo_amount = p.lfo_amount;
        let vel = note.velocity;
        let mut arts = Vec::with_capacity(MODULES);
        for (m, module) in st.modules.iter().enumerate() {
            let mut row = Vec::with_capacity(module.arts.len());
            for (a, art) in module.arts.iter().enumerate() {
                let bip = bipolar(m, a);
                let centre = if bip { 0.5 } else { 0.0 };
                let comb = |x: f32, y: f32| if bip { x + y } else { x * y };
                let mut fixed = map_at(&art.maps[MAP_KEY], f64::from(kmap) * 0.008_333_333_333_333_333, centre);
                fixed = comb(fixed, map_at(&art.maps[MAP_VEL], f64::from(vel), centre));
                let rmap = &art.maps[MAP_RANDOM];
                if random_on && !rmap.points.iter().all(|pt| pt.level == s(1.0 - centre)) {
                    fixed = comb(fixed, map_at(rmap, rnd.next(), centre));
                }
                fixed = comb(fixed, map_at(&art.maps[MAP_UNISON], uni.position, centre));
                let limit = !(m < OPERATORS && a == OP_PITCH);
                let mut env = Walk::new(&art.env, ctx.step_of(art.env.flags), centre, limit);
                let lfo_c = &art.lfo;
                let gphase = if lfo_c.flags & GLOBAL != 0 { global_lfo.get(m).and_then(|r| r.get(a)).copied().flatten() } else { None };
                let lfo = Lfo::new(lfo_c, ctx.step_of(lfo_c.flags), gphase);
                let cutoff = (OPERATORS..OPERATORS + FILTERS).contains(&m) && a == F_CUTOFF;
                let h1 = module.header[1] as f32 / 128.0;
                let amount = if cutoff { h1 } else { art.env_amount };
                let lamt = if cutoff { lfo_amount * h1 } else { lfo_amount };
                // Master knobs on volume envelopes, and the filters' cutoff and resonance envelopes.
                if art.env.enabled && art.env.flags & GLOBAL == 0 {
                    let is_vol = (m < OPERATORS && a == OP_VOLUME) || ((OPERATORS..9).contains(&m) && a == F_VOLUME) || (m == 9 && a == 1);
                    let is_cr = (OPERATORS..9).contains(&m) && (a == F_CUTOFF || a == F_RES);
                    let base = if is_vol { Some(0) } else if is_cr { Some(4) } else { None };
                    if let Some(b) = base {
                        let k = |i: usize| st.s[b + i] as f32;
                        let u = uni.variation;
                        env.scales[0] = env.scales[0] * k(0) * u;
                        env.scales[1] = env.scales[1] * k(1) * (0.015625 - u);
                        env.scales[2] += k(2) * (1.0 / 128.0);
                        env.scales[3] = env.scales[3] * k(3) * (1.0 / 128.0);
                        env.rescale(&art.env);
                    }
                }
                if art.env.enabled {
                    let opts = st.switch(9);
                    if opts & 1 != 0 {
                        env.scales[0] *= 2.0 - 2.0 * vel;
                        env.rescale(&art.env);
                    }
                    // Portamento: the operators' pitch envelopes start with the interval as their offset.
                    if m < OPERATORS && a == OP_PITCH && st.switch(4) != 0 && art.env.decay >= 0 && module.header[2] != 0 {
                        if let Some(prev) = note.previous {
                            let mut d = f64::from(prev) - f64::from(note.levels.pitch);
                            match st.switch(7) {
                                1 => {
                                    while d.abs() > 1250.0 {
                                        d -= 1200.0 * d.signum();
                                    }
                                }
                                k @ 2..=5 => {
                                    let lim = f64::from(6 - k as i32) * 100.0;
                                    d = d.clamp(-lim, lim);
                                }
                                _ => {}
                            }
                            if st.switch(5) != 0 {
                                env.scales[0] = s(f64::from(env.scales[0]) * (d.abs() * f64::from(module.header[2]) * 6.944_444_444_444_445e-7));
                            }
                            env.reset(&art.env, s(d * f64::from(s(1.0 / 2400.0))));
                        }
                    }
                    if opts & 2 != 0 && art.env.decay >= 0 {
                        let dt = (art.env.time_to(art.env.decay as usize) * f64::from(env.scales[0]) * f64::from(vel)) / env.step;
                        env.advance(&art.env, dt);
                    }
                }
                let env_on = art.env.enabled;
                let lfo_on = art.lfo.enabled;
                let mut v;
                if env_on || lfo_on {
                    v = if env_on { art.base + env.out * amount } else { art.base };
                    if lfo_on {
                        v = if bip { v + lfo.value * lamt } else { v * (lfo.value * lamt * 2.0 + 1.0) };
                    }
                    v = comb(v, fixed);
                } else {
                    v = fixed;
                }
                let mods = &p.mods[m][a];
                for (k, t) in mods.iter().enumerate() {
                    if let Some(t) = t {
                        let idx = (round(f64::from(ctx.knobs[k]) + f64::from(if k == 0 { note.levels.mod_x } else { note.levels.mod_y }) * 256.0)).clamp(0, 512) as usize;
                        v = comb(v, t[idx]);
                    }
                }
                if m < OPERATORS && (a == OP_MOD || a == OP_DAMP) {
                    v = v.clamp(0.0, 1.0);
                }
                let has_mod = mods.iter().any(Option::is_some);
                row.push(ArtRun {
                    env,
                    lfo,
                    env_on,
                    lfo_on,
                    amount,
                    lfo_amount: lamt,
                    bip,
                    fixed,
                    value: v,
                    prev: v,
                    runs: env_on || lfo_on || has_mod,
                });
            }
            arts.push(row);
        }
        let mut ops = Vec::with_capacity(OPERATORS);
        let soften = st.soften;
        let k44 = ctx.k44;
        for o in 0..OPERATORS {
            let module = &st.modules[o];
            let op = &module.op;
            let mut r = OpRun { used: p.used[o], per_sample: p.per_sample[o], ..OpRun::default() };
            if r.used {
                let a = &arts[o];
                r.freq = freq(op, ctx.rate, cents, uni.detune);
                r.depth = flush(a[OP_MOD].value.clamp(0.0, 1.0));
                r.ring = r.depth;
                r.vol = flush(a[OP_VOLUME].value * (module.header[1] as f32 * (1.0 / 256.0)));
                let mut ph = s(f64::from(s(f64::from(module.header[0]) / 256.0 + f64::from(uni.phase))) + f64::from(a[OP_PHASE].value));
                if op.flags[FREE_RUN] != 0 {
                    ph = s(f64::from(ph) + f64::from(s(ctx.counter as f64 * r.freq)));
                }
                let ph = f64::from(ph);
                r.phase = ph + 64.0 - round(ph) as f64;
                if op.noise != 0 && op.flags[PLUCKED] == 0 {
                    let nl = curve(op.noise as f32 / 128.0);
                    r.noise_keep = 1.0 - nl;
                    r.noise_scale = s(f64::from(nl) * (f64::from(ctx.rate) / 44100.0).sqrt());
                }
                // Attack smoothing.
                let flag = op.flags[SMOOTHING] != 0;
                if flag || soften > 0 {
                    let sm = round(f64::from(s(f64::from(s((f64::from(soften as f32 * (1.0 / 128.0)) * 101f64.ln()).exp() - 1.0)) * 10.0)));
                    let kk = s(96.0 / (k44 * k44));
                    let first = if sm < 1 { 0.0 } else { (((0.775 - vel) * sm as f32) * kk).max(0.0) };
                    let n = round(f64::from(first) + if flag { f64::from(kk) } else { 0.0 });
                    r.smooth_n = n.max(0) as usize;
                }
                r.buf = Vec::new();
            }
            ops.push(r);
        }
        let mut filters = Vec::with_capacity(FILTERS);
        for f in 0..FILTERS {
            let m = OPERATORS + f;
            let module = &st.modules[m];
            filters.push(p.used[m].then(|| {
                FilterRun::new(module.header, module.filter.kind, module.filter.oversample, module.filter.track, ctx.rate, arts[m][F_VOLUME].value)
            }));
        }
        let mut v = Voice {
            tag,
            levels: note.levels,
            velocity: vel,
            release_velocity: note.release,
            uni,
            key,
            leader,
            follower,
            arts,
            ops,
            filters,
            fbuf: vec![Vec::new(); FILTERS],
            out: [OutGain::default(); 9],
            block_left: 0,
            first_block: true,
            first_stretch: true,
            released: false,
            finished: false,
            done: false,
            last_pitch: note.levels.pitch,
            mod_m: [ctx.knobs[0] as f32, ctx.knobs[1] as f32],
            rank: 0,
            count: 0,
        };
        // Plucked strings are filled at note-on.
        for o in 0..OPERATORS {
            if v.ops[o].used && p.plucked(o) {
                v.start_pluck(ctx, o);
            }
        }
        v
    }

    fn start_pluck(&mut self, ctx: &Ctx, o: usize) {
        let p = ctx.patch;
        let op = &p.st.modules[o].op;
        let Some(table) = &p.tables[o] else { return };
        let r = &mut self.ops[o];
        let len = (round(88200.0 / f64::from(s(r.freq * f64::from(ctx.rate))))).max(2) as usize;
        let d = self.arts[o][OP_DAMP].value.clamp(0.0, 1.0);
        let damp = s(f64::from(d) * 0.99 + 0.01);
        let keep = 1.0 - damp;
        let tune = tune(len, keep);
        let mut q = (round(r.phase.rem_euclid(1.0) * 4_294_967_296.0) as u64 & 0xFFFF_FFFF) as u32;
        let step = round(f64::from(s(4.294_967_3e9 / f64::from(tune)))) as u32;
        let w = if op.noise != 0 { curve(op.noise as f32 / 128.0) } else { 0.0 };
        let mut seed = q;
        let mut ring = Vec::with_capacity(len);
        for _ in 0..len {
            let i = ((q >> 18) & 16383) as usize;
            let fr = (q & 0x3FFFF) as f32 * 3.814_697_3e-6;
            let mut y = table.t[i] + (table.t[i + 1] - table.t[i]) * fr;
            if w != 0.0 {
                seed = lcg(seed);
                let u = (seed as i32) as f32 * (1.0 / 2_147_483_648.0);
                y = y * (1.0 - w) + u * s(1.5 * f64::from(w));
            }
            ring.push(y);
            q = q.wrapping_add(step);
        }
        let mean = ring.iter().fold(0f32, |a, &x| a + x) / len as f32;
        for x in ring.iter_mut() {
            *x -= mean;
        }
        // The damping filter starts from the ring's last value, as if the ring had already gone round once.
        let last = f64::from(ring[len - 1]);
        r.pluck = Some(Pluck { ring, damp, keep, tune, first: true, damp_value: d, inc: -1.0, last, ..Pluck::default() });
    }

    /// Global envelopes (flag 0x02) take over the walks of a voice still held, keeping their own time scales.
    pub fn adopt_globals(&mut self, patch: &Patch, other: &Voice) {
        for (m, module) in patch.st.modules.iter().enumerate() {
            for (a, art) in module.arts.iter().enumerate() {
                if art.env.enabled && art.env.flags & GLOBAL != 0 {
                    let scales = self.arts[m][a].env.scales;
                    self.arts[m][a].env = other.arts[m][a].env.clone();
                    self.arts[m][a].env.scales = scales;
                }
            }
        }
    }

    /// The host changed the final levels.
    pub fn set_levels(&mut self, l: Levels) {
        self.levels = l;
    }

    /// Note-off: every envelope and LFO is released; switch 10 scales the releases by the release velocity.
    pub fn release(&mut self, patch: &Patch) {
        if self.released {
            return;
        }
        self.released = true;
        let rv = patch.st.switch(10) != 0;
        for (m, module) in patch.st.modules.iter().enumerate() {
            for (a, art) in module.arts.iter().enumerate() {
                let r = &mut self.arts[m][a];
                if rv {
                    let k = (1.0 - self.release_velocity) * 2.0;
                    r.env.scales[3] *= k;
                    r.lfo.walk.scales[3] *= k;
                }
                r.env.release(&art.env);
                r.lfo.walk.release(&art.lfo);
            }
        }
    }

    /// Step ranks (the voice's place among the keys held, and their number).
    pub fn set_rank(&mut self, patch: &Patch, rank: i32, count: i32) {
        self.rank = rank;
        self.count = count;
        for (m, module) in patch.st.modules.iter().enumerate() {
            for (a, art) in module.arts.iter().enumerate() {
                let r = &mut self.arts[m][a];
                for (w, c) in [(&mut r.env, &art.env), (&mut r.lfo.walk, &art.lfo)] {
                    w.rank = rank;
                    w.count = count;
                    w.keys_gone(c);
                }
            }
        }
    }

    /// The start of a control block: walks step, articulators take their values, the operators' ramps are set.
    fn begin_block(&mut self, ctx: &Ctx) {
        let p = ctx.patch;
        let st = &p.st;
        let big_b = ctx.big_b;
        let dt = if self.first_block { 0.0 } else { 1.0 };
        // Mod X / Y smoothing.
        let mut idx = [0usize; 2];
        for k in 0..2 {
            let host = if k == 0 { self.levels.mod_x } else { self.levels.mod_y };
            let knob = ctx.knobs[k] as f32;
            if st.s32(0) {
                let blk = ((big_b * 8) as f64 / ((8.0 * f64::from(ctx.rate) / 44100.0) * 96.0)) as f64;
                let c = s((0.01f64.ln() * blk / 96.0).exp());
                self.mod_m[k] = knob + (self.mod_m[k] - knob) * c;
            } else {
                self.mod_m[k] = knob;
            }
            idx[k] = round(f64::from(self.mod_m[k]) + f64::from(host) * 256.0).clamp(0, 512) as usize;
        }
        let mut all_done = true;
        for (m, module) in st.modules.iter().enumerate() {
            for (a, art) in module.arts.iter().enumerate() {
                let r = &mut self.arts[m][a];
                if !r.runs {
                    continue;
                }
                if r.env_on {
                    r.env.advance(&art.env, dt);
                    all_done &= r.env.finished();
                }
                if r.lfo_on {
                    r.lfo.advance(&art.lfo, dt);
                    all_done &= r.lfo.walk.finished();
                }
                let bip = r.bip;
                let comb = |x: f32, y: f32| if bip { x + y } else { x * y };
                let mut v = r.fixed;
                for (k, t) in p.mods[m][a].iter().enumerate() {
                    if let Some(t) = t {
                        v = comb(v, t[idx[k]]);
                    }
                }
                if r.env_on {
                    v = if bip { v + r.env.out * r.amount } else { (v * r.env.out) * r.amount };
                }
                if r.lfo_on {
                    v = if bip { v + r.lfo.value * r.lfo_amount } else { v * (r.lfo.value * r.lfo_amount * 2.0 + 1.0) };
                }
                r.value = v;
            }
        }
        if self.released && !self.first_block && all_done {
            self.finished = true;
        }
        let ctl = big_b as f32;
        let ra = ctx.ramp_a;
        let lim = |x: f32| x.clamp(-ra, ra);
        for o in 0..OPERATORS {
            if !self.ops[o].used {
                continue;
            }
            let module = &st.modules[o];
            let a = &mut self.arts[o];
            let dv = a[OP_PHASE].value;
            let dphase = s((f64::from(dv) - f64::from(a[OP_PHASE].prev)) / big_b as f64);
            a[OP_PHASE].prev = dv;
            let range = s(f64::from(module.header[2]) * 2.0 * L2);
            let factor = s(f64::from(s(f64::from(a[OP_PITCH].value) * f64::from(range))).exp());
            let mod_v = a[OP_MOD].value;
            let vol_v = a[OP_VOLUME].value;
            let damp_v = a[OP_DAMP].value.clamp(0.0, 1.0);
            let r = &mut self.ops[o];
            r.dphase = dphase;
            r.factor = factor;
            r.depth = flush(r.depth);
            r.vol = flush(r.vol);
            let dt = flush(mod_v.clamp(0.0, 1.0));
            r.depth_step = lim((dt - r.depth) / ctl);
            let vt = flush(vol_v * (module.header[1] as f32 * (1.0 / 256.0)));
            r.vol_step = lim((vt - r.vol) / ctl);
            if let Some(pl) = &mut r.pluck {
                pl.damp_value = damp_v;
            }
        }
        self.block_left = big_b;
        self.first_block = false;
    }

    /// Renders the operators of `n` samples of the current control block into their buffers.
    fn render_ops(&mut self, ctx: &Ctx, n: usize) {
        let p = ctx.patch;
        let st = &p.st;
        let nn = N as f64;
        if self.levels.pitch != self.last_pitch {
            self.last_pitch = self.levels.pitch;
            let cents = f64::from(self.levels.pitch) + 100.0 * f64::from(st.main_pitch);
            for o in 0..OPERATORS {
                if self.ops[o].used {
                    self.ops[o].freq = freq(&st.modules[o].op, ctx.rate, cents, self.uni.detune);
                }
            }
        }
        let global = st.s32(2);
        let f1 = self.ops[0].factor;
        for o in 0..OPERATORS {
            let r = &mut self.ops[o];
            if !r.used {
                continue;
            }
            let mut inc = s(r.freq * f64::from(r.factor));
            if global && o > 0 {
                inc = inc * f1;
            }
            r.inc_cycles = inc;
            r.inc = s(f64::from(s(f64::from(inc) + f64::from(r.dphase))) * nn).min((N / 2) as f32);
            if !r.started {
                r.started = true;
                let op = &st.modules[o].op;
                if op.flags[BAND_LIMIT] != 0 && op.flags[PLUCKED] == 0 {
                    if let Some(t) = &p.tables[o] {
                        r.table = wave::band_limited(t, s(f64::from(inc) * f64::from(ctx.rate)), ctx.rate, st.switch(2) != 0);
                    }
                }
                r.phase *= nn;
            }
            let c = if r.per_sample { 64.0 } else { 1.0 };
            r.phase += (c - round(r.phase * f64::from(s(1.0 / nn))) as f64) * nn;
            r.buf.clear();
            r.buf.resize(n, 0.0);
        }
        // Amounts of phase modulation and ring cells: destination d, source o.
        let mut seed = NOISE_SEED.load(Ordering::Relaxed);
        let ps: Vec<usize> = (0..OPERATORS).filter(|&o| self.ops[o].used && self.ops[o].per_sample).collect();
        if !ps.is_empty() {
            let mut amt = [[0f32; 6]; 6];
            let mut rg = [[0f32; 6]; 6];
            let mut has_pm = [false; 6];
            let mut has_ring = [false; 6];
            for &o in &ps {
                for d in 0..OPERATORS {
                    let gc = p.g[d][o];
                    if gc != 0.0 {
                        amt[o][d] = if d == o { s(f64::from(s(f64::from(gc) * 35.0)) / 128.0) * N as f32 } else { s(f64::from(gc) * 4.0) * N as f32 };
                        has_pm[o] = true;
                    }
                    let rc = p.ring[d][o];
                    if rc != 0.0 && d != o {
                        rg[o][d] = rc;
                        has_ring[o] = true;
                    }
                }
            }
            for j in 0..n {
                for &o in &ps {
                    let op = &st.modules[o].op;
                    let (y, depth_before) = {
                        let r = &mut self.ops[o];
                        let pos = f64::from(r.mod_in) * f64::from(r.depth) + r.phase;
                        r.phase += f64::from(r.inc);
                        r.mod_in = 0.0;
                        let mut y = if let Some(pl) = &mut r.pluck {
                            pluck_sample(pl, r.inc_cycles)
                        } else {
                            let t = r.table.as_deref().or(p.tables[o].as_ref().map(|t| t.t.as_slice())).unwrap();
                            let k = round(pos - 0.5);
                            let i = k.rem_euclid(N as i64) as usize;
                            let a = f64::from(t[i]);
                            a + (f64::from(t[i + 1]) - a) * (pos - k as f64)
                        };
                        if r.noise_scale != 0.0 || (op.noise != 0 && r.pluck.is_none()) {
                            seed = lcg(seed);
                            let sc = f64::from(r.noise_scale);
                            y = y * f64::from(r.noise_keep) + (f64::from(seed) * f64::from(s(sc * 4.656_613e-10)) - sc);
                        }
                        let dpt = r.depth;
                        y *= (1.0 + f64::from(r.ring)) - f64::from(dpt);
                        r.ring = dpt;
                        r.depth = r.depth + r.depth_step;
                        (y, dpt)
                    };
                    let _ = depth_before;
                    if has_ring[o] {
                        for d in 0..OPERATORS {
                            let rd = rg[o][d];
                            if rd != 0.0 {
                                let dr = &mut self.ops[d];
                                dr.ring = s((f64::from(rd) * y + f64::from(s(1.0 - f64::from(rd.abs())))) * f64::from(dr.ring));
                            }
                        }
                    }
                    let r = &mut self.ops[o];
                    let vol = f64::from(r.vol) + f64::from(r.vol_step);
                    let y = y * vol;
                    r.vol = s(vol);
                    r.buf[j] = s(y);
                    if has_pm[o] {
                        for d in 0..OPERATORS {
                            let a = amt[o][d];
                            if a != 0.0 {
                                let dr = &mut self.ops[d];
                                dr.mod_in = s(f64::from(a) * y + f64::from(dr.mod_in));
                            }
                        }
                    }
                }
            }
        }
        NOISE_SEED.store(seed, Ordering::Relaxed);
        // Vector operators.
        for o in 0..OPERATORS {
            let r = &mut self.ops[o];
            if !r.used || r.per_sample {
                continue;
            }
            let inc = r.inc;
            if r.vol == 0.0 && r.vol_step == 0.0 {
                r.phase += f64::from(s(n as f64 * f64::from(inc)));
                continue;
            }
            let t = r.table.as_deref().or(p.tables[o].as_ref().map(|t| t.t.as_slice())).unwrap();
            let pp = r.phase as f32;
            let mut lane = [pp, pp + inc, pp + 2.0 * inc, pp + 3.0 * inc];
            let s4 = 4.0 * inc;
            let mut vol = r.vol;
            let mask = N - 1;
            for j in 0..n {
                let l = j % 4;
                let sp = lane[l];
                lane[l] += s4;
                let i = sp.trunc();
                let f = sp - i;
                let i = (i as i64 as usize) & mask;
                vol += r.vol_step;
                r.buf[j] = (t[i] + (t[i + 1] - t[i]) * f) * vol;
            }
            r.vol = vol;
            r.phase += n as f64 * f64::from(inc);
        }
    }

    /// DC removal and attack smoothing on the operators' buffers.
    fn post_ops(&mut self, ctx: &Ctx, n: usize) {
        let st = &ctx.patch.st;
        let k1 = f64::from(s(ctx.k44 * 2.2835e-05));
        for o in 0..OPERATORS {
            let r = &mut self.ops[o];
            if !r.used {
                continue;
            }
            if st.modules[o].op.flags[DC_REMOVAL] != 0 {
                let (mut y, mut v) = r.dc;
                for x in r.buf[..n].iter_mut() {
                    let xi = f64::from(*x);
                    v += (xi - y) * k1;
                    y += v;
                    v *= 0.96;
                    *x = s(xi - y);
                }
                r.dc = (flush_le(y), flush_le(v));
            }
            if r.smooth_done < r.smooth_n {
                let inc = 1.0 / r.smooth_n as f32;
                let mut c = r.smooth_c;
                let mut y = r.smooth_y;
                let left = r.smooth_n - r.smooth_done;
                let m = left.min(n);
                for x in r.buf[..m].iter_mut() {
                    y += c * (*x - y);
                    *x = y;
                    c += inc;
                }
                r.smooth_y = y;
                r.smooth_c = r.smooth_c + inc * m as f32;
                r.smooth_done += m;
            }
        }
    }

    /// The operators' buffers scaled by this voice's unison volume, for a leader of shared filters.
    pub fn take_ops(&self) -> Vec<Vec<f32>> {
        self.ops.iter().map(|r| r.buf.iter().map(|&x| x * self.uni.volume).collect()).collect()
    }

    /// Filters and outputs of one stretch into the buses.
    fn post(&mut self, ctx: &Ctx, n: usize, vgain: f32, dry: &mut [Vec<f32>; 2], fx: &mut [Vec<f32>; 2], at: usize) {
        let p = ctx.patch;
        let st = &p.st;
        for f in 0..FILTERS {
            let m = OPERATORS + f;
            if self.filters[f].is_none() {
                continue;
            }
            let mut x = std::mem::take(&mut self.fbuf[f]);
            x.clear();
            x.resize(n, 0.0);
            for o in 0..OPERATORS {
                let gc = p.g[m][o];
                if gc != 0.0 && self.ops[o].used {
                    for (d, &v) in x.iter_mut().zip(&self.ops[o].buf[..n]) {
                        *d += v * gc;
                    }
                }
            }
            if f > 0 && st.modules[m - 1].header[9] != 0 && self.filters[f - 1].is_some() {
                let gn = crate::num::cell_gain(st.modules[m - 1].header[9]);
                let prev = std::mem::take(&mut self.fbuf[f - 1]);
                for (d, &v) in x.iter_mut().zip(&prev[..n]) {
                    *d += v * gn;
                }
                self.fbuf[f - 1] = prev;
            }
            let a = &self.arts[m];
            let h = &st.modules[m].header;
            let inp = Inputs {
                c: h[2] as f32 / 256.0 + a[F_CUTOFF].value,
                r: h[3] as f32 / 256.0 + a[F_RES].value,
                volume: a[F_VOLUME].value,
                low: a[F_LOW].value,
                band: a[F_BAND].value,
                high: a[F_HIGH].value,
                shaper_mix: a[F_SHAPER].value,
            };
            let sh: Option<&Shaper> = p.shapers[f].as_ref();
            let on = st.modules[m].filter.shaper_on;
            self.filters[f].as_mut().unwrap().run(&mut x, inp, ctx.kf, ctx.ramp_a, ctx.svf, sh, on);
            self.fbuf[f] = x;
        }
        // Output gains.
        let uv = if self.leader { 1.0 } else { self.uni.volume };
        let limit = s(f64::from(ctx.ramp_o) * f64::from(uv));
        let fx_pan = self.arts[9][0].value;
        let fx_vol = self.arts[9][1].value;
        for m in 0..9 {
            if !p.used[m] {
                continue;
            }
            let (go, gs) = (p.g[m][COL_OUT], p.g[m][COL_SEND]);
            if go == 0.0 && gs == 0.0 {
                continue;
            }
            let pan_v = self.arts[m][0].value;
            let cell_pan = st.cell(m, COL_PAN) as f32 / 512.0;
            let upan = if self.leader { 0.5 } else { self.uni.pan };
            let pan = ((self.levels.pan * 0.5 + upan + cell_pan + pan_v) * 2.0 - 1.0).clamp(-1.0, 1.0);
            let split = |pan: f32, level: f32| -> [f32; 2] {
                let ang = (1.0 + f64::from(pan)) * PI / 4.0;
                [s(ang.cos() * f64::from(level)), s(ang.sin() * f64::from(level))]
            };
            let mut target = [0f32; 4];
            if !self.finished {
                if go != 0.0 {
                    let [l, r] = split(pan, vgain * uv * go);
                    target[0] = l;
                    target[1] = r;
                }
                if gs != 0.0 {
                    let fpan = (((self.levels.pan * 0.5 + upan + cell_pan + pan_v) + st.s[15] as f32 / 256.0 + fx_pan) * 2.0 - 1.0).clamp(-1.0, 1.0);
                    let [l, r] = split(fpan, vgain * uv * gs * fx_vol);
                    target[2] = l;
                    target[3] = r;
                }
            }
            let buf: &[f32] = if m < OPERATORS { &self.ops[m].buf[..n] } else { &self.fbuf[m - OPERATORS][..n] };
            let gains = &mut self.out[m];
            if self.first_stretch {
                gains.g = target;
            }
            for k in 0..4 {
                let mut gl = Glide { value: gains.g[k] };
                let r = gl.ramp(target[k], n, limit);
                gains.g[k] = gl.value;
                if r.start == 0.0 && r.step == 0.0 {
                    continue;
                }
                let bus = if k < 2 { &mut dry[k][at..at + n] } else { &mut fx[k - 2][at..at + n] };
                add_gain(bus, buf, r);
            }
        }
        self.first_stretch = false;
        if self.finished && self.out.iter().all(|g| g.g.iter().all(|&x| x == 0.0)) {
            self.done = true;
        }
    }

    /// Renders `n` voice-rate frames at `at` into the buses (a voice that does not share filters).
    pub fn render(&mut self, ctx: &Ctx, n: usize, dry: &mut [Vec<f32>; 2], fx: &mut [Vec<f32>; 2], at: usize) {
        let vgain = s(f64::from(self.levels.vol) * f64::from(curve(ctx.patch.st.main_volume as f32 * (1.0 / 128.0))));
        let mut done = 0;
        while done < n && !self.done {
            if self.block_left == 0 {
                self.begin_block(ctx);
            }
            let len = self.block_left.min(n - done);
            self.render_ops(ctx, len);
            self.post_ops(ctx, len);
            self.post(ctx, len, vgain, dry, fx, at + done);
            self.block_left -= len;
            done += len;
        }
    }

    /// Shared filters: the voices of one note render their operators; the leader sums them and goes on alone.
    pub fn render_group(group: &mut [Voice], ctx: &Ctx, n: usize, dry: &mut [Vec<f32>; 2], fx: &mut [Vec<f32>; 2], at: usize) {
        let vgain = s(f64::from(group[0].levels.vol) * f64::from(curve(ctx.patch.st.main_volume as f32 * (1.0 / 128.0))));
        let mut done = 0;
        while done < n && !group[0].done {
            if group[0].block_left == 0 {
                for v in group.iter_mut() {
                    v.begin_block(ctx);
                }
            }
            let len = group[0].block_left.min(n - done);
            for v in group.iter_mut() {
                v.render_ops(ctx, len);
            }
            for o in 0..OPERATORS {
                if !group[0].ops[o].used {
                    continue;
                }
                let mut sum = vec![0f32; len];
                for v in group[1..].iter() {
                    for (d, &x) in sum.iter_mut().zip(&v.ops[o].buf[..len]) {
                        *d += x * v.uni.volume;
                    }
                }
                let lv = group[0].uni.volume;
                for (d, &x) in sum.iter_mut().zip(&group[0].ops[o].buf[..len]) {
                    *d += x * lv;
                }
                group[0].ops[o].buf[..len].copy_from_slice(&sum);
            }
            let (leader, rest) = group.split_first_mut().unwrap();
            leader.post_ops(ctx, len);
            leader.post(ctx, len, vgain, dry, fx, at + done);
            for v in rest.iter_mut() {
                v.block_left -= len;
            }
            leader.block_left -= len;
            done += len;
        }
    }
}

/// `single(bus + x * gain)` with the gain running in extended precision.
fn add_gain(bus: &mut [f32], x: &[f32], r: Ramp) {
    let mut g = f64::from(r.start);
    let step = f64::from(r.step);
    for (b, &v) in bus.iter_mut().zip(x) {
        *b = s(f64::from(*b) + f64::from(v) * g);
        g += step;
    }
}

/// An operator's frequency in cycles per sample (spec "Frequency and increment").
pub fn freq(op: &OpBlock, rate: u32, cents: f64, detune: f32) -> f64 {
    let ratio = f64::from(s(f64::from(s(f64::from(op.ratio as f32 * s(523.251_130_601_197 / f64::from(rate))))) / 20000.0));
    let offset = s(f64::from(op.offset) / (f64::from(rate) * 100.0));
    let f = s((cents * L2).exp() * ratio * f64::from(detune) + f64::from(offset)).max(0.0);
    f64::from(f)
}

fn tune(len: usize, keep: f32) -> f32 {
    let l = len as f64;
    let k = f64::from(keep);
    s(l * ((k * (1.0 / l).sin()) / (1.0 - k * (1.0 / l).cos()) + 1.0))
}

/// One output sample of a plucked string.
fn pluck_sample(pl: &mut Pluck, inc: f32) -> f64 {
    let damp = s(f64::from(pl.damp_value) * 0.99 + 0.01);
    if pl.inc != inc || damp != pl.damp {
        let keep = 1.0 - damp;
        if keep != pl.keep {
            pl.tune = tune(pl.ring.len(), keep);
        }
        pl.damp = damp;
        pl.keep = keep;
        pl.inc = inc;
        let per = f64::from(s(f64::from(pl.tune) * f64::from(inc.max(0.0))));
        pl.whole = per.trunc() as u64;
        pl.frac = ((per - per.trunc()) * 4_294_967_296.0) as u32;
    }
    let (a, carry) = pl.acc.overflowing_add(pl.frac);
    pl.acc = a;
    let mut count = pl.whole + u64::from(carry);
    if pl.first {
        count += 1;
        pl.first = false;
    }
    let len = pl.ring.len();
    let (damp, keep) = (f64::from(pl.damp), f64::from(pl.keep));
    for _ in 0..count {
        pl.prev = pl.cur;
        pl.cur = f64::from(pl.ring[pl.pos]);
        let nv = s(pl.cur * damp + pl.last * keep);
        pl.ring[pl.pos] = nv;
        pl.last = f64::from(nv);
        pl.pos = (pl.pos + 1) % len;
    }
    pl.prev + (pl.cur - pl.prev) * (f64::from(pl.acc) * (1.0 / 4_294_967_296.0))
}

#[allow(dead_code)]
const _: f32 = TINY;
