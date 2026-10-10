//! Block-based playback engine: schedules notes from patterns / playlist clips, plays sampler
//! and audio-clip channels, and sums them through the mixer routing graph.
//!
// spec: audio/sampler-playback.md, audio/gain-laws.md, flp/events/playlist.md (clips),
//       flp/events/patterns.md (notes), flp/events/mixer.md (routing), audio/pattern-controls.md (repeat, control points),
//       audio/channel-envelopes.md (volume and pitch envelopes)

use std::sync::Arc;

use os_model::{ChannelKind, ClipSource, ControlPoint, Project};

use crate::effects::Effect;
use crate::envelope;
use crate::instrument::{SharedInstrument, Slot};
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
    /// Note velocity byte (instrument channels get it as 0..=1).
    velocity: u8,
    /// Sample position (source frames) the voice starts at.
    from: f64,
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
    /// The channel's volume and pitch envelopes, when switched on (sampler channels).
    vol_env: Option<envelope::Params>,
    pitch_env: Option<envelope::Params>,
    /// Seconds per output frame.
    frame_s: f64,
    /// A generator plugin channel; it sounds only once an instrument is attached (`Engine::set_instrument`).
    plugin: bool,
    instrument: Slot,
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
    /// Frames played so far, and the age at which the note ended (the channel envelopes' clock).
    age: u64,
    off: Option<u64>,
}

/// Channel pitch automation range: ±2 semitones, the channel's default pitch range (spec: flp/events/automation.md).
const AUTO_PITCH_CENTS: f64 = 200.0;

/// Release fade of a live note (an OpenStudio choice, spec: ui/typing-keyboard.md open questions).
const RELEASE_SECONDS: f64 = 0.015;

/// An automation clip placed in output frames (spec: ui/playlist.md §8, flp/events/automation.md).
#[derive(Debug, Clone, Copy)]
struct AutoClip {
    start: u64,
    end: u64,
    /// Beats into the automation at the clip start (its window).
    from_beat: f64,
    /// Index into `Engine::curves`.
    curve: usize,
    target: os_model::target::Target,
}

struct MixTrack {
    /// Fader position and pan as set (automation rewrites them, then the matrix).
    vol: f64,
    pan: f64,
    on: bool,
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
    /// Instrument channels: scheduled note ends (frame, channel, key), live notes waiting for the next span
    /// (channel, key, velocity; negative = note off), this span's events (channel, offset, key, velocity) and scratch.
    inst_offs: Vec<(u64, usize, u8)>,
    inst_live: Vec<(usize, u8, f32)>,
    inst_events: Vec<(usize, u32, u8, f32)>,
    scratch: (Vec<f32>, Vec<f32>),
    /// Frames instruments keep being processed after their last held note (their release / tail is not known).
    inst_tail: u64,
    /// Automation clips of the playlist and their curves (song mode).
    autos: Vec<AutoClip>,
    curves: Vec<os_model::Automation>,
    frames_per_beat: f64,
    pub report: EngineReport,
}

