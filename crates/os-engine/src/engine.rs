//! Block-based playback engine: schedules notes from patterns / playlist clips, plays sampler
//! and audio-clip channels, and sums them through the mixer routing graph.
//!
// spec: audio/sampler-playback.md, audio/gain-laws.md, flp/events/playlist.md (clips),
//       flp/events/patterns.md (notes), flp/events/mixer.md (routing)

use std::sync::Arc;

use os_model::{ChannelKind, ClipSource, Project};

use crate::effects::Effect;
use crate::laws;
use crate::sample::{Sample, SampleLibrary};

/// What to play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayMode {
    /// The first arrangement's playlist. Falls back to the current pattern if it has no clips.
    Song,
    /// One pattern (1-based), from its start.
    Pattern(u16),
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Trigger {
    frame: u64,
    channel: usize,
    rate: f64,
    /// Output frame at which the voice is cut (end of an audio clip), if any.
    cut: Option<u64>,
    /// Per-note gain (channel volume × velocity × pan).
    gain: (f32, f32),
    /// Note key and the frame where the note ends (for showing sounding keys).
    key: u16,
    end: u64,
}

#[derive(Debug, Clone)]
struct ChannelState {
    sample: Option<Arc<Sample>>,
    /// Channel volume gain (0 when the channel is muted).
    volume: f64,
    /// Channel pan, −1..=1.
    pan: f64,
    insert: usize,
    /// Sample-rate conversion × channel pitch.
    base_rate: f64,
}

#[derive(Debug, Clone, Copy)]
struct Voice {
    channel: usize,
    pos: f64,
    step: f64,
    /// Output frames left before the voice is cut; `None` = play the whole sample.
    remaining: Option<u64>,
    gain: (f32, f32),
}


struct MixTrack {
    /// Insert effects in slot order (applied before the fader).
    effects: Vec<Box<dyn Effect>>,
    /// Fader × pan as `[[L←L, L←R], [R←L, R←R]]`.
    matrix: [[f32; 2]; 2],
    /// (destination, send gain). Sidechain-only routes (level 0) are dropped.
    routes: Vec<(usize, f32)>,
}

/// Things the engine skipped, so callers can report them.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EngineReport {
    pub plugin_channels_silent: usize,
    pub automation_clips_ignored: usize,
    pub effects_bypassed: usize,
    pub effects_active: usize,
    pub missing_samples: Vec<String>,
}

pub struct Engine {
    rate: u32,
    channels: Vec<ChannelState>,
    tracks: Vec<MixTrack>,
    /// Mixer tracks in processing order: every track before the tracks it sends to.
    order: Vec<usize>,
    triggers: Vec<Trigger>,
    next_trigger: usize,
    /// Notes started and not yet past their length: (channel, key, end frame).
    sounding: Vec<(usize, u16, u64)>,
    voices: Vec<Voice>,
    frame: u64,
    /// Frame where the scheduled material ends (voices may ring past it).
    end_frame: u64,
    bus: Vec<Vec<f32>>,
    pub report: EngineReport,
}

