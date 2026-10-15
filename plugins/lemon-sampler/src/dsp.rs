// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Lemon Sampler voices: pitch, loop, declicking, envelopes and LFOs of the five targets, the channel filter, and the
//! realtime stretch methods. The played sample itself is built in `fx`.
//!
// spec: plugins/lemon-sampler.md (§5 Sound), audio/sampler-playback.md (pitch, velocity, resampling),
// spec: audio/channel-envelopes.md (envelope laws), audio/sampler.md (the resampler, the loop, the end of a voice),
// spec: audio/declick.md (opening and closing fades), audio/channel-filter.md, audio/voice-levels.md

use std::f64::consts::PI;
use std::sync::Arc;

use os_dsp::resample::{self, Quality};
use os_dsp::stretch::Stream;
use os_engine::{envelope, laws, Sample};

/// Time stretch to `ratio` × the length, the pitch moved by the factor `shift` (spec §5.6: Signalsmith Stretch).
pub fn stretch(s: &Sample, ratio: f64, shift: f64) -> Sample {
    let frames = (s.frames() as f64 * ratio).round().max(1.0) as usize;
    let (left, right) = os_dsp::stretch::whole(&s.left, &s.right, s.rate, frames, shift);
    Sample { rate: s.rate, left, right }
}

// ---------------------------------------------------------------------------------------------------------------
// Voices (spec §5.1 - §5.3)

/// One LFO (spec §5.2).
#[derive(Clone, Copy, Debug, Default)]
pub struct Lfo {
    /// 0 sine, 1 triangle, 2 pulse.
    pub shape: u8,
    pub delay: f64,
    pub attack: f64,
    pub amount: f64,
    pub hz: f64,
    pub global: bool,
}

impl Lfo {
    pub fn active(&self) -> bool {
        self.amount.abs() > 1e-4
    }

    fn wave(&self, phase: f64) -> f64 {
        let ph = phase.fract();
        match self.shape {
            0 => (2.0 * PI * ph).sin(),
            1 => 1.0 - 4.0 * ((ph + 0.25).fract() - 0.5).abs(),
            _ => {
                if ph < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
        }
    }

    /// Value −amount..amount at voice time `t` with the given phase.
    fn value(&self, t: f64, phase: f64) -> f64 {
        if !self.active() || t < self.delay {
            return 0.0;
        }
        let fade = if self.attack > 0.0 { ((t - self.delay) / self.attack).min(1.0) } else { 1.0 };
        self.amount * fade * self.wave(phase)
    }
}

/// An envelope target: the envelope (when on) with its amount, and its LFO.
#[derive(Clone, Copy, Debug, Default)]
pub struct Target {
    pub env: Option<envelope::Params>,
    /// Envelope amount (−1..1; the Volume target ignores it).
    pub amount: f64,
    pub lfo: Lfo,
}

pub const PAN: usize = 0;
pub const VOL: usize = 1;
pub const MODX: usize = 2;
pub const MODY: usize = 3;
pub const PITCH: usize = 4;

/// Settings read once per block.
#[derive(Clone, Copy, Debug)]
pub struct Block {
    pub root: i32,
    /// Fine tune plus pitch bend, in cents.
    pub cents: f64,
    /// Resample and the realtime stretch methods: the stretch ratio (MUL) and TIME's target length in seconds (which
    /// overrides the ratio); the realtime methods also the PITCH shift in cents (spec §5.6).
    pub ts_cents: f64,
    pub ts_ratio: f64,
    pub ts_time: Option<f64>,
    /// A realtime stretch method: voices stretch while they play.
    pub realtime: bool,
    pub targets: [Target; 5],
    /// Filter type (index into `params::FILTERS`), MOD X / MOD Y knob positions.
    pub f_type: u8,
    pub f_cut: f32,
    pub f_res: f32,
    pub start_offset: f64,
    /// "Use loop points": voices play the played sample's loop when it has one (the sampler flag of value 8).
    pub loop_on: bool,
    pub declick: i32,
    pub zone: (i32, i32),
    pub add_key: bool,
}

impl Default for Block {
    fn default() -> Self {
        Block {
            root: 60,
            cents: 0.0,
            ts_cents: 0.0,
            ts_ratio: 1.0,
            ts_time: None,
            realtime: false,
            targets: [Target::default(); 5],
            f_type: 0,
            f_cut: 1.0,
            f_res: 0.0,
            start_offset: 0.0,
            loop_on: false,
            declick: 0,
            zone: (0, crate::params::KEY_MAX),
            add_key: false,
        }
    }
}

/// A declick mode's fades (spec: audio/declick.md, the table of modes): the opening's length (ms; step removal in
/// modes 1 and 2, a gain fade otherwise), the closing's length (ms; the sweep in mode 1, a gain fade otherwise), the
/// fades' tension, and whether the voice bleeds (plays on past a clip's trimmed end under the closing).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Declick {
    pub open_ms: f64,
    pub step_removal: bool,
    pub close_ms: f64,
    pub sweep: bool,
    pub tension: f32,
    pub bleed: bool,
}

pub fn declick(mode: i32) -> Declick {
    let d = |open_ms, close_ms, tension, bleed| Declick { open_ms, step_removal: false, close_ms, sweep: false, tension, bleed };
    match mode {
        1 => Declick { step_removal: true, sweep: true, ..d(4.0, 10.0, 0.0, false) },
        2 => Declick { step_removal: true, ..d(4.0, 20.0, 0.0, true) },
        3 => d(20.0, 20.0, 0.0, true),
        4 => d(100.0, 100.0, 0.0, true),
        5 => d(200.0, 200.0, 5.0, true),
        // Not offered in the settings, valid as stored data. 7 and up name no preset in FL; they play as mode 0 here.
        6 => d(0.0, 2.0, 0.0, true),
        _ => d(0.0, 10.0, 0.0, false),
    }
}

/// Samples in a fade of `ms` at `rate`: `Round(ms * 0.001 * rate)`, at least 1.
fn fade_len(ms: f64, rate: f64) -> u32 {
    ((ms * 0.001 * rate).round() as u32).max(1)
}

/// A gain fade's value at table index `j` (1..=n) of a fade of `n` samples: a raised cosine, bent by the tension.
/// Closing: `j = i + 1`; opening: `j = n - i` (spec: audio/declick.md "Gain fade").
fn fade_gain(j: u32, n: u32, tension: f32) -> f32 {
    let scale = (std::f64::consts::PI / f64::from(n + 1)) as f32;
    let g = ((j as f32 * scale).cos() + 1.0) * 0.5;
    if tension == 0.0 {
        return g;
    }
    let (k4, k8) = (1.0 / tension.abs(), (tension.abs() + 1.0).ln());
    if tension > 0.0 {
        1.0 - (((1.0 - g) * k8).exp() - 1.0) * k4
    } else {
        ((g * k8).exp() - 1.0) * k4
    }
}

/// The closing of a voice: its declick mode's fade (or sweep), `n` samples long, from the sample it starts on.
#[derive(Clone, Copy, Debug)]
struct Close {
    /// Samples done.
    i: u32,
    n: u32,
    tension: f32,
    /// Mode 1's sweep: the corner's growth per sample and the length of the last millisecond's cosine.
    sweep: Option<(f64, u32)>,
    /// The sweep's low-pass memory per side.
    lp: [f32; 2],
}

impl Close {
    fn new(d: &Declick, rate: f64) -> Close {
        let n = fade_len(d.close_ms, rate);
        let sweep = d.sweep.then(|| (22050f64.ln() / (2f64.ln() * f64::from(n)) * 2f64.ln(), n.min(((rate * 0.001).round() as u32).max(1))));
        Close { i: 0, n, tension: d.tension, sweep, lp: [0.0; 2] }
    }

    /// Ends every voice quickly (Stop, all notes off): mode 0's 10 ms fade.
    fn quick(rate: f64) -> Close {
        Close::new(&declick(0), rate)
    }

