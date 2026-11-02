// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Sampler voices: pitch, loop, declicking, the channel's envelopes and LFOs, the channel filter, the gains, and
//! the realtime stretch methods. The played sample itself is built in `fx`.
//!
//! Everything that changes during a note changes at tick starts, as in FL: the envelopes and LFOs step, the ratio, the
//! filter's knob positions and the gain targets are worked out, and the gains ramp to their targets over the tick.
//! Lemon Studio hands the plugin one tick per `process` call ([`Time::host_ticks`]); in another host the voices keep a
//! tick clock of their own.
//!
// spec: plugins/lemon-sampler.md (§5 Sound), audio/sampler.md (a voice, the resampler, the ratio, the loop, the end of
// spec: a voice, the gains), audio/envelopes-and-lfos.md, audio/timing.md (samples per tick), audio/declick.md (opening
// spec: and closing fades), audio/channel-filter.md, audio/voice-levels.md, audio/conventions.md (pan split)

use std::f64::consts::PI;
use std::sync::Arc;

use os_dsp::articulator::{Record, Shared, Timing, Voice as Articulators};
use os_dsp::resample::{self, Quality};
use os_dsp::stretch::Stream;
use os_engine::instrument::VOICE_GAIN_FRAMES;
use os_engine::{laws, Sample};

use crate::stream::{Feed, Played};

/// Time stretch to `ratio` × the length, the pitch moved by the factor `shift` (spec §5.6: Signalsmith Stretch).
pub fn stretch(s: &Sample, ratio: f64, shift: f64) -> Sample {
    let frames = (s.frames() as f64 * ratio).round().max(1.0) as usize;
    let (left, right) = os_dsp::stretch::whole(&s.left, &s.right, s.rate, frames, shift);
    Sample { rate: s.rate, left, right }
}

// ---------------------------------------------------------------------------------------------------------------
// Voices (spec §5.1 - §5.3)

pub const PAN: usize = Record::PAN;
pub const VOL: usize = Record::VOLUME;
pub const MODX: usize = Record::MOD_X;
pub const MODY: usize = Record::MOD_Y;
pub const PITCH: usize = Record::PITCH;

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
    /// The channel's five envelope and LFO records: pan, volume, mod X, mod Y, pitch (spec:
    /// audio/envelopes-and-lfos.md "Settings").
    pub records: [Record; 5],
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
            records: Record::new_channel_records(),
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

/// The song's timing as the voices need it (spec: audio/timing.md "Samples per tick").
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Time {
    pub tempo: f64,
    /// Ticks per quarter note.
    pub ppq: u16,
    /// Steps (sixteenths) per beat of the bar: 4 in x/4, 2 in x/8 (spec "The tempo counts the bar's beats").
    pub steps: u32,
    /// The song tick the next `process` call starts on, while the song plays: the global LFOs' shared phase starts
    /// from it (spec: audio/envelopes-and-lfos.md "The global LFO").
    pub song_tick: Option<u64>,
    /// The host hands the voices one tick per `process` call (Lemon Studio's engine); otherwise the voices count the
    /// ticks themselves from their first call. **(OS)**
    pub host_ticks: bool,
}

impl Default for Time {
    fn default() -> Self {
        Time { tempo: 130.0, ppq: 96, steps: 4, song_tick: None, host_ticks: false }
    }
}

