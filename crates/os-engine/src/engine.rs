//! Tick-based playback engine: schedules notes from patterns / playlist clips on the sequencer's tick clock, plays
//! sampler and audio-clip channels, and sums them through the mixer routing graph. Everything that is scheduled
//! happens at a tick's start; the engine renders whole ticks and hands them out in the block sizes the caller asks
//! for, so live playback and an offline render produce the same samples.
//!
// spec: audio/timing.md (tick clock, what a tick does, the song's end, the tail), audio/swing.md,
//       audio/song-structure.md, audio/implementing.md ("Playing live"), audio/sampler-playback.md,
//       audio/gain-laws.md, flp/events/playlist.md (clips), flp/events/patterns.md (notes),
//       flp/events/mixer.md (routing), audio/pattern-controls.md (repeat, control points),
//       audio/channel-envelopes.md (volume and pitch envelopes)

use std::sync::Arc;

use os_model::{ChannelKind, ClipSource, ControlPoint, Project};

use crate::clock::Clock;
use os_dsp::glide::Glide;
use crate::effects::Effect;
use crate::envelope;
use crate::instrument::{SharedInstrument, Slot, TransportInfo};
use crate::laws;
use crate::sample::{Sample, SampleLibrary};

mod notes;
mod param_auto;
pub use param_auto::PARAM_STEP;

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
    /// Tick at which the note starts.
    tick: u64,
    channel: usize,
    /// Key rate; the voice plays at the channel's current base rate times this.
    rate: f64,
    /// Tick at which the voice is cut (end of an audio clip), if any.
    cut: Option<u64>,
    /// Velocity gain and note pan (âˆ’1..=1); the channel's volume and pan apply while it plays.
    vel: f64,
    pan: f64,
    /// Note key.
    key: u16,
    /// Tick at which the note is released; `None` = a step (length 0), which has no end of its own and is released
    /// with the song (spec: audio/song-structure.md, "A note").
    end: Option<u64>,
    /// Note velocity byte (instrument channels get it as 0..=1).
    velocity: u8,
    /// Sample position (source frames) the voice starts at.
    from: f64,
    /// The same as a fraction of the sample (instrument channels: the sampler plugin starts there).
    start: f32,
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
    /// Polyphony, mono, portamento and cut group rules for the instrument's notes (spec: audio/channel-voices.md).
    rules: notes::Rules,
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

/// A channel control point placed on its tick.
#[derive(Debug, Clone, Copy)]
struct CtlEvent {
    tick: u64,
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
    /// Tick at which the voice is cut (the end of an audio clip), if any.
    cut: Option<u64>,
    /// Tick at which the note is released (the envelopes' release starts); `None` = released with the song.
    release_at: Option<u64>,
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

/// An automation clip placed on the tick line (spec: ui/playlist.md Â§8, flp/events/automation.md).
#[derive(Debug, Clone, Copy)]
struct AutoClip {
    /// Ticks at which the clip starts and ends.
    start: u64,
    end: u64,
    /// Beats into the automation at the clip start (its window).
    from_beat: f64,
    /// Index into `Engine::curves`.
    curve: usize,
    target: os_model::target::Target,
}

struct MixTrack {
    /// Fader (0..16000, 12800 = unity), pan (-6400..6400) and stereo separation (-64..64, positive merges) as stored;
    /// automation rewrites them, then the matrix (spec: audio/mixer.md).
    vol: i32,
    pan: i32,
    sep: i32,
    on: bool,
    /// Swap left / right and reverse polarity (spec: ui/mixer.md §4; OpenStudio applies them to the matrix).
    swap: bool,
    polarity: bool,
    /// The three-band equaliser, after the slots and before the fader.
    eq: os_dsp::mixer::TrackEq,
    /// Insert effects in slot order (applied before the fader).
    effects: Vec<FxSlot>,
    /// Slot LEDs and mix levels by slot, for hosted effects attached later.
    slot_on: [bool; 10],
    slot_mix: [f32; 10],
    /// Largest |sample| per side after the fader since the meters last read them.
    peak: [f32; 2],
    /// The fader matrix's target gains `[[L←L, L←R], [R←L, R←R]]`, and the gains as they glide towards them.
    matrix: [[f32; 2]; 2],
    gains: [[Glide; 2]; 2],
    /// (destination, send gain). Sidechain-only routes (level 0) are dropped.
    routes: Vec<(usize, f32)>,
}

/// One filled mixer slot: OpenStudio's own effect or a hosted plugin, with the slot's LED and mix level.
struct FxSlot {
    slot: usize,
    on: bool,
    mix: f32,
    fx: Fx,
}

enum Fx {
    Own(Box<dyn Effect>),
    Hosted(crate::instrument::SharedEffect),
}

impl MixTrack {
    fn unity() -> MixTrack {
        let unity = [[1.0, 0.0], [0.0, 1.0]];
        MixTrack {
            vol: 12800,
            pan: 0,
            sep: 0,
            on: true,
            swap: false,
            polarity: false,
            eq: os_dsp::mixer::TrackEq::default(),
            effects: Vec::new(),
            slot_on: [true; 10],
            slot_mix: [1.0; 10],
            peak: [0.0; 2],
            matrix: unity,
            gains: unity.map(|r| r.map(Glide::new)),
            routes: vec![],
        }
    }

