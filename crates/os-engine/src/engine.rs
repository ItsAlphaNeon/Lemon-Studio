//! Block-based playback engine: schedules notes from patterns / playlist clips, plays sampler
//! and audio-clip channels, and sums them through the mixer routing graph.
//!
// spec: audio/sampler-playback.md, audio/gain-laws.md, flp/events/playlist.md (clips),
//       flp/events/patterns.md (notes), flp/events/mixer.md (routing), audio/pattern-controls.md (repeat, control points)

use std::sync::Arc;

use os_model::{ChannelKind, ClipSource, ControlPoint, Project};

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
    /// Key rate; the voice plays at the channel's current base rate times this.
    rate: f64,
    /// Output frame at which the voice is cut (end of an audio clip), if any.
    cut: Option<u64>,
    /// Velocity gain and note pan (−1..=1); the channel's volume and pan apply while it plays.
    vel: f64,
    pan: f64,
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
    /// Channel is on (a muted channel stays silent whatever a volume point says).
    enabled: bool,
    /// Sample-rate conversion alone (`base_rate` without the channel pitch).
    src_ratio: f64,
    /// Values at the start of playback, restored when control points are replayed from the top.
    init: (f64, f64, f64),
    /// Volume parameter 0..=1 (before the volume law) at the start of the current span.
    vol_v: f64,
    init_v: f64,
    /// Count of glides started (`Ramp::id`, `Voice::serial`).
    serial: u32,
    /// A volume / pan control point in progress (the parameters glide, then the laws apply).
    ramp: Option<Ramp>,
}

/// Glide of the channel's volume and pan parameters towards a control point's values.
#[derive(Debug, Clone, Copy)]
struct Ramp {
    v0: f64,
    v1: f64,
    p0: f64,
    p1: f64,
    /// Frames each parameter takes (0 = already there).
    len_v: u32,
    len_p: u32,
    /// Frames done at the start of the current span.
    pos: u32,
    /// Voices born before this glide started follow it; later notes start at the final values.
    id: u32,
}

impl Ramp {
    /// How far the volume / pan parameter is after `frames` (0..=1).
    fn volume_at(&self, frames: f64) -> f64 {
        if self.len_v == 0 { 1.0 } else { (frames / f64::from(self.len_v)).min(1.0) }
    }
    fn pan_at(&self, frames: f64) -> f64 {
        if self.len_p == 0 { 1.0 } else { (frames / f64::from(self.len_p)).min(1.0) }
    }
}

/// Glide of a volume / pan control point on voices already sounding: the parameter moves linearly and the
/// law is applied to it. FL's volume glide lasts about 9.3 ms (shorter for tiny changes) and its pan glide
/// about 4.7 ms (measured; spec: audio/pattern-controls.md).
const VOLUME_GLIDE_SECONDS: f64 = 0.0093;
const PAN_GLIDE_SECONDS: f64 = 0.0047;

/// A channel control point placed in output frames.
#[derive(Debug, Clone, Copy)]
struct CtlEvent {
    frame: u64,
    channel: usize,
    param: u16,
    value: i32,
}

impl ChannelState {
    fn reset(&mut self) {
        (self.volume, self.pan, self.base_rate) = self.init;
        self.vol_v = self.init_v;
        self.ramp = None;
    }

    /// Gain of a note that starts now: a glide in progress is already at its final values for it.
    fn start_gain(&self, vel: f64, note_pan: f64) -> (f32, f32) {
        match self.ramp {
            None => self.gain(vel, note_pan),
            Some(r) => {
                let g = self.vol_gain(r.v1) * vel;
                let (pl, pr) = laws::channel_pan(r.p1 + note_pan);
                ((g * pl) as f32, (g * pr) as f32)
            }
        }
    }

    fn vol_gain(&self, v: f64) -> f64 {
        if self.enabled { laws::channel_gain(v) } else { 0.0 }
    }