    /// The next closing sample of a stereo frame, or `None` once the closing has run through.
    fn apply(&mut self, l: f32, r: f32, rate: f64) -> Option<(f32, f32)> {
        if self.i >= self.n {
            return None;
        }
        let i = self.i;
        self.i += 1;
        Some(match self.sweep {
            // A one-pole high-pass whose corner rises from 1 Hz to 22050 Hz, then a cosine over the last millisecond
            // (spec: audio/declick.md "Sweep").
            Some((k, m)) => {
                let c = ((2.0 / rate) * (f64::from(i + 1) * k).exp()).min(1.0) as f32;
                let w = if i < self.n - m {
                    1.0
                } else {
                    (((f64::from(i + 1 - (self.n - m)) * (1.0 / f64::from(m + 1)) * std::f64::consts::PI).cos() + 1.0) * 0.5) as f32
                };
                self.lp[0] += c * (l - self.lp[0]);
                self.lp[1] += c * (r - self.lp[1]);
                ((l - self.lp[0]) * w, (r - self.lp[1]) * w)
            }
            None => {
                let g = fade_gain(i + 1, self.n, self.tension);
                (l * g, r * g)
            }
        })
    }
}

/// Frames between modulation updates: the voices' "tick" (**Lemon Studio**: a fixed 32 frames, where FL uses the
/// sequencer's tick; filter coefficients, gain and pan targets change at these points).
const CTL: usize = 32;

/// A voice's pan gains for a pan position (−1..1, clamped): the constant-power split at √2, so a centred pan is 1 on
/// both sides. The host applies the channel's own pan split on the instrument's output, so with either pan centred
/// the product is the split of their sum (spec: audio/voice-levels.md "Final levels", audio/conventions.md
/// "Pan split").
pub fn pan_gains(pan: f64) -> (f32, f32) {
    let a = (pan.clamp(-1.0, 1.0) + 1.0) * PI / 4.0;
    ((std::f64::consts::SQRT_2 * a.cos()) as f32, (std::f64::consts::SQRT_2 * a.sin()) as f32)
}

#[derive(Clone, Copy)]
struct Voice {
    key: u8,
    /// Started by a host note: its end is reported back (spec: audio/voices.md §4). Editor previews are not.
    host: bool,
    /// Host note id (per-note modulation), if the host gave one.
    id: Option<i32>,
    vel: f32,
    pos: f64,
    /// Key rate times the sample / host rate ratio.
    key_step: f64,
    /// Host tuning of this key's voices, semitones (portamento glides; spec: audio/channel-voices.md).
    tune: f64,
    /// The note's own pan (−1..1) and its filter offsets in knob units, from the host per note (CLAP note expression
    /// PAN, per-note modulation of the filter knobs; spec: audio/voice-levels.md).
    note_pan: f64,
    mod_x: f32,
    mod_y: f32,
    age: u64,
    off: Option<u64>,
    /// Opening length in samples: only a voice that starts inside its sample has one (0 = none).
    open: u32,
    /// The closing, once it has started (note end, choke, the end of the sample).
    close: Option<Close>,
    phase: [f64; 5],
    filt: Filter,
    /// Gain at the end of the last control span (ramps start here).
    gain: f32,
    pan: (f32, f32),
    /// Its stretcher in `Engine::streams` (realtime stretch methods).
    rt: Option<u16>,
    /// Frames left of the crossfade from the plain reading to the stretcher (a voice that starts stretching while it
    /// plays).
    xf: u32,
}

/// Output frames a realtime stretch voice makes at a time.
const RT_BLOCK: usize = 128;

/// A realtime voice has nothing to stretch while stretching would make the sample, at the voice's pitch, no more than
/// this much longer or shorter (seconds): it then plays the sample by resampling at its pitch. TIME is kept in 1/192 of
/// a beat, so a sample fitted to its own duration can be a little off it (a few ms; half a unit is 2.6 ms at 60 BPM).
/// **(OS)**
const NEAR: f64 = 0.003;

/// Frames of the crossfade from the plain reading to the stretcher when a voice starts stretching while it plays.
/// **(OS)**
const SWITCH: u32 = 256;

/// Nothing to stretch: `frames` of sample last about as long read at `step` (its pitch) as at `speed` (the stretch).
fn nothing_to_stretch(frames: f64, step: f64, speed: f64, rate: f64) -> bool {
    (frames / step - frames / speed).abs() <= NEAR * rate
}

/// The stretcher of a voice in a realtime stretch method and the reader that feeds it (spec §5.6). A voice starts it
/// only once it has something to stretch (`render_voice`).
struct Rt {
    st: Stream,
    busy: bool,
    started: bool,
    /// Next input frame: the reader runs the stretcher's lead ahead of what is heard.
    rpos: i64,
    /// Input frames owed (fractions of a frame carried to the next block).
    owed: f64,
    input: Vec<f32>,
    l: Vec<f32>,
    r: Vec<f32>,
    at: usize,
    /// Output frames still to drop: the stretcher's latency, and the frames run ahead of a start inside the sample.
    skip: usize,
    /// At a start on the sample's first frame: the last `fold_n` frames out before the start (interleaved stereo,
    /// nearest the start first), folded back onto as many after it; `folded` of them are done.
    fold: Vec<f32>,
    fold_n: usize,
    folded: usize,
}

impl Rt {
    fn new(rate: u32) -> Rt {
        let st = Stream::new(rate);
        let fold = vec![0.0; 2 * st.delay()];
        let input = Vec::with_capacity(2 * (st.history() + st.lead()).max(RT_BLOCK * 16));
        Rt {
            st,
            busy: false,
            started: false,
            rpos: 0,
            owed: 0.0,
            input,
            l: vec![0.0; RT_BLOCK],
            r: vec![0.0; RT_BLOCK],
            at: RT_BLOCK,
            skip: 0,
            fold,
            fold_n: 0,
            folded: 0,
        }
    }

    /// Pushes the next input frame onto `input`: the loop followed, silence before the start and past the end.
    fn feed(&mut self, s: &Sample, (ls, le): (f64, Option<f64>)) {
        if let Some(le) = le {
            let (ls, le) = (ls as i64, le as i64);
            if self.rpos >= le {
                self.rpos -= le - ls;
            }
        }
        let (a, b) = match usize::try_from(self.rpos) {
            Ok(i) if i < s.frames() => (s.left[i], s.right[i]),
            _ => (0.0, 0.0),
        };
        self.input.push(a);
        self.input.push(b);
        self.rpos += 1;
    }

    /// Starts the stretcher for a voice heard from `pos` on, its latency taken out (spec: audio/time-stretch.md
    /// "Lemon Studio"). **(OS)** How it starts: a voice on the sample's first frame starts the way the library starts a
    /// whole buffer (silence before it; what comes out before the start is folded back onto what follows, so a
    /// stretched note starts like the same sample stretched first). A voice further in (a start offset, a trimmed
    /// clip, a voice that starts stretching while it plays) first gives the stretcher the frames before `pos` and runs
    /// it one analysis interval ahead, so the note is heard at once and at full level.
    fn start(&mut self, s: &Sample, pos: f64, transpose: f64, speed: f64, lp: (f64, Option<f64>)) {
        let fresh = pos < 1.0;
        let (hist, ahead) = if fresh { (0, 0) } else { (self.st.history(), self.st.interval()) };
        let back = (ahead as f64 * speed).round() as i64;
        self.rpos = pos.max(0.0).floor() as i64 - back - hist as i64;
        self.owed = 0.0;
        self.input.clear();
        for _ in 0..hist + self.st.lead() {
            self.feed(s, lp);
        }
        self.st.set_transpose(transpose);
        self.st.start(&self.input, speed);
        self.skip = self.st.delay() + ahead;
        self.fold_n = if fresh { self.st.delay() } else { 0 };
        self.folded = 0;
        self.at = RT_BLOCK;
        self.started = true;
    }

    /// The next output frame: `transpose` = the pitch factor, `speed` = input frames per output frame.
    fn frame(&mut self, s: &Sample, transpose: f64, speed: f64, lp: (f64, Option<f64>)) -> (f32, f32) {
        loop {
            if self.at < RT_BLOCK {
                let (mut a, mut b) = (self.l[self.at], self.r[self.at]);
                self.at += 1;
                if self.skip > 0 {
                    self.skip -= 1;
                    if self.skip < self.fold_n {
                        (self.fold[2 * self.skip], self.fold[2 * self.skip + 1]) = (a, b);
                    }
                    continue;
                }
                if self.folded < self.fold_n {
                    a -= self.fold[2 * self.folded];
                    b -= self.fold[2 * self.folded + 1];
                    self.folded += 1;
                }
                return (a, b);
            }
            self.owed += RT_BLOCK as f64 * speed;
            let k = self.owed.floor();
            self.owed -= k;
            self.input.clear();
            for _ in 0..k as usize {
                self.feed(s, lp);
            }
            self.st.set_transpose(transpose);
            self.st.process(&self.input, &mut self.l, &mut self.r);
            self.at = 0;
        }
    }
}