    /// The fader matrix (volume, pan, stereo separation, switch; spec: audio/mixer.md "Fader matrix") with swap and
    /// polarity on top, as `[[L←L, L←R], [R←L, R←R]]`.
    fn compute_matrix(&self) -> [[f32; 2]; 2] {
        let [ll, rl, rr, lr] = os_dsp::mixer::fader_matrix(self.vol, self.pan, self.sep, self.on);
        let mut m = [[ll, rl], [lr, rr]];
        if self.swap {
            m = [[m[0][1], m[0][0]], [m[1][1], m[1][0]]];
        }
        if self.polarity {
            m = m.map(|r| r.map(|g| -g));
        }
        m
    }
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
    /// Channel control points in tick order, and the next one to apply.
    controls: Vec<CtlEvent>,
    next_ctl: usize,
    /// Notes started and not yet released: (channel, key, release tick; `u64::MAX` = a live note).
    sounding: Vec<(usize, u16, u64)>,
    voices: Vec<Voice>,
    /// The tick clock of the current pass, the next tick to render and its first sample.
    clock: Clock,
    tick: u64,
    frame: u64,
    /// Tick at which the scheduled material ends: the song's end (voices may ring past it).
    end_tick: u64,
    /// Restart at tick 0 on reaching `end_tick` (pattern playback).
    looping: bool,
    /// Only live notes, no schedule: finished once everything is silent; the tail rule does not apply.
    live_only: bool,
    /// The tail rule has ended the song: a tick at or after its end stayed quiet (spec: audio/timing.md).
    tail_done: bool,
    tempo: f64,
    ppq: u16,
    bus: Vec<Vec<f32>>,
    /// The last rendered tick (interleaved L/R), how much of it has been handed out, its tick index and first sample.
    out: Vec<f32>,
    out_pos: usize,
    out_tick: u64,
    out_frame: u64,
    /// Instrument channels: scheduled note ends (tick, channel, key, cut = choke at the end of an audio clip), live
    /// notes waiting for the next tick (channel, key, velocity; negative = note off), this tick's events (channel,
    /// offset, key, velocity (`OFF` / `CHOKE` below zero), sample start fraction) and scratch.
    inst_offs: Vec<(u64, usize, u8, bool)>,
    inst_live: Vec<(usize, u8, f32)>,
    inst_events: Vec<(usize, u32, u8, f32, f32)>,
    /// This tick's events after the channels' note rules, sorted by channel, and each channel's notes at its
    /// instrument (spec: audio/channel-voices.md).
    inst_notes: Vec<(usize, u32, notes::Note)>,
    note_voices: Vec<notes::Voices>,
    scratch: (Vec<f32>, Vec<f32>),
    /// Dry copy of a track buffer for slots mixed below 100 %.
    fx_dry: Vec<f32>,
    /// Frames instruments keep being processed after their last held note (their release / tail is not known).
    inst_tail: u64,
    /// A hosted instrument reported sound still to come during the last tick.
    inst_active: bool,
    /// Automation clips of the playlist and their curves (song mode).
    autos: Vec<AutoClip>,
    curves: Vec<os_model::Automation>,
    /// Clips on plugin parameters: this tick's events.
    params: param_auto::ParamAuto,
    pub report: EngineReport,
}

/// Seconds an instrument keeps running after its last note (an OpenStudio choice).
const INSTRUMENT_TAIL_SECONDS: u64 = 5;
/// `inst_events` velocity values below zero: a note off, a choke.
const OFF: f32 = notes::OFF;
const CHOKE: f32 = notes::CHOKE;

/// A tick whose every output sample stays below this magnitude (-96 dB) is quiet: the first quiet tick at or after
/// the song's end is the file's last (spec: audio/timing.md, "Where the file ends").
const QUIET: f32 = 1.584_893_1e-5;
/// Safety stop for the tail: minutes rendered past the song's end before giving up on silence (an OpenStudio choice).
const TAIL_LIMIT_SECONDS: f64 = 600.0;

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
                    rules: notes::Rules::new(&ch.misc, rate),
                }
            })
            .collect();

        let tracks: Vec<MixTrack> = if project.mixer.is_empty() {
            vec![MixTrack::unity()]
        } else {
            project
                .mixer
                .iter()
                .map(|m| {
                    let mut effects = Vec::new();
                    for (k, slot) in m.slots.iter().enumerate() {
                        let Some(slot) = slot else { continue };
                        match crate::effects::from_slot(&slot.name, &slot.state, rate) {
                            Some(fx) => {
                                effects.push(FxSlot { slot: k, on: m.slot_on[k], mix: m.slot_mix[k] as f32, fx: Fx::Own(fx) });
                                report.effects_active += 1;
                            }
                            None => report.effects_bypassed += 1,
                        }
                    }
                    let routes = if m.index == 0 {
                        vec![]
                    } else {
                        m.routes
                            .iter()
                            .filter(|&&r| usize::from(r) < n_tracks && r != m.index)
                            .map(|&r| {
                                // A route without a level record is at unity (spec: flp/engine-events.md "The mixer").
                                let level = m.send_levels.iter().find(|(d, _)| *d == r).map(|&(_, l)| l);
                                (usize::from(r), level.map_or(1.0, |l| os_dsp::mixer::send_gain((l * 16000.0).round() as i32)))
                            })
                            .filter(|&(_, g)| g > 0.0)
                            .collect()
                    };
                    let mut t = MixTrack {
                        vol: (m.volume.unwrap_or(0.8) * 16000.0).round() as i32,
                        pan: (m.pan.unwrap_or(0.0) * 6400.0).round() as i32,
                        sep: (m.stereo_separation.unwrap_or(0.0) * 64.0).round() as i32,
                        on: m.enabled,
                        swap: m.swap,
                        polarity: m.polarity,
                        eq: os_dsp::mixer::TrackEq::new(m.eq, rate),
                        effects,
                        slot_on: m.slot_on,
                        slot_mix: m.slot_mix.map(|v| v as f32),
                        routes,
                        ..MixTrack::unity()
                    };
                    // At the start of the song every gain is at its target.
                    t.matrix = t.compute_matrix();
                    t.gains = t.matrix.map(|r| r.map(Glide::new));
                    t
                })
                .collect()
        };
        let order = processing_order(&tracks);

        let mut controls: Vec<CtlEvent> = schedule_controls(project, mode)
            .into_iter()
            .filter(|&(_, channel, _, _)| channel < channels.len())
            .map(|(tick, channel, param, value)| CtlEvent { tick, channel, param, value })
            .collect();
        controls.sort_by_key(|c| c.tick); // stable: equal ticks keep their order
        let (mut sched, end_tick) = schedule(project, mode, &mut report);
        sched.sort_by_key(|s| s.tick);
        let triggers = sched
            .into_iter()
            .filter_map(|n: Sched| {
                let state = channels.get(n.channel)?;
                if state.sample.is_none() && !state.plugin {
                    return None;
                }
                Some(Trigger {
                    tick: n.tick,
                    channel: n.channel,
                    rate: laws::playback_rate(n.key, 0),
                    cut: n.cut,
                    vel: laws::velocity_gain(n.velocity),
                    pan: laws::note_pan(n.pan),
                    key: n.key,
                    end: (n.len > 0).then_some(n.tick + n.len),
                    velocity: n.velocity,
                    from: state.sample.as_ref().map_or(0.0, |s| n.from_s * f64::from(s.rate)),
                    start: state.sample.as_ref().filter(|s| s.frames() > 0).map_or(0.0, |s| (n.from_s * f64::from(s.rate) / s.frames() as f64) as f32),
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
                // Plugin parameters play (spec: plugins/plugin-parameters.md Â§2); tempo and unknown targets do not.
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
                    start: u64::from(c.position),
                    end: u64::from(c.end()),
                    from_beat: c.window.map_or(0.0, |w| w.0),
                    curve,
                    target,
                });
            }
        }
        // Later clips win where clips on the same target overlap.
        autos.sort_by_key(|a| a.start);

        Engine {
            params: param_auto::ParamAuto::new(autos.iter().any(|a| param_auto::is_param(a.target))),
            autos,
            curves,
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
            clock: Clock::new(rate, ppq, tempo),
            tick: 0,
            frame: 0,
            end_tick,
            looping: false,
            live_only: false,
            tail_done: false,
            tempo,
            ppq,
            out: Vec::with_capacity(8192),
            out_pos: 0,
            out_tick: 0,
            out_frame: 0,
            inst_offs: Vec::with_capacity(64),
            inst_live: Vec::with_capacity(64),
            inst_events: Vec::with_capacity(64),
            inst_notes: Vec::with_capacity(64),
            note_voices: vec![notes::Voices::default(); project.channels.len()],
            scratch: (Vec::new(), Vec::new()),
            fx_dry: Vec::new(),
            inst_tail: 0,
            inst_active: false,
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

    /// Runs mixer slot `slot` of track `track` through a hosted effect plugin (replacing whatever the slot had).
    pub fn set_effect(&mut self, track: usize, slot: usize, fx: crate::instrument::SharedEffect) {
        let Some(t) = self.tracks.get_mut(track) else { return };
        if slot >= 10 {
            return;
        }
        t.effects.retain(|e| e.slot != slot);
        let at = t.effects.partition_point(|e| e.slot < slot);
        t.effects.insert(at, FxSlot { slot, on: t.slot_on[slot], mix: t.slot_mix[slot], fx: Fx::Hosted(fx) });
    }

    /// Peak level per mixer track (L, R) since the last call, after each track's fader; resets them. `out` keeps the
    /// larger of its old and the new values.
    pub fn take_track_peaks(&mut self, out: &mut Vec<[f32; 2]>) {
        out.resize(self.tracks.len(), [0.0; 2]);
        for (o, t) in out.iter_mut().zip(&mut self.tracks) {
            o[0] = o[0].max(t.peak[0]);
            o[1] = o[1].max(t.peak[1]);
            t.peak = [0.0; 2];
        }
    }

    /// True when the channel is played by an attached instrument.
    pub fn has_instrument(&self, channel: usize) -> bool {
        self.channels.get(channel).is_some_and(|c| c.instrument.0.is_some())
    }

    /// A live note on an instrument channel that ends by itself after about `frames` (note previews): it starts on the
    /// next tick and ends on the tick that frame count reaches.
    pub fn note_for(&mut self, channel: usize, key: u16, velocity: u8, frames: u64) {
        if !self.has_instrument(channel) {
            return;
        }
        let k = key.min(127) as u8;
        let ticks = (frames as f64 / self.clock.samples_per_tick()).ceil().max(1.0) as u64;
        self.inst_live.push((channel, k, f32::from(velocity) / 128.0));
        self.inst_offs.push((self.tick + ticks, channel, k, false));
        self.tail_done = false;
    }

    /// Loop the scheduled material: on reaching its end, playback continues from tick 0 (ringing voices carry on).
    /// Used for pattern playback.
    pub fn set_looping(&mut self, on: bool) {
        self.looping = on;
    }

    /// Drop the scheduled notes: the engine only plays live notes (`note_on`).
    pub fn clear_schedule(&mut self) {
        self.triggers.clear();
        self.next_trigger = 0;
        self.end_tick = 0;
        self.live_only = true;
    }

    /// Start a live note on channel `channel` (rack position) at the next tick; it rings until
    /// `note_off` (or the sample ends). Velocity 0..=128, 100 = unity.
    pub fn note_on(&mut self, channel: usize, key: u16, velocity: u8) {
        let Some(ch) = self.channels.get(channel) else { return };
        self.tail_done = false;
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
            cut: None,
            release_at: None,
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

    /// Frames handed out so far in the current pass (the sample being heard; a looping engine starts again at 0).
    pub fn position(&self) -> u64 {
        self.out_frame + (self.out_pos / 2) as u64
    }

    /// The position being heard in ticks, with the fraction of the tick handed out so far.
    pub fn tick_position(&self) -> f64 {
        if self.out.is_empty() {
            return self.tick as f64;
        }
        self.out_tick as f64 + self.out_pos as f64 / self.out.len() as f64
    }

    /// The song's end in frames at the project tempo (voices may ring past it).
    pub fn length(&self) -> u64 {
        Clock::frame_of_tick(self.rate, self.ppq, self.tempo, self.end_tick)
    }

    /// The song's end in ticks.
    pub fn end_tick(&self) -> u64 {
        self.end_tick
    }

    /// True once all scheduled material has been started (voices may still ring): the loop point of a pattern.
    pub fn past_end(&self) -> bool {
        self.tick >= self.end_tick && self.next_trigger >= self.triggers.len()
    }

    /// True once playback is over and everything handed out (never while looping). A song ends by the tail rule: on
    /// the first quiet tick at or after its end. An engine that only plays live notes is over while it is silent and
    /// starts again with the next note.
    pub fn finished(&self) -> bool {
        let drained = self.out_pos >= self.out.len();
        if self.live_only {
            let rest_silent = self.out[self.out_pos.min(self.out.len())..].iter().all(|&s| s == 0.0);
            return rest_silent && self.voices.is_empty() && self.inst_tail == 0 && self.inst_live.is_empty();
        }
        !self.looping && drained && self.tail_done && self.inst_live.is_empty()
    }

    /// Start playback in the tick that contains `frame` (at the project tempo; notes before that tick are not
    /// triggered).
    pub fn seek(&mut self, frame: u64) {
        let (tick, _) = Clock::tick_of_frame(self.rate, self.ppq, self.tempo, frame);
        self.seek_tick(tick);
    }

    /// Start playback at tick `tick`: control points before it are applied, notes before it are not triggered.
    pub fn seek_tick(&mut self, tick: u64) {
        self.clock = Clock::at_tick(self.rate, self.ppq, self.tempo, tick);
        self.tick = tick;
        self.frame = self.clock.next_start();
        self.next_trigger = self.triggers.partition_point(|t| t.tick < tick);
        self.sync_controls(tick);
        self.params.forget();
        self.out.clear();
        self.out_pos = 0;
        self.out_tick = tick;
        self.out_frame = self.frame;
        self.tail_done = false;
    }

    /// Applies the control points due on the tick starting now, smoothing volume and pan.
    fn run_controls(&mut self) {
        while let Some(&e) = self.controls.get(self.next_ctl) {
            if e.tick > self.tick {
                break;
            }
            self.next_ctl += 1;
            if let Some(ch) = self.channels.get_mut(e.channel) {
                ch.apply(e.param, e.value, self.rate);
            }
        }
    }

    /// Channels as they are at tick `tick`: their own settings with every control point before it applied.
    fn sync_controls(&mut self, tick: u64) {
        for ch in &mut self.channels {
            ch.reset();
        }
        self.next_ctl = 0;
        while let Some(&e) = self.controls.get(self.next_ctl) {
            if e.tick >= tick {
                break;
            }
            self.next_ctl += 1;
            if let Some(ch) = self.channels.get_mut(e.channel) {
                ch.apply(e.param, e.value, 0);
            }
        }
    }

    /// Take over playback from `old` (built from an earlier version of the project) at its position: the tick it
    /// already rendered plays out, ringing voices keep playing, notes before the position are not retriggered, and
    /// effect states carry over when the mixer layout is unchanged. Edits are heard from the next tick: ringing voices
    /// take the new channel volume / pan and mixer levels (ramped). A tempo change keeps the tick position and sizes
    /// the ticks after it.
    pub fn continue_from(&mut self, old: &mut Engine) {
        self.looping = old.looping;
        let tick = old.tick;
        self.clock = if self.rate == old.rate && self.ppq == old.ppq {
            let mut c = old.clock;
            c.set_tempo(self.rate, self.ppq, self.tempo);
            c
        } else {
            Clock::at_tick(self.rate, self.ppq, self.tempo, tick)
        };
        self.tick = tick;
        self.frame = self.clock.next_start();
        if self.rate == old.rate {
            self.out = std::mem::take(&mut old.out);
            self.out_pos = old.out_pos;
            self.out_tick = old.out_tick;
            self.out_frame = old.out_frame;
        } else {
            (self.out_tick, self.out_frame) = (tick, self.frame);
        }
        self.next_trigger = self.triggers.partition_point(|t| t.tick < tick);
        self.sync_controls(tick);
        let n = self.channels.len();
        self.voices = std::mem::take(&mut old.voices);
        self.voices.retain(|v| v.channel < n);
        self.sounding = std::mem::take(&mut old.sounding);
        self.inst_live = std::mem::take(&mut old.inst_live);
        // Notes held at the instruments (mono / polyphony / glide state) carry over.
        self.note_voices = std::mem::take(&mut old.note_voices);
        self.note_voices.resize(n, notes::Voices::default());
        self.inst_tail = old.inst_tail;
        self.inst_offs = std::mem::take(&mut old.inst_offs);
        self.inst_offs.retain(|o| o.1 < n);
        if self.rate == old.rate && self.tracks.len() == old.tracks.len() {
            for (new, o) in self.tracks.iter_mut().zip(&mut old.tracks) {
                new.gains = o.gains;
                // Own effects keep their state (delay lines, envelopes) when the slot layout is unchanged; the new
                // slot LEDs and mix levels apply.
                let same = new.effects.len() == o.effects.len() && new.effects.iter().zip(&o.effects).all(|(a, b)| a.slot == b.slot);
                if same {
                    for (a, b) in new.effects.iter_mut().zip(&mut o.effects) {
                        if let (Fx::Own(_), Fx::Own(_)) = (&a.fx, &b.fx) {
                            std::mem::swap(&mut a.fx, &mut b.fx);
                        }
                    }
                }
            }
        }
    }

    /// Notes playing at the current position, as (channel index, key).
    pub fn sounding(&self) -> impl Iterator<Item = (usize, u16)> + '_ {
        self.sounding.iter().map(|&(c, k, _)| (c, k))
    }

    /// Applies every automation clip that covers the tick starting now to its target (volumes and pans glide over
    /// the tick, the mixer ramps its matrix). Outside its clips a target keeps its last value.
    fn run_automation(&mut self, len: usize) {
        use os_model::target::Target;
        if self.autos.is_empty() {
            return;
        }
        let now = self.tick;
        let len = len.max(1) as u32;
        let mut done: Vec<Target> = Vec::new();
        for a in self.autos.iter().rev() {
            if now < a.start || now >= a.end || done.contains(&a.target) {
                continue;
            }
            done.push(a.target);
            let beat = a.from_beat + (now - a.start) as f64 / f64::from(self.ppq);
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
                // Volume Round(16000 v), pan Round((2 v - 1) * 6400) (spec: audio/mixer.md "Changes while the song plays").
                Target::MixerVolume(t) | Target::MixerPan(t) => {
                    if let Some(m) = self.tracks.get_mut(usize::from(t)) {
                        if matches!(a.target, Target::MixerVolume(_)) {
                            m.vol = (16000.0 * v).round_ties_even() as i32;
                        } else {
                            m.pan = ((2.0 * v - 1.0) * 6400.0).round_ties_even() as i32;
                        }
                        m.matrix = m.compute_matrix();
                    }
                }
                // Plugin parameters: sampled through the tick by `run_param_automation`.
                Target::ChannelParam(..) | Target::SlotParam(..) | Target::Tempo | Target::Other(_) => {}
            }
        }
    }

    /// Fills `out` with the next `out.len() / 2` stereo frames (interleaved L/R), rendering whole ticks as needed. Once
    /// playback is over the rest is silence.
    pub fn process(&mut self, out: &mut [f32]) {
        let mut at = 0;
        while at < out.len() {
            if self.out_pos >= self.out.len() {
                if self.finished() {
                    out[at..].fill(0.0);
                    return;
                }
                self.render_tick();
            }
            let n = (self.out.len() - self.out_pos).min(out.len() - at);
            out[at..at + n].copy_from_slice(&self.out[self.out_pos..self.out_pos + n]);
            self.out_pos += n;
            at += n;
        }
    }

    /// Renders the next tick into `self.out`, in the order of spec audio/timing.md ("What a tick does"): the clock
    /// moves; control points, then automation clips; notes that end are released, then notes that start are
    /// triggered; then the tick's samples are produced.
    fn render_tick(&mut self) {
        // A looping engine starts again at tick 0 when the pattern is over; voices and effects carry on.
        if self.looping && self.end_tick > 0 && self.tick >= self.end_tick {
            self.wrap();
        }
        let tick = self.tick;
        let (start, len) = self.clock.advance();
        let song = self.transport_at(start);
        self.run_controls();
        self.run_automation(len);
        self.run_param_automation(len);
        for b in &mut self.bus {
            b.clear();
            b.resize(len * 2, 0.0);
        }
        // One tick before the song's end every note still sounding is released (spec: audio/timing.md).
        let song_end = !self.looping && !self.live_only && self.end_tick > 0 && tick + 1 == self.end_tick;
        self.release_notes(tick, song_end);
        self.start_notes(tick);
        let channels = &self.channels;
        let bus = &mut self.bus;
        self.voices.retain_mut(|v| render_voice(v, channels, bus, 0, 0, len));
        self.render_instruments(len, &song);
        for ch in &mut self.channels {
            ch.advance(len as u32);
        }
        self.tick = tick + 1;
        self.frame = start + len as u64;
        self.mix(len, &song);
        self.out.clear();
        self.out.extend_from_slice(&self.bus[0][..len * 2]);
        self.out_pos = 0;
        self.out_tick = tick;
        self.out_frame = start;
        // The tail rule: the first quiet tick at or after the song's end is the last one.
        if !self.looping && !self.live_only && tick >= self.end_tick {
            let quiet = !self.inst_active && self.out.iter().all(|s| s.abs() < QUIET);
            let limit = (TAIL_LIMIT_SECONDS * f64::from(self.rate) / self.clock.samples_per_tick()) as u64;
            if quiet || tick >= self.end_tick + limit {
                self.tail_done = true;
            }
        }
    }

    /// Releases what ends on `tick`: notes whose length is over and audio clips whose end has come; with `song_end`
    /// (one tick before the song's end) every note still sounding. Audio clips are not released early: they end at
    /// their own end, which is never after the song's (spec: audio/timing.md, audio/audio-clips.md "The end of the song").
    fn release_notes(&mut self, tick: u64, song_end: bool) {
        // Audio clips end at their cut.
        self.voices.retain(|v| v.cut.is_none_or(|c| c > tick));
        for v in &mut self.voices {
            if v.live.is_some() || v.off.is_some() {
                continue;
            }
            let due = (song_end && v.cut.is_none()) || v.release_at.is_some_and(|r| r <= tick);
            // A note's end only matters with the volume envelope on: otherwise the sampler plays on (measured).
            if due && self.channels[v.channel].vol_env.is_some() {
                v.off = Some(v.age);
            }
        }
        // Instrument notes: their note offs and chokes, all of them at the song's end.
        let mut i = 0;
        while i < self.inst_offs.len() {
            let (end, ch, key, cut) = self.inst_offs[i];
            if end <= tick || (song_end && !cut) {
                self.inst_events.push((ch, 0, key, if cut { CHOKE } else { OFF }, 0.0));
                self.inst_offs.swap_remove(i);
            } else {
                i += 1;
            }
        }
        self.sounding.retain(|&(_, _, end)| end == u64::MAX || (end > tick && !song_end));
    }

    /// Triggers the notes that start on `tick`.
    fn start_notes(&mut self, tick: u64) {
        // A plain step lights its key for one step (instrument channels: it ends after one step, an OpenStudio choice
        // for hosted plugins, which have no song-length release).
        let step = u64::from(self.ppq / 4).max(1);
        while let Some(t) = self.triggers.get(self.next_trigger).copied() {
            if t.tick > tick {
                break;
            }
            self.next_trigger += 1;
            if t.tick < tick {
                continue;
            }
            self.sounding.push((t.channel, t.key, t.end.unwrap_or(t.tick + step)));
            if self.channels[t.channel].plugin {
                if self.channels[t.channel].instrument.0.is_some() {
                    let k = t.key.min(127) as u8;
                    self.inst_events.push((t.channel, 0, k, f32::from(t.velocity) / 128.0, t.start));
                    // An audio clip ends at its cut (spec: plugins/open-sampler.md Â§7); notes end with a note off.
                    match t.cut {
                        Some(c) => self.inst_offs.push((c, t.channel, k, true)),
                        None => self.inst_offs.push((t.end.unwrap_or(t.tick + step), t.channel, k, false)),
                    }
                }
                continue;
            }
            let gain = self.channels[t.channel].start_gain(t.vel, t.pan);
            let serial = self.channels[t.channel].serial;
            self.voices.push(Voice {
                channel: t.channel,
                pos: t.from,
                step: t.rate,
                cut: t.cut,
                release_at: t.end,
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
                off: None,
            });
        }
    }

    /// Sends this tick's note events to each instrument and mixes its output into the channel's insert with the
    /// channel's volume and pan. An instrument that is busy (locked elsewhere) is skipped for the tick.
    fn render_instruments(&mut self, n: usize, song: &TransportInfo) {
        for (ch, key, vel) in self.inst_live.drain(..) {
            self.inst_events.push((ch, 0, key, if vel < 0.0 { OFF } else { vel }, 0.0));
        }
        self.inst_events.sort_by_key(|e| e.1);
        let rules: Vec<notes::Rules> = self.channels.iter().map(|c| c.rules).collect();
        self.note_voices.resize(self.channels.len(), notes::Voices::default());
        notes::apply(&self.inst_events, &rules, &mut self.note_voices, n as u32, &mut self.inst_notes);
        let held = !self.inst_offs.is_empty()
            || self.sounding.iter().any(|&(c, _, end)| end == u64::MAX && self.channels.get(c).is_some_and(|ch| ch.instrument.0.is_some()));
        if held || !self.inst_events.is_empty() {
            self.inst_tail = u64::from(self.rate) * INSTRUMENT_TAIL_SECONDS;
        } else {
            self.inst_tail = self.inst_tail.saturating_sub(n as u64);
        }
        if self.scratch.0.len() < n {
            self.scratch.0.resize(n, 0.0);
            self.scratch.1.resize(n, 0.0);
        }
        let mut active = false;
        for (c, ch) in self.channels.iter().enumerate() {
            let Some(inst) = &ch.instrument.0 else { continue };
            let Ok(mut inst) = inst.try_lock() else {
                self.params.resend_channel(c);
                continue;
            };
            inst.transport(song);
            // Parameter values before the notes at the same frame (spec: plugins/plugin-parameters.md Â§2).
            for (off, index, value) in self.params.channel_span(c, 0, n) {
                inst.param(off, index, value);
            }
            let first = self.inst_notes.partition_point(|e| e.0 < c);
            for &(_, off, note) in self.inst_notes[first..].iter().take_while(|e| e.0 == c) {
                let off = off.min(n.saturating_sub(1) as u32);
                match note {
                    notes::Note::Choke(key) => inst.choke(off, key),
                    notes::Note::Off(key) => inst.note_off(off, key),
                    notes::Note::On { key, vel, start } if start > 0.0 => inst.note_on_from(off, key, vel, start),
                    notes::Note::On { key, vel, .. } => inst.note_on(off, key, vel),
                    notes::Note::Tune(key, semitones) => inst.tune(off, key, semitones),
                }
            }
            let (l, r) = (&mut self.scratch.0[..n], &mut self.scratch.1[..n]);
            inst.process(self.rate, l, r);
            // A sample that is still playing must not be cut off by the idle tail.
            active |= inst.active();
            let (gl, gr) = ch.gain(1.0, 0.0);
            let out = &mut self.bus[ch.insert];
            for f in 0..n {
                out[f * 2] += l[f] * gl;
                out[f * 2 + 1] += r[f] * gr;
            }
        }
        self.inst_events.clear();
        self.inst_active = active;
        if active {
            self.inst_tail = u64::from(self.rate) * INSTRUMENT_TAIL_SECONDS;
        }
    }

    /// The mixer over this tick: each track's effects, then its gain / pan (ramped over the tick after an edit), then
    /// its output added into its destinations; the master's output is the song.
    fn mix(&mut self, frames: usize, song: &TransportInfo) {
        let limit = os_dsp::glide::engine_limit(self.rate);
        for &t in &self.order {
            self.params.deliver_slots(t, &self.tracks[t].effects);
            let mut buf = std::mem::take(&mut self.bus[t]);
            let rate = self.rate;
            let (dry, l, r) = (&mut self.fx_dry, &mut self.scratch.0, &mut self.scratch.1);
            let track = &mut self.tracks[t];
            for fx in &mut track.effects {
                run_slot(fx, &mut buf, rate, song, dry, l, r);
            }
            track.eq.process(&mut buf);
            // The four gains glide to their targets: a per-sample step for this tick, at most `limit`, applied as a
            // running sum (spec: audio/mixer.md "Gains ramp").
            let to = track.matrix;
            let mut ramps = [[track.gains[0][0].block(to[0][0], frames, limit), track.gains[0][1].block(to[0][1], frames, limit)], [
                track.gains[1][0].block(to[1][0], frames, limit),
                track.gains[1][1].block(to[1][1], frames, limit),
            ]];
            if ramps.iter().flatten().all(|r| r.is_flat()) {
                let [[ll, lr], [rl, rr]] = ramps.map(|r| r.map(|g| g.first()));
                for f in buf.chunks_exact_mut(2) {
                    let (l, r) = (f[0], f[1]);
                    f[0] = ll * l + lr * r;
                    f[1] = rr * r + rl * l;
                }
            } else {
                for f in buf.chunks_exact_mut(2) {
                    let [[ll, lr], [rl, rr]] = [[ramps[0][0].next(), ramps[0][1].next()], [ramps[1][0].next(), ramps[1][1].next()]];
                    let (l, r) = (f[0], f[1]);
                    f[0] = ll * l + lr * r;
                    f[1] = rr * r + rl * l;
                }
            }
            let pk = &mut self.tracks[t].peak;
            for f in buf.chunks_exact(2) {
                pk[0] = pk[0].max(f[0].abs());
                pk[1] = pk[1].max(f[1].abs());
            }
            for &(dest, g) in &self.tracks[t].routes {
                for (d, s) in self.bus[dest].iter_mut().zip(&buf) {
                    *d += *s * g;
                }
            }
            self.bus[t] = buf;
        }
    }

    /// The transport hosted plugins see at `frame` (spec: plugins/vst3-hosting.md Â§1): playing at the project tempo,
    /// in 4/4 (the project's time signature is not read yet), with the pattern loop while looping.
    fn transport_at(&self, frame: u64) -> TransportInfo {
        let mut start = TransportInfo {
            playing: true,
            tempo: self.tempo,
            frame: 0,
            beats: 0.0,
            bar_start: 0.0,
            bar: 0,
            numerator: 4,
            denominator: 4,
            looping: None,
        };
        if self.looping && self.end_tick > 0 {
            start.looping = Some((0.0, self.end_tick as f64 / f64::from(self.ppq)));
        }
        start.advanced(frame, self.rate)
    }

    /// Loop point: back to tick 0 (sounding notes keep their remaining length).
    fn wrap(&mut self) {
        let at = self.end_tick;
        for o in &mut self.inst_offs {
            o.0 = o.0.saturating_sub(at);
        }
        for v in &mut self.voices {
            v.cut = v.cut.map(|c| c.saturating_sub(at));
            v.release_at = v.release_at.map(|r| r.saturating_sub(at));
        }
        self.clock.restart();
        self.tick = 0;
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

    /// Renders the whole song: every tick up to the first quiet one at or after the song's end (spec: audio/timing.md,
    /// tail "leave remainder"). The length is a whole number of ticks. `_block` is kept for callers of the
    /// block-based engine.
    pub fn render_all(&mut self, _block: usize) -> Vec<f32> {
        let mut out = Vec::new();
        out.extend_from_slice(&self.out[self.out_pos..]);
        self.out_pos = self.out.len();
        while !self.finished() {
            self.render_tick();
            out.extend_from_slice(&self.out);
            self.out_pos = self.out.len();
        }
        out
    }
}