    /// Moves the span-start values on by `frames` of a running glide.
    fn advance(&mut self, frames: u32) {
        let Some(r) = self.ramp.as_mut() else { return };
        r.pos = r.pos.saturating_add(frames);
        let pos = f64::from(r.pos);
        self.vol_v = r.v0 + (r.v1 - r.v0) * r.volume_at(pos);
        self.pan = r.p0 + (r.p1 - r.p0) * r.pan_at(pos);
        if r.pos >= r.len_v && r.pos >= r.len_p {
            (self.vol_v, self.pan) = (r.v1, r.p1);
            self.ramp = None;
        }
        self.volume = self.vol_gain(self.vol_v);
    }

    /// Starts gliding the volume and / or pan parameter to a new value over `len` frames (0 = at once).
    fn glide(&mut self, v1: Option<f64>, p1: Option<f64>, len: u32) {
        // A parameter that is not being set carries on towards its old target in the time it had left.
        let (tv, tp, rest_v, rest_p) = self.ramp.map_or((self.vol_v, self.pan, 0, 0), |r| {
            (r.v1, r.p1, r.len_v.saturating_sub(r.pos), r.len_p.saturating_sub(r.pos))
        });
        let (nv, np) = (v1.unwrap_or(tv), p1.unwrap_or(tp));
        let len_v = if v1.is_some() { len } else { rest_v };
        let len_p = if p1.is_some() { len } else { rest_p };
        if (nv, np) == (self.vol_v, self.pan) && self.ramp.is_none() {
            return;
        }
        if len_v == 0 && len_p == 0 {
            self.ramp = None;
            (self.vol_v, self.pan) = (nv, np);
            self.volume = self.vol_gain(nv);
        } else {
            self.serial += 1;
            self.ramp = Some(Ramp { v0: self.vol_v, v1: nv, p0: self.pan, p1: np, len_v, len_p, pos: 0, id: self.serial });
        }
    }

    /// A control point replaces the channel's own setting (spec: audio/pattern-controls.md).
    fn apply(&mut self, param: u16, value: i32, rate: u32) {
        match param {
            ControlPoint::VOLUME => self.glide(Some((f64::from(value) / 12800.0).clamp(0.0, 1.0)), None, (f64::from(rate) * VOLUME_GLIDE_SECONDS).round() as u32),
            ControlPoint::PAN => self.glide(None, Some((f64::from(value) / 6400.0 - 1.0).clamp(-1.0, 1.0)), (f64::from(rate) * PAN_GLIDE_SECONDS).round() as u32),
            ControlPoint::PITCH => self.base_rate = self.src_ratio * laws::playback_rate(60, value),
            _ => {}
        }
    }

    /// Gain of a note on this channel: channel volume × velocity × pan.
    // spec: audio/sampler-playback.md (velocity multiplies volume; pans add, clamped)
    fn gain(&self, vel: f64, note_pan: f64) -> (f32, f32) {
        let g = self.volume * vel;
        let (pl, pr) = laws::channel_pan(self.pan + note_pan);
        ((g * pl) as f32, (g * pr) as f32)
    }
}

#[derive(Debug, Clone, Copy)]
struct Voice {
    channel: usize,
    pos: f64,
    /// Key rate; times the channel's base rate (so pitch edits reach ringing voices).
    step: f64,
    /// Output frames left before the voice is cut; `None` = play the whole sample.
    remaining: Option<u64>,
    vel: f64,
    pan: f64,
    /// Gain applied at the end of the last span; ramps to the channel's current gain, so
    /// volume / pan edits reach ringing voices without clicks.
    gain: (f32, f32),
    /// Gain the ramp is heading for, its per-frame step and the frames left.
    dest: (f32, f32),
    delta: (f32, f32),
    ramp_left: u32,
    /// The channel's glide count when the voice started.
    serial: u32,
    /// Key of a live note (`note_on`), which `note_off` releases.
    live: Option<u16>,
    /// Release fade after a live note off: (frames left, fade length).
    release: Option<(u32, u32)>,
}

/// Release fade of a live note (an OpenStudio choice, spec: ui/typing-keyboard.md open questions).
const RELEASE_SECONDS: f64 = 0.015;