pub struct Engine {
    pub sample: Option<Arc<Sample>>,
    /// The played sample's loop (start and end frame, both inclusive), whether or not voices use it.
    pub looped: Option<(usize, usize)>,
    /// Frames of the played sample that the whole loaded file stands for: per-note starts from a host are fractions of
    /// the file (0 = the played sample's own length).
    pub file_frames: f64,
    /// How the resampler gets its weights: tables for live playback, the formula for an offline render.
    pub quality: Quality,
    rate: f32,
    voices: Vec<Voice>,
    /// Free-running phases of the global LFOs.
    global_phase: [f64; 5],
    /// Stretchers of the realtime stretch voices (made when first needed, then reused).
    streams: Vec<Rt>,
    /// Host voices that ended since the last `take_ended`: (key, host note id).
    ended: Vec<(u8, Option<i32>)>,
}

pub const MAX_VOICES: usize = 128;

impl Engine {
    pub fn new(rate: f32) -> Self {
        Engine {
            sample: None,
            looped: None,
            file_frames: 0.0,
            quality: Quality::Table,
            rate,
            voices: Vec::with_capacity(MAX_VOICES),
            global_phase: [0.0; 5],
            streams: Vec::new(),
            ended: Vec::with_capacity(MAX_VOICES * 2),
        }
    }

    pub fn set_rate(&mut self, rate: f32) {
        if rate != self.rate {
            self.voices.clear();
            self.streams.clear();
            self.ended.clear();
        }
        self.rate = rate;
    }

    /// Drops every voice. Not reported: a host resetting the instrument forgets its voices itself.
    pub fn reset(&mut self) {
        self.voices.clear();
        self.streams.iter_mut().for_each(|r| r.busy = false);
        self.ended.clear();
    }

    /// Host voices that ended since the last call, and host notes that started none: (key, host note id).
    pub fn take_ended(&mut self) -> std::vec::Drain<'_, (u8, Option<i32>)> {
        self.ended.drain(..)
    }

    /// A voice is over: its stretcher is free and a host voice's end is noted.
    fn end(&mut self, v: &Voice) {
        self.free(v);
        if v.host {
            self.ended.push((v.key, v.id));
        }
    }

    /// Ends every voice at once (no sample to play).
    fn end_all(&mut self) {
        let voices = std::mem::take(&mut self.voices);
        for v in &voices {
            self.end(v);
        }
        self.voices = voices;
        self.voices.clear();
    }

    /// A free stretcher for a new realtime voice.
    fn take_stream(&mut self) -> Option<u16> {
        let i = match self.streams.iter().position(|r| !r.busy) {
            Some(i) => i,
            None if self.streams.len() < MAX_VOICES => {
                self.streams.push(Rt::new(self.rate as u32));
                self.streams.len() - 1
            }
            None => return None,
        };
        let r = &mut self.streams[i];
        (r.busy, r.started) = (true, false);
        Some(i as u16)
    }

    /// Frees the stretcher of a voice that ended.
    fn free(&mut self, v: &Voice) {
        if let Some(r) = v.rt.and_then(|i| self.streams.get_mut(usize::from(i))) {
            r.busy = false;
        }
    }

    /// True while any voice is playing.
    pub fn has_voices(&self) -> bool {
        !self.voices.is_empty()
    }

    /// Keys currently sounding.
    pub fn sounding(&self) -> impl Iterator<Item = u8> + '_ {
        self.voices.iter().filter(|v| v.close.is_none()).map(|v| v.key)
    }

    /// The loop the voices play: the played sample's, when "Use loop points" is on and its end lies after its start
    /// (spec: audio/sampler.md "The sample file"). Start and one past the end, in frames.
    fn loop_frames(&self, b: &Block, n: usize) -> (f64, Option<f64>) {
        match self.looped {
            Some((s, e)) if b.loop_on && e > s && e < n => (s as f64, Some((e + 1) as f64)),
            _ => (0.0, None),
        }
    }

    /// How many times its length the current sample lasts in Resample or a realtime method: TIME's length when set,
    /// else MUL (spec §5.6).
    fn stretch_factor(b: &Block, s: &Sample) -> f64 {
        let ratio = match b.ts_time {
            Some(t) if s.frames() > 0 => t / (s.frames() as f64 / f64::from(s.rate.max(1))),
            _ => b.ts_ratio,
        };
        ratio.max(1e-3)
    }

    /// A note of the plugin's own (the editor's keyboard): its voice is not reported to the host.
    pub fn note_on(&mut self, key: u8, velocity: f32, b: &Block) {
        self.start(key, velocity, None, false, b);
    }

    /// A host note, `id` its note id (per-note modulation reaches it by that id). Its voice's end is reported, and a
    /// note that starts no voice is reported at once (spec: audio/voices.md §4).
    pub fn note_on_id(&mut self, key: u8, velocity: f32, id: Option<i32>, b: &Block) {
        self.start(key, velocity, id, true, b);
    }

    fn start(&mut self, key: u8, velocity: f32, id: Option<i32>, host: bool, b: &Block) {
        let zkey = if b.add_key { i32::from(key) - (b.root - 60) } else { i32::from(key) };
        let s = match self.sample.clone() {
            Some(s) if zkey >= b.zone.0 && zkey <= b.zone.1 && s.frames() > 0 => s,
            _ => {
                if host {
                    self.ended.push((key, id));
                }
                return;
            }
        };
        // A new note leaves the channel's other voices alone, the same key's too (spec: audio/sampler.md "A voice");
        // cut groups and the voice limit are the host's note rules (audio/note-rules.md).
        if self.voices.len() >= MAX_VOICES {
            let v = self.voices.remove(0);
            self.end(&v);
        }
        let ratio = f64::from(s.rate) / f64::from(self.rate);
        let key_step = 2f64.powf((f64::from(key) - f64::from(b.root)) / 12.0) * ratio;
        // FL velocity byte = plugin velocity × 128; 100 = unity (spec: audio/sampler-playback.md).
        let vel = laws::velocity_gain((velocity.clamp(0.0, 1.0) * 128.0).round() as u8) as f32;
        let pos = (b.start_offset.clamp(0.0, 1.0) * s.frames() as f64).floor();
        let open = if pos > 0.0 { fade_len(declick(b.declick).open_ms, f64::from(self.rate)) } else { 0 };
        let mut phase = [0.0; 5];
        for (i, t) in b.targets.iter().enumerate() {
            if t.lfo.global {
                phase[i] = self.global_phase[i];
            }
        }
        let rt = if b.realtime { self.take_stream() } else { None };
        self.voices.push(Voice {
            key,
            host,
            id,
            vel,
            pos,
            key_step,
            tune: 0.0,
            note_pan: 0.0,
            mod_x: 0.0,
            mod_y: 0.0,
            age: 0,
            off: None,
            open,
            close: None,
            phase,
            filt: Filter::default(),
            gain: -1.0,
            pan: (1.0, 1.0),
            rt,
            xf: 0,
        });
    }

    pub fn note_off(&mut self, key: u8, b: &Block) {
        let close = Close::new(&declick(b.declick), f64::from(self.rate));
        let vol_env = b.targets[VOL].env.is_some();
        let looped = self.sample.as_ref().is_some_and(|s| self.loop_frames(b, s.frames()).1.is_some());
        for v in self.voices.iter_mut().filter(|v| v.key == key && v.off.is_none()) {
            v.off = Some(v.age);
            // A looping voice without a volume envelope closes at the note's end; a one-shot voice without one
            // ignores it (spec: audio/sampler.md "The end of a voice").
            if looped && !vol_env && v.close.is_none() {
                v.close = Some(close);
            }
        }
    }

    /// A note that has not played yet starts `extra` further in, on top of START OFFSET: a fraction of the loaded file,
    /// counted in the played sample's frames (spec §7: a trimmed audio clip; audio/precomputed-effects.md "Audio
    /// clips": a trim selects frames of the processed sample by the file's rate).
    pub fn start_later(&mut self, id: i32, extra: f32, b: &Block) {
        let Some(s) = &self.sample else { return };
        let n = s.frames() as f64;
        let file = if self.file_frames > 0.0 { self.file_frames } else { n };
        let open = fade_len(declick(b.declick).open_ms, f64::from(self.rate));
        for v in self.voices.iter_mut().filter(|v| v.id == Some(id) && v.age == 0) {
            let at = b.start_offset.clamp(0.0, 1.0) * n + f64::from(extra.max(0.0)) * file;
            v.pos = at.min(n).floor();
            v.open = if v.pos > 0.0 { open } else { 0 };
        }
    }

    /// Tunes the voices of `key` to `semitones` from the key's pitch (the host's tuning expression: glides).
    pub fn tune(&mut self, key: u8, semitones: f64) {
        for v in self.voices.iter_mut().filter(|v| v.key == key) {
            v.tune = semitones;
        }
    }

    /// The voices a per-note event reaches: by note id when the host gives one, else by key.
    fn of_note(&mut self, id: Option<i32>, key: u8) -> impl Iterator<Item = &mut Voice> {
        self.voices.iter_mut().filter(move |v| match id {
            Some(id) if id >= 0 => v.id == Some(id),
            _ => v.key == key,
        })
    }

    /// The note's own pan (−1..1, the CLAP note expression PAN), also for a voice that is already sounding.
    pub fn pan(&mut self, id: Option<i32>, key: u8, pan: f32) {
        for v in self.of_note(id, key) {
            v.note_pan = f64::from(pan);
        }
    }

    /// The note's filter offsets in knob units (per-note modulation of MOD X / MOD Y), also while it sounds.
    pub fn modulate(&mut self, id: i32, mod_x: Option<f32>, mod_y: Option<f32>) {
        for v in self.voices.iter_mut().filter(|v| v.id == Some(id)) {
            if let Some(x) = mod_x {
                v.mod_x = x;
            }
            if let Some(y) = mod_y {
                v.mod_y = y;
            }
        }
    }

    /// Closes the voices of `key` with the declick mode's closing, whatever their envelopes (the host's choke: an
    /// audio clip ends, a cut group, the voice limit; spec: audio/note-rules.md).
    pub fn choke(&mut self, key: u8, b: &Block) {
        let close = Close::new(&declick(b.declick), f64::from(self.rate));
        for v in self.voices.iter_mut().filter(|v| v.key == key && v.close.is_none()) {
            v.close = Some(close);
        }
    }

    /// Ends every voice with a short fade.
    pub fn all_off(&mut self) {
        let close = Close::quick(f64::from(self.rate));
        for v in self.voices.iter_mut().filter(|v| v.close.is_none()) {
            v.close = Some(close);
        }
    }

    /// Adds the voices into `out_l` / `out_r`.
    pub fn render(&mut self, out_l: &mut [f32], out_r: &mut [f32], b: &Block) {
        let len = out_l.len();
        let rate = f64::from(self.rate);
        for (i, t) in b.targets.iter().enumerate() {
            self.global_phase[i] = (self.global_phase[i] + t.lfo.hz * len as f64 / rate).fract();
        }
        let Some(s) = self.sample.clone() else {
            self.end_all();
            return;
        };
        let n = s.frames();
        if n == 0 {
            self.end_all();
            return;
        }
        // Resample plays the sample faster or slower; a realtime voice reads `speed` input frames per output frame
        // and its stretcher keeps the pitch.
        let factor = Self::stretch_factor(b, &s);
        let rs = if b.realtime { 1.0 } else { 1.0 / factor };
        let speed = f64::from(s.rate) / rate / factor;
        let lp = self.loop_frames(b, n);
        let mut i = 0;
        while i < self.voices.len() {
            let v = &mut self.voices[i];
            let rt = v.rt.and_then(|k| self.streams.get_mut(usize::from(k)));
            let alive = render_voice(v, rt, &s, b, (rs, speed), lp, rate, self.quality, out_l, out_r);
            if alive {
                i += 1;
            } else {
                let v = self.voices.swap_remove(i);
                self.end(&v);
            }
        }
    }
}