impl Engine {
    pub fn new(project: &Project, library: &mut SampleLibrary, rate: u32, mode: PlayMode) -> Self {
        let mut report = EngineReport::default();
        let tempo = project.tempo.unwrap_or(140.0);
        let ppq = project.ppq.max(1);
        let n_tracks = project.mixer.len().max(1);

        let channels: Vec<ChannelState> = project
            .channels
            .iter()
            .map(|ch| {
                let plays_sample = matches!(ch.kind, ChannelKind::Sampler | ChannelKind::AudioClip);
                if ch.kind == ChannelKind::Plugin {
                    report.plugin_channels_silent += 1;
                }
                let sample = if plays_sample {
                    match ch.sample_path.as_deref() {
                        Some(p) => {
                            let s = library.get(p);
                            if s.is_none() {
                                report.missing_samples.push(p.to_string());
                            }
                            s
                        }
                        // OpenStudio choice: an empty sampler channel plays a built-in tone so new
                        // projects make sound (FL's empty sampler is silent).
                        None if ch.kind == ChannelKind::Sampler => Some(fallback_tone(rate)),
                        None => None,
                    }
                } else {
                    None
                };
                let volume = if ch.enabled { laws::channel_gain(ch.volume) } else { 0.0 };
                let base_rate = sample.as_ref().map_or(1.0, |s| f64::from(s.rate) / f64::from(rate))
                    * laws::playback_rate(60, ch.pitch_cents);
                let insert = usize::from(ch.mixer_track).min(n_tracks - 1);
                ChannelState { sample, volume, pan: ch.pan, insert, base_rate }
            })
            .collect();

        let tracks: Vec<MixTrack> = if project.mixer.is_empty() {
            vec![MixTrack { effects: Vec::new(), matrix: [[1.0, 0.0], [0.0, 1.0]], routes: vec![] }]
        } else {
            project
                .mixer
                .iter()
                .map(|m| {
                    let mut effects = Vec::new();
                    for slot in m.slots.iter().flatten() {
                        match crate::effects::from_slot(&slot.name, &slot.state, rate) {
                            Some(fx) => {
                                effects.push(fx);
                                report.effects_active += 1;
                            }
                            None => report.effects_bypassed += 1,
                        }
                    }
                    let g = if m.enabled { m.volume.map_or(1.0, laws::fader_gain) } else { 0.0 };
                    let pan = laws::mixer_pan(m.pan.unwrap_or(0.0));
                    let matrix = pan.map(|row| row.map(|x| (x * g) as f32));
                    let routes = if m.index == 0 {
                        vec![]
                    } else {
                        m.routes
                            .iter()
                            .filter(|&&r| usize::from(r) < n_tracks && r != m.index)
                            .map(|&r| {
                                let level = m.send_levels.iter().find(|(d, _)| *d == r).map(|&(_, l)| l);
                                (usize::from(r), level.map_or(1.0, laws::fader_gain) as f32)
                            })
                            .filter(|&(_, g)| g > 0.0)
                            .collect()
                    };
                    MixTrack { effects, matrix, routes }
                })
                .collect()
        };
        let order = processing_order(&tracks);

        let (mut triggers, end_tick) = schedule(project, mode, &mut report);
        triggers.sort_by(|a, b| a.tick.total_cmp(&b.tick));
        let triggers = triggers
            .into_iter()
            .filter_map(|n: Sched| {
                let state = channels.get(n.channel)?;
                state.sample.as_ref()?;
                // spec: audio/sampler-playback.md (velocity multiplies volume; pans add, clamped)
                let g = state.volume * laws::velocity_gain(n.velocity);
                let (pl, pr) = laws::channel_pan(state.pan + laws::note_pan(n.pan));
                // A plain step (length 0) counts as one step long.
                let len = if n.len > 0.0 { n.len } else { f64::from(ppq) / 4.0 };
                Some(Trigger {
                    key: n.key,
                    end: laws::tick_to_frame(n.tick + len, ppq, tempo, rate),
                    frame: laws::tick_to_frame(n.tick, ppq, tempo, rate),
                    channel: n.channel,
                    rate: state.base_rate * laws::playback_rate(n.key, 0),
                    cut: n.cut.map(|c| laws::tick_to_frame(c, ppq, tempo, rate)),
                    gain: ((g * pl) as f32, (g * pr) as f32),
                })
            })
            .collect();

        Engine {
            rate,
            channels,
            bus: vec![Vec::new(); tracks.len()],
            tracks,
            order,
            triggers,
            next_trigger: 0,
            sounding: Vec::with_capacity(256),
            voices: Vec::with_capacity(1024),
            frame: 0,
            end_frame: laws::tick_to_frame(end_tick, ppq, tempo, rate),
            report,
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    /// Frames rendered so far.
    pub fn position(&self) -> u64 {
        self.frame
    }

    /// Length of the scheduled material in frames (voices may ring past it).
    pub fn length(&self) -> u64 {
        self.end_frame
    }

    /// True once the scheduled material has played (voices may still ring): the loop point of a
    /// pattern.
    pub fn past_end(&self) -> bool {
        self.frame >= self.end_frame && self.next_trigger >= self.triggers.len()
    }

    /// True once all scheduled material has played and every voice has finished.
    pub fn finished(&self) -> bool {
        self.frame >= self.end_frame && self.next_trigger >= self.triggers.len() && self.voices.is_empty()
    }

    /// Take over playback from `old` (built from an earlier version of the project) at its
    /// position: ringing voices keep playing, notes before the position are not retriggered, and
    /// effect states carry over when the mixer layout is unchanged. Edits are heard immediately.
    pub fn continue_from(&mut self, old: &mut Engine) {
        self.frame = old.frame;
        self.next_trigger = self.triggers.partition_point(|t| t.frame < old.frame);
        let n = self.channels.len();
        self.voices = std::mem::take(&mut old.voices);
        self.voices.retain(|v| v.channel < n);
        self.sounding = std::mem::take(&mut old.sounding);
        if self.rate == old.rate && self.tracks.len() == old.tracks.len() {
            for (new, o) in self.tracks.iter_mut().zip(&mut old.tracks) {
                if new.effects.len() == o.effects.len() {
                    new.effects = std::mem::take(&mut o.effects);
                }
            }
        }
    }

    /// Notes playing at the current position, as (channel index, key).
    pub fn sounding(&self) -> impl Iterator<Item = (usize, u16)> + '_ {
        self.sounding.iter().map(|&(c, k, _)| (c, k))
    }

    /// Renders the next `out.len() / 2` stereo frames (interleaved L/R) into `out`.
    pub fn process(&mut self, out: &mut [f32]) {
        let frames = out.len() / 2;
        for b in &mut self.bus {
            b.clear();
            b.resize(frames * 2, 0.0);
        }
        let block_end = self.frame + frames as u64;

        // Continue voices started in earlier blocks.
        let channels = &self.channels;
        let bus = &mut self.bus;
        self.voices.retain_mut(|v| render_voice(v, channels, bus, 0, frames));
        // Start voices whose trigger falls in this block, each from its exact offset.
        while let Some(t) = self.triggers.get(self.next_trigger).copied() {
            if t.frame >= block_end {
                break;
            }
            self.next_trigger += 1;
            self.sounding.push((t.channel, t.key, t.end));
            let offset = t.frame.saturating_sub(self.frame) as usize;
            let remaining = t.cut.map(|c| c.saturating_sub(t.frame));
            let mut v = Voice { channel: t.channel, pos: 0.0, step: t.rate, remaining, gain: t.gain };
            if render_voice(&mut v, &self.channels, &mut self.bus, offset, frames) {
                self.voices.push(v);
            }
        }

        // Mixer: apply each track's gain/pan, then add into its destinations.
        for &t in &self.order {
            let [[ll, lr], [rl, rr]] = self.tracks[t].matrix;
            let mut buf = std::mem::take(&mut self.bus[t]);
            for fx in &mut self.tracks[t].effects {
                fx.process(&mut buf);
            }
            for f in buf.chunks_exact_mut(2) {
                let (l, r) = (f[0], f[1]);
                f[0] = ll * l + lr * r;
                f[1] = rl * l + rr * r;
            }
            for &(dest, g) in &self.tracks[t].routes {
                for (d, s) in self.bus[dest].iter_mut().zip(&buf) {
                    *d += *s * g;
                }
            }
            self.bus[t] = buf;
        }
        out.copy_from_slice(&self.bus[0][..frames * 2]);
        self.frame = block_end;
        self.sounding.retain(|&(_, _, end)| end > block_end);
    }

    /// Renders everything to an interleaved stereo buffer.
    pub fn render_all(&mut self, block: usize) -> Vec<f32> {
        let mut out = Vec::new();
        let mut buf = vec![0.0f32; block * 2];
        while !self.finished() {
            self.process(&mut buf);
            out.extend_from_slice(&buf);
        }
        // Trim to the last non-silent frame, but never shorter than the scheduled length.
        let min_len = (self.end_frame as usize) * 2;
        let last = out.iter().rposition(|s| s.abs() > 0.0).map_or(0, |i| (i / 2 + 1) * 2);
        out.truncate(last.max(min_len).min(out.len()));
        out
    }
}

/// Resampling window half-width in source samples.
const SINC_W: f64 = 13.5;

/// Adds one voice into its channel's insert bus from `offset` to the block end.
/// Returns false when the voice has finished.
///
/// Rate 1 copies samples exactly. Other rates use a Blackman-windowed sinc with a fixed
/// 13.5-sample window; above rate 1 the cutoff drops to the output Nyquist (`1/rate`).
// spec: audio/sampler-playback.md (resampling)
fn render_voice(v: &mut Voice, channels: &[ChannelState], bus: &mut [Vec<f32>], offset: usize, frames: usize) -> bool {
    let ch = &channels[v.channel];
    let Some(s) = ch.sample.as_ref() else { return false };
    let n = s.frames();
    let (gl, gr) = v.gain;
    let out = &mut bus[ch.insert];
    let exact = v.step == 1.0;
    let scale = v.step.max(1.0);
    let mut pos = v.pos;
    for f in offset..frames {
        if v.remaining == Some(0) {
            return false;
        }
        if let Some(r) = v.remaining.as_mut() {
            *r -= 1;
        }
        if pos >= n as f64 {
            v.pos = pos;
            return false;
        }
        let (l, r) = if exact {
            let i = pos as usize;
            (s.left[i], s.right[i])
        } else {
            let i0 = (pos - SINC_W).ceil().max(0.0) as usize;
            let i1 = ((pos + SINC_W).floor() as usize).min(n - 1);
            let (mut l, mut r) = (0.0f64, 0.0f64);
            for i in i0..=i1 {
                let t = i as f64 - pos;
                let w = sinc(t / scale) / scale * blackman(t);
                l += f64::from(s.left[i]) * w;
                r += f64::from(s.right[i]) * w;
            }
            (l as f32, r as f32)
        };
        out[f * 2] += l * gl;
        out[f * 2 + 1] += r * gr;
        pos += v.step;
    }
    v.pos = pos;
    pos < n as f64
}

fn sinc(x: f64) -> f64 {
    if x == 0.0 {
        1.0
    } else {
        let px = std::f64::consts::PI * x;
        px.sin() / px
    }
}

fn blackman(t: f64) -> f64 {
    if t.abs() >= SINC_W {
        return 0.0;
    }
    let x = std::f64::consts::PI * t / SINC_W;
    0.42 + 0.5 * x.cos() + 0.08 * (2.0 * x).cos()
}

/// Orders tracks so that every track is processed before any track it routes to.
fn processing_order(tracks: &[MixTrack]) -> Vec<usize> {
    fn visit(t: usize, tracks: &[MixTrack], state: &mut [u8], order: &mut Vec<usize>) {
        if state[t] != 0 {
            return; // done, or a cycle (ignored)
        }
        state[t] = 1;
        for &(d, _) in &tracks[t].routes {
            visit(d, tracks, state, order);
        }
        state[t] = 2;
        order.push(t);
    }
    let mut state = vec![0u8; tracks.len()];
    let mut order = Vec::with_capacity(tracks.len());
    for t in 0..tracks.len() {
        visit(t, tracks, &mut state, &mut order);
    }
    order.reverse(); // sources first, destinations (Master) last
    order
}

/// A scheduled note.
#[derive(Debug, Clone, Copy)]
struct Sched {
    tick: f64,
    channel: usize,
    key: u16,
    velocity: u8,
    /// Note pan byte, 64 = center.
    pan: u8,
    /// Tick at which the voice is cut (audio clip end).
    cut: Option<f64>,
    /// Note length in ticks (0 = a plain step).
    len: f64,
}

impl Sched {
    fn note(start: f64, n: &os_model::Note) -> Self {
        Sched { tick: start + f64::from(n.position), channel: usize::from(n.channel), key: n.key, velocity: n.velocity, pan: n.pan, cut: None, len: f64::from(n.length) }
    }
}

/// A plucked tone at key 60 (261.6 Hz), about 1.5 s: decaying harmonics of a saw.
fn fallback_tone(rate: u32) -> Arc<Sample> {
    use std::f64::consts::TAU;
    let n = (f64::from(rate) * 1.5) as usize;
    let f0 = 261.625_565;
    let mut left = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f64 / f64::from(rate);
        let attack = (t / 0.004).min(1.0);
        let mut v = 0.0;
        for h in 1..=10 {
            let hf = f64::from(h);
            if f0 * hf > f64::from(rate) / 2.0 {
                break;
            }
            v += (TAU * f0 * hf * t).sin() / hf * (-t * (2.5 + 2.0 * hf)).exp();
        }
        left.push((0.45 * attack * v) as f32);
    }
    Arc::new(Sample { rate, right: left.clone(), left })
}