/// Notes still held on instruments end when the engine goes away (stop, or a new song).
impl Drop for Engine {
    fn drop(&mut self) {
        for &(_, c, key, _) in &self.inst_offs {
            if let Some(Ok(mut inst)) = self.channels.get(c).and_then(|ch| ch.instrument.0.as_ref()).map(|i| i.lock()) {
                inst.note_off(0, key);
            }
        }
        for &(c, key, _) in self.sounding.iter().filter(|s| s.2 == u64::MAX) {
            if let Some(Ok(mut inst)) = self.channels.get(c).and_then(|ch| ch.instrument.0.as_ref()).map(|i| i.lock()) {
                inst.note_off(0, key.min(127) as u8);
            }
        }
        // Mono legato notes sound under their first key.
        for (c, v) in self.note_voices.iter().enumerate() {
            for key in v.taken_over() {
                if let Some(Ok(mut inst)) = self.channels.get(c).and_then(|ch| ch.instrument.0.as_ref()).map(|i| i.lock()) {
                    inst.note_off(0, key);
                }
            }
        }
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
    let base_step = ch.base_rate * v.step;
    let mut step = base_step;
    let out = &mut bus[ch.insert];
    // Rate 1 copies samples only while the position is whole (a voice that was pitched keeps its fraction).
    let exact = step == 1.0 && v.pos.fract() == 0.0 && ch.pitch_env.is_none();
    let mut scale = step.max(1.0);
    let off_s = v.off.map(|o| o as f64 * ch.frame_s);
    let mut pos = v.pos;
    for f in offset..end {
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

/// One mixer slot over interleaved `buf`: skipped when its LED is off, blended with the dry signal by its mix level.
fn run_slot(fx: &mut FxSlot, buf: &mut [f32], rate: u32, song: &TransportInfo, dry: &mut Vec<f32>, l: &mut Vec<f32>, r: &mut Vec<f32>) {
    if !fx.on || fx.mix <= 0.0 {
        return;
    }
    let blend = fx.mix < 1.0;
    if blend {
        dry.clear();
        dry.extend_from_slice(buf);
    }
    match &mut fx.fx {
        Fx::Own(e) => e.process(buf),
        Fx::Hosted(h) => {
            // A busy instance (its window is being set up) passes the sound through for this block.
            let Ok(mut h) = h.try_lock() else { return };
            let n = buf.len() / 2;
            l.clear();
            r.clear();
            l.extend(buf.chunks_exact(2).map(|f| f[0]));
            r.extend(buf.chunks_exact(2).map(|f| f[1]));
            h.transport_fx(song);
            h.process_fx(rate, &mut l[..n], &mut r[..n]);
            for (i, f) in buf.chunks_exact_mut(2).enumerate() {
                f[0] = l[i];
                f[1] = r[i];
            }
        }
    }
    if blend {
        // out = wet * m + dry * (1 - m), single arithmetic (spec: audio/mixer.md "An effect slot").
        let (m, d) = (fx.mix, 1.0 - fx.mix);
        for (o, x) in buf.iter_mut().zip(dry.iter()) {
            *o = *o * m + *x * d;
        }
    }
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

/// A scheduled note on the song's tick line.
#[derive(Debug, Clone, Copy)]
struct Sched {
    tick: u64,
    channel: usize,
    key: u16,
    velocity: u8,
    /// Note pan byte, 64 = center.
    pan: u8,
    /// Tick at which the voice is cut (audio clip end).
    cut: Option<u64>,
    /// Note length in ticks (0 = a step: it has no end of its own).
    len: u64,
    /// Seconds into the sample where the voice starts (audio clip window start).
    from_s: f64,
}

impl Sched {
    fn note(tick: u64, n: &os_model::Note) -> Self {
        Sched { tick, channel: usize::from(n.channel), key: n.key, velocity: n.velocity, pan: n.pan, cut: None, len: u64::from(n.length), from_s: 0.0 }
    }
}

/// Swing delay of a note in whole ticks (spec: audio/swing.md). With a sixteenth `step = PPQ / 4` and `half = step / 2`,
/// only notes in the first half of an odd-numbered step (counted from 0 at the pattern start) move, by
/// `Trunc(swing * share / (128 * 128) * (half - offset))` ticks: a note on the step moves half a step at full swing,
/// and at full swing every note of that half lands on the step's middle. `share` is the channel's share (0..=128).
pub fn swing_delay(position: u32, ppq: u16, swing: u8, share: u16) -> u32 {
    let step = u32::from(ppq) / 4;
    if swing == 0 || step == 0 {
        return 0;
    }
    let half = f64::from(step) / 2.0;
    let (index, offset) = (position / step, position % step);
    if index % 2 == 0 || f64::from(offset) >= half {
        return 0;
    }
    ((f64::from(swing) * f64::from(share)) / (128.0 * 128.0) * (half - f64::from(offset))).trunc() as u32
}

/// The channel's share of the swing as stored (0..=128) from the Misc tab value.
fn swing_share(project: &Project, channel: u16) -> u16 {
    project.channels.get(usize::from(channel)).map_or(128, |c| (c.misc.swing.clamp(0.0, 1.0) * 128.0).round() as u16)
}

/// Schedules a note whose pattern starts at tick `start` (may be negative: a clip that begins inside its pattern).
/// With `swing` the note's start moves by its position inside the pattern; `cap` is the tick at which a note with a
/// length is released at the latest (a clip's end with "play truncated notes"; steps keep their own release). A
/// repeating note is retriggered at every multiple of its interval inside its length (spec:
/// audio/pattern-controls.md, note repeat); a plain step counts as one step long for that.
fn push_note(out: &mut Vec<Sched>, start: i64, n: &os_model::Note, project: &Project, swing: bool, cap: Option<u64>) {
    let ppq = project.ppq.max(1);
    let delay = if swing { swing_delay(n.position, ppq, project.swing.unwrap_or(0), swing_share(project, n.channel)) } else { 0 };
    let first = start + i64::from(n.position) + i64::from(delay);
    if first < 0 {
        return;
    }
    let first = Sched::note(first as u64, n);
    let span = if n.length > 0 { u64::from(n.length) } else { u64::from(ppq / 4).max(1) };
    let capped = |s: Sched| match cap {
        Some(c) if s.len > 0 => Sched { len: s.len.min(c.saturating_sub(s.tick)), ..s },
        _ => s,
    };
    match n.repeat_ticks(ppq) {
        Some(interval) => {
            let mut off = 0;
            while off < span {
                let len = if n.length > 0 { first.len - off } else { 0 };
                let s = capped(Sched { tick: first.tick + off, len, ..first });
                if n.length == 0 || s.len > 0 {
                    out.push(s);
                }
                off += u64::from(interval.max(1));
            }
        }
        None => {
            let s = capped(first);
            if n.length == 0 || s.len > 0 {
                out.push(s);
            }
        }
    }
}

/// Where a pattern clip plays (spec: audio/song-structure.md, "Song mode"): its pattern time `[from, from + len)`, the
/// tick its pattern time starts at, its own end on the song, and the tick from which it starts no more notes (the
/// next clip of the same pattern taking the pattern over, or its end).
#[derive(Debug, Clone, Copy)]
struct PatternPlay {
    pattern: u16,
    from: u64,
    len: u64,
    origin: i64,
    end: u64,
    starts_until: u64,
}

/// The pattern clips that play, with their windows. A clip whose start offset is negative (uncut) plays the whole
/// pattern, whatever length its record stores. A pattern has one play position: a later clip of the same pattern
/// takes it over from its start, and of two clips that start on the same tick only the later one in the file plays.
/// Muted clips and clips on a switched-off track play nothing and take nothing over.
fn pattern_plays(project: &Project, clips: &[os_model::Clip]) -> Vec<PatternPlay> {
    let mut list: Vec<(usize, &os_model::Clip, u16)> = clips
        .iter()
        .enumerate()
        .filter(|(_, c)| clip_audible(project, c))
        .filter_map(|(i, c)| match c.source {
            ClipSource::Pattern(n) => Some((i, c, n)),
            _ => None,
        })
        .collect();
    list.sort_by_key(|&(i, c, n)| (n, c.position, i));
    let mut out = Vec::new();
    for (k, &(_, c, n)) in list.iter().enumerate() {
        let later = &list[k + 1..];
        if later.first().is_some_and(|&(_, d, m)| m == n && d.position == c.position) {
            continue;
        }
        let (from, len) = match c.window {
            Some(w) => (w.0.max(0.0).round() as u64, u64::from(c.length)),
            None => (0, pattern_length(project, n)),
        };
        let start = u64::from(c.position);
        let end = start + len;
        let takeover = later.iter().take_while(|&&(_, _, m)| m == n).map(|&(_, d, _)| u64::from(d.position)).find(|&p| p > start);
        out.push(PatternPlay { pattern: n, from, len, origin: i64::from(c.position) - from as i64, end, starts_until: end.min(takeover.unwrap_or(u64::MAX)) });
    }
    out
}

/// Channel control points as (tick, channel, parameter, value): a pattern's own, or those of every
/// pattern clip of the playlist (spec: audio/pattern-controls.md, audio/song-structure.md "Control points").
fn schedule_controls(project: &Project, mode: PlayMode) -> Vec<(u64, usize, u16, i32)> {
    let points = |n: u16| project.patterns.iter().find(|p| p.number == n).map(|p| p.controls.as_slice()).unwrap_or(&[]);
    let place = |out: &mut Vec<_>, n: u16| {
        for c in points(n) {
            out.push((u64::from(c.position), usize::from(c.channel()), c.param(), c.value));
        }
    };
    let mut out = Vec::new();
    match mode {
        PlayMode::Pattern(n) => place(&mut out, n),
        PlayMode::Song => {
            let clips = project.arrangement().map(|a| a.clips.as_slice()).unwrap_or(&[]);
            if clips.is_empty() {
                place(&mut out, project.current_pattern.unwrap_or(1));
            }
            // Points inside a clip's window act, placed with the clip like notes.
            for play in pattern_plays(project, clips) {
                for p in points(play.pattern).iter().filter(|p| u64::from(p.position) >= play.from && u64::from(p.position) < play.from + play.len) {
                    let tick = play.origin + i64::from(p.position);
                    if tick >= 0 && (tick as u64) < play.starts_until {
                        out.push((tick as u64, usize::from(p.channel()), p.param(), p.value));
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

/// Ticks in a bar: `PPQ * 4 * numerator / denominator` (spec: audio/timing.md, "Settings").
fn bar_ticks(project: &Project) -> u64 {
    let (num, den) = project.time_signature.unwrap_or((4, 4));
    (u64::from(project.ppq.max(1)) * 4 * u64::from(num.max(1)) / u64::from(den.max(1))).max(1)
}

/// A pattern's length in ticks: an explicit length (the channel rack's length box, spec: ui/channel-rack.md Â§7) wins;
/// otherwise the end of its last note (a length of 0 counting as 1) rounded up to whole bars, at least one bar. A
/// pattern whose notes end exactly on a bar line is that long (spec: audio/timing.md, "Where the song ends").
fn pattern_length(project: &Project, number: u16) -> u64 {
    let Some(p) = project.patterns.iter().find(|p| p.number == number) else { return bar_ticks(project) };
    if let Some(l) = p.length.filter(|&l| l > 0) {
        return u64::from(l);
    }
    let bar = bar_ticks(project);
    let last = p.notes.iter().map(|n| u64::from(n.position) + u64::from(n.length.max(1))).max().unwrap_or(0);
    last.div_ceil(bar).max(1) * bar
}

/// Note triggers plus the song's end tick: in pattern mode the pattern's length, in song mode the largest end of a
/// clip that is not muted (clips on a switched-off playlist track count; spec: audio/timing.md, audio/song-structure.md).
fn schedule(project: &Project, mode: PlayMode, report: &mut EngineReport) -> (Vec<Sched>, u64) {
    let pattern_notes = |n: u16| project.patterns.iter().find(|p| p.number == n).map(|p| p.notes.as_slice()).unwrap_or(&[]);
    let one_pattern = |n: u16| {
        // Muted notes are silent (spec: ui/piano-roll.md Â§9); notes past an explicit length are not played.
        let end = pattern_length(project, n);
        let mut t = Vec::new();
        for x in pattern_notes(n).iter().filter(|x| !x.muted() && u64::from(x.position) < end) {
            push_note(&mut t, 0, x, project, true, None);
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
            let mut end: u64 = 0;
            let truncated = project.play_truncated.unwrap_or(false);
            for play in pattern_plays(project, clips) {
                for note in pattern_notes(play.pattern).iter().filter(|x| !x.muted()) {
                    let p = u64::from(note.position);
                    let cap = truncated.then_some(play.end);
                    if p >= play.from && p < play.from + play.len {
                        // Starts inside the window: from its place in the clip, unless the pattern was taken over.
                        if play.origin + (p as i64) < play.starts_until as i64 {
                            push_note(&mut out, play.origin, note, project, true, cap);
                        }
                    } else if truncated && p < play.from && note.length > 0 && p + u64::from(note.length) > play.from {
                        // Began before the window and reaches into it: starts at the clip's position, for the part
                        // inside the window.
                        let inside = (p + u64::from(note.length)).min(play.from + play.len) - play.from;
                        let cut = os_model::Note { position: play.from as u32, length: inside as u32, ..*note };
                        if play.origin + (play.from as i64) < play.starts_until as i64 {
                            push_note(&mut out, play.origin, &cut, project, false, cap);
                        }
                    }
                }
            }
            for c in clips {
                let start = u64::from(c.position);
                let len = u64::from(c.length);
                if !c.muted {
                    end = end.max(start + len);
                }
                // Muted clips and clips on muted tracks are silent (spec: ui/playlist.md Â§2, Â§6).
                if !clip_audible(project, c) {
                    continue;
                }
                match c.source {
                    // Pattern clips were placed above.
                    ClipSource::Pattern(_) => {}
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
            plugin_state: None,
            color: None,
            enabled: true,
            volume: 0.78125,
            pan: 0.0,
            pitch_cents: 0,
            mixer_track: 0,
            sample_path: None, // plays the built-in tone
            layer_children: vec![],
            envelopes: Default::default(), rack: Default::default(), misc: Default::default(), sound: Default::default(),
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

    /// Records the transport it is given before each block (instrument and effect).
    #[derive(Default)]
    struct Clock {
        seen: Vec<(TransportInfo, usize)>,
        next: Option<TransportInfo>,
    }

    impl crate::Instrument for Clock {
        fn note_on(&mut self, _offset: u32, _key: u8, _velocity: f32) {}
        fn note_off(&mut self, _offset: u32, _key: u8) {}
        fn transport(&mut self, info: &TransportInfo) {
            self.next = Some(*info);
        }
        fn process(&mut self, _rate: u32, l: &mut [f32], _r: &mut [f32]) {
            let t = self.next.take().expect("a transport before each block");
            self.seen.push((t, l.len()));
        }
    }

    impl crate::instrument::HostedEffect for Clock {
        fn transport_fx(&mut self, info: &TransportInfo) {
            self.next = Some(*info);
        }
        fn process_fx(&mut self, _rate: u32, l: &mut [f32], _r: &mut [f32]) {
            let t = self.next.take().expect("a transport before each block");
            self.seen.push((t, l.len()));
        }
    }

    /// spec: plugins/vst3-hosting.md §1 (transport): hosted plugins see the playing position, tempo and loop.
    #[test]
    fn plugins_see_the_transport() {
        let mut p = project(vec![long_note(0, 96, 0)], vec![]);
        p.channels[0].kind = ChannelKind::Plugin;
        p.channels[0].mixer_track = 1;
        p.mixer = (0..3).map(os_model::MixerTrack::new).collect();
        let mut lib = SampleLibrary::default();
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Pattern(1));
        let inst = Arc::new(std::sync::Mutex::new(Clock::default()));
        let fx = Arc::new(std::sync::Mutex::new(Clock::default()));
        e.set_instrument(0, inst.clone());
        e.set_effect(1, 0, fx.clone());
        // Pattern playback loops (the app turns it on for pattern mode).
        e.set_looping(true);
        let mut out = vec![0.0f32; 512 * 2];
        let blocks = 200;
        for _ in 0..blocks {
            e.process(&mut out);
        }
        // 130 BPM at 48 kHz; the pattern is one bar (4 beats) and loops.
        let fpb = 48000.0 * 60.0 / 130.0;
        let bar = 4.0 * fpb;
        let end = e.length();
        assert!((end as f64 - bar).abs() < 2.0, "one bar: {end} frames");
        for (who, seen) in [("instrument", &inst.lock().unwrap().seen), ("effect", &fx.lock().unwrap().seen)] {
            let mut frame = 0u64;
            for (t, n) in seen.iter() {
                if frame >= end {
                    frame -= end;
                }
                assert!(t.playing && t.tempo == 130.0 && (t.numerator, t.denominator) == (4, 4), "{who} {t:?}");
                assert_eq!(t.frame, frame, "{who}");
                assert!((t.beats - frame as f64 / fpb).abs() < 1e-9, "{who} {t:?}");
                assert_eq!((t.bar, t.bar_start), (0, 0.0), "{who}: a one-bar loop stays in bar 0");
                // The loop is the pattern's length in beats: 384 ticks at 96 PPQ.
                let (a, b) = t.looping.expect("pattern playback loops");
                assert!(a == 0.0 && b == 4.0, "{who} {t:?}");
                frame += *n as u64;
            }
            // Every frame handed out was rendered, in whole ticks (at most one tick ahead).
            let total: usize = seen.iter().map(|s| s.1).sum();
            assert!(total >= 512 * blocks && total < 512 * blocks + 231, "{who}: every frame is covered ({total})");
            // 48000 * 60 / (130 * 96) = 230.77 samples per tick.
            assert!(seen.iter().all(|s| s.1 == 230 || s.1 == 231), "{who}: one call per tick");
        }
        let t = TransportInfo { looping: None, ..fx.lock().unwrap().seen[0].0 };
        let later = t.advanced(48000 * 3, 48000);
        assert_eq!((later.frame, later.bar), (144000, 1));
        assert!((later.beats - 6.5).abs() < 1e-9 && (later.bar_start - 4.0).abs() < 1e-9, "{later:?}");
    }

    // spec: audio/swing.md (whole ticks; notes in the first half of an odd step move towards its middle)
    #[test]
    fn swing_delays_off_beat_sixteenths() {
        assert_eq!(swing_delay(0, 96, 128, 128), 0);
        assert_eq!(swing_delay(24, 96, 128, 128), 12);
        assert_eq!(swing_delay(24, 96, 64, 128), 6);
        assert_eq!(swing_delay(48, 96, 128, 128), 0);
        assert_eq!(swing_delay(72, 96, 32, 128), 3);
        assert_eq!(swing_delay(24, 96, 0, 128), 0);
        // Swing 50 on the step: 50 / 128 of half a step = 4.69 ticks, truncated.
        assert_eq!(swing_delay(24, 96, 50, 128), 4);
        // The channel's share scales it.
        assert_eq!(swing_delay(24, 96, 128, 64), 6);
        // Off the grid: 6 ticks into the step at full swing lands on the step's middle; the second half stays.
        assert_eq!(swing_delay(30, 96, 128, 128), 6);
        assert_eq!(swing_delay(36, 96, 128, 128), 0);
        assert_eq!(swing_delay(40, 96, 128, 128), 0);
    }

    /// Main swing reaches played notes: an off-beat step starts late on an instrument channel.
    #[test]
    fn swing_delays_played_notes() {
        let mut p = project(vec![Note::step(0, 24), Note::step(0, 48)], vec![]);
        p.swing = Some(128);
        p.channels[0].kind = ChannelKind::Plugin;
        let mut lib = SampleLibrary::default();
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Pattern(1));
        let probe = Arc::new(std::sync::Mutex::new(Probe { log: vec![], held: vec![], at: 0 }));
        e.set_instrument(0, probe.clone());
        let mut out = vec![0.0f32; 512 * 2];
        for _ in 0..100 {
            e.process(&mut out);
        }
        let ons: Vec<u64> = probe.lock().unwrap().log.iter().filter(|l| l.1).map(|l| l.0).collect();
        assert_eq!(ons[0], laws::tick_to_frame(36.0, 96, 130.0, 48000), "{ons:?}");
        assert_eq!(ons[1], laws::tick_to_frame(48.0, 96, 130.0, 48000), "{ons:?}");
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
        let mut ticks = |p: &Project| -> Vec<(u64, u64)> {
            let (t, _) = schedule(p, PlayMode::Pattern(1), &mut report);
            t.iter().map(|s| (s.tick, s.len)).collect()
        };
        // 1/8 (48 ticks) over 384 ticks: eight triggers, the last one 48 ticks long.
        let t = ticks(&project(vec![long_note(0, 384, 4)], vec![]));
        assert_eq!(t.len(), 8);
        assert_eq!(t[1], (48, 336));
        assert_eq!(t[7], (336, 48));
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
        assert_eq!(c, vec![(24, 0, ControlPoint::VOLUME, 6400), (96, 3, ControlPoint::PAN, 0)]);
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
        Engine::new(p, &mut lib, 48000, PlayMode::Song).triggers.iter().map(|t| t.tick).collect()
    }

    // spec: ui/playlist.md §2, §6 (muted clips / tracks are silent), flp/events/playlist.md "Clip offsets"
    #[test]
    fn song_respects_mutes_and_clip_windows() {
        use os_model::{Clip, ClipSource};
        let notes = vec![Note::step(0, 0), Note::step(0, 96), Note::step(0, 192)];
        // A clip at bar 2 with the window [96, 288): the notes at 96 and 192 play at 384 and 480.
        let mut c = Clip::new(384, 192, 0, ClipSource::Pattern(1));
        c.window = Some((96.0, 288.0));
        assert_eq!(song_triggers(&song(notes.clone(), vec![c])), vec![384, 480]);
        let mut m = c;
        m.muted = true;
        assert!(song_triggers(&song(notes.clone(), vec![m])).is_empty(), "muted clip");
        let mut p = song(notes.clone(), vec![c]);
        p.arrangements[0].tracks[0].enabled = false;
        assert!(song_triggers(&p).is_empty(), "muted track");
        // An uncut clip plays the whole pattern, whatever length its record stores (spec: audio/song-structure.md).
        let plain = Clip::new(0, 100, 1, ClipSource::Pattern(1));
        assert_eq!(song_triggers(&song(notes, vec![plain])), vec![0, 96, 192]);
    }

    fn song_notes(p: &Project) -> Vec<(u64, Option<u64>)> {
        let mut lib = SampleLibrary::new(None);
        Engine::new(p, &mut lib, 48000, PlayMode::Song).triggers.iter().map(|t| (t.tick, t.end)).collect()
    }

    /// spec: audio/song-structure.md "One play position per pattern": a later clip of the same pattern takes it over
    /// (the earlier one starts no more notes, started notes keep their length); of two clips on one tick only the
    /// later one in the file plays; a clip of another pattern takes nothing over.
    #[test]
    fn a_pattern_has_one_play_position() {
        use os_model::{Clip, ClipSource};
        let notes = vec![long_note(0, 300, 0), long_note(96, 48, 0), long_note(192, 48, 0), long_note(288, 48, 0)];
        // The second clip starts at 150, inside the first: the first plays 0 (keeping its 300 ticks) and 96.
        let first = Clip::new(0, 384, 0, ClipSource::Pattern(1));
        let second = Clip::new(150, 384, 1, ClipSource::Pattern(1));
        assert_eq!(song_notes(&song(notes.clone(), vec![first, second])), vec![(0, Some(300)), (96, Some(144)), (150, Some(450)), (246, Some(294)), (342, Some(390)), (438, Some(486))]);
        // Two clips on one tick sound once.
        assert_eq!(song_triggers(&song(notes.clone(), vec![first, first])).len(), 4);
        // Another pattern's clip does not interrupt.
        let mut p = song(notes, vec![first, Clip::new(150, 384, 1, ClipSource::Pattern(2))]);
        p.patterns.push(Pattern { number: 2, notes: vec![Note::step(0, 0)], ..Pattern::default() });
        assert_eq!(song_triggers(&p), vec![0, 96, 150, 192, 288]);
    }

    /// spec: audio/song-structure.md "Play truncated notes": on, a note that began before the window starts at the
    /// clip for its part inside the window, and notes end at the clip's end at the latest; off, neither.
    #[test]
    fn truncated_notes_follow_the_option() {
        use os_model::{Clip, ClipSource};
        let notes = vec![long_note(0, 200, 0), long_note(150, 200, 0)];
        let mut c = Clip::new(384, 100, 0, ClipSource::Pattern(1));
        c.window = Some((100.0, 200.0));
        let mut p = song(notes, vec![c]);
        assert_eq!(song_notes(&p), vec![(434, Some(634))], "off: only the note starting inside, at full length");
        p.play_truncated = Some(true);
        assert_eq!(song_notes(&p), vec![(384, Some(484)), (434, Some(484))], "on: the earlier note from the clip start, both cut at its end");
    }

    // spec: plugins/open-sampler.md §7 (playlist clips through a hosted sampler: sample start and choke)
    #[test]
    fn audio_clips_on_instruments_start_late_and_are_choked() {
        use os_model::{Clip, ClipSource};
        #[derive(Default)]
        struct Log(Vec<(u64, &'static str, f32)>, u64);
        impl crate::Instrument for Log {
            fn note_on(&mut self, o: u32, _k: u8, _v: f32) {
                self.0.push((self.1 + u64::from(o), "on", 0.0));
            }
            fn note_off(&mut self, o: u32, _k: u8) {
                self.0.push((self.1 + u64::from(o), "off", 0.0));
            }
            fn note_on_from(&mut self, o: u32, _k: u8, _v: f32, start: f32) {
                self.0.push((self.1 + u64::from(o), "from", start));
            }
            fn choke(&mut self, o: u32, _k: u8) {
                self.0.push((self.1 + u64::from(o), "choke", 0.0));
            }
            fn process(&mut self, _rate: u32, l: &mut [f32], _r: &mut [f32]) {
                self.1 += l.len() as u64;
            }
        }
        // A one-second sample; the first clip starts 250 ms into it, the second plays it from the top.
        let wav = std::env::temp_dir().join("os-engine-clip-start.wav");
        crate::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &vec![0.25; 96000]).unwrap();
        let mut trimmed = Clip::new(0, 96, 0, ClipSource::Channel(0));
        trimmed.window = Some((250.0, 711.0));
        let mut p = song(vec![], vec![trimmed, Clip::new(384, 96, 0, ClipSource::Channel(0))]);
        p.channels[0].kind = ChannelKind::AudioClip;
        p.channels[0].sample_path = Some(wav.to_string_lossy().into_owned());
        let mut lib = SampleLibrary::new(None);
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Song);
        let log = Arc::new(std::sync::Mutex::new(Log::default()));
        e.set_instrument(0, log.clone());
        e.render_all(480);
        let f = |t: f64| laws::tick_to_frame(t, 96, 130.0, 48000);
        let got = log.lock().unwrap().0.clone();
        assert_eq!(got, vec![(0, "from", 0.25), (f(96.0), "choke", 0.0), (f(384.0), "on", 0.0), (f(480.0), "choke", 0.0)]);
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
            .map(os_model::MixerTrack::new)
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

    /// Halves what passes through (a stand-in for a hosted effect plugin).
    struct Halve;

    impl crate::instrument::HostedEffect for Halve {
        fn process_fx(&mut self, _rate: u32, left: &mut [f32], right: &mut [f32]) {
            left.iter_mut().chain(right.iter_mut()).for_each(|v| *v *= 0.5);
        }
    }

    fn insert_project() -> Project {
        let mut p = project(vec![long_note(0, 96, 0)], vec![]);
        p.channels[0].mixer_track = 1;
        p.mixer = (0..3).map(os_model::MixerTrack::new).collect();
        p
    }

    fn render_with(p: &Project, fx: bool) -> Vec<f32> {
        let mut lib = SampleLibrary::new(None);
        let mut e = Engine::new(p, &mut lib, 44100, PlayMode::Pattern(1));
        if fx {
            e.set_effect(1, 0, std::sync::Arc::new(std::sync::Mutex::new(Halve)));
        }
        e.render_all(512)
    }

    /// spec: ui/mixer.md §7: a slot's plugin processes its track; the LED bypasses it, the mix knob blends it.
    #[test]
    fn hosted_slot_effects_follow_led_and_mix() {
        let p = insert_project();
        let dry = peak(&render_with(&p, false), 0);
        assert!(dry > 0.01);
        let wet = peak(&render_with(&p, true), 0);
        assert!((wet / dry - 0.5).abs() < 1e-3, "halved: {wet} / {dry}");
        let mut off = p.clone();
        off.mixer[1].slot_on[0] = false;
        assert!((peak(&render_with(&off, true), 0) - dry).abs() < 1e-6, "LED off = bypass");
        let mut half = p.clone();
        half.mixer[1].slot_mix[0] = 0.5;
        assert!((peak(&render_with(&half, true), 0) / dry - 0.75).abs() < 1e-3, "mix 50 %");
        let mut lib = SampleLibrary::new(None);
        let mut e = Engine::new(&p, &mut lib, 44100, PlayMode::Pattern(1));
        let mut buf = vec![0.0f32; 1024];
        e.process(&mut buf);
        let mut peaks = Vec::new();
        e.take_track_peaks(&mut peaks);
        assert!(peaks[1][0] > 0.0 && peaks[0][0] > 0.0 && peaks[2][0] == 0.0, "track meters: {peaks:?}");
    }

    /// spec: ui/mixer.md §4: polarity flips the sign, swap exchanges sides, separation -100 % is mono.
    #[test]
    fn polarity_swap_and_separation() {
        let p = insert_project();
        let base = render_with(&p, false);
        let mut q = p.clone();
        q.mixer[1].polarity = true;
        let flipped = render_with(&q, false);
        assert!(base.iter().zip(&flipped).all(|(a, b)| (a + b).abs() < 1e-6), "polarity");
        // Swap acts on the track's input, before its pan (an OpenStudio reading; FL's order is unmeasured).
        let m = MixTrack { swap: true, ..MixTrack::unity() }.compute_matrix();
        assert!(m[0][0].abs() < 1e-6 && m[1][1].abs() < 1e-6 && m[0][1] > 0.5 && m[1][0] > 0.5, "swapped: {m:?}");
        // A positive separation merges: fully mono at 64 (spec: audio/mixer.md).
        let mut t = MixTrack { sep: 64, ..MixTrack::unity() };
        let m = t.compute_matrix();
        assert!((m[0][0] - m[0][1]).abs() < 1e-6 && (m[1][0] - m[1][1]).abs() < 1e-6, "merged: {m:?}");
        t.sep = 0;
        let m = t.compute_matrix();
        assert!(m[0][1].abs() < 1e-6 && m[1][0].abs() < 1e-6, "original: {m:?}");
    }
}