/// Modulation at a voice time: gain, pan, extra cents, and the envelope and LFO parts of the filter knobs (singles).
/// `None` = the envelope has ended.
struct Mods {
    gain: f64,
    pan: f64,
    cents: f64,
    part_x: f32,
    part_y: f32,
}

fn mods(v: &Voice, b: &Block, t: f64, off: Option<f64>) -> Option<Mods> {
    let tg = &b.targets;
    let lfo = |i: usize| tg[i].lfo.value(t, v.phase[i]);
    let env = |i: usize| tg[i].env.map(|p| p.level(t, off));
    let mut gain = 1.0;
    if let Some(l) = env(VOL) {
        gain = l?;
    }
    let lv = lfo(VOL);
    if lv != 0.0 {
        // Tremolo: down to 1 − |amount| (spec §5.2).
        let a = tg[VOL].lfo.amount.abs();
        let w = lv / a.max(1e-9);
        gain *= 1.0 - a * (1.0 - w) / 2.0;
    }
    let pan = v.note_pan + lfo(PAN);
    // The amounts at full scale are 2400 cents, as FL stores them (spec: audio/envelopes-and-lfos.md).
    let mut cents = lfo(PITCH) * 2400.0;
    if let Some(l) = env(PITCH) {
        cents += l.unwrap_or(0.0) * tg[PITCH].amount * 2400.0;
    }
    let mut part_x = lfo(MODX) * 0.5;
    if let Some(l) = env(MODX) {
        part_x += l.unwrap_or(0.0) * tg[MODX].amount;
    }
    let mut part_y = lfo(MODY) * 0.5;
    if let Some(l) = env(MODY) {
        part_y += l.unwrap_or(0.0) * tg[MODY].amount;
    }
    Some(Mods { gain, pan, cents, part_x: part_x as f32, part_y: part_y as f32 })
}

#[allow(clippy::too_many_arguments)]
fn render_voice(
    v: &mut Voice,
    mut rt: Option<&mut Rt>,
    s: &Sample,
    b: &Block,
    (rs, speed): (f64, f64),
    (ls, le): (f64, Option<f64>),
    rate: f64,
    quality: Quality,
    out_l: &mut [f32],
    out_r: &mut [f32],
) -> bool {
    let len = out_l.len();
    let n = s.frames() as f64;
    let dc = declick(b.declick);
    let frame_s = 1.0 / rate;
    let mut f = 0;
    while f < len {
        let chunk = CTL.min(len - f);
        let t0 = v.age as f64 * frame_s;
        let off = v.off.map(|o| o as f64 * frame_s);
        let Some(m0) = mods(v, b, t0, off) else { return false };
        let g1 = match mods(v, b, t0 + chunk as f64 * frame_s, off) {
            Some(m) => m.gain,
            None => 0.0,
        };
        // The filter's knob positions: the knob plus the note's offset, then the envelope and LFO part (spec:
        // audio/channel-filter.md "Knob positions"; the host hands the note's mod X with the channel's offset in it).
        let x = (b.f_cut + v.mod_x) + m0.part_x;
        let y = (b.f_res + v.mod_y) + m0.part_y;
        let corr = v.filt.tick(b.f_type, x, y, CTL, rate);
        // The first span starts at the plain gain; type 5's correction is reached through the ramp.
        let g0 = if v.gain < 0.0 { m0.gain as f32 } else { v.gain };
        let g1 = g1 as f32 * corr;
        let pan = pan_gains(m0.pan);
        let (p0, p1) = (if v.gain < 0.0 { pan } else { v.pan }, pan);
        // The pitch factor: a resampling step, or a realtime voice's transposition (with the stretch's PITCH).
        let step = v.key_step * 2f64.powf((b.cents + m0.cents + v.tune * 100.0 + b.ts_cents) / 1200.0) * rs;
        // A realtime voice plays the sample by resampling while there is nothing to stretch (the root key, no PITCH,
        // MUL 1 or TIME the sample's own duration); its stretcher starts the first time there is, and stays. **(OS)**
        if let Some(x) = rt.as_mut() {
            if !x.started && !nothing_to_stretch(n, step, speed, rate) {
                x.start(s, v.pos, step, speed, (ls, le));
                v.xf = if v.age > 0 { SWITCH } else { 0 };
            }
        }
        let stretching = rt.as_ref().is_some_and(|x| x.started);
        // What is heard moves on by `step` source frames per frame, or by `speed` in a stretching voice.
        let advance = if stretching { speed } else { step };
        for j in 0..chunk {
            // The loop: past its end the position goes back by the loop's length (spec: audio/sampler.md "In a
            // sustain loop").
            if let Some(le) = le {
                while v.pos >= le {
                    v.pos -= le - ls;
                }
            }
            if v.pos < 0.0 {
                return false;
            }
            // The end of the sample: the closing starts on the first sample at or past it and works on the held
            // last frame (spec: audio/declick.md "When the closing starts").
            if v.pos >= n && v.close.is_none() {
                v.close = Some(Close::new(&dc, rate));
            }
            let (mut l, mut r) = match rt.as_mut() {
                Some(x) if stretching => {
                    let (a, b) = x.frame(s, step, speed, (ls, le));
                    if v.xf > 0 {
                        let (pa, pb) = resample::read(&s.left, &s.right, v.pos, speed, quality);
                        let u = v.xf as f32 / SWITCH as f32;
                        v.xf -= 1;
                        (a + (pa - a) * u, b + (pb - b) * u)
                    } else {
                        (a, b)
                    }
                }
                _ => resample::read(&s.left, &s.right, v.pos, step.abs(), quality),
            };
            l = v.filt.run(0, j, l);
            r = v.filt.run(1, j, r);
            // The fades act on the signal before the gains (spec: audio/declick.md "Where the fades sit"). FL works on
            // a voice's fades only once it closes, so an opening shows only on a voice that closes while it is younger
            // than its opening ("When the opening applies"); the step removal of modes 1 and 2 is not done here.
            if let Some(c) = v.close.as_mut() {
                if v.open > 0 && !dc.step_removal && v.age < u64::from(v.open) {
                    let o = fade_gain(v.open - v.age as u32, v.open, dc.tension);
                    (l, r) = (l * o, r * o);
                }
                match c.apply(l, r, rate) {
                    Some(x) => (l, r) = x,
                    None => return false,
                }
            }
            let u = j as f32 / chunk as f32;
            let g = v.vel * (g0 + (g1 - g0) * u);
            let (pl, pr) = (p0.0 + (p1.0 - p0.0) * u, p0.1 + (p1.1 - p0.1) * u);
            out_l[f + j] += l * g * pl;
            out_r[f + j] += r * g * pr;
            v.pos += advance;
            v.age += 1;
        }
        v.filt.end(chunk);
        v.gain = g1;
        v.pan = p1;
        for (i, t) in b.targets.iter().enumerate() {
            v.phase[i] = (v.phase[i] + t.lfo.hz * chunk as f64 * frame_s).fract();
        }
        f += chunk;
    }
    true
}