/// Note triggers plus the end tick of the scheduled material.
fn schedule(project: &Project, mode: PlayMode, report: &mut EngineReport) -> (Vec<Sched>, f64) {
    let bar = f64::from(project.ppq.max(1)) * 4.0;
    let pattern_notes = |n: u16| project.patterns.iter().find(|p| p.number == n).map(|p| p.notes.as_slice()).unwrap_or(&[]);
    let pattern_end = |n: u16| {
        let last = pattern_notes(n).iter().map(|x| f64::from(x.position) + f64::from(x.length)).fold(0.0, f64::max);
        ((last / bar).floor() + 1.0).max(1.0) * bar
    };
    let one_pattern = |n: u16| {
        // Muted notes are silent (spec: ui/piano-roll.md §9).
        let t: Vec<Sched> = pattern_notes(n).iter().filter(|x| !x.muted()).map(|x| Sched::note(0.0, x)).collect();
        (t, pattern_end(n))
    };
    match mode {
        PlayMode::Pattern(n) => one_pattern(n),
        PlayMode::Song => {
            let clips = project.arrangements.first().map(|a| a.clips.as_slice()).unwrap_or(&[]);
            if clips.is_empty() {
                return one_pattern(project.current_pattern.unwrap_or(1));
            }
            let mut out = Vec::new();
            let mut end: f64 = 0.0;
            for c in clips {
                let start = f64::from(c.position);
                let len = f64::from(c.length);
                end = end.max(start + len);
                match c.source {
                    ClipSource::Pattern(n) => {
                        for note in pattern_notes(n).iter().filter(|x| !x.muted()) {
                            if f64::from(note.position) < len {
                                out.push(Sched::note(start, note));
                            }
                        }
                    }
                    ClipSource::Channel(ch) => match project.channels.get(usize::from(ch)).map(|c| c.kind) {
                        // An audio clip plays its channel's sample from the start, cut at the clip end.
                        Some(ChannelKind::AudioClip | ChannelKind::Sampler) => {
                            out.push(Sched { tick: start, channel: usize::from(ch), key: 60, velocity: 100, pan: 64, cut: Some(start + len), len });
                        }
                        _ => report.automation_clips_ignored += 1,
                    },
                }
            }
            (out, end)
        }
    }
}