impl Time {
    /// Samples per tick at `rate`: `rate * 60 * 4 / (tempo * steps * PPQ)`, the tempo first rounded to a single.
    pub fn samples_per_tick(&self, rate: f64) -> f64 {
        let tempo = f64::from(self.tempo.max(1.0) as f32);
        rate * 60.0 * 4.0 / (tempo * f64::from(self.steps.max(1)) * f64::from(self.ppq.max(1)))
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

/// Samples in a mode's opening at `rate`: none in modes 0 and 6.
fn open_len(d: &Declick, rate: f64) -> u32 {
    if d.open_ms > 0.0 { fade_len(d.open_ms, rate) } else { 0 }
}

/// The step removal's share of the first sample taken off at output sample `i` of an opening of `n` samples: a falling
/// raised cosine with the exact cosine, `single((cos((i + 1) * (1 / (n + 1)) * pi) + 1) * 0.5)` in doubles (spec:
/// audio/declick.md "Step removal").
fn step_gain(i: u32, n: u32) -> f32 {
    (((f64::from(i + 1) * (1.0 / f64::from(n + 1)) * std::f64::consts::PI).cos() + 1.0) * 0.5) as f32
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

/// `ln 2 / 12 / 100` in doubles: the ratio is `exp(cents * C) * q` (spec: audio/sampler.md "The ratio").
const C: f64 = std::f64::consts::LN_2 / 12.0 / 100.0;

/// A gain closer to zero than this (2^-24) is 0 (spec: audio/conventions.md "Glide", rule 3).
const TINY: f32 = 5.960_464_5e-8;

/// A voice's pan gains for a pan position (−1..1, clamped) when the host splits the channel's pan after the voice: the
/// constant-power split of `sqrt(2)`, so a centred pan is 1 on both sides, and with either pan centred the product is
/// the split of their sum (spec: audio/voice-levels.md "Final levels", audio/conventions.md "Pan split").
pub fn pan_gains(pan: f64) -> (f32, f32) {
    let [l, r] = split(pan as f32, std::f32::consts::SQRT_2);
    (l, r)
}

/// The pan split of `level` at `pan` (spec: audio/conventions.md "Pan split"): the angle and the products in doubles,
/// each gain rounded once. A centred pan gives `level * 0.7071...` on each side.
fn split(pan: f32, level: f32) -> [f32; 2] {
    let lv = f64::from(level);
    let a = (f64::from(pan.clamp(-1.0, 1.0)) + 1.0) * (PI / 4.0);
    [(lv * a.cos()) as f32, (lv * a.sin()) as f32]
}

/// The step that moves `gain` towards `target` over a tick of nominal length `n` (spec: audio/sampler.md "Gains"):
/// `(target - gain) / n` within `±limit`. A limited step whose end, `gain + step * n`, would come closer to zero than
/// 2^-24 becomes `-gain / n`.
fn glide_step(gain: f32, target: f32, n: u32, limit: f32) -> f32 {
    let n = n.max(1) as f32;
    let step = (target - gain) / n;
    if step.abs() <= limit {
        return step;
    }
    let step = limit.copysign(step);
    if (gain + step * n).abs() < TINY {
        -gain / n
    } else {
        step
    }
}

/// One side's gain through a tick, by sums in single precision as FL builds it (spec: audio/sampler.md "Gains"): the
/// even samples from the gain at the tick's start, the odd ones from that plus a step, each series advancing by two
/// steps, for the first `L - L mod 8` samples of a tick of `L`; the samples after them carry the even series on by one
/// step each.
#[derive(Clone, Copy, Debug)]
struct Ramp {
    even: f32,
    odd: f32,
    step: f32,
    /// Samples done in the tick, and the interleaved ones (`L - L mod 8`).
    j: u32,
    pairs: u32,
}

impl Ramp {
    fn new(gain: f32, step: f32, len: u32) -> Ramp {
        Ramp { even: gain, odd: gain + step, step, j: 0, pairs: len - len % 8 }
    }

    #[inline]
    fn next(&mut self) -> f32 {
        let j = self.j;
        self.j += 1;
        if j >= self.pairs {
            let g = self.even;
            self.even += self.step;
            return g;
        }
        let two = self.step * 2.0;
        let s = if j % 2 == 0 { &mut self.even } else { &mut self.odd };
        let g = *s;
        *s += two;
        g
    }
}

/// Where a voice is in its current tick.
#[derive(Clone, Copy, Debug)]
struct TickState {
    /// Samples of the tick the voice plays (fewer in the tick it starts in, when it starts inside one), and done.
    len: u32,
    done: u32,
    /// The playback ratio, set at the tick's start.
    ratio: f64,
    /// Away from the sample's edges the tick's positions are `p + k * R` from its start `p`, which then moves on by
    /// `R * L` at once; near an edge (`None`) they are summed sample by sample (spec: audio/sampler.md "How the
    /// position moves").
    from: Option<f64>,
    /// At a ratio of exactly 1, from a whole position away from the edges, the frames are handed on unchanged
    /// ("The copy").
    copy: bool,
    /// Each side's gain.
    ramp: [Ramp; 2],
}

#[derive(Clone, Copy)]
struct Voice {
    key: u8,
    /// Started by a host note: its end is reported back (spec: audio/voices.md §4). Editor previews are not.
    host: bool,
    /// Host note id (per-note modulation), if the host gave one.
    id: Option<i32>,
    /// The note's volume (its velocity on the level curve).
    vel: f32,
    pos: f64,
    /// Cents from the root key, and the played sample's rate over the output rate (`q`).
    key_cents: f32,
    q: f64,
    /// Host tuning of this key's voices, semitones (portamento glides; spec: audio/channel-voices.md).
    tune: f64,
    /// The note's own pan (−1..1) and its filter offsets in knob units, from the host per note (CLAP note expression
    /// PAN, per-note modulation of the filter knobs; spec: audio/voice-levels.md).
    note_pan: f64,
    mod_x: f32,
    mod_y: f32,
    /// Samples played.
    age: u64,
    /// The note has ended.
    released: bool,
    /// Opening length in samples: only a voice that starts inside its sample has one (0 = none).
    open: u32,
    /// A trimmed audio clip's end frame, in the played sample's frames (`None`: the sample's end).
    end: Option<f64>,
    /// The fades are worked on: from the tick in which the voice reaches its end or is released.
    fades: bool,
    /// The step removal's first sample per side, taken when the fades are worked on from the voice's first sample.
    step: Option<(f32, f32)>,
    /// The closing, once it has started (note end, choke, the end of the sample).
    close: Option<Close>,
    /// The volume envelope ran out in the last tick: the closing starts with this one.
    close_next: bool,
    /// The five envelopes and LFOs; ticks stepped.
    art: Articulators,
    ticks: u32,
    /// The volume envelope starts in its delay or attack: the gain ramps in from 0 in the first tick.
    ramp_in: bool,
    /// The voice has gone round its loop: the loop's start is an edge from then on.
    looped: bool,
    filt: Filter,
    /// Each side's gain at the start of the current tick.
    gain: [f32; 2],
    /// The tick the voice is in (its number) and where in it.
    serial: u64,
    tick: TickState,
    /// Its stretcher in `Engine::streams` (realtime stretch methods).
    rt: Option<u16>,
    /// Frames left of the crossfade from the plain reading to the stretcher (a voice that starts stretching while it
    /// plays).
    xf: u32,
    /// The host's gain on the voice (CLAP volume expression: an audio clip's fades and gain), once it gives one.
    vol: Option<HostGain>,
    /// Where it reads a sample kept on disk, and frames left of its fade back in after its frames came late.
    feed: Feed,
    gap: u32,
}

/// The gain the host gives a voice (`Engine::volume`): where it stands, where it glides to, the step per frame and the
/// frames of the glide left.
#[derive(Clone, Copy)]
struct HostGain {
    now: f32,
    to: f32,
    step: f32,
    left: u32,
}

impl HostGain {
    /// The gain of this frame; the glide then moves on a frame.
    fn next(&mut self) -> f32 {
        let g = self.now;
        if self.left > 0 {
            self.left -= 1;
            self.now = if self.left == 0 { self.to } else { self.now + self.step };
        }
        g
    }
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

/// **(Lemon Studio)** Frames of a voice's fade back in after its frames from disk came late (live playback only).
const GAP: u32 = 64;

/// The played sample a block renders: in memory, or kept on disk.
enum Src {
    Ram(Arc<Sample>),
    Disk(Arc<Played>),
}

impl Src {
    fn frames(&self) -> usize {
        match self {
            Src::Ram(s) => s.frames(),
            Src::Disk(d) => d.frames(),
        }
    }

    fn rate(&self) -> u32 {
        match self {
            Src::Ram(s) => s.rate,
            Src::Disk(d) => d.rate(),
        }
    }
}

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
    /// **(Lemon Studio)** The played sample kept on disk (Keep on disk), played in place of `sample` (`set_disk`).
    disk: Option<Arc<Played>>,
    /// Voices wait for frames from disk that are not in yet, instead of playing silence until they are: an offline
    /// render or an export.
    pub wait: bool,
    /// The played sample's loop (start and end frame, both inclusive), whether or not voices use it.
    pub looped: Option<(usize, usize)>,
    /// Frames of the played sample that the whole loaded file stands for: per-note starts from a host are fractions of
    /// the file (0 = the played sample's own length).
    pub file_frames: f64,
    /// How the resampler gets its weights: tables for live playback, the formula for an offline render.
    pub quality: Quality,
    rate: f32,
    voices: Vec<Voice>,
    /// The song's timing; the records' times in ticks, and the records, PPQ and tempo they were worked out for.
    time: Time,
    timing: [Timing; 5],
    timed: Option<([Record; 5], u16, f32)>,
    /// The global LFOs' shared phases, and the song tick they start from when the song starts playing.
    shared: Shared,
    start_at: Option<u64>,
    /// The current tick: its number (counted from 1), the samples left of it, its nominal length.
    serial: u64,
    left: u32,
    nominal: u32,
    /// The voices' own tick clock (another host): the running sum of samples per tick and the samples counted.
    clock: f64,
    counted: u64,
    /// Samples left of the `process` call (host ticks: the call is one tick).
    call: u32,
    /// The channel's volume (with its multiplier) and pan (with its offset), as the host hands them (`set_levels`).
    levels: (f32, f32),
    /// Stretchers of the realtime stretch voices (made when first needed, then reused).
    streams: Vec<Rt>,
    /// Host voices that ended since the last `take_ended`: (key, host note id).
    ended: Vec<(u8, Option<i32>)>,
}

pub const MAX_VOICES: usize = 128;

impl Engine {
    pub fn new(rate: f32) -> Self {
        Self::with_room(rate, MAX_VOICES)
    }

    /// An engine with room made for `voices` voices (it still plays up to `MAX_VOICES`): Lemon Drum Sampler keeps one
    /// per layer, most of them quiet.
    pub fn with_room(rate: f32, voices: usize) -> Self {
        let voices = voices.clamp(1, MAX_VOICES);
        Engine {
            sample: None,
            disk: None,
            wait: false,
            looped: None,
            file_frames: 0.0,
            quality: Quality::Table,
            rate,
            voices: Vec::with_capacity(voices),
            time: Time::default(),
            timing: Timing::all(&Record::new_channel_records(), 96, 130.0),
            timed: None,
            shared: Shared::from_tick(0, &[0; 5]),
            start_at: None,
            serial: 0,
            left: 0,
            nominal: 1,
            clock: 0.0,
            counted: 0,
            call: 0,
            levels: (std::f32::consts::SQRT_2, 0.0),
            streams: Vec::new(),
            ended: Vec::with_capacity(voices * 2),
        }
    }

    pub fn set_rate(&mut self, rate: f32) {
        if rate != self.rate {
            self.release_feeds();
            self.voices.clear();
            self.streams.clear();
            self.ended.clear();
        }
        self.rate = rate;
    }

    /// The channel's levels: its volume times its volume multiplier, and its pan plus its pan offset (spec:
    /// audio/voice-levels.md "The channel's levels"). Every voice's final levels take them from the next tick start
    /// on, sounding voices included ("Changes while a note sounds"). Without a call they are `sqrt(2)` (a single) and
    /// 0, for a host that puts the channel's levels on the output itself with a split of its own: a centred voice is
    /// then at its note's volume on both sides, and the host's split makes it the split of the sum of the pans.
    pub fn set_levels(&mut self, volume: f32, pan: f32) {
        self.levels = (volume, pan);
    }

    /// Drops every voice. Not reported: a host resetting the instrument forgets its voices itself.
    pub fn reset(&mut self) {
        self.release_feeds();
        self.voices.clear();
        self.streams.iter_mut().for_each(|r| r.busy = false);
        self.ended.clear();
    }

    /// The song's timing for the next `process` call, of `len` samples. With [`Time::host_ticks`] the call is one tick,
    /// which starts on its first sample. The global LFOs' shared phases start again when the song starts playing.
    pub fn begin(&mut self, len: usize, time: Time) {
        match (time.song_tick, self.time.song_tick) {
            (Some(t), None) => self.start_at = Some(t),
            (None, _) => self.start_at = None,
            _ => {}
        }
        self.time = time;
        if time.host_ticks && len > 0 {
            if self.left > 0 {
                // The last call ended inside its tick: the voices close it where it stands.
                for v in self.voices.iter_mut() {
                    end_tick(v);
                }
                self.left = 0;
            }
            self.call = len as u32;
        }
    }

    /// The records' times in ticks for the records, the PPQ and the tempo in force (spec: audio/envelopes-and-lfos.md
    /// "Time knobs": the tempo held as a single). Voices carry on from where they are.
    fn retime(&mut self, records: &[Record; 5]) {
        let key = (*records, self.time.ppq.max(1), self.time.tempo.max(1.0) as f32);
        if self.timed != Some(key) {
            self.timing = Timing::all(records, key.1, f64::from(key.2));
            self.timed = Some(key);
        }
    }

    /// A tick starts: its length (the rest of the `process` call with host ticks, or a nominal tick of it when the call
    /// is longer than a tick; else from the running sum of samples per tick, spec: audio/timing.md "The tick clock"),
    /// and the shared phases move on before any voice steps.
    fn start_tick(&mut self, b: &Block) {
        self.retime(&b.records);
        if let Some(t) = self.start_at.take() {
            self.shared = Shared::from_tick(t, &self.timing.map(|t| t.lfo_step()));
        }
        self.shared.tick(&self.timing);
        let spt = self.time.samples_per_tick(f64::from(self.rate));
        self.nominal = (spt.round_ties_even() as u32).max(1);
        self.left = if self.time.host_ticks && self.call > 0 {
            if self.call <= self.nominal + 1 {
                self.call
            } else {
                self.nominal
            }
        } else {
            self.clock += spt;
            let end = self.clock.round_ties_even().max(0.0) as u64;
            let len = end.saturating_sub(self.counted).max(1);
            self.counted += len;
            len as u32
        };
        self.serial += 1;
    }

    /// Host voices that ended since the last call, and host notes that started none: (key, host note id).
    pub fn take_ended(&mut self) -> std::vec::Drain<'_, (u8, Option<i32>)> {
        self.ended.drain(..)
    }

    /// Plays `disk` (a played sample kept on disk) from now on, or `sample` when `None`. Voices reading another file
    /// let go of their rings.
    pub fn set_disk(&mut self, disk: Option<Arc<Played>>) {
        let same = matches!((&self.disk, &disk), (Some(a), Some(b)) if Arc::ptr_eq(a.disk(), b.disk()));
        if !same {
            self.release_feeds();
        }
        self.disk = disk;
    }

    pub fn disk(&self) -> Option<&Arc<Played>> {
        self.disk.as_ref()
    }

    /// Every voice lets go of its ring.
    fn release_feeds(&mut self) {
        if let Some(d) = self.disk.clone() {
            for v in self.voices.iter_mut() {
                d.release(&mut v.feed);
            }
        }
    }

    /// Length and rate of the played sample.
    fn shape(&self) -> Option<(usize, u32)> {
        match (&self.disk, &self.sample) {
            (Some(d), _) => Some((d.frames(), d.rate())),
            (None, Some(s)) => Some((s.frames(), s.rate)),
            _ => None,
        }
    }

    /// A voice is over: its stretcher and its ring are free and a host voice's end is noted.
    fn end(&mut self, v: &Voice) {
        retire(v, &mut self.streams, self.disk.as_ref(), &mut self.ended);
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
    fn stretch_factor(b: &Block, (frames, rate): (usize, u32)) -> f64 {
        let ratio = match b.ts_time {
            Some(t) if frames > 0 => t / (frames as f64 / f64::from(rate.max(1))),
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
        let (frames, rate) = match self.shape() {
            Some((n, rate)) if zkey >= b.zone.0 && zkey <= b.zone.1 && n > 0 => (n, rate),
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
        // The played sample's rate times the reciprocal of the output rate (spec: audio/sampler.md "The ratio").
        let q = f64::from(rate) * (1.0 / f64::from(self.rate));
        let key_cents = ((i32::from(key) - b.root) * 100) as f32;
        // FL velocity byte = plugin velocity × 128; 100 = unity (spec: audio/sampler-playback.md).
        let vel = laws::velocity_gain((velocity.clamp(0.0, 1.0) * 128.0).round() as u8) as f32;
        let pos = (b.start_offset.clamp(0.0, 1.0) * frames as f64).floor();
        let open = if pos > 0.0 { open_len(&declick(b.declick), f64::from(self.rate)) } else { 0 };
        self.retime(&b.records);
        let art = Articulators::start(&b.records, &self.timing);
        // A volume envelope that starts in its delay or attack ramps the gain in from 0 (spec: audio/sampler.md "Gains").
        let ramp_in = art.envelope(VOL).out() == 0.0;
        // The realtime methods stretch a sample in memory (one kept on disk is decoded for them).
        let rt = if b.realtime && self.disk.is_none() { self.take_stream() } else { None };
        // From disk: START OFFSET's frames are held, unless the knob just moved (the frames there start coming now).
        let mut feed = Feed::default();
        if let Some(d) = &self.disk {
            d.start_at(&mut feed, pos, false);
        }
        self.voices.push(Voice {
            key,
            host,
            id,
            vel,
            pos,
            key_cents,
            q,
            tune: 0.0,
            note_pan: 0.0,
            mod_x: 0.0,
            mod_y: 0.0,
            age: 0,
            released: false,
            open,
            end: None,
            fades: false,
            step: None,
            close: None,
            close_next: false,
            art,
            ticks: 0,
            ramp_in,
            looped: false,
            filt: Filter::default(),
            gain: [0.0; 2],
            serial: 0,
            tick: TickState::DONE,
            rt,
            xf: 0,
            vol: None,
            feed,
            gap: 0,
        });
    }

    pub fn note_off(&mut self, key: u8, b: &Block) {
        self.retime(&b.records);
        let close = Close::new(&declick(b.declick), f64::from(self.rate));
        let vol_env = b.records[VOL].on;
        let looped = self.shape().is_some_and(|(n, _)| self.loop_frames(b, n).1.is_some());
        for v in self.voices.iter_mut().filter(|v| v.key == key && !v.released) {
            v.released = true;
            // With the volume envelope on, the envelopes go into their release and the voice closes when it has run
            // out; without it a looping voice closes at the note's end, and a one-shot voice ignores the end, its other
            // envelopes too (spec: audio/sampler.md "The end of a voice").
            if vol_env {
                v.art.release(&b.records, &self.timing);
                v.fades = true;
            } else if looped && v.close.is_none() {
                v.close = Some(close);
            }
        }
    }

    /// A note that has not played yet starts `extra` further in, on top of START OFFSET: a fraction of the loaded file,
    /// counted in the played sample's frames (spec §7: a trimmed audio clip; audio/precomputed-effects.md "Audio
    /// clips": a trim selects frames of the processed sample by the file's rate).
    pub fn start_later(&mut self, id: i32, extra: f32, b: &Block) {
        let Some((n, _)) = self.shape() else { return };
        let n = n as f64;
        let file = if self.file_frames > 0.0 { self.file_frames } else { n };
        let open = open_len(&declick(b.declick), f64::from(self.rate));
        for v in self.voices.iter_mut().filter(|v| v.id == Some(id) && v.age == 0) {
            let at = b.start_offset.clamp(0.0, 1.0) * n + f64::from(extra.max(0.0)) * file;
            v.pos = at.min(n).floor();
            v.open = if v.pos > 0.0 { open } else { 0 };
            // From disk: the frames there start coming now, when they are not held, and are held from the next
            // played sample on.
            if let Some(d) = &self.disk {
                d.start_at(&mut v.feed, v.pos, true);
            }
        }
    }

    /// A note that has not played yet ends at `frac` of the loaded file, on top of START OFFSET, in the played sample's
    /// frames: a trimmed audio clip's end frame, limited to the sample and at least where the voice starts. The voice
    /// closes there through the declick mode (spec §7, audio/audio-clips.md "What a clip plays").
    pub fn end_at(&mut self, id: i32, frac: f32, b: &Block) {
        let Some((n, _)) = self.shape() else { return };
        let n = n as f64;
        let file = if self.file_frames > 0.0 { self.file_frames } else { n };
        for v in self.voices.iter_mut().filter(|v| v.id == Some(id) && v.age == 0) {
            let at = b.start_offset.clamp(0.0, 1.0) * n + f64::from(frac.max(0.0)) * file;
            v.end = Some(at.round_ties_even().clamp(v.pos, n));
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

    /// Closes the voices of a note with the declick mode's closing, whatever their envelopes (the host's choke: an
    /// audio clip ends, its own voice by note id; a cut group, the voice limit; spec: audio/note-rules.md).
    pub fn choke(&mut self, id: Option<i32>, key: u8, b: &Block) {
        let close = Close::new(&declick(b.declick), f64::from(self.rate));
        for v in self.of_note(id, key).filter(|v| v.close.is_none()) {
            v.close = Some(close);
        }
    }

    /// The host's gain on a note's voices (CLAP volume expression: an audio clip's fades and gain, spec:
    /// audio/audio-clips.md "Fades and clip gain"). A voice takes the first at once and glides to each later one in a
    /// straight line over `VOICE_GAIN_FRAMES` frames, as the engine gives them: one every that many frames from a
    /// block's start. **(OS)** It multiplies the voice's output after the voice's own gains.
    pub fn volume(&mut self, id: Option<i32>, key: u8, gain: f32) {
        let (to, n) = (gain.max(0.0), VOICE_GAIN_FRAMES);
        for v in self.of_note(id, key) {
            v.vol = Some(match v.vol {
                None => HostGain { now: to, to, step: 0.0, left: 0 },
                Some(h) => HostGain { now: h.now, to, step: (to - h.now) / n as f32, left: n },
            });
        }
    }

    /// Ends every voice with a short fade.
    pub fn all_off(&mut self) {
        let close = Close::quick(f64::from(self.rate));
        for v in self.voices.iter_mut().filter(|v| v.close.is_none()) {
            v.close = Some(close);
        }
    }

    /// Adds the voices into `out_l` / `out_r`, newest voice first (spec: audio/sampler.md "A voice"), tick by tick.
    pub fn render(&mut self, out_l: &mut [f32], out_r: &mut [f32], b: &Block) {
        let len = out_l.len();
        let rate = f64::from(self.rate);
        let src = match (&self.disk, &self.sample) {
            (Some(d), _) => Some(Src::Disk(d.clone())),
            (None, Some(s)) => Some(Src::Ram(s.clone())),
            _ => None,
        };
        let src = src.filter(|s| s.frames() > 0);
        if src.is_none() {
            self.end_all();
        }
        let mut f = 0;
        while f < len {
            if self.left == 0 {
                self.start_tick(b);
            }
            let chunk = (self.left as usize).min(len - f);
            if let Some(s) = &src {
                // Resample plays the sample faster or slower; a realtime voice reads `speed` input frames per output
                // frame and its stretcher keeps the pitch.
                let factor = Self::stretch_factor(b, (s.frames(), s.rate()));
                let cx = Cx {
                    serial: self.serial,
                    left: self.left,
                    nominal: self.nominal,
                    rate,
                    limit: ((44100.0 / rate) as f32) * 0.001,
                    records: b.records,
                    timing: self.timing,
                    phases: self.shared.phases(&b.records),
                    rs: if b.realtime { 1.0 } else { 1.0 / factor },
                    speed: f64::from(s.rate()) / rate / factor,
                    lp: self.loop_frames(b, s.frames()),
                    dc: declick(b.declick),
                    quality: self.quality,
                    wait: self.wait,
                    levels: self.levels,
                };
                let mut i = self.voices.len();
                while i > 0 {
                    i -= 1;
                    let v = &mut self.voices[i];
                    let rt = match s {
                        Src::Ram(_) => v.rt.and_then(|k| self.streams.get_mut(usize::from(k))),
                        Src::Disk(_) => None,
                    };
                    if !render_voice(v, rt, s, b, &cx, &mut out_l[f..f + chunk], &mut out_r[f..f + chunk]) {
                        let v = self.voices.remove(i);
                        retire(&v, &mut self.streams, self.disk.as_ref(), &mut self.ended);
                    }
                }
            }
            self.left -= chunk as u32;
            self.call = self.call.saturating_sub(chunk as u32);
            f += chunk;
        }
        // The reader tops up the rings the block read from.
        if let Some(Src::Disk(d)) = &src {
            if self.voices.iter().any(|v| v.feed.active()) {
                d.disk().wake();
            }
        }
    }
}

/// A voice is over: its stretcher and its ring are free and a host voice's end is noted.
fn retire(v: &Voice, streams: &mut [Rt], disk: Option<&Arc<Played>>, ended: &mut Vec<(u8, Option<i32>)>) {
    if let Some(r) = v.rt.and_then(|i| streams.get_mut(usize::from(i))) {
        r.busy = false;
    }
    if let Some(d) = disk {
        d.release(&mut v.feed.clone());
    }
    if v.host {
        ended.push((v.key, v.id));
    }
}

/// What every voice of a tick shares.
struct Cx {
    /// The tick's number and the samples left of it.
    serial: u64,
    left: u32,
    /// The tick's nominal length (`Round(samples per tick)`) and the gains' step limit
    /// (`single(single(44100 / rate) * 0.001)`).
    nominal: u32,
    rate: f64,
    limit: f32,
    records: [Record; 5],
    timing: [Timing; 5],
    phases: [Option<u32>; 5],
    /// The resample stretch on the ratio, and a realtime voice's input frames per output frame.
    rs: f64,
    speed: f64,
    /// The loop: start and one past the end.
    lp: (f64, Option<f64>),
    dc: Declick,
    quality: Quality,
    wait: bool,
    /// The channel's volume and pan.
    levels: (f32, f32),
}

impl TickState {
    /// Between ticks, and before a voice's first.
    const DONE: TickState = TickState { len: 0, done: 0, ratio: 1.0, from: None, copy: false, ramp: [Ramp::FLAT; 2] };
}

impl Ramp {
    const FLAT: Ramp = Ramp { even: 0.0, odd: 0.0, step: 0.0, j: 0, pairs: 0 };
}

/// A voice's tick ends after `done` samples: each gain ends at `gain + step * done` (one product), the position at
/// `p + R * done` away from the edges, and the filter takes the tick's end (spec: audio/sampler.md "Gains", "How the
/// position moves"; audio/channel-filter.md).
fn end_tick(v: &mut Voice) {
    let t = v.tick;
    if t.len == 0 {
        return;
    }
    let d = t.done;
    for (g, r) in v.gain.iter_mut().zip(&t.ramp) {
        *g += r.step * d as f32;
    }
    if let Some(p) = t.from {
        v.pos = p + t.ratio * f64::from(d);
    }
    v.filt.end(d as usize);
    v.tick = TickState::DONE;
}

/// A voice's tick starts, `len` samples of it left (spec: audio/sampler.md "A voice", in its order): the envelopes and
/// LFOs step, the ratio is worked out, the filter takes its knob positions, and the gains get their targets and steps.
fn start_tick(v: &mut Voice, mut rt: Option<&mut Rt>, src: &Src, b: &Block, cx: &Cx, len: u32) {
    end_tick(v);
    // The volume envelope ran out in the last tick: the closing starts with this one's first sample.
    if v.close_next && v.close.is_none() {
        v.close = Some(Close::new(&cx.dc, cx.rate));
    }
    v.art.step(&cx.records, &cx.timing, &cx.phases);
    let parts = v.art.parts(&cx.records);
    if v.art.volume_ended(&cx.records) {
        v.close_next = true;
    }
    // The ratio: the voice's pitch and the envelope's and LFO's cents as one single (spec: audio/sampler.md "The
    // ratio"); a realtime voice's transposition (with the stretch's PITCH).
    let cents = (f64::from(v.key_cents) + b.cents + v.tune * 100.0 + b.ts_cents) as f32 + parts.pitch;
    let ratio = (f64::from(cents) * C).exp() * v.q * cx.rs;
    // A realtime voice plays the sample by resampling while there is nothing to stretch (the root key, no PITCH, MUL 1
    // or TIME the sample's own duration); its stretcher starts the first time there is, and stays. **(OS)**
    if let (Some(x), Src::Ram(s)) = (rt.as_mut(), src) {
        if !x.started && !nothing_to_stretch(src.frames() as f64, ratio, cx.speed, cx.rate) {
            x.start(s, v.pos, ratio, cx.speed, cx.lp);
            v.xf = if v.age > 0 { SWITCH } else { 0 };
        }
    }
    let stretching = rt.as_ref().is_some_and(|x| x.started);
    // The filter's knob positions: the knob plus the note's offset, then the envelope and LFO part (spec:
    // audio/channel-filter.md "Knob positions"; the host hands the note's mod X with the channel's offset in it).
    let x = (b.f_cut + v.mod_x) + parts.mod_x;
    let y = (b.f_res + v.mod_y) + parts.mod_y;
    let corr = v.filt.tick(b.f_type, x, y, cx.nominal as usize, cx.rate);
    // The gains: the pan split of the final volume, times the filter's correction (type 5). The final levels are the
    // note's with the channel's (spec: audio/voice-levels.md "Final levels"): `note volume * channel volume`,
    // `note pan + channel pan`. The first tick starts at the plain gain, or at 0 when the volume envelope starts in
    // its delay or attack.
    let (volume, pan) = cx.levels;
    let plain = split((v.note_pan as f32 + pan) + parts.pan, (v.vel * volume) * parts.volume);
    if v.ticks == 0 {
        v.gain = if v.ramp_in { [0.0; 2] } else { plain };
    }
    v.ticks += 1;
    let gain = v.gain;
    let ramp = |k: usize| Ramp::new(gain[k], glide_step(gain[k], plain[k] * corr, cx.nominal, cx.limit), len);
    // Near an edge of the sample or of its loop the positions are summed sample by sample.
    let (ls, le) = cx.lp;
    let n = resample::half_width(ratio) as f64;
    let first = if v.looped { ls } else { 0.0 };
    let end = le.unwrap_or_else(|| v.end.unwrap_or(src.frames() as f64));
    let near = v.pos.round_ties_even() < first + n + 2.0 || v.pos + ratio * f64::from(len) >= end - n - 3.0;
    // The fades are worked on from the tick at whose end the position is at or past the end frame (spec:
    // audio/declick.md "When the opening applies").
    let advance = if stretching { cx.speed } else { ratio };
    if le.is_none() && v.pos + advance * f64::from(len) >= end {
        v.fades = true;
    }
    let from = (!near && !stretching).then_some(v.pos);
    let copy = from.is_some() && ratio == 1.0 && (v.pos as f32).fract() == 0.0 && matches!(src, Src::Ram(_));
    v.tick = TickState { len, done: 0, ratio, from, copy, ramp: [ramp(0), ramp(1)] };
}

/// Plays the next `out_l.len()` samples of the voice's current tick, starting the tick when it has not yet. False once
/// the voice is over.
fn render_voice(v: &mut Voice, mut rt: Option<&mut Rt>, src: &Src, b: &Block, cx: &Cx, out_l: &mut [f32], out_r: &mut [f32]) -> bool {
    if v.serial != cx.serial {
        start_tick(v, rt.as_deref_mut(), src, b, cx, cx.left);
        v.serial = cx.serial;
    }
    let n = src.frames() as f64;
    // A trimmed audio clip ends at its end frame. Without bleeding the frames past it read the last kept frame, as at
    // the end of the sample; with it the voice plays on into the sample under the closing (spec: audio/audio-clips.md
    // "What a clip plays", audio/declick.md "When the closing starts").
    let end = v.end.unwrap_or(n);
    let kept = if cx.dc.bleed { src.frames() } else { (end as usize).max(1) };
    let (ls, le) = cx.lp;
    let stretching = rt.as_ref().is_some_and(|x| x.started);
    let ratio = v.tick.ratio;
    // What is heard moves on by the ratio, or by `speed` in a stretching voice.
    let advance = if stretching { cx.speed } else { ratio };
    for j in 0..out_l.len() {
        let k = v.tick.done;
        let pos = match v.tick.from {
            Some(p) => p + f64::from(k) * ratio,
            None => {
                // The loop: past its end the position goes back by the loop's length (spec: audio/sampler.md "In a
                // sustain loop").
                if let Some(le) = le {
                    while v.pos >= le {
                        v.pos -= le - ls;
                        v.looped = true;
                    }
                }
                v.pos
            }
        };
        if pos < 0.0 {
            return false;
        }
        // The end of the sample or of a trimmed clip: the closing starts on the first sample at or past it (spec:
        // audio/declick.md "When the closing starts"). A clip whose start and end frame coincide plays nothing.
        if pos >= end && v.close.is_none() {
            if v.age == 0 && v.end.is_some() {
                return false;
            }
            v.close = Some(Close::new(&cx.dc, cx.rate));
            v.fades = true;
        }
        let (mut l, mut r) = match (rt.as_mut(), src) {
            (Some(x), Src::Ram(s)) if stretching => {
                let (a, b) = x.frame(s, ratio, cx.speed, cx.lp);
                if v.xf > 0 {
                    let (pa, pb) = resample::read(&s.left, &s.right, pos, cx.speed, cx.quality);
                    let u = v.xf as f32 / SWITCH as f32;
                    v.xf -= 1;
                    (a + (pa - a) * u, b + (pb - b) * u)
                } else {
                    (a, b)
                }
            }
            // A ratio of exactly 1 from a whole position hands the frames on unchanged (spec: audio/sampler.md "The
            // copy").
            (_, Src::Ram(s)) if v.tick.copy => {
                let i = pos.round_ties_even() as usize;
                (s.left[i], s.right[i])
            }
            (_, Src::Ram(s)) => resample::read(&s.left[..kept], &s.right[..kept], pos, ratio, cx.quality),
            // From disk: the same frames read the same way. While its frames are late (live playback) the voice is
            // silent and keeps time (the frames are skipped); it fades back in once they are there.
            (_, Src::Disk(d)) => match d.read(&mut v.feed, pos, ratio, cx.quality, cx.wait) {
                Some((a, b)) if v.gap > 0 => {
                    let g = 1.0 - v.gap as f32 / GAP as f32;
                    v.gap -= 1;
                    (a * g, b * g)
                }
                Some(x) => x,
                None => {
                    v.gap = GAP;
                    (0.0, 0.0)
                }
            },
        };
        l = v.filt.run(0, k as usize, l);
        r = v.filt.run(1, k as usize, r);
        // The fades act on the signal before the gains (spec: audio/declick.md "Where the fades sit"). FL works on a
        // voice's fades only from the tick in which it reaches its end or is released, so an opening shows only on a
        // voice that gets there while it is younger than its opening ("When the opening applies").
        if (v.fades || v.close.is_some()) && v.age < u64::from(v.open) {
            if cx.dc.step_removal {
                // Modes 1 and 2 take the voice's first sample off the signal along a falling raised cosine, when the
                // fades are worked on from that sample on ("Step removal").
                if v.age == 0 {
                    v.step = Some((l, r));
                }
                if let Some((fl, fr)) = v.step {
                    let h = step_gain(v.age as u32, v.open);
                    (l, r) = (l - fl * h, r - fr * h);
                }
            } else {
                let o = fade_gain(v.open - v.age as u32, v.open, cx.dc.tension);
                (l, r) = (l * o, r * o);
            }
        }
        if let Some(c) = v.close.as_mut() {
            match c.apply(l, r, cx.rate) {
                Some(x) => (l, r) = x,
                None => return false,
            }
        }
        // `out = (resampled * fade) * gain`, each product a single; an audio clip's gain from the host after that.
        let (mut ol, mut or) = (l * v.tick.ramp[0].next(), r * v.tick.ramp[1].next());
        if let Some(h) = v.vol.as_mut() {
            let h = h.next();
            (ol, or) = (ol * h, or * h);
        }
        out_l[j] += ol;
        out_r[j] += or;
        if v.tick.from.is_none() {
            v.pos += advance;
        }
        v.age += 1;
        v.tick.done += 1;
    }
    if v.tick.done == v.tick.len {
        end_tick(v);
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

    /// A tick's length in the filter's tests.
    const CTL: usize = 32;

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
        e.choke(None, 60, &b2);
        for _ in 0..2 {
            l.fill(0.0);
            e.render(&mut l, &mut r, &b2);
        }
        assert!(e.has_voices(), "20 ms not over after 10");
        e.render(&mut l, &mut r, &b2);
        e.render(&mut l, &mut r, &b2);
        assert!(!e.has_voices());
    }

    // spec: audio/audio-clips.md "Fades and clip gain", "Automatic crossfades"
    /// The host's gain on a voice (two audio clips of one sample on one key): the first a voice gets it takes at once,
    /// each later one it reaches in a straight line over `VOICE_GAIN_FRAMES` frames, and the key's other voice keeps
    /// its own; a choke by note id closes that voice alone.
    #[test]
    fn host_gains_and_chokes_reach_one_voice() {
        let mut e = Engine::new(48000.0);
        e.sample = Some(Arc::new(Sample { rate: 48000, left: vec![0.5; 48000], right: vec![0.5; 48000] }));
        let b = Block::default();
        e.note_on_id(60, 100.0 / 128.0, Some(1), &b);
        e.note_on_id(60, 100.0 / 128.0, Some(2), &b);
        e.volume(Some(1), 60, 0.0);
        e.volume(Some(1), 60, 1.0);
        e.volume(Some(2), 60, 0.5);
        let n = VOICE_GAIN_FRAMES as usize;
        let (mut l, mut r) = (vec![0.0; 2 * n], vec![0.0; 2 * n]);
        e.render(&mut l, &mut r, &b);
        for (j, v) in l.iter().enumerate() {
            let want = 0.5 * 0.5 + 0.5 * j.min(n) as f32 / n as f32;
            assert!((v - want).abs() < 1e-5, "frame {j}: {v} vs {want}");
        }
        e.choke(Some(1), 60, &b);
        let (mut l, mut r) = (vec![0.0; 1440], vec![0.0; 1440]);
        e.render(&mut l, &mut r, &b);
        assert!(l[960..].iter().all(|v| (v - 0.25).abs() < 1e-6), "the other voice plays on: {}", l[1439]);
        assert_eq!(e.voices.iter().map(|v| v.id).collect::<Vec<_>>(), vec![Some(2)]);
    }

    /// A voice of note id 1 started `start` and ended `end` of the way into `s` (a trimmed audio clip), rendered for
    /// `len` samples.
    fn clip(s: &Sample, mode: i32, start: f32, end: f32, len: usize) -> (Engine, Vec<f32>) {
        let mut e = Engine::new(48000.0);
        e.sample = Some(Arc::new(s.clone()));
        let b = Block { declick: mode, ..Block::default() };
        e.note_on_id(60, 100.0 / 128.0, Some(1), &b);
        e.start_later(1, start, &b);
        e.end_at(1, end, &b);
        let (mut l, mut r) = (vec![0.0; len], vec![0.0; len]);
        e.render(&mut l, &mut r, &b);
        (e, l)
    }

    // spec: audio/audio-clips.md "What a clip plays", audio/declick.md "When the closing starts": a trimmed clip closes
    // on the first sample at or past its end frame. Under the closing it holds the last kept frame without bleeding
    // (mode 0) and plays on into the sample with it (mode 3).
    #[test]
    fn a_trimmed_voice_closes_at_its_end_frame() {
        let s = ramp(4800);
        for (mode, bleed) in [(0, false), (3, true)] {
            let (e, l) = clip(&s, mode, 0.25, 0.75, 4800);
            for j in [0, 600, 2399] {
                assert!((l[j] - s.left[1200 + j]).abs() < 1e-5, "mode {mode}, sample {j}: {}", l[j]);
            }
            let n = fade_len(declick(mode).close_ms, 48000.0);
            for i in 0..n {
                let from = if bleed { s.left[3600 + i as usize] } else { s.left[3599] };
                let want = from * fade_gain(i + 1, n, 0.0);
                let got = l[2400 + i as usize];
                assert!((got - want).abs() < 1e-5, "mode {mode}, closing sample {i}: {got} vs {want}");
            }
            assert!(l[2400 + n as usize..].iter().all(|v| *v == 0.0), "mode {mode}: over after the closing");
            assert!(!e.has_voices());
        }
        // A clip whose start and end frame coincide plays nothing.
        let (e, l) = clip(&s, 3, 0.25, 0.25, 480);
        assert!(l.iter().all(|v| *v == 0.0) && !e.has_voices());
    }

    // spec: audio/declick.md "Step removal", "When the opening applies": a clip in mode 1 or 2 that ends in its first
    // tick has its first sample taken off along a falling raised cosine over 4 ms; one that ends in a later tick keeps
    // its start.
    #[test]
    fn step_removal_opens_a_clip_that_ends_in_its_first_tick() {
        let s = Sample { rate: 48000, left: vec![0.5; 4800], right: vec![0.5; 4800] };
        let n = fade_len(4.0, 48000.0);
        assert_eq!(n, 192);
        assert!(step_gain(0, n) > 0.9999 && step_gain(n - 1, n) < 1e-4);
        for mode in [1, 2] {
            // From frame 480 to 528: 48 samples, within the first tick (230.8 samples).
            let (_, l) = clip(&s, mode, 0.1, 0.11, 960);
            assert!(l[0].abs() < 1e-4, "mode {mode}: starts from zero: {}", l[0]);
            for i in 0..48 {
                let want = 0.5 - 0.5 * step_gain(i, n);
                assert!((l[i as usize] - want).abs() < 1e-5, "mode {mode}, sample {i}: {} vs {want}", l[i as usize]);
            }
            let (_, l) = clip(&s, mode, 0.1, 0.5, 960);
            assert!((l[0] - 0.5).abs() < 1e-5 && (l[100] - 0.5).abs() < 1e-5, "mode {mode}: {} {}", l[0], l[100]);
        }
        // Modes 0 and 6 have no opening.
        for mode in [0, 6] {
            let (_, l) = clip(&s, mode, 0.1, 0.11, 960);
            assert!((l[0] - 0.5).abs() < 1e-5, "mode {mode}: {}", l[0]);
        }
    }

    // spec: audio/declick.md "When the opening applies": the fades are worked on from the tick in which the voice
    // reaches its end, so a clip shorter than its gain-fade opening shows the opening's later part in its last tick
    // only, and the opening runs on under the closing.
    #[test]
    fn a_short_clip_opens_in_the_tick_it_ends_in() {
        let s = Sample { rate: 48000, left: vec![0.5; 4800], right: vec![0.5; 4800] };
        // Frames 480 to 1080: 600 samples in mode 3, whose opening is 960.
        let (_, l) = clip(&s, 3, 0.1, 0.225, 1600);
        let spt = Time::default().samples_per_tick(48000.0);
        let ends = (1..).map(|k| (k as f64 * spt).round_ties_even() as usize);
        let last = ends.take_while(|&e| 480 + e < 1080).last().unwrap_or(0);
        assert_eq!(last, 462);
        let open = |age: usize| fade_gain(960 - age as u32, 960, 0.0);
        assert!((l[last - 1] - 0.5).abs() < 1e-5, "untouched before the tick: {}", l[last - 1]);
        for age in [last, 500, 599] {
            assert!((l[age] - 0.5 * open(age)).abs() < 1e-5, "sample {age}: {}", l[age]);
        }
        let want = 0.5 * open(600) * fade_gain(1, 960, 0.0);
        assert!((l[600] - want).abs() < 1e-5, "the closing's first sample: {} vs {want}", l[600]);
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

    /// Host ticks of the given lengths (Lemon Studio's engine: one tick per call), at 130 BPM and 96 PPQ; the left
    /// side of each.
    fn host(e: &mut Engine, b: &Block, lens: &[usize]) -> Vec<Vec<f32>> {
        let time = Time { host_ticks: true, ..Time::default() };
        lens.iter()
            .map(|&len| {
                let (mut l, mut r) = (vec![0.0; len], vec![0.0; len]);
                e.begin(len, time);
                e.render(&mut l, &mut r, b);
                l
            })
            .collect()
    }

    /// A 44100 Hz sample of `n` frames that changes from frame to frame.
    fn wobble(n: usize) -> Sample {
        let left: Vec<f32> = (0..n).map(|i| ((i as f64 * 0.01).sin() * 0.5) as f32).collect();
        Sample { rate: 44100, right: left.clone(), left }
    }

    // spec: audio/timing.md "Samples per tick", "The tick clock": without host ticks the voices count ticks of
    // `Round(T)` from their first call, whatever the calls' lengths.
    #[test]
    fn the_voices_count_ticks_by_the_running_sum() {
        let mut e = Engine::new(44100.0);
        let time = Time { tempo: 185.0, ..Time::default() };
        assert!((time.samples_per_tick(44100.0) - 148.986).abs() < 1e-3);
        let b = Block::default();
        let (mut l, mut r) = (vec![0.0; 100], vec![0.0; 100]);
        let mut done = 0;
        let ten = (10.0 * time.samples_per_tick(44100.0)).round_ties_even() as usize;
        while done < ten {
            let n = 100.min(ten - done);
            e.begin(n, time);
            e.render(&mut l[..n], &mut r[..n], &b);
            done += n;
        }
        assert_eq!((e.serial, e.left, e.nominal), (10, 0, 149));
    }

    // Lemon Studio's engine calls once per tick: every call is a tick, and a call longer than a tick is cut into
    // nominal ticks. **(OS)**
    #[test]
    fn host_calls_are_ticks() {
        let mut e = Engine::new(44100.0);
        e.sample = Some(Arc::new(wobble(44100)));
        let b = Block::default();
        e.note_on(60, 100.0 / 128.0, &b);
        // 44100 * 60 / (130 * 96) = 212.0 samples per tick.
        host(&mut e, &b, &[150, 213, 1, 212]);
        assert_eq!((e.serial, e.voices[0].ticks), (4, 4));
        host(&mut e, &b, &[1000]);
        assert_eq!(e.serial, 9, "212 * 4 + 152");
    }

    // spec: audio/sampler.md "The ratio": `exp(cents * ln 2 / 1200) * q`, constant within the tick; three octaves up
    // falls one double short of 8.
    #[test]
    fn the_ratio_is_an_exponential_of_the_cents() {
        let mut e = Engine::new(44100.0);
        e.sample = Some(Arc::new(wobble(44100)));
        let b = Block::default();
        let (mut l, mut r) = (vec![0.0; 10], vec![0.0; 10]);
        for (key, want) in [(60, 1.0), (72, 2.0), (96, 7.999_999_999_999_998)] {
            e.reset();
            e.note_on(key, 0.5, &b);
            e.render(&mut l, &mut r, &b);
            assert_eq!(e.voices[0].tick.ratio, want, "key {key}");
        }
    }

    // spec: audio/sampler.md "Gains": a gain moves by at most `single(single(44100 / rate) * 0.001)` per sample
    // towards its target, in a step worked out from the tick's nominal length.
    #[test]
    fn gains_glide_by_a_limited_step() {
        assert_eq!(glide_step(0.0, 0.1, 149, 0.001), 0.1 / 149.0);
        assert_eq!(glide_step(0.0, 1.0, 149, 0.001), 0.001);
        assert_eq!(glide_step(1.0, 0.0, 149, 0.001), -0.001);
        let limit = ((44100.0 / 48000.0) as f32) * 0.001;
        assert_eq!(limit, 0.000_918_75);
    }

    // spec: audio/sampler.md "Gains": within a tick the even samples run from the gain, the odd ones from the gain plus
    // a step, both by two steps, for the first `L - L mod 8` samples; the rest go on from the even series by single
    // steps.
    #[test]
    fn a_tick_gain_is_built_by_interleaved_sums() {
        let (g, step) = (0.3f32, 1.234_5e-4f32);
        for len in [151u32, 152] {
            let mut r = Ramp::new(g, step, len);
            let got: Vec<f32> = (0..len).map(|_| r.next()).collect();
            assert_eq!(got[..3], [g, g + step, g + step * 2.0]);
            let (mut even, mut odd) = (g, g + step);
            let pairs = (len - len % 8) as usize;
            for (j, &x) in got[..pairs].iter().enumerate() {
                let s = if j % 2 == 0 { &mut even } else { &mut odd };
                assert_eq!(x, *s, "tick of {len}, sample {j}");
                *s += step * 2.0;
            }
            for &x in &got[pairs..] {
                assert_eq!(x, even);
                even += step;
            }
            // The two series round differently from one running sum.
            let mut sum = g;
            let plain: Vec<f32> = (0..len)
                .map(|_| {
                    let x = sum;
                    sum += step;
                    x
                })
                .collect();
            assert_ne!(got, plain);
        }
    }

    // spec: audio/sampler.md "The copy": at a ratio of exactly 1, from a whole position away from the edges, a tick's
    // frames are handed on unchanged. The voice's first tick, near the sample's start, goes through the weights.
    #[test]
    fn a_ratio_of_one_copies_the_frames() {
        let mut e = Engine::new(44100.0);
        let s = wobble(44100);
        e.sample = Some(Arc::new(s.clone()));
        let b = Block::default();
        // Velocity 100 and the centred split give the voice a gain of exactly 1.
        e.note_on(60, 100.0 / 128.0, &b);
        let (mut l, mut r) = (vec![0.0; 212], vec![0.0; 212]);
        e.begin(212, Time { host_ticks: true, ..Time::default() });
        e.render(&mut l[..10], &mut r[..10], &b);
        assert!(!e.voices[0].tick.copy);
        e.render(&mut l[10..], &mut r[10..], &b);
        let out = host(&mut e, &b, &[212]);
        assert_eq!(out[0], s.left[212..424]);
        let v = &e.voices[0];
        assert_eq!((v.tick.len, v.pos), (0, 424.0), "the position moved on by R * L");
    }

    // spec: audio/envelopes-and-lfos.md "State and timing", audio/sampler.md "Gains", "The end of a voice": a volume
    // envelope that starts in its attack ramps the gain in from 0; released, it runs its release, and the voice closes
    // after the envelope has run out.
    #[test]
    fn the_volume_envelope_steps_per_tick() {
        let mut e = Engine::new(44100.0);
        e.sample = Some(Arc::new(Sample { rate: 44100, left: vec![0.5; 441000], right: vec![0.5; 441000] }));
        let mut b = Block::default();
        let vol = &mut b.records[VOL];
        (vol.on, vol.attack, vol.release) = (true, 1000, 1000);
        e.note_on(60, 100.0 / 128.0, &b);
        let out = host(&mut e, &b, &[212; 40]);
        assert_eq!(out[0][0], 0.0, "the gain starts at 0");
        // Steps of at most 0.001 per sample (on a signal of 0.5). A tick ends at `gain + step * L`, a product, not the
        // running sums: a step at a tick's end may be a little longer.
        let all: Vec<f32> = out.concat();
        assert!(all.windows(2).all(|w| (w[1] - w[0]).abs() <= 0.5 * 0.001 * 1.01));
        assert!(out[39][0] > 0.1, "{}", out[39][0]);
        e.note_off(60, &b);
        let mut ticks = 0;
        while e.has_voices() {
            host(&mut e, &b, &[212]);
            ticks += 1;
            assert!(ticks < 200);
        }
        assert!(ticks > 2, "the release plays out first: {ticks}");
    }

    // spec: audio/envelopes-and-lfos.md "The global LFO": with the flag every voice of the channel reads one phase,
    // which moves on every tick of the song from the tick playback starts on; without it each voice has its own.
    #[test]
    fn the_global_lfo_shares_one_phase() {
        for global in [true, false] {
            let mut e = Engine::new(44100.0);
            e.sample = Some(Arc::new(wobble(441000)));
            let mut b = Block::default();
            let p = &mut b.records[PITCH];
            (p.lfo_amount, p.lfo_delay, p.lfo_attack) = (100, 0, 100);
            p.flags = if global { Record::GLOBAL_LFO } else { 0 };
            let time = Time { host_ticks: true, song_tick: Some(0), ..Time::default() };
            let tick = |e: &mut Engine| {
                let (mut l, mut r) = (vec![0.0; 212], vec![0.0; 212]);
                e.begin(212, time);
                e.render(&mut l, &mut r, &b);
            };
            e.note_on(60, 0.5, &b);
            for _ in 0..30 {
                tick(&mut e);
            }
            e.note_on(64, 0.5, &b);
            tick(&mut e);
            let (old, new) = (e.voices[0].art.lfo(PITCH).value(), e.voices[1].art.lfo(PITCH).value());
            assert_eq!(old == new, global, "{old} {new}");
        }
    }

    // spec: audio/voice-levels.md "Final levels", "Changes while a note sounds"; audio/sampler.md "Gains": the channel's
    // volume multiplies the note's and its pan adds to the note's in the voice's gains; a change reaches a sounding
    // voice at the next tick start, through the limited gain ramp.
    #[test]
    fn the_channel_levels_are_part_of_the_voice() {
        let mut e = Engine::new(44100.0);
        e.sample = Some(Arc::new(Sample { rate: 44100, left: vec![0.5; 44100], right: vec![0.5; 44100] }));
        let b = Block::default();
        e.set_levels(0.5, 0.25);
        e.note_on(60, 100.0 / 128.0, &b);
        host(&mut e, &b, &[212]);
        let at = split(0.25, e.voices[0].vel * 0.5);
        assert_eq!(e.voices[0].gain, at, "the first tick starts at the plain gain");
        e.set_levels(1.0, 0.25);
        host(&mut e, &b, &[212]);
        let target = split(0.25, e.voices[0].vel);
        for k in 0..2 {
            assert!(target[k] - at[k] > 0.212, "{target:?} {at:?}");
            assert!((e.voices[0].gain[k] - (at[k] + 0.001 * 212.0)).abs() < 1e-5, "one tick at the limit");
        }
        host(&mut e, &b, &[212; 4]);
        for k in 0..2 {
            assert!((e.voices[0].gain[k] - target[k]).abs() < 1e-6, "arrived");
        }
    }
}