// ---------------------------------------------------------------------------------------------------------------
// The channel filter (spec: audio/channel-filter.md)

/// `Exp(double(single(z * single(ln base)))) - 1`: the curves of the filter's knob positions.
fn curve(z: f64, ln_base: f32) -> f64 {
    f64::from((z * f64::from(ln_base)) as f32).exp() - 1.0
}

/// `single(1 / 600)`.
const INV600: f32 = (1.0 / 600.0) as f32;

/// One side's memory of a biquad stage (singles).
#[derive(Clone, Copy, Debug, Default)]
struct Biquad {
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

/// One side's memory of a state-variable stage.
#[derive(Clone, Copy, Debug, Default)]
struct Svf {
    low: f64,
    band: f64,
}

/// What a tick's knob positions make of the filter.
#[derive(Clone, Copy, Debug)]
enum Coef {
    /// Type 0 (the fast low-pass): its `c` moves by `step` per sample from where it stands; `r` at once.
    Fast { step: f64, r: f64 },
    /// Types 1 to 5: `b0, b1, b2, a1, a2` (singles), run once or twice (type 5).
    Biquad { b: [f32; 5], twice: bool },
    /// Types 6 and 7: `f`, `d`, and the second stage's `d` (type 7).
    Svf { f: f64, d: f64, d2: Option<f64> },
}

/// A voice's filter (spec: audio/channel-filter.md): its memory per side, and the fast low-pass's `c`.
#[derive(Clone, Copy, Debug)]
struct Filter {
    /// The fast low-pass passes the voice unfiltered until a setting first asks for filtering.
    on: bool,
    /// The fast low-pass's `c` (set in the voice's first tick, then gliding) and memory `v`, `o` per side.
    c: Option<f64>,
    fast: [(f64, f64); 2],
    bq: [[Biquad; 2]; 2],
    sv: [[Svf; 2]; 2],
    coef: Coef,
}

impl Default for Filter {
    fn default() -> Self {
        Filter { on: false, c: None, fast: [(0.0, 0.0); 2], bq: Default::default(), sv: Default::default(), coef: Coef::Fast { step: 0.0, r: 0.0 } }
    }
}

impl Filter {
    /// Works out a tick's coefficients from the knob positions `x` (mod X) and `y` (mod Y), for a tick of `n` samples
    /// at `rate`. Returns the gain correction (type 5; 1 otherwise).
    fn tick(&mut self, kind: u8, x: f32, y: f32, n: usize, rate: f64) -> f32 {
        let k = (44100.0 / rate) as f32;
        match kind {
            1..=5 => {
                let ln51 = 51f64.ln() as f32;
                let lnq = (0.625 * 51f64.ln()) as f32;
                let w = ((curve(f64::from(x), ln51) * f64::from(0.0596f32) + 0.02) * f64::from(k)) as f32;
                let w = w.max((0.02 * f64::from(k)) as f32).min(3.0);
                let q = if y > 0.0 { (curve(f64::from(y.min(1.0)), lnq) * f64::from(0.4f32) + 0.1) as f32 } else { 0.1 };
                let (sn, cs) = (f64::from(w).sin() as f32, f64::from(w).cos() as f32);
                let alpha = sn / (2.0 * q);
                let nn = 1.0 / (f64::from(alpha) + 1.0);
                let (csd, al) = (f64::from(cs), f64::from(alpha));
                let a1 = (nn * csd * -2.0) as f32;
                let a2 = (nn * (1.0 - al)) as f32;
                let b = match kind {
                    2 => {
                        let b0 = (nn * al) as f32;
                        [b0, 0.0, -b0, a1, a2]
                    }
                    3 => {
                        let b1 = (-(nn * (csd + 1.0))) as f32;
                        [-b1 * 0.5, b1, -b1 * 0.5, a1, a2]
                    }
                    4 => {
                        let b0 = nn as f32;
                        [b0, a1, b0, a1, a2]
                    }
                    _ => {
                        let b1 = (nn * (1.0 - csd)) as f32;
                        [b1 * 0.5, b1, b1 * 0.5, a1, a2]
                    }
                };
                self.coef = Coef::Biquad { b, twice: kind == 5 };
                if kind == 5 {
                    (1.0 - f64::from(q) * (3.2 - f64::from(w)) * 0.046875) as f32
                } else {
                    1.0
                }
            }
            6 | 7 => {
                let f = if x > 0.0 { (((f64::from(x) * 26f64.ln()).exp() - 1.0) as f32 * 0.04 * k).min(1.0) } else { 0.0 };
                let yd = f64::from(y);
                let d = if y <= 0.0 {
                    1.0
                } else if y < 1.0 {
                    1.0 - 0.8 * yd
                } else {
                    0.2
                };
                self.coef = Coef::Svf { f: f64::from(f), d, d2: (kind == 7).then(|| d + (1.0 - d) * 0.25) };
                1.0
            }
            _ => {
                let ln601 = 601f64.ln() as f32;
                let target = if x <= 0.0 {
                    0.0
                } else if x < 1.0 {
                    (f64::from((curve(f64::from(x), ln601) * f64::from(INV600)) as f32) * f64::from(k)).min(1.0)
                } else {
                    1.0
                };
                let r = if y > 0.0 { 1.0 - f64::from((curve(1.0 - 0.625 * f64::from(y.min(1.0)), ln601) * f64::from(INV600)) as f32) } else { 0.0 };
                self.on |= target != 1.0 || r != 0.0;
                // The voice's first tick sets c; afterwards it glides by a limited step per sample.
                let step = match self.c {
                    None => {
                        self.c = Some(target);
                        0.0
                    }
                    Some(c) => {
                        let limit = f64::from((f64::from(k) * 0.001) as f32);
                        ((target - c) / n.max(1) as f64).clamp(-limit, limit)
                    }
                };
                self.coef = Coef::Fast { step, r };
                1.0
            }
        }
    }