struct MixTrack {
    /// Insert effects in slot order (applied before the fader).
    effects: Vec<Box<dyn Effect>>,
    /// Fader × pan as `[[L←L, L←R], [R←L, R←R]]`.
    matrix: [[f32; 2]; 2],
    /// Matrix applied at the end of the last block; ramps to `matrix` over one block.
    cur: [[f32; 2]; 2],
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
    /// Channel control points in frame order, and the next one to apply.
    controls: Vec<CtlEvent>,
    next_ctl: usize,
    /// Notes started and not yet past their length: (channel, key, end frame).
    sounding: Vec<(usize, u16, u64)>,
    voices: Vec<Voice>,
    frame: u64,
    /// Frame where the scheduled material ends (voices may ring past it).
    end_frame: u64,
    /// Restart at frame 0 on reaching `end_frame` (pattern playback), sample-accurately.
    looping: bool,
    tempo: f64,
    ppq: u16,
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
                        None if ch.kind == ChannelKind::Sampler => {
                            Some(library.tones.entry(rate).or_insert_with(|| fallback_tone(rate)).clone())
                        }
                        None => None,
                    }
                } else {
                    None
                };
                let volume = if ch.enabled { laws::channel_gain(ch.volume) } else { 0.0 };
                let base_rate = sample.as_ref().map_or(1.0, |s| f64::from(s.rate) / f64::from(rate))
                    * laws::playback_rate(60, ch.pitch_cents);
                let insert = usize::from(ch.mixer_track).min(n_tracks - 1);
                let src_ratio = sample.as_ref().map_or(1.0, |s| f64::from(s.rate) / f64::from(rate));
                ChannelState {
                    sample,
                    volume,
                    pan: ch.pan,
                    insert,
                    base_rate,
                    enabled: ch.enabled,
                    src_ratio,
                    init: (volume, ch.pan, base_rate),
                    vol_v: ch.volume,
                    init_v: ch.volume,
                    serial: 0,
                    ramp: None,
                }
            })
            .collect();

        let tracks: Vec<MixTrack> = if project.mixer.is_empty() {
            let unity = [[1.0, 0.0], [0.0, 1.0]];
            vec![MixTrack { effects: Vec::new(), matrix: unity, cur: unity, routes: vec![] }]
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
                    MixTrack { effects, matrix, cur: matrix, routes }
                })
                .collect()
        };
        let order = processing_order(&tracks);

        let mut controls: Vec<CtlEvent> = schedule_controls(project, mode)
            .into_iter()
            .filter(|&(_, channel, _, _)| channel < channels.len())
            .map(|(tick, channel, param, value)| CtlEvent { frame: laws::tick_to_frame(tick, ppq, tempo, rate), channel, param, value })
            .collect();
        controls.sort_by_key(|c| c.frame); // stable: equal frames keep their order
        let (mut triggers, end_tick) = schedule(project, mode, &mut report);
        triggers.sort_by(|a, b| a.tick.total_cmp(&b.tick));
        let triggers = triggers
            .into_iter()
            .filter_map(|n: Sched| {
                let state = channels.get(n.channel)?;
                state.sample.as_ref()?;
                // A plain step (length 0) counts as one step long.
                let len = if n.len > 0.0 { n.len } else { f64::from(ppq) / 4.0 };
                Some(Trigger {
                    key: n.key,
                    end: laws::tick_to_frame(n.tick + len, ppq, tempo, rate),
                    frame: laws::tick_to_frame(n.tick, ppq, tempo, rate),
                    channel: n.channel,
                    rate: laws::playback_rate(n.key, 0),
                    cut: n.cut.map(|c| laws::tick_to_frame(c, ppq, tempo, rate)),
                    vel: laws::velocity_gain(n.velocity),
                    pan: laws::note_pan(n.pan),
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
            controls,
            next_ctl: 0,
            sounding: Vec::with_capacity(256),
            voices: Vec::with_capacity(1024),
            frame: 0,
            end_frame: laws::tick_to_frame(end_tick, ppq, tempo, rate),
            looping: false,
            tempo,
            ppq,
            report,
        }
    }

    /// Loop the scheduled material: on reaching its end, playback continues from frame 0 within
    /// the same block (ringing voices carry on). Used for pattern playback.
    pub fn set_looping(&mut self, on: bool) {
        self.looping = on;
    }

    /// Drop the scheduled notes: the engine only plays live notes (`note_on`).
    pub fn clear_schedule(&mut self) {
        self.triggers.clear();
        self.next_trigger = 0;
        self.end_frame = 0;
    }

    /// Start a live note on channel `channel` (rack position) at the next block; it rings until
    /// `note_off` (or the sample ends). Velocity 0..=128, 100 = unity.
    pub fn note_on(&mut self, channel: usize, key: u16, velocity: u8) {
        let Some(ch) = self.channels.get(channel) else { return };
        if ch.sample.is_none() {
            return;
        }
        let (vel, pan) = (laws::velocity_gain(velocity), 0.0);
        let gain = ch.start_gain(vel, pan);
        let serial = ch.serial;
        let step = laws::playback_rate(key, 0);
        self.note_off(channel, key); // retrigger
        self.voices.push(Voice {
            channel,
            pos: 0.0,
            step,
            remaining: None,
            vel,
            pan,
            gain,
            dest: gain,
            delta: (0.0, 0.0),
            ramp_left: 0,
            serial,
            live: Some(key),
            release: None,
        });
        self.sounding.push((channel, key, u64::MAX));
    }

    /// Release a live note with a short fade.
    pub fn note_off(&mut self, channel: usize, key: u16) {
        let fade = (f64::from(self.rate) * RELEASE_SECONDS) as u32;
        for v in &mut self.voices {
            if v.channel == channel && v.live == Some(key) && v.release.is_none() {
                v.release = Some((fade.max(1), fade.max(1)));
            }
        }
        self.sounding.retain(|&(c, k, end)| !(c == channel && k == key && end == u64::MAX));
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

    /// True once all scheduled material has played and every voice has finished (never while
    /// looping).
    pub fn finished(&self) -> bool {
        !self.looping && self.frame >= self.end_frame && self.next_trigger >= self.triggers.len() && self.voices.is_empty()
    }

    /// Start playback at `frame` (notes before it are not triggered).
    pub fn seek(&mut self, frame: u64) {
        self.frame = frame;
        self.next_trigger = self.triggers.partition_point(|t| t.frame < frame);
        self.sync_controls(frame);
    }

    /// Applies one control point at a time as playback reaches it, smoothing volume and pan.
    fn run_controls(&mut self) {
        while let Some(&e) = self.controls.get(self.next_ctl) {
            if e.frame > self.frame {
                break;
            }
            self.next_ctl += 1;
            if let Some(ch) = self.channels.get_mut(e.channel) {
                ch.apply(e.param, e.value, self.rate);
            }
        }
    }

    /// Channels as they are at `frame`: their own settings with every control point before it applied.
    fn sync_controls(&mut self, frame: u64) {
        for ch in &mut self.channels {
            ch.reset();
        }
        self.next_ctl = 0;
        while let Some(&e) = self.controls.get(self.next_ctl) {
            if e.frame >= frame {
                break;
            }
            self.next_ctl += 1;
            if let Some(ch) = self.channels.get_mut(e.channel) {
                ch.apply(e.param, e.value, 0);
            }
        }
    }

    /// Take over playback from `old` (built from an earlier version of the project) at its
    /// position: ringing voices keep playing, notes before the position are not retriggered, and
    /// effect states carry over when the mixer layout is unchanged. Edits are heard immediately:
    /// ringing voices take the new channel volume / pan and mixer levels (ramped over a block).
    /// A tempo change keeps the musical position (same tick, new frame).
    pub fn continue_from(&mut self, old: &mut Engine) {
        let (from, to) = ((old.tempo, old.ppq, old.rate), (self.tempo, self.ppq, self.rate));
        let to_new = move |f: u64| -> u64 {
            if from == to {
                return f;
            }
            let tick = f as f64 / f64::from(from.2.max(1)) * from.0 / 60.0 * f64::from(from.1);
            laws::tick_to_frame(tick, to.1, to.0, to.2)
        };
        self.looping = old.looping;
        self.frame = to_new(old.frame);
        let frame = self.frame;
        self.next_trigger = self.triggers.partition_point(|t| t.frame < frame);
        self.sync_controls(frame);
        let n = self.channels.len();
        self.voices = std::mem::take(&mut old.voices);
        self.voices.retain(|v| v.channel < n);
        self.sounding = std::mem::take(&mut old.sounding);
        for s in &mut self.sounding {
            if s.2 != u64::MAX {
                s.2 = to_new(s.2); // MAX = a live note, held until note off
            }
        }
        if self.rate == old.rate && self.tracks.len() == old.tracks.len() {
            for (new, o) in self.tracks.iter_mut().zip(&mut old.tracks) {
                new.cur = o.cur;
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
        // A looping engine splits the block at the loop point and carries on from frame 0.
        let mut at = 0;
        while at < frames {
            let looping = self.looping && self.end_frame > 0;
            if looping && self.frame >= self.end_frame {
                self.wrap();
            }
            self.run_controls();
            let mut n = frames - at;
            if looping {
                n = n.min((self.end_frame - self.frame) as usize);
            }
            // Control points take effect on their exact frame.
            if let Some(c) = self.controls.get(self.next_ctl) {
                n = n.min(c.frame.saturating_sub(self.frame).max(1) as usize);
            }
            self.render_span(at, at + n);
            at += n;
        }

        // Mixer: apply each track's gain/pan (ramped after an edit), then add into its destinations.
        for &t in &self.order {
            let (to, from) = (self.tracks[t].matrix, self.tracks[t].cur);
            self.tracks[t].cur = to;
            let mut buf = std::mem::take(&mut self.bus[t]);
            for fx in &mut self.tracks[t].effects {
                fx.process(&mut buf);
            }
            if from == to {
                let [[ll, lr], [rl, rr]] = to;
                for f in buf.chunks_exact_mut(2) {
                    let (l, r) = (f[0], f[1]);
                    f[0] = ll * l + lr * r;
                    f[1] = rl * l + rr * r;
                }
            } else {
                let n = frames.max(1) as f32;
                for (i, f) in buf.chunks_exact_mut(2).enumerate() {
                    let k = (i + 1) as f32 / n;
                    let m = |r: usize, c: usize| from[r][c] + (to[r][c] - from[r][c]) * k;
                    let (l, r) = (f[0], f[1]);
                    f[0] = m(0, 0) * l + m(0, 1) * r;
                    f[1] = m(1, 0) * l + m(1, 1) * r;
                }
            }
            for &(dest, g) in &self.tracks[t].routes {
                for (d, s) in self.bus[dest].iter_mut().zip(&buf) {
                    *d += *s * g;
                }
            }
            self.bus[t] = buf;
        }
        out.copy_from_slice(&self.bus[0][..frames * 2]);
    }

    /// Renders voices into the insert buses for block frames `a..b`, starting triggers that fall
    /// in it from their exact offset.
    fn render_span(&mut self, a: usize, b: usize) {
        let span_end = self.frame + (b - a) as u64;
        // Continue voices started earlier.
        let channels = &self.channels;
        let bus = &mut self.bus;
        self.voices.retain_mut(|v| render_voice(v, channels, bus, a, a, b));
        while let Some(t) = self.triggers.get(self.next_trigger).copied() {
            if t.frame >= span_end {
                break;
            }
            self.next_trigger += 1;
            self.sounding.push((t.channel, t.key, t.end));
            let offset = a + t.frame.saturating_sub(self.frame) as usize;
            let remaining = t.cut.map(|c| c.saturating_sub(t.frame));
            let gain = self.channels[t.channel].start_gain(t.vel, t.pan);
            let serial = self.channels[t.channel].serial;
            let mut v = Voice {
                channel: t.channel,
                pos: 0.0,
                step: t.rate,
                remaining,
                vel: t.vel,
                pan: t.pan,
                gain,
                dest: gain,
                delta: (0.0, 0.0),
                ramp_left: 0,
                serial,
                live: None,
                release: None,
            };
            if render_voice(&mut v, &self.channels, &mut self.bus, a, offset, b) {
                self.voices.push(v);
            }
        }
        for ch in &mut self.channels {
            ch.advance((b - a) as u32);
        }
        self.frame = span_end;
        self.sounding.retain(|&(_, _, end)| end > span_end);
    }

    /// Loop point: back to frame 0 (sounding notes keep their remaining length).
    fn wrap(&mut self) {
        let at = self.frame;
        self.frame = 0;
        self.next_trigger = 0;
        // Control points replay from the top: until the first one the channel has its own settings
        // (an OpenStudio choice; FL's behaviour at the loop point is not measured).
        self.sync_controls(0);
        for s in &mut self.sounding {
            if s.2 != u64::MAX {
                s.2 = s.2.saturating_sub(at);
            }
        }
        self.sounding.retain(|&(_, _, end)| end > 0);
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

/// Adds one voice into its channel's insert bus for block frames `offset..end`.
/// Returns false when the voice has finished.
///
/// The gain ramps from the voice's last gain to the channel's current one over the span (no
/// change = constant). Rate 1 copies samples exactly. Other rates use a Blackman-windowed sinc with a fixed
/// 13.5-sample window; above rate 1 the cutoff drops to the output Nyquist (`1/rate`).
// spec: audio/sampler-playback.md (resampling)
fn render_voice(v: &mut Voice, channels: &[ChannelState], bus: &mut [Vec<f32>], span: usize, offset: usize, end: usize) -> bool {
    let ch = &channels[v.channel];
    let Some(s) = ch.sample.as_ref() else { return false };
    let n = s.frames();
    let glide = ch.ramp.filter(|r| v.serial < r.id);
    let target = ch.start_gain(v.vel, v.pan);
    if glide.is_none() && target != v.dest {
        // Any edit smooths over this span (a control point's glide is handled per frame below).
        let len = end.saturating_sub(offset).max(1) as u32;
        v.dest = target;
        v.delta = ((target.0 - v.gain.0) / len as f32, (target.1 - v.gain.1) / len as f32);
        v.ramp_left = len;
    }
    let (mut gl, mut gr) = v.gain;
    let (dl, dr) = v.delta;
    let mut left = v.ramp_left;
    let step = ch.base_rate * v.step;
    let out = &mut bus[ch.insert];
    // Rate 1 copies samples only while the position is whole (a voice that was pitched keeps its fraction).
    let exact = step == 1.0 && v.pos.fract() == 0.0;
    let scale = step.max(1.0);
    let mut pos = v.pos;
    for f in offset..end {
        if v.remaining == Some(0) {
            return false;
        }
        if let Some(r) = v.remaining.as_mut() {
            *r -= 1;
        }
        if pos >= n as f64 {
            v.pos = pos;
            v.gain = (gl, gr);
            return false;
        }
        let fade = match v.release.as_mut() {
            None => 1.0,
            Some((0, _)) => return false,
            Some((left, len)) => {
                *left -= 1;
                *left as f32 / *len as f32
            }
        };
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
        if let Some(r) = glide {
            // The volume parameter glides linearly and the volume law applies to it; the pan glides linearly
            // in the left / right gains (spec: audio/pattern-controls.md).
            let frames = f64::from(r.pos) + (f - span) as f64 + 1.0;
            let g = ch.vol_gain(r.v0 + (r.v1 - r.v0) * r.volume_at(frames)) * v.vel;
            let (a, b) = (laws::channel_pan(r.p0 + v.pan), laws::channel_pan(r.p1 + v.pan));
            let x = r.pan_at(frames);
            let (pl, pr) = (a.0 + (b.0 - a.0) * x, a.1 + (b.1 - a.1) * x);
            (gl, gr) = ((g * pl) as f32, (g * pr) as f32);
            v.dest = (gl, gr);
            left = 0;
        } else if left > 0 {
            gl += dl;
            gr += dr;
            left -= 1;
            if left == 0 {
                (gl, gr) = v.dest;
            }
        }
        out[f * 2] += l * gl * fade;
        out[f * 2 + 1] += r * gr * fade;
        pos += step;
    }
    v.pos = pos;
    v.gain = (gl, gr);
    v.ramp_left = left;
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

/// Schedules a note; a repeating note is retriggered at every multiple of its interval inside its
/// length (spec: audio/pattern-controls.md, note repeat). A plain step counts as one step long.
fn push_note(out: &mut Vec<Sched>, start: f64, n: &os_model::Note, ppq: u16) {
    let first = Sched::note(start, n);
    let span = if n.length > 0 { f64::from(n.length) } else { f64::from(ppq) / 4.0 };
    match n.repeat_ticks(ppq) {
        Some(interval) => {
            let mut off = 0.0;
            while off < span {
                let len = if n.length > 0 { first.len - off } else { 0.0 };
                out.push(Sched { tick: first.tick + off, len, ..first });
                off += f64::from(interval);
            }
        }
        None => out.push(first),
    }
}

/// Channel control points as (tick, channel, parameter, value): a pattern's own, or those of every
/// pattern clip of the playlist (spec: audio/pattern-controls.md).
fn schedule_controls(project: &Project, mode: PlayMode) -> Vec<(f64, usize, u16, i32)> {
    let points = |n: u16| project.patterns.iter().find(|p| p.number == n).map(|p| p.controls.as_slice()).unwrap_or(&[]);
    let place = |out: &mut Vec<_>, start: f64, limit: f64, n: u16| {
        for c in points(n).iter().filter(|c| f64::from(c.position) < limit) {
            out.push((start + f64::from(c.position), usize::from(c.channel()), c.param(), c.value));
        }
    };
    let mut out = Vec::new();
    match mode {
        PlayMode::Pattern(n) => place(&mut out, 0.0, f64::INFINITY, n),
        PlayMode::Song => {
            let clips = project.arrangements.first().map(|a| a.clips.as_slice()).unwrap_or(&[]);
            if clips.is_empty() {
                place(&mut out, 0.0, f64::INFINITY, project.current_pattern.unwrap_or(1));
            }
            for c in clips {
                if let ClipSource::Pattern(n) = c.source {
                    place(&mut out, f64::from(c.position), f64::from(c.length), n);
                }
            }
        }
    }
    out
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
        let mut t = Vec::new();
        for x in pattern_notes(n).iter().filter(|x| !x.muted()) {
            push_note(&mut t, 0.0, x, project.ppq);
        }
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
                                push_note(&mut out, start, note, project.ppq);
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

#[cfg(test)]
mod tests {
    // spec: audio/pattern-controls.md (note repeat and channel control points, measured in FL)
    use super::*;
    use os_model::{Channel, Note, Pattern};

    fn channel() -> Channel {
        Channel {
            index: 0,
            kind: ChannelKind::Sampler,
            name: None,
            plugin: None,
            color: None,
            enabled: true,
            volume: 0.78125,
            pan: 0.0,
            pitch_cents: 0,
            mixer_track: 0,
            sample_path: None, // plays the built-in tone
            layer_children: vec![],
        }
    }

    fn project(notes: Vec<Note>, controls: Vec<ControlPoint>) -> Project {
        Project {
            ppq: 96,
            tempo: Some(130.0),
            channels: vec![channel()],
            patterns: vec![Pattern { number: 1, notes, controls, ..Pattern::default() }],
            ..Project::default()
        }
    }

    fn long_note(position: u32, length: u32, repeat: u8) -> Note {
        Note { length, repeat, ..Note::step(0, position) }
    }

    fn render(p: &Project) -> Vec<f32> {
        let mut lib = SampleLibrary::new(None);
        Engine::new(p, &mut lib, 44100, PlayMode::Pattern(1)).render_all(512)
    }

    fn peak(x: &[f32], ch: usize) -> f32 {
        x.chunks_exact(2).map(|f| f[ch].abs()).fold(0.0, f32::max)
    }

    #[test]
    fn repeat_retriggers_inside_the_note() {
        let mut report = EngineReport::default();
        let mut ticks = |p: &Project| -> Vec<(f64, f64)> {
            let (t, _) = schedule(p, PlayMode::Pattern(1), &mut report);
            t.iter().map(|s| (s.tick, s.len)).collect()
        };
        // 1/8 (48 ticks) over 384 ticks: eight triggers, the last one 48 ticks long.
        let t = ticks(&project(vec![long_note(0, 384, 4)], vec![]));
        assert_eq!(t.len(), 8);
        assert_eq!(t[1], (48.0, 336.0));
        assert_eq!(t[7], (336.0, 48.0));
        // Counts measured in FL for a 384-tick note (index 1..=14).
        let counts = [4, 6, 6, 8, 11, 12, 16, 22, 24, 32, 43, 48, 64, 96];
        for (i, want) in counts.iter().enumerate() {
            let got = ticks(&project(vec![long_note(0, 384, i as u8 + 1)], vec![])).len();
            assert_eq!(got, *want, "repeat index {}", i + 1);
        }
        // No repeat, and a plain step (one step long) with a step-sized repeat: one trigger.
        assert_eq!(ticks(&project(vec![long_note(0, 384, 0)], vec![])).len(), 1);
        assert_eq!(ticks(&project(vec![long_note(48, 0, 7)], vec![])).len(), 1);
    }

    #[test]
    fn controls_are_scheduled_per_pattern_and_clip() {
        let pts = vec![ControlPoint::new(24, 0, ControlPoint::VOLUME, 6400), ControlPoint::new(96, 3, ControlPoint::PAN, 0)];
        let p = project(vec![long_note(0, 96, 0)], pts);
        let c = schedule_controls(&p, PlayMode::Pattern(1));
        assert_eq!(c, vec![(24.0, 0, ControlPoint::VOLUME, 6400), (96.0, 3, ControlPoint::PAN, 0)]);
    }

    #[test]
    fn volume_point_replaces_channel_volume() {
        let note = vec![long_note(0, 96, 0)];
        let plain = render(&project(note.clone(), vec![]));
        // v = 0.5 from the first tick: the note plays at law(0.5) / law(0.78125) = 0.4204 of the default.
        let quiet = render(&project(note.clone(), vec![ControlPoint::new(0, 0, ControlPoint::VOLUME, 6400)]));
        let ratio = peak(&quiet, 0) / peak(&plain, 0);
        assert!((ratio - 0.4204).abs() < 0.002, "ratio {ratio}");
        // A point equal to the channel's own volume changes nothing.
        let same = render(&project(note, vec![ControlPoint::new(0, 0, ControlPoint::VOLUME, 10000)]));
        assert_eq!(same, plain);
    }

    #[test]
    fn pan_point_moves_the_note() {
        let note = vec![long_note(0, 96, 0)];
        let left = render(&project(note, vec![ControlPoint::new(0, 0, ControlPoint::PAN, 0)]));
        assert!(peak(&left, 0) > 0.05);
        assert!(peak(&left, 1) < 1e-6, "right is silent at full left");
    }

    #[test]
    fn pitch_point_shifts_by_cents() {
        let note = vec![long_note(0, 96, 0)];
        let end = |x: &[f32]| x.chunks_exact(2).rposition(|f| f[0].abs() > 1e-4).unwrap_or(0) as f64;
        let plain = render(&project(note.clone(), vec![]));
        let up = render(&project(note, vec![ControlPoint::new(0, 0, ControlPoint::PITCH, 1200)]));
        // An octave up plays the sample twice as fast, so it ends in half the time.
        let ratio = end(&up) / end(&plain);
        assert!((ratio - 0.5).abs() < 0.03, "ratio {ratio}");
    }

    #[test]
    fn seeking_past_a_point_applies_it() {
        let p = project(vec![long_note(0, 384, 0)], vec![ControlPoint::new(24, 0, ControlPoint::VOLUME, 6400)]);
        let mut lib = SampleLibrary::new(None);
        let mut e = Engine::new(&p, &mut lib, 44100, PlayMode::Pattern(1));
        e.seek(laws::tick_to_frame(48.0, 96, 130.0, 44100));
        assert!((e.channels[0].volume - laws::channel_gain(0.5)).abs() < 1e-12);
        e.seek(0);
        assert!((e.channels[0].volume - laws::channel_gain(0.78125)).abs() < 1e-12, "back before the point");
    }
}