/// Seconds an instrument keeps running after its last note (an OpenStudio choice).
const INSTRUMENT_TAIL_SECONDS: u64 = 5;

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
                let env_on = ch.kind == ChannelKind::Sampler;
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
                    vol_env: env_on.then(|| envelope::Params::new(&ch.envelopes[os_model::Envelope::VOLUME])).flatten(),
                    pitch_env: env_on.then(|| envelope::Params::new(&ch.envelopes[os_model::Envelope::PITCH])).flatten(),
                    frame_s: 1.0 / f64::from(rate),
                    plugin: ch.kind == ChannelKind::Plugin,
                    instrument: Slot::default(),
                }
            })
            .collect();

        let tracks: Vec<MixTrack> = if project.mixer.is_empty() {
            let unity = [[1.0, 0.0], [0.0, 1.0]];
            vec![MixTrack { vol: 0.8, pan: 0.0, on: true, effects: Vec::new(), matrix: unity, cur: unity, routes: vec![] }]
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
                    MixTrack { vol: m.volume.unwrap_or(0.8), pan: m.pan.unwrap_or(0.0), on: m.enabled, effects, matrix, cur: matrix, routes }
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
                if state.sample.is_none() && !state.plugin {
                    return None;
                }
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
                    velocity: n.velocity,
                    from: state.sample.as_ref().map_or(0.0, |s| n.from_s * f64::from(s.rate)),
                })
            })
            .collect();

        // Automation clips (song mode only): each plays its curve onto its target from its start to its end.
        let mut curves = Vec::new();
        let mut autos = Vec::new();
        if mode == PlayMode::Song {
            for c in project.arrangement().map(|a| a.clips.as_slice()).unwrap_or(&[]) {
                let ClipSource::Channel(ch) = c.source else { continue };
                let Some(a) = project.automation(ch) else { continue };
                let Some(target) = a.target.map(os_model::target::decode) else { continue };
                if !clip_audible(project, c) {
                    continue;
                }
                if matches!(target, os_model::target::Target::Other(_) | os_model::target::Target::Tempo) {
                    report.automation_clips_ignored += 1;
                    continue;
                }
                let curve = match curves.iter().position(|x: &os_model::Automation| x.channel == ch) {
                    Some(i) => i,
                    None => {
                        curves.push(a.clone());
                        curves.len() - 1
                    }
                };
                autos.push(AutoClip {
                    start: laws::tick_to_frame(f64::from(c.position), ppq, tempo, rate),
                    end: laws::tick_to_frame(f64::from(c.end()), ppq, tempo, rate),
                    from_beat: c.window.map_or(0.0, |w| w.0),
                    curve,
                    target,
                });
            }
        }
        // Later clips win where clips on the same target overlap.
        autos.sort_by_key(|a| a.start);

        Engine {
            autos,
            curves,
            frames_per_beat: f64::from(rate) * 60.0 / tempo.max(1.0),
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
            inst_offs: Vec::with_capacity(64),
            inst_live: Vec::with_capacity(64),
            inst_events: Vec::with_capacity(64),
            scratch: (Vec::new(), Vec::new()),
            inst_tail: 0,
            report,
        }
    }

    /// Plays generator-plugin channel `channel` through `inst` (a shared instance that outlives this engine).
    /// Plugin channels, and sampler channels played by a hosted sampler plugin (OpenStudio hosts Open Sampler for every
    /// sampler channel): the channel's notes go to `inst` instead of the engine's own sample voices.
    pub fn set_instrument(&mut self, channel: usize, inst: SharedInstrument) {
        if let Some(ch) = self.channels.get_mut(channel) {
            if ch.plugin && ch.instrument.0.is_none() {
                self.report.plugin_channels_silent = self.report.plugin_channels_silent.saturating_sub(1);
            }
            ch.plugin = true;
            ch.instrument = Slot(Some(inst));
        }
    }

    /// True when the channel is played by an attached instrument.
    pub fn has_instrument(&self, channel: usize) -> bool {
        self.channels.get(channel).is_some_and(|c| c.instrument.0.is_some())
    }

    /// A live note on an instrument channel that ends by itself after `frames` (note previews).
    pub fn note_for(&mut self, channel: usize, key: u16, velocity: u8, frames: u64) {
        if !self.has_instrument(channel) {
            return;
        }
        let k = key.min(127) as u8;
        self.inst_live.push((channel, k, f32::from(velocity) / 128.0));
        self.inst_offs.push((self.frame + frames.max(1), channel, k));
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
        if ch.instrument.0.is_some() {
            self.inst_live.push((channel, key.min(127) as u8, f32::from(velocity) / 128.0));
            self.sounding.push((channel, key, u64::MAX));
            return;
        }
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
            age: 0,
            off: None,
        });
        self.sounding.push((channel, key, u64::MAX));
    }

    /// Release a live note with a short fade.
    pub fn note_off(&mut self, channel: usize, key: u16) {
        if self.has_instrument(channel) && self.sounding.iter().any(|&(c, k, end)| c == channel && k == key && end == u64::MAX) {
            self.inst_live.push((channel, key.min(127) as u8, -1.0));
        }
        let fade = (f64::from(self.rate) * RELEASE_SECONDS) as u32;
        let vol_env = self.channels.get(channel).is_some_and(|c| c.vol_env.is_some());
        for v in &mut self.voices {
            if v.channel == channel && v.live == Some(key) && v.release.is_none() && v.off.is_none() {
                if vol_env {
                    // The volume envelope's release ends the note.
                    v.off = Some(v.age);
                } else {
                    v.release = Some((fade.max(1), fade.max(1)));
                }
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

    /// The playback position in ticks at the engine's tempo (a looping engine wraps to 0 at its end).
    pub fn tick_position(&self) -> f64 {
        self.frame as f64 / f64::from(self.rate.max(1)) * self.tempo / 60.0 * f64::from(self.ppq)
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
        !self.looping && self.frame >= self.end_frame && self.next_trigger >= self.triggers.len() && self.voices.is_empty() && self.inst_tail == 0 && self.inst_live.is_empty()
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
        self.inst_live = std::mem::take(&mut old.inst_live);
        self.inst_tail = old.inst_tail;
        self.inst_offs = std::mem::take(&mut old.inst_offs);
        for o in &mut self.inst_offs {
            o.0 = to_new(o.0);
        }
        self.inst_offs.retain(|o| o.1 < n);
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

    /// Applies every automation clip that covers the current frame to its target (once per block; volumes and
    /// pans glide over the block, the mixer ramps its matrix). Outside its clips a target keeps its last value.
    fn run_automation(&mut self, frames: usize) {
        use os_model::target::Target;
        if self.autos.is_empty() {
            return;
        }
        let now = self.frame;
        let len = frames.max(1) as u32;
        let mut done: Vec<Target> = Vec::new();
        for a in self.autos.iter().rev() {
            if now < a.start || now >= a.end || done.contains(&a.target) {
                continue;
            }
            done.push(a.target);
            let beat = a.from_beat + (now - a.start) as f64 / self.frames_per_beat;
            let Some(v) = self.curves[a.curve].value_at(beat) else { continue };
            let v = v.clamp(0.0, 1.0);
            // spec: flp/events/automation.md "Targets" (value scaling)
            match a.target {
                Target::ChannelVolume(c) => {
                    if let Some(ch) = self.channels.get_mut(usize::from(c)) {
                        ch.glide(Some(v), None, len);
                    }
                }
                Target::ChannelPan(c) => {
                    if let Some(ch) = self.channels.get_mut(usize::from(c)) {
                        ch.glide(None, Some((v - 0.5) * 2.0), len);
                    }
                }
                Target::ChannelPitch(c) => {
                    if let Some(ch) = self.channels.get_mut(usize::from(c)) {
                        let cents = ((v - 0.5) * 2.0 * AUTO_PITCH_CENTS).round() as i32;
                        ch.base_rate = ch.src_ratio * laws::playback_rate(60, cents);
                    }
                }
                Target::MixerVolume(t) | Target::MixerPan(t) => {
                    if let Some(m) = self.tracks.get_mut(usize::from(t)) {
                        if matches!(a.target, Target::MixerVolume(_)) {
                            m.vol = v;
                        } else {
                            m.pan = (v - 0.5) * 2.0;
                        }
                        let g = if m.on { laws::fader_gain(m.vol) } else { 0.0 };
                        m.matrix = laws::mixer_pan(m.pan).map(|row| row.map(|x| (x * g) as f32));
                    }
                }
                Target::Tempo | Target::Other(_) => {}
            }
        }
    }

    /// Renders the next `out.len() / 2` stereo frames (interleaved L/R) into `out`.
    pub fn process(&mut self, out: &mut [f32]) {
        let frames = out.len() / 2;
        self.run_automation(frames);
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
            if self.channels[t.channel].plugin {
                if self.channels[t.channel].instrument.0.is_some() {
                    let k = t.key.min(127) as u8;
                    self.inst_events.push((t.channel, (offset - a) as u32, k, f32::from(t.velocity) / 128.0));
                    self.inst_offs.push((t.end, t.channel, k));
                }
                continue;
            }
            let remaining = t.cut.map(|c| c.saturating_sub(t.frame));
            let gain = self.channels[t.channel].start_gain(t.vel, t.pan);
            let serial = self.channels[t.channel].serial;
            let mut v = Voice {
                channel: t.channel,
                pos: t.from,
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
                age: 0,
                // A note-off only matters with the volume envelope on: otherwise the sampler plays on (measured).
                off: self.channels[t.channel].vol_env.map(|_| t.end.saturating_sub(t.frame)),
            };
            if render_voice(&mut v, &self.channels, &mut self.bus, a, offset, b) {
                self.voices.push(v);
            }
        }
        self.render_instruments(a, b, span_end);
        for ch in &mut self.channels {
            ch.advance((b - a) as u32);
        }
        self.frame = span_end;
        self.sounding.retain(|&(_, _, end)| end > span_end);
    }

    /// Sends this span's note events to each instrument and mixes its output into the channel's insert with the
    /// channel's volume and pan. An instrument that is busy (locked elsewhere) is skipped for the span.
    fn render_instruments(&mut self, a: usize, b: usize, span_end: u64) {
        let mut i = 0;
        while i < self.inst_offs.len() {
            let (end, ch, key) = self.inst_offs[i];
            if end < span_end {
                self.inst_events.push((ch, end.saturating_sub(self.frame) as u32, key, -1.0));
                self.inst_offs.swap_remove(i);
            } else {
                i += 1;
            }
        }
        for (ch, key, vel) in self.inst_live.drain(..) {
            self.inst_events.push((ch, 0, key, vel));
        }
        self.inst_events.sort_by_key(|e| e.1);
        let held = !self.inst_offs.is_empty()
            || self.sounding.iter().any(|&(c, _, end)| end == u64::MAX && self.channels.get(c).is_some_and(|ch| ch.instrument.0.is_some()));
        if held || !self.inst_events.is_empty() {
            self.inst_tail = u64::from(self.rate) * INSTRUMENT_TAIL_SECONDS;
        } else {
            self.inst_tail = self.inst_tail.saturating_sub((b - a) as u64);
        }
        let n = b - a;
        if self.scratch.0.len() < n {
            self.scratch.0.resize(n, 0.0);
            self.scratch.1.resize(n, 0.0);
        }
        let mut active = false;
        for (c, ch) in self.channels.iter().enumerate() {
            let Some(inst) = &ch.instrument.0 else { continue };
            let Ok(mut inst) = inst.try_lock() else { continue };
            for &(_, off, key, vel) in self.inst_events.iter().filter(|e| e.0 == c) {
                let off = off.min(n.saturating_sub(1) as u32);
                if vel < 0.0 { inst.note_off(off, key) } else { inst.note_on(off, key, vel) }
            }
            let (l, r) = (&mut self.scratch.0[..n], &mut self.scratch.1[..n]);
            inst.process(self.rate, l, r);
            // A sample that is still playing must not be cut off by the idle tail.
            active |= inst.active();
            let (gl, gr) = ch.gain(1.0, 0.0);
            let out = &mut self.bus[ch.insert];
            for f in 0..n {
                out[(a + f) * 2] += l[f] * gl;
                out[(a + f) * 2 + 1] += r[f] * gr;
            }
        }
        self.inst_events.clear();
        if active {
            self.inst_tail = u64::from(self.rate) * INSTRUMENT_TAIL_SECONDS;
        }
    }

    /// Loop point: back to frame 0 (sounding notes keep their remaining length).
    fn wrap(&mut self) {
        let at = self.frame;
        for o in &mut self.inst_offs {
            o.0 = o.0.saturating_sub(at);
        }
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
/// Notes still held on instruments end when the engine goes away (stop, or a new song).
impl Drop for Engine {
    fn drop(&mut self) {
        for &(_, c, key) in &self.inst_offs {
            if let Some(Ok(mut inst)) = self.channels.get(c).and_then(|ch| ch.instrument.0.as_ref()).map(|i| i.lock()) {
                inst.note_off(0, key);
            }
        }
        for &(c, key, _) in self.sounding.iter().filter(|s| s.2 == u64::MAX) {
            if let Some(Ok(mut inst)) = self.channels.get(c).and_then(|ch| ch.instrument.0.as_ref()).map(|i| i.lock()) {
                inst.note_off(0, key.min(127) as u8);
            }
        }
    }
}

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
    let base_step = ch.base_rate * v.step;
    let mut step = base_step;
    let out = &mut bus[ch.insert];
    // Rate 1 copies samples only while the position is whole (a voice that was pitched keeps its fraction).
    let exact = step == 1.0 && v.pos.fract() == 0.0 && ch.pitch_env.is_none();
    let mut scale = step.max(1.0);
    let off_s = v.off.map(|o| o as f64 * ch.frame_s);
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
        let t_s = v.age as f64 * ch.frame_s;
        v.age += 1;
        let env_gain = match &ch.vol_env {
            None => 1.0,
            Some(p) => match p.level(t_s, off_s) {
                Some(l) => l as f32,
                None => {
                    v.pos = pos;
                    v.gain = (gl, gr);
                    v.ramp_left = left;
                    return false;
                }
            },
        };
        if let Some(p) = &ch.pitch_env {
            step = base_step * 2f64.powf(p.level(t_s, off_s).unwrap_or(0.0) * p.amount / 1200.0);
            scale = step.max(1.0);
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
        out[f * 2] += l * gl * fade * env_gain;
        out[f * 2 + 1] += r * gr * fade * env_gain;
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
    /// Seconds into the sample where the voice starts (audio clip window start).
    from_s: f64,
}

impl Sched {
    fn note(start: f64, n: &os_model::Note) -> Self {
        Sched { tick: start + f64::from(n.position), channel: usize::from(n.channel), key: n.key, velocity: n.velocity, pan: n.pan, cut: None, len: f64::from(n.length), from_s: 0.0 }
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
            let clips = project.arrangement().map(|a| a.clips.as_slice()).unwrap_or(&[]);
            if clips.is_empty() {
                place(&mut out, 0.0, f64::INFINITY, project.current_pattern.unwrap_or(1));
            }
            for c in clips.iter().filter(|c| clip_audible(project, c)) {
                if let ClipSource::Pattern(n) = c.source {
                    // Points inside the clip's window, placed relative to it.
                    let from = c.window.map_or(0.0, |w| w.0);
                    for p in points(n).iter().filter(|p| f64::from(p.position) >= from && f64::from(p.position) < from + f64::from(c.length)) {
                        out.push((f64::from(c.position) - from + f64::from(p.position), usize::from(p.channel()), p.param(), p.value));
                    }
                }
            }
        }
    }
    out
}

/// A clip plays unless it is muted or its playlist track is (spec: ui/playlist.md §2, §6).
fn clip_audible(project: &Project, c: &os_model::Clip) -> bool {
    let track_on = project.arrangement().and_then(|a| a.tracks.get(usize::from(c.track))).is_none_or(|t| t.enabled);
    !c.muted && track_on
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
    // An explicit length (channel rack length box, spec: ui/channel-rack.md §7) wins over whole bars.
    let explicit = |n: u16| project.patterns.iter().find(|p| p.number == n).and_then(|p| p.length).filter(|&l| l > 0).map(f64::from);
    let pattern_end = |n: u16| {
        explicit(n).unwrap_or_else(|| {
            let last = pattern_notes(n).iter().map(|x| f64::from(x.position) + f64::from(x.length)).fold(0.0, f64::max);
            ((last / bar).floor() + 1.0).max(1.0) * bar
        })
    };
    let one_pattern = |n: u16| {
        // Muted notes are silent (spec: ui/piano-roll.md §9); notes past an explicit length are not played.
        let end = pattern_end(n);
        let mut t = Vec::new();
        for x in pattern_notes(n).iter().filter(|x| !x.muted() && f64::from(x.position) < end) {
            push_note(&mut t, 0.0, x, project.ppq);
        }
        (t, end)
    };
    match mode {
        PlayMode::Pattern(n) => one_pattern(n),
        PlayMode::Song => {
            let clips = project.arrangement().map(|a| a.clips.as_slice()).unwrap_or(&[]);
            if clips.is_empty() {
                return one_pattern(project.current_pattern.unwrap_or(1));
            }
            let mut out = Vec::new();
            let mut end: f64 = 0.0;
            for c in clips {
                let start = f64::from(c.position);
                let len = f64::from(c.length);
                end = end.max(start + len);
                // Muted clips and clips on muted tracks are silent (spec: ui/playlist.md §2, §6).
                if !clip_audible(project, c) {
                    continue;
                }
                match c.source {
                    // A pattern clip plays the notes inside its window [from, from + length) (spec: playlist.md "Clip offsets").
                    ClipSource::Pattern(n) => {
                        let from = c.window.map_or(0.0, |w| w.0);
                        for note in pattern_notes(n).iter().filter(|x| !x.muted()) {
                            let p = f64::from(note.position);
                            if p >= from && p < from + len {
                                push_note(&mut out, start - from, note, project.ppq);
                            }
                        }
                    }
                    ClipSource::Channel(ch) => match project.channels.get(usize::from(ch)).map(|c| c.kind) {
                        // An audio clip plays its channel's sample from the window start (milliseconds), cut at the clip end.
                        Some(ChannelKind::AudioClip | ChannelKind::Sampler) => {
                            let from_s = c.window.map_or(0.0, |w| w.0.max(0.0) / 1000.0);
                            out.push(Sched { tick: start, channel: usize::from(ch), key: 60, velocity: 100, pan: 64, cut: Some(start + len), len, from_s });
                        }
                        Some(ChannelKind::Automation) => {}
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
            envelopes: Default::default(),
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

    /// Records note events and outputs a constant 1.0 while any note is held.
    struct Probe {
        log: Vec<(u64, bool, u8, f32)>,
        held: Vec<u8>,
        at: u64,
    }

    impl crate::Instrument for Probe {
        fn note_on(&mut self, offset: u32, key: u8, velocity: f32) {
            self.log.push((self.at + u64::from(offset), true, key, velocity));
            self.held.push(key);
        }
        fn note_off(&mut self, offset: u32, key: u8) {
            self.log.push((self.at + u64::from(offset), false, key, 0.0));
            self.held.retain(|&k| k != key);
        }
        fn process(&mut self, _rate: u32, l: &mut [f32], r: &mut [f32]) {
            let v = if self.held.is_empty() { 0.0 } else { 1.0 };
            l.fill(v);
            r.fill(v);
            self.at += l.len() as u64;
        }
    }

    // spec: plugins/plugin-formats.md (hosting: notes at frame offsets; the channel's volume and pan apply)
    #[test]
    fn instrument_channels_get_timed_notes() {
        let mut p = project(vec![long_note(96, 48, 0)], vec![]);
        p.channels[0].kind = ChannelKind::Plugin;
        let mut lib = SampleLibrary::default();
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Pattern(1));
        let probe = Arc::new(std::sync::Mutex::new(Probe { log: vec![], held: vec![], at: 0 }));
        e.set_instrument(0, probe.clone());
        assert_eq!(e.report.plugin_channels_silent, 0);
        let mut out = vec![0.0f32; 512 * 2];
        let mut frames = Vec::new();
        for _ in 0..200 {
            e.process(&mut out);
            frames.extend_from_slice(&out);
        }
        let on = laws::tick_to_frame(96.0, 96, 130.0, 48000);
        let off = laws::tick_to_frame(144.0, 96, 130.0, 48000);
        let log = probe.lock().unwrap().log.clone();
        assert_eq!(log.len(), 2, "{log:?}");
        assert_eq!((log[0].0, log[0].1, log[0].2), (on, true, 60));
        assert!((log[0].3 - 100.0 / 128.0).abs() < 1e-6);
        assert_eq!((log[1].0, log[1].1), (off, false));
        // Output = 1.0 x channel gain x pan law while the note is held.
        let (gl, _) = laws::channel_pan(0.0);
        let g = (laws::channel_gain(0.78125) * gl) as f32;
        assert_eq!(frames[(on as usize - 1024) * 2], 0.0);
        let held = (on as usize + 256) * 2;
        assert!((frames[held] - g).abs() < 1e-6, "{} vs {g}", frames[held]);
        // (The probe holds its level for a whole process call; the next span after the note-off is silent.)
        assert_eq!(frames[(off as usize + 1024) * 2], 0.0);
    }

    /// A sampler channel played by a hosted sampler (OpenStudio's Open Sampler): its notes go to the instrument, the
    /// engine's own voice for the channel (the empty sampler's tone) stays silent.
    #[test]
    fn sampler_channels_can_be_played_by_an_instrument() {
        let p = project(vec![long_note(0, 48, 0)], vec![]);
        assert_eq!(p.channels[0].kind, ChannelKind::Sampler);
        let mut lib = SampleLibrary::default();
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Pattern(1));
        let probe = Arc::new(std::sync::Mutex::new(Probe { log: vec![], held: vec![], at: 0 }));
        e.set_instrument(0, probe.clone());
        assert!(e.has_instrument(0));
        let mut out = vec![0.0f32; 512 * 2];
        let mut frames = Vec::new();
        for _ in 0..40 {
            e.process(&mut out);
            frames.extend_from_slice(&out);
        }
        let log = probe.lock().unwrap().log.clone();
        assert_eq!((log[0].1, log[0].2), (true, 60), "{log:?}");
        // Only the probe's flat level: no tone voice on top of it.
        let (gl, _) = laws::channel_pan(0.0);
        let g = (laws::channel_gain(p.channels[0].volume) * gl) as f32;
        assert!(frames.chunks_exact(2).all(|f| f[0] == 0.0 || (f[0] - g).abs() < 1e-6));
    }

    #[test]
    fn dropping_an_engine_releases_held_instrument_notes() {
        let mut p = project(vec![], vec![]);
        p.channels[0].kind = ChannelKind::Plugin;
        let mut lib = SampleLibrary::default();
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Pattern(1));
        let probe = Arc::new(std::sync::Mutex::new(Probe { log: vec![], held: vec![], at: 0 }));
        e.set_instrument(0, probe.clone());
        e.note_on(0, 64, 100);
        let mut out = vec![0.0f32; 256];
        e.process(&mut out);
        drop(e);
        assert!(probe.lock().unwrap().held.is_empty(), "note released on drop");
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

    fn last_sound(x: &[f32]) -> usize {
        x.chunks_exact(2).rposition(|f| f[0].abs() > 1e-5).unwrap_or(0)
    }

    fn with_envelope(index: usize, e: os_model::Envelope) -> Project {
        let mut p = project(vec![long_note(0, 24, 0)], vec![]);
        p.channels[0].envelopes[index] = e;
        p
    }

    #[test]
    fn volume_envelope_shapes_and_ends_the_note() {
        // spec: audio/channel-envelopes.md
        let plain = render(&project(vec![long_note(0, 24, 0)], vec![]));
        assert!(last_sound(&plain) > 44100, "the plain tone rings past a second");
        let off = os_model::Envelope { enabled: true, delay: 100, attack: 100, hold: 100, decay: 100, sustain: 128, release: 100, ..Default::default() };
        // Envelope on, nothing set: full level, and the note ends with its length (24 ticks = 0.115 s) plus a short fade.
        let on = render(&with_envelope(os_model::Envelope::VOLUME, off));
        let end = last_sound(&on);
        assert!(end < ((0.115 + 0.02) * 44100.0) as usize, "ended at {end}");
        assert!(end > (0.115 * 44100.0) as usize, "ended at {end}");
        // A 0.83 s attack keeps the start quiet.
        let slow = render(&with_envelope(os_model::Envelope::VOLUME, os_model::Envelope { attack: 25000, ..off }));
        let head = |x: &[f32]| x[..4410].chunks_exact(2).map(|f| f[0].abs()).fold(0.0, f32::max);
        assert!(head(&slow) < 0.12 * head(&on), "{} vs {}", head(&slow), head(&on));
        // A release of 0.47 s lets the note ring on after its length.
        let rel = render(&with_envelope(os_model::Envelope::VOLUME, os_model::Envelope { release: 20000, ..off }));
        assert!(last_sound(&rel) > 44100 / 2 && last_sound(&rel) > end + 10_000);
    }

    #[test]
    fn pitch_envelope_shifts_the_sample() {
        let plain = render(&project(vec![long_note(0, 24, 0)], vec![]));
        let up = os_model::Envelope { enabled: true, delay: 100, attack: 100, hold: 100, decay: 100, sustain: 128, release: 20000, amount: 1200, ..Default::default() };
        let high = render(&with_envelope(os_model::Envelope::PITCH, up));
        // An octave up while the note is held, falling back over the release: the 1.5 s tone ends sooner.
        assert!(last_sound(&plain) > 60_000 && last_sound(&high) + 8_000 < last_sound(&plain), "{} vs {}", last_sound(&plain), last_sound(&high));
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

    fn song(notes: Vec<Note>, clips: Vec<os_model::Clip>) -> Project {
        let mut p = project(notes, vec![]);
        p.arrangements = vec![os_model::Arrangement {
            clips,
            tracks: (1..=4).map(|n| os_model::PlaylistTrack { number: n, name: None, color: None, enabled: true }).collect(),
            ..Default::default()
        }];
        p
    }

    fn song_triggers(p: &Project) -> Vec<u64> {
        let mut lib = SampleLibrary::new(None);
        Engine::new(p, &mut lib, 48000, PlayMode::Song).triggers.iter().map(|t| t.frame).collect()
    }

    // spec: ui/playlist.md §2, §6 (muted clips / tracks are silent), flp/events/playlist.md "Clip offsets"
    #[test]
    fn song_respects_mutes_and_clip_windows() {
        use os_model::{Clip, ClipSource};
        let notes = vec![Note::step(0, 0), Note::step(0, 96), Note::step(0, 192)];
        let f = |t: f64| laws::tick_to_frame(t, 96, 130.0, 48000);
        // A clip at bar 2 with the window [96, 288): the notes at 96 and 192 play at 384 and 480.
        let mut c = Clip::new(384, 192, 0, ClipSource::Pattern(1));
        c.window = Some((96.0, 288.0));
        assert_eq!(song_triggers(&song(notes.clone(), vec![c])), vec![f(384.0), f(480.0)]);
        let mut m = c;
        m.muted = true;
        assert!(song_triggers(&song(notes.clone(), vec![m])).is_empty(), "muted clip");
        let mut p = song(notes.clone(), vec![c]);
        p.arrangements[0].tracks[0].enabled = false;
        assert!(song_triggers(&p).is_empty(), "muted track");
        // An unwindowed clip plays the pattern from its start, cut at the clip length.
        let plain = Clip::new(0, 100, 1, ClipSource::Pattern(1));
        assert_eq!(song_triggers(&song(notes, vec![plain])), vec![f(0.0), f(96.0)]);
    }

    // spec: ui/playlist.md §8 (automation clips drive their target), flp/events/automation.md (value scaling)
    #[test]
    fn automation_clips_drive_channel_and_mixer() {
        use os_model::{AutoPoint, Automation, ChannelKind, Clip, ClipSource, target};
        let mut p = song(vec![long_note(0, 384 * 4, 0)], vec![Clip::new(0, 384 * 4, 0, ClipSource::Pattern(1))]);
        let mut auto_ch = channel();
        auto_ch.index = 1;
        auto_ch.kind = ChannelKind::Automation;
        p.channels.push(auto_ch);
        p.mixer = (0..2)
            .map(|i| os_model::MixerTrack {
                index: i, name: None, color: None, enabled: true, slots: Default::default(), routes: if i == 0 { vec![] } else { vec![0] },
                send_levels: vec![], volume: Some(0.8), pan: Some(0.0), stereo_separation: Some(0.0),
            })
            .collect();
        // Channel volume falls from 1.0 to 0.0 over 8 beats, starting at bar 2.
        let a = Automation { channel: 1, target: Some(target::channel(0, 0)), points: vec![AutoPoint::new(0.0, 1.0), AutoPoint::new(8.0, 0.0)], header: vec![0; 21], tail: vec![] };
        p.automations.push(a);
        p.arrangements[0].clips.push(Clip::new(384, 768, 1, ClipSource::Channel(1)));
        let mut lib = SampleLibrary::new(None);
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Song);
        assert_eq!(e.autos.len(), 1);
        let mut out = vec![0.0f32; 480 * 2];
        let at = |e: &mut Engine, tick: f64| {
            let frame = laws::tick_to_frame(tick, 96, 130.0, 48000);
            while e.frame < frame {
                e.process(&mut out.clone());
            }
            e.channels[0].vol_v
        };
        assert!((at(&mut e, 100.0) - 0.78125).abs() < 1e-9, "before the clip: the channel's own volume");
        let mid = at(&mut e, 384.0 + 384.0);
        assert!((mid - 0.5).abs() < 0.02, "half way: {mid}");
        let end = at(&mut e, 384.0 + 760.0);
        assert!(end < 0.03, "near the end: {end}");
        let after = at(&mut e, 384.0 * 3.5);
        assert!(after < 0.03, "after the clip it keeps the last value: {after}");
        // A mixer pan clip moves the track's matrix.
        p.automations[0].target = Some(target::mixer(1, target::MIXER_PAN));
        p.automations[0].points = vec![AutoPoint::new(0.0, 0.0), AutoPoint::new(8.0, 0.0)];
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Song);
        e.seek(laws::tick_to_frame(400.0, 96, 130.0, 48000));
        e.process(&mut out);
        let m = e.tracks[1].matrix;
        assert!(m[1][0].abs() < 1e-6 && m[1][1].abs() < 1e-6, "full left: no right output {m:?}");
        let _ = &mut out;
    }
}