    /// Sample `j` of the tick, one side (`side` 0 = left).
    #[inline]
    fn run(&mut self, side: usize, j: usize, x: f32) -> f32 {
        match self.coef {
            Coef::Fast { step, r } => {
                if !self.on {
                    return x;
                }
                let c = self.c.unwrap_or(1.0) + j as f64 * step;
                let (v, o) = &mut self.fast[side];
                *v += (f64::from(x) - *o) * c;
                *o += *v;
                *v *= r;
                *o as f32
            }
            Coef::Biquad { b, twice } => {
                let mut y = biquad(&mut self.bq[0][side], &b, x);
                if twice {
                    y = biquad(&mut self.bq[1][side], &b, y);
                }
                y
            }
            Coef::Svf { f, d, d2 } => {
                let y = svf(&mut self.sv[0][side], f, d, f64::from(x));
                match d2 {
                    Some(d2) => svf(&mut self.sv[1][side], f, d2, f64::from(y)),
                    None => y,
                }
            }
        }
    }

    /// The end of a tick of `len` samples: the fast low-pass's `c` moves on, the state-variable memory is rounded.
    fn end(&mut self, len: usize) {
        match self.coef {
            Coef::Fast { step, .. } => {
                if let Some(c) = self.c.as_mut() {
                    *c = f64::from((*c + step * len as f64) as f32);
                }
            }
            Coef::Svf { .. } => {
                for v in self.sv.iter_mut().flatten() {
                    v.low = round_small(v.low);
                    v.band = round_small(v.band);
                }
            }
            Coef::Biquad { .. } => {}
        }
    }
}

/// A state-variable value at the end of a tick: rounded to single, set to 0 below 2^-24 in magnitude.
fn round_small(v: f64) -> f64 {
    let s = v as f32;
    if s.abs() < 5.960_464_5e-8 { 0.0 } else { f64::from(s) }
}

#[inline]
fn biquad(m: &mut Biquad, b: &[f32; 5], x: f32) -> f32 {
    let d = |v: f32| f64::from(v);
    let y = (d(x) * d(b[0]) + (((d(m.x2) * d(b[2]) + d(m.x1) * d(b[1])) - d(m.y1) * d(b[3])) - d(m.y2) * d(b[4]))) as f32;
    (m.x2, m.x1, m.y2, m.y1) = (m.x1, x, m.y1, y);
    y
}

#[inline]
fn svf(m: &mut Svf, f: f64, d: f64, x: f64) -> f32 {
    m.low += m.band * f;
    m.band += (x - (m.band * d + m.low)) * f;
    m.low as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(n: usize) -> Sample {
        let left: Vec<f32> = (0..n).map(|i| i as f32 / n as f32).collect();
        Sample { rate: 48000, right: left.clone(), left }
    }

    #[test]
    fn root_key_plays_the_sample_exactly() {
        let s = ramp(100);
        let mut e = Engine::new(48000.0);
        e.sample = Some(Arc::new(s.clone()));
        let b = Block::default();
        e.note_on(60, 100.0 / 128.0, &b);
        let (mut l, mut r) = (vec![0.0; 64], vec![0.0; 64]);
        e.render(&mut l, &mut r, &b);
        assert_eq!(&l[..], &s.left[..64]);
        assert_eq!(&r[..], &s.right[..64]);
        let (mut l2, mut r2) = (vec![0.0; 64], vec![0.0; 64]);
        e.render(&mut l2, &mut r2, &b);
        assert_eq!(l2[35], s.left[99]);
        // Past the end the last frame is held under the 10 ms closing (spec: audio/declick.md).
        assert!((l2[36] - s.left[99]).abs() < 1e-3 && l2[63] < l2[36], "{} {}", l2[36], l2[63]);
        assert_eq!(e.sounding().count(), 0, "closing");
        let (mut l3, mut r3) = (vec![0.0; 480], vec![0.0; 480]);
        e.render(&mut l3, &mut r3, &b);
        assert!(l3[451] > 0.0 && l3[452..].iter().all(|v| *v == 0.0), "480 closing samples");
        assert!(!e.has_voices());
    }

    #[test]
    fn octave_up_plays_twice_as_fast() {
        let mut e = Engine::new(48000.0);
        e.sample = Some(Arc::new(ramp(1000)));
        let b = Block::default();
        e.note_on(72, 100.0 / 128.0, &b);
        let (mut l, mut r) = (vec![0.0; 600], vec![0.0; 600]);
        e.render(&mut l, &mut r, &b);
        assert!((l[200] - 0.4).abs() < 0.01, "{}", l[200]);
        // The end comes after 500 samples; the closing fades the held last frame.
        assert!(l[505] > 0.99 && l[550] < l[505], "{} {}", l[505], l[550]);
        let (mut l, mut r) = (vec![0.0; 600], vec![0.0; 600]);
        e.render(&mut l, &mut r, &b);
        assert!(!e.has_voices());
    }

    // spec: audio/sampler.md "A voice": a channel's voices do not affect one another, the same key's neither.
    #[test]
    fn a_repeated_key_leaves_the_ringing_voice_alone() {
        let mut e = Engine::new(48000.0);
        e.sample = Some(Arc::new(sine(440.0, 48000)));
        let b = Block::default();
        e.note_on(60, 0.78, &b);
        let (mut l, mut r) = (vec![0.0; 480], vec![0.0; 480]);
        e.render(&mut l, &mut r, &b);
        e.note_on(60, 0.78, &b);
        e.render(&mut l, &mut r, &b);
        assert_eq!(e.voices.len(), 2);
        assert_eq!(e.sounding().count(), 2);
        // A choke closes both through the mode's closing: 20 ms in mode 2.
        let b2 = Block { declick: 2, ..Block::default() };
        e.choke(60, &b2);
        for _ in 0..2 {
            l.fill(0.0);
            e.render(&mut l, &mut r, &b2);
        }
        assert!(e.has_voices(), "20 ms not over after 10");
        e.render(&mut l, &mut r, &b2);
        e.render(&mut l, &mut r, &b2);
        assert!(!e.has_voices());
    }

    // spec: audio/declick.md "Gain fade": the closing falls from just under 1 to just over 0; tension 5 bends it.
    #[test]
    fn closing_fades_follow_the_tables() {
        assert_eq!(fade_len(10.0, 44100.0), 441);
        assert_eq!(fade_len(4.0, 44100.0), 176);
        assert_eq!(fade_len(2.0, 44100.0), 88);
        let n = 441;
        assert!(fade_gain(1, n, 0.0) > 0.9999 && fade_gain(1, n, 0.0) < 1.0);
        assert!(fade_gain(n, n, 0.0) > 0.0 && fade_gain(n, n, 0.0) < 1e-4);
        assert!((fade_gain(221, n, 0.0) - 0.5).abs() < 1e-6);
        let mid = fade_gain(221, n, 5.0);
        assert!(mid > 0.5 && mid < 1.0, "{mid}");
        // The sweep ends silent and keeps a constant from jumping.
        let mut c = Close::new(&declick(1), 44100.0);
        let mut last = (1.0, 1.0);
        while let Some(x) = c.apply(1.0, 1.0, 44100.0) {
            last = x;
        }
        assert!(last.0.abs() < 1e-3);
    }

    // spec: audio/sampler.md "In a sustain loop": the played sample's loop, used while "Use loop points" is on.
    #[test]
    fn zone_and_loop() {
        let mut e = Engine::new(48000.0);
        e.sample = Some(Arc::new(ramp(100)));
        e.looped = Some((50, 99));
        let b = Block { zone: (48, 59), ..Block::default() };
        e.note_on(60, 0.78, &b);
        assert!(!e.has_voices(), "outside the zone");
        let b = Block { loop_on: true, ..Block::default() };
        e.note_on(60, 100.0 / 128.0, &b);
        let (mut l, mut r) = (vec![0.0; 400], vec![0.0; 400]);
        e.render(&mut l, &mut r, &b);
        assert!(e.has_voices(), "a loop keeps playing");
        assert_eq!(l[100], l[50], "jumps back to the loop start");
        assert_eq!(l[99], l[149]);
        e.note_off(60, &b);
        let (mut l, mut r) = (vec![0.0; 2000], vec![0.0; 2000]);
        e.render(&mut l, &mut r, &b);
        assert!(!e.has_voices(), "note-off ends a loop without a volume envelope");
        // With "Use loop points" off the sample plays once, and the note's end does not cut it.
        let b = Block::default();
        e.note_on(60, 100.0 / 128.0, &b);
        e.note_off(60, &b);
        let (mut l, mut r) = (vec![0.0; 64], vec![0.0; 64]);
        e.render(&mut l, &mut r, &b);
        assert_eq!(l[63], 63.0 / 100.0);
    }

    fn sine(hz: f64, n: usize) -> Sample {
        let x: Vec<f32> = (0..n).map(|i| (2.0 * PI * hz * i as f64 / 48000.0).sin() as f32 * 0.5).collect();
        Sample { rate: 48000, left: x.clone(), right: x }
    }

    /// Frequency from the rising zero crossings.
    fn hz(x: &[f32]) -> f64 {
        let ups: Vec<usize> = x.windows(2).enumerate().filter(|(_, w)| w[0] < 0.0 && w[1] >= 0.0).map(|(i, _)| i).collect();
        (ups.len() - 1) as f64 * 48000.0 / (ups[ups.len() - 1] - ups[0]) as f64
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    /// Renders a held note of `key` for `secs` seconds at 48 kHz.
    fn play(e: &mut Engine, key: u8, b: &Block, secs: f64) -> Vec<f32> {
        e.note_on(key, 100.0 / 128.0, b);
        let mut out = Vec::new();
        let (mut l, mut r) = (vec![0.0; 480], vec![0.0; 480]);
        for _ in 0..(secs * 100.0) as usize {
            l.fill(0.0);
            r.fill(0.0);
            e.render(&mut l, &mut r, b);
            out.extend_from_slice(&l);
        }
        out
    }

    // spec §5.6: the realtime methods stretch while the voice plays; the key and PITCH move only the pitch.
    #[test]
    fn realtime_voices_keep_the_length_and_move_the_pitch() {
        let mut e = Engine::new(48000.0);
        e.sample = Some(Arc::new(sine(440.0, 48000)));
        // Twice as long, an octave up by the key; then by PITCH.
        for (key, cents) in [(72u8, 0.0), (60, 1200.0)] {
            let b = Block { realtime: true, ts_ratio: 2.0, ts_cents: cents, ..Block::default() };
            let out = play(&mut e, key, &b, 2.5);
            let f = hz(&out[24000..72000]);
            assert!((f - 880.0).abs() < 4.0, "key {key}: {f} Hz");
            assert!(rms(&out[80000..92000]) > 0.1, "key {key}: still playing after the sample's own length");
            assert!(!e.has_voices(), "key {key}: over after twice its length");
            e.reset();
        }
        assert!(e.streams.iter().all(|r| !r.busy), "stretchers freed");
    }

    // spec: audio/time-stretch.md "Lemon Studio" (OS): a realtime voice with nothing to stretch plays the sample as
    // it is, its stretcher never started (here a 44.1 kHz sample at 48 kHz: rate conversion alone).
    #[test]
    fn a_realtime_voice_with_nothing_to_stretch_plays_the_sample_as_it_is() {
        let x: Vec<f32> = (0..30000).map(|i| ((i * 7919 % 2003) as f32 / 2003.0 - 0.5) * (-(i as f32) / 9000.0).exp()).collect();
        let s = Arc::new(Sample { rate: 44100, left: x.clone(), right: x });
        let mut plain = Engine::new(48000.0);
        plain.sample = Some(s.clone());
        let mut e = Engine::new(48000.0);
        e.sample = Some(s);
        let rt = Block { realtime: true, ts_ratio: 1.0, ..Block::default() };
        assert_eq!(play(&mut e, 60, &rt, 0.8), play(&mut plain, 60, &Block::default(), 0.8));
        assert!(!e.streams.is_empty() && e.streams.iter().all(|r| !r.started), "the stretcher stayed off");
        // TIME a little off the duration (0.68 s to 0.6826 s): still nothing to stretch.
        e.reset();
        plain.reset();
        let near = Block { realtime: true, ts_ratio: 1.004, ..Block::default() };
        assert_eq!(play(&mut e, 60, &near, 0.8), play(&mut plain, 60, &Block::default(), 0.8));
        assert!(e.streams.iter().all(|r| !r.started), "the stretcher stayed off");
        // The same on a long sample is 0.1 s longer: stretched.
        e.reset();
        e.sample = Some(Arc::new(sine(440.0, 1_200_000)));
        play(&mut e, 60, &near, 0.1);
        assert!(e.streams.iter().any(|r| r.started), "stretching");
    }

    // spec §5.6 + audio/time-stretch.md "Lemon Studio" (OS): a voice whose pitch moves while it plays starts its
    // stretcher then, without a click, and keeps its length.
    #[test]
    fn a_realtime_voice_starts_stretching_when_its_pitch_moves() {
        let mut e = Engine::new(48000.0);
        e.sample = Some(Arc::new(sine(440.0, 48000)));
        let b = Block { realtime: true, ts_ratio: 1.0, ..Block::default() };
        let mut out = play(&mut e, 60, &b, 0.1);
        assert!(e.streams.iter().all(|r| !r.started));
        e.tune(60, 1.0);
        let (mut l, mut r) = (vec![0.0; 480], vec![0.0; 480]);
        for i in 0..110 {
            if i == 85 {
                assert!(e.has_voices(), "still playing before the sample's own length");
            }
            l.fill(0.0);
            r.fill(0.0);
            e.render(&mut l, &mut r, &b);
            out.extend_from_slice(&l);
        }
        assert!(e.streams.iter().any(|r| r.started), "stretching");
        assert!(!e.has_voices(), "over at the sample's length");
        let f = hz(&out[14400..43200]);
        assert!((f - 440.0 * 2f64.powf(1.0 / 12.0)).abs() < 3.0, "{f} Hz");
        // No click where it changes over: no step much bigger than the sine's own.
        let jump = out[4700..5400].windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!(jump < 0.05, "{jump}");
        assert!(rms(&out[4800..5400]) > 0.3, "{}", rms(&out[4800..5400]));
    }

    // spec: audio/time-stretch.md "Lemon Studio" (OS): a stretched voice from the sample's start begins the way the
    // sample stretched first begins: on time, at its level.
    #[test]
    fn a_realtime_voice_starts_like_the_sample_stretched_first() {
        let x: Vec<f32> = (0..24000)
            .map(|i| {
                let t = i as f64 / 48000.0;
                ((2.0 * PI * (50.0 * t + 110.0 / 30.0 * (1.0 - (-30.0 * t).exp()))).sin() * (-12.0 * t).exp() * 0.9) as f32
            })
            .collect();
        let s = Sample { rate: 48000, left: x.clone(), right: x };
        let whole = stretch(&s, 2.0, 1.0);
        let mut e = Engine::new(48000.0);
        e.sample = Some(Arc::new(s));
        let out = play(&mut e, 60, &Block { realtime: true, ts_ratio: 2.0, ..Block::default() }, 0.1);
        let err = out.iter().zip(&whole.left).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(err < 1e-4, "{err}");
    }

    // spec: audio/time-stretch.md "Lemon Studio" (OS): a stretched voice that starts inside the sample is heard at
    // once at full level (its stretcher is given what lies before the start).
    #[test]
    fn a_realtime_voice_inside_the_sample_starts_at_full_level() {
        let mut e = Engine::new(48000.0);
        e.sample = Some(Arc::new(sine(1000.0, 96000)));
        let peak = |x: &[f32]| x.iter().fold(0.0, |m: f32, v| m.max(v.abs()));
        for (ratio, key) in [(2.0, 60u8), (0.5, 60), (1.0, 67)] {
            let b = Block { realtime: true, ts_ratio: ratio, start_offset: 0.5, ..Block::default() };
            let out = play(&mut e, key, &b, 0.1);
            // Peaks over two periods or more.
            for (a, z) in [(0, 96), (96, 240), (240, 480), (480, 1440)] {
                let db = 20.0 * (peak(&out[a..z]) / 0.5).log10();
                assert!(db.abs() < 1.0, "ratio {ratio} key {key}, frames {a}..{z}: {db} dB");
            }
            e.reset();
        }
    }

    // spec §5.6: Resample fits TIME by speed; PITCH plays no part.
    #[test]
    fn resample_fits_the_time_by_speed() {
        let mut e = Engine::new(48000.0);
        e.sample = Some(Arc::new(sine(440.0, 48000)));
        let b = Block { ts_time: Some(2.0), ts_cents: 0.0, ..Block::default() };
        let out = play(&mut e, 60, &b, 1.0);
        assert!((hz(&out[4800..43200]) - 220.0).abs() < 1.0);
    }

    #[test]
    fn stretch_keeps_pitch_and_changes_length() {
        let n = 48000;
        let sine: Vec<f32> = (0..n).map(|i| (2.0 * PI * 440.0 * i as f64 / 48000.0).sin() as f32 * 0.5).collect();
        let s = Sample { rate: 48000, left: sine.clone(), right: sine };
        let st = stretch(&s, 2.0, 1.0);
        assert_eq!(st.frames(), 2 * n);
        // A fifth up at the same length.
        let up = stretch(&s, 1.0, 1.5);
        assert_eq!(up.frames(), n);
        assert!((hz(&up.left[12000..36000]) - 660.0).abs() < 3.0);
        // Count zero crossings in the middle: the same frequency.
        let zc = |v: &[f32]| v.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        let c = zc(&st.left[24000..72000]);
        assert!((c as i32 - 440).abs() <= 6, "{c}");
    }

    /// Runs `n` ticks of a constant `x` through a filter of `kind` at knob positions `cut`, `res` (44.1 kHz); the
    /// last output.
    fn settle(kind: u8, cut: f32, res: f32, x: f32, ticks: usize) -> f32 {
        let mut f = Filter::default();
        let mut y = 0.0;
        for _ in 0..ticks {
            f.tick(kind, cut, res, CTL, 44100.0);
            for j in 0..CTL {
                y = f.run(0, j, x);
                f.run(1, j, x);
            }
            f.end(CTL);
        }
        y
    }

    // spec: audio/channel-filter.md: the low-pass types and the band stop pass a constant, the band pass and the
    // high pass remove it.
    #[test]
    fn the_filter_types_pass_or_block_a_constant() {
        for kind in [0u8, 1, 4, 5, 6, 7] {
            let y = settle(kind, 0.5, 0.0, 0.5, 400);
            assert!((y - 0.5).abs() < 1e-3, "type {kind}: {y}");
        }
        for kind in [2u8, 3] {
            let y = settle(kind, 0.5, 0.0, 0.5, 400);
            assert!(y.abs() < 1e-3, "type {kind}: {y}");
        }
        // With resonance the fast low-pass still settles on the input.
        let y = settle(0, 0.3, 0.6, 0.25, 2000);
        assert!((y - 0.25).abs() < 1e-3, "{y}");
    }

    // spec: audio/channel-filter.md "Type 0": fully open and without resonance the fast low-pass is not running.
    #[test]
    fn an_open_fast_low_pass_leaves_the_voice_alone() {
        let mut f = Filter::default();
        assert_eq!(f.tick(0, 1.0, 0.0, CTL, 44100.0), 1.0);
        assert_eq!(f.run(0, 0, 0.123), 0.123);
        assert_eq!(f.run(1, 5, -0.5), -0.5);
        // Once a setting asks for filtering it stays on, even back at the open position.
        f.tick(0, 0.9, 0.0, CTL, 44100.0);
        f.end(CTL);
        f.tick(0, 1.0, 0.0, CTL, 44100.0);
        assert!(f.on);
    }

    // spec: audio/channel-filter.md "Type 0": the first tick sets c, later ticks glide it by at most
    // single(44100 / rate) * 0.001 per sample.
    #[test]
    fn the_fast_low_pass_glides() {
        let mut f = Filter::default();
        f.tick(0, 0.5, 0.0, CTL, 44100.0);
        let c0 = f.c.unwrap();
        assert!(c0 > 0.0 && c0 < 0.1, "{c0}");
        f.end(CTL);
        assert_eq!(f.c.unwrap(), c0, "no step in the first tick");
        f.tick(0, 1.0, 0.0, CTL, 44100.0);
        f.end(CTL);
        let c1 = f.c.unwrap();
        assert!((c1 - c0 - 0.032).abs() < 1e-6, "{c0} -> {c1}");
    }

    // spec: audio/channel-filter.md: type 5 lowers the voice's gain a little (the correction), the others do not.
    #[test]
    fn the_double_low_pass_corrects_its_gain() {
        let mut f = Filter::default();
        for kind in [0u8, 1, 2, 3, 4, 6, 7] {
            assert_eq!(f.tick(kind, 0.7, 0.4, CTL, 44100.0), 1.0, "type {kind}");
        }
        let g = f.tick(5, 1.0, 0.0, CTL, 44100.0);
        assert!((g - (1.0 - 0.1 * 0.2 * 0.046875) as f32).abs() < 1e-6, "{g}");
        // Full resonance at the lowest cutoff: q = 0.4 * (51^0.625 - 1) + 0.1, w = 0.02.
        let g = f.tick(5, 0.0, 1.0, CTL, 44100.0);
        let q = 0.4 * ((0.625 * 51f64.ln()).exp() - 1.0) + 0.1;
        assert!((f64::from(g) - (1.0 - q * 3.18 * 0.046875)).abs() < 1e-4, "{g}");
    }

    /// The level of a voice playing a sine of `hz` with the given block, after it settles.
    fn level(hz: f64, b: &Block, f: impl Fn(&mut Engine)) -> (f32, f32) {
        let mut e = Engine::new(48000.0);
        e.sample = Some(Arc::new(sine(hz, 48000)));
        e.note_on_id(60, 100.0 / 128.0, Some(7), b);
        f(&mut e);
        let (mut l, mut r) = (vec![0.0; 9600], vec![0.0; 9600]);
        e.render(&mut l, &mut r, b);
        (rms(&l[4800..]), rms(&r[4800..]))
    }

    #[test]
    fn a_low_cutoff_dulls_the_voice() {
        let open = level(9000.0, &Block::default(), |_| {}).0;
        assert!((open - 0.5 / 2f32.sqrt()).abs() < 1e-3, "{open}");
        for kind in [0u8, 1, 5, 6, 7] {
            let b = Block { f_type: kind, f_cut: 0.2, ..Block::default() };
            let dull = level(9000.0, &b, |_| {}).0;
            assert!(dull < open * 0.1, "type {kind}: {dull}");
        }
    }

    // spec: audio/voice-levels.md "Final levels": the note's mod X adds to the channel's.
    #[test]
    fn a_note_offset_moves_its_voices_filter() {
        let b = Block { f_type: 1, f_cut: 1.0, ..Block::default() };
        let open = level(1000.0, &b, |_| {}).0;
        let dull = level(1000.0, &b, |e| e.modulate(7, Some(-0.8), None)).0;
        assert!(open > 0.25 && dull < open * 0.2, "{open} {dull}");
        // Other notes keep theirs; mod Y raises the resonance.
        let other = level(1000.0, &b, |e| e.modulate(8, Some(-0.8), None)).0;
        assert_eq!(other, open);
        let b = Block { f_type: 1, f_cut: 0.75, ..Block::default() };
        let plain = level(1000.0, &b, |_| {}).0;
        let res = level(1000.0, &b, |e| e.modulate(7, None, Some(1.0))).0;
        assert!(res != plain, "{plain} {res}");
    }

    // spec: audio/voice-levels.md "Final levels", audio/conventions.md "Pan split": the voice splits at √2 so the
    // host's centred channel split makes the note's own split.
    #[test]
    fn a_note_pan_splits_the_voice() {
        assert_eq!(pan_gains(0.0), (1.0, 1.0));
        let (l, r) = pan_gains(-1.0);
        assert!((l - 2f32.sqrt()).abs() < 1e-6 && r == 0.0);
        let (l, r) = pan_gains(0.5);
        let a = 1.5 * PI / 4.0;
        assert!((l * std::f32::consts::FRAC_1_SQRT_2 - a.cos() as f32).abs() < 1e-6);
        assert!((r * std::f32::consts::FRAC_1_SQRT_2 - a.sin() as f32).abs() < 1e-6);
        // Pans beyond the range clamp.
        assert_eq!(pan_gains(3.0), pan_gains(1.0));
        let (open_l, open_r) = level(9000.0, &Block::default(), |_| {});
        let (left, right) = level(9000.0, &Block::default(), |e| e.pan(Some(7), 60, -1.0));
        assert!((left - open_l * 2f32.sqrt()).abs() < 1e-4 && right < 1e-6, "{left} {right} {open_r}");
        // By key when the host names no note.
        let (_, right) = level(9000.0, &Block::default(), |e| e.pan(None, 60, -1.0));
        assert!(right < 1e-6);
    }
}
