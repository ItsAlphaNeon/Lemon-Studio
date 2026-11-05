// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Tick-based playback engine: schedules notes from patterns / playlist clips on the sequencer's tick clock, plays
//! sampler and audio-clip channels, and sums them through the mixer routing graph. Everything that is scheduled
//! happens at a tick's start; the engine renders whole ticks and hands them out in the block sizes the caller asks
//! for, so live playback and an offline render produce the same samples.
//!
// spec: audio/timing.md (tick clock, what a tick does, the song's end, the tail), audio/swing.md,
//       audio/song-structure.md, audio/implementing.md ("Playing live"), audio/sampler-playback.md,
//       audio/gain-laws.md, flp/events/playlist.md (clips), flp/events/patterns.md (notes),
//       flp/events/mixer.md (routing), audio/pattern-controls.md (repeat, control points),
//       audio/channel-envelopes.md (volume and pitch envelopes), audio/time-stretch.md

use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};

use os_model::target::Target;
use os_model::{ChannelKind, ClipSource, ControlPoint, Project, Stretching};

use crate::clock::Clock;
use os_dsp::glide::Glide;
use crate::effects::Effect;
use crate::envelope;
use crate::instrument::{Instrument, SharedInstrument, Slot, TransportInfo};
use crate::laws;
use crate::sample::{Sample, SampleLibrary};

pub mod auto_bypass;
mod controllers;
mod layers;
mod marks;
mod notes;
mod param_auto;
mod multicore;
pub use multicore::LoadGate;
mod pdc;
mod rec;
pub use layers::Mt19937;
pub use param_auto::PARAM_STEP;
pub use rec::{Point as RecordPoint, RecordTap};

/// What to play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayMode {
    /// The first arrangement's playlist. Falls back to the current pattern if it has no clips.
    Song,
    /// One pattern (1-based), from its start.
    Pattern(u16),
}

/// How much a nudge of the Tempo tapper speeds playback up or slows it down (spec: ui/tempo-tapper.md §6: about 2 %).
const NUDGE: f64 = 0.02;

/// The metronome's regions for `mode` (`Engine::click_bars`): the song's arrangement when it has clips (an empty
/// playlist plays a pattern, which counts in the project's bar), the pattern's own markers in pattern mode. A region's
/// beat is its bar over its numerator: `PPQ * 4 / denominator` ticks, one step-per-beat for the project's bar.
fn click_bars(project: &Project, mode: PlayMode) -> Vec<(u64, u32, u16)> {
    let markers = match mode {
        PlayMode::Song => project.arrangement().filter(|a| !a.clips.is_empty()).map_or(&[][..], |a| &a.markers),
        PlayMode::Pattern(n) => project.pattern(n).map_or(&[][..], |p| &p.markers),
    };
    let beats = |sig: (u16, u16)| sig.0.max(1);
    project.marker_bars(markers).into_iter().map(|(start, bar, sig)| (u64::from(start), (bar / u32::from(beats(sig))).max(1), beats(sig))).collect()
}

/// The spans of ticks a whole-song render of a song ending at `end` plays, in order (`Engine::follow_markers`; spec:
/// audio/song-structure.md "Time markers"): from the Start marker (or 0), broken at skip markers.
pub fn rendered_spans(markers: &[os_model::TimeMarker], end: u64) -> Vec<(u64, u64)> {
    let m = marks::Marks::new(markers);
    let start = m.start().filter(|&s| s < end).unwrap_or(0);
    m.rendered(start, end)
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Trigger {
    /// Tick at which the note starts.
    tick: u64,
    /// Tick the note or clip itself starts at (`tick` moves where it starts late, `Engine::clips_under_way`).
    at: u64,
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
    /// The note's fine pitch in semitones (instrument channels get it as the note's tuning).
    fine: f32,
    /// Tick at which the note is released; `None` = a step (length 0), which has no end of its own and is released
    /// with the song (spec: audio/song-structure.md, "A note").
    end: Option<u64>,
    /// Note velocity byte (instrument channels get it as 0..=1).
    velocity: u8,
    /// The note's own pan and mod X / Y as instruments get them.
    levels: crate::NoteLevels,
    /// Sample position (source frames) the voice starts at.
    from: f64,
    /// The same as a fraction of the sample (instrument channels: the sampler plugin starts there).
    start: f32,
    /// An audio clip's trimmed end as a fraction of the sample (0 = none; instrument channels: the sampler plugin
    /// ends the voice there).
    stop: f32,
    /// Which buffer of the channel it plays: 0 = the channel's sample, `k` = `keyed[k - 1]` (realtime stretch).
    buf: u16,
    /// An audio clip's fades, gain and late-start declick (`None` = plays as it is).
    shape: Option<ClipShape>,
    /// A slide note: no sound of its own, it glides the channel's voices of its colour.
    slide: bool,
    /// The note's colour group (0..=15).
    colour: u8,
    /// The note's own portamento switch (spec: audio/note-rules.md, "A note's own portamento switch").
    porta: bool,
    /// A note a layer handed on: the layer and the child's crossfade gain.
    via: Option<Via>,
    /// The group it cuts and the group its voice belongs to as the note carries them from a layer (`None`: the
    /// channel's own).
    cuts: Option<(u32, u32)>,
    /// A note the main swing can move (`None`: one it never moves).
    swing: Option<Swung>,
}

/// What a scheduled note or audio clip is known by when an edited project's engine takes over during playback
/// (`Engine::continue_from`): its channel (a layer's child: the layer), its key, and the tick it starts at before swing.
type Origin = (usize, u16, u64);

impl Trigger {
    fn origin(&self) -> Origin {
        (self.via.map_or(self.channel, |w| w.layer), self.key, self.swing.map_or(self.at, |s| s.base))
    }
}

/// What a note the main swing can move needs to be placed again when the swing changes while the song plays
/// (`Engine::set_swing`): where it starts without swing, its position in its pattern and its channel's share (spec:
/// audio/swing.md), the delay it has now, and how its end follows (`Swung::end`).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Swung {
    base: u64,
    position: u32,
    share: u16,
    delay: u32,
    /// Its length without swing (0 = a step), whether "Truncate swing notes" keeps its end, and the tick its clip
    /// releases it at the latest.
    len: u64,
    keep_end: bool,
    cap: Option<u64>,
}

impl Swung {
    /// The note's end when it starts at `tick`, as `push_note` gives it: with "Truncate swing notes" where it ends
    /// without swing, else its length after `tick`; never past the clip's cap, at least one tick long.
    fn end(&self, tick: u64) -> Option<u64> {
        if self.len == 0 {
            return None;
        }
        let end = if self.keep_end { self.base + self.len } else { tick + self.len };
        Some(self.cap.map_or(end, |c| end.min(c)).max(tick + 1))
    }
}

/// The layer a voice's note came through, and the child's crossfade gain (1 without the crossfade) (spec:
/// audio/note-rules.md "Layer channels").
#[derive(Debug, Clone, Copy, PartialEq)]
struct Via {
    layer: usize,
    gain: f64,
}

/// What a layer adds to the levels of the voices its notes started (spec: audio/note-rules.md "Layer channels": the
/// final volume is multiplied by the layer's volume, its pan, pitch and mods add): its volume with the multiplier (0
/// when it is off), its pan with the offset, its pitch knob in cents, its mod X / mod Y.
#[derive(Debug, Clone, Copy, PartialEq)]
struct LayerAdds {
    volume: f64,
    pan: f64,
    cents: f64,
    mods: (f32, f32),
}

/// An audio clip's fades, gain and start declick on its voice's clock (spec: audio/audio-clips.md "Fades and clip
/// gain", "Starting inside a clip").
#[derive(Debug, Clone, Copy, PartialEq)]
struct ClipShape {
    /// Frames of the clip already past when the voice starts (playback started inside the clip).
    since: u64,
    /// The clip's length in frames.
    total: u64,
    fade_in: u64,
    fade_out: u64,
    tension_in: f64,
    tension_out: f64,
    gain: f32,
    /// Frames of the short fade a late start begins with (0 = none).
    declick: u64,
}

impl ClipShape {
    /// The gain `age` frames after the voice started.
    fn at(&self, age: u64) -> f32 {
        let t = self.since + age;
        let mut g = f64::from(self.gain);
        if self.fade_in > 0 && t < self.fade_in {
            g *= os_model::fade_curve(t as f64 / self.fade_in as f64, self.tension_in);
        }
        if self.fade_out > 0 {
            let left = self.total.saturating_sub(t);
            if left < self.fade_out {
                g *= os_model::fade_curve(left as f64 / self.fade_out as f64, self.tension_out);
            }
        }
        if self.declick > 0 && age < self.declick {
            g *= os_model::fade_curve(age as f64 / self.declick as f64, 0.0);
        }
        g as f32
    }
}

/// Frames of the fade-in a late start inside an audio clip begins with (a Lemon Studio choice, spec:
/// audio/audio-clips.md "Starting inside a clip").
const LATE_START_SECONDS: f64 = 0.005;

/// What an instrument channel's notes were last tuned with: the channel's shift (semitones), and per key the fine pitch
/// of the note last started there, its glide and the key the instrument was given for it; `keys` marks the keys
/// started on it. A change of the shift (the main pitch, the pitch knob, automation) retunes those keys, so sounding
/// notes follow it (spec: audio/voice-levels.md "The main pitch"). `moves`: the instrument ignores a note's tuning
/// (`Instrument::takes_tuning`), so a plugin channel's notes start on keys moved by their pitch's whole semitones and
/// are tuned only by what is left; their note offs go to the moved keys.
#[derive(Debug, Clone, Copy)]
struct InstTuning {
    shift: f32,
    moves: bool,
    fine: [f32; 128],
    glide: [f32; 128],
    sent: [u8; 128],
    keys: u128,
}

impl Default for InstTuning {
    fn default() -> Self {
        InstTuning { shift: 0.0, moves: false, fine: [0.0; 128], glide: [0.0; 128], sent: std::array::from_fn(|k| k as u8), keys: 0 }
    }
}

impl InstTuning {
    /// The key the instrument was given for `key`'s latest note.
    fn sent(&self, key: u8) -> u8 {
        self.sent[usize::from(key & 127)]
    }

    /// The tuning (semitones) of `key`'s latest note: the shift, its fine pitch and glide, less what its key moved.
    fn tuning(&self, key: u8) -> f32 {
        let k = usize::from(key & 127);
        self.shift + self.fine[k] + self.glide[k] - (f32::from(self.sent[k]) - f32::from(key))
    }
}

/// An audio clip with a shape playing on an instrument channel: its voice, frames since the voice started, and the
/// gain the voice was last given (an instrument that takes voice gains; `None` before the first).
#[derive(Debug, Clone, Copy)]
struct InstClip {
    channel: usize,
    voice: u32,
    shape: ClipShape,
    age: u64,
    sent: Option<f32>,
}

/// An audio clip's fades (milliseconds, tension) and gain: the fades it plays (with automatic crossfades on, an edge
/// without its bit plays as the overlaps make it, `os_model::crossfade::playing`) and the gain the record stores.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ClipFx {
    fade_in: (f32, f32),
    fade_out: (f32, f32),
    gain: f32,
}

impl ClipFx {
    fn of(c: &os_model::Clip, (fade_in, fade_out): (Option<(f32, f32)>, Option<(f32, f32)>)) -> Option<ClipFx> {
        let fx = ClipFx { fade_in: fade_in.unwrap_or((0.0, 0.0)), fade_out: fade_out.unwrap_or((0.0, 0.0)), gain: c.gain() };
        (fx.fade_in.0 > 0.0 || fx.fade_out.0 > 0.0 || fx.gain != 1.0).then_some(fx)
    }
}

#[derive(Debug, Clone)]
struct ChannelState {
    /// The frames the channel's voices play (`None` for a sample that only `length` stands for).
    sample: Option<Arc<Sample>>,
    /// A channel its hosted instrument plays from the start (`Engine::with_instruments`): the length in frames and the
    /// rate of its sample in place of the decoded frames, which the engine never plays (`played`).
    length: Option<(usize, u32)>,
    /// Channel volume gain (0 when the channel is muted).
    volume: f64,
    /// Channel pan, −1..=1.
    pan: f64,
    /// The project's panning law (event 23), for the channel pan.
    pan_law: laws::PanLaw,
    insert: usize,
    /// Sample-rate conversion × channel pitch.
    base_rate: f64,
    /// Channel is on (a muted channel stays silent whatever a volume point says, and starts no notes).
    enabled: bool,
    /// The switch and the mixer track the channel stores, which automation changes while the song plays
    /// (`Engine::switch_channel`, `Engine::route_channel`) and a new position or a loop puts back first.
    stored: (bool, usize),
    /// Sample-rate conversion alone (`base_rate` without the channel pitch).
    src_ratio: f64,
    /// The pitch knob in cents as control points and automation leave it, and what the channel adds to it: fine tune,
    /// root note and (when the channel follows it) the main pitch (spec: audio/voice-levels.md "The channel's levels").
    /// `pitched` is false when the channel's pitch does not move its sample: a stretch time fits it, or a realtime
    /// stretch method, whose pitch is in `keyed` (spec: audio/time-stretch.md).
    knob: f64,
    init_knob: f64,
    /// A controller's pitch bend in cents on top of the knob, while its wheel is off centre (never saved; spec:
    /// ui/midi-settings.md §4).
    bend: f64,
    fine: i32,
    root: u8,
    main: Option<i32>,
    init_main: Option<i32>,
    pitched: bool,
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
    /// The channel plugin's Auto-bypass when silent (`os_model::PluginCpu::smart_disable`; `instrument_job`).
    smart: bool,
    /// A sampler channel: the instrument playing it (Lemon Sampler) applies the root note and fine tune itself, so its
    /// notes' tuning leaves them out (spec: plugins/lemon-sampler.md §7), and takes the rest as tuning (its keys are
    /// never moved, `InstTuning::moves`).
    own_tuning: bool,
    instrument: Slot,
    /// Polyphony, mono, portamento and cut group rules for the instrument's notes (spec: audio/channel-voices.md).
    rules: notes::Rules,
    /// Trim positions (source frames) times this are positions in what the channel plays: the stretch factor when
    /// that is a stretched copy.
    from_scale: f64,
    /// Realtime stretch methods: the stretch factor and cents, and the sample stretched for each key the channel's
    /// notes play.
    realtime: Option<(f64, f64)>,
    keyed: Vec<(u16, Arc<Sample>)>,
    /// An audio clip channel's own voice level, on what its hosted instrument plays (1 on other channels; spec:
    /// audio/audio-clips.md "Level").
    clip_level: f32,
    /// The level adjustments and filter knobs as automation leaves them, and as the channel stores them.
    adjust: Adjust,
    init_adjust: Adjust,
    /// A layer channel: its children and how it hands its notes on.
    layer: Option<layers::Layer>,
    /// The envelopes, LFOs and filter its hosted instrument gets (one that runs them on its voices).
    voicing: crate::instrument::ChannelVoicing,
    /// A hosted instrument's channel: the layer whose note it was given last. A hosted instrument hears that layer's
    /// levels on its channel's (a Lemon Studio choice: the host does not reach a plugin's voices one by one, so a
    /// child that also plays notes of its own, or of two layers, hears them all with the levels of the last one).
    via: Option<Via>,
}

/// A channel's level adjustments (event 229) and filter knob positions (spec: audio/voice-levels.md "The channel's
/// levels", "Values from automation").
#[derive(Debug, Clone, Copy, PartialEq)]
struct Adjust {
    /// Pan offset, -6400..6400.
    pan: i32,
    /// The volume multiplier as a gain: `curve(stored / 12800)`, 1 at the default 12800.
    mult: f32,
    /// Mod X and mod Y offsets, -256..256.
    mods: [i32; 2],
    /// Cutoff and resonance knobs (0..256) now and as stored: moving them changes the voices' mod X / mod Y by the
    /// difference.
    filter: [i32; 2],
    stored_filter: [i32; 2],
}

impl Adjust {
    /// The adjustments a channel stores (the model keeps them as knob positions: pan offset / 6400, (multiplier -
    /// 12800) / 12800, mod offsets / 256).
    fn of(ch: &os_model::Channel) -> Adjust {
        let l = ch.misc.levels;
        let at = |v: f32, scale: f32| (f64::from(v) * f64::from(scale)).round() as i32;
        let filter = [ch.sound.cutoff, ch.sound.resonance];
        Adjust {
            pan: at(l[0], 6400.0).clamp(-6400, 6400),
            mult: Adjust::multiplier((at(l[1], 12800.0) + 12800).clamp(0, 25600)),
            mods: [at(l[2], 256.0).clamp(-256, 256), at(l[3], 256.0).clamp(-256, 256)],
            filter,
            stored_filter: filter,
        }
    }

    /// The gain of a stored multiplier (0..25600): `curve(single(m) * single(1 / 12800))`.
    fn multiplier(m: i32) -> f32 {
        os_dsp::level::curve(os_dsp::level::knob(m, os_dsp::level::R12800))
    }

    /// The channel's mod X and mod Y: the offsets plus the filter knobs' moves, over 256.
    fn mods(&self) -> (f32, f32) {
        let m = |k: usize| (self.mods[k] + self.filter[k] - self.stored_filter[k]) as f32 / 256.0;
        (m(0), m(1))
    }
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

/// A pattern control point placed on its tick.
#[derive(Debug, Clone, Copy)]
struct CtlEvent {
    tick: u64,
    to: CtlTo,
    /// In the setting's own unit (spec: audio/automation.md "Pattern control points").
    value: i32,
}

/// What a control point sets.
#[derive(Debug, Clone, Copy, PartialEq)]
enum CtlTo {
    /// A channel's own setting: rack position, parameter (`ChannelState::apply`).
    Channel(usize, u16),
    /// A mixer value, a slot, the tempo.
    Target(os_model::target::Target),
    /// A plugin's parameter, with the level the value stands for once the plugin is known (`Engine::set_effect`,
    /// `Engine::set_instrument`; spec: plugins/plugin-parameters.md §1).
    Param(os_model::target::Target, Option<f64>),
}

/// Frames of a sample of `frames` frames stretched to `factor` times its length.
fn stretched_frames(frames: usize, factor: f64) -> usize {
    (frames as f64 * factor).round().max(1.0) as usize
}

impl ChannelState {
    /// The buffer a voice plays (`Trigger::buf`).
    fn buffer(&self, buf: u16) -> Option<&Arc<Sample>> {
        match buf {
            0 => self.sample.as_ref(),
            b => self.keyed.get(usize::from(b) - 1).map(|k| &k.1),
        }
    }

    /// The length in frames and the rate of what the channel plays: its sample, or the length that stands for it.
    fn played(&self) -> Option<(usize, u32)> {
        self.sample.as_ref().map(|s| (s.frames(), s.rate)).or(self.length)
    }

    /// Frames of the buffer `buf` (`buffer`; the sample's length for a channel that only has that).
    fn frames(&self, buf: u16) -> Option<usize> {
        match buf {
            0 => self.played().map(|p| p.0),
            b => self.buffer(b).map(|s| s.frames()),
        }
    }

    fn reset(&mut self) {
        (self.volume, self.pan, self.base_rate) = self.init;
        (self.enabled, self.insert) = self.stored;
        (self.knob, self.main) = (self.init_knob, self.init_main);
        self.vol_v = self.init_v;
        self.ramp = None;
        self.adjust = self.init_adjust;
    }

    /// The pan with the channel's pan offset: `single(pan * 6400 + offset) * single(1 / 6400)`, the stored integers
    /// added before they are scaled (spec: audio/voice-levels.md "The channel's levels"); the pan as it is without an
    /// offset.
    fn pan_of(&self, pan: f64) -> f64 {
        if self.adjust.pan == 0 {
            return pan;
        }
        f64::from((pan * 6400.0 + f64::from(self.adjust.pan)) as f32 * os_dsp::level::R6400)
    }

    /// What this channel adds as a layer to its children's voices. The pitch is the pitch knob alone (a Lemon Studio
    /// choice: the spec compares a layer's pitch knob only, with its fine tune, root note and the main pitch at their
    /// defaults).
    fn layer_adds(&self) -> LayerAdds {
        LayerAdds { volume: self.volume, pan: self.pan_of(self.pan), cents: self.knob, mods: self.adjust.mods() }
    }

    /// The gain of a note that starts now (`start_gain`) of a voice whose note came through a layer.
    fn start_gain_via(&self, vel: f64, note_pan: f64, layer: Option<(LayerAdds, f64)>) -> (f32, f32) {
        match layer {
            None => self.start_gain(vel, note_pan),
            Some((a, gain)) => self.start_gain(vel * a.volume * gain, note_pan + a.pan),
        }
    }

    /// The channel's levels for a hosted instrument.
    ///
    /// A control point or automation value is handed on whole from its tick: the instrument takes it into its voices'
    /// gain ramp, one limited ramp per tick (spec: audio/sampler.md "Gains"); the engine's glide is its own voices'
    /// and its output gain's.
    fn levels(&self) -> crate::instrument::ChannelLevels {
        let (mod_x, mod_y) = self.adjust.mods();
        let (v, p) = self.ramp.map_or((self.vol_v, self.pan), |r| (r.v1, r.p1));
        crate::instrument::ChannelLevels { volume: self.vol_gain(v) as f32, pan: self.pan_of(p) as f32, mod_x, mod_y }
    }

    /// A value for one of the channel's filter knobs or level adjustments from an automation clip (`v` 0..1, spec:
    /// audio/voice-levels.md "Values from automation"); sounding voices follow.
    fn automate_level(&mut self, id: u16, v: f64) {
        use os_model::target;
        let round = |x: f64| x.round_ties_even() as i32;
        let a = &mut self.adjust;
        match id {
            target::CHANNEL_CUTOFF => a.filter[0] = round(256.0 * v),
            target::CHANNEL_RESONANCE => a.filter[1] = round(256.0 * v),
            target::CHANNEL_PAN_OFFSET => a.pan = round(12800.0 * v) - 6400,
            target::CHANNEL_VOLUME_MULT => {
                a.mult = Adjust::multiplier(round(25600.0 * v));
                self.volume = self.vol_gain(self.vol_v);
            }
            target::CHANNEL_MOD_X => a.mods[0] = round(512.0 * v) - 256,
            target::CHANNEL_MOD_Y => a.mods[1] = round(512.0 * v) - 256,
            _ => {}
        }
    }

    /// Gain of a note that starts now: a glide in progress is already at its final values for it.
    fn start_gain(&self, vel: f64, note_pan: f64) -> (f32, f32) {
        match self.ramp {
            None => self.gain(vel, note_pan),
            Some(r) => {
                let g = self.vol_gain(r.v1) * vel;
                let (pl, pr) = laws::channel_pan_law(self.pan_of(r.p1) + note_pan, self.pan_law);
                ((g * pl) as f32, (g * pr) as f32)
            }
        }
    }

    /// The channel's volume gain for volume parameter `v`: its level curve times the volume multiplier (spec:
    /// audio/voice-levels.md "Final levels": the channel's two first), 0 when the channel is off.
    fn vol_gain(&self, v: f64) -> f64 {
        if self.enabled { laws::channel_gain(v) * f64::from(self.adjust.mult) } else { 0.0 }
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

    /// Sets the pitch knob (cents) and with it the channel's rate: the knob, fine tune, root note and main pitch
    /// (spec: audio/voice-levels.md), counted here from key 60 as the voices' key rates are.
    fn retune(&mut self, knob: f64) {
        self.knob = knob;
        self.base_rate = self.src_ratio * if self.pitched { laws::cents_rate(self.pitch_from_60()) } else { 1.0 };
    }

    /// The channel's pitch in cents from key 60.
    fn pitch_from_60(&self) -> f64 {
        f64::from(laws::channel_pitch(self.knob, self.fine, self.root, self.main)) + self.bend + 6000.0
    }

    /// The tuning (semitones) a hosted instrument hears with each note: the channel's pitch, without the root note and
    /// fine tune when the instrument applies those itself.
    fn hosted_shift(&self) -> f32 {
        let shift = if self.own_tuning {
            if self.pitched { (f64::from(laws::channel_pitch(self.knob, 0, 60, self.main)) + 6000.0) / 100.0 } else { 0.0 }
        } else {
            12.0 * (self.base_rate / self.src_ratio).log2()
        };
        let shift = shift as f32;
        if shift.is_finite() && shift.abs() > 1e-3 { shift } else { 0.0 }
    }

    /// A control point replaces the channel's own setting (spec: audio/pattern-controls.md).
    fn apply(&mut self, param: u16, value: i32, rate: u32) {
        match param {
            ControlPoint::VOLUME => self.glide(Some((f64::from(value) / 12800.0).clamp(0.0, 1.0)), None, (f64::from(rate) * VOLUME_GLIDE_SECONDS).round() as u32),
            ControlPoint::PAN => self.glide(None, Some((f64::from(value) / 6400.0 - 1.0).clamp(-1.0, 1.0)), (f64::from(rate) * PAN_GLIDE_SECONDS).round() as u32),
            ControlPoint::PITCH => self.retune(f64::from(value)),
            // The filter knobs' positions 0..256 (spec: audio/voice-levels.md "Values from automation").
            os_model::target::CHANNEL_CUTOFF => self.adjust.filter[0] = value.clamp(0, 256),
            os_model::target::CHANNEL_RESONANCE => self.adjust.filter[1] = value.clamp(0, 256),
            _ => {}
        }
    }

    /// `gain` for a voice whose note came through a layer.
    fn gain_via(&self, vel: f64, note_pan: f64, layer: Option<(LayerAdds, f64)>) -> (f32, f32) {
        match layer {
            None => self.gain(vel, note_pan),
            Some((a, gain)) => self.gain(vel * a.volume * gain, note_pan + a.pan),
        }
    }

    /// `gain_via` of no note pan where the channel will be `frames` from now, a running glide that far along.
    fn gain_after(&self, frames: u32, vel: f64, layer: Option<(LayerAdds, f64)>) -> (f32, f32) {
        let Some(r) = self.ramp else { return self.gain_via(vel, 0.0, layer) };
        let (lv, lp) = layer.map_or((1.0, 0.0), |(a, g)| (a.volume * g, a.pan));
        let at = f64::from(r.pos.saturating_add(frames));
        let g = self.vol_gain(r.v0 + (r.v1 - r.v0) * r.volume_at(at)) * vel * lv;
        let (pl, pr) = laws::channel_pan_law(self.pan_of(r.p0 + (r.p1 - r.p0) * r.pan_at(at)) + lp, self.pan_law);
        ((g * pl) as f32, (g * pr) as f32)
    }

    /// Gain of a note on this channel: channel volume × velocity × pan.
    // spec: audio/sampler-playback.md (velocity multiplies volume; pans add, clamped)
    fn gain(&self, vel: f64, note_pan: f64) -> (f32, f32) {
        let g = self.volume * vel;
        let (pl, pr) = laws::channel_pan_law(self.pan_of(self.pan) + note_pan, self.pan_law);
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
    /// An audio clip's fades and gain.
    shape: Option<ClipShape>,
    /// The channel buffer it plays (`Trigger::buf`).
    buf: u16,
    /// Its note's key, and where slide notes have moved it.
    key: u16,
    slide: VoiceSlide,
    /// Its note's colour group: slide notes of that colour move it.
    colour: u8,
    /// The layer its note came through.
    via: Option<Via>,
    /// The note or audio clip that started it (`None`: a live note).
    origin: Option<Origin>,
}

/// A voice's levels as slide notes move them (spec: audio/song-structure.md "Slide notes", audio/note-rules.md
/// "Glides"): the pitch moved in cents, the velocity byte and the note pan, singles that each tick of a glide adds its
/// step to; the key rate the voice started with.
#[derive(Debug, Clone, Copy, PartialEq)]
struct VoiceSlide {
    cents: f32,
    velocity: f32,
    pan: f32,
    rate: f64,
    /// Steps per tick of a glide under way, and the steps left.
    step: (f32, f32, f32),
    left: u32,
}

impl VoiceSlide {
    fn new(rate: f64, velocity: u8, pan: f64) -> Self {
        VoiceSlide { cents: 0.0, velocity: f32::from(velocity), pan: pan as f32, rate, step: (0.0, 0.0, 0.0), left: 0 }
    }
}

/// Release fade of a live note (a Lemon Studio choice, spec: ui/typing-keyboard.md open questions).
const RELEASE_SECONDS: f64 = 0.015;

/// An automation clip placed on the tick line (spec: ui/playlist.md Â§8, flp/events/automation.md).
#[derive(Debug, Clone)]
struct AutoClip {
    /// Ticks at which the clip starts and ends.
    start: u64,
    end: u64,
    /// Beats into the automation at the clip start (its window).
    from_beat: f64,
    /// Index into `Engine::curves`.
    curve: usize,
    /// Every target the clip drives (one link each, spec: audio/automation.md "The link"), unknown ones left out.
    targets: Vec<os_model::target::Target>,
    /// The walk along the curve and the tick steps it has taken (spec: audio/automation.md "Walking the curve").
    walk: Option<(os_model::curve::Walk, u64)>,
    /// The clip's range (its channel's two level knobs) and its LFO (spec: audio/automation.md "Range", "The LFO").
    out: os_model::clip_lfo::ClipOut,
    /// The walk along the LFO's own envelope.
    env: os_model::clip_lfo::EnvWalk,
}

struct MixTrack {
    /// Fader (0..16000, 12800 = unity), pan (-6400..6400) and stereo separation (-64..64, positive merges) as stored;
    /// automation rewrites them, then the matrix (spec: audio/mixer.md).
    vol: i32,
    pan: i32,
    sep: i32,
    on: bool,
    /// The project's panning law (event 23), for the pan in the matrix.
    law: laws::PanLaw,
    /// Swap left / right and reverse polarity, on the fader matrix (spec: audio/mixer.md "Fader matrix").
    swap: bool,
    polarity: bool,
    /// The track's "effect slots enabled" switch: when off no slot runs (spec: audio/mixer.md).
    slots_enabled: bool,
    /// Allow threaded processing: off, the track renders on the rendering thread even on several cores (spec:
    /// audio/multicore.md §5).
    threaded: bool,
    /// The three-band equaliser, after the slots and before the fader.
    eq: os_dsp::mixer::TrackEq,
    /// Insert effects in slot order (applied before the fader).
    effects: Vec<FxSlot>,
    /// Slot switches and stored mix levels (0..12800) by slot, for hosted effects attached later.
    slot_on: [bool; 10],
    slot_mix: [i32; 10],
    /// Auto-bypass when silent, by slot (`auto_bypass`), for hosted effects attached later.
    slot_smart: [bool; 10],
    /// Largest |sample| per side after the fader since the meters last read them.
    peak: [f32; 2],
    /// The fader matrix's target gains `[[L←L, L←R], [R←L, R←R]]`, and the gains as they glide towards them.
    matrix: [[f32; 2]; 2],
    gains: [[Glide; 2]; 2],
    /// The track's sends. Routes at level 0 (sidechain only) pass no audio but stay: automation may raise them.
    routes: Vec<Route>,
    /// The settings as stored, put back when playback starts at another position.
    stored: Stored,
    /// The output and input latency settings in milliseconds (event 236; spec: audio/delay-compensation.md "Settings").
    latency: (f32, f32),
    /// What the engine itself plays into the track (sampler voices, the metronome) waits here for the latest path
    /// into it (delay compensation).
    pad: pdc::Pad,
    /// The device input the track hears (-1: none, or not monitored) and the output pair it plays on directly (-1:
    /// none), numbered as stored (spec: audio/track-io.md §2.1, §2.2).
    input: i32,
    output: i32,
    /// Armed for disk recording: where the take is tapped and the input it records (spec: audio/track-io.md §2.5).
    rec: Option<rec::Arm>,
}

/// The audio device's inputs as they arrive, for the mixer tracks that monitor one (spec: audio/track-io.md §2.2).
pub trait DeviceInput: Send {
    /// Inputs the device has open.
    fn channels(&self) -> usize;
    /// The next `out.len() / channels` frames of every input, interleaved; what has not arrived is silence.
    fn read(&mut self, out: &mut [f32]);
}

/// A stream's device: its inputs, and its output pairs, which the master and the tracks' direct outputs play on. With
/// it the engine hands out `2 × pairs` interleaved channels instead of the master (`Engine::set_device`).
struct DeviceIo {
    input: Option<Box<dyn DeviceInput>>,
    pairs: usize,
    /// This tick's inputs (interleaved), and what the tracks send to the pairs directly.
    inputs: Vec<f32>,
    direct: Vec<f32>,
}

/// Input number `n` of a device with `channels` inputs as (left, right) channels: the stereo pairs first, then each
/// input alone, heard on both sides (spec: audio/track-io.md §2.1).
pub fn input_channels(n: i32, channels: usize) -> Option<(usize, usize)> {
    let n = usize::try_from(n).ok()?;
    let pairs = channels / 2;
    if n < pairs {
        Some((2 * n, 2 * n + 1))
    } else {
        (n - pairs < channels).then_some((n - pairs, n - pairs))
    }
}

/// A mixer track's settings as the project stores them.
#[derive(Debug, Clone)]
struct Stored {
    vol: i32,
    pan: i32,
    sep: i32,
    eq: [[i32; 3]; 3],
    slot_on: [bool; 10],
    slot_mix: [i32; 10],
    /// Send gains, one per route.
    sends: Vec<f32>,
}

/// A send to another track: its gain and the gain as it glides there (spec: audio/mixer.md "Sends", "Gains ramp").
struct Route {
    dest: usize,
    gain: f32,
    glide: Glide,
    /// Delay compensation: the send, after its level, waits here for the latest path into `dest`.
    pad: pdc::Pad,
}

/// One filled mixer slot: Lemon Studio's own effect or a hosted plugin, with the slot's switch and stored mix level
/// (0..12800) and its dry level as it glides (spec: audio/mixer.md "An effect slot").
struct FxSlot {
    slot: usize,
    on: bool,
    mix: i32,
    /// Auto-bypass when silent, and how long the slot has heard and put out silence (`auto_bypass`).
    smart: bool,
    quiet: auto_bypass::Quiet,
    /// The last tick passed the effect by.
    skipped: bool,
    dry: Glide,
    /// Switched on during the song: the effect is emptied before it runs again.
    fresh: bool,
    fx: Fx,
    /// The delay the effect last reported (read after it processes), and the pad its dry part waits in for it (spec:
    /// audio/delay-compensation.md "A slot's dry part").
    delay: u32,
    dry_pad: pdc::Pad,
    /// The sidechain input: the track it listens to (worked out with the pads), and the pad that delays that track's
    /// output to the lateness at the slot (spec: audio/sidechain.md "Sidechain input").
    key: Option<usize>,
    key_pad: pdc::Pad,
    /// A hosted plugin's input ports, main and first auxiliary bus (0 = none), and the track its main bus takes instead
    /// of the track's own audio, with the pad that delays it like a sidechain input (spec: audio/sidechain.md "Hosted
    /// plugins' input buses").
    ports: [usize; 2],
    main: Option<usize>,
    main_pad: pdc::Pad,
}

impl FxSlot {
    fn new(slot: usize, on: bool, mix: i32, smart: bool, fx: Fx) -> FxSlot {
        let delay = match &fx {
            Fx::Own(e) => e.latency() as u32,
            Fx::Hosted(_) => 0,
        };
        FxSlot {
            slot,
            on,
            mix,
            smart,
            quiet: auto_bypass::Quiet::default(),
            skipped: false,
            dry: Glide::new(os_dsp::mixer::dry_level(mix)),
            fresh: false,
            fx,
            delay,
            dry_pad: pdc::Pad::default(),
            key: None,
            key_pad: pdc::Pad::default(),
            ports: [0; 2],
            main: None,
            main_pad: pdc::Pad::default(),
        }
    }

    /// The number of the track's input the slot listens to, counted from 1 (0 = none).
    fn key_input(&self) -> usize {
        match &self.fx {
            Fx::Own(e) => e.key_input(),
            Fx::Hosted(_) => self.ports[1],
        }
    }
}

/// A stored mix level (0..12800) from the model's 0..1.
fn mix_level(v: f64) -> i32 {
    (v.clamp(0.0, 1.0) * 12800.0).round() as i32
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
            law: laws::PanLaw::Circular,
            swap: false,
            polarity: false,
            slots_enabled: true,
            threaded: true,
            eq: os_dsp::mixer::TrackEq::default(),
            effects: Vec::new(),
            slot_on: [true; 10],
            slot_mix: [12800; 10],
            slot_smart: [false; 10],
            peak: [0.0; 2],
            matrix: unity,
            gains: unity.map(|r| r.map(Glide::new)),
            routes: vec![],
            stored: Stored {
                vol: 12800,
                pan: 0,
                sep: 0,
                eq: os_dsp::mixer::EQ_DEFAULT,
                slot_on: [true; 10],
                slot_mix: [12800; 10],
                sends: vec![],
            },
            latency: (0.0, 0.0),
            pad: pdc::Pad::default(),
            input: -1,
            output: -1,
            rec: None,
        }
    }

    /// The settings as they are now, kept to be put back by [`MixTrack::restore`].
    fn store(&mut self) {
        self.stored = Stored {
            vol: self.vol,
            pan: self.pan,
            sep: self.sep,
            eq: [0, 1, 2].map(|b| self.eq.settings(b)),
            slot_on: self.slot_on,
            slot_mix: self.slot_mix,
            sends: self.routes.iter().map(|r| r.gain).collect(),
        };
    }

    /// Puts the stored settings back (playback starting at another position: nothing of an automation clip holds
    /// before the clip; spec: audio/automation.md "When a clip acts"), with every gain at its target when `settle`,
    /// else gliding there.
    fn restore(&mut self, settle: bool) {
        let s = self.stored.clone();
        (self.vol, self.pan, self.sep) = (s.vol, s.pan, s.sep);
        for (b, band) in s.eq.iter().enumerate() {
            for (f, &v) in band.iter().enumerate() {
                if self.eq.settings(b)[f] != v {
                    self.eq.set(b, f, v);
                }
            }
        }
        for k in 0..10 {
            self.set_slot_on(k, s.slot_on[k]);
            self.set_slot_mix(k, s.slot_mix[k]);
        }
        for (r, &g) in self.routes.iter_mut().zip(&s.sends) {
            r.gain = g;
        }
        self.matrix = self.compute_matrix();
        if settle {
            self.settle();
        }
    }

    /// Every gain at its target at once: the matrix, the slots' dry levels, the sends.
    fn settle(&mut self) {
        self.gains = self.matrix.map(|r| r.map(Glide::new));
        for fx in &mut self.effects {
            fx.dry = Glide::new(os_dsp::mixer::dry_level(fx.mix));
        }
        for r in &mut self.routes {
            r.glide = Glide::new(r.gain);
        }
    }

    /// Slot `slot`'s switch. A slot switched on empties its effect first; neither way fades (spec: audio/mixer.md
    /// "Changes while the song plays").
    fn set_slot_on(&mut self, slot: usize, on: bool) {
        let Some(s) = self.slot_on.get_mut(slot) else { return };
        *s = on;
        for fx in self.effects.iter_mut().filter(|f| f.slot == slot) {
            fx.fresh |= on && !fx.on;
            fx.on = on;
        }
    }

    /// Slot `slot`'s stored mix level (0..12800); its dry level glides there.
    fn set_slot_mix(&mut self, slot: usize, mix: i32) {
        let Some(m) = self.slot_mix.get_mut(slot) else { return };
        *m = mix;
        for fx in self.effects.iter_mut().filter(|f| f.slot == slot) {
            fx.mix = mix;
        }
    }

    /// The fader matrix (volume, pan under the project's panning law, stereo separation, switch; spec: audio/mixer.md
    /// "Fader matrix", audio/gain-laws.md "Panning law: Triangular") with swap and polarity on top, as
    /// `[[L←L, L←R], [R←L, R←R]]`.
    fn compute_matrix(&self) -> [[f32; 2]; 2] {
        let [ll, rl, rr, lr] = os_dsp::mixer::fader_matrix_law(self.vol, self.pan, self.sep, self.on, self.law);
        let mut m = [[ll, rl], [lr, rr]];
        // Swap exchanges the two outputs after pan and separation, not the inputs: (LL, RR, RL, LR) become
        // (LR, RL, RR, LL).
        if self.swap {
            m = [m[1], m[0]];
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
    /// The main volume's gain `curve(stored / 12800)` and its ramp (spec: audio/mixer.md "Main volume"). Playback
    /// multiplies the master by it; an export does not (FL applies it after writing the tick), but the tail test sees it.
    main_gain: f32,
    main_glide: os_dsp::glide::Glide,
    /// Set while rendering an export (`render_ticks`), cleared by live playback (`process`).
    exporting: bool,
    channels: Vec<ChannelState>,
    tracks: Vec<MixTrack>,
    /// Mixer tracks in processing order: every track before the tracks it sends to.
    order: Vec<usize>,
    triggers: Vec<Trigger>,
    next_trigger: usize,
    /// The main swing in force (0..=128): the project's, or what automation last set (`set_swing`); and whether a
    /// scheduled note can move with it.
    swing: u8,
    swung: bool,
    /// Audio clips already under way where playback starts, moved to that tick (spec: audio/audio-clips.md,
    /// "Starting inside a clip"), and with `truncated_on_transport` the notes under way there.
    late: Vec<Trigger>,
    /// A paused playback let go of the audio clips its hosted instruments played (`release_instruments`): resuming
    /// starts them again where they have got to.
    relaunch_clips: bool,
    /// "Play truncated notes on transport" (`set_truncated_on_transport`): a seek starts the notes under way.
    truncated_on_transport: bool,
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
    /// While looping: the playlist's time selection (start, end ticks), played over and over in place of the song.
    loop_range: Option<(u64, u64)>,
    /// Only live notes, no schedule: finished once everything is silent; the tail rule does not apply.
    live_only: bool,
    /// A live-only engine that never finishes: its hosted instruments are processed every tick, so notes played in
    /// their own editors sound (`set_awake`).
    awake: bool,
    /// The tail rule has ended the song: a tick at or after its end stayed quiet (spec: audio/timing.md).
    tail_done: bool,
    /// Pause (`pause`): the channels fade out of the mixer and are then held where they are (`held`) while the mixer
    /// runs on; `resume` fades them back in. Frames left of the fade out / in, and the fades' length.
    held: bool,
    fade_out: u32,
    fade_in: u32,
    fade_len: u32,
    /// A recording count-in (`set_count_in`): ticks of it left, and its length. While it lasts the position holds at the
    /// start and the metronome counts in (spec: ui/recording.md §3).
    count_in: u64,
    count_in_len: u64,
    /// Recording with Loop record off (`set_run_on`): while the flag is set, a looping playback runs on past its end
    /// instead of going back to the start (spec: ui/recording.md §4).
    run_on: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// The Tempo tapper's nudge (`set_nudge`): -1, 0 or +1, and the one the clock runs at now.
    nudge: Option<std::sync::Arc<std::sync::atomic::AtomicI8>>,
    nudged: i8,
    /// The arrangement's time markers (song mode with clips; spec: audio/transport.md "Time markers in SONG mode").
    marks: marks::Marks,
    /// The next tick was arrived at by a start or a jump: markers on it do not act.
    landed: bool,
    /// A quantized song jump (`jump_on`): (unit, target) in ticks.
    quantized_jump: Option<(u64, u64)>,
    /// A Start recording marker was passed: the song runs on past its end.
    punched: bool,
    /// The Marker pause the channels are fading out toward, and the one playback holds on until it is taken
    /// (`take_marker_pause`).
    pause_at: Option<u64>,
    marker_paused: Option<u64>,
    /// A render that follows the markers (`follow_markers`): the spans of ticks it plays, in order (empty: the song
    /// from where it starts).
    spans: Vec<(u64, u64)>,
    tempo: f64,
    ppq: u16,
    /// Steps per beat (event 18): the tempo counts beats of this many sixteenths, so the ticks run at `tempo × steps /
    /// 4` quarter notes a minute (`tick_tempo`; spec: audio/timing.md "The tempo counts the bar's beats").
    steps: u16,
    /// The project's time signature, numerator and denominator (4/4 when the file has none): what hosted plugins are
    /// told (spec: plugins/vst3-hosting.md §1). A bar whose steps per beat make no signature (3) is told as its length
    /// in sixteenths over 16 (what FL tells plugins is not measured: spec: audio/song-structure.md, open questions).
    signature: (u16, u16),
    bus: Vec<Vec<f32>>,
    /// The last rendered tick (interleaved L/R), how much of it has been handed out, the song position its first
    /// sample was made at (tick, with the share of a tick of `out_len` frames, and frame), and whether it is a held
    /// one (`render_held`: the position stays at that tick and frame all through it). With delay compensation the
    /// position is that of the stream `lag` frames earlier, where what is heard was made.
    out: Vec<f32>,
    out_pos: usize,
    out_tick: f64,
    out_frame: u64,
    out_held: bool,
    out_len: u64,
    /// Playing on a device (`set_device`): `out` holds its output pairs instead of the master.
    io: Option<DeviceIo>,
    /// Instrument channels: scheduled note ends (tick, channel, key, an audio clip's voice: choked at the clip's end;
    /// `None`: a note, which ends with a note off; the note or clip that started it, `None` for a live note), live
    /// notes waiting for the next tick (channel, key, velocity; negative = note off), this tick's events.
    inst_offs: Vec<(u64, usize, u8, Option<u32>, Option<Origin>)>,
    /// Audio clips with fades or gain playing on instrument channels.
    inst_shapes: Vec<InstClip>,
    /// The name the next audio clip voice on an instrument gets (`Instrument::voice_start`).
    next_voice: u32,
    /// Live notes for instrument channels waiting for the next tick (channel, key, velocity or -1 for an end, MIDI
    /// channel).
    inst_live: Vec<(usize, u8, f32, u8)>,
    /// Controller messages for instrument channels waiting for the next tick (channel, message; `Engine::midi`).
    inst_midi: Vec<(usize, [u8; 3])>,
    inst_events: Vec<notes::Event>,
    /// Per instrument channel: the tuning its notes were last given (`InstTuning`).
    inst_tuning: Vec<InstTuning>,
    /// This tick's events after the channels' note rules, sorted by channel, and each channel's notes at its
    /// instrument (spec: audio/note-rules.md).
    inst_notes: Vec<(usize, u32, notes::Note)>,
    note_voices: Vec<notes::Voices>,
    /// Per instrument channel: the frames of the ticks its instrument missed (still busy after `BUSY_WAIT`) and the
    /// notes it missed (channel, note); both are made up when it is reached again, so it stays in time with the song.
    inst_owed: Vec<usize>,
    inst_missed: Vec<(usize, notes::Note)>,
    /// When this tick stops waiting for hosted plugins busy elsewhere (set at its first wait, `busy_wait` from then).
    busy_until: Option<Instant>,
    busy_wait: Duration,
    /// Ticks hosted plugins missed since `take_missed` (each heard as a gap on its channel or track).
    missed: u64,
    /// Per track, this tick's output when a slot listens to it as its sidechain input.
    key_out: Vec<Option<Vec<f32>>>,
    /// Frames instruments keep being processed after their last held note (their release / tail is not known).
    inst_tail: u64,
    /// A hosted instrument reported sound still to come during the last tick.
    inst_active: bool,
    /// Per channel, this tick: a note of it is held or due to end (its instrument is not auto-bypassed).
    inst_held: Vec<bool>,
    /// Automation clips of the playlist and their curves (song mode).
    autos: Vec<AutoClip>,
    curves: Vec<os_model::Automation>,
    /// The value last handed on per automation target (a value is passed on only when it differs), and the tick from
    /// which values are set in place rather than smoothed (playback's first tick after a start, seek or loop).
    auto_last: Vec<(os_model::target::Target, i64)>,
    auto_from: u64,
    /// Controller links from slots to targets (spec: plugins/lemon-peak-controller.md "Links").
    ctl_links: Vec<controllers::CtlLink>,
    /// The project's initial values the engine can set (song mode), as automation values: a song that loops sets them
    /// again at its start (spec: ui/settings-choices.md "Advanced: Initialize controls automatically").
    init: Vec<(os_model::target::Target, i64)>,
    /// Clips on plugin parameters: this tick's events.
    params: param_auto::ParamAuto,
    /// The project's random generator (random layers; spec: audio/channel-misc.md "FL's random generator").
    rng: layers::Mt19937,
    /// The metronome of a playback stream (`set_metronome`), and the beats from one accent to the next: the project
    /// signature's numerator (spec: ui/metronome.md §4).
    metronome: Option<crate::metronome::Metronome>,
    beats_per_accent: u16,
    /// Where the metronome counts from (spec: ui/metronome.md §4, audio/song-structure.md "Time signatures and a
    /// pattern's length"): `(start tick, beat ticks, beats per bar)` per time-signature region, the arrangement's
    /// markers in song mode and the pattern's in pattern mode, the project's bar from 0 until the first one.
    click_bars: Vec<(u64, u32, u16)>,
    /// The Tempo tapper's clicks (`tap_click`), on any live stream.
    tap_clicks: crate::metronome::TapClicks,
    /// The sampler channels' interpolation window, half its width in source samples (`set_interpolation`; 1 or less:
    /// linear).
    sinc_w: f64,
    /// Whether the insert tracks' slots and the master's run at all (`set_effects`: an export's Insert / Master
    /// effects switches; spec: audio/export.md §3).
    insert_fx: bool,
    master_fx: bool,
    /// Every mixer track's post-fader output, tick after tick, while capturing (`set_track_capture`; an export's Split
    /// mixer tracks).
    capture: Option<Vec<Vec<f32>>>,
    /// Disk recording: what the armed tracks take while the song plays goes here (`set_record_tap`).
    record: Option<std::sync::Arc<dyn RecordTap>>,
    /// Plugin delay compensation: the pads not kept with the tracks, the head of the stream skipped, the values
    /// waiting for late audio (spec: audio/delay-compensation.md).
    pdc: pdc::Pdc,
    /// The stored main volume is exactly 12800: with a delay the tail test looks at less of a tick (spec:
    /// audio/timing.md "Where the file ends").
    main_unity: bool,
    /// Rendering on several cores: the pool and the mixer's graph (spec: audio/multicore.md).
    multicore: multicore::Multicore,
    pub report: EngineReport,
}

/// Length of the fade that takes the channels out of the mixer at Pause, and back in at Resume (as the stream's own
/// stop fade).
const PAUSE_FADE_SECONDS: f64 = 0.006;
/// Seconds an instrument keeps running after its last note (a Lemon Studio choice).
const INSTRUMENT_TAIL_SECONDS: u64 = 5;

/// How long one tick waits in all for hosted plugins that are busy elsewhere: the UI thread holds an instance for
/// microseconds at a time (its housekeeping), longer only to open an editor, load a state or restart it. A plugin
/// still busy after that misses the tick (spec: audio/implementing.md "Playing live").
const BUSY_WAIT: Duration = Duration::from_millis(2);

/// A tick whose every output sample stays below this magnitude (-96 dB) is quiet: the first quiet tick at or after
/// the song's end is the file's last (spec: audio/timing.md, "Where the file ends").
const QUIET: f32 = 1.584_893_1e-5;
/// Safety stop for the tail: minutes rendered past the song's end before giving up on silence (a Lemon Studio choice).
const TAIL_LIMIT_SECONDS: f64 = 600.0;

impl Engine {
    pub fn new(project: &Project, library: &mut SampleLibrary, rate: u32, mode: PlayMode) -> Self {
        Self::build(project, library, rate, mode, &[])
    }

    /// `new` with hosted instruments on channels (rack rows) from the start, each attached as `set_instrument` does.
    /// The engine never plays the sample of a sampler or audio clip channel among them: of it, it keeps only the length
    /// and the rate (trims, stretching, how long a one-shot note rings) where the library reads them without decoding
    /// the file (`SampleLibrary::length`), not the decoded frames (spec: plugins/lemon-sampler.md "Keep on disk (Lemon
    /// Studio)"). A channel whose instrument comes later (`set_instrument`) keeps its decoded sample, as before.
    pub fn with_instruments(project: &Project, library: &mut SampleLibrary, rate: u32, mode: PlayMode, instruments: &[(usize, SharedInstrument)]) -> Self {
        let hosted: Vec<usize> = instruments.iter().map(|(row, _)| *row).collect();
        let mut e = Self::build(project, library, rate, mode, &hosted);
        for (row, inst) in instruments {
            e.set_instrument(*row, inst.clone());
        }
        e
    }

    fn build(project: &Project, library: &mut SampleLibrary, rate: u32, mode: PlayMode, hosted: &[usize]) -> Self {
        let mut report = EngineReport::default();
        let tempo = project.tempo.unwrap_or(140.0);
        let ppq = project.ppq.max(1);
        // The tempo the ticks run at (spec: audio/timing.md "The tempo counts the bar's beats").
        let tick_tempo = project.tick_tempo(tempo);
        let n_tracks = project.mixer.len().max(1);
        // The panning law of the channel and mixer track pans (event 23; spec: audio/gain-laws.md "Panning law:
        // Triangular"). Not of other pans: whether it moves them was not measured. An edit is heard from the next tick,
        // as any pan edit (`continue_from`).
        let pan_law = laws::PanLaw::from_event(project.panning_law);

        let mut channels: Vec<ChannelState> = project
            .channels
            .iter()
            .enumerate()
            .map(|(row, ch)| {
                let plays_sample = matches!(ch.kind, ChannelKind::Sampler | ChannelKind::AudioClip);
                if ch.kind == ChannelKind::Plugin {
                    report.plugin_channels_silent += 1;
                }
                // A channel its hosted instrument plays: the length and rate of its sample are all it needs.
                let length = ch.sample_path.as_deref().filter(|_| plays_sample && hosted.contains(&row)).and_then(|p| library.length(p));
                // The hosted sampler loops the sample: "Use loop points" is on (a sampler's default; an audio clip's is
                // off) and there is a loop, the channel's own (one that is not the whole file) or the file's. Its voices
                // are not one-shot voices: they sound until their note ends (spec: audio/note-rules.md, audio/sampler.md
                // "The sample file"). The engine's own voices never loop.
                let use_loop = ch.sound.sampler_flags.map_or(ch.kind == ChannelKind::Sampler, |f| f & os_model::ChannelSound::USE_LOOP != 0);
                let own_loop = |n: usize| ch.sound.loop_points.is_some_and(|[a, b]| b > a && (a > 0 || b < n as i64));
                let looped = length.is_some_and(|(n, _)| {
                    use_loop && (own_loop(n) || ch.sample_path.as_deref().is_some_and(|p| library.file_loops(p)))
                });
                let sample = if plays_sample && length.is_none() {
                    match ch.sample_path.as_deref() {
                        Some(p) => {
                            let s = library.get(p);
                            if s.is_none() {
                                report.missing_samples.push(p.to_string());
                            }
                            s
                        }
                        // Lemon Studio choice: an empty sampler channel plays a built-in tone so new
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
                let adjust = Adjust::of(ch);
                let volume = if ch.enabled { laws::channel_gain(ch.volume) * f64::from(adjust.mult) } else { 0.0 };
                // Stretching (spec: audio/time-stretch.md): resample plays the sample faster or slower; the other methods
                // play a stretched copy, made once per library (the realtime ones one copy per key, made below). A
                // library that leaves stretching to the host's sampler plugin plays them by resampling instead, at the
                // stretched length.
                let played = sample.as_ref().map(|s| (s.frames(), s.rate)).or(length);
                let mut how = played
                    .filter(|_| plays_sample)
                    .map_or(Stretching::None, |(frames, r)| ch.sound.stretching(frames as f64 / f64::from(r.max(1)), tick_tempo));
                if library.resample_stretches {
                    if let Stretching::Whole { factor, .. } | Stretching::Realtime { factor, .. } = how {
                        how = Stretching::Resample { speed: 1.0 / factor };
                    }
                }
                let (mut sample, mut length, mut speed, mut from_scale, mut realtime) = (sample, length, 1.0, 1.0, None);
                match how {
                    Stretching::None => {}
                    Stretching::Resample { speed: s } => speed = s,
                    Stretching::Whole { factor, cents } => {
                        from_scale = factor;
                        sample = sample.map(|s| library.stretched(&s, stretched_frames(s.frames(), factor), 2f64.powf(cents / 1200.0)));
                        // The instrument plays it: the stretched copy's length is all the trims need.
                        length = length.map(|(frames, r)| (stretched_frames(frames, factor), r));
                    }
                    Stretching::Realtime { factor, cents } => {
                        from_scale = factor;
                        realtime = Some((factor, cents));
                    }
                }
                let pitched = ch.sound.channel_pitch_applies() && realtime.is_none();
                let src_ratio = played.map_or(1.0, |(_, r)| f64::from(r) / f64::from(rate)) * speed;
                let insert = usize::from(ch.mixer_track).min(n_tracks - 1);
                let knob = f64::from(ch.pitch_cents);
                let main = ch.misc.main_pitch.then(|| i32::from(project.main_pitch.unwrap_or(0)));
                let mut state = ChannelState {
                    sample,
                    length,
                    volume,
                    pan: ch.pan,
                    pan_law,
                    insert,
                    base_rate: src_ratio,
                    enabled: ch.enabled,
                    stored: (ch.enabled, insert),
                    src_ratio,
                    knob,
                    init_knob: knob,
                    bend: 0.0,
                    fine: ch.misc.fine,
                    root: ch.misc.root,
                    main,
                    init_main: main,
                    pitched,
                    init: (volume, ch.pan, src_ratio),
                    vol_v: ch.volume,
                    init_v: ch.volume,
                    serial: 0,
                    ramp: None,
                    vol_env: env_on.then(|| envelope::Params::new(&ch.envelopes[os_model::Envelope::VOLUME])).flatten(),
                    pitch_env: env_on.then(|| envelope::Params::new(&ch.envelopes[os_model::Envelope::PITCH])).flatten(),
                    frame_s: 1.0 / f64::from(rate),
                    plugin: ch.kind == ChannelKind::Plugin,
                    smart: ch.misc.cpu.smart_disable,
                    own_tuning: ch.kind.is_sampler(),
                    instrument: Slot::default(),
                    rules: notes::Rules::new(&ch.misc, ppq),
                    from_scale,
                    realtime,
                    keyed: Vec::new(),
                    clip_level: if ch.kind == ChannelKind::AudioClip { os_dsp::level::audio_clip_level() } else { 1.0 },
                    adjust,
                    init_adjust: adjust,
                    layer: (ch.kind == ChannelKind::Layer).then(|| layers::Layer::of(ch)),
                    voicing: crate::instrument::ChannelVoicing::of(ch, ppq),
                    via: None,
                };
                state.retune(knob);
                state.init.2 = state.base_rate;
                // What the note rules need to know of the channel's voices (spec: audio/note-rules.md).
                let played = state.played();
                let r = &mut state.rules;
                r.plugin = ch.kind == ChannelKind::Plugin;
                r.one_shot = ch.kind.is_sampler() && state.vol_env.is_none() && !looped;
                r.root = ch.misc.root;
                r.close = notes::closing_frames(ch.sound.declick, rate);
                r.ring = match (played, r.plugin) {
                    _ if looped => f64::INFINITY,
                    (Some((frames, _)), false) if frames > 0 => frames as f64 / state.src_ratio.max(1e-9) * state.realtime.map_or(1.0, |t| t.0.max(1e-9)),
                    _ => f64::INFINITY,
                };
                if looped {
                    let release = state.vol_env.as_ref().map_or(0.0, |e| e.release_seconds() * f64::from(rate));
                    r.tail = release + f64::from(r.close);
                }
                state
            })
            .collect();

        let tracks: Vec<MixTrack> = if project.mixer.is_empty() {
            vec![MixTrack { eq: os_dsp::mixer::TrackEq::new(os_dsp::mixer::EQ_DEFAULT, rate), law: pan_law, output: 0, ..MixTrack::unity() }]
        } else {
            project
                .mixer
                .iter()
                .map(|m| {
                    let mut effects = Vec::new();
                    for (k, slot) in m.slots.iter().enumerate() {
                        let Some(slot) = slot else { continue };
                        match crate::effects::from_slot(&slot.name, &slot.state, rate) {
                            Some(mut fx) => {
                                fx.set_tempo(tempo);
                                effects.push(FxSlot::new(k, m.slot_on[k], mix_level(m.slot_mix[k]), m.slot_cpu[k].smart_disable, Fx::Own(fx)));
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
                                let gain = level.map_or(1.0, |l| os_dsp::mixer::send_gain((l * 16000.0).round() as i32));
                                Route { dest: usize::from(r), gain, glide: Glide::new(gain), pad: pdc::Pad::default() }
                            })
                            .collect()
                    };
                    let mut t = MixTrack {
                        vol: (m.volume.unwrap_or(0.8) * 16000.0).round() as i32,
                        pan: (m.pan.unwrap_or(0.0) * 6400.0).round() as i32,
                        sep: (m.stereo_separation.unwrap_or(0.0) * 64.0).round() as i32,
                        on: m.enabled,
                        law: pan_law,
                        swap: m.swap,
                        polarity: m.polarity,
                        slots_enabled: m.slots_enabled,
                        threaded: m.threaded,
                        eq: os_dsp::mixer::TrackEq::new(m.eq, rate),
                        effects,
                        slot_on: m.slot_on,
                        slot_mix: m.slot_mix.map(mix_level),
                        slot_smart: m.slot_cpu.map(|c| c.smart_disable),
                        routes,
                        latency: (m.latency_out, m.latency_in),
                        input: if m.monitored() { m.audio_in } else { -1 },
                        output: m.audio_out,
                        rec: rec::Arm::of(m),
                        ..MixTrack::unity()
                    };
                    // At the start of the song every gain is at its target.
                    t.matrix = t.compute_matrix();
                    t.gains = t.matrix.map(|r| r.map(Glide::new));
                    t.store();
                    t
                })
                .collect()
        };
        let order = processing_order(&tracks);

        let mut controls: Vec<CtlEvent> = schedule_controls(project, mode)
            .into_iter()
            .map(|(tick, kind, value)| CtlEvent { tick, to: ctl_to(kind), value })
            .filter(|e| !matches!(e.to, CtlTo::Channel(c, _) if c >= channels.len()))
            .collect();
        controls.sort_by_key(|c| c.tick); // stable: equal ticks keep their order
        let (mut sched, end_tick) = schedule(project, mode, &mut report);
        sched.sort_by_key(|s| s.tick);
        let mut triggers: Vec<Trigger> = sched
            .into_iter()
            .filter_map(|n: Sched| {
                let state = channels.get(n.channel)?;
                if state.played().is_none() && !state.plugin && state.layer.is_none() {
                    return None;
                }
                let (from, start, stop) = match state.played() {
                    // Trims are source time: in a stretched copy they come `from_scale` times later (spec:
                    // audio/time-stretch.md "Trims").
                    Some((frames, r)) if frames > 0 => {
                        let at = |s: f64| s * f64::from(r) * state.from_scale;
                        let played = frames as f64 * if state.realtime.is_some() { state.from_scale } else { 1.0 };
                        let from = at(n.from_s);
                        (from, (from / played) as f32, n.to_s.map_or(0.0, |s| (at(s) / played).max(1e-9) as f32))
                    }
                    _ => (0.0, 0.0, 0.0),
                };
                Some(Trigger {
                    tick: n.tick,
                    at: n.tick,
                    channel: n.channel,
                    // The note's fine pitch is the voice's pitch offset, kept apart from its key (spec: audio/voice-levels.md).
                    rate: laws::playback_rate(n.key, laws::fine_pitch_cents(n.fine)),
                    fine: laws::fine_pitch_cents(n.fine) as f32 / 100.0,
                    cut: n.cut,
                    // A playlist audio clip's voice has its own volume in place of a velocity: unity through a channel at
                    // its default volume, centred (spec: audio/audio-clips.md "Level").
                    vel: if n.cut.is_some() { f64::from(os_dsp::level::audio_clip_level()) } else { laws::velocity_gain(n.velocity) },
                    pan: laws::note_pan(n.pan),
                    key: n.key,
                    end: (n.len > 0).then_some(n.tick + n.len),
                    velocity: n.velocity,
                    levels: crate::NoteLevels::of_bytes(n.pan, n.mod_x, n.mod_y),
                    from,
                    start,
                    stop,
                    buf: 0,
                    shape: n.clip.map(|fx| {
                        let ms = |v: f32| (f64::from(v.max(0.0)) * f64::from(rate) / 1000.0).round() as u64;
                        let total = Clock::frame_of_tick(rate, ppq, tick_tempo, n.tick + n.len) - Clock::frame_of_tick(rate, ppq, tick_tempo, n.tick);
                        ClipShape {
                            since: 0,
                            total,
                            fade_in: ms(fx.fade_in.0),
                            fade_out: ms(fx.fade_out.0),
                            tension_in: f64::from(fx.fade_in.1),
                            tension_out: f64::from(fx.fade_out.1),
                            gain: fx.gain,
                            declick: 0,
                        }
                    }),
                    slide: n.slide,
                    colour: n.colour,
                    porta: n.porta,
                    via: None,
                    cuts: None,
                    swing: n.swing,
                })
            })
            .collect();

        // Realtime stretch methods: each key a channel's notes play gets the sample stretched at that pitch (the
        // channel's pitch with its fine tune, root and main pitch, and the stretch's own included), and its notes play
        // that copy as it is.
        for t in triggers.iter_mut() {
            if let Some(i) = keyed_copy(&mut channels[t.channel], library, t.key) {
                t.buf = (i + 1) as u16;
                t.rate = 1.0;
            }
        }
        // A layer's notes play on its children: they get the copies for the keys the layer plays.
        let layered: Vec<(usize, u16)> = triggers
            .iter()
            .filter_map(|t| channels[t.channel].layer.as_ref().map(|l| (l.children.clone(), t.key)))
            .flat_map(|(children, key)| children.into_iter().map(move |c| (c, key)))
            .collect();
        for (c, key) in layered {
            if let Some(ch) = channels.get_mut(c) {
                keyed_copy(ch, library, key);
            }
        }

        // Automation clips (song mode only): each plays its curve onto its target from its start to its end.
        let mut curves = Vec::new();
        let mut autos = Vec::new();
        if mode == PlayMode::Song {
            for c in project.arrangement().map(|a| a.clips.as_slice()).unwrap_or(&[]) {
                let ClipSource::Channel(ch) = c.source else { continue };
                let Some(a) = project.automation(ch) else { continue };
                if a.targets.is_empty() || !clip_audible(project, c) {
                    continue;
                }
                // A clip drives every target it is linked to; unknown targets are left out, and a clip without a known
                // one is ignored (spec: flp/events/automation.md "Event 227").
                let targets: Vec<_> = a
                    .targets
                    .iter()
                    .map(|&t| os_model::target::decode(t))
                    .filter(|t| !matches!(t, os_model::target::Target::Other(_)))
                    .collect();
                if targets.is_empty() {
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
                // The range comes from the clip channel's pan and volume (spec: audio/automation.md "Range").
                let (pan, volume) = project.channels.iter().find(|x| x.index == ch).map_or((-1.0, 1.0), |x| (x.pan, x.volume));
                autos.push(AutoClip {
                    start: u64::from(c.position),
                    end: u64::from(c.end()),
                    from_beat: c.window.map_or(0.0, |w| w.0),
                    curve,
                    targets,
                    walk: None,
                    out: os_model::clip_lfo::ClipOut::new(&a.lfo, pan, volume, ppq),
                    env: None,
                });
            }
        }
        // Later clips win where clips on the same target overlap.
        autos.sort_by_key(|a| a.start);
        // The initial values, in song mode. Where playback starts the app has already set them in the project.
        let init = if mode == PlayMode::Song {
            project
                .init_controls
                .iter()
                .filter_map(|c| Some((os_model::target::decode(c.target), (c.unit()? * 1_073_741_824.0).round_ties_even() as i64)))
                .collect()
        } else {
            Vec::new()
        };

        let main_volume = project.main_volume.unwrap_or(os_model::Project::MAIN_VOLUME_DEFAULT);
        let main_gain = os_dsp::level::curve(f32::from(main_volume) * os_dsp::level::R12800);
        // The markers act on a song played from the playlist (an empty playlist plays a pattern).
        let marks = match project.arrangement().filter(|a| mode == PlayMode::Song && !a.clips.is_empty()) {
            Some(a) => marks::Marks::new(&a.markers),
            None => marks::Marks::default(),
        };
        let mut pdc = pdc::Pdc::new(channels.len(), tracks.iter().map(|t| t.slot_on));
        pdc.manual = project.pdc_off;
        Engine {
            main_gain,
            main_glide: os_dsp::glide::Glide::new(main_gain),
            exporting: false,
            params: param_auto::ParamAuto::new(autos.iter().flat_map(|a| &a.targets).any(|&t| param_auto::is_param(t))),
            // Seeded, one output taken, at every start of playback or of a render (spec: audio/channel-misc.md "FL's
            // random generator"; whether FL's live playback draws as its render does is not covered, Lemon Studio
            // does).
            rng: layers::Mt19937::for_playback(),
            autos,
            curves,
            auto_last: Vec::new(),
            auto_from: 0,
            ctl_links: controllers::links(project),
            init,
            rate,
            channels,
            bus: vec![Vec::new(); tracks.len()],
            tracks,
            order,
            swing: project.swing.unwrap_or(0).min(128),
            swung: triggers.iter().any(|t| t.swing.is_some()),
            triggers,
            next_trigger: 0,
            late: Vec::new(),
            relaunch_clips: false,
            truncated_on_transport: false,
            controls,
            next_ctl: 0,
            sounding: Vec::with_capacity(256),
            voices: Vec::with_capacity(1024),
            clock: Clock::new(rate, ppq, tick_tempo),
            tick: 0,
            frame: 0,
            end_tick,
            looping: false,
            loop_range: None,
            live_only: false,
            awake: false,
            tail_done: false,
            held: false,
            count_in: 0,
            count_in_len: 0,
            nudge: None,
            nudged: 0,
            run_on: None,
            marks,
            landed: true,
            quantized_jump: None,
            punched: false,
            pause_at: None,
            marker_paused: None,
            spans: Vec::new(),
            fade_out: 0,
            fade_in: 0,
            fade_len: 0,
            tempo,
            ppq,
            steps: project.bar().1,
            signature: project.signature().unwrap_or_else(|| {
                let (beats, steps) = project.bar();
                (beats.saturating_mul(steps), 16)
            }),
            out: Vec::with_capacity(8192),
            out_pos: 0,
            out_tick: 0.0,
            out_frame: 0,
            out_held: false,
            out_len: 1,
            io: None,
            inst_offs: Vec::with_capacity(64),
            inst_shapes: Vec::new(),
            next_voice: 0,
            inst_live: Vec::with_capacity(64),
            inst_midi: Vec::with_capacity(64),
            inst_events: Vec::with_capacity(64),
            inst_tuning: Vec::new(),
            inst_notes: Vec::with_capacity(64),
            note_voices: vec![notes::Voices::default(); project.channels.len()],
            inst_owed: Vec::new(),
            inst_missed: Vec::new(),
            busy_until: None,
            busy_wait: BUSY_WAIT,
            missed: 0,
            key_out: Vec::new(),
            inst_tail: 0,
            inst_active: false,
            inst_held: Vec::new(),
            metronome: None,
            // Accent on every numerator-th beat (spec: ui/metronome.md §4): event 17, beats per bar.
            beats_per_accent: project.bar().0,
            click_bars: click_bars(project, mode),
            tap_clicks: crate::metronome::TapClicks::new(rate),
            sinc_w: SINC_W,
            insert_fx: true,
            master_fx: true,
            capture: None,
            record: None,
            pdc,
            main_unity: main_volume == os_model::Project::MAIN_VOLUME_DEFAULT,
            multicore: multicore::Multicore::new(crate::parallel::global()),
            report,
        }
    }

    /// Clicks with `switch` while playing live (spec: ui/metronome.md); an export never does.
    pub fn set_metronome(&mut self, switch: std::sync::Arc<crate::metronome::Switch>) {
        self.metronome = Some(crate::metronome::Metronome::new(switch, self.rate));
    }

    /// Count in `bars` bars of the time signature's beats before playback moves (spec: ui/recording.md §3): the
    /// position holds where it is, the metronome clicks every beat whether it is on or off, live notes and the effects
    /// sound. Needs `set_metronome` for the clicks.
    pub fn set_count_in(&mut self, bars: u32) {
        let bar = u64::from(self.beat_ticks()) * u64::from(self.beats_per_accent.max(1));
        self.count_in = bar * u64::from(bars);
        self.count_in_len = self.count_in;
        self.tail_done = false;
    }

    /// While `flag` is set, a looping playback runs on past its end (spec: ui/recording.md §4); it is read every tick.
    pub fn set_run_on(&mut self, flag: std::sync::Arc<std::sync::atomic::AtomicBool>) {
        self.run_on = Some(flag);
    }

    /// The Tempo tapper's nudge buttons (spec: ui/tempo-tapper.md §6): while `flag` is -1 (+1), playback runs 2 % slower
    /// (faster) from the next tick on; the tempo itself stays. An export never nudges.
    pub fn set_nudge(&mut self, flag: std::sync::Arc<std::sync::atomic::AtomicI8>) {
        self.nudge = Some(flag);
    }

    /// Ticks of the count-in left to render (0: none, or over).
    pub fn counting_in(&self) -> u64 {
        self.count_in
    }

    /// The mixer track the metronome's clicks and the Tempo tapper's enter (spec: ui/metronome.md §4): the switch's
    /// track when the project has that insert track (not the master, not the hidden last track), else the master.
    fn click_track(&self) -> usize {
        let t = self.metronome.as_ref().map_or(0, |m| usize::from(m.switch().track()));
        if t + 1 < self.bus.len() { t } else { 0 }
    }

    fn beat_ticks(&self) -> u32 {
        (u32::from(self.ppq) * u32::from(self.steps) / 4).max(1)
    }

    /// A click from the next tick, for a tap on the Tempo tapper (spec: ui/tempo-tapper.md §8): the metronome's Tick,
    /// accented or not, at the metronome's volume `db`, into the master track like the metronome's clicks; also on a
    /// stream of live notes.
    pub fn tap_click(&mut self, accent: bool, db: i8) {
        self.tap_clicks.start(accent, db);
        self.tail_done = false;
    }

    /// Plays generator-plugin channel `channel` through `inst` (a shared instance that outlives this engine).
    /// Plugin channels, and sampler channels played by a hosted sampler plugin (Lemon Studio hosts Lemon Sampler for every
    /// sampler channel): the channel's notes go to `inst` instead of the engine's own sample voices.
    pub fn set_instrument(&mut self, channel: usize, inst: SharedInstrument) {
        if let Some(ch) = self.channels.get_mut(channel) {
            if ch.plugin && ch.instrument.0.is_none() {
                self.report.plugin_channels_silent = self.report.plugin_channels_silent.saturating_sub(1);
            }
            ch.plugin = true;
            if let Ok(h) = inst.try_lock() {
                let mine = |g: Target| matches!(g, Target::ChannelParam(c, _) if usize::from(c) == channel);
                param_points(&mut self.controls, mine, |k| h.param_range(k));
            }
            ch.instrument = Slot(Some(inst));
            // Its delay is read after it processes; the pads are worked out again then.
            self.pdc.due = true;
            // A new instance owes nothing its predecessor missed.
            if let Some(o) = self.inst_owed.get_mut(channel) {
                *o = 0;
            }
            self.inst_missed.retain(|m| m.0 != channel);
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
        if let Ok(h) = fx.try_lock() {
            let mine = |g: Target| matches!(g, Target::SlotParam(a, b, _) if usize::from(a) == track && usize::from(b) == slot);
            param_points(&mut self.controls, mine, |k| h.param_range_fx(k));
        }
        t.effects.insert(at, FxSlot::new(slot, t.slot_on[slot], t.slot_mix[slot], t.slot_smart[slot], Fx::Hosted(fx)));
        self.pdc.due = true;
    }

    /// The input ports of the hosted plugin in `slot` of mixer track `track`: `main` for its main input bus, `aux` for
    /// its first auxiliary one, each the number of a track's input counted from 1, or 0 for none (spec:
    /// audio/sidechain.md "Hosted plugins' input buses").
    pub fn set_effect_ports(&mut self, track: usize, slot: usize, main: usize, aux: usize) {
        let Some(fx) = self.tracks.get_mut(track).and_then(|t| t.effects.iter_mut().find(|e| e.slot == slot)) else { return };
        if let Fx::Hosted(_) = fx.fx {
            if fx.ports != [main, aux] {
                fx.ports = [main, aux];
                self.pdc.due = true;
            }
        }
    }

    /// Ticks hosted plugins missed since the last call (each one still busy elsewhere after the tick's wait), heard as
    /// a gap on its channel or mixer track.
    pub fn take_missed(&mut self) -> u64 {
        std::mem::take(&mut self.missed)
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
    /// A controller message for `channel` (spec: ui/midi-settings.md §4): an instrument channel's plugin gets it on the
    /// next tick as it came; a channel without one takes pitch bend (0xE0) as a bend of ±`bend_cents` from the centre
    /// on top of its pitch, and ignores the rest.
    pub fn midi(&mut self, channel: usize, bytes: [u8; 3], bend_cents: f64) {
        let Some(ch) = self.channels.get_mut(channel) else { return };
        if ch.instrument.0.is_some() {
            self.tail_done = false;
            self.inst_midi.push((channel, bytes));
            return;
        }
        if bytes[0] & 0xF0 == 0xE0 {
            let v = i32::from(bytes[1] & 0x7F) | i32::from(bytes[2] & 0x7F) << 7;
            ch.bend = f64::from(v - 8192) / if v >= 8192 { 8191.0 } else { 8192.0 } * bend_cents;
            ch.retune(ch.knob);
        }
    }

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
        self.inst_live.push((channel, k, f32::from(velocity) / 128.0, 0));
        self.inst_offs.push((self.tick + ticks, channel, k, None, None));
        self.tail_done = false;
    }

    /// Loop the scheduled material: on reaching its end, playback continues from tick 0 (ringing voices carry on).
    /// Used for pattern playback.
    /// The sampler channels' interpolation: a windowed sinc `points` source samples wide (27: playback's), 2 or less:
    /// linear (spec: audio/export.md §3 "Resampling").
    pub fn set_interpolation(&mut self, points: u32) {
        self.sinc_w = f64::from(points) / 2.0;
    }

    /// Renders on `pool`'s cores whatever the program's switches say (`None`: on the rendering thread alone). An engine
    /// starts with the program's pool (`parallel::global`), used while its switches are on (spec: audio/multicore.md).
    pub fn set_threads(&mut self, pool: Option<std::sync::Arc<crate::parallel::Pool>>) {
        self.multicore.set_pool(pool, false);
    }

    /// Whether the last tick passed the effect in slot `slot` of mixer track `track` by, silent with Auto-bypass when
    /// silent on (spec: audio/auto-bypass.md).
    pub fn slot_bypassed(&self, track: usize, slot: usize) -> bool {
        self.tracks.get(track).is_some_and(|t| t.effects.iter().any(|e| e.slot == slot && e.skipped))
    }

    /// Runs the insert tracks' effect slots (`insert`) and the master's (`master`), or skips them.
    pub fn set_effects(&mut self, insert: bool, master: bool) {
        (self.insert_fx, self.master_fx) = (insert, master);
    }

    /// Plays on a device with `pairs` stereo output pairs (at least one) and, for the tracks that monitor one, its
    /// inputs: from then on a tick hands out `2 × pairs` interleaved channels, the master on its output pair and each
    /// track's direct output added to its own (spec: audio/track-io.md §2.2). Set before the first tick.
    pub fn set_device(&mut self, pairs: usize, input: Option<Box<dyn DeviceInput>>) {
        self.io = Some(DeviceIo { input, pairs: pairs.max(1), inputs: Vec::new(), direct: Vec::new() });
    }

    /// Samples in a frame of what the engine hands out: 2, or 2 × the device's output pairs (`set_device`).
    pub fn width(&self) -> usize {
        self.io.as_ref().map_or(2, |io| 2 * io.pairs)
    }

    /// Hands the armed tracks' takes to `tap` from now on, tick after tick while the song plays (not in a count-in or
    /// while paused), each taken where its record source says (spec: audio/track-io.md §2.5). `None` stops it.
    pub fn set_record_tap(&mut self, tap: Option<std::sync::Arc<dyn RecordTap>>) {
        self.record = tap;
    }

    /// Keeps every mixer track's post-fader output (before its sends) from now on: `take_track_audio` hands it over.
    pub fn set_track_capture(&mut self, on: bool) {
        self.capture = on.then(|| vec![Vec::new(); self.tracks.len()]);
        self.pdc.taken = self.pdc.handed;
    }

    /// The tracks' output kept since `set_track_capture` (interleaved L/R per track, master first), emptied: as much of
    /// each as has been handed out of the song since. With delay compensation each track skipped its own lateness at
    /// the start (a track less late than the song is ahead by the difference: what it has beyond is kept for the next
    /// call).
    pub fn take_track_audio(&mut self) -> Vec<Vec<f32>> {
        let n = 2 * (self.pdc.handed - self.pdc.taken) as usize;
        self.pdc.taken = self.pdc.handed;
        self.capture.as_mut().map(|c| c.iter_mut().map(|t| t.drain(..n.min(t.len())).collect()).collect()).unwrap_or_default()
    }

    /// Renders ticks `from` to `to` only: playback starts at `from` (`seek_tick`) and the song ends at `to`.
    pub fn set_span(&mut self, from: u64, to: u64) {
        self.end_tick = to.max(from + 1);
        self.seek_tick(from);
    }

    /// A whole-song render follows the time markers (spec: audio/song-structure.md "Time markers"): it starts at the
    /// Start marker, with the tick clock at 0 there, and at a skip marker goes on at the next marker with the clock
    /// running on; a skip whose goal is at or past the song's end ends the song there. Call it before rendering, in
    /// place of `seek_tick` / `set_span`.
    pub fn follow_markers(&mut self) {
        if self.marks.is_empty() || self.end_tick == 0 {
            return;
        }
        let end = self.end_tick;
        let start = self.marks.start().filter(|&s| s < end).unwrap_or(0);
        let spans = self.marks.rendered(start, end);
        if start > 0 {
            self.seek_tick(start);
            self.clock.restart();
            self.frame = 0;
            self.out_frame = 0;
        }
        self.end_tick = spans.last().map_or(end, |s| s.1);
        // A song that ends at a skip starts nothing after it (its tail still rings out).
        if self.end_tick < end {
            let n = self.triggers.partition_point(|t| t.tick < self.end_tick);
            self.triggers.truncate(n);
            self.next_trigger = self.next_trigger.min(n);
        }
        self.spans = spans;
    }

    /// The marker playback has just come to hold on (a Marker pause), once: the transport shows it as paused.
    pub fn take_marker_pause(&mut self) -> Option<u64> {
        self.marker_paused.take()
    }

    pub fn set_looping(&mut self, on: bool) {
        self.looping = on;
    }

    /// A looping song's loop points (the playlist's time selection, `None` or empty = the whole song): reaching the
    /// end, playback continues from the start. Played from outside the range, it gets there at the range's end, or
    /// at the song's end when it started past the range.
    pub fn set_loop_range(&mut self, range: Option<(u64, u64)>) {
        self.loop_range = range.filter(|&(a, b)| b > a);
    }

    /// A song jump quantized to `unit` ticks (the playlist's Quantize song jumps): playback goes on to the next tick
    /// that is a multiple of `unit` (this one if it is), then jumps to `to`, the way a marker jump lands. A later call
    /// replaces one still waiting.
    pub fn jump_on(&mut self, unit: u64, to: u64) {
        self.quantized_jump = Some((unit.max(1), to));
    }

    /// "Play truncated notes on transport" (Settings > Audio > Mixer; spec: ui/settings-choices.md "Transport: Play
    /// truncated notes and Reset plugins"): on, `seek_tick` starts the notes that are under way at the new place
    /// (`notes_under_way`); off (the default), they are skipped and their channels stay silent until their next note.
    /// Set it before seeking.
    pub fn set_truncated_on_transport(&mut self, on: bool) {
        self.truncated_on_transport = on;
    }

    /// Drop the scheduled notes: the engine only plays live notes (`note_on`).
    pub fn clear_schedule(&mut self) {
        self.triggers.clear();
        self.next_trigger = 0;
        self.late.clear();
        self.end_tick = 0;
        self.live_only = true;
        self.marks = marks::Marks::default();
    }

    /// Pause in place (spec: audio/transport.md "Pause"): the sampler channels (voices, audio clips) fade out of the
    /// mixer over about 6 ms and are then held exactly where they are, with the song position, while the mixer's
    /// effects run on with nothing coming in, so a reverb's or a delay's tail rings out. The hosted instruments get a
    /// note off for every note they hold once the fade is over and go on being processed, so their releases and their
    /// own effects ring out too, as in FL (spec: audio/transport.md "Tails after Pause and Stop").
    pub fn pause(&mut self) {
        if self.fade_out > 0 || self.held {
            return;
        }
        self.fade_len = ((f64::from(self.rate) * PAUSE_FADE_SECONDS).round() as u32).max(1);
        // Out from where a fade in has got to.
        self.fade_out = self.fade_len - self.fade_in.min(self.fade_len);
        self.fade_in = 0;
    }

    /// Resume after `pause`: the channels go on from where they were held, fading back in.
    pub fn resume(&mut self) {
        if !self.held && self.fade_out == 0 {
            return;
        }
        self.fade_len = self.fade_len.max(1);
        self.fade_in = self.fade_len - self.fade_out.min(self.fade_len);
        self.fade_out = 0;
        self.held = false;
        if std::mem::take(&mut self.relaunch_clips) {
            let channels = &self.channels;
            let clips = self.clips_under_way(self.tick).into_iter().filter(|t| channels.get(t.channel).is_some_and(|c| c.instrument.0.is_some()));
            self.late.extend(clips);
        }
    }

    /// Paused: the channels are held (or fading out to be).
    pub fn paused(&self) -> bool {
        self.held || self.fade_out > 0
    }

    /// Paused, and the channels' fade is over: nothing but the hosted instruments and the mixer is processed
    /// (`render_held`).
    pub fn held(&self) -> bool {
        self.held
    }

    /// Keep a live-only engine running while nothing plays (an instrument's editor is open): its instruments are
    /// processed every tick, so notes from the editor's own keyboard sound. Off, it finishes again once silent.
    pub fn set_awake(&mut self, on: bool) {
        self.awake = on;
    }

    /// Note offs now for every note held at the hosted instruments, which this engine then forgets (they do not
    /// sound again when it plays on). For a paused playback while another stream processes the same instruments (live
    /// notes, an open editor): its held notes would ring there for as long as it is paused.
    pub fn release_instruments(&mut self) {
        // A note an instrument missed while busy would start after its note off: it never starts.
        self.inst_missed.retain(|m| !matches!(m.1, notes::Note::On { .. }));
        self.send_held_offs();
        self.relaunch_clips |= self.inst_offs.iter().any(|o| o.3.is_some());
        self.inst_offs.clear();
        let channels = &self.channels;
        self.sounding.retain(|&(c, _, _)| channels.get(c).is_none_or(|ch| ch.instrument.0.is_none()));
        for v in &mut self.note_voices {
            *v = notes::Voices::default();
        }
    }

    /// A stopped playback that rings out its effects' tails (its channels held, `pause`): the voices it holds will
    /// never sound again; they go, so they are not counted.
    pub fn forget_voices(&mut self) {
        self.voices.clear();
        self.sounding.clear();
    }

    /// Start a live note on channel `channel` (rack position) at the next tick; it rings until
    /// `note_off` (or the sample ends). Velocity 0..=128, 100 = unity.
    pub fn note_on(&mut self, channel: usize, key: u16, velocity: u8) {
        self.note_on_midi(channel, key, velocity, 0);
    }

    /// `note_on` on MIDI channel `midi` (0..15): an instrument channel's plugin gets the note, and its end, on it (MIDI
    /// channel through, spec: ui/channel-rack.md §18); other channels ignore it.
    pub fn note_on_midi(&mut self, channel: usize, key: u16, velocity: u8, midi: u8) {
        let Some(ch) = self.channels.get(channel) else { return };
        self.tail_done = false;
        if ch.instrument.0.is_some() {
            self.inst_live.push((channel, key.min(127) as u8, f32::from(velocity) / 128.0, midi & 15));
            self.sounding.push((channel, key, u64::MAX));
            return;
        }
        if ch.sample.is_none() {
            return;
        }
        let (vel, pan) = (laws::velocity_gain(velocity), 0.0);
        let gain = ch.start_gain(vel, pan);
        let serial = ch.serial;
        // A realtime stretch copy made for this key plays as it is.
        let (buf, step) = match ch.keyed.iter().position(|k| k.0 == key) {
            Some(i) => ((i + 1) as u16, 1.0),
            None => (0, laws::playback_rate(key, 0)),
        };
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
            shape: None,
            buf,
            key,
            slide: VoiceSlide::new(step, velocity, pan),
            colour: 0,
            via: None,
            origin: None,
        });
        self.sounding.push((channel, key, u64::MAX));
    }

    /// Release a live note with a short fade.
    pub fn note_off(&mut self, channel: usize, key: u16) {
        if self.has_instrument(channel) && self.sounding.iter().any(|&(c, k, end)| c == channel && k == key && end == u64::MAX) {
            self.inst_live.push((channel, key.min(127) as u8, -1.0, 0));
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
    /// Paused, it stays where the channels are held.
    pub fn position(&self) -> u64 {
        if self.out_held {
            return self.out_frame;
        }
        self.out_frame + (self.out_pos / self.width()) as u64
    }

    /// The position being heard in ticks, with the fraction of the tick handed out so far.
    pub fn tick_position(&self) -> f64 {
        if self.out.is_empty() {
            return self.tick as f64;
        }
        if self.out_held {
            return self.out_tick;
        }
        self.out_tick + (self.out_pos / self.width()) as f64 / self.out_len as f64
    }

    /// The song's end in frames at the project tempo (voices may ring past it).
    pub fn length(&self) -> u64 {
        Clock::frame_of_tick(self.rate, self.ppq, self.tick_tempo(), self.end_tick)
    }

    /// Frames from where playback starts (`seek_tick`, `set_span`, `follow_markers`) to the song's end (voices may ring
    /// past it); a render that follows the markers counts the ticks it plays.
    pub fn frames_left(&self) -> u64 {
        if self.spans.is_empty() {
            return self.length().saturating_sub(self.frame);
        }
        let ticks: u64 = self.spans.iter().enumerate().map(|(i, &(a, b))| if i == 0 { b.saturating_sub(self.tick.max(a)) } else { b - a }).sum();
        let mut c = self.clock;
        let from = c.next_start();
        for _ in 0..ticks {
            c.advance();
        }
        c.next_start() - from
    }

    /// The tempo the ticks run at, in quarter notes a minute (`Project::tick_tempo`).
    fn tick_tempo(&self) -> f64 {
        self.tempo * f64::from(self.steps) / 4.0
    }

    /// The tempo the clock runs the ticks at: `tick_tempo` with the nudge.
    fn clock_tempo(&self) -> f64 {
        self.tick_tempo() * (1.0 + NUDGE * f64::from(self.nudged))
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
    /// starts again with the next note; one kept awake (`set_awake`) is never over.
    pub fn finished(&self) -> bool {
        let drained = self.out_pos >= self.out.len();
        if self.live_only {
            if self.awake {
                return false;
            }
            // Quiet, not exact zeros: an effect's tail may never reach zero.
            let rest_silent = self.out[self.out_pos.min(self.out.len())..].iter().all(|&s| s.abs() < QUIET);
            return rest_silent
                && self.voices.is_empty()
                && self.inst_tail == 0
                && self.inst_live.is_empty()
                && self.inst_midi.is_empty()
                && !self.tap_clicks.sounding()
                && self.pdc.drain == 0;
        }
        // With delay compensation as many frames as the head skipped follow the song's end (the file keeps its length).
        let flushed = self.pdc.flush.is_none_or(|f| f == 0);
        !self.looping && !self.paused() && drained && self.tail_done && flushed && self.inst_live.is_empty() && !self.tap_clicks.sounding()
    }

    /// Start playback in the tick that contains `frame` (at the project tempo; notes before that tick as `seek_tick`
    /// has them).
    pub fn seek(&mut self, frame: u64) {
        let (tick, _) = Clock::tick_of_frame(self.rate, self.ppq, self.tick_tempo(), frame);
        self.seek_tick(tick);
    }

    /// Start playback at tick `tick`: control points before it are applied, audio clips that are under way start on it
    /// from where their sample has got to. Notes before it are not triggered, except, with "Play truncated notes on
    /// transport" on (`set_truncated_on_transport`), those still going at it: they start afresh on it.
    pub fn seek_tick(&mut self, tick: u64) {
        self.clock = Clock::at_tick(self.rate, self.ppq, self.tick_tempo(), tick);
        self.tick = tick;
        self.landed = true;
        self.pause_at = None;
        self.frame = self.clock.next_start();
        self.next_trigger = self.triggers.partition_point(|t| t.tick < tick);
        for t in &mut self.tracks {
            t.restore(true);
        }
        self.sync_controls(tick);
        self.late = self.clips_under_way(tick);
        if self.truncated_on_transport {
            let notes = self.notes_under_way(tick);
            self.late.extend(notes);
        }
        self.params.forget();
        self.out.clear();
        self.out_pos = 0;
        self.out_tick = tick as f64;
        self.out_frame = self.frame;
        self.out_held = false;
        self.tail_done = false;
        // Delay compensation starts afresh: empty pads, the head of the stream skipped again (spec:
        // audio/delay-compensation.md "Lemon Studio").
        self.pdc.restart();
        for t in &mut self.tracks {
            t.pad.clear();
            for r in &mut t.routes {
                r.pad.clear();
            }
            for fx in &mut t.effects {
                fx.dry_pad.clear();
                fx.key_pad.clear();
                fx.main_pad.clear();
            }
        }
    }

    /// Audio clips that started before `tick` and are not over at it, moved to `tick` with their sample position
    /// advanced by the time since their start; a clip whose sample has run out by then stays silent (spec:
    /// audio/audio-clips.md, "Starting inside a clip"). Notes of pattern clips are not started late.
    fn clips_under_way(&self, tick: u64) -> Vec<Trigger> {
        let frame = |t: u64| Clock::frame_of_tick(self.rate, self.ppq, self.tick_tempo(), t);
        let at = frame(tick);
        self.triggers[..self.next_trigger]
            .iter()
            .filter(|t| t.cut.is_some_and(|c| c > tick))
            .filter_map(|t| {
                let ch = self.channels.get(t.channel)?;
                let frames = ch.frames(t.buf)? as f64;
                let since = at - frame(t.tick);
                let from = t.from + since as f64 * t.rate * ch.base_rate;
                let total = t.cut.map_or(u64::MAX, |c| frame(c) - frame(t.tick));
                let plain = ClipShape { since: 0, total, fade_in: 0, fade_out: 0, tension_in: 0.0, tension_out: 0.0, gain: 1.0, declick: 0 };
                let declick = (LATE_START_SECONDS * f64::from(self.rate)).round() as u64;
                let shape = Some(ClipShape { since, declick, ..t.shape.unwrap_or(plain) });
                (from < frames).then(|| Trigger { tick, from, start: (from / frames) as f32, shape, ..*t })
            })
            .collect()
    }

    /// Notes that started before `tick` and are still going at it (start < `tick` < end), moved to `tick` for what is
    /// left of their length: each sounds there as if it started there, afresh (a sampler voice from the sample's
    /// start, its envelopes from the top; an instrument gets a new note on), and ends at its own end (spec:
    /// ui/settings-choices.md "Transport: Play truncated notes and Reset plugins", on). Lemon Studio's rules: a step
    /// (no length of its own) never counts as going on, a slide note is not started again, a layer's note goes
    /// through the layer (`start_notes`), and in song mode the notes are those the clip-edge rule (event 30, "Play
    /// truncated notes in clips") has placed. The retriggers of a note repeat share the note's end: only the latest of
    /// them starts again (a Lemon Studio choice).
    fn notes_under_way(&self, tick: u64) -> Vec<Trigger> {
        let mut out: Vec<Trigger> = Vec::new();
        for t in &self.triggers[..self.next_trigger] {
            if t.cut.is_some() || t.slide || !t.end.is_some_and(|e| e > tick) {
                continue;
            }
            out.retain(|o| !(o.channel == t.channel && o.key == t.key && o.end == t.end));
            out.push(Trigger { tick, ..*t });
        }
        out
    }

    /// Applies the control points due on the tick starting now, smoothing volume and pan; a mixer track's values ramp
    /// over the tick's `len` frames as a clip's do (spec: audio/automation.md "Pattern control points").
    fn run_controls(&mut self, len: usize) {
        let len = if self.tick == self.auto_from { 0 } else { len.max(1) as u32 };
        while let Some(&e) = self.controls.get(self.next_ctl) {
            if e.tick > self.tick {
                break;
            }
            self.next_ctl += 1;
            self.apply_point(e, len, self.rate);
        }
    }

    /// One control point: a channel's setting, any other target's value as an automation level (the waits for late
    /// audio included), a plugin's parameter for this tick's parameter events. `rate` 0 sets a channel in place.
    fn apply_point(&mut self, e: CtlEvent, len: u32, rate: u32) {
        match e.to {
            CtlTo::Channel(c, param) => {
                if let Some(ch) = self.channels.get_mut(c) {
                    ch.apply(param, e.value, rate);
                }
            }
            // The tempo for the ticks after this one: the point's own tick keeps its length.
            CtlTo::Target(Target::Tempo) => {
                let bpm = f64::from(e.value) / 1000.0;
                if bpm > 0.0 && bpm != self.tempo {
                    self.tempo = bpm;
                    self.clock.set_tempo(self.rate, self.ppq, self.clock_tempo());
                }
            }
            CtlTo::Target(t) => {
                if let Some(v) = point_level(t, e.value) {
                    self.set_auto_values(vec![(t, (v * 1_073_741_824.0).round() as i64)], len);
                }
            }
            CtlTo::Param(t, Some(v)) => self.params.point(t, v),
            CtlTo::Param(_, None) => {}
        }
    }

    /// What the automation clips over before tick `tick` have left on their targets (spec: audio/automation.md "A new
    /// position"): the one that ends last decides, the latest of those that end together. A clip going on at `tick` hands
    /// on its own value on that tick.
    fn chase_automation(&mut self, tick: u64) {
        let mut last: Vec<(os_model::target::Target, usize)> = Vec::new();
        for (i, a) in self.autos.iter().enumerate().filter(|(_, a)| a.end <= tick && a.end > a.start) {
            for &t in &a.targets {
                match last.iter_mut().find(|l| l.0 == t) {
                    Some(l) if self.autos[l.1].end <= a.end => l.1 = i,
                    Some(_) => {}
                    None => last.push((t, i)),
                }
            }
        }
        let beat = os_model::curve::tick_step(self.ppq);
        for (t, i) in last {
            let a = &self.autos[i];
            let steps = a.end - a.start;
            let Some(level) = os_model::curve::value_at(&self.curves[a.curve].points, a.from_beat + steps as f64 * beat) else { continue };
            let env = a.out.envelope(&mut None, steps, self.ppq);
            let n = os_model::clip_lfo::handed_on(a.out.out(level as f32, steps, env));
            if param_auto::is_param(t) {
                self.params.point(t, n as f64 / 1_073_741_824.0);
            } else {
                self.auto_value(t, n, 0);
            }
        }
    }

    /// Channels as they are at tick `tick`: their own settings with every control point before it applied.
    fn sync_controls(&mut self, tick: u64) {
        // A hosted instrument's channel that automation had moved goes back to its own track first.
        let moved = self.channels.iter().any(|ch| ch.instrument.0.is_some() && ch.insert != ch.stored.1);
        for ch in &mut self.channels {
            ch.reset();
        }
        if moved {
            self.replan();
        }
        // Automation values are handed on again from here, in place.
        self.auto_last.clear();
        self.auto_from = tick;
        self.next_ctl = 0;
        self.params.points.clear();
        self.chase_automation(tick);
        while let Some(&e) = self.controls.get(self.next_ctl) {
            if e.tick >= tick {
                break;
            }
            self.next_ctl += 1;
            self.apply_point(e, 0, 0);
        }
        // The notes from here on take the swing in force.
        self.reswing();
    }

    /// Whether this engine uses the workers, as far as the load goes (`Multicore`): playback that restarts takes it on
    /// with `set_load_gate`, or each jump would start on one thread until the load shows again.
    pub fn load_gate(&self) -> LoadGate {
        self.multicore.gate()
    }

    pub fn set_load_gate(&mut self, gate: LoadGate) {
        self.multicore.set_gate(gate);
    }

    /// Take over playback from `old` (built from an earlier version of the project) at its position: the tick it
    /// already rendered plays out, ringing voices keep playing, notes before the position are not retriggered, and
    /// effect states carry over when the mixer layout is unchanged. Edits are heard from the next tick: ringing voices
    /// take the new channel volume / pan and mixer levels (ramped). A tempo change keeps the tick position and sizes
    /// the ticks after it.
    pub fn continue_from(&mut self, old: &mut Engine) {
        self.looping = old.looping;
        self.loop_range = old.loop_range;
        self.truncated_on_transport = old.truncated_on_transport;
        // The metronome carries on, with a click that is sounding (rebuilt at a new rate).
        self.metronome = old.metronome.take().map(|m| if m.rate() == self.rate { m } else { crate::metronome::Metronome::new(m.switch().clone(), self.rate) });
        // So does a tap's click (a tap usually changes the tempo while its click sounds).
        if old.tap_clicks.rate() == self.rate {
            std::mem::swap(&mut self.tap_clicks, &mut old.tap_clicks);
        }
        let tick = old.tick;
        self.clock = if self.rate == old.rate && self.ppq == old.ppq {
            let mut c = old.clock;
            c.set_tempo(self.rate, self.ppq, self.tick_tempo());
            c
        } else {
            Clock::at_tick(self.rate, self.ppq, self.tick_tempo(), tick)
        };
        self.tick = tick;
        self.frame = self.clock.next_start();
        // The device, with where its inputs have been read to.
        if old.io.is_some() {
            self.io = old.io.take();
        }
        if self.rate == old.rate {
            self.out = std::mem::take(&mut old.out);
            self.out_pos = old.out_pos;
            self.out_tick = old.out_tick;
            self.out_frame = old.out_frame;
            self.out_held = old.out_held;
            self.out_len = old.out_len;
            // Delay compensation goes on: no new head is skipped; the delays are read again on the first tick.
            self.pdc.take_over(&mut old.pdc);
        } else {
            (self.out_tick, self.out_frame) = (tick as f64, self.frame);
            self.pdc.fresh = false;
        }
        self.next_trigger = self.triggers.partition_point(|t| t.tick < tick);
        self.sync_controls(tick);
        let n = self.channels.len();
        // Clips moved to a seek tick that has not been rendered yet still start there.
        self.late = std::mem::take(&mut old.late);
        self.late.retain(|t| t.channel < n);
        self.voices = std::mem::take(&mut old.voices);
        self.voices.retain(|v| v.channel < n);
        // The random generator and whose turn it is on each layer go on; voices keep their layer while it is one.
        self.rng = old.rng.clone();
        let is_layer = |c: usize| self.channels.get(c).is_some_and(|ch| ch.layer.is_some());
        for v in &mut self.voices {
            v.via = v.via.filter(|w| is_layer(w.layer));
        }
        let vias: Vec<Option<Via>> = old.channels.iter().map(|o| o.via.filter(|w| is_layer(w.layer))).collect();
        for (new, (o, via)) in self.channels.iter_mut().zip(old.channels.iter().zip(vias)) {
            if let (Some(a), Some(b)) = (new.layer.as_mut(), o.layer.as_ref()) {
                a.turn = b.turn;
            }
            new.via = via;
        }
        // Voices and late clips playing a realtime stretch copy keep playing that copy.
        for i in 0..self.voices.len() {
            let (c, b) = (self.voices[i].channel, self.voices[i].buf);
            self.voices[i].buf = self.adopt(&old.channels, c, b);
        }
        for i in 0..self.late.len() {
            let (c, b) = (self.late[i].channel, self.late[i].buf);
            self.late[i].buf = self.adopt(&old.channels, c, b);
        }
        self.sounding = std::mem::take(&mut old.sounding);
        self.inst_live = std::mem::take(&mut old.inst_live);
        self.inst_midi = std::mem::take(&mut old.inst_midi);
        // A controller's pitch bend stays on its channel.
        for (new, o) in self.channels.iter_mut().zip(&old.channels) {
            if o.bend != 0.0 {
                new.bend = o.bend;
                new.retune(new.knob);
            }
        }
        // Notes held at the instruments (mono / polyphony / glide state) carry over.
        self.note_voices = std::mem::take(&mut old.note_voices);
        self.note_voices.resize(n, notes::Voices::default());
        self.inst_tail = old.inst_tail;
        self.awake = old.awake;
        (self.held, self.fade_out, self.fade_in, self.fade_len) = (old.held, old.fade_out, old.fade_in, old.fade_len);
        (self.count_in, self.count_in_len) = (old.count_in, old.count_in_len);
        self.run_on = old.run_on.take();
        self.record = old.record.take();
        (self.landed, self.punched, self.pause_at, self.marker_paused) = (old.landed, old.punched, old.pause_at, old.marker_paused.take());
        self.spans = std::mem::take(&mut old.spans);
        self.inst_offs = std::mem::take(&mut old.inst_offs);
        self.inst_offs.retain(|o| o.1 < n);
        self.inst_shapes = std::mem::take(&mut old.inst_shapes);
        self.inst_shapes.retain(|s| s.channel < n);
        // An edit while the song plays (spec: audio/transport.md "Edits while playing"): an audio clip still sounding
        // from a clip the new project no longer has where it was (deleted or moved) ends now, faded out as a live note
        // is; a hosted instrument's note or audio clip that the new project no longer has gets its note off or its
        // choke on the next tick. What is still there plays on as it was. An audio clip the old project did not have
        // there (added or moved) that is under way starts at once, inside the clip as playback started there would.
        if !self.live_only {
            let before: std::collections::HashSet<Origin> = old.triggers.iter().chain(&self.late).map(Trigger::origin).collect();
            let new: Vec<Trigger> = self.clips_under_way(tick).into_iter().filter(|t| !before.contains(&t.origin())).collect();
            self.late.extend(new);
            let kept: std::collections::HashSet<Origin> = self.triggers.iter().chain(&self.late).map(Trigger::origin).collect();
            let gone = |o: Option<Origin>| o.is_some_and(|o| !kept.contains(&o));
            let fade = ((f64::from(self.rate) * RELEASE_SECONDS) as u32).max(1);
            for v in &mut self.voices {
                if v.cut.is_some() && v.release.is_none() && gone(v.origin) {
                    v.release = Some((fade, fade));
                }
            }
            for o in &mut self.inst_offs {
                if gone(o.4) {
                    o.0 = o.0.min(tick);
                }
            }
        }
        // The voices playing on keep their names: new ones get others.
        self.next_voice = old.next_voice;
        // An instance that plays on still owes what it missed; a new one starts afresh.
        let same = |c: usize| match (self.channels.get(c).and_then(|ch| ch.instrument.0.as_ref()), old.channels.get(c).and_then(|ch| ch.instrument.0.as_ref())) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        };
        let owed: Vec<usize> = old.inst_owed.iter().enumerate().map(|(c, &o)| if same(c) { o } else { 0 }).collect();
        let mut missed = std::mem::take(&mut old.inst_missed);
        missed.retain(|m| same(m.0));
        self.inst_owed = owed;
        self.inst_missed = missed;
        // So does the tuning its notes were given: a changed channel pitch retunes them on the next tick.
        self.inst_tuning = old.inst_tuning.iter().enumerate().map(|(c, &t)| if same(c) { t } else { InstTuning::default() }).collect();
        self.missed = old.missed;
        // The main volume glides from where it was.
        self.main_glide = old.main_glide;
        self.exporting = old.exporting;
        self.multicore.keep_gate(&old.multicore);
        self.multicore.keep_gains(&old.multicore);
        if self.rate == old.rate && self.tracks.len() == old.tracks.len() {
            for (new, o) in self.tracks.iter_mut().zip(&mut old.tracks) {
                new.gains = o.gains;
                // The pads keep what they hold while the mixer is laid out as before.
                new.pad = std::mem::take(&mut o.pad);
                // Sends and slot mix levels glide on from where they were.
                if new.routes.len() == o.routes.len() && new.routes.iter().zip(&o.routes).all(|(a, b)| a.dest == b.dest) {
                    for (a, b) in new.routes.iter_mut().zip(&mut o.routes) {
                        a.glide = b.glide;
                        a.pad = std::mem::take(&mut b.pad);
                    }
                }
                // Own effects keep their state (delay lines, envelopes) when the slot layout is unchanged; the new
                // slot LEDs and mix levels apply.
                let same = new.effects.len() == o.effects.len() && new.effects.iter().zip(&o.effects).all(|(a, b)| a.slot == b.slot);
                if same {
                    for (a, b) in new.effects.iter_mut().zip(&mut o.effects) {
                        a.dry = b.dry;
                        a.quiet = b.quiet;
                        a.dry_pad = std::mem::take(&mut b.dry_pad);
                        a.key_pad = std::mem::take(&mut b.key_pad);
                        a.main_pad = std::mem::take(&mut b.main_pad);
                        if let (Fx::Own(_), Fx::Own(_)) = (&a.fx, &b.fx) {
                            std::mem::swap(&mut a.fx, &mut b.fx);
                        }
                    }
                }
            }
        }
    }

    /// The index (+ 1) in this engine's channel `c` of the copy `buf` that `old`'s channel `c` plays, added when missing.
    fn adopt(&mut self, old: &[ChannelState], c: usize, buf: u16) -> u16 {
        let Some(k) = (buf > 0).then(|| old.get(c).and_then(|o| o.keyed.get(usize::from(buf) - 1))).flatten().cloned() else { return 0 };
        let ch = &mut self.channels[c];
        let i = ch.keyed.iter().position(|x| Arc::ptr_eq(&x.1, &k.1)).unwrap_or_else(|| {
            ch.keyed.push(k);
            ch.keyed.len() - 1
        });
        (i + 1) as u16
    }

    /// The tempo in force (BPM): the project's, or what tempo automation last set (spec: audio/timing.md "Tempo that
    /// changes").
    pub fn tempo(&self) -> f64 {
        self.tempo
    }

    /// The engine's own voices sounding (sampler voices, previews; hosted instruments count theirs: spec
    /// audio/voices.md §1).
    pub fn voice_count(&self) -> usize {
        self.voices.len()
    }

    /// Notes playing at the current position, as (channel index, key).
    pub fn sounding(&self) -> impl Iterator<Item = (usize, u16)> + '_ {
        self.sounding.iter().map(|&(c, k, _)| (c, k))
    }

    /// Applies every automation clip that covers the tick starting now to its target: the curve's value one tick ahead,
    /// passed on only when it differs from the one last passed on (spec: audio/automation.md "When a clip acts").
    /// Volumes and pans glide over the tick (set in place on playback's first tick), the mixer ramps its matrix.
    /// Outside its clips a target keeps its last value.
    fn run_automation(&mut self, len: usize) {
        use os_model::target::Target;
        self.params.ticks.clear();
        self.params.point_ticks();
        if self.autos.is_empty() {
            return;
        }
        let now = self.tick;
        // Values held back for late audio whose tick has come, before this tick's own (spec:
        // audio/delay-compensation.md "Automation waits for late audio").
        if self.pdc.waiting.iter().any(|w| w.0 <= now) {
            let (due, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pdc.waiting).into_iter().partition(|w| w.0 <= now);
            self.pdc.waiting = rest;
            for (_, target, n) in due {
                self.auto_value(target, n, len.max(1) as u32);
            }
        }
        let b = os_model::curve::tick_step(self.ppq);
        let mut values: Vec<(Target, i64)> = Vec::new();
        for a in self.autos.iter_mut().rev() {
            // Where clips on one target overlap, the latest decides that target.
            if now < a.start || now >= a.end || a.targets.iter().all(|t| values.iter().any(|v| v.0 == *t)) {
                continue;
            }
            let pts = &self.curves[a.curve].points;
            // The value for tick `now` is the walk's after `now + 1 - start` tick steps (and the window's step first).
            let steps = now + 1 - a.start;
            let (mut walk, mut taken) = match a.walk {
                Some(w) if w.1 <= steps => w,
                _ => {
                    let Some(mut w) = os_model::curve::Walk::new(pts) else { continue };
                    if a.from_beat > 0.0 {
                        w.step(pts, a.from_beat);
                    }
                    (w, 0)
                }
            };
            while taken < steps {
                walk.step(pts, b);
                taken += 1;
            }
            a.walk = Some((walk, taken));
            // The level put into the clip's range and moved by its LFO, whose phase starts with the clip and whose
            // envelope is walked from the clip's start (spec: audio/automation.md "What a clip hands on", "The LFO's
            // value").
            let env = a.out.envelope(&mut a.env, steps, self.ppq);
            let n = os_model::clip_lfo::handed_on(a.out.out(walk.value(pts), steps, env));
            for &t in &a.targets {
                if !values.iter().any(|v| v.0 == t) {
                    values.push((t, n));
                }
            }
        }
        let len = if now == self.auto_from { 0 } else { len.max(1) as u32 };
        self.set_auto_values(values, len);
    }

    /// Passes automation values (2^30 = 1) on to their targets, each only when it differs from the one last passed on;
    /// `len`: the ticks volumes and pans glide over (0 = set in place). A value for a mixer track or slot whose audio
    /// is late waits the whole ticks that fit into the lateness; nothing waits on playback's first tick (spec:
    /// audio/delay-compensation.md "Automation waits for late audio").
    fn set_auto_values(&mut self, values: Vec<(os_model::target::Target, i64)>, len: u32) {
        for (target, n) in values {
            let wait = if len == 0 { 0 } else { self.wait_ticks(target) };
            if wait > 0 {
                self.pdc.waiting.push((self.tick + wait, target, n));
            } else {
                self.auto_value(target, n, len);
            }
        }
    }

    /// Passes automation value `n` on to `target` when it differs from the one last passed on (`set_auto_values`).
    fn auto_value(&mut self, target: os_model::target::Target, n: i64, len: u32) {
        use os_model::target::Target;
        // Plugin parameters: per-tick plugins get this value, the others the curve sampled through the tick
        // (`run_param_automation`).
        if param_auto::is_param(target) {
            self.params.tick_value(target, n);
            return;
        }
        match self.auto_last.iter_mut().find(|l| l.0 == target) {
            Some(l) if l.1 == n => return,
            Some(l) => l.1 = n,
            None => self.auto_last.push((target, n)),
        }
        let v = n as f64 / 1_073_741_824.0;
        let r12800 = (12800.0 * v).round_ties_even() as i32;
        match target {
            // spec: audio/voice-levels.md "Values from automation"
            Target::ChannelVolume(c) => {
                if let Some(ch) = self.channels.get_mut(usize::from(c)) {
                    ch.glide(Some(f64::from(os_dsp::level::knob(r12800, os_dsp::level::R12800))), None, len);
                }
            }
            Target::ChannelPan(c) => {
                if let Some(ch) = self.channels.get_mut(usize::from(c)) {
                    ch.glide(None, Some(f64::from(os_dsp::level::knob(r12800 - 6400, os_dsp::level::R6400))), len);
                }
            }
            Target::ChannelPitch(c) => {
                if let Some(ch) = self.channels.get_mut(usize::from(c)) {
                    ch.retune(f64::from(laws::auto_pitch_cents(v)));
                }
            }
            Target::ChannelLevel(c, id) => {
                if let Some(ch) = self.channels.get_mut(usize::from(c)) {
                    ch.automate_level(id, v);
                }
            }
            // Volume Round(16000 v), pan Round((2 v - 1) * 6400), separation Round((2 v - 1) * 64) (spec:
            // audio/mixer.md "Changes while the song plays"). At the start of playback the gains are at their
            // targets at once.
            Target::MixerVolume(t) | Target::MixerPan(t) | Target::MixerSeparation(t) => {
                if let Some(m) = self.tracks.get_mut(usize::from(t)) {
                    match target {
                        Target::MixerVolume(_) => m.vol = (16000.0 * v).round_ties_even() as i32,
                        Target::MixerPan(_) => m.pan = ((2.0 * v - 1.0) * 6400.0).round_ties_even() as i32,
                        _ => m.sep = ((2.0 * v - 1.0) * 64.0).round_ties_even() as i32,
                    }
                    m.matrix = m.compute_matrix();
                    if len == 0 {
                        m.gains = m.matrix.map(|r| r.map(Glide::new));
                    }
                }
            }
            // Gain Round((2 v - 1) * 1800), frequency and width Round(65536 v), taken at the tick start.
            Target::MixerEq(t, band, field) => {
                if let Some(m) = self.tracks.get_mut(usize::from(t)) {
                    let value = if field == 0 { ((2.0 * v - 1.0) * 1800.0).round_ties_even() } else { (65536.0 * v).round_ties_even() };
                    m.eq.set(usize::from(band), usize::from(field), value as i32);
                }
            }
            // A send's gain curve(single(v * 1.25)), gliding like the fader's gains. Only routes the project has
            // carry sound, so a value for a send without a route does nothing.
            Target::MixerSend(t, d) => {
                if let Some(route) = self.tracks.get_mut(usize::from(t)).and_then(|m| m.routes.iter_mut().find(|r| r.dest == usize::from(d))) {
                    route.gain = os_dsp::mixer::send_gain_auto(v);
                    if len == 0 {
                        route.glide = Glide::new(route.gain);
                    }
                }
            }
            // A slot's switch: on when Round(v) = 1 (half to even). An insert whose slots all go off, or one comes on
            // again, has its delay compensation worked out again with the next tick.
            Target::SlotOn(t, s) => {
                if let Some(m) = self.tracks.get_mut(usize::from(t)) {
                    m.set_slot_on(usize::from(s), v.round_ties_even() == 1.0);
                    self.pdc.switched = true;
                }
            }
            // A slot's mix level Round(12800 v); its dry level glides there.
            Target::SlotMix(t, s) => {
                if let Some(m) = self.tracks.get_mut(usize::from(t)) {
                    m.set_slot_mix(usize::from(s), r12800);
                    if len == 0 {
                        for fx in m.effects.iter_mut().filter(|f| f.slot == usize::from(s)) {
                            fx.dry = Glide::new(os_dsp::mixer::dry_level(fx.mix));
                        }
                    }
                }
            }
            // Plugin parameters: sampled through the tick by `run_param_automation`.
            // The tempo for the ticks after this one: BPM = (10000 + Round(single(v) * 512000)) / 1000 (spec:
            // audio/timing.md "Tempo that changes"; this tick's length was settled before).
            Target::Tempo => {
                let bpm = (10000.0 + (f64::from(v as f32) * 512000.0).round_ties_even()) / 1000.0;
                if bpm != self.tempo {
                    self.tempo = bpm;
                    self.clock.set_tempo(self.rate, self.ppq, self.clock_tempo());
                }
            }
            // The channel rack's swing knob, mixer box and LED (spec: flp/events/automation.md "Targets"): the main
            // swing `v × 128` to the nearest step, the mixer track `v × 500` to the nearest track up to the last insert,
            // the switch on from 0.5.
            Target::MainSwing => self.set_swing(os_model::target::swing_of(v)),
            Target::MainPitch => self.set_main_pitch(os_model::target::main_pitch_of(v)),
            Target::ChannelMixerTrack(c) => {
                let last = self.tracks.len().saturating_sub(2).max(1).min(self.tracks.len() - 1);
                self.route_channel(usize::from(c), usize::from(os_model::target::mixer_track_of(v, last as u16)));
            }
            Target::ChannelOn(c) => self.switch_channel(usize::from(c), os_model::target::channel_on(v)),
            Target::ChannelParam(..) | Target::SlotParam(..) | Target::Other(_) => {}
        }
    }

    /// The project's main pitch (cents) from this tick on (spec: audio/voice-levels.md "The main pitch"): every channel
    /// that follows it has its pitch worked out again and its sounding voices follow, as for a change of the channel's
    /// own pitch; a channel that does not follow it is not touched.
    fn set_main_pitch(&mut self, cents: i32) {
        for ch in self.channels.iter_mut().filter(|c| c.main.is_some_and(|m| m != cents)) {
            ch.main = Some(cents);
            ch.retune(ch.knob);
        }
    }

    /// The main swing in force from this tick on (spec: audio/swing.md, flp/events/automation.md "Targets"). How the
    /// original's notes follow a change in the middle of a pattern was not studied (audio/automation.md "Not covered");
    /// Lemon Studio's design: the notes not started yet are placed again with the new swing, each by its own position
    /// in its pattern as the song's layout places it, and one the change moves to before now starts now. Notes already
    /// started keep their place and length.
    fn set_swing(&mut self, swing: u8) {
        if swing != self.swing {
            self.swing = swing;
            self.reswing();
        }
    }

    /// Places the notes not started yet that the swing can move ([`Swung`]) with the swing in force, keeping the list
    /// in tick order: after a change of the swing, and where playback goes on at a new place (a loop, a jump, a seek).
    fn reswing(&mut self) {
        if !self.swung {
            return;
        }
        let (now, ppq, swing) = (self.tick, self.ppq, self.swing);
        let from = self.next_trigger.min(self.triggers.len());
        let mut moved = false;
        for t in &mut self.triggers[from..] {
            let Some(s) = t.swing.as_mut() else { continue };
            let delay = swing_delay(s.position, ppq, swing, s.share);
            let tick = (s.base + u64::from(delay)).max(now);
            if (delay, tick) != (s.delay, t.tick) {
                s.delay = delay;
                (t.tick, t.end) = (tick, s.end(tick));
                moved = true;
            }
        }
        if moved {
            // Stable: notes that start together keep their order.
            self.triggers[from..].sort_by_key(|t| t.tick);
        }
    }

    /// Channel `c` plays into mixer track `track` from this tick on (the rack's mixer box): its voices, those under way
    /// included, and its instrument's output go there. Lemon Studio's design, as for an edit of the box while the song
    /// plays (heard from the next tick, `continue_from`): the move is made at the tick's start and nothing crossfades;
    /// what the track left holds rings out there. A hosted instrument's channel has its delay compensation worked out
    /// again before the tick renders (spec: audio/delay-compensation.md).
    fn route_channel(&mut self, c: usize, track: usize) {
        let last = self.tracks.len() - 1;
        let Some(ch) = self.channels.get_mut(c) else { return };
        if ch.insert != track.min(last) {
            ch.insert = track.min(last);
            if ch.instrument.0.is_some() {
                self.replan();
            }
        }
    }

    /// Works the delay compensation out again now (on a pass's first tick `first_plan` does it).
    fn replan(&mut self) {
        self.pdc.due = true;
        if !self.pdc.first {
            self.plan_delays();
        }
    }

    /// Channel `c` switched on or off from this tick (the rack's LED; spec: audio/voice-levels.md "The other ids of a
    /// channel", the switch). Off, its sounding notes end on this tick as a note's end ends them (a sampler voice
    /// without the volume envelope plays on) and its level goes to 0, so nothing of it is heard; an audio clip under way
    /// is silenced with it, not ended. On again, its later notes play. A channel that is off starts no notes
    /// (`start_trigger`), so a note under way when it comes on stays silent (Lemon Studio's reading of the spec).
    fn switch_channel(&mut self, c: usize, on: bool) {
        let Some(ch) = self.channels.get_mut(c) else { return };
        if ch.enabled == on {
            return;
        }
        ch.enabled = on;
        ch.volume = ch.vol_gain(ch.vol_v);
        if on {
            return;
        }
        if ch.vol_env.is_some() {
            for v in self.voices.iter_mut().filter(|v| v.channel == c && v.cut.is_none() && v.live.is_none() && v.off.is_none()) {
                v.off = Some(v.age);
            }
        }
        // Its instrument's notes get their note offs now; its audio clips' voices play on.
        let events = &mut self.inst_events;
        self.inst_offs.retain(|&(_, ch, key, voice, _)| {
            if ch != c || voice.is_some() {
                return true;
            }
            events.push(notes::Event::off(ch, 0, key));
            false
        });
        self.sounding.retain(|s| s.0 != c || s.2 == u64::MAX);
    }

    /// Fills `out` with the next `out.len() / 2` stereo frames (interleaved L/R), rendering whole ticks as needed. Once
    /// playback is over the rest is silence.
    pub fn process(&mut self, out: &mut [f32]) {
        self.exporting = false;
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

    /// Renders the next whole tick for a stream that queues ticks ahead of its device (spec: audio/implementing.md
    /// "Playing live") and hands out its samples (interleaved L/R) with the tick and the frame it starts on, and
    /// whether it is held (paused: the position stays at that tick and frame all through it), and the frames a tick
    /// lasts there; `None` once playback is over (a live engine renders again after its next note). With delay
    /// compensation the samples are those heard now and the position where the song was when they were made (a tick's
    /// worth of samples can then span two ticks of the song).
    pub fn next_tick(&mut self) -> Option<(&[f32], f64, u64, bool, u64)> {
        self.exporting = false;
        // A tick wholly inside the head of the stream that delay compensation skips hands out nothing: on to the next.
        while self.out_pos >= self.out.len() {
            if self.finished() {
                return None;
            }
            self.render_tick();
        }
        let (from, w) = (self.out_pos, self.width());
        self.out_pos = self.out.len();
        let (tick, frame) = match self.out_held {
            true => (self.out_tick, self.out_frame),
            false => (self.out_tick + (from / w) as f64 / self.out_len as f64, self.out_frame + (from / w) as u64),
        };
        Some((&self.out[from..], tick, frame, self.out_held, self.out_len))
    }

    /// Takes a project edit in place (see [`LiveEdit`]), as a new engine for the edited project would have it.
    pub fn apply_edit(&mut self, edit: LiveEdit) {
        match edit {
            LiveEdit::ChannelVolume(c, v) => {
                if let Some(ch) = self.channels.get_mut(c) {
                    ch.glide(Some(v), None, 0);
                }
            }
            LiveEdit::ChannelPan(c, p) => {
                if let Some(ch) = self.channels.get_mut(c) {
                    ch.glide(None, Some(p), 0);
                }
            }
            LiveEdit::ChannelPitch(c, cents) => {
                if let Some(ch) = self.channels.get_mut(c) {
                    ch.init_knob = f64::from(cents);
                    ch.retune(f64::from(cents));
                }
            }
            LiveEdit::MixerVolume(t, v) | LiveEdit::MixerPan(t, v) | LiveEdit::MixerSeparation(t, v) => {
                if let Some(m) = self.tracks.get_mut(t) {
                    match edit {
                        LiveEdit::MixerVolume(..) => m.vol = (v * 16000.0).round() as i32,
                        LiveEdit::MixerPan(..) => m.pan = (v * 6400.0).round() as i32,
                        _ => m.sep = (v * 64.0).round() as i32,
                    }
                    m.matrix = m.compute_matrix();
                }
            }
        }
    }

    /// Renders the next tick into `self.out`, in the order of spec audio/timing.md ("What a tick does"): the clock
    /// moves; control points, then automation clips; notes that end are released, then notes that start are
    /// triggered; then the tick's samples are produced.
    fn render_tick(&mut self) {
        let t0 = std::time::Instant::now();
        self.render_tick_now();
        let length = self.out_len as f32 / self.rate.max(1) as f32;
        self.multicore.note_tick(t0.elapsed().as_secs_f32(), length, self.exporting);
    }

    fn render_tick_now(&mut self) {
        // Plugins that ask are told they are on the audio thread (CLAP's thread-check), here as on the workers.
        let _audio = crate::parallel::AudioScope::enter();
        if self.held {
            return self.render_held();
        }
        if self.count_in > 0 {
            return self.render_count_in();
        }
        // A render that follows the markers goes on at the next span's start, the tick clock running on.
        if self.spans.len() > 1 && self.tick == self.spans[0].1 {
            self.spans.remove(0);
            self.jump_to(self.spans[0].0, true);
        }
        // A quantized song jump waits for a tick on its unit; markers on the tick it lands on do not act.
        if let Some((unit, to)) = self.quantized_jump {
            if self.tick % unit == 0 && !self.landed {
                self.quantized_jump = None;
                self.jump_to(to, false);
                self.landed = true;
            }
        }
        // Time markers in live song playback (spec: audio/transport.md "Time markers in SONG mode"): they act on the
        // tick playback reaches by playing, not while the time selection loops.
        let landed = std::mem::replace(&mut self.landed, false);
        let marks_on = self.looping && !self.live_only && !self.marks.is_empty() && self.loop_range.is_none();
        if marks_on && !landed {
            match self.marks.live(self.tick, self.end_tick) {
                Some(marks::Act::Jump(to)) => self.jump_to(to, false),
                Some(marks::Act::PunchIn) => self.punched = true,
                // The hold is here already (the fade started early, below).
                Some(marks::Act::Pause) | None => {}
            }
        }
        // A Marker pause coming: the channels' fade starts so that it ends on the marker, where playback then holds.
        if marks_on && self.pause_at.is_none() && self.fade_out == 0 && self.fade_in == 0 {
            if let Some(p) = self.marks.pause_ahead(self.tick).filter(|&p| p < self.end_tick) {
                let fade = f64::from(self.rate) * PAUSE_FADE_SECONDS;
                if ((p - self.tick - 1) as f64) * self.clock.samples_per_tick() < fade {
                    let mut c = self.clock;
                    let frames: usize = (self.tick..p).map(|_| c.advance().1).sum();
                    self.fade_len = (frames as u32).max(1);
                    self.fade_out = self.fade_len;
                    self.pause_at = Some(p);
                }
            }
        }
        // A looping engine starts again at tick 0 when the pattern is over (a song: at its Loop or Start marker), or
        // at its loop points' start on reaching their end; voices and effects carry on. Recording with Loop record
        // off, or past a Start recording marker, it runs on.
        let run_on = self.run_on.as_ref().is_some_and(|f| f.load(std::sync::atomic::Ordering::Relaxed)) || self.punched;
        if self.looping && !run_on {
            match self.loop_range.filter(|_| !self.live_only) {
                Some((a, b)) if self.tick == b => self.jump_to(a, false),
                Some((a, b)) if self.end_tick > 0 && self.tick >= self.end_tick && self.tick > b => self.jump_to(a, false),
                None if self.end_tick > 0 && self.tick >= self.end_tick => self.jump_to(self.marks.end_target(self.end_tick), false),
                _ => {}
            }
        }
        let tick = self.tick;
        // The song already ended by the tail rule: only what delay compensation skipped at the head still follows.
        let was_done = self.tail_done;
        let (start, len) = self.clock.advance();
        let song = self.transport_at(start);
        self.busy_until = None;
        // A delay that changed, or an insert whose slots all went off, works the pads out again before this tick's
        // values arrive; on the first tick they are worked out after its slot values.
        self.check_delays();
        self.run_controls(len);
        self.run_automation(len);
        self.run_param_automation(len);
        self.run_controllers(len);
        for b in &mut self.bus {
            b.clear();
            b.resize(len * 2, 0.0);
        }
        if self.pdc.first {
            self.first_plan();
        }
        // One tick before the song's end every note still sounding is released (spec: audio/timing.md).
        let song_end = !self.looping && !self.live_only && self.end_tick > 0 && tick + 1 == self.end_tick;
        self.release_notes(tick, song_end);
        self.start_notes(tick);
        // The sampler voices play into their tracks and the instruments render (on several cores while the generator
        // switch is on; spec: audio/multicore.md); the instruments' outputs join the tracks after the clicks.
        self.render_generators(len, &song, true);
        // The metronome's click enters its mixer track (spec: ui/metronome.md §4): live playback only, and no new
        // click once a song is over.
        // Counted from the start of the time-signature region the tick is in, so the accent is on its bar lines.
        let (from, beat, beats) = self.click_bars.iter().rev().find(|r| r.0 <= tick).copied().unwrap_or((0, self.beat_ticks(), self.beats_per_accent));
        let click = self.click_track();
        if let Some(m) = self.metronome.as_mut().filter(|_| !self.exporting && !self.live_only) {
            let start = self.looping || tick < self.end_tick;
            m.tick(tick - from, beat, beats, start, &mut self.bus[click][..len * 2]);
        }
        if !self.exporting {
            self.tap_clicks.mix(&mut self.bus[click][..len * 2]);
        }
        self.pad_voices(len);
        // Pause / resume: the sampler channels fade out of (into) the mixer's inputs; the effects behind them run on.
        // The hosted instruments are left out: at the hold they are let go of their notes and ring on.
        if self.fade_out > 0 || self.fade_in > 0 {
            let total = self.fade_len as f32;
            let (left, out) = if self.fade_out > 0 { (self.fade_out as f32, true) } else { (self.fade_in as f32, false) };
            for b in &mut self.bus {
                for (f, fr) in b[..len * 2].chunks_exact_mut(2).enumerate() {
                    let down = ((left - f as f32) / total).clamp(0.0, 1.0);
                    let g = if out { down } else { 1.0 - down };
                    fr[0] *= g;
                    fr[1] *= g;
                }
            }
            if out {
                self.fade_out = self.fade_out.saturating_sub(len as u32);
                self.held = self.fade_out == 0;
                if self.held {
                    self.marker_paused = self.pause_at.take().or(self.marker_paused);
                    // Held from here: the hosted instruments are let go of what they hold and ring on.
                    self.release_instruments();
                }
            } else {
                self.fade_in = self.fade_in.saturating_sub(len as u32);
            }
        }
        self.instruments_out(len);
        for ch in &mut self.channels {
            ch.advance(len as u32);
        }
        self.tick = tick + 1;
        // A nudge pressed or let go changes the ticks after this one, as a tempo change does.
        let nudge = self.nudge.as_ref().filter(|_| !self.exporting).map_or(0, |n| n.load(std::sync::atomic::Ordering::Relaxed).signum());
        if nudge != self.nudged {
            self.nudged = nudge;
            self.clock.set_tempo(self.rate, self.ppq, self.clock_tempo());
        }
        self.frame = start + len as u64;
        self.mix(len, &song);
        self.out.clear();
        self.out.extend_from_slice(&self.bus[0][..len * 2]);
        // The main volume (spec: audio/mixer.md "Main volume"): playback hears it as a ramped gain on the master; an
        // export writes the samples without it and only its tail test sees it.
        let gain = if self.exporting {
            self.main_gain
        } else {
            let mut ramp = self.main_glide.block(self.main_gain, len, os_dsp::glide::engine_limit(self.rate));
            if !(ramp.is_flat() && ramp.first() == 1.0) {
                for f in self.out.chunks_exact_mut(2) {
                    let g = ramp.next();
                    f[0] *= g;
                    f[1] *= g;
                }
            }
            1.0
        };
        // The tail rule: the first quiet tick at or after the song's end is the last one. With delay compensation, and
        // the stored main volume exactly 12800, only the part of the tick the file has by then is tested: with a delay
        // of a tick or more none, and the tick counts as quiet (spec: audio/timing.md "Where the file ends").
        if !self.looping && !self.live_only && tick >= self.end_tick {
            let d = self.pdc.lag as usize;
            let quiet = if d > 0 && self.main_unity {
                d >= len || (!self.inst_active && self.out[2 * d..].iter().all(|s| (s * gain).abs() < QUIET))
            } else {
                !self.inst_active && self.out.iter().all(|s| (s * gain).abs() < QUIET)
            };
            let limit = (TAIL_LIMIT_SECONDS * f64::from(self.rate) / self.clock.samples_per_tick()) as u64;
            if quiet || tick >= self.end_tick + limit {
                self.tail_done = true;
            }
        }
        self.route_out();
        self.hand_out(tick, start, false, was_done);
        // The file keeps its length: as many frames as the head skipped follow the tick that ended the song.
        if self.tail_done && !was_done && self.pdc.lag > 0 {
            self.pdc.flush = Some(self.pdc.lag - self.pdc.head);
        }
    }

    /// What the engine itself played into each track this tick (sampler voices, the metronome's clicks) waits in the
    /// track's pad for the latest path into it (spec: audio/delay-compensation.md "Pads").
    fn pad_voices(&mut self, len: usize) {
        for (b, t) in self.bus.iter_mut().zip(&mut self.tracks) {
            t.pad.run(&mut b[..len * 2]);
        }
    }

    /// A tick of the recording count-in (`set_count_in`): handed out at the held position like a paused tick, but live
    /// notes and voices still ringing sound, and the metronome counts in.
    fn render_count_in(&mut self) {
        // Whole ticks of the count-in's own clock, so its beats fall on time.
        let done = self.count_in_len - self.count_in;
        let at = |t: u64| Clock::frame_of_tick(self.rate, self.ppq, self.tick_tempo(), t);
        let len = (at(done + 1) - at(done)).max(1) as usize;
        let mut song = self.transport_at(self.frame);
        song.playing = false;
        for b in &mut self.bus {
            b.clear();
            b.resize(len * 2, 0.0);
        }
        let (channels, w) = (&self.channels, self.sinc_w);
        let bus = &mut self.bus;
        self.voices.retain_mut(|v| render_voice(v, channels, bus, 0, 0, len, w));
        let beat = self.beat_ticks();
        let click = self.click_track();
        if let Some(m) = self.metronome.as_mut() {
            m.count_in(done, beat, self.beats_per_accent, &mut self.bus[click][..len * 2]);
        }
        self.pad_voices(len);
        self.render_instruments(len, &song);
        for ch in &mut self.channels {
            ch.advance(len as u32);
        }
        self.count_in -= 1;
        self.mix(len, &song);
        self.out.clear();
        self.out.extend_from_slice(&self.bus[0][..len * 2]);
        let mut ramp = self.main_glide.block(self.main_gain, len, os_dsp::glide::engine_limit(self.rate));
        if !(ramp.is_flat() && ramp.first() == 1.0) {
            for f in self.out.chunks_exact_mut(2) {
                let g = ramp.next();
                f[0] *= g;
                f[1] *= g;
            }
        }
        self.route_out();
        self.hand_out(self.tick, self.frame, true, false);
    }

    /// A tick's worth of the hosted instruments and the mixer while paused (`pause`): the position, the sampler
    /// channels and the automation stay where they are; the instruments, let go of their notes, and the effects run on,
    /// so their tails ring out. It is handed out at the held position: the tick and the frame the next tick starts on.
    fn render_held(&mut self) {
        let len = (self.clock.samples_per_tick().round() as usize).max(1);
        let mut song = self.transport_at(self.frame);
        song.playing = false;
        // Each held tick waits for its busy plugins anew, as a playing one does: a wait used up on an earlier tick
        // would leave every later moment the UI holds an effect a missed tick (heard as dropouts while tails ring).
        self.busy_until = None;
        for b in &mut self.bus {
            b.clear();
            b.resize(len * 2, 0.0);
        }
        self.render_instruments(len, &song);
        self.mix(len, &song);
        self.out.clear();
        self.out.extend_from_slice(&self.bus[0][..len * 2]);
        let mut ramp = self.main_glide.block(self.main_gain, len, os_dsp::glide::engine_limit(self.rate));
        if !(ramp.is_flat() && ramp.first() == 1.0) {
            for f in self.out.chunks_exact_mut(2) {
                let g = ramp.next();
                f[0] *= g;
                f[1] *= g;
            }
        }
        self.route_out();
        self.hand_out(self.tick, self.frame, true, false);
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
            let (end, ch, key, voice, _) = self.inst_offs[i];
            if end <= tick || (song_end && voice.is_none()) {
                self.inst_events.push(match voice {
                    Some(v) => notes::Event { voice: Some(v), ..notes::Event::choke(ch, 0, key) },
                    None => notes::Event::off(ch, 0, key),
                });
                self.inst_offs.swap_remove(i);
            } else {
                i += 1;
            }
        }
        // Steps (length 0) have no end of their own: with the notes whose offs are pending, every note an instrument
        // still holds is released with the song (spec: audio/song-structure.md "A note").
        if song_end {
            for (c, v) in self.note_voices.iter().enumerate() {
                self.inst_events.extend(v.ending_keys().into_iter().map(|key| notes::Event::off(c, 0, key)));
            }
        }
        self.sounding.retain(|&(_, _, end)| end == u64::MAX || (end > tick && !song_end));
    }

    /// Triggers the notes that start on `tick`.
    fn start_notes(&mut self, tick: u64) {
        // A plain step lights its key for one step; it sounds on until the song's end (`release_notes`).
        let step = u64::from(self.ppq / 4).max(1);
        for t in std::mem::take(&mut self.late) {
            if t.tick != tick {
                continue;
            }
            // A layer's note under way where playback starts goes through the layer, as at its own start.
            if self.channels[t.channel].layer.is_some() {
                self.hand_on(t, step);
            } else {
                self.start_trigger(t, step);
            }
        }
        let mut slides = Vec::new();
        while let Some(t) = self.triggers.get(self.next_trigger).copied() {
            if t.tick > tick {
                break;
            }
            self.next_trigger += 1;
            if t.tick < tick {
                continue;
            }
            if t.slide {
                slides.push(t);
            } else if self.channels[t.channel].layer.is_some() {
                self.hand_on(t, step);
            } else {
                self.start_trigger(t, step);
            }
        }
        // Slide notes take effect after the tick's note starts (spec: audio/timing.md "What a tick does").
        for t in slides {
            self.start_slide(t);
        }
        self.step_slides();
    }

    /// A slide note (spec: audio/song-structure.md "Slide notes"): it makes no sound. Every voice of its channel that
    /// is not fading out, released voices and voices started on this tick included, glides by `slide key - the pitch of
    /// the highest held voice` and to the slide note's velocity and pan, in `n + 1` steps over its length `n` in ticks,
    /// the first at once; a glide under way is replaced from where it is. A voice whose note has not ended counts as
    /// held (a voice without the volume envelope never ends its note). Without a held voice nothing moves. On a hosted
    /// instrument the note rules glide the pitch.
    ///
    /// A slide note on a layer moves the voices the layer's notes started, on all its children; a slide note on a
    /// child does not move those (spec: audio/note-rules.md "Layer channels").
    fn start_slide(&mut self, t: Trigger) {
        let n = t.end.map_or(0, |e| e.saturating_sub(t.tick)).min(u64::from(u32::MAX - 1)) as u32;
        let key = t.key.min(127) as u8;
        let is_layer = self.channels[t.channel].layer.is_some();
        if let Some(layer) = &self.channels[t.channel].layer {
            for (i, &c) in layer.children.iter().enumerate() {
                let hosted = self.channels.get(c).is_some_and(|ch| ch.plugin && ch.instrument.0.is_some());
                if hosted && !layer.children[..i].contains(&c) {
                    let slide = notes::Event::slide(c, 0, key, n);
                    self.inst_events.push(notes::Event { layer: Some(t.channel), colour: t.colour, ..slide });
                }
            }
        } else if self.channels[t.channel].plugin {
            if self.channels[t.channel].instrument.0.is_some() {
                self.inst_events.push(notes::Event { colour: t.colour, ..notes::Event::slide(t.channel, 0, key, n) });
            }
            return;
        }
        let own = |v: &Voice| if is_layer { v.via.is_some_and(|w| w.layer == t.channel) } else { v.channel == t.channel && v.via.is_none() };
        let moves = |v: &Voice| own(v) && v.colour == t.colour && v.cut.is_none() && v.release.is_none();
        let top = self.voices.iter().filter(|v| moves(v) && v.off.is_none()).map(|v| f32::from(v.key) * 100.0 + v.slide.cents).reduce(f32::max);
        let Some(top) = top else { return };
        let by = f32::from(t.key) * 100.0 - top;
        let steps = (n + 1) as f32;
        let (velocity, pan) = (f32::from(t.velocity), t.pan as f32);
        for v in self.voices.iter_mut().filter(|v| moves(v)) {
            let s = &mut v.slide;
            let d = |to: f32, own: f32| if to == own { 0.0 } else { (to - own) / steps };
            s.step = (if by == 0.0 { 0.0 } else { by / steps }, d(velocity, s.velocity), d(pan, s.pan));
            s.left = n + 1;
        }
    }

    /// One step of every voice's slide glide (spec: audio/note-rules.md "Glides": each level adds its step, the target
    /// is reached by the additions, not set). The pitch moves the voice's key rate, the velocity byte its velocity
    /// level (linearly, then the velocity law), the pan its note pan.
    fn step_slides(&mut self) {
        for v in &mut self.voices {
            let s = &mut v.slide;
            if s.left == 0 {
                continue;
            }
            s.left -= 1;
            let (dc, dv, dp) = s.step;
            if dc != 0.0 {
                s.cents += dc;
                v.step = s.rate * laws::cents_rate(f64::from(s.cents));
            }
            if dv != 0.0 {
                s.velocity += dv;
                v.vel = laws::volume_curve(f64::from(s.velocity.max(0.0)) / 100.0);
            }
            if dp != 0.0 {
                s.pan += dp;
                v.pan = f64::from(s.pan);
            }
        }
    }

    /// Starts one note or audio clip: a voice of the engine's sampler, or a note at the channel's instrument.
    fn start_trigger(&mut self, t: Trigger, step: u64) {
        // A channel that is off starts no notes (`switch_channel`); an audio clip starts, silenced by its level.
        if !self.channels[t.channel].enabled && t.cut.is_none() {
            return;
        }
        self.sounding.push((t.channel, t.key, t.end.unwrap_or(t.tick + step)));
        if self.channels[t.channel].plugin {
            if self.channels[t.channel].instrument.0.is_some() {
                self.channels[t.channel].via = t.via;
                let k = t.key.min(127) as u8;
                // An audio clip's voice gets a name: its gains and its end reach it alone, while another clip of the
                // channel plays on the same key (spec: audio/audio-clips.md "Automatic crossfades").
                let voice = (t.cut.is_some() || t.shape.is_some()).then(|| {
                    let v = self.next_voice;
                    self.next_voice = v.wrapping_add(1);
                    v
                });
                self.inst_events.push(notes::Event {
                    ch: t.channel,
                    off: 0,
                    key: k,
                    vel: f32::from(t.velocity) / 128.0,
                    start: t.start,
                    end: t.stop,
                    fine: t.fine,
                    long: t.end.is_some(),
                    glide: 0,
                    levels: t.levels,
                    cuts: t.cuts,
                    layer: t.via.map(|w| w.layer),
                    porta: t.porta,
                    voice,
                    colour: t.colour,
                });
                if let (Some(shape), Some(voice)) = (t.shape, voice) {
                    self.inst_shapes.push(InstClip { channel: t.channel, voice, shape, age: 0, sent: None });
                }
                // An audio clip ends at its cut (spec: plugins/lemon-sampler.md Â§7), or by itself at its end frame
                // on an instrument that ends it there (spec: audio/audio-clips.md "What a clip plays"): that one is
                // choked on the song's last tick if it still sounds then ("The end of the song"), and when it is looped
                // or the song ends otherwise, as every clip is. Notes end with a note off.
                let inst = self.channels[t.channel].instrument.0.as_ref();
                let ends = t.stop > 0.0 && inst.and_then(|i| i.lock().ok()).is_some_and(|i| i.voice_ends());
                match t.cut {
                    Some(_) if ends => {
                        let song = !self.looping && !self.live_only && self.end_tick > t.tick;
                        self.inst_offs.push((if song { self.end_tick } else { u64::MAX }, t.channel, k, voice, Some(t.origin())));
                    }
                    Some(c) => self.inst_offs.push((c, t.channel, k, voice, Some(t.origin()))),
                    // A step has no end of its own: released with the song (`release_notes`).
                    None => {
                        if let Some(end) = t.end {
                            self.inst_offs.push((end, t.channel, k, None, Some(t.origin())));
                        }
                    }
                }
            }
            return;
        }
        let layer = t.via.and_then(|w| self.channels.get(w.layer).map(|l| (l.layer_adds(), w.gain)));
        let gain = self.channels[t.channel].start_gain_via(t.vel, t.pan, layer);
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
            shape: t.shape,
            buf: t.buf,
            key: t.key,
            slide: VoiceSlide::new(t.rate, t.velocity, t.pan),
            colour: t.colour,
            via: t.via,
            origin: Some(t.origin()),
        });
    }

    /// A note on a layer channel (spec: audio/note-rules.md "Layer channels"). It makes no voice of its own: its cut
    /// group acts with the layer's own numbers (by the rule for channels whose voices are not one-shot voices), then
    /// the note goes to the children the layer's mode picks, in the order of the list. A child whose voices are
    /// one-shot voices and that has a group set writes its own numbers into the note, for itself and the children
    /// after it, and with each child the note's cut acts again. Each child plays the note as its own (its mono,
    /// portamento and voice limit apply), with the layer's levels added to its voice's and the crossfade gain on the
    /// volume. A child that is switched off is passed over; so is a child that is itself a layer (a Lemon Studio
    /// choice: layers among a layer's children are not covered).
    fn hand_on(&mut self, t: Trigger, step: u64) {
        let rng = &mut self.rng;
        let Some(layer) = self.channels[t.channel].layer.as_mut() else { return };
        let picked: Vec<(usize, f64)> = layer.places(rng).map(|i| (layer.children[i], layer.gain(i))).collect();
        let r = self.channels[t.channel].rules;
        let mut cuts = (r.cut, notes::voice_group(&r, r.cut, r.cut_by, t.end.is_some()));
        if cuts.0 > 0 {
            self.inst_events.push(notes::Event::cut(t.channel, 0, cuts.0));
        }
        self.sounding.push((t.channel, t.key, t.end.unwrap_or(t.tick + step)));
        let adds = self.channels[t.channel].layer_adds();
        for (child, gain) in picked {
            let Some(ch) = self.channels.get(child).filter(|ch| ch.enabled && ch.layer.is_none()) else { continue };
            let r = ch.rules;
            if r.one_shot && (r.cut > 0 || r.cut_by > 0) {
                cuts = (r.cut, r.cut_by);
            }
            if ch.played().is_none() && !ch.plugin {
                continue;
            }
            // A hosted instrument hears the layer's pitch as part of the note's tuning (the engine's voices follow
            // the layer's pitch while they sound).
            let mut c = Trigger { channel: child, via: Some(Via { layer: t.channel, gain }), cuts: Some(cuts), fine: t.fine + (adds.cents / 100.0) as f32, ..t };
            if ch.realtime.is_some() {
                if let Some(i) = ch.keyed.iter().position(|k| k.0 == t.key) {
                    (c.buf, c.rate) = ((i + 1) as u16, 1.0);
                }
            }
            self.start_trigger(c, step);
        }
    }

    /// Sends this tick's note events to each instrument and mixes its output into the channel's insert with the
    /// channel's volume and pan. An instrument busy elsewhere is waited for (`lock_busy`); one still busy misses the
    /// tick and makes it up when it is reached again.
    fn render_instruments(&mut self, n: usize, song: &TransportInfo) {
        self.render_generators(n, song, false);
        self.instruments_out(n);
    }

    /// The generators of a tick (spec: audio/multicore.md): the instruments' notes worked out, then each instrument
    /// renders its channel (`instrument_job`) and, with `voices`, the sampler voices render into their tracks' inputs,
    /// on the pool while the generator switch is on. What the instruments put out waits for `instruments_out`.
    fn render_generators(&mut self, n: usize, song: &TransportInfo, voices: bool) {
        for (ch, key, vel, midi) in self.inst_live.drain(..) {
            self.inst_events.push(if vel < 0.0 {
                notes::Event::off(ch, 0, key)
            } else {
                let on = notes::Event::on(ch, 0, key, vel);
                notes::Event { levels: crate::NoteLevels { channel: midi, ..on.levels }, ..on }
            });
        }
        self.inst_events.sort_by_key(|e| e.off);
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
        let c = self.channels.len();
        self.inst_held.clear();
        self.inst_held.resize(c, false);
        for ch in self.sounding.iter().map(|s| s.0).chain(self.inst_offs.iter().map(|o| o.1)) {
            if let Some(h) = self.inst_held.get_mut(ch) {
                *h = true;
            }
        }
        if self.inst_owed.len() < c {
            self.inst_owed.resize(c, 0);
        }
        if self.inst_tuning.len() < c {
            self.inst_tuning.resize(c, InstTuning::default());
        }
        let cx = InstCx {
            n,
            rate: self.rate,
            song,
            export: self.exporting,
            wait: self.busy_wait,
            channels: &self.channels,
            notes: &self.inst_notes,
            missed: &self.inst_missed,
            midi: &self.inst_midi,
            params: &self.params,
            shapes: multicore::Shapes::of(&mut self.inst_shapes),
            held: &self.inst_held,
            auto_bypass: auto_bypass::enabled(),
        };
        let parts = multicore::GenParts {
            tuning: &mut self.inst_tuning,
            owed: &mut self.inst_owed,
            pads: &mut self.pdc.chan_pad,
            until: &mut self.busy_until,
            voices: voices.then_some((&mut self.voices, self.sinc_w)),
            bus: &mut self.bus,
        };
        self.multicore.generators(&cx, parts);
    }

    /// The instruments' outputs of the tick join their channels' inserts, in channel order; an instrument that missed
    /// the tick owes its notes and frames, a delay that changed works the pads out again with the next tick.
    fn instruments_out(&mut self, n: usize) {
        let mut active = false;
        for (c, o) in self.multicore.instruments() {
            if !o.ran {
                // Still busy: it misses the tick. Its notes wait and its frames are owed, so it stays in time.
                self.params.resend_channel(c);
                let first = self.inst_notes.partition_point(|e| e.0 < c);
                self.inst_missed.extend(self.inst_notes[first..].iter().take_while(|e| e.0 == c).map(|&(c, _, note)| (c, note)));
                self.missed += 1;
                continue;
            }
            // Its delay, read while it was held: a change works the pads out again with the next tick.
            if let Some(d) = self.pdc.chan_delay.get_mut(c).filter(|d| **d != o.delay) {
                *d = o.delay;
                self.pdc.due = true;
            }
            // A sample that is still playing must not be cut off by the idle tail.
            active |= o.active;
            for (x, s) in self.bus[self.channels[c].insert].iter_mut().zip(&o.buf) {
                *x += *s;
            }
        }
        // The missed notes and controller messages of the instruments that ran have been sent.
        let ran = |c: usize| self.multicore.instrument_ran(c);
        self.inst_missed.retain(|m| !ran(m.0));
        self.inst_midi.retain(|m| !ran(m.0));
        self.inst_events.clear();
        // A clip's shape is kept half a second past its end (the choke's own fade).
        let tail = u64::from(self.rate / 2);
        for s in &mut self.inst_shapes {
            s.age += n as u64;
        }
        self.inst_shapes.retain(|s| s.shape.since + s.age < s.shape.total.saturating_add(tail));
        self.inst_active = active;
        if active {
            self.inst_tail = u64::from(self.rate) * INSTRUMENT_TAIL_SECONDS;
        }
    }

    /// On a device (`set_device`): the tick's master, in `out`, goes to the master's output pair and joins the tracks'
    /// direct outputs; `out` then holds every pair (spec: audio/track-io.md §2.2).
    fn route_out(&mut self) {
        let Some(io) = self.io.as_mut() else { return };
        let w = 2 * io.pairs;
        io.direct.resize(self.out.len() / 2 * w, 0.0);
        if let Some(p) = usize::try_from(self.tracks[0].output).ok().filter(|&p| p < io.pairs) {
            for (d, s) in io.direct.chunks_exact_mut(w).zip(self.out.chunks_exact(2)) {
                d[2 * p] += s[0];
                d[2 * p + 1] += s[1];
            }
        }
        std::mem::swap(&mut self.out, &mut io.direct);
    }

    /// The mixer over this tick: each track's effects, then its gain / pan (ramped over the tick after an edit), then
    /// its output added into its destinations; the master's output is the song. The tracks run in processing order, or
    /// on several cores with the same result (`multicore`; spec: audio/multicore.md).
    fn mix(&mut self, frames: usize, song: &TransportInfo) {
        let limit = os_dsp::glide::engine_limit(self.rate);
        // The device's inputs over this tick, read whether a track hears them or not (a track that starts to monitor
        // hears what arrives from then on), and the pairs the tracks' direct outputs are added into.
        if let Some(io) = self.io.as_mut() {
            io.direct.clear();
            io.direct.resize(frames * 2 * io.pairs, 0.0);
            if let Some(i) = io.input.as_mut() {
                io.inputs.resize(frames * i.channels(), 0.0);
                i.read(&mut io.inputs);
            }
        }
        // On the first tick the slot values arrived before delay compensation was worked out.
        if !self.pdc.delivered {
            for &t in &self.order {
                self.params.deliver_slots(t, &self.tracks[t].effects);
            }
        }
        let (io, direct) = match self.io.as_mut() {
            Some(DeviceIo { input, pairs, inputs, direct }) => (Some((&inputs[..], input.as_ref().map_or(0, |i| i.channels()), *pairs)), Some(direct)),
            None => (None, None),
        };
        let cx = multicore::MixCx {
            rate: self.rate,
            limit,
            song,
            frames,
            master_fx: self.master_fx,
            insert_fx: self.insert_fx,
            auto_bypass: auto_bypass::enabled(),
            io,
            keys: multicore::Keys::of(&mut self.key_out),
            export: self.exporting,
            wait: self.busy_wait,
            // The tick being mixed is the one before `tick`.
            record: self.record.as_deref().filter(|_| song.playing).map(|r| (r, self.tick.saturating_sub(1))),
            tick: (self.tick.saturating_sub(1), self.ppq),
        };
        let parts = multicore::MixParts {
            order: &self.order,
            tracks: &mut self.tracks,
            bus: &mut self.bus,
            capture: self.capture.as_deref_mut(),
            cap_skip: &mut self.pdc.cap_skip,
            direct,
            until: &mut self.busy_until,
            missed: &mut self.missed,
        };
        let moved = self.multicore.mix(parts, &cx);
        self.pdc.delivered = false;
        // A slot that reported another delay: the pads are worked out again with the next tick.
        if moved {
            self.pdc.due = true;
        }
    }

    /// The transport hosted plugins see at `frame` (spec: plugins/vst3-hosting.md Â§1): playing at the project tempo,
    /// in the project's time signature, with the pattern loop while looping. The tempo is the one the ticks run at, in
    /// quarter notes (`tick_tempo`): what FL tells plugins in a bar of other than 4 steps per beat is not measured.
    fn transport_at(&self, frame: u64) -> TransportInfo {
        let mut start = TransportInfo {
            playing: true,
            tempo: self.tick_tempo(),
            frame: 0,
            beats: 0.0,
            bar_start: 0.0,
            bar: 0,
            numerator: self.signature.0,
            denominator: self.signature.1,
            looping: None,
        };
        if self.looping {
            let ppq = f64::from(self.ppq);
            match self.loop_range {
                Some((a, b)) => start.looping = Some((a as f64 / ppq, b as f64 / ppq)),
                None if self.end_tick > 0 => start.looping = Some((0.0, self.end_tick as f64 / ppq)),
                None => {}
            }
        }
        start.advanced(frame, self.rate)
    }

    /// Loop point or marker jump: on to tick `to`, back or forward (sounding notes keep their remaining length). Away
    /// from tick 0 it lands as a seek does: control points before it applied, audio clips under way started from where
    /// they have got to. With `keep_clock` (a render's skip, spec: audio/song-structure.md "Time markers") the tick
    /// clock and the sample count run on; else the clock is the one of tick `to`.
    fn jump_to(&mut self, to: u64, keep_clock: bool) {
        // Scheduled ends move with the position: `back` ticks earlier (negative: later).
        let back = self.tick as i64 - to as i64;
        let shift = |t: u64| (t as i64 - back).max(0) as u64;
        // Audio clips end at the loop point: the replay starts them again (from the top, or where they are under way
        // at `to`), so one running on past the loop's end would sound twice, and once more every loop.
        self.voices.retain(|v| v.cut.is_none());
        // So do audio clips a hosted instrument plays (their end is a choke of that voice).
        let events = &mut self.inst_events;
        self.inst_offs.retain(|&(_, ch, key, voice, _)| {
            let Some(v) = voice else { return true };
            events.push(notes::Event { voice: Some(v), ..notes::Event::choke(ch, 0, key) });
            false
        });
        for o in &mut self.inst_offs {
            o.0 = shift(o.0);
        }
        for v in &mut self.voices {
            v.cut = v.cut.map(shift);
            v.release_at = v.release_at.map(shift);
        }
        if !keep_clock {
            if to == 0 {
                self.clock.restart();
            } else {
                self.clock = Clock::at_tick(self.rate, self.ppq, self.tick_tempo(), to);
            }
        }
        self.tick = to;
        self.frame = self.clock.next_start();
        self.next_trigger = self.triggers.partition_point(|t| t.tick < to);
        // Control points replay from the top: until the first one the channel has its own settings, and mixer tracks
        // glide back to theirs until automation hands its values on again (a Lemon Studio choice; FL's behaviour at
        // the loop point is not measured).
        for t in &mut self.tracks {
            t.restore(false);
        }
        self.sync_controls(to);
        if to > 0 {
            self.late = self.clips_under_way(to);
            if self.truncated_on_transport {
                let notes = self.notes_under_way(to);
                self.late.extend(notes);
            }
        }
        for s in &mut self.sounding {
            if s.2 != u64::MAX {
                s.2 = shift(s.2);
            }
        }
        self.sounding.retain(|&(_, _, end)| end > 0);
        // Delay compensation keeps its pads; values still waiting for late audio are dropped, and the slot switches put
        // back may have switched an insert's slots all off or one on again.
        self.pdc.waiting.clear();
        self.pdc.switched = true;
        // A song back at its start: the initial values first, then automation clips act there as usual.
        if to == 0 && !self.init.is_empty() {
            self.set_auto_values(self.init.clone(), 0);
        }
    }

    /// Renders the whole song: every tick up to the first quiet one at or after the song's end (spec: audio/timing.md,
    /// tail "leave remainder"). The length is a whole number of ticks. `_block` is kept for callers of the
    /// block-based engine.
    pub fn render_all(&mut self, _block: usize) -> Vec<f32> {
        let mut out = Vec::new();
        self.render_ticks(usize::MAX, &mut out);
        out
    }

    /// `render_all` in steps (an export with a progress bar): appends up to `ticks` more ticks to `out`; true once the
    /// song is over.
    pub fn render_ticks(&mut self, ticks: usize, out: &mut Vec<f32>) -> bool {
        self.exporting = true;
        out.extend_from_slice(&self.out[self.out_pos..]);
        self.out_pos = self.out.len();
        for _ in 0..ticks {
            if self.finished() {
                return true;
            }
            self.render_tick();
            out.extend_from_slice(&self.out);
            self.out_pos = self.out.len();
        }
        self.finished()
    }
}

/// Notes still held on instruments end when the engine goes away (stop, or a new song).
impl Drop for Engine {
    fn drop(&mut self) {
        self.send_held_offs();
    }
}

impl Engine {
    /// A note off to its instrument for every note held there: scheduled notes that have not ended, live notes, and
    /// voices whose notes have not ended.
    fn send_held_offs(&self) {
        // The key each note was given to its instrument as (moved by the channel pitch, `InstTuning`).
        let sent = |c: usize, key: u8| self.inst_tuning.get(c).map_or(key, |t| t.sent(key));
        // Audio clips a hosted instrument plays end with a choke of their voice: a note off would leave a sample
        // without a volume envelope playing on.
        for &(_, c, key, voice, _) in &self.inst_offs {
            if let (Some(v), Some(Ok(mut inst))) = (voice, self.channels.get(c).and_then(|ch| ch.instrument.0.as_ref()).map(|i| i.lock())) {
                inst.voice_choke(0, sent(c, key), v);
            }
        }
        let mut notes: Vec<(usize, u8)> = self.inst_offs.iter().filter(|o| o.3.is_none()).map(|&(_, c, key, _, _)| (c, key)).collect();
        notes.extend(self.sounding.iter().filter(|s| s.2 == u64::MAX).map(|&(c, key, _)| (c, key.min(127) as u8)));
        // Voices whose notes have not ended (a mono voice sounds under its first key).
        let mut voices: Vec<(usize, u8)> = Vec::new();
        for (c, v) in self.note_voices.iter().enumerate() {
            voices.extend(v.held_keys().map(|key| (c, key)));
        }
        // A note is held in both lists: one note off each. A key held twice (a clip's note still sounding as the next
        // clip starts the same key) gets two, or its second voice rings on.
        let count = |v: &[(usize, u8)], k: (usize, u8)| v.iter().filter(|&&x| x == k).count();
        let mut keys: Vec<(usize, u8)> = notes.iter().chain(&voices).copied().collect();
        keys.sort_unstable();
        keys.dedup();
        let held = keys.into_iter().flat_map(|k| std::iter::repeat_n(k, count(&notes, k).max(count(&voices, k))));
        for (c, key) in held {
            if let Some(Ok(mut inst)) = self.channels.get(c).and_then(|ch| ch.instrument.0.as_ref()).map(|i| i.lock()) {
                inst.note_off(0, sent(c, key));
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
/// The place in `ch.keyed` of the copy of its sample stretched for `key`, made when missing (realtime stretch methods
/// only): the channel's pitch with its fine tune, root and main pitch, and the stretch's own included.
fn keyed_copy(ch: &mut ChannelState, library: &mut SampleLibrary, key: u16) -> Option<usize> {
    let (Some((factor, cents)), Some(s)) = (ch.realtime, ch.sample.clone()) else { return None };
    Some(match ch.keyed.iter().position(|k| k.0 == key) {
        Some(i) => i,
        None => {
            let transpose = laws::playback_rate(key, 0) * laws::cents_rate(ch.pitch_from_60()) * 2f64.powf(cents / 1200.0);
            ch.keyed.push((key, library.stretched(&s, stretched_frames(s.frames(), factor), transpose)));
            ch.keyed.len() - 1
        }
    })
}

fn render_voice(v: &mut Voice, channels: &[ChannelState], bus: &mut [Vec<f32>], span: usize, offset: usize, end: usize, w: f64) -> bool {
    render_voice_into(v, channels, &mut bus[channels[v.channel].insert], span, offset, end, w)
}

/// `render_voice` into `out`, its track's input (or a buffer of its own, added into it later).
fn render_voice_into(v: &mut Voice, channels: &[ChannelState], out: &mut [f32], span: usize, offset: usize, end: usize, w: f64) -> bool {
    let ch = &channels[v.channel];
    let Some(s) = ch.buffer(v.buf) else { return false };
    let n = s.frames();
    let glide = ch.ramp.filter(|r| v.serial < r.id);
    // A layer's note: the layer's volume and crossfade gain multiply, its pan and pitch add (spec: audio/note-rules.md
    // "Layer channels").
    let layer = v.via.and_then(|w| channels.get(w.layer).map(|l| (l.layer_adds(), w.gain)));
    let (lv, lp) = layer.map_or((1.0, 0.0), |(a, g)| (a.volume * g, a.pan));
    let target = ch.start_gain_via(v.vel, v.pan, layer);
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
    let base_step = ch.base_rate * v.step * layer.map_or(1.0, |(a, _)| laws::cents_rate(a.cents));
    let mut step = base_step;
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
        let shape = v.shape.map_or(1.0, |s| s.at(v.age));
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
        } else if w <= 1.0 {
            // Linear (an export's 2-point resampling).
            let i = pos as usize;
            let (j, x) = ((i + 1).min(n - 1), (pos - i as f64) as f32);
            (s.left[i] + (s.left[j] - s.left[i]) * x, s.right[i] + (s.right[j] - s.right[i]) * x)
        } else {
            let i0 = (pos - w).ceil().max(0.0) as usize;
            let i1 = ((pos + w).floor() as usize).min(n - 1);
            let (mut l, mut r) = (0.0f64, 0.0f64);
            for i in i0..=i1 {
                let t = i as f64 - pos;
                let k = sinc(t / scale) / scale * blackman(t, w);
                l += f64::from(s.left[i]) * k;
                r += f64::from(s.right[i]) * k;
            }
            (l as f32, r as f32)
        };
        if let Some(r) = glide {
            // The volume parameter glides linearly and the volume law applies to it; the pan glides linearly
            // in the left / right gains (spec: audio/pattern-controls.md).
            let frames = f64::from(r.pos) + (f - span) as f64 + 1.0;
            let g = ch.vol_gain(r.v0 + (r.v1 - r.v0) * r.volume_at(frames)) * v.vel * lv;
            let law = ch.pan_law;
            let (a, b) = (laws::channel_pan_law(ch.pan_of(r.p0) + v.pan + lp, law), laws::channel_pan_law(ch.pan_of(r.p1) + v.pan + lp, law));
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
        out[f * 2] += l * gl * fade * env_gain * shape;
        out[f * 2 + 1] += r * gr * fade * env_gain * shape;
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

fn blackman(t: f64, w: f64) -> f64 {
    if t.abs() >= w {
        return 0.0;
    }
    let x = std::f64::consts::PI * t / w;
    0.42 + 0.5 * x.cos() + 0.08 * (2.0 * x).cos()
}

/// One mixer track's own state for its turn in a tick (`mix_track`).
struct Lane<'a> {
    t: usize,
    track: &'a mut MixTrack,
    /// What reached the track (its voices, channels and inputs); the track's output once it has run.
    buf: &'a mut Vec<f32>,
    /// While capturing, the track's capture and the lateness still to skip at its start (`set_track_capture`).
    capture: Option<&'a mut Vec<f32>>,
    skip: Option<&'a mut u64>,
    /// What it adds to other tracks and to the device's pairs.
    out: &'a mut multicore::Outbox,
    /// When the tick stops waiting for hosted plugins busy elsewhere, the ticks they missed, a slot's delay changed.
    until: &'a mut Option<Instant>,
    missed: &'a mut u64,
    moved: &'a mut bool,
}

/// A mixer track's turn in a tick (`Engine::mix`): a monitored input joins, its slots, its equaliser, its fader matrix;
/// its output for the slots that listen to it, its meters, its capture; then what it sends to other tracks and plays on
/// a device pair goes to its outbox.
fn mix_track(lane: Lane, cx: &multicore::MixCx, s: &mut multicore::MixScratch) {
    let Lane { t, track, buf, capture, skip, out, until, missed, moved } = lane;
    let (frames, limit) = (cx.frames, cx.limit);
    out.clear();
    // A monitored input joins what reaches the track, before its slots (spec: audio/track-io.md §2.2).
    if let Some((inputs, ch, _)) = cx.io {
        if let Some((l, r)) = input_channels(track.input, ch) {
            for (f, x) in buf.chunks_exact_mut(2).zip(inputs.chunks_exact(ch)) {
                f[0] += x[l];
                f[1] += x[r];
            }
        }
    }
    // Disk recording, while the song plays (spec: audio/track-io.md §2.5).
    let tap = cx.record.and_then(|(r, at)| Some((r, at, track.rec?)));
    if let Some((r, at, arm)) = tap.filter(|(_, _, a)| matches!(a.point, RecordPoint::External | RecordPoint::WithMixer)) {
        rec_input(r, t, at, arm, track.input, cx.io, buf, &mut s.rec);
    }
    // A slot runs when its own switch is on and the track's slots are enabled (spec: audio/mixer.md).
    if track.slots_enabled && if t == 0 { cx.master_fx } else { cx.insert_fx } {
        let (dry, l, r, key_buf, main_buf) = (&mut s.dry, &mut s.l, &mut s.r, &mut s.key, &mut s.main);
        for fx in &mut track.effects {
            // SAFETY: the tracks a slot listens to do not run meanwhile (`multicore`).
            let feed = |s: Option<usize>| s.and_then(|s| unsafe { cx.keys.get(s) }).filter(|k| k.len() == buf.len());
            let key = feed(fx.key).map(|k| (k, &mut *key_buf));
            let main = feed(fx.main).map(|k| (k, &mut *main_buf));
            let extra = (t, &track.routes[..], &mut *out);
            fx.skipped = run_slot(fx, buf, cx.rate, limit, (cx.song, cx.tick), dry, l, r, (&mut *until, cx.wait, cx.export, &mut *missed), &mut *moved, extra, (key, main), cx.auto_bypass);
        }
    }
    if let Some((r, at, rec::Arm { point: RecordPoint::PostFx, .. })) = tap {
        r.push(t, at, buf);
    }
    track.eq.process(buf);
    if let Some((r, at, rec::Arm { point: RecordPoint::PostEq, .. })) = tap {
        r.push(t, at, buf);
    }
    // The four gains glide to their targets: a per-sample step for this tick, at most `limit`, applied as a running
    // sum (spec: audio/mixer.md "Gains ramp").
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
    if let Some((r, at, rec::Arm { point: RecordPoint::PostLevel, .. })) = tap {
        r.push(t, at, buf);
    }
    // What the track puts out, for the slots that listen to it (spec: audio/sidechain.md "Sidechain input").
    // SAFETY: no slot reads it meanwhile (`multicore`).
    if let Some(k) = unsafe { cx.keys.slot(t) } {
        k.clear();
        k.extend_from_slice(buf);
    }
    let pk = &mut track.peak;
    for f in buf.chunks_exact(2) {
        pk[0] = pk[0].max(f[0].abs());
        pk[1] = pk[1].max(f[1].abs());
    }
    // The direct output, after the fader matrix, besides the sends (the master's goes out in `route_out`).
    if let (Some((_, _, pairs)), true) = (cx.io, t != 0) {
        if let Some(p) = usize::try_from(track.output).ok().filter(|&p| p < pairs) {
            out.put(multicore::Dest::Pair(p)).extend_from_slice(buf);
        }
    }
    if let Some(c) = capture {
        // Each track's own lateness is skipped at the start (`take_track_audio`).
        let skip = skip.map_or(0, |s| {
            let k = (*s).min(frames as u64);
            *s -= k;
            k as usize
        });
        c.extend_from_slice(&buf[skip * 2..frames * 2]);
    }
    // Sends glide like the matrix gains; a route at 0 passes nothing.
    for route in &mut track.routes {
        let mut ramp = route.glide.block(route.gain, frames, limit);
        if route.pad.len() > 0 {
            // Delay compensation: the send's level, then its pad, which runs every tick (also at level 0).
            let padded = out.put(multicore::Dest::Track(route.dest));
            if ramp.is_flat() {
                let g = ramp.first();
                padded.extend(buf.iter().map(|s| *s * g));
            } else {
                for s in buf.chunks_exact(2) {
                    let g = ramp.next();
                    padded.extend([s[0] * g, s[1] * g]);
                }
            }
            route.pad.run(padded);
        } else if !ramp.is_flat() {
            let into = out.put(multicore::Dest::Track(route.dest));
            for s in buf.chunks_exact(2) {
                let g = ramp.next();
                into.extend([s[0] * g, s[1] * g]);
            }
        } else if ramp.first() != 0.0 {
            let g = ramp.first();
            out.put(multicore::Dest::Track(route.dest)).extend(buf.iter().map(|s| *s * g));
        }
    }
}

/// Disk recording from the device input (External input only), or with what reaches the track before its slots
/// (External and mixer input; `buf` holds the input already when the track hears it: its `input` is set). A single
/// input is on both sides; a track with no input, or with one the device does not have, records silence or the mixer
/// alone. `take` is the lane's scratch.
#[allow(clippy::too_many_arguments)]
fn rec_input(tap: &dyn RecordTap, t: usize, at: u64, arm: rec::Arm, input: i32, io: Option<(&[f32], usize, usize)>, buf: &[f32], take: &mut Vec<f32>) {
    take.clear();
    if arm.point == RecordPoint::WithMixer {
        take.extend_from_slice(buf);
    } else {
        take.resize(buf.len(), 0.0);
    }
    let heard = input >= 0 && arm.point == RecordPoint::WithMixer;
    if let (Some((inputs, ch, _)), false) = (io, heard) {
        if let Some((l, r)) = input_channels(arm.input, ch) {
            for (f, x) in take.chunks_exact_mut(2).zip(inputs.chunks_exact(ch)) {
                f[0] += x[l];
                f[1] += x[r];
            }
        }
    }
    tap.push(t, at, take);
}

/// What every generator channel's turn of a tick reads (`instrument_job`).
struct InstCx<'a> {
    n: usize,
    rate: u32,
    song: &'a TransportInfo,
    export: bool,
    wait: Duration,
    channels: &'a [ChannelState],
    /// This tick's notes by channel, the notes instruments missed, controller messages waiting.
    notes: &'a [(usize, u32, notes::Note)],
    missed: &'a [(usize, notes::Note)],
    midi: &'a [(usize, [u8; 3])],
    params: &'a param_auto::ParamAuto,
    shapes: multicore::Shapes,
    /// Per channel: a note held or due to end (`Engine::inst_held`); the program's Auto-bypass when silent setting.
    held: &'a [bool],
    auto_bypass: bool,
}

// SAFETY: `shapes` is shared under its own rule (each channel's turn touches its own clips); the rest is read only.
unsafe impl Sync for InstCx<'_> {}

/// A generator channel's own state for its turn.
struct InstLane<'a> {
    c: usize,
    fines: &'a mut InstTuning,
    owed: &'a mut usize,
    pad: Option<&'a mut pdc::Pad>,
    until: &'a mut Option<Instant>,
    out: &'a mut multicore::InstOut,
}

/// A generator channel's turn in a tick: its instrument gets the tick's notes and renders; what the channel adds to its
/// insert (its volume and pan, its pad) goes to `out` (`Engine::instruments_out` adds it in). An instrument busy
/// elsewhere after the tick's wait misses the tick (`out.ran` stays false) and owes its frames.
fn instrument_job(cx: &InstCx, lane: InstLane, sc: &mut multicore::MixScratch) {
    let InstLane { c, fines, owed, pad, until, out } = lane;
    let n = cx.n;
    let ch = &cx.channels[c];
    (out.ran, out.active) = (false, false);
    out.buf.clear();
    let Some(inst) = ch.instrument.0.as_ref() else { return };
    let Some(mut inst) = lock_busy(inst, until, cx.wait, cx.export) else {
        *owed += n;
        out.quiet.note(false, n);
        return;
    };
    out.ran = true;
    if sc.l.len() < n {
        sc.l.resize(n, 0.0);
        sc.r.resize(n, 0.0);
    }
    let first = cx.notes.partition_point(|e| e.0 < c);
    // Auto-bypass when silent (spec: audio/auto-bypass.md "Instruments", Lemon Studio's design): an instrument that has
    // had nothing to play and put out silence for the hosted hold is not run, until a note, a controller message, a
    // parameter value, an audio clip or a pitch change reaches it, or it says it still sounds. It adds nothing to its
    // insert meanwhile.
    let quiet_in = ch.smart
        && cx.auto_bypass
        && *owed == 0
        && !cx.held.get(c).copied().unwrap_or(false)
        && cx.notes.get(first).is_none_or(|e| e.0 != c)
        && !cx.missed.iter().any(|m| m.0 == c)
        && !cx.midi.iter().any(|m| m.0 == c)
        && cx.params.channel_ticks(c).next().is_none()
        && cx.params.channel_span(c, 0, n).next().is_none()
        && fines.shift == ch.hosted_shift()
        // SAFETY: only this channel's turn touches its clips.
        && unsafe { cx.shapes.of_channel(c) }.next().is_none()
        && !inst.active();
    if quiet_in && out.quiet.skip(true, Some(auto_bypass::hold_frames(auto_bypass::HOSTED_HOLD, cx.rate, out.delay))) {
        out.active = false;
        return;
    }
    let notes = cx.notes[first..].iter().take_while(|e| e.0 == c);
    // The channel's pitch (knob, control points, automation, fine tune, root note, main pitch; a sampler channel's
    // instrument applies root and fine itself) reaches the instrument as each note's tuning, with the note's fine
    // pitch, on top of any glide; notes keep the pitch they started with. A plugin channel's instrument that ignores a
    // tuning hears its whole semitones as moved keys instead.
    let shift = ch.hosted_shift();
    fines.moves = !ch.own_tuning && !inst.takes_tuning();
    // A changed channel pitch reaches the notes already sounding.
    if fines.shift != shift {
        fines.shift = shift;
        for key in (0..128u8).filter(|&k| fines.keys >> k & 1 == 1) {
            inst.tune(0, fines.sent(key), fines.tuning(key));
        }
    }
    // Ticks it missed are made up first: their notes at once, then their frames, unheard (at most a second; an
    // instance held longer than that loses that much of its place).
    let owed = std::mem::take(owed);
    let mut missed = cx.missed.iter().filter(|m| m.0 == c).peekable();
    if owed > 0 || missed.peek().is_some() {
        inst.transport(cx.song);
        for m in missed {
            send_note(&mut *inst, 0, m.1, fines);
        }
        let mut left = owed.min(cx.rate as usize);
        while left > 0 {
            let k = left.min(sc.l.len());
            if k == 0 {
                break;
            }
            inst.process(cx.rate, &mut sc.l[..k], &mut sc.r[..k]);
            left -= k;
        }
    }
    inst.transport(cx.song);
    // Controller messages before this tick's notes.
    for &(_, bytes) in cx.midi.iter().filter(|m| m.0 == c) {
        inst.midi(0, bytes);
    }
    // The layer whose notes it plays adds its levels to the channel's.
    let layer = ch.via.and_then(|w| cx.channels.get(w.layer).map(|l| (l.layer_adds(), w.gain)));
    let mut levels = ch.levels();
    if let Some((a, gain)) = layer {
        levels.volume = (f64::from(levels.volume) * a.volume * gain) as f32;
        levels.pan += a.pan as f32;
        levels.mod_x += a.mods.0;
        levels.mod_y += a.mods.1;
    }
    let own_levels = inst.channel_levels(levels);
    inst.channel_voicing(&ch.voicing);
    // Parameter values before the notes at the same frame (spec: plugins/plugin-parameters.md §2); one per tick at its
    // start for a plugin that takes them so (spec: plugins/fruity-abi.md "Parameters"), parameter by parameter (a
    // channel's container: each member as its plugin takes them).
    for (index, value) in cx.params.channel_ticks(c) {
        if inst.tick_params_at(index) {
            inst.param(0, index, value);
        }
    }
    for (off, index, value) in cx.params.channel_span(c, 0, n) {
        if !inst.tick_params_at(index) {
            inst.param(off, index, value);
        }
    }
    for &(_, off, note) in notes {
        send_note(&mut *inst, off.min(n.saturating_sub(1) as u32), note, fines);
    }
    // Audio clip fades and gain on this channel (spec: audio/audio-clips.md "Fades and clip gain"): on each clip's
    // voice when the instrument takes voice gains, so clips that overlap (a crossfade) each have their own. A voice
    // gets its gain now at its start, then every `VOICE_GAIN_FRAMES` frames the gain due that many frames on, which it
    // glides to; a gain that stays is not sent again.
    let voiced = inst.voice_gains();
    // SAFETY: only this channel's turn touches its clips.
    let shapes = || unsafe { cx.shapes.of_channel(c) };
    if voiced {
        let step = crate::instrument::VOICE_GAIN_FRAMES;
        for s in shapes() {
            if s.sent.is_none() {
                let g = s.shape.at(s.age);
                inst.voice_gain(0, s.voice, g);
                s.sent = Some(g);
            }
            for off in (0..n as u32).step_by(step as usize) {
                let g = s.shape.at(s.age + u64::from(off + step));
                if s.sent != Some(g) {
                    inst.voice_gain(off, s.voice, g);
                    s.sent = Some(g);
                }
            }
        }
    }
    let (l, r) = (&mut sc.l[..n], &mut sc.r[..n]);
    inst.process(cx.rate, l, r);
    out.delay = inst.reported_delay();
    // An instrument without voice gains: the clips' shapes on its whole output. A clip that has ended keeps the channel
    // silent through its choke's tail only while no other clip of the channel plays; then the playing clips' gains
    // alone count. Clips that overlap still share one gain, so a crossfade between them dips.
    if !voiced && shapes().next().is_some() {
        for f in 0..n {
            let ended = |s: &InstClip| s.shape.since + s.age + f as u64 >= s.shape.total;
            let playing = shapes().any(|s| !ended(&*s));
            let g: f32 = shapes().filter(|s| !(playing && ended(&**s))).map(|s| s.shape.at(s.age + f as u64)).product();
            l[f] *= g;
            r[f] *= g;
        }
    }
    out.active = inst.active();
    // An audio clip channel's voices are its playlist clips: their own level is on the channel's output, since a
    // hosted sampler takes velocities only up to 128 (spec: audio/audio-clips.md "Level").
    let cl = ch.clip_level;
    let (gl, gr) = if own_levels { (cl, cl) } else { ch.gain_after(n as u32, f64::from(cl), layer) };
    // A level that changed smooths over the tick, as a voice's does: it has arrived when the curve says so (spec:
    // audio/automation.md).
    let (sl, sr) = out.gain.replace((gl, gr)).unwrap_or((gl, gr));
    if (sl, sr) == (gl, gr) {
        out.buf.extend(l.iter().zip(r.iter()).flat_map(|(&a, &b)| [a * gl, b * gr]));
    } else {
        let (dl, dr) = ((gl - sl) / n as f32, (gr - sr) / n as f32);
        out.buf.extend(l.iter().zip(r.iter()).enumerate().flat_map(|(f, (&a, &b))| {
            let x = (f + 1) as f32;
            [a * (sl + dl * x), b * (sr + dr * x)]
        }));
    }
    out.quiet.note(quiet_in && !out.active && auto_bypass::silent(&out.buf), n);
    // Delay compensation: the channel waits for the latest path into its track.
    if let Some(pad) = pad.filter(|p| p.len() > 0) {
        pad.run(&mut out.buf);
    }
}

/// One tick of a mixer slot over interleaved `buf`: skipped when its switch is off, blended with the dry signal by its
/// dry level, which glides to its mix level by at most `limit` a sample (spec: audio/mixer.md "An effect slot", "A
/// slot's mix level while it moves"). A slot at mix 0 still runs its effect.
///
/// `(t, routes, out)`: the slot's track, its routes and its outbox, for an own effect's extra output, which is
/// added to the input of one of the track's outputs (spec: audio/sidechain.md "Extra output"). It is not padded for
/// delay compensation (audio/delay-compensation.md); it is connected from the first tick (the cleaner choice of
/// audio/quirks.md for FL's late connection).
///
/// `key`: the output of the track the slot listens to as its sidechain input, and a buffer for it after the slot's
/// pad, which runs also while the slot is off (spec: audio/sidechain.md "While the slot is off"). It is connected from
/// the first tick too.
#[allow(clippy::too_many_arguments)]
fn run_slot(
    fx: &mut FxSlot,
    buf: &mut [f32],
    rate: u32,
    limit: f32,
    (song, tick): (&TransportInfo, (u64, u16)),
    dry: &mut Vec<f32>,
    l: &mut Vec<f32>,
    r: &mut Vec<f32>,
    (until, wait, export, missed): (&mut Option<Instant>, Duration, bool, &mut u64),
    moved: &mut bool,
    (t, routes, out): (usize, &[Route], &mut multicore::Outbox),
    (key, main): (Option<(&[f32], &mut Vec<f32>)>, Option<(&[f32], &mut Vec<f32>)>),
    auto: bool,
) -> bool {
    let n = buf.len() / 2;
    let ramp = fx.dry.block(os_dsp::mixer::dry_level(fx.mix), n, limit);
    let fill = |(src, padded): (&[f32], &mut Vec<f32>), pad: &mut pdc::Pad| {
        padded.clear();
        padded.extend_from_slice(src);
        pad.run(padded);
    };
    let key = key.map(|k| {
        fill((k.0, &mut *k.1), &mut fx.key_pad);
        &k.1[..]
    });
    let main = main.map(|m| {
        fill((m.0, &mut *m.1), &mut fx.main_pad);
        &m.1[..]
    });
    if !fx.on {
        return false;
    }
    // Auto-bypass when silent (spec: audio/auto-bypass.md): a slot that has heard and put out silence for its hold time
    // passes its silent input on without running the effect, until sound reaches the effect again (its input, the
    // sidechain or the main bus it takes).
    let smart = fx.smart && auto;
    let quiet_in = smart && auto_bypass::silent(buf) && key.is_none_or(auto_bypass::silent) && main.is_none_or(auto_bypass::silent);
    if smart {
        let hold = match &fx.fx {
            Fx::Own(e) => e.quiet_hold(),
            Fx::Hosted(_) => Some(auto_bypass::HOSTED_HOLD),
        };
        if fx.quiet.skip(quiet_in, hold.map(|h| auto_bypass::hold_frames(h, rate, fx.delay))) {
            return true;
        }
    }
    let mut loud_extra = false;
    let blend = !(ramp.is_flat() && ramp.first() == 0.0);
    let fresh = std::mem::take(&mut fx.fresh);
    // The dry part waits for the effect's delay (spec: audio/delay-compensation.md "A slot's dry part"): every tick,
    // also at a mix of 100 %, and emptied with the effect when the slot is switched on.
    let padded = fx.dry_pad.len() > 0;
    if fresh {
        fx.dry_pad.clear();
    }
    if blend || padded {
        dry.clear();
        dry.extend_from_slice(buf);
        fx.dry_pad.run(dry);
    }
    // The delay the effect reports, read right after it processes.
    let delay = match &mut fx.fx {
        Fx::Own(e) => {
            if fresh {
                e.reset();
            }
            e.transport(song, tick);
            match key {
                Some(key) => e.process_keyed(buf, key),
                None => e.process(buf),
            }
            if let Some((k, signal)) = e.extra() {
                loud_extra = smart && !auto_bypass::silent(signal);
                // On a track routed nowhere the extra output joins the track's own audio behind the effect, or the dry
                // part when the slot's mix level is below 100 %.
                let dest = output(routes, t, k);
                if dest != t {
                    out.put(multicore::Dest::Track(dest)).extend_from_slice(signal);
                } else {
                    let into: &mut [f32] = if blend { &mut dry[..] } else { &mut *buf };
                    for (d, s) in into.iter_mut().zip(signal) {
                        *d += *s;
                    }
                }
            }
            e.latency() as u32
        }
        Fx::Hosted(h) => {
            // An instance still busy elsewhere after the tick's wait passes the sound through for this tick (a
            // poisoned one always does).
            let Some(mut h) = lock_busy(h, until, wait, export) else {
                *missed += u64::from(!h.is_poisoned());
                fx.fresh |= fresh;
                fx.quiet.note(false, n);
                return false;
            };
            if fresh {
                h.reset_fx();
            }
            // A main port takes another input instead of the track's own audio; an auxiliary port without an input hears
            // the track's own audio too (spec: audio/sidechain.md "Hosted plugins' input buses").
            let src: &[f32] = main.unwrap_or(buf);
            l.clear();
            r.clear();
            l.extend(src.chunks_exact(2).map(|f| f[0]));
            r.extend(src.chunks_exact(2).map(|f| f[1]));
            h.transport_fx(song);
            // The key's two sides go behind the input's in `l` and `r`.
            match key {
                Some(k) => {
                    l.extend(k.chunks_exact(2).map(|f| f[0]));
                    r.extend(k.chunks_exact(2).map(|f| f[1]));
                }
                None => {
                    l.extend_from_within(..n);
                    r.extend_from_within(..n);
                }
            }
            let ((il, kl), (ir, kr)) = (l.split_at_mut(n), r.split_at_mut(n));
            h.process_fx_keyed(rate, il, ir, (kl, kr));
            for (i, f) in buf.chunks_exact_mut(2).enumerate() {
                f[0] = l[i];
                f[1] = r[i];
            }
            h.reported_delay_fx()
        }
    };
    if delay != fx.delay {
        fx.delay = delay;
        *moved = true;
    }
    if blend {
        os_dsp::mixer::slot_blend(buf, dry, ramp);
    }
    if smart {
        fx.quiet.note(quiet_in && !loud_extra && auto_bypass::silent(buf), n);
    }
    false
}

/// Input `k` (from 1) of track `t`: the tracks routed into it in track order, whatever the route's level, a number past
/// the end meaning the last one; `None` when nothing is routed into it (spec: audio/sidechain.md "Settings").
fn input(tracks: &[MixTrack], t: usize, k: usize) -> Option<usize> {
    let mut ins = (0..tracks.len()).filter(|&s| s != t && tracks[s].routes.iter().any(|r| r.dest == t));
    let last = ins.clone().last();
    ins.nth(k.max(1) - 1).or(last)
}

/// Input `k` (from 1) of track `t` with no stand-in: `None` past the last input, as for a hosted plugin's port (spec:
/// audio/sidechain.md "Hosted plugins' input buses").
fn input_at(tracks: &[MixTrack], t: usize, k: usize) -> Option<usize> {
    (0..tracks.len()).filter(|&s| s != t && tracks[s].routes.iter().any(|r| r.dest == t)).nth(k.checked_sub(1)?)
}

/// Output `k` (from 1) of track `t`: the tracks it is routed to in track order, a number past the end meaning the last
/// one; a track routed nowhere (the master) is its own only output (spec: audio/sidechain.md "Settings").
fn output(routes: &[Route], t: usize, k: usize) -> usize {
    let mut at = None;
    for _ in 0..k.max(1) {
        match routes.iter().map(|r| r.dest).filter(|&d| at.is_none_or(|a| d > a)).min() {
            Some(d) => at = Some(d),
            None => break,
        }
    }
    at.unwrap_or(t)
}

/// Hands one note event to an instrument at frame `off` of its next block. `fines`: the channel's pitch in semitones
/// and, per key, the fine pitch of the note last started there and the key it was given as (notes keep the pitch they
/// started with).
fn send_note(inst: &mut dyn Instrument, off: u32, note: notes::Note, fines: &mut InstTuning) {
    match note {
        notes::Note::Choke(key, None) => inst.choke(off, fines.sent(key)),
        notes::Note::Choke(key, Some(voice)) => inst.voice_choke(off, fines.sent(key), voice),
        notes::Note::Off(key) => inst.note_off(off, fines.sent(key)),
        notes::Note::On { key, vel, start, end, fine, levels, voice } => {
            // An instrument that ignores a tuning plays the note's pitch to the nearest semitone on a moved key.
            let sent = if fines.moves { (i32::from(key) + (fines.shift + fine).round() as i32).clamp(0, 127) as u8 } else { key };
            // The note's own pan and mod X / Y go with it (spec: audio/voice-levels.md "The note's own levels").
            match voice {
                Some(v) => inst.voice_start(off, sent, vel, start, end, levels, v),
                None => inst.note_start(off, sent, vel, start, levels),
            }
            let k = key & 127;
            (fines.fine[usize::from(k)], fines.glide[usize::from(k)], fines.sent[usize::from(k)]) = (fine, 0.0, sent);
            fines.keys |= 1 << k;
            let tuning = fines.tuning(key);
            if tuning.abs() > 1e-3 {
                inst.tune(off, sent, tuning);
            }
        }
        notes::Note::Tune(key, semitones) => {
            fines.glide[usize::from(key & 127)] = semitones;
            inst.tune(off, fines.sent(key), fines.tuning(key));
        }
    }
}

/// Locks a hosted plugin the UI thread also uses. While it is busy there the tick waits, until `until` (set at the
/// tick's first wait, `wait` from then); `None` once that has passed. An export waits as long as it takes.
fn lock_busy<'a, T: ?Sized>(m: &'a Mutex<T>, until: &mut Option<Instant>, wait: Duration, export: bool) -> Option<MutexGuard<'a, T>> {
    if export {
        return m.lock().ok();
    }
    loop {
        match m.try_lock() {
            Ok(g) => return Some(g),
            Err(TryLockError::Poisoned(_)) => return None,
            Err(TryLockError::WouldBlock) => {}
        }
        let now = Instant::now();
        if now >= *until.get_or_insert(now + wait) {
            return None;
        }
        std::thread::yield_now();
    }
}

/// A project edit a playing engine can take in place ([`Engine::apply_edit`]), so that moving a knob does not build a new
/// engine: the channels' volume, pan and pitch knobs and the mixer tracks' volume, pan and separation, by index, as
/// `Channel` and `MixerTrack` hold them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LiveEdit {
    ChannelVolume(usize, f64),
    ChannelPan(usize, f64),
    ChannelPitch(usize, i32),
    MixerVolume(usize, f64),
    MixerPan(usize, f64),
    MixerSeparation(usize, f64),
}

impl LiveEdit {
    /// `project` as this edit leaves it.
    pub fn apply_to(self, project: &mut Project) {
        let channel = |p: &mut Project, c: usize, f: &mut dyn FnMut(&mut os_model::Channel)| {
            if let Some(ch) = p.channels.get_mut(c) {
                f(ch);
            }
        };
        match self {
            LiveEdit::ChannelVolume(c, v) => channel(project, c, &mut |ch| ch.volume = v),
            LiveEdit::ChannelPan(c, p) => channel(project, c, &mut |ch| ch.pan = p),
            LiveEdit::ChannelPitch(c, cents) => channel(project, c, &mut |ch| ch.pitch_cents = cents),
            LiveEdit::MixerVolume(t, v) => {
                if let Some(m) = project.mixer.get_mut(t) {
                    m.volume = Some(v);
                }
            }
            LiveEdit::MixerPan(t, v) => {
                if let Some(m) = project.mixer.get_mut(t) {
                    m.pan = Some(v);
                }
            }
            LiveEdit::MixerSeparation(t, v) => {
                if let Some(m) = project.mixer.get_mut(t) {
                    m.stereo_separation = Some(v);
                }
            }
        }
    }
}

/// The edits that turn `sent` into `new`, when those are all that differ and there is one at least; `sent` is left with
/// them applied. `None`: something else differs, or nothing does, and a new engine is built as ever (it also reads the
/// program's settings). A track's level that goes unset is not such an edit.
pub fn live_edits(sent: &mut Project, new: &Project) -> Option<Vec<LiveEdit>> {
    if sent.channels.len() != new.channels.len() || sent.mixer.len() != new.mixer.len() {
        return None;
    }
    let mut edits = Vec::new();
    for (c, (a, b)) in sent.channels.iter().zip(&new.channels).enumerate() {
        if a.volume != b.volume {
            edits.push(LiveEdit::ChannelVolume(c, b.volume));
        }
        if a.pan != b.pan {
            edits.push(LiveEdit::ChannelPan(c, b.pan));
        }
        if a.pitch_cents != b.pitch_cents {
            edits.push(LiveEdit::ChannelPitch(c, b.pitch_cents));
        }
    }
    for (t, (a, b)) in sent.mixer.iter().zip(&new.mixer).enumerate() {
        if let (true, Some(v)) = (a.volume != b.volume, b.volume) {
            edits.push(LiveEdit::MixerVolume(t, v));
        }
        if let (true, Some(v)) = (a.pan != b.pan, b.pan) {
            edits.push(LiveEdit::MixerPan(t, v));
        }
        if let (true, Some(v)) = (a.stereo_separation != b.stereo_separation, b.stereo_separation) {
            edits.push(LiveEdit::MixerSeparation(t, v));
        }
    }
    if edits.is_empty() {
        return None;
    }
    for e in &edits {
        e.apply_to(sent);
    }
    (*sent == *new).then_some(edits)
}

/// Orders tracks so that every track is processed before any track it routes to.
fn processing_order(tracks: &[MixTrack]) -> Vec<usize> {
    fn visit(t: usize, tracks: &[MixTrack], state: &mut [u8], order: &mut Vec<usize>) {
        if state[t] != 0 {
            return; // done, or a cycle (ignored)
        }
        state[t] = 1;
        for r in &tracks[t].routes {
            visit(r.dest, tracks, state, order);
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
    /// Note fine pitch byte, 120 = none.
    fine: u8,
    velocity: u8,
    /// Note pan byte, 64 = center.
    pan: u8,
    /// Note mod X / mod Y bytes, 128 = none.
    mod_x: u8,
    mod_y: u8,
    /// Tick at which the voice is cut (audio clip end).
    cut: Option<u64>,
    /// Note length in ticks (0 = a step: it has no end of its own).
    len: u64,
    /// Seconds into the sample where the voice starts (audio clip window start).
    from_s: f64,
    /// Seconds into the sample where an audio clip's window ends (`None`: the sample's end).
    to_s: Option<f64>,
    /// An audio clip's fades and gain.
    clip: Option<ClipFx>,
    /// A slide note.
    slide: bool,
    /// The note's colour group.
    colour: u8,
    /// The note's own portamento switch.
    porta: bool,
    /// A note the main swing can move.
    swing: Option<Swung>,
}

impl Sched {
    fn note(tick: u64, n: &os_model::Note) -> Self {
        Sched {
            tick,
            channel: usize::from(n.channel),
            key: n.key,
            fine: n.fine_pitch,
            velocity: n.velocity,
            pan: n.pan,
            mod_x: n.mod_x,
            mod_y: n.mod_y,
            cut: None,
            len: u64::from(n.length),
            from_s: 0.0,
            to_s: None,
            clip: None,
            slide: n.slide(),
            colour: n.colour(),
            porta: n.porta(),
            swing: None,
        }
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
/// audio/pattern-controls.md, note repeat); a plain step counts as one step long for that. A note the swing can move
/// keeps what `Engine::set_swing` needs to move it again ([`Swung`]); its repeats keep their number.
fn push_note(out: &mut Vec<Sched>, start: i64, n: &os_model::Note, project: &Project, swing: bool, cap: Option<u64>) {
    let ppq = project.ppq.max(1);
    let share = swing_share(project, n.channel);
    let delay = if swing { swing_delay(n.position, ppq, project.swing.unwrap_or(0), share) } else { 0 };
    let first = start + i64::from(n.position) + i64::from(delay);
    if first < 0 {
        return;
    }
    let mut first = Sched::note(first as u64, n);
    // Truncate swing notes: a swung note with a length keeps its end (spec: audio/swing.md, Lemon Studio's rule).
    let keep_end = n.length > 0 && project.channels.get(usize::from(n.channel)).is_some_and(|c| c.misc.truncate_swing);
    if delay > 0 && keep_end {
        first.len = first.len.saturating_sub(u64::from(delay)).max(1);
    }
    // Swing can move it: in the first half of an odd step, on a channel with a share.
    let movable = swing && swing_delay(n.position, ppq, 128, share) > 0;
    let swung = |off: u64| {
        movable.then(|| Swung {
            base: first.tick - u64::from(delay) + off,
            position: n.position,
            share,
            delay,
            len: u64::from(n.length).saturating_sub(off),
            keep_end,
            cap,
        })
    };
    let span = if n.length > 0 { first.len } else { u64::from(ppq / 4).max(1) };
    let capped = |s: Sched| match cap {
        Some(c) if s.len > 0 => Sched { len: s.len.min(c.saturating_sub(s.tick)), ..s },
        _ => s,
    };
    match n.repeat_ticks(ppq) {
        Some(interval) => {
            let mut off = 0;
            while off < span {
                let len = if n.length > 0 { first.len - off } else { 0 };
                let s = capped(Sched { tick: first.tick + off, len, swing: swung(off), ..first });
                if n.length == 0 || s.len > 0 {
                    out.push(s);
                }
                off += u64::from(interval.max(1));
            }
        }
        None => {
            let s = capped(Sched { swing: swung(0), ..first });
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

/// Control points as (tick, target word, value): a pattern's own, or those of every pattern clip of the playlist
/// (spec: audio/pattern-controls.md, audio/song-structure.md "Control points").
fn schedule_controls(project: &Project, mode: PlayMode) -> Vec<(u64, u32, i32)> {
    let points = |n: u16| project.patterns.iter().find(|p| p.number == n).map(|p| p.controls.as_slice()).unwrap_or(&[]);
    let place = |out: &mut Vec<_>, n: u16| {
        for c in points(n) {
            out.push((u64::from(c.position), c.kind, c.value));
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
                        out.push((tick as u64, p.kind, p.value));
                    }
                }
            }
        }
    }
    out
}

/// What a control point's target word sets: a channel's own setting below the project's place, else the target an
/// automation link with that word names (spec: audio/automation.md "Pattern control points"). A channel's switch and
/// mixer track are set as a clip sets them (`point_level`).
fn ctl_to(kind: u32) -> CtlTo {
    use os_model::target::{self, CHANNEL_MIXER_TRACK, CHANNEL_ON, PLUGIN_PARAM};
    let (place, id) = ((kind >> 16) as u16, kind as u16);
    if place < 0x4000 && id < PLUGIN_PARAM && id != CHANNEL_ON && id != CHANNEL_MIXER_TRACK {
        return CtlTo::Channel(usize::from(place), id);
    }
    match target::decode(kind) {
        t @ (Target::ChannelParam(..) | Target::SlotParam(..)) => CtlTo::Param(t, None),
        t => CtlTo::Target(t),
    }
}

/// The automation level (0..1) a point's value in its setting's own unit stands for: the inverse of what a clip's
/// level becomes (`Engine::auto_value`; spec: audio/automation.md "Pattern control points"). `None` for the targets a
/// point does not move (a send's unit is not measured).
fn point_level(t: Target, value: i32) -> Option<f64> {
    let v = f64::from(value);
    Some(match t {
        Target::MixerVolume(_) => v / 16000.0,
        Target::MixerPan(_) => (v / 6400.0 + 1.0) / 2.0,
        Target::MixerSeparation(_) => (v / 64.0 + 1.0) / 2.0,
        Target::MixerEq(_, _, 0) => (v / 1800.0 + 1.0) / 2.0,
        Target::MixerEq(..) => v / 65536.0,
        Target::SlotOn(..) => v,
        Target::SlotMix(..) => v / 12800.0,
        // The channel rack's controls in their own units: the switch 0 / 1, the track number, the main swing 0..128.
        Target::ChannelOn(_) => v.clamp(0.0, 1.0),
        Target::ChannelMixerTrack(_) => os_model::target::mixer_track_level(value.clamp(0, 500) as u16),
        Target::MainSwing => v.clamp(0.0, 128.0) / 128.0,
        // The main pitch in cents (spec: audio/voice-levels.md "The main pitch").
        Target::MainPitch => (v.clamp(-1200.0, 1200.0) + 1200.0) / 2400.0,
        _ => return None,
    })
}

/// Gives the plugin-parameter points `mine` picks their level, from the parameter's range as the plugin states it
/// (`os_model::target::plugin_point_level`).
fn param_points(controls: &mut [CtlEvent], mine: impl Fn(Target) -> bool, range: impl Fn(usize) -> Option<(f64, f64)>) {
    for e in controls {
        if let CtlTo::Param(t, level) = &mut e.to {
            let (Target::ChannelParam(_, k) | Target::SlotParam(_, _, k)) = *t else { continue };
            if mine(*t) {
                *level = range(usize::from(k)).map(|(min, max)| os_model::target::plugin_point_level(min, max, e.value));
            }
        }
    }
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

/// A pattern's length in ticks, by the rule the app shares ([`Project::pattern_length`]): an explicit length wins;
/// otherwise its last note's end rounded up to a bar line of the pattern's own time signature, at least one bar (spec:
/// audio/song-structure.md, "Time signatures and a pattern's length"; audio/timing.md, "Where the song ends").
fn pattern_length(project: &Project, number: u16) -> u64 {
    u64::from(project.pattern_length(number))
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
            let truncated = project.play_truncated;
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
            // spec: audio/audio-clips.md "Automatic crossfades"
            let fades = os_model::crossfade::playing(project, clips, project.tempo.unwrap_or(140.0));
            for (c, &fx) in clips.iter().zip(&fades) {
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
                        // An audio clip plays its channel's sample from the window start (milliseconds) to the
                        // window end, either one negative meaning the whole sample (spec: audio/audio-clips.md
                        // "Settings"), cut at the clip end where the instrument does not end it there.
                        Some(ChannelKind::AudioClip | ChannelKind::Sampler) => {
                            let from_s = c.window.map_or(0.0, |w| w.0.max(0.0) / 1000.0);
                            let to_s = c.window.filter(|w| w.0 >= 0.0 && w.1 >= 0.0).map(|w| w.1 / 1000.0);
                            out.push(Sched { tick: start, channel: usize::from(ch), key: 60, fine: 120, velocity: 100, pan: 64, mod_x: 128, mod_y: 128, cut: Some(start + len), len, from_s, to_s, clip: ClipFx::of(c, fx), slide: false, colour: 0, porta: false, swing: None });
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
            id: os_model::Id::NONE,
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

    /// spec: plugins/vst3-hosting.md §1: the transport carries the project's time signature, and its bars and the
    /// pattern loop follow it (a pattern is whole bars of the project's bar, spec: audio/song-structure.md). The bar
    /// is events 17 and 18, beats per bar and steps per beat: 6/8 is (6, 2) (spec: flp/engine-events.md "Project").
    #[test]
    fn plugins_see_the_project_time_signature() {
        // 6/8: six beats of two sixteenths, three quarter notes. A time division of three beats of three steps has no
        // signature: told as its nine sixteenths, 9/16 (2.25 quarter notes). The 130 BPM count the bar's beats, so the
        // quarter notes the transport counts go at 130 × steps / 4 a minute (spec: audio/timing.md "The tempo counts
        // the bar's beats").
        for (bar, sig, quarters, tempo) in [((6, 2), (6, 8), 3.0, 65.0), ((3, 3), (9, 16), 2.25, 97.5)] {
            let fpb = 48000.0 * 60.0 / tempo;
            let mut p = project(vec![long_note(0, 96, 0)], vec![]);
            p.bar = Some(bar);
            p.channels[0].kind = ChannelKind::Plugin;
            let mut lib = SampleLibrary::default();
            let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Pattern(1));
            let inst = Arc::new(std::sync::Mutex::new(Clock::default()));
            e.set_instrument(0, inst.clone());
            e.set_looping(true);
            let mut out = vec![0.0f32; 512 * 2];
            e.process(&mut out);
            let t = inst.lock().unwrap().seen[0].0;
            // The one-bar pattern loops after a bar, and the beat after it is in bar 1.
            assert_eq!((t.numerator, t.denominator, t.looping), (sig.0, sig.1, Some((0.0, quarters))));
            assert_eq!(t.tempo, tempo);
            let later = TransportInfo { looping: None, ..t }.advanced(((quarters + 1.0) * fpb) as u64, 48000);
            assert_eq!((later.bar, later.bar_start), (1, quarters), "{later:?}");
        }
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

    // spec: audio/song-structure.md "A note": a step has no end of its own and is released with the song.
    #[test]
    fn a_step_on_an_instrument_is_released_with_the_song() {
        use os_model::{Clip, ClipSource};
        let mut p = song(vec![Note::step(0, 0), long_note(96, 48, 0)], vec![Clip::new(0, 384, 0, ClipSource::Pattern(1))]);
        p.channels[0].kind = ChannelKind::Plugin;
        let mut lib = SampleLibrary::default();
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Song);
        let probe = Arc::new(std::sync::Mutex::new(Probe { log: vec![], held: vec![], at: 0 }));
        e.set_instrument(0, probe.clone());
        let end = e.end_tick();
        e.render_all(512);
        drop(e);
        let at = |t: u64| laws::tick_to_frame(t as f64, 96, 130.0, 48000);
        let offs: Vec<(u64, u8)> = probe.lock().unwrap().log.iter().filter(|l| !l.1).map(|l| (l.0, l.2)).collect();
        // The long note ends at its end; the step only on the song's last tick, not after a sixteenth.
        assert_eq!(offs, vec![(at(144), 60), (at(end - 1), 60)], "song end at tick {end}");
    }

    /// Truncate swing notes: a swung note with a length keeps its end; without it the whole note moves (spec:
    /// audio/swing.md).
    #[test]
    fn truncate_swing_notes_keeps_the_end() {
        for (truncate, end) in [(false, 60.0), (true, 48.0)] {
            let mut p = project(vec![long_note(24, 24, 0)], vec![]);
            p.swing = Some(128);
            p.channels[0].kind = ChannelKind::Plugin;
            p.channels[0].misc.truncate_swing = truncate;
            let mut lib = SampleLibrary::default();
            let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Pattern(1));
            let probe = Arc::new(std::sync::Mutex::new(Probe { log: vec![], held: vec![], at: 0 }));
            e.set_instrument(0, probe.clone());
            let mut out = vec![0.0f32; 512 * 2];
            for _ in 0..100 {
                e.process(&mut out);
            }
            let log = probe.lock().unwrap().log.clone();
            let on = log.iter().find(|l| l.1).map(|l| l.0);
            let off = log.iter().find(|l| !l.1).map(|l| l.0);
            assert_eq!(on, Some(laws::tick_to_frame(36.0, 96, 130.0, 48000)), "{log:?}");
            assert_eq!(off, Some(laws::tick_to_frame(end, 96, 130.0, 48000)), "truncate {truncate}: {log:?}");
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

    /// Plays 1.0 on both sides and keeps the channel levels it was given; the engine applies volume and pan.
    struct LevelProbe(Vec<crate::instrument::ChannelLevels>);

    impl Instrument for LevelProbe {
        fn note_on(&mut self, _: u32, _: u8, _: f32) {}
        fn note_off(&mut self, _: u32, _: u8) {}
        fn process(&mut self, _: u32, left: &mut [f32], right: &mut [f32]) {
            left.fill(1.0);
            right.fill(1.0);
        }
        fn channel_levels(&mut self, levels: crate::instrument::ChannelLevels) -> bool {
            if self.0.last() != Some(&levels) {
                self.0.push(levels);
            }
            false
        }
    }

    // spec: audio/voice-levels.md "The channel's levels", "Final levels", "Values from automation"
    #[test]
    fn level_adjustments_scale_and_offset_the_channel() {
        use os_model::{AutoPoint, Automation, Clip, ClipSource, target};
        let mut p = song(vec![long_note(0, 384 * 4, 0)], vec![Clip::new(0, 384 * 4, 0, ClipSource::Pattern(1))]);
        p.channels[0].kind = ChannelKind::Plugin;
        // Pan offset 3200, multiplier 16640 (1.3: +6.6 dB), mod X offset 64, mod Y offset -128, as the model keeps them.
        p.channels[0].misc.levels = [0.5, 0.3, 0.25, -0.5];
        p.channels[0].sound.cutoff = 200;
        let mut lib = SampleLibrary::default();
        let e = Engine::new(&p, &mut lib, 48000, PlayMode::Song);
        let ch = &e.channels[0];
        let mult = os_dsp::level::curve(os_dsp::level::knob(16640, os_dsp::level::R12800));
        assert!((mult - 2.155).abs() < 0.01, "{mult}");
        assert_eq!(ch.volume, laws::channel_gain(0.78125) * f64::from(mult));
        assert_eq!(ch.pan_of(ch.pan), 0.5);
        assert_eq!(ch.levels(), crate::instrument::ChannelLevels { volume: ch.volume as f32, pan: 0.5, mod_x: 0.25, mod_y: -0.5 });
        // The pan and its offset are added as stored integers, then scaled once.
        assert_eq!(ch.pan_of(-0.25), f64::from(1600.0f32 * os_dsp::level::R6400));
        // A hosted instrument's output gets the channel's volume with the multiplier, panned with the offset.
        let render = |p: &Project| {
            let mut lib = SampleLibrary::default();
            let mut e = Engine::new(p, &mut lib, 48000, PlayMode::Song);
            let probe = Arc::new(Mutex::new(LevelProbe(vec![])));
            e.set_instrument(0, probe.clone());
            let mut out = Vec::new();
            for _ in 0..800 {
                out.extend_from_slice(e.next_tick().unwrap().0);
            }
            let levels = probe.lock().unwrap().0.clone();
            (out, levels)
        };
        let (out, _) = render(&p);
        let (l, r) = (out[1000], out[1001]);
        let (pl, pr) = laws::channel_pan(0.5);
        let g = laws::channel_gain(0.78125) * f64::from(mult);
        assert!((f64::from(l) - g * pl).abs() < 1e-6 && (f64::from(r) - g * pr).abs() < 1e-6, "{l} {r}");
        // Automation: the multiplier curve(Round(25600 v) / 12800), the pan offset Round(12800 v) - 6400, the cutoff
        // Round(256 v) moving mod X by its difference to the stored 200, the mod Y offset Round(512 v) - 256.
        let clips = [
            (target::CHANNEL_VOLUME_MULT, 0.75),
            (target::CHANNEL_PAN_OFFSET, 0.25),
            (target::CHANNEL_CUTOFF, 0.5),
            (target::CHANNEL_MOD_Y, 1.0),
        ];
        for (i, &(id, v)) in clips.iter().enumerate() {
            let index = i as u16 + 1;
            let mut c = channel();
            c.index = index;
            c.kind = ChannelKind::Automation;
            // A new clip's range, 0..1 (stored pan 0, volume 12800).
            (c.pan, c.volume) = (-1.0, 1.0);
            p.channels.push(c);
            let points = vec![AutoPoint::new(0.0, v), AutoPoint::new(8.0, v)];
            p.automations.push(Automation { channel: index, targets: vec![target::channel(0, id)], points, header: vec![0; 21], tail: vec![], lfo: Default::default() });
            p.arrangements[0].clips.push(Clip::new(384, 768, index, ClipSource::Channel(index)));
        }
        let (out, levels) = render(&p);
        let last = *levels.last().unwrap();
        let mult = os_dsp::level::curve(os_dsp::level::knob(19200, os_dsp::level::R12800));
        let g = laws::channel_gain(0.78125) * f64::from(mult);
        assert_eq!(last, crate::instrument::ChannelLevels { volume: g as f32, pan: -0.5, mod_x: (64.0 + 128.0 - 200.0) / 256.0, mod_y: 1.0 });
        assert_eq!(levels[0].pan, 0.5, "before the clips: as stored");
        let f = 2 * laws::tick_to_frame(600.0, 96, 130.0, 48000) as usize;
        let (pl, pr) = laws::channel_pan(-0.5);
        assert!((f64::from(out[f]) - g * pl).abs() < 1e-6 && (f64::from(out[f + 1]) - g * pr).abs() < 1e-6);
        // Starting before the clips: as stored again.
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Song);
        while e.tick <= 600 {
            e.next_tick();
        }
        e.seek_tick(10);
        assert_eq!(e.channels[0].levels().pan, 0.5);
        // Pattern control points move the filter knobs too (0..256).
        let mut ch = Engine::new(&p, &mut lib, 48000, PlayMode::Song).channels.remove(0);
        ch.apply(target::CHANNEL_RESONANCE, 64, 48000);
        assert_eq!(ch.levels().mod_y, -0.25);
    }

    // spec: audio/sampler.md "Gains": a control point's change reaches the voices as one gain ramp per tick, limited by
    // the instrument, not over a fixed glide time.
    #[test]
    fn instruments_get_a_control_points_value_on_its_tick() {
        let mut p = project(vec![long_note(0, 384, 0)], vec![]);
        p.channels[0].kind = ChannelKind::Plugin;
        p.patterns[0].controls = vec![ControlPoint::new(24, 0, ControlPoint::VOLUME, 3200), ControlPoint::new(48, 0, ControlPoint::PAN, 9600)];
        let mut lib = SampleLibrary::default();
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Pattern(1));
        let probe = Arc::new(Mutex::new(LevelProbe(vec![])));
        e.set_instrument(0, probe.clone());
        while e.tick <= 60 {
            e.next_tick();
        }
        let levels = probe.lock().unwrap().0.clone();
        let ch = &e.channels[0];
        let quarter = (laws::channel_gain(0.25) * f64::from(ch.adjust.mult)) as f32;
        let vols: Vec<f32> = levels.iter().map(|l| l.volume).collect();
        let pans: Vec<f32> = levels.iter().map(|l| l.pan).collect();
        assert_eq!(vols.iter().filter(|&&v| v != vols[0]).collect::<Vec<_>>(), vec![&quarter; 2], "no value on the way");
        assert_eq!(pans.iter().filter(|&&p| p != pans[0]).count(), 1, "{pans:?}");
        assert_eq!(*pans.last().unwrap(), ch.pan_of(0.5) as f32);
    }

    /// spec: audio/implementing.md "Playing live": an instrument the UI thread holds for a moment is waited for, so it
    /// misses no frame and no note.
    #[test]
    fn instruments_busy_for_a_moment_are_waited_for() {
        let mut p = project(vec![long_note(0, 48, 0)], vec![]);
        p.channels[0].kind = ChannelKind::Plugin;
        let mut lib = SampleLibrary::default();
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Pattern(1));
        e.busy_wait = Duration::from_secs(30);
        let probe = Arc::new(std::sync::Mutex::new(Probe { log: vec![], held: vec![], at: 0 }));
        e.set_instrument(0, probe.clone());
        let held = probe.lock().unwrap();
        let render = std::thread::spawn(move || {
            let mut out = vec![0.0f32; 512 * 2];
            e.process(&mut out);
            (e, out)
        });
        std::thread::sleep(Duration::from_millis(50));
        drop(held);
        let (mut e, out) = render.join().unwrap();
        assert_eq!(e.take_missed(), 0);
        let p = probe.lock().unwrap();
        assert_eq!(p.at, e.frame, "every rendered frame was processed");
        assert_eq!(p.log.iter().map(|l| (l.0, l.1)).collect::<Vec<_>>(), vec![(0, true)]);
        assert!(out.iter().all(|v| *v > 0.0), "the note sounds from the first frame");
    }

    /// An instrument still busy after the wait misses the tick: its notes wait and its frames are made up when it is
    /// reached again, so it plays on in time (heard as a gap, not as a shift).
    #[test]
    fn instruments_busy_too_long_catch_up() {
        let mut p = project(vec![long_note(0, 96, 0)], vec![]);
        p.channels[0].kind = ChannelKind::Plugin;
        let mut lib = SampleLibrary::default();
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Pattern(1));
        e.busy_wait = Duration::ZERO;
        let probe = Arc::new(std::sync::Mutex::new(Probe { log: vec![], held: vec![], at: 0 }));
        e.set_instrument(0, probe.clone());
        let mut out = vec![0.0f32; 512 * 2];
        let held = probe.lock().unwrap();
        e.process(&mut out);
        drop(held);
        let missed = e.take_missed();
        assert!(missed >= 2, "{missed}");
        assert!(out.iter().all(|v| *v == 0.0), "a gap while it is busy");
        assert_eq!(probe.lock().unwrap().at, 0);
        e.process(&mut out);
        let p = probe.lock().unwrap();
        assert_eq!(p.at, e.frame, "in time again");
        assert_eq!(p.log.iter().map(|l| (l.0, l.1)).collect::<Vec<_>>(), vec![(0, true)], "the note it missed, where it was due");
        assert!(out[out.len() - 2] > 0.0, "playing again");
        assert_eq!(e.take_missed(), 0);
    }

    /// A sampler channel played by a hosted sampler (Lemon Studio's Lemon Sampler): its notes go to the instrument, the
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

    /// A slow-decaying tail: follows the loudest input and lets it fall away (a reverb's tail, for the test).
    struct Ring(f32);

    impl crate::instrument::HostedEffect for Ring {
        fn process_fx(&mut self, _rate: u32, left: &mut [f32], right: &mut [f32]) {
            for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                self.0 = (self.0 * 0.9995).max(l.abs());
                *l = self.0;
                *r = self.0;
            }
        }

        fn reset_fx(&mut self) {
            self.0 = 0.0;
        }
    }

    /// While held (paused, or a Stop's tails ringing out), an effect the UI thread holds for a moment is waited for on
    /// every tick, not only until the first tick's wait is used up.
    #[test]
    fn held_ticks_wait_for_busy_effects() {
        let mut p = project(vec![long_note(0, 384, 0)], vec![]);
        p.channels[0].mixer_track = 1;
        p.mixer = (0..2).map(os_model::MixerTrack::new).collect();
        let mut lib = SampleLibrary::default();
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Pattern(1));
        e.busy_wait = Duration::from_secs(30);
        let fx = Arc::new(Mutex::new(Ring(0.0)));
        e.set_effect(1, 0, fx.clone());
        for _ in 0..4 {
            e.next_tick();
        }
        e.pause();
        while !e.held {
            e.next_tick();
        }
        // The wait of an earlier tick, used up.
        e.busy_until = Some(Instant::now());
        let held = fx.lock().unwrap();
        let render = std::thread::spawn(move || {
            e.next_tick();
            e
        });
        std::thread::sleep(Duration::from_millis(20));
        drop(held);
        let mut e = render.join().unwrap();
        assert_eq!(e.take_missed(), 0, "the held tick waited for the effect");
    }

    // spec: audio/transport.md "Pause", "Tails after Pause and Stop"
    #[test]
    fn pause_holds_the_position_and_lets_the_plugins_ring_out() {
        let mut p = project(vec![long_note(0, 384 * 4, 0)], vec![]);
        p.channels[0].kind = ChannelKind::Plugin;
        p.channels[0].mixer_track = 1;
        p.mixer = (0..2).map(os_model::MixerTrack::new).collect();
        let mut lib = SampleLibrary::default();
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Pattern(1));
        let probe = Arc::new(std::sync::Mutex::new(Probe { log: vec![], held: vec![], at: 0 }));
        e.set_instrument(0, probe.clone());
        e.set_effect(1, 0, Arc::new(Mutex::new(Ring(0.0))));
        let peak = |e: &mut Engine, ticks: usize| {
            let mut m = 0.0f32;
            for _ in 0..ticks {
                if let Some((s, ..)) = e.next_tick() {
                    m = s.iter().fold(m, |m, v| m.max(v.abs()));
                }
            }
            m
        };
        let playing = peak(&mut e, 20);
        assert!(playing > 0.1, "the note sounds through the effect: {playing}");
        e.pause();
        // About 6 ms of fade (a tick is about 230 frames), then the channels are held where they are.
        peak(&mut e, 4);
        let (tick, at) = (e.tick, probe.lock().unwrap().at);
        assert!(e.paused() && !e.finished());
        // Held ticks are handed out at the held position, which stays put all through them.
        for _ in 0..3 {
            let (_, t, f, held, _) = e.next_tick().unwrap();
            assert_eq!((t, f, held), (tick as f64, e.frame, true), "a held tick");
            assert_eq!((e.tick_position(), e.position()), (tick as f64, e.frame), "the position holds");
        }
        let tail = peak(&mut e, 40);
        assert!(tail > 0.01 && tail < playing, "the effect's tail rings on while paused: {tail}");
        let probe_now = probe.lock().unwrap();
        assert_eq!(e.tick, tick, "the position is held");
        assert!(probe_now.at > at, "the instrument goes on being processed");
        assert!(probe_now.held.is_empty(), "its note was let go of: {:?}", probe_now.held);
        drop(probe_now);
        // Resume: playback goes on from where it was held; the note let go of does not come back.
        e.resume();
        assert!(!e.paused());
        let frame = e.frame;
        let (_, t, f, held, _) = e.next_tick().unwrap();
        assert_eq!((t, f, held), (tick as f64, frame, false), "goes on from the held tick and frame");
        peak(&mut e, 20);
        assert!(e.tick > tick && probe.lock().unwrap().held.is_empty(), "the note is not started again");
    }

    /// A key held twice at a pause (a note still sounding as another starts the same key, as two clips meeting do):
    /// the instrument gets a note off for each, so neither voice rings on while paused.
    #[test]
    fn a_key_held_twice_gets_two_note_offs_at_a_pause() {
        /// Holds one voice per note on, and lets one go per note off.
        struct Voices(Vec<u8>);
        impl crate::Instrument for Voices {
            fn note_on(&mut self, _offset: u32, key: u8, _velocity: f32) {
                self.0.push(key);
            }
            fn note_off(&mut self, _offset: u32, key: u8) {
                if let Some(i) = self.0.iter().position(|&k| k == key) {
                    self.0.remove(i);
                }
            }
            fn process(&mut self, _rate: u32, l: &mut [f32], r: &mut [f32]) {
                let v = if self.0.is_empty() { 0.0 } else { 1.0 };
                l.fill(v);
                r.fill(v);
            }
        }
        let mut p = project(vec![long_note(0, 384, 0), long_note(96, 384, 0)], vec![]);
        p.channels[0].kind = ChannelKind::Plugin;
        let mut lib = SampleLibrary::default();
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Pattern(1));
        let inst = Arc::new(std::sync::Mutex::new(Voices(Vec::new())));
        e.set_instrument(0, inst.clone());
        while e.tick < 120 {
            e.next_tick();
        }
        assert_eq!(inst.lock().unwrap().0.len(), 2, "both notes sound");
        e.pause();
        for _ in 0..10 {
            e.next_tick();
        }
        assert!(e.held());
        assert_eq!(inst.lock().unwrap().0, Vec::<u8>::new(), "every voice let go of");
    }

    /// An instrument whose notes fade out over its release: a level that falls by `fall` each frame once let go of.
    struct Release {
        level: f32,
        on: bool,
        fall: f32,
    }

    impl crate::Instrument for Release {
        fn note_on(&mut self, _offset: u32, _key: u8, _velocity: f32) {
            (self.level, self.on) = (1.0, true);
        }
        fn note_off(&mut self, _offset: u32, _key: u8) {
            self.on = false;
        }
        fn process(&mut self, _rate: u32, l: &mut [f32], r: &mut [f32]) {
            for (a, b) in l.iter_mut().zip(r.iter_mut()) {
                (*a, *b) = (self.level, self.level);
                if !self.on {
                    self.level *= self.fall;
                }
            }
        }
    }

    /// A hosted instrument's release rings on through a pause, decaying as it would have, and is not cut by the
    /// pause's fade (spec: audio/transport.md "Tails after Pause and Stop": FL processes the instruments while paused).
    #[test]
    fn an_instruments_release_rings_on_while_paused() {
        let mut p = project(vec![long_note(0, 384 * 4, 0)], vec![]);
        p.channels[0].kind = ChannelKind::Plugin;
        let mut lib = SampleLibrary::default();
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Pattern(1));
        // About -60 dB after one second.
        let fall = (1e-3f32).powf(1.0 / 48000.0);
        e.set_instrument(0, Arc::new(std::sync::Mutex::new(Release { level: 0.0, on: false, fall })));
        let peaks = |e: &mut Engine, ticks: usize| -> Vec<f32> {
            let mut v = Vec::new();
            for _ in 0..ticks {
                if let Some((s, ..)) = e.next_tick() {
                    v.push(s.iter().fold(0.0f32, |m, x| m.max(x.abs())));
                }
            }
            v
        };
        let playing = peaks(&mut e, 20);
        assert!(playing.iter().all(|&v| v > 0.1), "the note sounds: {playing:?}");
        e.pause();
        let paused = peaks(&mut e, 60);
        assert!(e.held(), "held");
        // Every tick of the pause: no drop to silence at the fade, a release decaying smoothly.
        assert!(paused[0] >= playing[0] * 0.99, "not faded out: {} of {}", paused[0], playing[0]);
        assert!(paused.windows(2).all(|w| w[1] <= w[0] && w[1] > 0.0), "decays: {paused:?}");
        let last = *paused.last().unwrap();
        assert!(last < paused[5] * 0.9, "the release goes on while paused: {} -> {last}", paused[5]);
    }

    /// An instrument's editor is open: the live engine keeps processing it with no note from the host, so a note
    /// from the editor's own keyboard sounds; closed, the engine is over once silent.
    #[test]
    fn an_awake_live_engine_processes_its_instruments() {
        let mut p = project(vec![], vec![]);
        p.channels[0].kind = ChannelKind::Plugin;
        let mut lib = SampleLibrary::default();
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Pattern(1));
        e.clear_schedule();
        let probe = Arc::new(std::sync::Mutex::new(Probe { log: vec![], held: vec![], at: 0 }));
        e.set_instrument(0, probe.clone());
        assert!(e.next_tick().is_none(), "idle live engine renders nothing");
        e.set_awake(true);
        let quiet = e.next_tick().map(|(s, ..)| s.iter().all(|&v| v == 0.0));
        assert_eq!(quiet, Some(true), "awake: rendered, silent");
        assert!(probe.lock().unwrap().at > 0, "the instrument was processed");
        // A key pressed in the plugin's editor (the host sent no note).
        probe.lock().unwrap().held.push(70);
        let loud = e.next_tick().map(|(s, ..)| s.iter().any(|&v| v != 0.0));
        assert_eq!(loud, Some(true), "the editor's note is heard");
        probe.lock().unwrap().held.clear();
        e.set_awake(false);
        let mut ticks = 0;
        while e.next_tick().is_some() {
            ticks += 1;
            assert!(ticks < 10_000, "finishes once silent");
        }
    }

    /// A paused playback whose instruments another stream processes lets go of its held notes, once.
    #[test]
    fn released_instrument_notes_end_once() {
        let mut p = project(vec![], vec![]);
        p.channels[0].kind = ChannelKind::Plugin;
        let mut lib = SampleLibrary::default();
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Pattern(1));
        let probe = Arc::new(std::sync::Mutex::new(Probe { log: vec![], held: vec![], at: 0 }));
        e.set_instrument(0, probe.clone());
        e.note_on(0, 64, 100);
        let mut out = vec![0.0f32; 256];
        e.process(&mut out);
        assert_eq!(probe.lock().unwrap().held, [64]);
        e.release_instruments();
        assert!(probe.lock().unwrap().held.is_empty(), "released at once");
        e.note_off(0, 64);
        drop(e);
        let offs = probe.lock().unwrap().log.iter().filter(|l| !l.1).count();
        assert_eq!(offs, 1, "one note off");
    }

    /// A controller's pitch bend (spec: ui/midi-settings.md §4): a native channel bends ±200 cents linearly; an
    /// instrument channel's plugin gets the message on the next tick instead.
    #[test]
    fn pitch_bend_from_a_controller() {
        let mut p = project(vec![], vec![]);
        let mut lib = SampleLibrary::default();
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Pattern(1));
        let base = e.channels[0].pitch_from_60();
        for (bytes, cents) in [([0xe0, 0x7f, 0x7f], 200.0), ([0xe0, 0, 0], -200.0), ([0xe0, 0, 0x60], 100.0), ([0xe0, 0, 0x40], 0.0)] {
            e.midi(0, bytes, 200.0);
            let got = e.channels[0].pitch_from_60() - base;
            assert!((got - cents).abs() < 0.1, "{bytes:?}: {got} cents, not {cents}");
        }
        p.channels[0].kind = ChannelKind::Plugin;
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Pattern(1));
        let probe = Arc::new(std::sync::Mutex::new(Probe { log: vec![], held: vec![], at: 0 }));
        e.set_instrument(0, probe.clone());
        e.midi(0, [0xe0, 0x7f, 0x7f], 200.0);
        assert_eq!((e.inst_midi.len(), e.channels[0].bend), (1, 0.0), "queued for the plugin");
        let mut out = vec![0.0f32; 256];
        e.process(&mut out);
        assert!(e.inst_midi.is_empty(), "delivered");
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

    // spec: audio/song-structure.md "Slide notes", audio/note-rules.md "Glides"
    #[test]
    fn slide_notes_glide_the_sounding_voice_and_make_no_sound() {
        let slide = |position: u32, length: u32, key: u16, velocity: u8| Note { key, velocity, flags: 0x4000 | Note::SLIDE, ..long_note(position, length, 0) };
        // A note on key 60, a slide note to 72 at tick 96 lasting 3 ticks with a lower velocity, then one back to 60.
        let p = project(vec![long_note(0, 384, 0), slide(96, 3, 72, 50), slide(200, 0, 60, 100)], vec![]);
        let mut lib = SampleLibrary::new(None);
        let mut e = Engine::new(&p, &mut lib, 44100, PlayMode::Pattern(1));
        let after = |e: &mut Engine, tick: u64| {
            while e.tick <= tick {
                e.next_tick();
            }
            assert_eq!(e.voices.len(), 1, "slide notes start no voice");
            (e.voices[0].step, e.voices[0].vel)
        };
        let (rate, vel) = after(&mut e, 95);
        // Four steps of 300 cents, the first on the slide's own tick: an octave up after tick 99.
        let (r96, v96) = after(&mut e, 96);
        assert!((r96 / rate - 2f64.powf(0.25)).abs() < 1e-6, "{}", r96 / rate);
        assert!(v96 < vel);
        let (r99, v99) = after(&mut e, 99);
        assert!((r99 / rate - 2.0).abs() < 1e-5, "{}", r99 / rate);
        assert!((v99 - laws::velocity_gain(50)).abs() < 1e-6);
        assert_eq!(after(&mut e, 150).0, r99, "the glide is over");
        // Length 0: the whole way in one step on its tick; the velocity goes back too.
        let (r200, v200) = after(&mut e, 200);
        assert!((r200 / rate - 1.0).abs() < 1e-5 && (v200 - vel).abs() < 1e-6);
        // Without a sounding voice a slide note does nothing and is not heard.
        let quiet = project(vec![slide(0, 96, 72, 100)], vec![]);
        assert_eq!(peak(&render(&quiet), 0), 0.0);
        // Only the voices of the slide note's colour move: one of colour 2 leaves a voice of colour 0 where it is.
        let other = Note { group: 2, ..slide(96, 0, 72, 50) };
        let p = project(vec![long_note(0, 384, 0), other], vec![]);
        let mut e = Engine::new(&p, &mut lib, 44100, PlayMode::Pattern(1));
        assert_eq!(after(&mut e, 95), after(&mut e, 120), "another colour's slide moves nothing");
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
        assert_eq!(c, vec![(24, u32::from(ControlPoint::VOLUME), 6400), (96, 3 << 16 | u32::from(ControlPoint::PAN), 0)]);
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

    /// Control points on a mixer track's volume, a slot's mix level and the tempo set them in their own units from
    /// their tick on; seeking back before them puts the track's own values back (spec: audio/automation.md "Pattern
    /// control points").
    #[test]
    fn points_set_mixer_values_and_the_tempo() {
        use os_model::target;
        let point = |position: u32, kind: u32, value: i32| ControlPoint { position, kind, value };
        let mut p = project(vec![long_note(0, 384, 0)], vec![]);
        p.mixer = (0..3).map(os_model::MixerTrack::new).collect();
        p.patterns[0].controls = vec![
            point(24, target::mixer(1, target::MIXER_VOLUME), 8000),
            point(24, target::mixer(2, target::MIXER_PAN), -3200),
            point(24, target::slot(1, 2, target::SLOT_MIX), 6400),
            point(48, target::TEMPO, 150_000),
        ];
        let mut lib = SampleLibrary::new(None);
        let mut e = Engine::new(&p, &mut lib, 44100, PlayMode::Pattern(1));
        let mut out = vec![0.0f32; 512];
        while e.tick < 30 {
            e.process(&mut out);
        }
        assert_eq!((e.tracks[1].vol, e.tracks[2].pan, e.tracks[1].slot_mix[2]), (8000, -3200, 6400));
        assert_eq!(e.tempo, 130.0);
        while e.tick < 50 {
            e.process(&mut out);
        }
        assert_eq!(e.tempo, 150.0);
        e.seek(0);
        assert_eq!((e.tracks[1].vol, e.tracks[1].slot_mix[2]), (12800, 12800), "back before the points");
    }

    fn song(notes: Vec<Note>, clips: Vec<os_model::Clip>) -> Project {
        let mut p = project(notes, vec![]);
        p.arrangements = vec![os_model::Arrangement {
            clips,
            tracks: (1..=4).map(os_model::PlaylistTrack::new).collect(),
            ..Default::default()
        }];
        p
    }

    /// A pattern's bars are its own (spec: audio/song-structure.md, "Time signatures and a pattern's length"): the
    /// table's rows at 96 PPQ, the last note ending at 300, as the pattern's length in pattern mode.
    #[test]
    fn pattern_length_follows_the_patterns_own_time_signature() {
        use os_model::{Clip, TimeMarker};
        let sig = |position: u32, n: u8, d: u8| TimeMarker { kind: TimeMarker::SIGNATURE, numerator: n, denominator: d, ..TimeMarker::new(position, &format!("{n}/{d}")) };
        let ending = |end: u32| vec![Note { length: end - 192, ..Note::step(0, 192) }];
        let len = |p: &Project| Engine::new(p, &mut SampleLibrary::new(None), 48000, PlayMode::Pattern(1)).end_tick();
        // The arrangement has a clip of the pattern at bar 3.
        let at = |notes: Vec<Note>, project_sig: Option<(u16, u16)>, pattern: Vec<TimeMarker>, arrangement: Vec<TimeMarker>| {
            let mut p = song(notes, vec![Clip::new(768, 384, 0, ClipSource::Pattern(1))]);
            p.bar = project_sig.map(|(n, d)| (n, 16 / d));
            p.patterns[0].markers = pattern;
            p.arrangements[0].markers = arrangement;
            len(&p)
        };
        assert_eq!(at(ending(300), None, vec![], vec![]), 384, "4/4, no marker");
        assert_eq!(at(ending(300), Some((3, 4)), vec![], vec![]), 576, "project in 3/4");
        assert_eq!(at(ending(300), None, vec![], vec![sig(0, 3, 4)]), 384, "3/4 marker in the arrangement at 0");
        assert_eq!(at(ending(300), None, vec![], vec![sig(768, 3, 4)]), 384, "3/4 marker in the arrangement at the clip");
        assert_eq!(at(ending(300), None, vec![sig(0, 3, 4)], vec![]), 576, "3/4 marker at the pattern's start");
        assert_eq!(at(ending(500), None, vec![sig(0, 3, 4)], vec![]), 576, "a note ending at 500");
        assert_eq!(at(ending(300), None, vec![sig(192, 3, 4)], vec![]), 480, "3/4 marker at tick 192: 192 + 288");
        // Ending exactly on a bar line; an empty pattern is one bar of its own signature; the explicit length wins.
        assert_eq!(at(ending(384), None, vec![], vec![]), 384);
        assert_eq!(at(vec![], None, vec![sig(0, 3, 8)], vec![]), 144);
        let mut p = project(ending(300), vec![]);
        p.patterns[0].markers = vec![sig(0, 3, 4)];
        p.patterns[0].length = Some(192);
        assert_eq!(len(&p), 192);
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
        p.play_truncated = true;
        assert_eq!(song_notes(&p), vec![(384, Some(484)), (434, Some(484))], "on: the earlier note from the clip start, both cut at its end");
    }

    // spec: plugins/lemon-sampler.md §7 (playlist clips through a hosted sampler: sample start and choke)
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
        let wav = std::env::temp_dir().join(format!("os-engine-clip-start-{}.wav", std::process::id()));
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

    // spec: plugins/lemon-sampler.md "Keep on disk (Lemon Studio)"
    /// A channel its hosted instrument plays from the start keeps only its sample's length and rate, not the decoded
    /// frames, where the library's reader gives them. The instrument is told the same notes at the same frames and the
    /// same places in the sample as when the engine holds the decode (trimmed clips, playback started inside a clip, a
    /// file at another rate than the output, a stretch time the instrument makes or the engine would), and the render
    /// is the same to the bit. The library lets go of a decode it held, and reads a file again when it changes. A
    /// channel without an instrument still decodes its sample, and so does every channel when there is no reader.
    #[test]
    fn hosted_samples_keep_only_their_length() {
        use os_model::{stretch_mode, Clip, ClipSource};
        #[derive(Default)]
        struct Log(Vec<(u64, &'static str, u8, f32)>, u64);
        impl crate::Instrument for Log {
            fn note_on(&mut self, o: u32, k: u8, _v: f32) {
                self.0.push((self.1 + u64::from(o), "on", k, 0.0));
            }
            fn note_off(&mut self, o: u32, k: u8) {
                self.0.push((self.1 + u64::from(o), "off", k, 0.0));
            }
            fn note_on_from(&mut self, o: u32, k: u8, _v: f32, start: f32) {
                self.0.push((self.1 + u64::from(o), "from", k, start));
            }
            fn choke(&mut self, o: u32, k: u8) {
                self.0.push((self.1 + u64::from(o), "choke", k, 0.0));
            }
            fn process(&mut self, _rate: u32, l: &mut [f32], _r: &mut [f32]) {
                self.1 += l.len() as u64;
            }
        }
        fn read(path: &std::path::Path) -> Option<(usize, u32)> {
            crate::sample::length(path).map(|(n, r)| (n as usize, r))
        }
        // `frames` frames of a ramp at `rate`.
        let write = |wav: &std::path::Path, rate: u32, frames: u32| {
            let ramp: Vec<f32> = (0..frames).flat_map(|i| [i as f32 / frames as f32; 2]).collect();
            crate::wav::write_f32_stereo(&mut std::fs::File::create(wav).unwrap(), rate, &ramp).unwrap();
        };
        // The hosted channels' file: one second at 44.1 kHz (the output runs at 48 kHz). The channel without an
        // instrument has a file of its own.
        let hosted_wav = std::env::temp_dir().join(format!("os-engine-hosted-length-{}.wav", std::process::id()));
        let own_wav = std::env::temp_dir().join(format!("os-engine-hosted-own-{}.wav", std::process::id()));
        write(&hosted_wav, 44100, 44100);
        write(&own_wav, 48000, 48000);
        // Channel 0 plays audio clips: one trimmed 250 ms in, one from the top. Channel 1 is a sampler whose notes are
        // fitted to two beats, channel 2 one without an instrument.
        let mut trimmed = Clip::new(0, 96, 0, ClipSource::Channel(0));
        trimmed.window = Some((250.0, 711.0));
        let notes = vec![Note { channel: 1, ..long_note(0, 96, 0) }, Note { channel: 2, key: 64, ..long_note(96, 96, 0) }];
        let mut p = song(notes, vec![trimmed, Clip::new(192, 192, 0, ClipSource::Channel(0)), Clip::new(0, 384, 1, ClipSource::Pattern(1))]);
        p.channels = (0..3u16).map(|i| Channel { index: i, ..channel() }).collect();
        p.channels[0].kind = ChannelKind::AudioClip;
        for (c, wav) in [(0, &hosted_wav), (1, &hosted_wav), (2, &own_wav)] {
            p.channels[c].sample_path = Some(wav.to_string_lossy().into_owned());
        }
        p.channels[1].sound.stretch[0] = (2.0f32 * 192.0).to_bits();
        let logs = || [Arc::new(std::sync::Mutex::new(Log::default())), Arc::new(std::sync::Mutex::new(Log::default()))];
        // The engine with the decode (the instruments attached once it is built, as before) or with the length alone.
        let build = |p: &Project, lib: &mut SampleLibrary, hosted: bool, logs: &[Arc<std::sync::Mutex<Log>>; 2]| -> Engine {
            let list: Vec<(usize, SharedInstrument)> = logs.iter().enumerate().map(|(row, l)| (row, l.clone() as SharedInstrument)).collect();
            if hosted {
                return Engine::with_instruments(p, lib, 48000, PlayMode::Song, &list);
            }
            let mut e = Engine::new(p, lib, 48000, PlayMode::Song);
            for (row, i) in list {
                e.set_instrument(row, i);
            }
            e
        };
        // What the channels' lengths decide, and the notes' places in their samples.
        let shape = |e: &Engine| {
            let channels: Vec<_> = e.channels.iter().map(|c| (c.played(), c.src_ratio, c.base_rate, c.from_scale, c.rules.ring)).collect();
            let triggers: Vec<_> = e.triggers.iter().map(|t| (t.tick, t.channel, t.from, t.start)).collect();
            (channels, triggers)
        };
        let bits = |a: &[f32], b: &[f32]| a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits());
        // A realtime method the instrument plays (the host's library resamples it), and the same with the engine making
        // its copies, a method that stretches the whole sample, and resampling.
        for (mode, resample) in [(stretch_mode::STRETCH, true), (stretch_mode::STRETCH, false), (stretch_mode::E3_GENERIC, false), (stretch_mode::RESAMPLE, false)] {
            p.channels[1].sound.stretch[3] = mode as u32;
            let what = format!("mode {mode}, resampled {resample}");
            let mut runs = Vec::new();
            for hosted in [false, true] {
                let mut lib = SampleLibrary::new(None);
                lib.resample_stretches = resample;
                lib.length_reader = Some(read);
                let (top, inside) = (logs(), logs());
                let mut e = build(&p, &mut lib, hosted, &top);
                let summary = shape(&e);
                if hosted {
                    assert!(e.channels[..2].iter().all(|c| c.sample.is_none() && c.keyed.is_empty()), "{what}: no frames held");
                } else {
                    assert!(e.channels[..2].iter().all(|c| c.sample.is_some()), "{what}");
                }
                assert!(e.channels[2].sample.is_some(), "{what}: the channel without an instrument decodes its sample");
                let out = e.render_all(480);
                // Started inside the second clip (ticks 192 to 384).
                let mut e = build(&p, &mut lib, hosted, &inside);
                e.seek_tick(240);
                let late = e.render_all(480);
                let events = |l: &[Arc<std::sync::Mutex<Log>>; 2]| l.iter().map(|l| l.lock().unwrap().0.clone()).collect::<Vec<_>>();
                runs.push((summary, out, late, events(&top), events(&inside)));
            }
            let (memory, length) = (&runs[0], &runs[1]);
            assert_eq!(memory.0, length.0, "{what}: lengths, rates, trims");
            assert!(bits(&memory.1, &length.1) && bits(&memory.2, &length.2), "{what}: the same render");
            assert_eq!(memory.3, length.3, "{what}: the notes from the top");
            assert_eq!(memory.4, length.4, "{what}: the notes started inside a clip");
            assert_eq!(length.3[0][0], (0, "from", 60, 0.25), "{what}: the trimmed clip starts 250 ms in");
            assert!(length.4[0].iter().any(|n| n.1 == "from" && n.3 > 0.0), "{what}: the clip under way starts where it has got to");
            assert!(length.3[1].iter().any(|n| n.1 == "on"), "{what}: the sampler's note");
        }

        // The library lets go of the decode it held once a hosted channel needs only the length.
        let mut lib = SampleLibrary::new(None);
        lib.resample_stretches = true;
        lib.length_reader = Some(read);
        let e = Engine::new(&p, &mut lib, 48000, PlayMode::Song);
        let decode = Arc::downgrade(e.channels[0].sample.as_ref().unwrap());
        drop(e);
        assert!(decode.upgrade().is_some() && crate::sample::held(&hosted_wav), "the library keeps what it decoded");
        let e = build(&p, &mut lib, true, &logs());
        assert!(decode.upgrade().is_none() && !crate::sample::held(&hosted_wav), "nothing holds the decode any more");
        assert_eq!(e.channels[0].played(), Some((44100, 44100)));
        // A changed file is read again.
        write(&hosted_wav, 44100, 22050);
        let e = build(&p, &mut lib, true, &logs());
        assert_eq!(e.channels[0].played(), Some((22050, 44100)));
        // Without a reader, hosted channels decode their sample.
        let e = build(&p, &mut SampleLibrary::new(None), true, &logs());
        assert!(e.channels.iter().all(|c| c.sample.is_some() && c.length.is_none()));
        for wav in [hosted_wav, own_wav] {
            let _ = std::fs::remove_file(wav);
        }
    }

    /// A hosted sampler that loops its sample (user report 2026-10-26: a single-cycle wave with its file's `smpl` loop
    /// rang on for ever): its voices are not one-shot voices, so the note's end reaches the instrument when the note
    /// ends, however short the sample. A sample without a loop, or with "Use loop points" off, keeps its one-shot
    /// voices, whose note ends never reach it. The channel's own loop points loop it too.
    #[test]
    fn a_looping_hosted_sampler_gets_its_note_ends() {
        #[derive(Default)]
        struct Log(Vec<(u64, bool)>, u64);
        impl crate::Instrument for Log {
            fn note_on(&mut self, o: u32, _k: u8, _v: f32) {
                self.0.push((self.1 + u64::from(o), true));
            }
            fn note_off(&mut self, o: u32, _k: u8) {
                self.0.push((self.1 + u64::from(o), false));
            }
            fn process(&mut self, _rate: u32, l: &mut [f32], _r: &mut [f32]) {
                self.1 += l.len() as u64;
            }
        }
        fn read(path: &std::path::Path) -> Option<(usize, u32)> {
            crate::sample::length(path).map(|(n, r)| (n as usize, r))
        }
        // 550 frames: about 11 ms, played out long before the note's end.
        let wav = std::env::temp_dir().join(format!("os-engine-looped-{}.wav", std::process::id()));
        crate::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &vec![0.25; 1100]).unwrap();
        let mut p = project(vec![long_note(0, 96, 0)], vec![]);
        p.channels[0].sample_path = Some(wav.to_string_lossy().into_owned());
        // The note ends one beat in, at 130 BPM.
        let end = (48000.0 * 60.0 / 130.0) as u64;
        let offs = |p: &Project, loops: fn(&std::path::Path) -> bool| {
            let mut lib = SampleLibrary::new(None);
            lib.length_reader = Some(read);
            lib.loop_reader = Some(loops);
            let log = Arc::new(std::sync::Mutex::new(Log::default()));
            let mut e = Engine::with_instruments(p, &mut lib, 48000, PlayMode::Pattern(1), &[(0, log.clone() as SharedInstrument)]);
            e.render_all(480);
            let offs: Vec<u64> = log.lock().unwrap().0.iter().filter(|n| !n.1).map(|n| n.0).collect();
            offs
        };
        let at_end = |o: &[u64]| o.len() == 1 && o[0].abs_diff(end) < 480;
        let o = offs(&p, |_| true);
        assert!(at_end(&o), "the file's loop: {o:?} (end {end})");
        assert!(offs(&p, |_| false).is_empty(), "no loop: a one-shot voice");
        p.channels[0].sound.sampler_flags = Some(os_model::ChannelSound::LOAD_REGIONS);
        assert!(offs(&p, |_| true).is_empty(), "Use loop points off");
        p.channels[0].sound.sampler_flags = None;
        p.channels[0].sound.loop_points = Some([100, 400]);
        let o = offs(&p, |_| false);
        assert!(at_end(&o), "the channel's own loop: {o:?}");
        let _ = std::fs::remove_file(&wav);
    }

    /// A paused playback whose hosted instruments another stream processes lets go of them: an audio clip's voice is
    /// choked (a note off leaves a sample without a volume envelope playing on), and resuming starts the clip again
    /// where the song has got to.
    // spec: audio/transport.md "Pause"
    #[test]
    fn a_released_pause_chokes_hosted_clips_and_resume_starts_them_again() {
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
        let wav = std::env::temp_dir().join(format!("os-engine-pause-clip-{}.wav", std::process::id()));
        crate::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &vec![0.25; 4 * 96000]).unwrap();
        let mut p = song(vec![], vec![Clip::new(0, 384, 0, ClipSource::Channel(0))]);
        p.channels[0].kind = ChannelKind::AudioClip;
        p.channels[0].sample_path = Some(wav.to_string_lossy().into_owned());
        let mut lib = SampleLibrary::new(None);
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Song);
        let log = Arc::new(std::sync::Mutex::new(Log::default()));
        e.set_instrument(0, log.clone());
        for _ in 0..48 {
            e.render_tick();
        }
        e.pause();
        for _ in 0..8 {
            e.render_tick();
        }
        assert!(e.paused());
        e.release_instruments();
        let names = |l: &Log| l.0.iter().map(|x| x.1).collect::<Vec<_>>();
        let before = names(&log.lock().unwrap());
        assert_eq!(before[..2], ["on", "choke"], "the clip is choked (a note off would leave it playing)");
        let at = e.tick;
        e.resume();
        e.render_tick();
        let l = log.lock().unwrap();
        assert_eq!(names(&l)[before.len()..], ["from"], "resuming starts the clip again");
        let start = l.0[before.len()].2;
        // The 4 s sample, at the song's place (within a few ticks: 5 ms each).
        let want = at as f32 / 96.0 * 60.0 / 130.0 / 4.0;
        assert!((start - want).abs() < 0.004, "where the song has got to: {start} vs {want}");
        drop(l);
        drop(e);
        assert!(names(&log.lock().unwrap())[before.len() + 1..].contains(&"choke"), "and is choked again when playback goes");
    }

    // spec: audio/audio-clips.md "Level"
    #[test]
    fn audio_clips_play_at_unity_through_a_default_channel() {
        use os_model::{Clip, ClipSource};
        /// Plays 0.5 on both sides while a note sounds, as a sampler plays a constant sample at velocity 100.
        #[derive(Default)]
        struct Held(bool);
        impl crate::Instrument for Held {
            fn note_on(&mut self, _o: u32, _k: u8, v: f32) {
                assert_eq!(v, 100.0 / 128.0, "the clip's voice reaches the sampler at velocity 100");
                self.0 = true;
            }
            fn note_off(&mut self, _o: u32, _k: u8) {}
            fn choke(&mut self, _o: u32, _k: u8) {
                self.0 = false;
            }
            fn process(&mut self, _rate: u32, l: &mut [f32], r: &mut [f32]) {
                let v = if self.0 { 0.5 } else { 0.0 };
                l.fill(v);
                r.fill(v);
            }
        }
        let wav = std::env::temp_dir().join(format!("os-engine-clip-level-{}.wav", std::process::id()));
        crate::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &vec![0.5; 96000]).unwrap();
        let mut p = song(vec![], vec![Clip::new(0, 192, 0, ClipSource::Channel(0))]);
        p.channels[0].kind = ChannelKind::AudioClip;
        p.channels[0].sample_path = Some(wav.to_string_lossy().into_owned());
        assert_eq!((p.channels[0].volume, p.channels[0].pan), (0.78125, 0.0), "the channel's default volume, centred");
        let mut lib = SampleLibrary::new(None);
        let mid = 2 * 24000;
        // The engine's own voice: the sample's 0.5 comes out as 0.5.
        let out = Engine::new(&p, &mut lib, 48000, PlayMode::Song).render_all(480);
        assert!((out[mid] - 0.5).abs() < 1e-4 && (out[mid + 1] - 0.5).abs() < 1e-4, "{} {}", out[mid], out[mid + 1]);
        // A hosted sampler on the clip's channel likewise.
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Song);
        e.set_instrument(0, Arc::new(std::sync::Mutex::new(Held::default())));
        let out = e.render_all(480);
        assert!((out[mid] - 0.5).abs() < 1e-4 && (out[mid + 1] - 0.5).abs() < 1e-4, "{} {}", out[mid], out[mid + 1]);
        // Other channel volumes scale it by curve(volume) / curve(0.78125).
        p.channels[0].volume = 1.0;
        let out = Engine::new(&p, &mut lib, 48000, PlayMode::Song).render_all(480);
        assert!((out[mid] - 0.5 / laws::channel_gain(0.78125) as f32).abs() < 1e-3, "{}", out[mid]);
        // A note on a sampler channel keeps its velocity level: 0.5 times the channel's 0.39.
        let mut n = song(vec![long_note(0, 192, 0)], vec![Clip::new(0, 192, 0, ClipSource::Pattern(1))]);
        n.channels[0].sample_path = Some(wav.to_string_lossy().into_owned());
        let out = Engine::new(&n, &mut lib, 48000, PlayMode::Song).render_all(480);
        assert!((out[mid] - 0.5 * 0.38961).abs() < 1e-3, "{}", out[mid]);
        let _ = std::fs::remove_file(&wav);
    }

    /// Each note's own pan and mod X / Y go to the hosted instrument with the note (spec: audio/voice-levels.md "The
    /// note's own levels"); an audio clip's note is centred.
    #[test]
    fn hosted_notes_carry_their_pan_and_mods() {
        use os_model::{Clip, ClipSource};
        #[derive(Default)]
        struct Log(Vec<(u8, crate::NoteLevels)>);
        impl crate::Instrument for Log {
            fn note_on(&mut self, _o: u32, _k: u8, _v: f32) {
                panic!("notes start with their levels");
            }
            fn note_off(&mut self, _o: u32, _k: u8) {}
            fn note_start(&mut self, _o: u32, key: u8, _v: f32, _start: f32, levels: crate::NoteLevels) {
                self.0.push((key, levels));
            }
            fn process(&mut self, _rate: u32, _l: &mut [f32], _r: &mut [f32]) {}
        }
        let mut panned = long_note(0, 48, 0);
        (panned.pan, panned.mod_x, panned.mod_y) = (96, 192, 64);
        let plain = Note { key: 62, ..long_note(48, 48, 0) };
        let p = song(vec![panned, plain], vec![Clip::new(0, 96, 0, ClipSource::Pattern(1))]);
        let mut e = Engine::new(&p, &mut SampleLibrary::new(None), 48000, PlayMode::Song);
        let log = Arc::new(std::sync::Mutex::new(Log::default()));
        e.set_instrument(0, log.clone());
        e.render_all(480);
        let got = log.lock().unwrap().0.clone();
        let half = crate::NoteLevels { pan: 0.5, mod_x: 0.5, mod_y: -0.5, channel: 0 };
        let centred = crate::NoteLevels::of_bytes(64, 128, 128);
        assert_eq!(centred, crate::NoteLevels::default());
        assert_eq!(got, vec![(60, half), (62, centred)]);
        assert_eq!(crate::NoteLevels::of_bytes(0, 0, 255), crate::NoteLevels { pan: -1.0, mod_x: -1.0, mod_y: 127.0 / 128.0, channel: 0 });
    }

    /// MIDI channel through (spec: ui/channel-rack.md §18): a live note's MIDI channel reaches the hosted instrument
    /// with the note; a plain live note is on channel 1.
    #[test]
    fn live_notes_carry_their_midi_channel() {
        #[derive(Default)]
        struct Log(Vec<(u8, u8)>);
        impl crate::Instrument for Log {
            fn note_on(&mut self, _o: u32, _k: u8, _v: f32) {
                panic!("notes start with their levels");
            }
            fn note_off(&mut self, _o: u32, _k: u8) {}
            fn note_start(&mut self, _o: u32, key: u8, _v: f32, _start: f32, levels: crate::NoteLevels) {
                self.0.push((key, levels.channel));
            }
            fn process(&mut self, _rate: u32, _l: &mut [f32], _r: &mut [f32]) {}
        }
        let mut p = project(vec![], vec![]);
        p.channels[0].kind = ChannelKind::Plugin;
        let mut e = Engine::new(&p, &mut SampleLibrary::default(), 48000, PlayMode::Pattern(1));
        let log = Arc::new(std::sync::Mutex::new(Log::default()));
        e.set_instrument(0, log.clone());
        e.note_on_midi(0, 64, 100, 5);
        e.note_on(0, 67, 100);
        e.note_on_midi(0, 69, 100, 0x13);
        let mut out = vec![0.0f32; 256];
        e.process(&mut out);
        assert_eq!(log.lock().unwrap().0, [(64, 5), (67, 0), (69, 3)], "the channel's low nibble");
    }

    /// Records what a layer's child is given, by frame: note ons (velocity), offs, chokes and tunings (semitones); it
    /// outputs 1.0 while a note is held.
    #[derive(Default)]
    struct Rec {
        at: u64,
        log: Vec<(u64, &'static str, u8, f32)>,
        held: Vec<u8>,
    }

    impl crate::Instrument for Rec {
        fn note_on(&mut self, o: u32, key: u8, v: f32) {
            self.log.push((self.at + u64::from(o), "on", key, v));
            self.held.push(key);
        }
        fn note_off(&mut self, o: u32, key: u8) {
            self.log.push((self.at + u64::from(o), "off", key, 0.0));
            self.held.retain(|&k| k != key);
        }
        fn choke(&mut self, o: u32, key: u8) {
            self.log.push((self.at + u64::from(o), "choke", key, 0.0));
            self.held.retain(|&k| k != key);
        }
        fn tune(&mut self, o: u32, key: u8, semitones: f32) {
            self.log.push((self.at + u64::from(o), "tune", key, semitones));
        }
        fn process(&mut self, _rate: u32, l: &mut [f32], r: &mut [f32]) {
            let v = if self.held.is_empty() { 0.0 } else { 1.0 };
            l.fill(v);
            r.fill(v);
            self.at += l.len() as u64;
        }
    }

    /// A project whose channel 0 is a layer with `flags` over the channels of `kinds` (1, 2, ...).
    fn layered(flags: u32, notes: Vec<Note>, kinds: &[ChannelKind]) -> Project {
        let mut p = project(notes, vec![]);
        p.channels[0].kind = ChannelKind::Layer;
        p.channels[0].layer_children = (1..=kinds.len() as u16).collect();
        p.channels[0].misc.layer.flags = flags;
        for (i, &kind) in kinds.iter().enumerate() {
            p.channels.push(Channel { index: i as u16 + 1, kind, ..channel() });
        }
        p
    }

    /// The engine for `p` with every channel but the first played by a recorder.
    fn recorded(p: &Project) -> (Engine, Vec<Arc<std::sync::Mutex<Rec>>>) {
        let mut e = Engine::new(p, &mut SampleLibrary::new(None), 48000, PlayMode::Pattern(1));
        let recs: Vec<_> = (1..p.channels.len())
            .map(|c| {
                let r = Arc::new(std::sync::Mutex::new(Rec::default()));
                e.set_instrument(c, r.clone());
                r
            })
            .collect();
        (e, recs)
    }

    /// The events of kind `kind` a recorder got: (frame, key, value).
    fn got(r: &Arc<std::sync::Mutex<Rec>>, kind: &str) -> Vec<(u64, u8, f32)> {
        r.lock().unwrap().log.iter().filter(|l| l.1 == kind).map(|l| (l.0, l.2, l.3)).collect()
    }

    /// A layer's note makes no sound of its own: every child plays it as its own note, with the layer's pitch on its
    /// pitch (here a key 3 semitones up: the children ignore a tuning) and the layer's volume and pan on its levels
    /// (spec: audio/note-rules.md "Layer channels").
    #[test]
    fn layer_notes_play_on_every_child_with_the_layers_levels() {
        let kinds = [ChannelKind::Plugin, ChannelKind::Plugin];
        let mut p = layered(0, vec![long_note(0, 48, 0)], &kinds);
        (p.channels[0].volume, p.channels[0].pitch_cents, p.channels[0].pan) = (1.0, 300, 0.5);
        let (mut e, recs) = recorded(&p);
        let out = e.render_all(512);
        let end = laws::tick_to_frame(48.0, 96, 130.0, 48000);
        for r in &recs {
            assert_eq!(got(r, "on"), vec![(0, 63, 100.0 / 128.0)]);
            assert_eq!(got(r, "tune"), vec![]);
            assert_eq!(got(r, "off"), vec![(end, 63, 0.0)]);
        }
        // The same notes written on the children themselves: the layer's volume and pan are what differs.
        let mut direct = p.clone();
        direct.patterns[0].notes = vec![Note { channel: 1, ..long_note(0, 48, 0) }, Note { channel: 2, ..long_note(0, 48, 0) }];
        let (mut e, _) = recorded(&direct);
        let plain = e.render_all(512);
        let (pl, pr) = laws::channel_pan(0.5);
        let (cl, cr) = laws::channel_pan(0.0);
        let layer = laws::channel_gain(1.0);
        let f = 1000;
        assert!(plain[f * 2] > 0.1);
        assert!((f64::from(out[f * 2]) - f64::from(plain[f * 2]) * layer * pl / cl).abs() < 1e-5, "{} {}", out[f * 2], plain[f * 2]);
        assert!((f64::from(out[f * 2 + 1]) - f64::from(plain[f * 2 + 1]) * layer * pr / cr).abs() < 1e-5);
    }

    /// Every note a recorder got has ended: per key, as many note offs or chokes as note ons, and none started last.
    fn all_ended(r: &Arc<std::sync::Mutex<Rec>>, what: &str) {
        let log = r.lock().unwrap().log.clone();
        for key in 0..=127u8 {
            let of: Vec<&str> = log.iter().filter(|l| l.2 == key && l.1 != "tune").map(|l| l.1).collect();
            let ons = of.iter().filter(|k| **k == "on").count();
            assert!(of.len() - ons >= ons && of.last() != Some(&"on"), "{what}: key {key} left sounding: {log:?}");
        }
    }

    /// Plays `p` to `tick`, pauses, and holds for a while: every note its plugin channels got has ended (spec:
    /// audio/transport.md "Pause": the paused playback lets go of every note it holds at the instruments).
    fn paused_at(p: &Project, tick: u64, what: &str) {
        let (mut e, recs) = recorded(p);
        while e.tick < tick {
            e.next_tick();
        }
        assert!(recs.iter().any(|r| !r.lock().unwrap().held.is_empty()), "{what}: notes sound before the pause");
        e.pause();
        for _ in 0..10 {
            e.next_tick();
        }
        assert!(e.held(), "{what}: held");
        for r in &recs {
            all_ended(r, what);
        }
    }

    /// The note rules keep their own records of what is held (a mono channel's list, portamento, a slide, a layer's
    /// children): a pause in the middle of each lets go of every note.
    #[test]
    fn a_pause_lets_go_of_every_note_whatever_the_note_rules() {
        let on = |c: u16, position: u32, length: u32, key: u16| Note { channel: c, key, ..long_note(position, length, 0) };
        // One plugin channel (1) behind an unused first channel, as `recorded` plays every channel but the first.
        let plugin = |notes: Vec<Note>| {
            let mut p = project(notes, vec![]);
            p.channels.push(Channel { index: 1, kind: ChannelKind::Plugin, ..channel() });
            p
        };
        let mut mono = plugin(vec![on(1, 0, 384, 60), on(1, 48, 384, 64), on(1, 96, 384, 67)]);
        mono.channels[1].misc.mono = true;
        paused_at(&mono, 120, "mono");
        let mut porta = plugin(vec![on(1, 0, 384, 60), on(1, 48, 384, 64)]);
        porta.channels[1].misc.porta = true;
        paused_at(&porta, 120, "portamento");
        let slide = Note { flags: 0x4000 | Note::SLIDE, ..on(1, 48, 24, 72) };
        paused_at(&plugin(vec![on(1, 0, 384, 60), slide]), 60, "slide");
        let kinds = [ChannelKind::Plugin, ChannelKind::Plugin];
        paused_at(&layered(0, vec![long_note(0, 384, 0), long_note(48, 384, 0)], &kinds), 120, "layer");
    }

    /// An instrument busy (held by the UI) as a note is due misses it; the note waits for it. A pause letting go of the
    /// notes then drops that waiting note on, or it would start after its note off and sound for as long as the
    /// playback stays paused.
    #[test]
    fn a_note_an_instrument_missed_does_not_start_after_the_pause() {
        let mut p = project(vec![long_note(1, 384, 0)], vec![]);
        p.channels.push(Channel { index: 1, kind: ChannelKind::Plugin, ..channel() });
        p.patterns[0].notes[0].channel = 1;
        let (mut e, recs) = recorded(&p);
        e.busy_wait = Duration::ZERO;
        e.next_tick();
        // Held elsewhere through the note's start and the end of the pause's fade.
        let rec = recs[0].clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let holder = std::thread::spawn(move || {
            let _g = rec.lock().unwrap();
            tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(200));
        });
        rx.recv().unwrap();
        e.pause();
        for _ in 0..4 {
            e.next_tick();
        }
        assert!(e.held(), "held");
        holder.join().unwrap();
        for _ in 0..4 {
            e.next_tick();
        }
        all_ended(&recs[0], "missed");
    }

    /// Flag 4: one child per note in turn, from the first; flag 1: one at random, drawn from the project's generator
    /// (seeded and one output taken at the start).
    #[test]
    fn layers_hand_notes_on_in_turn_or_at_random() {
        let notes: Vec<Note> = (0..12).map(|i| long_note(i * 24, 12, 0)).collect();
        let order = |flags: u32| {
            let p = layered(flags, notes.clone(), &[ChannelKind::Plugin, ChannelKind::Plugin, ChannelKind::Plugin]);
            let (mut e, recs) = recorded(&p);
            e.render_all(512);
            let mut ons: Vec<(u64, usize)> = recs.iter().enumerate().flat_map(|(c, r)| got(r, "on").into_iter().map(move |o| (o.0, c))).collect();
            ons.sort_unstable();
            ons.into_iter().map(|o| o.1).collect::<Vec<usize>>()
        };
        assert_eq!(order(os_model::LayerSettings::IN_TURN), [0, 1, 2, 0, 1, 2, 0, 1, 2, 0, 1, 2]);
        let mut g = layers::Mt19937::for_playback();
        let drawn: Vec<usize> = (0..12).map(|_| layers::place(g.next_u32(), 3)).collect();
        assert_eq!(order(os_model::LayerSettings::RANDOM), drawn);
        assert_eq!(order(os_model::LayerSettings::RANDOM | os_model::LayerSettings::IN_TURN), drawn);
        // Flags 0: every child gets every note.
        assert_eq!(order(0).len(), 36);
    }

    /// The note goes from child to child with its cut numbers: a first child of one-shot voices set to cut itself
    /// makes the second child's voices belong to its group, so the next layer note cuts both; a layer set to cut
    /// itself cuts nothing (notes with a length); another channel cuts the children through the layer's "cut by".
    #[test]
    fn a_layer_note_carries_its_cut_numbers_from_child_to_child() {
        let notes = vec![long_note(0, 96, 0), long_note(48, 96, 0)];
        let at = laws::tick_to_frame(48.0, 96, 130.0, 48000);
        let mut p = layered(0, notes.clone(), &[ChannelKind::Sampler, ChannelKind::Plugin]);
        (p.channels[1].misc.cut, p.channels[1].misc.cut_by) = (1, 1);
        let (mut e, recs) = recorded(&p);
        e.render_all(512);
        assert_eq!(got(&recs[0], "choke").first().map(|c| c.0), Some(at));
        assert_eq!(got(&recs[1], "off").first().map(|c| c.0), Some(at));
        assert_eq!(got(&recs[1], "on").len(), 2);

        let mut p = layered(0, notes.clone(), &[ChannelKind::Plugin, ChannelKind::Plugin]);
        (p.channels[0].misc.cut, p.channels[0].misc.cut_by) = (1, 1);
        let (mut e, recs) = recorded(&p);
        e.render_all(512);
        let first_end = laws::tick_to_frame(96.0, 96, 130.0, 48000);
        for r in &recs {
            assert_eq!(got(r, "off").first().map(|c| c.0), Some(first_end));
        }

        // Channel 3 is not a child; its note at tick 48 cuts group 2, which the layer's notes belong to.
        let mut p = layered(0, vec![long_note(0, 96, 0), Note { channel: 3, ..long_note(48, 12, 0) }], &[ChannelKind::Plugin, ChannelKind::Plugin, ChannelKind::Plugin]);
        p.channels[0].layer_children = vec![1, 2];
        p.channels[0].misc.cut_by = 2;
        p.channels[3].misc.cut = 2;
        let (mut e, recs) = recorded(&p);
        e.render_all(512);
        for r in &recs[..2] {
            assert_eq!(got(r, "off").first().map(|c| c.0), Some(at));
        }
    }

    /// A slide note on the layer moves the voices of the layer's notes on all children; a slide note on a child moves
    /// that child's own voices, not the layer's.
    #[test]
    fn a_layers_slide_note_moves_the_voices_of_its_notes() {
        let slide = |position: u32, key: u16, channel: u16| Note { key, channel, flags: 0x4000 | Note::SLIDE, ..long_note(position, 0, 0) };
        let notes = vec![long_note(0, 192, 0), Note { key: 64, channel: 1, ..long_note(0, 192, 0) }, slide(24, 67, 0), slide(48, 76, 1)];
        let p = layered(0, notes, &[ChannelKind::Plugin, ChannelKind::Plugin]);
        let (mut e, recs) = recorded(&p);
        e.render_all(512);
        let (t24, t48) = (laws::tick_to_frame(24.0, 96, 130.0, 48000), laws::tick_to_frame(48.0, 96, 130.0, 48000));
        assert_eq!(got(&recs[0], "tune"), vec![(t24, 60, 7.0), (t48, 64, 12.0)]);
        assert_eq!(got(&recs[1], "tune"), vec![(t24, 60, 7.0)]);
    }

    /// The engine's own sampler voices of a layer's note: the layer's volume and the crossfade gain multiply the
    /// child's level (crossfade 64 over two children: 0.75 and 0.25; one child alone gets the position, 0.25).
    #[test]
    fn engine_voices_of_a_layer_take_its_volume_and_crossfade() {
        let kinds = [ChannelKind::Sampler, ChannelKind::Sampler];
        let mut alone = layered(0, vec![Note { channel: 1, ..long_note(0, 48, 0) }], &kinds);
        alone.channels[0].volume = 1.0;
        let reference = peak(&render(&alone), 0);
        assert!(reference > 0.05);
        let layer = laws::channel_gain(1.0) as f32;
        for (children, want) in [(vec![1, 2], 1.0f32), (vec![1], 0.25), (vec![2, 1], 1.0)] {
            let mut p = alone.clone();
            p.patterns[0].notes = vec![long_note(0, 48, 0)];
            p.channels[0].layer_children = children.clone();
            p.channels[0].misc.layer = os_model::LayerSettings { flags: os_model::LayerSettings::CROSSFADE, crossfade: 64 };
            let got = peak(&render(&p), 0);
            assert!((got - reference * layer * want).abs() < 1e-4, "{children:?}: {got} vs {}", reference * layer * want);
        }
        // Without the crossfade both children play at full level.
        let mut p = alone.clone();
        p.patterns[0].notes = vec![long_note(0, 48, 0)];
        let got = peak(&render(&p), 0);
        assert!((got - reference * layer * 2.0).abs() < 1e-4, "{got}");
    }

    /// A hosted instrument is given the channel's envelopes, LFOs and filter before every tick's `process` call, whose
    /// frames are that tick's; a volume record the file did not have is a new channel's (spec: plugins/fruity-abi.md
    /// "Voices").
    #[test]
    fn hosted_instruments_get_the_channels_voicing_every_tick() {
        use os_dsp::articulator::Record;
        #[derive(Default)]
        struct V {
            got: Vec<crate::instrument::ChannelVoicing>,
            frames: Vec<usize>,
            pending: bool,
        }
        impl crate::Instrument for V {
            fn note_on(&mut self, _o: u32, _k: u8, _v: f32) {}
            fn note_off(&mut self, _o: u32, _k: u8) {}
            fn channel_voicing(&mut self, voicing: &crate::instrument::ChannelVoicing) {
                assert!(!self.pending, "one call per process call");
                self.got.push(*voicing);
                self.pending = true;
            }
            fn process(&mut self, _rate: u32, l: &mut [f32], _r: &mut [f32]) {
                assert!(std::mem::take(&mut self.pending), "a tick's process call follows its voicing");
                self.frames.push(l.len());
            }
        }
        let mut p = project(vec![long_note(0, 48, 0)], vec![]);
        p.channels[0].kind = ChannelKind::Plugin;
        let pitch = &mut p.channels[0].envelopes[os_model::Envelope::PITCH];
        (pitch.enabled, pitch.amount, pitch.flags) = (true, 700, 1);
        (p.channels[0].sound.cutoff, p.channels[0].sound.resonance, p.channels[0].sound.filter_type) = (100, 30, 6);
        let mut e = Engine::new(&p, &mut SampleLibrary::new(None), 48000, PlayMode::Pattern(1));
        let v = Arc::new(std::sync::Mutex::new(V::default()));
        e.set_instrument(0, v.clone());
        e.render_all(512);
        let v = v.lock().unwrap();
        assert!(v.got.len() >= 48, "{}", v.got.len());
        let spt = 48000.0 * 60.0 / 130.0 / 96.0;
        assert!(v.frames.iter().all(|&f| (f as f64 - spt).abs() < 1.0), "{:?}", &v.frames[..4]);
        let g = v.got[0];
        assert_eq!(g.records[Record::VOLUME], Record::new_channel(Record::VOLUME));
        let r = g.records[Record::PITCH];
        assert_eq!((r.on, r.amount, r.flags, r.attack), (true, 700, 1, 20000));
        assert_eq!((g.cutoff, g.resonance, g.filter_type, g.ppq), (100, 30, 6, 96));
        // A volume record from the file stays as it was.
        let mut q = p.clone();
        q.channels[0].envelopes[os_model::Envelope::VOLUME].flags = 2;
        let g = crate::instrument::ChannelVoicing::of(&q.channels[0], 96);
        assert_eq!((g.records[Record::VOLUME].flags, g.records[Record::VOLUME].release_tension), (2, 0));
    }

    /// Logs the keys of its notes (true: on) and its tunings; `tunes`: what `takes_tuning` says.
    #[derive(Default)]
    struct TuneLog {
        tunes: bool,
        keys: Vec<(u8, bool)>,
        tuned: Vec<(u8, f32)>,
    }

    impl crate::Instrument for TuneLog {
        fn note_on(&mut self, _o: u32, key: u8, _v: f32) {
            self.keys.push((key, true));
        }
        fn note_off(&mut self, _o: u32, key: u8) {
            self.keys.push((key, false));
        }
        fn tune(&mut self, _o: u32, key: u8, semitones: f32) {
            self.tuned.push((key, semitones));
        }
        fn takes_tuning(&self) -> bool {
            self.tunes
        }
        fn process(&mut self, _rate: u32, _l: &mut [f32], _r: &mut [f32]) {}
    }

    /// A hosted instrument hears the channel pitch as each note's tuning (the engine's own voices apply it themselves).
    /// On a plugin channel, one that ignores a tuning (most VST and CLAP instruments) hears the pitch's whole
    /// semitones as a moved key, and is tuned by what is left.
    #[test]
    fn channel_pitch_tunes_hosted_notes() {
        use os_model::{Clip, ClipSource};
        let mut p = song(vec![long_note(0, 96, 0)], vec![Clip::new(0, 96, 0, ClipSource::Pattern(1))]);
        let run = |p: &Project, tunes: bool| {
            let mut e = Engine::new(p, &mut SampleLibrary::new(None), 48000, PlayMode::Song);
            let log = Arc::new(std::sync::Mutex::new(TuneLog { tunes, ..TuneLog::default() }));
            e.set_instrument(0, log.clone());
            e.render_all(480);
            let l = log.lock().unwrap();
            (l.keys.clone(), l.tuned.clone())
        };
        let close = |got: &[(u8, f32)], key: u8, semitones: f32| got.len() == 1 && got[0].0 == key && (got[0].1 - semitones).abs() < 1e-3;
        for tunes in [false, true] {
            assert_eq!(run(&p, tunes), (vec![(60, true), (60, false)], vec![]), "no pitch, no tuning");
        }
        p.channels[0].pitch_cents = -250;
        // A sampler channel's instrument (Lemon Sampler) takes its tuning whatever it says.
        let (keys, got) = run(&p, false);
        assert!(keys == [(60, true), (60, false)] && close(&got, 60, -2.5), "{keys:?} {got:?}");
        // It applies root note and fine tune itself: the tuning leaves them out. A plugin channel's tuning holds them.
        (p.channels[0].misc.root, p.channels[0].misc.fine) = (48, 30);
        let (_, got) = run(&p, false);
        assert!(close(&got, 60, -2.5), "{got:?}");
        p.channels[0].kind = ChannelKind::Plugin;
        let (keys, got) = run(&p, true);
        assert!(keys == [(60, true), (60, false)] && close(&got, 60, 12.0 - 2.5 + 0.3), "{keys:?} {got:?}");
        // 9.8 semitones: 10 of them move the key, -0.2 is tuned; the note off goes to the moved key.
        let (keys, got) = run(&p, false);
        assert!(keys == [(70, true), (70, false)] && close(&got, 70, -0.2), "{keys:?} {got:?}");
        // Whole semitones only: no tuning.
        (p.channels[0].pitch_cents, p.channels[0].misc.fine) = (0, 0);
        assert_eq!(run(&p, false), (vec![(72, true), (72, false)], vec![]), "root note C4: an octave up");
        p.main_pitch = Some(-300);
        assert_eq!(run(&p, false), (vec![(69, true), (69, false)], vec![]), "and the main pitch");
        p.channels[0].misc.main_pitch = false;
        assert_eq!(run(&p, false).0, [(72, true), (72, false)], "unless the channel does not follow it");
        // Keys stay inside MIDI's: what is beyond is left to the tuning (72 semitones up from key 60).
        (p.channels[0].misc.root, p.channels[0].misc.main_pitch, p.main_pitch) = (0, true, Some(1200));
        let (keys, got) = run(&p, false);
        assert!(keys == [(127, true), (127, false)] && close(&got, 127, 5.0), "{keys:?} {got:?}");
    }

    /// A note held on a moved key keeps it: a channel pitch that changes while it sounds retunes it there, and its
    /// note off goes there, also when the engine goes away.
    #[test]
    fn moved_keys_keep_their_notes() {
        let mut p = project(vec![], vec![]);
        p.channels[0].kind = ChannelKind::Plugin;
        p.channels[0].misc.root = 55;
        let mut lib = SampleLibrary::default();
        let log = Arc::new(std::sync::Mutex::new(TuneLog::default()));
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Pattern(1));
        e.set_instrument(0, log.clone());
        e.note_on(0, 64, 100);
        let mut out = vec![0.0f32; 4096];
        e.process(&mut out);
        assert_eq!(log.lock().unwrap().keys, [(69, true)], "five semitones up");
        // The main pitch moves (the knob rebuilds the playing engine): the sounding note is tuned where it plays.
        p.main_pitch = Some(200);
        let mut e2 = Engine::new(&p, &mut lib, 48000, PlayMode::Pattern(1));
        e2.set_instrument(0, log.clone());
        e2.continue_from(&mut e);
        drop(e);
        e2.process(&mut out);
        assert_eq!(log.lock().unwrap().tuned, [(69, 2.0)]);
        // A new note takes the new pitch as its key; the engine going away ends both where they play.
        e2.note_on(0, 60, 100);
        e2.process(&mut out);
        drop(e2);
        assert_eq!(log.lock().unwrap().keys, [(69, true), (67, true), (67, false), (69, false)]);
    }

    // spec: audio/voice-levels.md "The main pitch": sounding voices follow a change, here a hosted sampler's note
    // stretched by Lemon Sampler, while the song plays on.
    #[test]
    fn main_pitch_moved_while_playing_retunes_hosted_notes() {
        use os_model::{Clip, ClipSource};
        #[derive(Default)]
        struct Log(Vec<(u8, f32)>);
        impl crate::Instrument for Log {
            fn note_on(&mut self, _o: u32, _k: u8, _v: f32) {}
            fn note_off(&mut self, _o: u32, _k: u8) {}
            fn tune(&mut self, _o: u32, key: u8, semitones: f32) {
                self.0.push((key, semitones));
            }
            fn process(&mut self, _rate: u32, _l: &mut [f32], _r: &mut [f32]) {}
        }
        let mut p = song(vec![long_note(0, 384, 0)], vec![Clip::new(0, 384, 0, ClipSource::Pattern(1))]);
        p.channels[0].sound.stretch = [0, 0, 0, os_model::stretch_mode::STRETCH as u32];
        let mut lib = SampleLibrary::new(None);
        lib.resample_stretches = true;
        let log = Arc::new(std::sync::Mutex::new(Log::default()));
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Song);
        e.set_instrument(0, log.clone());
        let mut out = Vec::new();
        e.render_ticks(48, &mut out);
        assert!(log.lock().unwrap().0.is_empty(), "no pitch, no tuning");
        // The main pitch moves (the knob rebuilds the playing engine): the note already sounding follows it.
        p.main_pitch = Some(300);
        let mut e2 = Engine::new(&p, &mut lib, 48000, PlayMode::Song);
        e2.set_instrument(0, log.clone());
        e2.continue_from(&mut e);
        e2.render_ticks(48, &mut out);
        assert_eq!(log.lock().unwrap().0, vec![(60, 3.0)]);
        // A channel that does not follow the main pitch is left alone.
        p.channels[0].misc.main_pitch = false;
        let mut e3 = Engine::new(&p, &mut lib, 48000, PlayMode::Song);
        e3.set_instrument(0, log.clone());
        e3.continue_from(&mut e2);
        e3.render_ticks(48, &mut out);
        assert_eq!(log.lock().unwrap().0, vec![(60, 3.0), (60, 0.0)], "back to its own pitch");
    }

    /// spec: audio/transport.md "Edits while playing": an audio clip deleted or moved while it sounds ends at once,
    /// faded out as a live note is; one still where it was plays on, also when it was made longer.
    #[test]
    fn clips_deleted_or_moved_while_playing_stop_sounding() {
        use os_model::{Clip, ClipSource};
        let wav = std::env::temp_dir().join(format!("os-engine-playing-edit-{}.wav", std::process::id()));
        crate::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &vec![0.25; 192000]).unwrap();
        // A two-second sample in a clip from 0 to 192, and one more at 384 that keeps the song going.
        let with = |clip: Option<Clip>| {
            let mut p = song(vec![], clip.into_iter().chain([Clip::new(384, 96, 0, ClipSource::Channel(0))]).collect());
            p.channels[0].kind = ChannelKind::AudioClip;
            p.channels[0].sample_path = Some(wav.to_string_lossy().into_owned());
            p
        };
        let clip = Clip::new(0, 192, 0, ClipSource::Channel(0));
        let playing = with(Some(clip));
        let mut lib = SampleLibrary::new(None);
        // 48 ticks into the clip the edited project takes over: the loudest sample of its next 24 ticks after 30 ms.
        let mut loudest = |edited: &Project| {
            let mut e = Engine::new(&playing, &mut lib, 48000, PlayMode::Song);
            e.render_ticks(48, &mut Vec::new());
            let mut e2 = Engine::new(edited, &mut lib, 48000, PlayMode::Song);
            e2.continue_from(&mut e);
            let mut out = Vec::new();
            e2.render_ticks(24, &mut out);
            out[2880..].iter().fold(0.0f32, |m, x| m.max(x.abs()))
        };
        assert!(loudest(&playing) > 0.05, "unchanged: plays on");
        assert!(loudest(&with(Some(Clip { length: 300, ..clip }))) > 0.05, "made longer: plays on");
        assert_eq!(loudest(&with(None)), 0.0, "deleted");
        assert_eq!(loudest(&with(Some(Clip { position: 200, ..clip }))), 0.0, "moved later");
        // Moved, still under the play position: it plays on from its new place; so does one added there.
        assert!(loudest(&playing) < 0.3, "one voice, not started again on top of itself");
        assert!(loudest(&with(Some(Clip { position: 24, ..clip }))) > 0.05, "moved under the play position");
        let quiet = with(None);
        let mut e = Engine::new(&quiet, &mut lib, 48000, PlayMode::Song);
        e.render_ticks(48, &mut Vec::new());
        let mut e2 = Engine::new(&playing, &mut lib, 48000, PlayMode::Song);
        e2.continue_from(&mut e);
        let mut out = Vec::new();
        e2.render_ticks(24, &mut out);
        assert!(out[2880..].iter().any(|x| x.abs() > 0.05), "added under the play position");
        let _ = std::fs::remove_file(&wav);
    }

    /// The same through a hosted instrument (spec: audio/transport.md "Edits while playing"): a deleted or moved audio
    /// clip is choked, and a deleted pattern clip's held note gets its note off, on the next tick; what is still there
    /// ends where it did.
    #[test]
    fn hosted_notes_of_deleted_clips_end_while_playing() {
        use os_model::{Clip, ClipSource};
        #[derive(Default)]
        struct Log(Vec<(u64, &'static str)>, u64);
        impl crate::Instrument for Log {
            fn note_on(&mut self, o: u32, _k: u8, _v: f32) {
                self.0.push((self.1 + u64::from(o), "on"));
            }
            fn note_off(&mut self, o: u32, _k: u8) {
                self.0.push((self.1 + u64::from(o), "off"));
            }
            fn note_on_from(&mut self, o: u32, _k: u8, _v: f32, _start: f32) {
                self.0.push((self.1 + u64::from(o), "from"));
            }
            fn choke(&mut self, o: u32, _k: u8) {
                self.0.push((self.1 + u64::from(o), "choke"));
            }
            fn process(&mut self, _rate: u32, l: &mut [f32], _r: &mut [f32]) {
                self.1 += l.len() as u64;
            }
        }
        let wav = std::env::temp_dir().join(format!("os-engine-playing-edit-hosted-{}.wav", std::process::id()));
        crate::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &vec![0.25; 192000]).unwrap();
        let f = |t: f64| laws::tick_to_frame(t, 96, 130.0, 48000);
        let mut lib = SampleLibrary::new(None);
        // 48 ticks of `playing`, then `edited` takes over up to tick 400: the tick of the note's end, rounded down.
        let mut end = |playing: &Project, edited: &Project| {
            let log = Arc::new(std::sync::Mutex::new(Log::default()));
            let mut e = Engine::new(playing, &mut lib, 48000, PlayMode::Song);
            e.set_instrument(0, log.clone());
            e.render_ticks(48, &mut Vec::new());
            let mut e2 = Engine::new(edited, &mut lib, 48000, PlayMode::Song);
            e2.set_instrument(0, log.clone());
            e2.continue_from(&mut e);
            e2.render_ticks(352, &mut Vec::new());
            let got = log.lock().unwrap().0.clone();
            let at = got.iter().find(|g| g.1 == "off" || g.1 == "choke").map(|g| g.0);
            at.map(|a| (0..).take_while(|&t| f(f64::from(t)) <= a).last().unwrap_or(0))
        };
        // An audio clip from 0 to 192 and one at 384 that keeps the song going.
        let audio = |clip: Option<Clip>| {
            let mut p = song(vec![], clip.into_iter().chain([Clip::new(384, 96, 0, ClipSource::Channel(0))]).collect());
            p.channels[0].kind = ChannelKind::AudioClip;
            p.channels[0].sample_path = Some(wav.to_string_lossy().into_owned());
            p
        };
        let clip = Clip::new(0, 192, 0, ClipSource::Channel(0));
        assert_eq!(end(&audio(Some(clip)), &audio(Some(clip))), Some(192), "unchanged: choked at its end");
        assert_eq!(end(&audio(Some(clip)), &audio(None)), Some(48), "deleted");
        assert_eq!(end(&audio(Some(clip)), &audio(Some(Clip { position: 200, ..clip }))), Some(48), "moved");
        // A pattern clip from 0 holding a note to 300, and one at 384.
        let pattern = |first: bool| {
            let mut p = song(vec![long_note(0, 300, 0)], first.then(|| Clip::new(0, 384, 0, ClipSource::Pattern(1))).into_iter().chain([Clip::new(384, 384, 1, ClipSource::Pattern(1))]).collect());
            p.channels[0].kind = ChannelKind::Plugin;
            p
        };
        assert_eq!(end(&pattern(true), &pattern(true)), Some(300), "unchanged: its note off at its end");
        assert_eq!(end(&pattern(true), &pattern(false)), Some(48), "deleted");
        let _ = std::fs::remove_file(&wav);
    }

    // spec: audio/voice-levels.md "The main pitch", audio/automation.md "Project targets": a control point holds the
    // cents, a clip's value v gives -1200 + Round(2400 v); a channel that follows the main pitch has its sounding notes
    // retuned, one that does not is left alone.
    #[test]
    fn main_pitch_automation_retunes_the_channels_that_follow_it() {
        use os_model::{target, Clip, ClipSource, ControlPoint};
        #[derive(Default)]
        struct Log(Vec<(u8, f32)>);
        impl crate::Instrument for Log {
            fn note_on(&mut self, _o: u32, _k: u8, _v: f32) {}
            fn note_off(&mut self, _o: u32, _k: u8) {}
            fn tune(&mut self, _o: u32, key: u8, semitones: f32) {
                self.0.push((key, semitones));
            }
            fn process(&mut self, _rate: u32, _l: &mut [f32], _r: &mut [f32]) {}
        }
        assert_eq!(target::decode(target::MAIN_PITCH), target::Target::MainPitch);
        assert_eq!([0.0, 0.25, 0.5, 0.75, 1.0].map(target::main_pitch_of), [-1200, -600, 0, 600, 1200]);
        assert_eq!(point_level(target::Target::MainPitch, 700).map(target::main_pitch_of), Some(700));
        let mut p = song(vec![long_note(0, 384 * 2, 0)], vec![Clip::new(0, 384 * 2, 0, ClipSource::Pattern(1))]);
        p.channels[0].sound.stretch = [0, 0, 0, os_model::stretch_mode::STRETCH as u32];
        p.patterns[0].controls.push(ControlPoint::new(48, 0x4000, 2, 700));
        add_auto(&mut p, target::MAIN_PITCH, flat(0.25), 192, 96);
        let run = |p: &Project| {
            let mut lib = SampleLibrary::new(None);
            lib.resample_stretches = true;
            let log = Arc::new(std::sync::Mutex::new(Log::default()));
            let mut e = Engine::new(p, &mut lib, 48000, PlayMode::Song);
            e.set_instrument(0, log.clone());
            e.render_ticks(384, &mut Vec::new());
            let tunes = log.lock().unwrap().0.clone();
            tunes
        };
        assert_eq!(run(&p), vec![(60, 7.0), (60, -6.0)], "the point's 700 cents, then the clip's -600");
        // Back before the point: the stored main pitch (0) again.
        let mut e = Engine::new(&p, &mut SampleLibrary::default(), 48000, PlayMode::Song);
        e.render_ticks(96, &mut Vec::new());
        assert_eq!(e.channels[0].main, Some(700));
        e.seek(0);
        assert_eq!(e.channels[0].main, Some(0));
        // Past it again: the point is replayed.
        e.seek_tick(120);
        assert_eq!(e.channels[0].main, Some(700));
        p.channels[0].misc.main_pitch = false;
        assert_eq!(run(&p), vec![], "a channel that does not follow the main pitch");
    }

    // spec: audio/audio-clips.md "Starting inside a clip"
    #[test]
    fn playback_started_inside_an_audio_clip_plays_it_from_there() {
        use os_model::{Clip, ClipSource};
        // A one-second ramp (each frame's value is its time) in a clip from tick 0 to 192 (0.92 s at 130 BPM).
        let wav = std::env::temp_dir().join(format!("os-engine-clip-inside-{}.wav", std::process::id()));
        let ramp: Vec<f32> = (0..48000).flat_map(|i| [i as f32 / 48000.0; 2]).collect();
        crate::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &ramp).unwrap();
        let mut p = song(vec![], vec![Clip::new(0, 192, 0, ClipSource::Channel(0))]);
        p.channels[0].kind = ChannelKind::AudioClip;
        p.channels[0].sample_path = Some(wav.to_string_lossy().into_owned());
        let mut lib = SampleLibrary::new(None);
        let f = |t: u64| crate::clock::Clock::frame_of_tick(48000, 96, 130.0, t) as usize;
        // From the top, then from tick 96: the second sounds as the first does from that tick on.
        let whole = Engine::new(&p, &mut lib, 48000, PlayMode::Song).render_all(480);
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Song);
        e.seek_tick(96);
        let inside = e.render_all(480);
        assert!(whole[f(96) * 2] > 0.1, "the clip sounds at tick 96");
        // After the 5 ms fade a late start begins with (240 frames at 48 kHz), the two agree.
        let worst = inside[480..2480].iter().zip(&whole[f(96) * 2 + 480..]).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(worst < 1e-6, "started inside, the clip plays from where it had got to (off by {worst})");
        assert!(inside[0].abs() < 1e-6 && inside[240] < whole[f(96) * 2 + 240], "the late start fades in");
        // A hosted sampler is told where in the sample to start.
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
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Song);
        let log = Arc::new(std::sync::Mutex::new(Log::default()));
        e.set_instrument(0, log.clone());
        e.seek_tick(96);
        e.render_all(480);
        let start = f(96) as f32 / 48000.0;
        assert_eq!(log.lock().unwrap().0, vec![(0, "from", start), ((f(192) - f(96)) as u64, "choke", 0.0)]);
        // A clip that is over, or whose sample has run out, does not start.
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Song);
        e.seek_tick(192);
        assert!(e.render_all(480).iter().all(|&v| v == 0.0), "a clip that is over stays silent");
        p.arrangements[0].clips[0].length = 384;
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Song);
        e.seek_tick(300);
        assert!(e.late.is_empty(), "the sample has run out by tick 300");
        let _ = std::fs::remove_file(&wav);
    }

    /// Frame of tick `t` at 48 kHz, PPQ 96, 130 BPM (the test projects').
    fn frame_48k(t: u64) -> u64 {
        crate::clock::Clock::frame_of_tick(48000, 96, 130.0, t)
    }

    /// `p` played by a recorder on channel 0 from tick `at`, with "Play truncated notes on transport" `on`: its note
    /// ons and note offs as (frame from the start, key).
    fn started_at(p: &Project, mode: PlayMode, on: bool, at: u64) -> (Vec<(u64, u8)>, Vec<(u64, u8)>, Vec<f32>) {
        let mut e = Engine::new(p, &mut SampleLibrary::default(), 48000, mode);
        let rec = Arc::new(std::sync::Mutex::new(Rec::default()));
        e.set_instrument(0, rec.clone());
        e.set_truncated_on_transport(on);
        e.seek_tick(at);
        let out = e.render_all(512);
        let keys = |kind: &str| got(&rec, kind).into_iter().map(|g| (g.0, g.1)).collect();
        (keys("on"), keys("off"), out)
    }

    // spec: ui/settings-choices.md "Transport: Play truncated notes and Reset plugins"
    /// "Play truncated notes on transport", on: started in the middle of a 2-bar note, the note sounds at once (a new
    /// note on at the first frame) and ends at its own end. Off, it is skipped and the channel stays silent. A note
    /// that is over is not started, one that starts on the place plays once, and a note repeat starts again once.
    #[test]
    fn truncated_notes_on_transport_start_the_notes_under_way() {
        let mut p = project(vec![long_note(0, 768, 0)], vec![]);
        p.channels[0].kind = ChannelKind::Plugin;
        // Four bars: the note does not end with the song (which lets go of every note on its last tick).
        p.patterns[0].length = Some(4 * 384);
        let (on, off, out) = started_at(&p, PlayMode::Pattern(1), true, 384);
        assert_eq!(on, vec![(0, 60)], "sounds at once");
        assert_eq!(off, vec![(frame_48k(768) - frame_48k(384), 60)], "ends at its own end");
        assert!(out[..1024].iter().any(|&v| v != 0.0), "heard in the first block");
        let (on, off, out) = started_at(&p, PlayMode::Pattern(1), false, 384);
        assert!(on.is_empty() && off.is_empty() && out.iter().all(|&v| v == 0.0), "off: skipped");
        p.patterns[0].notes = vec![long_note(0, 96, 0), Note { key: 62, ..long_note(384, 96, 0) }];
        let (on, _, _) = started_at(&p, PlayMode::Pattern(1), true, 384);
        assert_eq!(on, vec![(0, 62)], "the note that is over is not started, the one on the place plays once");
        // Repeats every 96 ticks (0, 96, 192, 288), all to tick 384: from 200, one note now, the next repeat at 288.
        p.patterns[0].notes = vec![long_note(0, 384, 1)];
        let (on, _, _) = started_at(&p, PlayMode::Pattern(1), true, 200);
        assert_eq!(on, vec![(0, 60), (frame_48k(288) - frame_48k(200), 60)]);
    }

    /// On a sampler channel the voice started in the middle of a note starts afresh: its sample from frame 0 and its
    /// envelopes from the top, frame for frame as the note sounded from its own start. A step and a slide note under
    /// way are not started again. A layer's note goes through the layer to its children. In song mode, the notes are
    /// those the clip-edge rule (event 30) placed.
    #[test]
    fn truncated_notes_on_transport_start_afresh() {
        use os_model::{Clip, ClipSource};
        // A one-second ramp (each frame's value is its time): any other place in the sample sounds different.
        let wav = std::env::temp_dir().join(format!("os-engine-truncated-transport-{}.wav", std::process::id()));
        let ramp: Vec<f32> = (0..48000).flat_map(|i| [i as f32 / 48000.0; 2]).collect();
        crate::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &ramp).unwrap();
        let mut p = project(vec![long_note(0, 768, 0)], vec![]);
        p.channels[0].sample_path = Some(wav.to_string_lossy().into_owned());
        let mut lib = SampleLibrary::new(None);
        let whole = Engine::new(&p, &mut lib, 48000, PlayMode::Pattern(1)).render_all(480);
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Pattern(1));
        e.set_truncated_on_transport(true);
        e.seek_tick(384);
        let inside = e.render_all(480);
        assert!(whole[4000] > 0.01, "the note sounds");
        let worst = inside[..20000].iter().zip(&whole[..20000]).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(worst < 1e-6, "from sample frame 0, envelopes from the top (off by {worst})");
        // A step and a slide note under way: nothing starts.
        let slide = Note { key: 67, flags: 0x4000 | Note::SLIDE, ..long_note(0, 768, 0) };
        p.patterns[0].notes = vec![Note::step(0, 0), slide];
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Pattern(1));
        e.set_truncated_on_transport(true);
        e.seek_tick(12);
        assert!(e.late.is_empty(), "neither the step nor the slide note starts again");
        assert!(e.render_all(480).iter().all(|&v| v == 0.0));
        let _ = std::fs::remove_file(&wav);

        // A layer's note under way: each child gets it, to its end.
        let mut lp = layered(0, vec![long_note(0, 768, 0)], &[ChannelKind::Plugin, ChannelKind::Plugin]);
        lp.patterns[0].length = Some(4 * 384);
        let (mut e, recs) = recorded(&lp);
        e.set_truncated_on_transport(true);
        e.seek_tick(384);
        e.render_all(512);
        for r in &recs {
            assert_eq!(got(r, "on"), vec![(0, 60, 100.0 / 128.0)]);
            assert_eq!(got(r, "off"), vec![(frame_48k(768) - frame_48k(384), 60, 0.0)]);
        }

        // Song mode: a clip from tick 384 showing pattern ticks 100..200 (spec: audio/song-structure.md "Play truncated
        // notes"). Started at 420, the note that began before the window (key 60) is under way only when event 30 put
        // it in the clip.
        let mut c = Clip::new(384, 100, 0, ClipSource::Pattern(1));
        c.window = Some((100.0, 200.0));
        let mut sp = song(vec![long_note(0, 200, 0), Note { key: 62, ..long_note(150, 200, 0) }], vec![c]);
        sp.channels[0].kind = ChannelKind::Plugin;
        let later = frame_48k(434) - frame_48k(420);
        assert_eq!(started_at(&sp, PlayMode::Song, true, 420).0, vec![(later, 62)], "event 30 off: not in the clip");
        sp.play_truncated = true;
        assert_eq!(started_at(&sp, PlayMode::Song, true, 420).0, vec![(0, 60), (later, 62)], "event 30 on");
        assert_eq!(started_at(&sp, PlayMode::Song, false, 420).0, vec![(later, 62)], "the transport setting off");
    }

    // spec: audio/audio-clips.md "Fades and clip gain"
    #[test]
    fn audio_clip_fades_and_gain() {
        use os_model::{Clip, ClipSource};
        let wav = std::env::temp_dir().join(format!("os-engine-clip-fades-{}.wav", std::process::id()));
        crate::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &vec![0.25; 96000]).unwrap();
        // A one-bar clip (1.846 s at 130 BPM) of a one-second DC sample: plain, then with fades and gain.
        let mut p = song(vec![], vec![Clip::new(0, 384, 0, ClipSource::Channel(0))]);
        p.channels[0].kind = ChannelKind::AudioClip;
        p.channels[0].sample_path = Some(wav.to_string_lossy().into_owned());
        let mut lib = SampleLibrary::new(None);
        let plain = Engine::new(&p, &mut lib, 48000, PlayMode::Song).render_all(480);
        let c = &mut p.arrangements[0].clips[0];
        c.set_fade_in(Some((100.0, 0.0)));
        c.set_fade_out(Some((50.0, 3.0)));
        c.set_gain(2.0);
        let faded = Engine::new(&p, &mut lib, 48000, PlayMode::Song).render_all(480);
        let g = |f: usize| faded[f * 2] / plain[f * 2];
        assert!(g(0).abs() < 1e-4, "starts silent: {}", g(0));
        let want = |x: f64| 2.0 * os_model::fade_curve(x, 0.0) as f32;
        for f in [1200, 2400, 3600] {
            assert!((g(f) - want(f as f64 / 4800.0)).abs() < 1e-4, "fade-in at {f}: {} vs {}", g(f), want(f as f64 / 4800.0));
        }
        assert!((g(24000) - 2.0).abs() < 1e-4, "the gain in the middle: {}", g(24000));
        // The sample ends after one second, before the clip and its fade-out: the fade-out is measured from the clip end.
        let clip_frames = crate::clock::Clock::frame_of_tick(48000, 96, 130.0, 384) as usize;
        assert!(clip_frames > 48000);
        // The same on a hosted instrument (constant 1.0 while a note is held).
        struct Held(bool);
        impl crate::Instrument for Held {
            fn note_on(&mut self, _o: u32, _k: u8, _v: f32) {
                self.0 = true;
            }
            fn note_off(&mut self, _o: u32, _k: u8) {
                self.0 = false;
            }
            fn choke(&mut self, _o: u32, _k: u8) {
                self.0 = false;
            }
            fn process(&mut self, _rate: u32, l: &mut [f32], r: &mut [f32]) {
                let v = if self.0 { 1.0 } else { 0.0 };
                l.fill(v);
                r.fill(v);
            }
        }
        let run = |p: &Project, lib: &mut SampleLibrary| {
            let mut e = Engine::new(p, lib, 48000, PlayMode::Song);
            e.set_instrument(0, Arc::new(std::sync::Mutex::new(Held(false))));
            e.render_all(480)
        };
        let faded = run(&p, &mut lib);
        let mut q = p.clone();
        let c = &mut q.arrangements[0].clips[0];
        c.set_fade_in(None);
        c.set_fade_out(None);
        c.set_gain(1.0);
        let plain = run(&q, &mut lib);
        let g = |f: usize| faded[f * 2] / plain[f * 2];
        assert!((g(2400) - want(0.5)).abs() < 1e-4, "instrument fade-in midway: {}", g(2400));
        assert!((g(24000) - 2.0).abs() < 1e-4, "instrument gain: {}", g(24000));
        let end = clip_frames - 1200; // half way through the 50 ms fade-out
        let w = 2.0 * os_model::fade_curve(1200.0 / 2400.0, 3.0) as f32;
        assert!((g(end) - w).abs() < 2e-3, "instrument fade-out half way: {} vs {w}", g(end));
        // An instrument that takes voice gains plays the same on its voice, from silence at the start.
        let voiced = |p: &Project, lib: &mut SampleLibrary| {
            let mut e = Engine::new(p, lib, 48000, PlayMode::Song);
            e.set_instrument(0, Arc::new(std::sync::Mutex::new(VoicedSampler::default())));
            e.render_all(480)
        };
        let (faded, plain) = (voiced(&p, &mut lib), voiced(&q, &mut lib));
        let g = |f: usize| faded[f * 2] / plain[f * 2];
        assert!(g(0).abs() < 1e-6, "voice gain at the start: {}", g(0));
        assert!((g(2400) - want(0.5)).abs() < 1e-3, "voice fade-in midway: {}", g(2400));
        assert!((g(24000) - 2.0).abs() < 1e-4, "voice gain: {}", g(24000));
        assert!((g(end) - w).abs() < 2e-3, "voice fade-out half way: {} vs {w}", g(end));
        let _ = std::fs::remove_file(&wav);
    }

    // spec: audio/audio-clips.md "Automatic crossfades"
    #[test]
    fn automatic_crossfades_play_at_unity() {
        use os_model::{Clip, ClipSource};
        let wav = std::env::temp_dir().join(format!("os-engine-crossfades-{}.wav", std::process::id()));
        crate::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &vec![0.25; 2 * 96000]).unwrap();
        // Two one-bar clips of a two-second DC sample on one track, the second from beat 3: 192 ticks of overlap.
        let mut p = song(vec![], vec![Clip::new(0, 384, 0, ClipSource::Channel(0)), Clip::new(192, 384, 0, ClipSource::Channel(0))]);
        p.channels[0].kind = ChannelKind::AudioClip;
        p.channels[0].sample_path = Some(wav.to_string_lossy().into_owned());
        let mut lib = SampleLibrary::new(None);
        let alone = {
            let mut q = p.clone();
            q.arrangements[0].clips.truncate(1);
            Engine::new(&q, &mut lib, 48000, PlayMode::Song).render_all(480)
        };
        let level = alone[24000 * 2];
        assert!(level > 0.1);
        let at = |t: u64| crate::clock::Clock::frame_of_tick(48000, 96, 130.0, t) as usize;
        // Off (a file without event 44): the clips add up where they overlap, no fades.
        let plain = Engine::new(&p, &mut lib, 48000, PlayMode::Song).render_all(480);
        assert!((plain[2 * at(288)] - 2.0 * level).abs() < 1e-4);
        // On: the records hold no fades (the app writes them), the engine plays the crossfade the overlap makes; the two
        // gains add up to 1 at any tension of the later clip's fade-in.
        p.auto_crossfades = Some(true);
        for t in [0.0f32, 2.289, -4.736] {
            p.arrangements[0].clips[1].set_fade_in_tension(t);
            let xf = Engine::new(&p, &mut lib, 48000, PlayMode::Song).render_all(480);
            for tick in [194, 220, 240, 288, 330, 380] {
                let f = at(tick);
                assert!((xf[2 * f] - level).abs() < 2e-4, "t {t} tick {tick}: {} vs {level}", xf[2 * f]);
            }
            // Before the overlap the first clip is at full level. The first clip muted (it still makes the crossfade):
            // the second clip's fade-in alone, at the law's middle in the overlap's middle.
            assert!((xf[2 * at(100)] - level).abs() < 1e-5);
            let mut q = p.clone();
            q.arrangements[0].clips[0].muted = true;
            let second = Engine::new(&q, &mut lib, 48000, PlayMode::Song).render_all(480);
            let mid = level * os_model::fade_curve(0.5, f64::from(t)) as f32;
            assert!((second[2 * at(288)] - mid).abs() < 2e-4, "t {t}: {} vs {mid}", second[2 * at(288)]);
        }
        // An edge set by hand plays as stored, and the switch leaves it: the first clip's fade-out at 50 ms.
        p.arrangements[0].clips[0].set_fade_out(Some((50.0, 0.0)));
        p.arrangements[0].clips[1].set_fade_in_tension(0.0);
        let fx = os_model::crossfade::playing(&p, &p.arrangements[0].clips, 130.0);
        assert_eq!(fx[0].1, Some((50.0, 0.0)));
        assert_eq!(fx[1].0.map(|f| f.0), Some(os_model::crossfade::ms(192, 130.0, 96)));
        let _ = std::fs::remove_file(&wav);
    }

    /// A hosted sampler that takes voice gains as `Instrument::voice_gain` asks (Lemon Sampler's way): every voice
    /// plays 0.5 on both sides (a DC sample) at its own gain, the first it is given at once and each later one reached
    /// over `VOICE_GAIN_FRAMES` frames; a voice's choke ends that voice alone, a key's choke all of them.
    #[derive(Default)]
    struct VoicedSampler {
        /// Takes no voice gains (chokes still reach one voice): the engine shapes the whole output.
        no_gains: bool,
        /// Ends its clip voices at their end itself (`Instrument::voice_ends`; this one does not, it only says so).
        ends: bool,
        /// The ends its voices were given, the voices choked and the frame each choke came on, the frames played.
        stops: Vec<f32>,
        chokes: Vec<(u32, u64)>,
        frames: u64,
        queue: Vec<(u32, VoicedEvent)>,
        /// (name, gain, glide step, glide target, frames left of the glide, a gain was given)
        voices: Vec<(Option<u32>, f32, f32, f32, u32, bool)>,
        /// Voices started, by name.
        started: Vec<Option<u32>>,
    }

    #[derive(Clone, Copy)]
    enum VoicedEvent {
        Start(Option<u32>),
        Gain(u32, f32),
        End(u32),
        EndAll,
    }

    impl crate::Instrument for VoicedSampler {
        fn note_on(&mut self, o: u32, _k: u8, _v: f32) {
            self.queue.push((o, VoicedEvent::Start(None)));
        }
        fn note_off(&mut self, _o: u32, _k: u8) {}
        fn voice_start(&mut self, o: u32, _k: u8, _v: f32, _start: f32, end: f32, _l: crate::NoteLevels, voice: u32) {
            self.queue.push((o, VoicedEvent::Start(Some(voice))));
            self.stops.push(end);
        }
        fn voice_gains(&self) -> bool {
            !self.no_gains
        }
        fn voice_ends(&self) -> bool {
            self.ends
        }
        fn voice_gain(&mut self, o: u32, voice: u32, gain: f32) {
            self.queue.push((o, VoicedEvent::Gain(voice, gain)));
        }
        fn voice_choke(&mut self, o: u32, _k: u8, voice: u32) {
            self.queue.push((o, VoicedEvent::End(voice)));
            self.chokes.push((voice, self.frames + u64::from(o)));
        }
        fn choke(&mut self, o: u32, _k: u8) {
            self.queue.push((o, VoicedEvent::EndAll));
        }
        fn process(&mut self, _rate: u32, l: &mut [f32], r: &mut [f32]) {
            let ramp = crate::instrument::VOICE_GAIN_FRAMES;
            self.queue.sort_by_key(|e| e.0);
            let mut queue = std::mem::take(&mut self.queue).into_iter().peekable();
            for f in 0..l.len() {
                while let Some((_, e)) = queue.next_if(|e| e.0 as usize <= f) {
                    match e {
                        VoicedEvent::Start(id) => {
                            self.voices.push((id, 1.0, 0.0, 1.0, 0, false));
                            self.started.push(id);
                        }
                        VoicedEvent::Gain(id, g) => {
                            for v in self.voices.iter_mut().filter(|v| v.0 == Some(id)) {
                                if v.5 {
                                    (v.2, v.3, v.4) = ((g - v.1) / ramp as f32, g, ramp);
                                } else {
                                    (v.1, v.4, v.5) = (g, 0, true);
                                }
                            }
                        }
                        VoicedEvent::End(id) => self.voices.retain(|v| v.0 != Some(id)),
                        VoicedEvent::EndAll => self.voices.clear(),
                    }
                }
                let mut sum = 0.0;
                for v in &mut self.voices {
                    sum += 0.5 * v.1;
                    if v.4 > 0 {
                        v.1 += v.2;
                        v.4 -= 1;
                        if v.4 == 0 {
                            v.1 = v.3;
                        }
                    }
                }
                (l[f], r[f]) = (sum, sum);
            }
            self.frames += l.len() as u64;
        }
    }

    // spec: audio/audio-clips.md "What a clip plays", "The end of the song"
    /// An instrument that ends a clip's voice at its trimmed end (Lemon Sampler's way) gets that end with the voice, as
    /// a fraction of the sample, and no choke at the clip's end tick: the voice ends by itself. One still sounding at
    /// the song's end is choked on that tick. An instrument that does not is choked at the clip's end tick.
    #[test]
    fn a_clip_voice_that_ends_itself_is_choked_only_at_the_song_end() {
        use os_model::{Clip, ClipSource};
        let wav = std::env::temp_dir().join(format!("os-engine-clip-ends-{}.wav", std::process::id()));
        crate::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &vec![0.25; 2 * 96000]).unwrap();
        let mut first = Clip::new(0, 192, 0, ClipSource::Channel(0));
        first.window = Some((250.0, 1000.0));
        let mut p = song(vec![], vec![first, Clip::new(384, 192, 0, ClipSource::Channel(0))]);
        p.channels[0].kind = ChannelKind::AudioClip;
        p.channels[0].sample_path = Some(wav.to_string_lossy().into_owned());
        let mut lib = SampleLibrary::new(None);
        let at = |t: u64| crate::clock::Clock::frame_of_tick(48000, 96, 130.0, t);
        for ends in [true, false] {
            let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Song);
            let inst = Arc::new(std::sync::Mutex::new(VoicedSampler { ends, ..Default::default() }));
            e.set_instrument(0, inst.clone());
            e.render_all(480);
            drop(e);
            let s = inst.lock().unwrap();
            assert_eq!(s.stops, vec![0.5, 0.0], "the first clip ends 1 s into 2");
            let (a, b) = (s.started[0].unwrap(), s.started[1].unwrap());
            let first_end = if ends { at(576) } else { at(192) };
            let mut chokes = s.chokes.clone();
            chokes.sort_unstable();
            assert_eq!(chokes, vec![(a, first_end), (b, at(576))], "ends {ends}");
        }
        let _ = std::fs::remove_file(&wav);
    }

    // spec: audio/audio-clips.md "Automatic crossfades", "Fades and clip gain"
    /// Two clips of one sample overlap on a hosted sampler: one channel, one key. Each voice gets its own fades, so the
    /// crossfade adds up to 1 as on the engine's own voices, and the first clip's end leaves the second playing to its
    /// own end.
    #[test]
    fn automatic_crossfades_on_a_hosted_sampler_play_at_unity() {
        use os_model::{Clip, ClipSource};
        let wav = std::env::temp_dir().join(format!("os-engine-hosted-crossfades-{}.wav", std::process::id()));
        crate::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &vec![0.25; 2 * 96000]).unwrap();
        let mut p = song(vec![], vec![Clip::new(0, 384, 0, ClipSource::Channel(0)), Clip::new(192, 384, 0, ClipSource::Channel(0))]);
        p.channels[0].kind = ChannelKind::AudioClip;
        p.channels[0].sample_path = Some(wav.to_string_lossy().into_owned());
        p.auto_crossfades = Some(true);
        let mut lib = SampleLibrary::new(None);
        let run = |p: &Project, lib: &mut SampleLibrary| {
            let mut e = Engine::new(p, lib, 48000, PlayMode::Song);
            let inst = Arc::new(std::sync::Mutex::new(VoicedSampler::default()));
            e.set_instrument(0, inst.clone());
            let out = e.render_all(480);
            let started = inst.lock().unwrap().started.clone();
            (out, started)
        };
        let alone = {
            let mut q = p.clone();
            q.arrangements[0].clips.truncate(1);
            run(&q, &mut lib).0
        };
        let level = alone[24000 * 2];
        assert!(level > 0.1);
        let at = |t: u64| crate::clock::Clock::frame_of_tick(48000, 96, 130.0, t) as usize;
        for t in [0.0f32, 2.289, -4.736] {
            p.arrangements[0].clips[1].set_fade_in_tension(t);
            let (xf, started) = run(&p, &mut lib);
            assert_eq!(started.len(), 2);
            assert!(started[0].is_some() && started[1].is_some() && started[0] != started[1], "each clip's voice has its own name: {started:?}");
            // Flat from the first clip's start to the second's end: no dip in the overlap, no gap after the first ends.
            let worst = (0..at(576)).map(|f| (xf[2 * f] - level).abs()).fold(0.0f32, f32::max);
            assert!(worst < 2e-4, "t {t}: off by {worst} (level {level})");
            assert!(xf[2 * (at(576) + 10)].abs() < 1e-6, "silent after the second clip");
            // The second clip alone (the first muted, it still makes the crossfade): its fade-in.
            let mut q = p.clone();
            q.arrangements[0].clips[0].muted = true;
            let second = run(&q, &mut lib).0;
            let mid = level * os_model::fade_curve(0.5, f64::from(t)) as f32;
            assert!((second[2 * at(288)] - mid).abs() < 2e-4, "t {t}: {} vs {mid}", second[2 * at(288)]);
            assert!(second[2 * at(100)].abs() < 1e-6, "nothing before the second clip");
        }
        let _ = std::fs::remove_file(&wav);
    }

    // spec: audio/audio-clips.md "Fades and clip gain"
    /// An instrument without voice gains gets the clips' shapes on its whole output. Where the first of two overlapping
    /// clips ends, the second plays on at its own gain (the first's end no longer silences the channel for its choke's
    /// tail), and once both have ended the channel is silent. In the overlap the two share one gain (not tested: it
    /// dips, a known limit of such instruments).
    #[test]
    fn an_instrument_without_voice_gains_plays_the_second_clip_on() {
        use os_model::{Clip, ClipSource};
        let wav = std::env::temp_dir().join(format!("os-engine-unvoiced-crossfades-{}.wav", std::process::id()));
        crate::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &vec![0.25; 2 * 96000]).unwrap();
        let mut p = song(vec![], vec![Clip::new(0, 384, 0, ClipSource::Channel(0)), Clip::new(192, 384, 0, ClipSource::Channel(0))]);
        p.channels[0].kind = ChannelKind::AudioClip;
        p.channels[0].sample_path = Some(wav.to_string_lossy().into_owned());
        p.auto_crossfades = Some(true);
        let mut lib = SampleLibrary::new(None);
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Song);
        e.set_instrument(0, Arc::new(std::sync::Mutex::new(VoicedSampler { no_gains: true, ..Default::default() })));
        let out = e.render_all(480);
        let at = |t: u64| crate::clock::Clock::frame_of_tick(48000, 96, 130.0, t) as usize;
        // A voice plays 0.5; before the overlap and after the first clip's end one voice plays at gain 1.
        assert!((out[2 * at(100)] - 0.5).abs() < 1e-4, "before the overlap: {}", out[2 * at(100)]);
        for tick in [390, 420, 480, 560] {
            assert!((out[2 * at(tick)] - 0.5).abs() < 1e-4, "tick {tick}, after the first clip: {}", out[2 * at(tick)]);
        }
        assert!(out[2 * (at(576) + 10)].abs() < 1e-6, "silent after the second clip");
        let _ = std::fs::remove_file(&wav);
    }

    // spec: audio/audio-clips.md "Stretching by resampling (mode 0)"
    #[test]
    fn resample_stretch_multiplier_slows_the_clip() {
        use os_model::{Clip, ClipSource};
        let wav = std::env::temp_dir().join(format!("os-engine-stretch-{}.wav", std::process::id()));
        let ramp: Vec<f32> = (0..48000).flat_map(|i| [i as f32 / 48000.0; 2]).collect();
        crate::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &ramp).unwrap();
        let mut p = song(vec![], vec![Clip::new(0, 384, 0, ClipSource::Channel(0))]);
        p.channels[0].kind = ChannelKind::AudioClip;
        p.channels[0].sample_path = Some(wav.to_string_lossy().into_owned());
        let mut lib = SampleLibrary::new(None);
        let plain = Engine::new(&p, &mut lib, 48000, PlayMode::Song).render_all(480);
        p.channels[0].sound.set_stretch_multiplier(2.0);
        assert_eq!(p.channels[0].sound.stretch, [0, 0, 10000, 0], "stored as log2(2) * 10000");
        let slow = Engine::new(&p, &mut lib, 48000, PlayMode::Song).render_all(480);
        // Twice as long: frame 2f of the stretched clip is frame f of the plain one.
        for f in [1000usize, 4000, 12000] {
            assert!((slow[4 * f] - plain[2 * f]).abs() < 1e-3, "frame {}: {} vs {}", 2 * f, slow[4 * f], plain[2 * f]);
        }
        let _ = std::fs::remove_file(&wav);
    }

    /// A 48 kHz wav of sine parts (Hz, seconds), removed on drop.
    struct Sines(std::path::PathBuf);

    impl Sines {
        fn new(name: &str, parts: &[(f64, f64)]) -> Sines {
            let path = std::env::temp_dir().join(format!("os-engine-{name}-{}.wav", std::process::id()));
            let mut frames = Vec::new();
            for &(hz, secs) in parts {
                for i in 0..(secs * 48000.0) as usize {
                    let v = (2.0 * std::f64::consts::PI * hz * i as f64 / 48000.0).sin() as f32 * 0.5;
                    frames.extend_from_slice(&[v, v]);
                }
            }
            crate::wav::write_f32_stereo(&mut std::fs::File::create(&path).unwrap(), 48000, &frames).unwrap();
            Sines(path)
        }

        fn path(&self) -> Option<String> {
            Some(self.0.to_string_lossy().into_owned())
        }
    }

    impl Drop for Sines {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// The left channel of interleaved `out` (`rate` Hz) between two times in seconds.
    fn left(out: &[f32], rate: f64, from: f64, to: f64) -> Vec<f32> {
        let (a, b) = ((from * rate) as usize, ((to * rate) as usize).min(out.len() / 2));
        (a..b).map(|i| out[2 * i]).collect()
    }

    /// Frequency from the rising zero crossings.
    fn hz(x: &[f32], rate: f64) -> f64 {
        let ups: Vec<usize> = x.windows(2).enumerate().filter(|(_, w)| w[0] < 0.0 && w[1] >= 0.0).map(|(i, _)| i).collect();
        assert!(ups.len() > 2, "no tone");
        (ups.len() - 1) as f64 * rate / (ups[ups.len() - 1] - ups[0]) as f64
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    // spec: audio/time-stretch.md "How each method plays"
    #[test]
    fn stretch_methods_change_the_length_not_the_pitch() {
        use os_model::{Clip, ClipSource, stretch_mode};
        let wav = Sines::new("stretch-len", &[(440.0, 1.0)]);
        for mode in [stretch_mode::AUTO, stretch_mode::E3_GENERIC, stretch_mode::STRETCH] {
            let mut p = song(vec![], vec![Clip::new(0, 384 * 2, 0, ClipSource::Channel(0))]);
            p.channels[0].kind = ChannelKind::AudioClip;
            p.channels[0].sample_path = wav.path();
            // Twice as long.
            p.channels[0].sound.stretch = [0, 0, 10000, mode as u32];
            let mut lib = SampleLibrary::new(None);
            let out = Engine::new(&p, &mut lib, 48000, PlayMode::Song).render_all(480);
            let f = hz(&left(&out, 48000.0, 0.3, 1.7), 48000.0);
            assert!((f - 440.0).abs() < 2.0, "mode {mode}: {f} Hz");
            assert!(rms(&left(&out, 48000.0, 1.5, 1.9)) > 0.1, "mode {mode}: still playing after the source's length");
            assert!(rms(&left(&out, 48000.0, 2.2, 2.6)) < 0.01, "mode {mode}: over after twice its length");
        }
    }

    /// Knobs moved while a note plays: the engine that takes them in place renders what a new engine takes over with.
    #[test]
    fn an_edit_in_place_sounds_as_a_new_engine() {
        let wav = Sines::new("live-edit", &[(440.0, 3.0)]);
        let mut p = project(vec![long_note(0, 384 * 4, 0)], vec![]);
        p.channels[0].sample_path = wav.path();
        p.mixer = (0..3).map(os_model::MixerTrack::new).collect();
        let mut edited = p.clone();
        let edits = [LiveEdit::ChannelVolume(0, 0.3), LiveEdit::ChannelPan(0, -0.4), LiveEdit::ChannelPitch(0, 700), LiveEdit::MixerVolume(1, 0.5), LiveEdit::MixerPan(1, 0.25), LiveEdit::MixerSeparation(0, 0.5)];
        for e in edits {
            e.apply_to(&mut edited);
        }
        let mut lib = SampleLibrary::new(None);
        let (mut a, mut b) = (Engine::new(&p, &mut lib, 44100, PlayMode::Pattern(1)), Engine::new(&p, &mut lib, 44100, PlayMode::Pattern(1)));
        let (mut x, mut y) = (Vec::new(), Vec::new());
        a.render_ticks(96, &mut x);
        b.render_ticks(96, &mut y);
        assert_eq!(x, y);
        for e in edits {
            a.apply_edit(e);
        }
        let mut c = Engine::new(&edited, &mut lib, 44100, PlayMode::Pattern(1));
        c.continue_from(&mut b);
        let (mut x, mut y) = (Vec::new(), Vec::new());
        a.render_ticks(192, &mut x);
        c.render_ticks(192, &mut y);
        assert_eq!(x.len(), y.len());
        let worst = x.iter().zip(&y).map(|(m, n)| (m - n).abs()).fold(0.0f32, f32::max);
        assert!(worst < 1e-5, "differ by {worst}");
        assert!(rms(&x) > 0.001, "something plays");
    }

    /// Levels alone are edits in place; anything else, or nothing at all, is a new engine.
    #[test]
    fn only_levels_are_edits_in_place() {
        let mut p = project(vec![long_note(0, 384, 0)], vec![]);
        p.mixer = (0..3).map(os_model::MixerTrack::new).collect();
        let mut sent = p.clone();
        let mut new = p.clone();
        new.channels[0].volume = 0.25;
        new.mixer[1].pan = Some(0.5);
        assert_eq!(live_edits(&mut sent, &new), Some(vec![LiveEdit::ChannelVolume(0, 0.25), LiveEdit::MixerPan(1, 0.5)]));
        assert!(sent == new, "taken into what was sent");
        assert_eq!(live_edits(&mut sent, &new), None, "nothing changed");
        new.channels[0].volume = 0.5;
        new.channels[0].name = Some("x".into());
        assert_eq!(live_edits(&mut sent, &new), None, "a name too");
        let mut sent = p.clone();
        let mut new = p.clone();
        new.channels[0].volume = 0.5;
        new.patterns[0].notes.clear();
        assert_eq!(live_edits(&mut sent, &new), None, "notes too");
    }

    // spec: audio/time-stretch.md "How each method plays" (realtime methods: the key moves the pitch, not the length)
    #[test]
    fn realtime_stretch_keeps_the_length_when_the_key_moves() {
        use os_model::stretch_mode;
        let wav = Sines::new("stretch-key", &[(440.0, 1.0)]);
        // An octave up by the key, then by the channel pitch: both play 880 Hz for the whole second.
        for (key, cents) in [(72u16, 0i32), (60, 1200)] {
            let mut p = project(vec![Note { key, ..long_note(0, 384, 0) }], vec![]);
            p.channels[0].sample_path = wav.path();
            p.channels[0].pitch_cents = cents;
            p.channels[0].sound.stretch = [0, 0, 0, stretch_mode::STRETCH as u32];
            let out = render(&p);
            let f = hz(&left(&out, 44100.0, 0.1, 0.9), 44100.0);
            assert!((f - 880.0).abs() < 4.0, "key {key} pitch {cents}: {f} Hz");
            assert!(rms(&left(&out, 44100.0, 0.7, 0.9)) > 0.1, "key {key} pitch {cents}: one second long");
            assert!(rms(&left(&out, 44100.0, 1.2, 1.4)) < 0.01, "key {key} pitch {cents}: and no longer");
        }
        // Plain playback for comparison: the key resamples, half as long.
        let mut p = project(vec![Note { key: 72, ..long_note(0, 384, 0) }], vec![]);
        p.channels[0].sample_path = wav.path();
        let out = render(&p);
        assert!(rms(&left(&out, 44100.0, 0.6, 0.8)) < 0.01, "resampled: over after half a second");
    }

    // spec: audio/time-stretch.md "Trims"
    #[test]
    fn trims_of_a_stretched_clip_are_source_time() {
        use os_model::{Clip, ClipSource, stretch_mode};
        let wav = Sines::new("stretch-trim", &[(440.0, 0.5), (880.0, 0.5)]);
        let mut clip = Clip::new(0, 384, 0, ClipSource::Channel(0));
        // Starts half a second into the source: its 880 Hz half, stretched to a second.
        clip.window = Some((500.0, 1000.0));
        let mut p = song(vec![], vec![clip]);
        p.channels[0].kind = ChannelKind::AudioClip;
        p.channels[0].sample_path = wav.path();
        p.channels[0].sound.stretch = [0, 0, 10000, stretch_mode::AUTO as u32];
        let mut lib = SampleLibrary::new(None);
        let out = Engine::new(&p, &mut lib, 48000, PlayMode::Song).render_all(480);
        let f = hz(&left(&out, 48000.0, 0.1, 0.8), 48000.0);
        assert!((f - 880.0).abs() < 4.0, "{f} Hz");
    }

    // spec: audio/timing.md "Tempo that changes"
    #[test]
    fn tempo_automation_resizes_the_ticks() {
        use os_model::{AutoPoint, Automation, ChannelKind, Clip, ClipSource, target};
        let mut p = song(vec![long_note(0, 384 * 4, 0)], vec![Clip::new(0, 384 * 4, 0, ClipSource::Pattern(1))]);
        let mut auto_ch = channel();
        auto_ch.index = 1;
        auto_ch.kind = ChannelKind::Automation;
        // A new clip's range, 0..1 (stored pan 0, volume 12800).
        (auto_ch.pan, auto_ch.volume) = (-1.0, 1.0);
        p.channels.push(auto_ch);
        // A flat tempo clip at 0.25: (10000 + Round(0.25 * 512000)) / 1000 = 138 BPM.
        p.automations.push(Automation { channel: 1, targets: vec![target::TEMPO], points: vec![AutoPoint::new(0.0, 0.25), AutoPoint::new(16.0, 0.25)], header: vec![0; 21], tail: vec![], lfo: Default::default() });
        p.arrangements[0].clips.push(Clip::new(0, 384 * 4, 1, ClipSource::Channel(1)));
        let mut lib = SampleLibrary::new(None);
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Song);
        let mut out = vec![0.0f32; 480 * 2];
        while e.tick < 384 * 4 {
            e.process(&mut out);
        }
        // The first tick is sized at 130 BPM, every later one at 138.
        let mut c = crate::clock::Clock::new(48000, 96, 130.0);
        c.advance();
        c.set_tempo(48000, 96, 138.0);
        // (Processing runs in blocks, so the engine may be a tick or two past the four bars.)
        for _ in 1..e.tick {
            c.advance();
        }
        assert_eq!(e.clock.next_start(), c.next_start(), "{} ticks, all but the first at 138 BPM", e.tick);
        assert_eq!(e.tempo, 138.0);
    }

    // spec: ui/playlist.md §8 (automation clips drive their target), flp/events/automation.md (value scaling)
    #[test]
    fn automation_clips_drive_channel_and_mixer() {
        use os_model::{AutoPoint, Automation, ChannelKind, Clip, ClipSource, target};
        let mut p = song(vec![long_note(0, 384 * 4, 0)], vec![Clip::new(0, 384 * 4, 0, ClipSource::Pattern(1))]);
        let mut auto_ch = channel();
        auto_ch.index = 1;
        auto_ch.kind = ChannelKind::Automation;
        // A new clip's range, 0..1 (stored pan 0, volume 12800).
        (auto_ch.pan, auto_ch.volume) = (-1.0, 1.0);
        p.channels.push(auto_ch);
        p.mixer = (0..2)
            .map(os_model::MixerTrack::new)
            .collect();
        // Channel volume falls from 1.0 to 0.0 over 8 beats, starting at bar 2.
        let a = Automation { channel: 1, targets: vec![target::channel(0, 0)], points: vec![AutoPoint::new(0.0, 1.0), AutoPoint::new(8.0, 0.0)], header: vec![0; 21], tail: vec![], lfo: Default::default() };
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
        p.automations[0].targets = vec![target::mixer(1, target::MIXER_PAN)];
        p.automations[0].points = vec![AutoPoint::new(0.0, 0.0), AutoPoint::new(8.0, 0.0)];
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Song);
        e.seek(laws::tick_to_frame(400.0, 96, 130.0, 48000));
        e.process(&mut out);
        let m = e.tracks[1].matrix;
        assert!(m[1][0].abs() < 1e-6 && m[1][1].abs() < 1e-6, "full left: no right output {m:?}");
        let _ = &mut out;
    }

    /// Adds an automation clip channel (the next rack row) driving `target` along `points`, and its clip at `start` for
    /// `len` ticks on playlist track 2.
    fn add_auto(p: &mut Project, target: u32, points: Vec<os_model::AutoPoint>, start: u32, len: u32) {
        use os_model::{Automation, Clip, ClipSource};
        let index = p.channels.len() as u16;
        let mut c = channel();
        (c.index, c.kind) = (index, ChannelKind::Automation);
        // A new clip's range, 0..1 (stored pan 0, volume 12800).
        (c.pan, c.volume) = (-1.0, 1.0);
        p.channels.push(c);
        p.automations.push(Automation { channel: index, targets: vec![target], points, header: vec![0; 21], tail: vec![], lfo: Default::default() });
        p.arrangements[0].clips.push(Clip::new(start, len, 1, ClipSource::Channel(index)));
    }

    /// A flat curve at `v`.
    fn flat(v: f64) -> Vec<os_model::AutoPoint> {
        vec![os_model::AutoPoint::new(0.0, v), os_model::AutoPoint::new(64.0, v)]
    }

    /// Plays `p` (channel 0 a probe instrument) from tick `from` for `ticks` ticks; the note ons' and offs' ticks.
    fn probe_ticks(p: &Project, mode: PlayMode, from: u64, ticks: f64) -> (Vec<f64>, Vec<f64>) {
        let mut e = Engine::new(p, &mut SampleLibrary::default(), 48000, mode);
        let probe = Arc::new(std::sync::Mutex::new(Probe { log: vec![], held: vec![], at: 0 }));
        e.set_instrument(0, probe.clone());
        e.seek_tick(from);
        let start = e.frame;
        let mut out = vec![0.0f32; 256 * 2];
        while e.tick_position() < from as f64 + ticks {
            e.process(&mut out);
        }
        // Back from frames to ticks (the tempo does not change).
        let tick = |f: u64| ((f + start) as f64 * 130.0 * 96.0 / (60.0 * 48000.0)).round();
        let log = probe.lock().unwrap().log.clone();
        (log.iter().filter(|l| l.1).map(|l| tick(l.0)).collect(), log.iter().filter(|l| !l.1).map(|l| tick(l.0)).collect())
    }

    /// The main swing as a target (spec: flp/events/automation.md "Targets": `v × 128`). A change while the song plays
    /// places the notes not started yet again (Lemon Studio's design, `Engine::set_swing`): before the clip the stored
    /// swing, from it on the clip's, after it the clip's last value, and a new position past it starts with that value
    /// (audio/automation.md "A new position"). A note the change moves to before now starts now. A control point
    /// (event 11's unit) does the same.
    #[test]
    fn main_swing_automation_places_the_notes_not_started_yet() {
        use os_model::{target, Clip, ClipSource, ControlPoint};
        // Off-beat sixteenths of a bar played twice; swing 32 moves them 3 ticks, 64 six, 128 twelve.
        let notes: Vec<Note> = [24, 72, 120, 168].into_iter().map(|t| Note::step(0, t)).collect();
        let mut p = song(notes.clone(), vec![Clip::new(0, 384, 0, ClipSource::Pattern(1)), Clip::new(384, 384, 0, ClipSource::Pattern(1))]);
        p.channels[0].kind = ChannelKind::Plugin;
        p.swing = Some(32);
        add_auto(&mut p, target::MAIN_SWING, flat(1.0), 96, 192);
        let (ons, _) = probe_ticks(&p, PlayMode::Song, 0, 760.0);
        assert_eq!(ons, [27.0, 75.0, 132.0, 180.0, 420.0, 468.0, 516.0, 564.0]);
        let (ons, _) = probe_ticks(&p, PlayMode::Song, 300, 400.0);
        assert_eq!(ons, [420.0, 468.0, 516.0, 564.0], "a new position past the clip");
        // 0.25 → 32: the swing the project has, nothing moves.
        p.automations[0].points = flat(0.25);
        assert_eq!(probe_ticks(&p, PlayMode::Song, 0, 380.0).0, [27.0, 75.0, 123.0, 171.0]);
        // Down to 0 at tick 30, with the first note due at 36 at full swing: it starts at once.
        p.swing = Some(128);
        p.automations[0].points = flat(0.0);
        p.arrangements[0].clips[2].position = 30;
        assert_eq!(probe_ticks(&p, PlayMode::Song, 0, 380.0).0, [30.0, 72.0, 120.0, 168.0]);

        // A control point at tick 48: 64 for the notes from there.
        let mut p = project(notes, vec![ControlPoint::new(48, 0x4000, 1, 64)]);
        p.channels[0].kind = ChannelKind::Plugin;
        p.swing = Some(32);
        assert_eq!(probe_ticks(&p, PlayMode::Pattern(1), 0, 380.0).0, [27.0, 78.0, 126.0, 174.0]);
    }

    /// A channel's mixer track as a target (the rack's mixer box; spec: flp/events/automation.md "Targets": `v × 500` to
    /// the nearest track, at most the last insert): the channel's voices go to that track from the tick the value
    /// arrives, those already sounding included; after the clip the track stays, and a new position past the clip
    /// starts there. A control point holds the track number.
    #[test]
    fn mixer_track_automation_routes_the_channel() {
        use os_model::{target, Clip, ClipSource, ControlPoint, MixerTrack};
        let mut p = song(vec![long_note(0, 384 * 2, 0)], vec![Clip::new(0, 384 * 2, 0, ClipSource::Pattern(1))]);
        // Master, 16 inserts and the last track; what goes to Insert 2 is not heard (its switch is off).
        p.mixer = (0..18).map(MixerTrack::new).collect();
        p.mixer[2].enabled = false;
        add_auto(&mut p, target::channel(0, target::CHANNEL_MIXER_TRACK), flat(0.004), 96, 96);
        let mut e = Engine::new(&p, &mut SampleLibrary::new(None), 48000, PlayMode::Song);
        let mut out = Vec::new();
        e.render_ticks(90, &mut out);
        assert_eq!(e.channels[0].insert, 0);
        assert!(rms(&out[out.len() - 2000..]) > 0.01, "heard on the master");
        out.clear();
        e.render_ticks(200, &mut out);
        assert_eq!(e.channels[0].insert, 2, "Insert 2 (0.004), and after the clip too");
        assert!(rms(&out[out.len() / 2..]) < 1e-5, "the sounding note went there");
        // A new position past the clip: Insert 2 at once; before it, the channel's own track.
        e.seek_tick(500);
        assert_eq!(e.channels[0].insert, 2);
        e.seek_tick(50);
        assert_eq!(e.channels[0].insert, 0);
        // The measured values: 0.0051 → 3; 0.05 and 1 → 16, the last insert.
        for (v, track) in [(0.0051, 3), (0.05, 16), (1.0, 16), (0.0, 0)] {
            p.automations[0].points = flat(v);
            let mut e = Engine::new(&p, &mut SampleLibrary::new(None), 48000, PlayMode::Song);
            e.seek_tick(400);
            assert_eq!(e.channels[0].insert, track, "{v}");
        }
        // A control point: Insert 5 from tick 96.
        let mut p = project(vec![long_note(0, 384, 0)], vec![ControlPoint::new(96, 0, target::CHANNEL_MIXER_TRACK, 5)]);
        p.mixer = (0..18).map(MixerTrack::new).collect();
        let mut e = Engine::new(&p, &mut SampleLibrary::new(None), 48000, PlayMode::Pattern(1));
        e.render_ticks(96, &mut Vec::new());
        assert_eq!(e.channels[0].insert, 0);
        e.render_ticks(1, &mut Vec::new());
        assert_eq!(e.channels[0].insert, 5);
    }

    /// A channel's switch as a target (the rack's LED; spec: flp/events/automation.md "Targets": on from 0.5;
    /// audio/voice-levels.md, the switch): off, its sounding notes end on that tick and the notes that start while it is
    /// off do not play; on again, its later notes play. By clip and by control point alike.
    #[test]
    fn channel_switch_automation_mutes_and_unmutes_the_channel() {
        use os_model::{target, Clip, ClipSource, ControlPoint};
        let notes = vec![Note { key: 60, ..long_note(0, 192, 0) }, Note { key: 62, ..long_note(96, 48, 0) }, Note { key: 64, ..long_note(288, 48, 0) }];
        let mut p = song(notes.clone(), vec![Clip::new(0, 384, 0, ClipSource::Pattern(1))]);
        p.channels[0].kind = ChannelKind::Plugin;
        let on = target::channel(0, target::CHANNEL_ON);
        add_auto(&mut p, on, flat(0.4999), 48, 96);
        add_auto(&mut p, on, flat(0.5), 240, 24);
        let (ons, offs) = probe_ticks(&p, PlayMode::Song, 0, 380.0);
        assert_eq!((ons.clone(), offs.clone()), (vec![0.0, 288.0], vec![48.0, 336.0]), "off at 48 (its note ends there), on at 240");
        // Pattern control points: 0 at tick 48, 1 at tick 240.
        let mut q = project(notes, vec![ControlPoint::new(48, 0, target::CHANNEL_ON, 0), ControlPoint::new(240, 0, target::CHANNEL_ON, 1)]);
        q.channels[0].kind = ChannelKind::Plugin;
        assert_eq!(probe_ticks(&q, PlayMode::Pattern(1), 0, 380.0), (ons, offs));
        // A sampler channel goes silent at once and stays so while off.
        let mut p = song(vec![long_note(0, 384, 0)], vec![Clip::new(0, 384, 0, ClipSource::Pattern(1))]);
        add_auto(&mut p, on, flat(0.0), 96, 96);
        let mut e = Engine::new(&p, &mut SampleLibrary::new(None), 48000, PlayMode::Song);
        let mut out = Vec::new();
        e.render_ticks(90, &mut out);
        assert!(rms(&out[out.len() - 2000..]) > 0.01);
        out.clear();
        e.render_ticks(100, &mut out);
        assert!(!e.channels[0].enabled);
        assert!(rms(&out[out.len() / 2..]) < 1e-6, "off");
    }

    // spec: flp/events/automation.md "Event 227" (one link per target); audio/automation.md "The link", "When a clip acts"
    #[test]
    fn an_automation_clip_drives_every_target_it_is_linked_to() {
        use os_model::{AutoPoint, Automation, ChannelKind, Clip, ClipSource, target};
        let mut p = song(vec![long_note(0, 384 * 4, 0)], vec![Clip::new(0, 384 * 4, 0, ClipSource::Pattern(1))]);
        for i in [1u16, 2] {
            let mut c = channel();
            c.index = i;
            c.kind = ChannelKind::Automation;
            // A new clip's range, 0..1 (stored pan 0, volume 12800).
            (c.pan, c.volume) = (-1.0, 1.0);
            p.channels.push(c);
        }
        p.mixer = (0..3).map(os_model::MixerTrack::new).collect();
        let flat = |v| vec![AutoPoint::new(0.0, v), AutoPoint::new(8.0, v)];
        // One clip on the channel volume and two mixer volumes, an unknown target among them.
        let targets = vec![target::channel(0, 0), target::mixer(1, target::MIXER_VOLUME), 0x7FFF_FFFF, target::mixer(2, target::MIXER_VOLUME)];
        p.automations.push(Automation { channel: 1, targets, points: flat(0.25), header: vec![0; 21], tail: vec![], lfo: Default::default() });
        // A later clip on mixer track 2 only: it decides that target while the first keeps the others.
        p.automations.push(Automation { channel: 2, targets: vec![target::mixer(2, target::MIXER_VOLUME)], points: flat(0.5), header: vec![0; 21], tail: vec![], lfo: Default::default() });
        p.arrangements[0].clips.push(Clip::new(0, 768, 1, ClipSource::Channel(1)));
        p.arrangements[0].clips.push(Clip::new(96, 768, 2, ClipSource::Channel(2)));
        let mut lib = SampleLibrary::new(None);
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Song);
        assert_eq!(e.report.automation_clips_ignored, 0);
        e.next_tick();
        // Round(16000 v) for the mixer, Round(12800 v) through the level curve for the channel.
        assert_eq!((e.tracks[1].vol, e.tracks[2].vol), (4000, 4000));
        assert!((e.channels[0].vol_v - f64::from(os_dsp::level::knob(3200, os_dsp::level::R12800))).abs() < 1e-9);
        while e.tick <= 100 {
            e.next_tick();
        }
        assert_eq!((e.tracks[1].vol, e.tracks[2].vol), (4000, 8000));
        // A clip whose targets are all unknown is ignored.
        p.automations[1].targets = vec![0x7FFF_FFFF];
        let e = Engine::new(&p, &mut lib, 48000, PlayMode::Song);
        assert_eq!((e.autos.len(), e.report.automation_clips_ignored), (1, 1));
    }

    /// Passes the sound through and counts how often it was emptied.
    struct Emptied(Arc<std::sync::atomic::AtomicUsize>);

    impl crate::instrument::HostedEffect for Emptied {
        fn process_fx(&mut self, _rate: u32, _left: &mut [f32], _right: &mut [f32]) {}

        fn reset_fx(&mut self) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    // spec: audio/mixer.md "Changes while the song plays", "An effect slot", "Equaliser"; flp/events/automation.md "Targets"
    #[test]
    fn automation_drives_mixer_slots_equaliser_sends_and_separation() {
        use os_model::{AutoPoint, Automation, ChannelKind, Clip, ClipSource, target};
        let mut p = song(vec![long_note(0, 384 * 4, 0)], vec![Clip::new(0, 384 * 4, 0, ClipSource::Pattern(1))]);
        p.channels[0].mixer_track = 1;
        p.mixer = (0..3).map(os_model::MixerTrack::new).collect();
        // A send at level 0 (sidechain only) passes nothing until automation raises it.
        p.mixer[1].routes = vec![0, 2];
        p.mixer[1].send_levels = vec![(2, 0.0)];
        let clips = [
            (target::slot(1, 0, target::SLOT_MIX), 0.5),
            (target::slot(1, 1, target::SLOT_ON), 0.5),
            (target::mixer(1, target::EQ_GAIN + 1), 0.75),
            (target::mixer(1, target::EQ_FREQ + 2), 0.5),
            (target::send(1, 2), 0.8),
            (target::mixer(1, target::MIXER_SEPARATION), 1.0),
        ];
        for (i, &(t, v)) in clips.iter().enumerate() {
            let index = i as u16 + 1;
            let mut c = channel();
            c.index = index;
            c.kind = ChannelKind::Automation;
            // A new clip's range, 0..1 (stored pan 0, volume 12800).
            (c.pan, c.volume) = (-1.0, 1.0);
            p.channels.push(c);
            let points = vec![AutoPoint::new(0.0, v), AutoPoint::new(8.0, v)];
            p.automations.push(Automation { channel: index, targets: vec![t], points, header: vec![0; 21], tail: vec![], lfo: Default::default() });
            p.arrangements[0].clips.push(Clip::new(384, 768, index, ClipSource::Channel(index)));
        }
        let mut lib = SampleLibrary::new(None);
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Song);
        assert_eq!(e.report.automation_clips_ignored, 0);
        let resets = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        e.set_effect(1, 0, Arc::new(Mutex::new(Halve)));
        e.set_effect(1, 1, Arc::new(Mutex::new(Emptied(resets.clone()))));
        let ticks = |e: &mut Engine, tick: u64| {
            while e.tick <= tick {
                e.next_tick();
            }
        };
        let dry = os_dsp::mixer::dry_level;
        ticks(&mut e, 383);
        let t = &e.tracks[1];
        assert_eq!((t.slot_mix[0], t.slot_on[1], t.eq.settings(1)[0], t.sep), (12800, true, 0, 0), "before the clips: as stored");
        assert_eq!(t.routes.iter().map(|r| (r.dest, r.gain)).collect::<Vec<_>>(), vec![(0, 1.0), (2, 0.0)]);
        ticks(&mut e, 384);
        let t = &e.tracks[1];
        // Mix Round(12800 v); the switch Round(v) = 1 with halves to even (0.5 is off); EQ gain Round((2 v - 1) * 1800),
        // frequency Round(65536 v); send curve(single(v * 1.25)); separation Round((2 v - 1) * 64).
        assert_eq!((t.slot_mix[0], t.effects[0].mix), (6400, 6400));
        assert!(!t.slot_on[1] && !t.effects[1].on);
        assert_eq!((t.eq.settings(1)[0], t.eq.settings(2)[1]), (900, 32768));
        assert!(!t.eq.is_flat());
        assert_eq!(t.routes[1].gain, os_dsp::mixer::send_gain_auto(0.8));
        assert_eq!(t.sep, 64);
        // The dry level and the send glide there: by at most the engine's limit per sample.
        assert!(t.effects[0].dry.current < dry(6400) && t.effects[0].dry.current > 0.0, "dry: {:?}", t.effects[0].dry);
        assert!(t.routes[1].glide.current > 0.0 && t.routes[1].glide.current < t.routes[1].gain);
        ticks(&mut e, 600);
        let t = &e.tracks[1];
        assert_eq!((t.effects[0].dry.current, t.routes[1].glide.current), (dry(6400), t.routes[1].gain));
        // Starting before the clips puts the stored settings back at once; the slot switched on again is emptied first.
        e.seek_tick(100);
        let t = &e.tracks[1];
        assert_eq!((t.slot_mix[0], t.slot_on[1], t.eq.settings(1)[0], t.eq.settings(2)[1], t.sep), (12800, true, 0, 55825, 0));
        assert_eq!((t.effects[0].dry.current, t.routes[1].gain, t.routes[1].glide.current), (0.0, 0.0, 0.0));
        assert_eq!(resets.load(std::sync::atomic::Ordering::Relaxed), 0);
        ticks(&mut e, 100);
        assert_eq!(resets.load(std::sync::atomic::Ordering::Relaxed), 1);
        // Playback starting inside the clips: every level at its target from the first tick.
        e.seek_tick(500);
        ticks(&mut e, 500);
        let t = &e.tracks[1];
        assert_eq!((t.effects[0].dry.current, t.routes[1].glide.current), (dry(6400), t.routes[1].gain));
        assert_eq!(t.gains.map(|r| r.map(|g| g.current)), t.matrix);
    }

    // spec: audio/automation.md "When a clip acts", "Walking the curve"; audio/voice-levels.md "Values from automation"
    #[test]
    fn automation_values_are_one_tick_ahead_and_passed_on_when_changed() {
        use os_model::{AutoMode, AutoPoint, Automation, ChannelKind, Clip, ClipSource, target};
        let mut p = song(vec![long_note(0, 384 * 4, 0)], vec![Clip::new(0, 384 * 4, 0, ClipSource::Pattern(1))]);
        let mut auto_ch = channel();
        auto_ch.index = 1;
        auto_ch.kind = ChannelKind::Automation;
        // A new clip's range, 0..1 (stored pan 0, volume 12800).
        (auto_ch.pan, auto_ch.volume) = (-1.0, 1.0);
        p.channels.push(auto_ch);
        // Channel pitch from the bottom, jumping to the top at beat 1 of a clip at tick 384, staying there for half a
        // beat, then falling to 0.75 at beat 2.
        let jump = AutoPoint { mode: AutoMode::Hold, ..AutoPoint::new(1.0, 1.0) };
        let points = vec![AutoPoint::new(0.0, 0.0), jump, AutoPoint::new(1.5, 1.0), AutoPoint::new(2.0, 0.75)];
        let a = Automation { channel: 1, targets: vec![target::channel(0, target::CHANNEL_PITCH)], points, header: vec![0; 21], tail: vec![], lfo: Default::default() };
        p.automations.push(a);
        p.arrangements[0].clips.push(Clip::new(384, 768, 1, ClipSource::Channel(1)));
        let mut lib = SampleLibrary::new(None);
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Song);
        let knob_after = |e: &mut Engine, tick: u64| {
            while e.tick <= tick {
                e.next_tick();
            }
            e.channels[0].knob
        };
        assert_eq!(knob_after(&mut e, 383), 0.0, "before the clip: the channel's own pitch");
        // Round(9600 v) - 4800 cents: the whole ±48 semitones, whatever the channel's range.
        assert_eq!(knob_after(&mut e, 384), -4800.0);
        // The jump at beat 1 (tick 480) is handed on one tick early.
        assert_eq!(knob_after(&mut e, 478), -4800.0);
        assert_eq!(knob_after(&mut e, 479), 4800.0);
        // A value is passed on only when it changes: something else moving the knob is left alone meanwhile.
        e.channels[0].knob = 0.0;
        assert_eq!(knob_after(&mut e, 481), 0.0);
        // The last point's level, 0.75 = +24 semitones, is applied on the clip's last tick and kept after it.
        assert_eq!(knob_after(&mut e, 384 + 767), 2400.0);
        assert_eq!(knob_after(&mut e, 384 * 3 + 10), 2400.0);
        // A seek puts the channel back to its own settings and hands the value on again.
        e.seek_tick(500);
        assert_eq!(e.channels[0].knob, 0.0);
        assert_eq!(knob_after(&mut e, 500), 4800.0);
        // Channel volume set in place on playback's first tick: no glide.
        p.automations[0].targets = vec![target::channel(0, target::CHANNEL_VOLUME)];
        p.automations[0].points = vec![AutoPoint::new(0.0, 0.5), AutoPoint::new(2.0, 0.5)];
        p.arrangements[0].clips[1] = Clip::new(0, 768, 1, ClipSource::Channel(1));
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Song);
        e.next_tick();
        assert!(e.channels[0].ramp.is_none());
        assert_eq!(e.channels[0].vol_v, 0.5);
    }

    /// A hosted instrument's output follows automated pan without a step at each tick.
    // spec: audio/automation.md "When a clip acts"
    #[test]
    fn automated_levels_reach_a_hosted_instrument_without_steps() {
        use os_model::{AutoPoint, Automation, ChannelKind, Clip, ClipSource, target};
        let mut p = song(vec![long_note(0, 384 * 4, 0)], vec![Clip::new(0, 384 * 4, 0, ClipSource::Pattern(1))]);
        p.channels[0].kind = ChannelKind::Plugin;
        let run = |p: &Project| {
            let mut lib = SampleLibrary::default();
            let mut e = Engine::new(p, &mut lib, 48000, PlayMode::Song);
            e.set_instrument(0, Arc::new(Mutex::new(LevelProbe(vec![]))));
            (0..200).map(|_| e.next_tick().unwrap().0.to_vec()).collect::<Vec<_>>()
        };
        // Nothing automated: one gain throughout.
        let still = run(&p).concat();
        assert!(still.chunks_exact(2).all(|f| f == &still[..2]));
        let mut auto_ch = channel();
        auto_ch.index = 1;
        auto_ch.kind = ChannelKind::Automation;
        (auto_ch.pan, auto_ch.volume) = (-1.0, 1.0);
        p.channels.push(auto_ch);
        let points = vec![AutoPoint::new(0.0, 0.0), AutoPoint::new(4.0, 1.0)];
        p.automations.push(Automation { channel: 1, targets: vec![target::channel(0, target::CHANNEL_PAN)], points, header: vec![0; 21], tail: vec![], lfo: Default::default() });
        p.arrangements[0].clips.push(Clip::new(0, 384 * 4, 1, ClipSource::Channel(1)));
        // The pan sweeps: frame to frame the output moves by a small part of what it moves in a tick.
        let ticks = run(&p);
        for side in 0..2 {
            let at = |o: &[f32], f: usize| o[f * 2 + side];
            let ends: Vec<f32> = ticks.iter().map(|t| at(t, t.len() / 2 - 1)).collect();
            let per_tick = ends.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
            let all = ticks.concat();
            let per_frame = (1..all.len() / 2).map(|f| (at(&all, f) - at(&all, f - 1)).abs()).fold(0.0, f32::max);
            assert!(per_tick > 0.0 && per_frame < per_tick / 8.0, "side {side}: {per_frame} a frame, {per_tick} a tick");
        }
    }

    /// A clip's range and LFO, as the values handed on to a mixer volume, tick by tick, against the page's formulas
    /// worked out here: the default LFO (amount 64, multiplying, one cycle a beat, a triangle starting where it passes
    /// 0 going up) on a level of 0.5 swings 0.25..0.75; a range of 0.2..0.8 (stored 2560 / 10240) with an adding LFO
    /// at amount 128 swings 0.2..0.8 around 0.5; a clip started part-way restarts the phase at its own start.
    // spec: audio/automation.md "Range", "What a clip hands on", "The LFO's value"
    #[test]
    fn a_clips_range_and_lfo_shape_the_values_handed_on() {
        use os_model::clip_lfo::ClipLfo;
        use os_model::{AutoPoint, Automation, ChannelKind, Clip, ClipSource, target};
        let tgt = target::mixer(1, target::MIXER_VOLUME);
        let make = |pan: f64, volume: f64, lfo: ClipLfo, clip: Clip| {
            let mut p = song(vec![long_note(0, 384 * 4, 0)], vec![Clip::new(0, 384 * 8, 0, ClipSource::Pattern(1))]);
            let mut auto_ch = channel();
            auto_ch.index = 1;
            auto_ch.kind = ChannelKind::Automation;
            (auto_ch.pan, auto_ch.volume) = (pan, volume);
            p.channels.push(auto_ch);
            let points = vec![AutoPoint::new(0.0, 0.5), AutoPoint::new(16.0, 0.5)];
            p.automations.push(Automation { channel: 1, targets: vec![tgt], points, header: vec![0; 21], tail: vec![], lfo });
            p.arrangements[0].clips.push(clip);
            p
        };
        // What the page says: the step per tick, the triangle, every operation a single.
        let b = f64::from((1.0f64 / 96.0) as f32);
        let q = (45819.0f64 / 65536.0) as f32;
        let e = ((f64::from(q) * 10001f64.ln()).exp() - 1.0) as f32;
        let step = (f64::from(e) * (4294967296.0 / 625.0) * b).min(805306368.0).round_ties_even() as u32;
        let tri = |ph: u32| -> f32 {
            let half = 1u32 << 31;
            if ph < half { 1.0 - ph as f32 * (2.0f64 / f64::from(half)) as f32 } else { (ph - half) as f32 * (2.0f64 / f64::from(half - 1)) as f32 - 1.0 }
        };
        let phase = |k: u64| (3u32 << 30).wrapping_add((k as u32).wrapping_mul(step));
        let r = 1.0f32 / 12800.0;
        let n = |out: f32| ((f64::from(out) * 1073741824.0).round_ties_even() as i64).clamp(0, 1 << 30);
        let run = |p: &Project, from: u64, to: u64| -> Vec<(u64, i64)> {
            let mut lib = SampleLibrary::new(None);
            let mut e = Engine::new(p, &mut lib, 48000, PlayMode::Song);
            let mut v = Vec::new();
            while e.tick <= to {
                e.next_tick();
                if e.tick > from {
                    v.push((e.tick - 1, e.auto_last.iter().find(|l| l.0 == target::decode(tgt)).map_or(-1, |l| l.1)));
                }
            }
            v
        };
        let on = ClipLfo { switch: -1, ..ClipLfo::default() };

        // The default LFO multiplying, from a clip at tick 96: the k-th tick of the clip (from 1) takes phase start + k step.
        let p = make(-1.0, 1.0, on.clone(), Clip::new(96, 384 * 4, 1, ClipSource::Channel(1)));
        let got = run(&p, 96, 96 + 384 * 4 - 1);
        let (mut lo, mut hi) = (1.0f32, 0.0f32);
        for &(tick, v) in &got {
            let k = tick + 1 - 96;
            let lfo = 0.5 * tri(phase(k));
            let out = (0.0 + 0.5 * (r * 12800.0)) * (lfo * (64.0 * (1.0 / 128.0) * (r * 12800.0)) * 2.0 + 1.0);
            assert_eq!(v, n(out), "tick {tick}");
            (lo, hi) = (lo.min(out), hi.max(out));
        }
        assert!((lo - 0.25).abs() < 2e-3 && (hi - 0.75).abs() < 2e-3, "{lo} {hi}");

        // A range of 0.2..0.8 and an LFO adding at amount 128.
        let (pan, vol) = (2560.0 / 6400.0 - 1.0, 10240.0 / 12800.0);
        let add = ClipLfo { mode: 1, amount: 128, ..on.clone() };
        let p = make(pan, vol, add, Clip::new(0, 384 * 4, 1, ClipSource::Channel(1)));
        let (min, range) = (2560.0f32 * r, 10240.0f32 * r - 2560.0f32 * r);
        let (mut lo, mut hi) = (1.0f32, 0.0f32);
        for &(tick, v) in &run(&p, 0, 384 * 4 - 1) {
            let out = min + 0.5 * range;
            let out = out + 0.5 * tri(phase(tick + 1)) * (128.0 * (1.0 / 128.0) * range);
            assert_eq!(v, n(out), "tick {tick}");
            (lo, hi) = (lo.min(out), hi.max(out));
        }
        assert!((lo - 0.2).abs() < 2e-3 && (hi - 0.8).abs() < 2e-3, "{lo} {hi}");
        // Without the LFO the range alone: 0.2 + 0.5 * 0.6.
        let p = make(pan, vol, ClipLfo::default(), Clip::new(0, 384, 1, ClipSource::Channel(1)));
        assert!(run(&p, 0, 300).iter().all(|&(_, v)| v == n(min + 0.5 * range)));

        // A clip whose window starts 2 beats in: the curve starts there, the LFO's phase at the clip's own start.
        let mut c = Clip::new(192, 384, 1, ClipSource::Channel(1));
        c.window = Some((2.0, 6.0));
        let p = make(-1.0, 1.0, on, c);
        for &(tick, v) in &run(&p, 192, 300) {
            let out = 0.5f32 * (0.5 * tri(phase(tick + 1 - 192)) * 0.5 * 2.0 + 1.0);
            assert_eq!(v, n(out), "tick {tick}");
        }
    }

    /// A five-bar song (one clip to tick 1920, a note every beat) with time markers (tick, kind), at 125 BPM and 48 kHz,
    /// where a tick is exactly 240 samples.
    fn marker_song(markers: &[(u32, u8)]) -> Project {
        use os_model::{Clip, ClipSource, TimeMarker};
        let notes = (0..20).map(|i| Note { key: 48 + i as u16, ..long_note(96 * i, 48, 0) }).collect();
        let mut p = song(notes, vec![Clip::new(0, 1920, 0, ClipSource::Pattern(1))]);
        p.tempo = Some(125.0);
        p.arrangements[0].markers = markers.iter().map(|&(t, k)| TimeMarker { kind: k, ..TimeMarker::new(t, "") }).collect();
        p
    }

    /// The ticks live song playback hands out from `from`, `n` of them, with whether each is held.
    fn live_ticks(p: &Project, from: u64, n: usize) -> Vec<(u64, bool)> {
        let mut e = Engine::new(p, &mut SampleLibrary::new(None), 48000, PlayMode::Song);
        e.set_looping(true);
        if from > 0 {
            e.seek_tick(from);
        }
        (0..n).map(|_| e.next_tick().map(|t| (t.1 as u64, t.3)).unwrap()).collect()
    }

    fn ticks_only(v: &[(u64, bool)]) -> Vec<u64> {
        v.iter().map(|t| t.0).collect()
    }

    /// spec: audio/transport.md "Time markers in SONG mode": the song's end goes on at the Loop marker, else the Start
    /// marker, else tick 0; playback started before a Start marker plays through what lies before it.
    #[test]
    fn the_song_end_goes_on_at_the_loop_or_start_marker() {
        use os_model::TimeMarker as M;
        let at_end = |markers: &[(u32, u8)]| ticks_only(&live_ticks(&marker_song(markers), 0, 1922))[1918..].to_vec();
        assert_eq!(at_end(&[]), [1918, 1919, 0, 1]);
        assert_eq!(at_end(&[(768, M::START)]), [1918, 1919, 768, 769]);
        assert_eq!(at_end(&[(384, M::START), (1152, M::LOOP)]), [1918, 1919, 1152, 1153]);
        assert_eq!(at_end(&[(384, M::LOOP), (1152, M::LOOP)]), [1918, 1919, 1152, 1153]);
        assert_eq!(ticks_only(&live_ticks(&marker_song(&[(768, M::START)]), 0, 3)), [0, 1, 2], "the pre-roll plays");
    }

    /// A skip jumps to the next marker when it is reached, not when playback starts on it; a marker loop goes back at
    /// its section's end, also when playback started inside it.
    #[test]
    fn skips_and_marker_loops_jump_when_reached() {
        use os_model::TimeMarker as M;
        let skip = marker_song(&[(384, M::SKIP), (1152, M::PLAIN)]);
        let t = ticks_only(&live_ticks(&skip, 0, 400));
        assert_eq!(&t[382..386], &[382, 383, 1152, 1153]);
        assert_eq!(&ticks_only(&live_ticks(&skip, 384, 2)), &[384, 385], "started on the skip");
        // After the song's end, it acts again.
        let round = ticks_only(&live_ticks(&skip, 1900, 420));
        assert_eq!(&round[18..22], &[1918, 1919, 0, 1]);
        assert_eq!(&round[402..406], &[382, 383, 1152, 1153]);

        let mloop = marker_song(&[(768, M::MARKER_LOOP), (1152, M::PLAIN)]);
        let t = ticks_only(&live_ticks(&mloop, 0, 1152 + 384 + 2));
        assert_eq!(&t[1150..1154], &[1150, 1151, 768, 769]);
        assert_eq!(&t[1150 + 384..1154 + 384], &[1150, 1151, 768, 769], "over and over");
        let inside = ticks_only(&live_ticks(&mloop, 900, 255));
        assert_eq!(&inside[250..254], &[1150, 1151, 768, 769]);
        // The time selection loops instead: no marker acts.
        let mut e = Engine::new(&skip, &mut SampleLibrary::new(None), 48000, PlayMode::Song);
        e.set_looping(true);
        e.set_loop_range(Some((0, 960)));
        let t: Vec<u64> = (0..400).map(|_| e.next_tick().unwrap().1 as u64).collect();
        assert_eq!(&t[382..386], &[382, 383, 384, 385]);
    }

    /// A quantized song jump waits for the next multiple of its unit, then lands on its target; markers there do not
    /// act. A tick on the unit jumps at once.
    #[test]
    fn a_quantized_jump_waits_for_its_unit() {
        use os_model::TimeMarker as M;
        let p = marker_song(&[(1152, M::SKIP), (1536, M::PLAIN)]);
        let mut e = Engine::new(&p, &mut SampleLibrary::new(None), 48000, PlayMode::Song);
        e.set_looping(true);
        let mut tick = || e.next_tick().unwrap().1 as u64;
        let t: Vec<u64> = (0..100).map(|_| tick()).collect();
        assert_eq!(t[99], 99);
        e.jump_on(384, 1152);
        let t: Vec<u64> = (0..300).map(|_| e.next_tick().unwrap().1 as u64).collect();
        assert_eq!(&t[282..288], &[382, 383, 1152, 1153, 1154, 1155], "on to 384, then the target; its skip did not act");
        let mut e = Engine::new(&p, &mut SampleLibrary::new(None), 48000, PlayMode::Song);
        e.set_looping(true);
        for _ in 0..96 {
            e.next_tick();
        }
        e.jump_on(96, 0);
        assert_eq!(e.next_tick().unwrap().1 as u64, 0, "on the unit: at once");
    }

    /// A Marker pause holds playback on the marker (the fade ends there), once; Resume plays on from it. A Start
    /// recording marker makes the song run on past its end.
    #[test]
    fn a_pause_marker_holds_and_a_punch_in_runs_on() {
        use os_model::TimeMarker as M;
        let p = marker_song(&[(768, M::PAUSE)]);
        let mut e = Engine::new(&p, &mut SampleLibrary::new(None), 48000, PlayMode::Song);
        e.set_looping(true);
        let mut last = Vec::new();
        let mut held_at = None;
        for _ in 0..800 {
            let (s, tick, _, held, _) = e.next_tick().unwrap();
            let tick = tick as u64;
            if held {
                held_at = Some(tick);
                break;
            }
            assert!(tick < 768);
            last = s.to_vec();
        }
        assert_eq!(held_at, Some(768));
        assert_eq!(e.take_marker_pause(), Some(768));
        assert_eq!(e.take_marker_pause(), None);
        assert!(e.paused());
        // The tick before the marker fades to silence at its end.
        assert!(last[last.len() - 2..].iter().all(|v| v.abs() < 1e-3), "{:?}", &last[last.len() - 2..]);
        e.resume();
        let after: Vec<(u64, bool)> = (0..3).map(|_| e.next_tick().map(|t| (t.1 as u64, t.3)).unwrap()).collect();
        assert_eq!(after, [(768, false), (769, false), (770, false)]);
        assert_eq!(e.take_marker_pause(), None);

        let punch = marker_song(&[(384, M::PUNCH_IN), (1152, M::PUNCH_OUT)]);
        assert_eq!(&ticks_only(&live_ticks(&punch, 0, 1922))[1918..], &[1918, 1919, 1920, 1921]);
        let out_only = marker_song(&[(1152, M::PUNCH_OUT)]);
        assert_eq!(&ticks_only(&live_ticks(&out_only, 0, 1922))[1918..], &[1918, 1919, 0, 1]);
    }

    /// spec: audio/song-structure.md "Time markers": a render starts at the Start marker with its clock at 0 there,
    /// and a skip goes on at the next marker with the clock running on; both files are the distance shorter, and
    /// after the jump they are the plain render's samples.
    #[test]
    fn a_render_starts_at_the_start_marker_and_jumps_at_skips() {
        use os_model::TimeMarker as M;
        const T: u64 = 240;
        // Renders on an instrument that logs its notes: (sample, key) of every note start, the file's length in
        // frames, and the frames the render said were left before it began.
        let render = |markers: &[(u32, u8)]| {
            let mut p = marker_song(markers);
            p.channels[0].kind = ChannelKind::Plugin;
            let mut e = Engine::new(&p, &mut SampleLibrary::new(None), 48000, PlayMode::Song);
            let probe = Arc::new(std::sync::Mutex::new(Probe { log: vec![], held: vec![], at: 0 }));
            e.set_instrument(0, probe.clone());
            e.follow_markers();
            let left = e.frames_left();
            let len = e.render_all(512).len() as u64 / 2;
            let ons: Vec<(u64, u8)> = probe.lock().unwrap().log.iter().filter(|l| l.1).map(|l| (l.0, l.2)).collect();
            (ons, len, left)
        };
        let note = |i: u64, shift: u64| ((96 * i - shift) * T, 48 + i as u8);
        let (plain, plain_len, plain_left) = render(&[]);
        assert_eq!(plain, (0..20).map(|i| note(i, 0)).collect::<Vec<_>>());
        assert_eq!(plain_left, 1920 * T);

        let (start, start_len, start_left) = render(&[(768, M::START)]);
        assert_eq!(start, (8..20).map(|i| note(i, 768)).collect::<Vec<_>>(), "from the marker, its clock at 0 there");
        assert_eq!((start_left, start_len), ((1920 - 768) * T, plain_len - 768 * T));

        let (skip, skip_len, skip_left) = render(&[(384, M::SKIP), (1152, M::PLAIN)]);
        let want: Vec<_> = (0..4).map(|i| note(i, 0)).chain((12..20).map(|i| note(i, 768))).collect();
        assert_eq!(skip, want, "the goal's note starts on the jump's sample");
        assert_eq!((skip_left, skip_len), ((1920 - 768) * T, plain_len - 768 * T));

        // A goal at or past the song's end ends the song at the skip; a skip back is passed over.
        let (ends, _, ends_left) = render(&[(384, M::SKIP), (2304, M::PLAIN)]);
        assert_eq!(ends, (0..4).map(|i| note(i, 0)).collect::<Vec<_>>());
        assert_eq!(ends_left, 384 * T);
        let (back, back_len, _) = render(&[(384, M::PLAIN), (1152, M::SKIP)]);
        assert_eq!((back, back_len), (plain, plain_len));
    }

    /// A looping song with loop points plays them over and over: from before the range it runs on to the range's end,
    /// from past it to the song's end, then from the range's start.
    #[test]
    fn a_song_loops_over_its_loop_points() {
        use os_model::{Clip, ClipSource};
        let p = song(vec![long_note(0, 96, 0)], vec![Clip::new(0, 384, 0, ClipSource::Pattern(1))]);
        let mut lib = SampleLibrary::new(None);
        let ticks = |from: u64, n: usize| {
            let mut e = Engine::new(&p, &mut SampleLibrary::new(None), 48000, PlayMode::Song);
            e.set_looping(true);
            e.set_loop_range(Some((96, 192)));
            e.seek_tick(from);
            (0..n).map(|_| e.next_tick().unwrap().1 as u64).collect::<Vec<_>>()
        };
        let from_start = ticks(0, 400);
        assert_eq!(&from_start[190..194], &[190, 191, 96, 97], "from before the range: back at its end");
        assert_eq!(from_start[192 + 96], 96, "and again");
        let past = ticks(300, 90);
        assert_eq!(&past[82..86], &[382, 383, 96, 97], "from past the range: back at the song's end");
        // No range (or an empty one): the whole song.
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Song);
        e.set_looping(true);
        e.set_loop_range(Some((96, 96)));
        let whole: Vec<u64> = (0..386).map(|_| e.next_tick().unwrap().1 as u64).collect();
        assert_eq!(&whole[383..], &[383, 0, 1]);
    }

    /// An audio clip running on past the loop's end ends there: the replay starts it again, once (it used to sound
    /// twice, then three times, louder every loop).
    #[test]
    fn audio_clips_end_at_the_loop_point() {
        use os_model::{Clip, ClipSource};
        let wav = std::env::temp_dir().join(format!("os-engine-loop-clip-{}.wav", std::process::id()));
        crate::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &vec![0.25; 2 * 48000 * 4]).unwrap();
        for range in [(0, 192), (96, 192)] {
            let mut p = song(vec![], vec![Clip::new(0, 384, 0, ClipSource::Channel(0))]);
            p.channels[0].kind = ChannelKind::AudioClip;
            p.channels[0].sample_path = Some(wav.to_string_lossy().into_owned());
            let mut e = Engine::new(&p, &mut SampleLibrary::new(None), 48000, PlayMode::Song);
            e.set_looping(true);
            e.set_loop_range(Some(range));
            let mut peak_first = 0.0f32;
            let mut peak_later = 0.0f32;
            for i in 0..1000 {
                let (s, tick, ..) = e.next_tick().unwrap();
                let p = s.iter().fold(0.0f32, |m, x| m.max(x.abs()));
                if i < 150 && tick > (range.0 + 5) as f64 {
                    peak_first = peak_first.max(p);
                } else if i > 600 {
                    peak_later = peak_later.max(p);
                }
                assert!(e.voices.iter().filter(|v| v.cut.is_some()).count() <= 1, "{range:?}: one clip voice at tick {tick}");
            }
            assert!(peak_first > 0.1 && peak_later < peak_first * 1.1, "{range:?}: {peak_first} then {peak_later}");
        }
    }

    /// A song that loops sets the project's initial values again at its start, before its automation clips act; a
    /// pattern doesn't (spec: ui/settings-choices.md "Advanced: Initialize controls automatically").
    #[test]
    fn initial_values_are_set_where_the_song_loops() {
        use os_model::{AutoPoint, Automation, ChannelKind, Clip, ClipSource, InitControl, target};
        let mut p = song(vec![long_note(0, 96, 0)], vec![Clip::new(0, 384, 0, ClipSource::Pattern(1))]);
        p.channels[0].volume = 0.25;
        let init = |t: u32, value: i32| InitControl { head: 0, target: t, value };
        p.init_controls = vec![
            init(target::channel(0, target::CHANNEL_VOLUME), 6400),
            init(target::channel(0, target::CHANNEL_PITCH), 1200),
            init(target::channel_param(0, 3), 99),
        ];
        // A clip on the pitch from tick 0: it acts after the initial value.
        let mut auto_ch = channel();
        auto_ch.index = 1;
        auto_ch.kind = ChannelKind::Automation;
        // A new clip's range, 0..1 (stored pan 0, volume 12800).
        (auto_ch.pan, auto_ch.volume) = (-1.0, 1.0);
        p.channels.push(auto_ch);
        let points = vec![AutoPoint::new(0.0, 0.75), AutoPoint::new(4.0, 0.75)];
        p.automations.push(Automation { channel: 1, targets: vec![target::channel(0, target::CHANNEL_PITCH)], points, header: vec![0; 21], tail: vec![], lfo: Default::default() });
        p.arrangements[0].clips.push(Clip::new(0, 384, 1, ClipSource::Channel(1)));
        let mut lib = SampleLibrary::new(None);
        let to_loop = |e: &mut Engine| {
            let mut last = 0;
            while e.tick >= last && e.tick < 10_000 {
                last = e.tick;
                e.next_tick();
            }
        };
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Song);
        e.set_looping(true);
        e.next_tick();
        assert_eq!((e.channels[0].vol_v, e.channels[0].knob), (0.25, 2400.0), "the start is the app's: the channel's own volume");
        e.channels[0].knob = 0.0;
        to_loop(&mut e);
        assert_eq!(e.init.len(), 2, "a plugin parameter's record is left out");
        assert_eq!(e.channels[0].vol_v, 0.5, "the initial volume where the song starts again");
        assert_eq!(e.channels[0].knob, 2400.0, "then the clip");
        // Pattern mode: no initial values.
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Pattern(1));
        e.set_looping(true);
        to_loop(&mut e);
        assert!(e.init.is_empty());
        assert_eq!(e.channels[0].vol_v, 0.25);
    }

    /// A jump past a clip that is over finds its target at the clip's last value, as in the reference and as playing up
    /// to there would, and a later clip on the same target decides. Before the first clip it finds the channel's own (a
    /// Lemon Studio difference: the reference leaves the target as it was).
    #[test]
    fn seeking_past_a_finished_clip_keeps_its_last_value() {
        use os_model::{target, AutoPoint, Automation, Clip, ClipSource};
        let mut p = song(vec![long_note(0, 960, 0)], vec![Clip::new(0, 960, 0, ClipSource::Pattern(1))]);
        for (index, level) in [(1, 0.75), (2, 0.25)] {
            let mut auto_ch = channel();
            auto_ch.index = index;
            auto_ch.kind = ChannelKind::Automation;
            (auto_ch.pan, auto_ch.volume) = (-1.0, 1.0);
            p.channels.push(auto_ch);
            let points = vec![AutoPoint::new(0.0, level), AutoPoint::new(4.0, level)];
            p.automations.push(Automation { channel: index, targets: vec![target::channel(0, target::CHANNEL_PITCH)], points, header: vec![0; 21], tail: vec![], lfo: Default::default() });
        }
        p.arrangements[0].clips.push(Clip::new(96, 96, 1, ClipSource::Channel(1)));
        p.arrangements[0].clips.push(Clip::new(480, 96, 1, ClipSource::Channel(2)));
        let mut lib = SampleLibrary::new(None);
        let played = |until: u64, lib: &mut SampleLibrary| {
            let mut e = Engine::new(&p, lib, 48000, PlayMode::Song);
            while e.tick < until {
                e.next_tick();
            }
            e.channels[0].knob
        };
        assert_eq!((played(288, &mut lib), played(768, &mut lib)), (2400.0, -2400.0), "playing there");
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Song);
        e.seek_tick(48);
        assert_eq!(e.channels[0].knob, 0.0, "before the clips");
        e.seek_tick(288);
        assert_eq!(e.channels[0].knob, 2400.0, "past the first");
        e.seek_tick(768);
        assert_eq!(e.channels[0].knob, -2400.0, "past the second");
        e.seek_tick(48);
        assert_eq!(e.channels[0].knob, 0.0, "back before them");
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
        // The track's "effect slots enabled" switch off: no slot runs (spec: audio/mixer.md).
        let mut disabled = p.clone();
        disabled.mixer[1].slots_enabled = false;
        assert!((peak(&render_with(&disabled, true), 0) - dry).abs() < 1e-6, "slots disabled = bypass");
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

    /// spec: audio/sidechain.md "Extra output", plugins/fruity-stereo-shaper.md "Difference output": a Stereo Shaper's
    /// difference goes to the n-th track its track is routed to (in track order, a number past the end meaning the
    /// last); on the master, which is routed nowhere, it joins the master's own audio, or the dry part below 100 % mix.
    #[test]
    fn a_stereo_shapers_difference_output_reaches_its_track() {
        use crate::effects::stereo_shaper::Settings;
        let plain = render_with(&insert_project(), false);
        assert!(peak(&plain, 0) > 0.01);
        let same = |a: &[f32], b: &[f32], k: f32| a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - k * y).abs() < 1e-6);
        // The matrix at zero: what is heard is the difference (the whole input), wherever it goes.
        let shaper = |difference: i32| os_model::SlotPlugin {
            id: os_model::Id::NONE,
            name: "Fruity Stereo Shaper".into(),
            state: Settings { v: [0, 0, 0, 0, 0, 0, difference, 1] }.to_state(),
        };
        let mut p = insert_project();
        p.mixer[1].routes = vec![2, 0];
        p.mixer[1].send_levels = vec![(2, 0.0)];
        p.mixer[1].slots[0] = Some(shaper(0));
        assert_eq!(peak(&render_with(&p, false), 0), 0.0, "no difference output");
        // Output 1 is the master, 2 is track 2 (at level 0, so only the difference reaches it), 3 and on also track 2.
        for k in [1, 2, 3, 50] {
            p.mixer[1].slots[0] = Some(shaper(k));
            assert!(same(&render_with(&p, false), &plain, 1.0), "difference output {k}");
        }
        p.mixer[1].slot_on[0] = false;
        assert!(same(&render_with(&p, false), &plain, 1.0), "a slot that is off passes its input and nothing else");
        // On the master: behind the effect, or in the dry part at mix 0 (the input twice).
        let mut p = insert_project();
        p.mixer[0].slots[0] = Some(shaper(1));
        assert!(same(&render_with(&p, false), &plain, 1.0));
        p.mixer[0].slot_mix[0] = 0.0;
        assert!(same(&render_with(&p, false), &plain, 2.0));
    }

    /// Plays 1.0 on both sides while a key is held, and counts its process calls.
    #[derive(Default)]
    struct GateProbe {
        held: usize,
        calls: usize,
    }

    impl Instrument for GateProbe {
        fn note_on(&mut self, _: u32, _: u8, _: f32) {
            self.held += 1;
        }
        fn note_off(&mut self, _: u32, _: u8) {
            self.held = self.held.saturating_sub(1);
        }
        fn process(&mut self, _: u32, left: &mut [f32], right: &mut [f32]) {
            self.calls += 1;
            let v = if self.held > 0 { 1.0 } else { 0.0 };
            left.fill(v);
            right.fill(v);
        }
    }

    /// spec: audio/auto-bypass.md "Instruments": a plugin channel with Auto-bypass when silent on stops running its
    /// instrument once it has had nothing to play and been silent for the hosted hold, and runs it again with the next
    /// note; what is heard does not change. With the switch off it always runs.
    #[test]
    fn a_silent_instrument_is_bypassed_until_its_next_note() {
        let run = |smart: bool| -> (Vec<f32>, usize) {
            // A short note, about six seconds of silence, another note.
            let mut p = insert_project();
            p.patterns[0].notes = vec![long_note(0, 48, 0), long_note(32 * 96, 48, 0)];
            p.channels[0].kind = ChannelKind::Plugin;
            p.channels[0].misc.cpu.smart_disable = smart;
            let mut lib = SampleLibrary::new(None);
            let mut e = Engine::new(&p, &mut lib, 44100, PlayMode::Pattern(1));
            let probe = Arc::new(Mutex::new(GateProbe::default()));
            e.set_instrument(0, probe.clone());
            let (mut out, mut block) = (Vec::new(), vec![0.0f32; 1024]);
            for _ in 0..44100 * 16 / 512 {
                e.process(&mut block);
                out.extend_from_slice(&block);
            }
            let calls = probe.lock().unwrap().calls;
            (out, calls)
        };
        let (plain, all) = run(false);
        let (out, fewer) = run(true);
        assert!(peak(&plain, 0) > 0.1, "the notes sound");
        assert!(out.iter().zip(&plain).all(|(a, b)| a == b), "what is heard does not change");
        // The pattern's two notes are 32 beats apart; the hold is four seconds of them.
        assert!(fewer < all * 3 / 4, "bypassed while silent: {fewer} of {all} calls");
        assert!(fewer > all / 4, "run again for the second note: {fewer} of {all}");
    }

    /// spec: audio/auto-bypass.md: with Auto-bypass when silent on, a slot stops running its effect once its track has
    /// been silent for the hold (a second for Lemon Studio's own effects) and runs it again with the next note; what is
    /// heard does not change. With the slot's switch off, or the program's setting off, the effect always runs.
    #[test]
    fn a_silent_slot_is_bypassed_until_sound_returns() {
        let balance = os_model::SlotPlugin {
            id: os_model::Id::NONE,
            name: "Fruity Balance".into(),
            state: crate::effects::balance::Settings { v: [0, 128] }.to_state(),
        };
        let run = |smart: bool| -> (Vec<f32>, Vec<bool>) {
            // A note, about three seconds of silence after its release, and another note.
            let mut p = insert_project();
            p.patterns[0].notes.push(long_note(16 * 96, 96, 0));
            p.mixer[1].slots[0] = Some(balance.clone());
            p.mixer[1].slot_cpu[0].smart_disable = smart;
            let mut lib = SampleLibrary::new(None);
            let mut e = Engine::new(&p, &mut lib, 44100, PlayMode::Pattern(1));
            let (mut out, mut skipped, mut block) = (Vec::new(), Vec::new(), vec![0.0f32; 1024]);
            for _ in 0..44100 * 9 / 512 {
                e.process(&mut block);
                out.extend_from_slice(&block);
                skipped.push(e.slot_bypassed(1, 0));
            }
            (out, skipped)
        };
        let (plain, never) = run(false);
        assert!(peak(&plain, 0) > 0.01 && !never.contains(&true), "the switch off: always run");
        let (out, skipped) = run(true);
        let first = skipped.iter().position(|&s| s).expect("bypassed once the track is silent");
        // The first note and its release sound for about three seconds: the hold starts when they end.
        let silent_from = out.chunks(1024).position(|c| auto_bypass::silent(c)).unwrap();
        // A second is 86 blocks; the silence may start inside the block before.
        assert!((silent_from + 84..silent_from + 88).contains(&first), "after the hold: {first} vs {silent_from}");
        assert!(skipped[first..].contains(&false), "the next note runs it again");
        assert!(out.iter().zip(&plain).all(|(a, b)| (a - b).abs() <= auto_bypass::SILENT), "what is heard does not change");

        auto_bypass::set_enabled(false);
        let (_, off) = run(true);
        auto_bypass::set_enabled(true);
        assert!(!off.contains(&true), "the program's setting off: always run");
    }

    /// A hosted effect that puts out its sidechain bus, or with `own` its main input.
    struct KeyOut {
        own: bool,
    }

    impl crate::instrument::HostedEffect for KeyOut {
        fn process_fx(&mut self, _rate: u32, _left: &mut [f32], _right: &mut [f32]) {
            unreachable!("hosted slots always get a sidechain bus");
        }

        fn process_fx_keyed(&mut self, _rate: u32, left: &mut [f32], right: &mut [f32], key: (&[f32], &[f32])) {
            if !self.own {
                left.copy_from_slice(key.0);
                right.copy_from_slice(key.1);
            }
        }
    }

    /// spec: audio/sidechain.md "Hosted plugins' input buses": on insert 1, which plays in both beats, a hosted plugin's
    /// auxiliary port N hears the track's N-th input (insert 2 plays in the first beat, insert 3 in the second, both
    /// routed in at level 0), port 0 or one past the last input the track's own audio; a main port takes that input
    /// instead of the track's own audio (the auxiliary port 0 then hears it too), and past the last input leaves the
    /// track's own.
    #[test]
    fn hosted_ports_pick_the_tracks_inputs() {
        let mut p = insert_project();
        p.mixer.push(os_model::MixerTrack::new(3));
        p.patterns[0].notes = vec![long_note(0, 96, 0), long_note(96, 96, 0), Note { channel: 1, ..long_note(0, 96, 0) }, Note { channel: 2, ..long_note(96, 96, 0) }];
        p.channels.push(Channel { index: 1, mixer_track: 2, ..channel() });
        p.channels.push(Channel { index: 2, mixer_track: 3, ..channel() });
        for t in [2, 3] {
            p.mixer[t].routes = vec![1];
            p.mixer[t].send_levels = vec![(1, 0.0)];
        }
        let render = |own: bool, main: usize, aux: usize| {
            let mut lib = SampleLibrary::new(None);
            let mut e = Engine::new(&p, &mut lib, 44100, PlayMode::Pattern(1));
            e.set_effect(1, 0, std::sync::Arc::new(std::sync::Mutex::new(KeyOut { own })));
            e.set_effect_ports(1, 0, main, aux);
            e.render_all(512)
        };
        // Each beat's sound, away from the notes' ends.
        let beat = (44100.0 * 60.0 / 130.0) as usize;
        let halves = |x: &[f32]| (peak(&x[2000..12000], 0), peak(&x[2 * beat + 2000..2 * beat + 12000], 0));
        // The first beat's release still sounds early in the second, below a tenth of the notes.
        let heard = |(a, b): (f32, f32)| (a > 0.1, b > 0.1);
        assert_eq!(heard(halves(&render(false, 0, 0))), (true, true), "no port: the track's own audio");
        assert_eq!(heard(halves(&render(false, 0, 1))), (true, false), "input 1");
        assert_eq!(heard(halves(&render(false, 0, 2))), (false, true), "input 2");
        assert_eq!(heard(halves(&render(false, 0, 3))), (true, true), "past the last input");
        assert_eq!(heard(halves(&render(false, 1, 0))), (true, false), "the main bus's input");
        assert_eq!(heard(halves(&render(true, 0, 0))), (true, true), "the track's own audio");
        assert_eq!(heard(halves(&render(true, 1, 0))), (true, false), "main port 1");
        assert_eq!(heard(halves(&render(true, 2, 0))), (false, true), "main port 2");
        assert_eq!(heard(halves(&render(true, 3, 0))), (true, true), "main port past the last input");
    }

    /// A version 7 Fruity Limiter state: `p`, the spare bank, editor bytes, sidechain input `key`, no flags.
    fn limiter_slot(p: [i32; 18], key: i32) -> os_model::SlotPlugin {
        let mut v = vec![7];
        v.extend_from_slice(&p);
        v.extend_from_slice(&p);
        v.extend_from_slice(&[0, 0, 0, 0, key]);
        let mut state = crate::effects::state_of(&v);
        state.push(0);
        os_model::SlotPlugin { id: os_model::Id::NONE, name: "Fruity Limiter".into(), state }
    }

    /// spec: audio/sidechain.md "Settings", "Sidechain input"; plugins/fruity-limiter-dsp.md: a Fruity Limiter on
    /// insert 1 keyed from insert 2, which is routed into it at level 0 only (FL's "sidechain to this track"): its
    /// compressor ducks insert 1 while insert 2 sounds and leaves it alone while insert 2 is silent.
    #[test]
    fn a_limiter_is_keyed_from_its_tracks_sidechain_input() {
        use crate::effects::dynamics::{COMP_RATIO, COMP_THRES, DEFAULTS, LIM_ATT};
        // Insert 1 plays quietly in both halves; insert 2 loud in the second.
        let mut p = insert_project();
        p.patterns[0].notes = vec![long_note(0, 96, 0), long_note(96, 96, 0)];
        p.channels[0].volume = 0.05;
        p.channels.push(Channel { index: 1, mixer_track: 2, ..channel() });
        p.mixer[2].routes = vec![1];
        p.mixer[2].send_levels = vec![(1, 0.0)];
        let mut lim = DEFAULTS;
        (lim[COMP_THRES], lim[COMP_RATIO], lim[LIM_ATT]) = (200, 1000, 0);
        let render = |key: i32, kick: bool| {
            let mut q = p.clone();
            q.mixer[1].slots[0] = Some(limiter_slot(lim, key));
            if kick {
                q.patterns[0].notes.push(Note { channel: 1, ..long_note(96, 96, 0) });
            }
            render_with(&q, false)
        };
        // The two beats (96 ticks at 130 BPM each), each note's first 4000 frames.
        let beat = (44100.0 * 60.0 / 130.0) as usize;
        let half = |x: &[f32]| (peak(&x[..8000], 0), peak(&x[2 * beat..2 * beat + 8000], 0));
        let own = render(0, true);
        let (a, b) = half(&own);
        assert!(a > 0.005 && (b - a).abs() < 0.001, "insert 1 alone is heard: {a} {b}");
        // Keyed from a silent track: the compressor hears nothing, as the quiet own input it hears without a key.
        let silent = render(1, false);
        assert!(own.iter().zip(&silent).all(|(a, b)| (a - b).abs() < 1e-4), "a silent key");
        // Keyed from the loud track: insert 1 drops while it sounds; numbers past the last input mean the last.
        for key in [1, 2, 7] {
            let keyed = half(&render(key, true));
            assert!((keyed.0 - half(&own).0).abs() < 1e-4, "{key}: first half untouched: {keyed:?}");
            assert!(keyed.1 < half(&own).1 * 0.75, "{key}: ducked: {keyed:?} {:?}", half(&own));
        }
    }

    /// spec: audio/sidechain.md "Sidechain input": the input waits for the slots above the keyed one (a look-ahead),
    /// minus how late its source already is; the inputs are counted in track order.
    #[test]
    fn a_sidechain_input_is_delayed_to_the_slot() {
        use crate::effects::dynamics::{DEFAULTS, LIM_ATT};
        let look = |p: &[i32; 18]| crate::effects::dynamics::Dynamics::new(44100.0, *p, 0).latency();
        let mut p = insert_project();
        p.mixer.push(os_model::MixerTrack::new(3));
        p.channels.push(Channel { index: 1, mixer_track: 2, ..channel() });
        p.mixer[2].routes = vec![1];
        p.mixer[3].routes = vec![1];
        let mut short = DEFAULTS;
        short[LIM_ATT] = 500;
        // Insert 1: an unkeyed limiter, then one keyed from input 2 (insert 3); insert 3 has its own short look-ahead.
        p.mixer[1].slots[0] = Some(limiter_slot(DEFAULTS, 0));
        p.mixer[1].slots[1] = Some(limiter_slot(DEFAULTS, 2));
        p.mixer[3].slots[0] = Some(limiter_slot(short, 0));
        let mut lib = SampleLibrary::new(None);
        let mut e = Engine::new(&p, &mut lib, 44100, PlayMode::Pattern(1));
        e.render_all(512);
        let keyed = &e.tracks[1].effects[1];
        assert_eq!(keyed.key, Some(3));
        // Insert 1 arrives as late as insert 3 leaves; the slot above adds its look-ahead.
        assert_eq!(keyed.key_pad.len(), look(&DEFAULTS));
        assert!(look(&short) > 0 && look(&short) < look(&DEFAULTS));
        assert_eq!(e.tracks[1].effects[0].key, None);
        assert!(e.key_out[3].is_some() && e.key_out[2].is_none());
    }

    /// spec: audio/sidechain.md "Settings": a track's inputs are the tracks routed into it, in track order.
    #[test]
    fn a_tracks_inputs_count_in_track_order() {
        let mut tracks: Vec<MixTrack> = (0..5).map(|_| MixTrack::unity()).collect();
        let route = |dest| Route { dest, gain: 0.0, glide: Glide::new(0.0), pad: pdc::Pad::default() };
        tracks[4].routes = vec![route(1)];
        tracks[2].routes = vec![route(0), route(1)];
        assert_eq!([1, 2, 3].map(|k| input(&tracks, 1, k)), [Some(2), Some(4), Some(4)]);
        assert_eq!(input(&tracks, 3, 1), None);
    }

    /// spec: audio/gain-laws.md "Panning law: Triangular": event 23 = 2 changes the channel pan and the mixer track
    /// pan (the louder side stays at the centre level, the quieter one falls in a straight line); 0 or no event keeps
    /// the circular laws. Heard end to end: a channel panned right 0.5 into an insert panned left 0.5.
    #[test]
    fn panning_law_triangular_moves_channel_and_mixer_pans() {
        let mut p = insert_project();
        p.channels[0].pan = 0.5;
        p.mixer[1].pan = Some(-0.5);
        let engine = |law: Option<u8>| {
            let mut q = p.clone();
            q.panning_law = law;
            Engine::new(&q, &mut SampleLibrary::new(None), 44100, PlayMode::Pattern(1))
        };
        let near = |a: f32, b: f64, tol: f64| (f64::from(a) - b).abs() < tol;
        // The channel: 0.7071 × (0.5, 1) of its volume under Triangular, cos / sin under Circular (also for 0).
        for (law, want) in [(Some(2), laws::channel_pan_law(0.5, laws::PanLaw::Triangular)), (None, laws::channel_pan(0.5)), (Some(0), laws::channel_pan(0.5))] {
            let e = engine(law);
            let ch = &e.channels[0];
            let (l, r) = ch.gain(1.0, 0.0);
            assert!(near(l, ch.volume * want.0, 1e-6) && near(r, ch.volume * want.1, 1e-6), "{law:?}: {l} {r}");
        }
        let c = laws::channel_pan(0.0).0;
        let (l, r) = laws::channel_pan_law(0.5, laws::PanLaw::Triangular);
        assert!((l / c - 0.5).abs() < 1e-12 && (r / c - 1.0).abs() < 1e-12);
        // The insert: [[L←L, L←R], [R←L, R←R]] = [[0.75, 0.25], [0, 0.5]] under Triangular, the measured circular
        // values otherwise.
        let m = engine(Some(2)).tracks[1].matrix;
        assert!(near(m[0][0], 0.75, 2e-4) && near(m[0][1], 0.25, 2e-4) && near(m[1][0], 0.0, 2e-4) && near(m[1][1], 0.5, 2e-4), "{m:?}");
        let m = engine(None).tracks[1].matrix;
        assert!(near(m[0][0], 1.0068, 1e-4) && near(m[0][1], 0.2997, 1e-3) && near(m[1][1], 0.5412, 1e-4), "{m:?}");
        // End to end: left over right at the loudest frame is (0.75 × 0.5 + 0.25 × 1) / (0.5 × 1) = 1.25 under
        // Triangular; circular gives (1.0068 × 0.4142 + 0.2997) / 0.5412.
        let ratio = |law: Option<u8>| {
            let mut q = p.clone();
            q.panning_law = law;
            let out = render_with(&q, false);
            let i = (0..out.len() / 2).max_by(|&a, &b| out[2 * a + 1].abs().total_cmp(&out[2 * b + 1].abs())).unwrap();
            f64::from(out[2 * i]) / f64::from(out[2 * i + 1])
        };
        assert!((ratio(Some(2)) - 1.25).abs() < 1e-3, "{}", ratio(Some(2)));
        let (cl, cr) = laws::channel_pan(0.5);
        let m = engine(None).tracks[1].matrix;
        let circular = (f64::from(m[0][0]) * cl + f64::from(m[0][1]) * cr) / (f64::from(m[1][1]) * cr);
        assert!((ratio(None) - circular).abs() < 1e-3, "{} {circular}", ratio(None));
    }

    /// spec: audio/mixer.md "Fader matrix": polarity flips the sign, swap exchanges the outputs, separation merges.
    #[test]
    fn polarity_swap_and_separation() {
        let p = insert_project();
        let base = render_with(&p, false);
        let mut q = p.clone();
        q.mixer[1].polarity = true;
        let flipped = render_with(&q, false);
        assert!(base.iter().zip(&flipped).all(|(a, b)| (a + b).abs() < 1e-6), "polarity");
        // Swap exchanges the outputs after pan: a track panned hard left with the swap sounds on the right only.
        let m = MixTrack { swap: true, ..MixTrack::unity() }.compute_matrix();
        assert!(m[0][0].abs() < 1e-6 && m[1][1].abs() < 1e-6 && m[0][1] > 0.5 && m[1][0] > 0.5, "swapped: {m:?}");
        let left = MixTrack { pan: -6400, ..MixTrack::unity() }.compute_matrix();
        let m = MixTrack { pan: -6400, swap: true, ..MixTrack::unity() }.compute_matrix();
        assert!(m[0].iter().all(|g| g.abs() < 1e-9), "left output silent: {m:?}");
        assert_eq!(m[1], left[0], "the right output is the old left output");
        // A positive separation merges: fully mono at 64 (spec: audio/mixer.md).
        let mut t = MixTrack { sep: 64, ..MixTrack::unity() };
        let m = t.compute_matrix();
        assert!((m[0][0] - m[0][1]).abs() < 1e-6 && (m[1][0] - m[1][1]).abs() < 1e-6, "merged: {m:?}");
        t.sep = 0;
        let m = t.compute_matrix();
        assert!(m[0][1].abs() < 1e-6 && m[1][0].abs() < 1e-6, "original: {m:?}");
    }
    /// spec: audio/mixer.md "Main volume", audio/timing.md "Where the file ends": an export's samples never carry the
    /// main volume, only its tail test does; playback hears it.
    #[test]
    fn main_volume_moves_the_tail_test_not_the_export() {
        // A note that still sounds at the song's end, so the tail test has something to hear.
        let mut base = insert_project();
        base.patterns[0].notes[0] = long_note(300, 80, 0);
        let at = |v: u16| Project { main_volume: Some(v), ..base.clone() };
        let unity = render(&base);
        for v in [1u16, 800, 6400, 16000] {
            let out = render(&at(v));
            let n = out.len().min(unity.len());
            assert_eq!(out[..n], unity[..n], "main volume {v}: the samples are those at 12800");
        }
        assert!(render(&at(1)).len() <= unity.len() && render(&at(16000)).len() >= unity.len());
        assert!(render(&at(1)).len() < unity.len(), "a tiny gain ends the file sooner");
        // Playback: the master times curve(6400 / 12800).
        let live = |p: &Project| {
            let mut lib = SampleLibrary::new(None);
            let mut e = Engine::new(p, &mut lib, 44100, PlayMode::Pattern(1));
            let mut buf = vec![0.0f32; 8192];
            e.process(&mut buf);
            buf
        };
        let early = insert_project();
        let (a, b) = (live(&early), live(&Project { main_volume: Some(6400), ..early.clone() }));
        let g = os_dsp::level::curve(6400.0 * os_dsp::level::R12800);
        assert!(peak(&a, 0) > 0.01);
        assert!(a.iter().zip(&b).all(|(x, y)| (x * g - y).abs() < 1e-6), "playback scaled by {g}");
    }

    /// spec: audio/voice-levels.md "The channel's levels": the root note, fine tune and main pitch (when the channel
    /// follows it) move the channel's pitch; the note's fine pitch is the voice's own offset.
    #[test]
    fn root_fine_tune_and_main_pitch_set_the_channel_pitch() {
        let rate_of = |p: &Project| {
            let mut lib = SampleLibrary::new(None);
            let e = Engine::new(p, &mut lib, 44100, PlayMode::Pattern(1));
            e.channels[0].base_rate / e.channels[0].src_ratio
        };
        let base = project(vec![long_note(0, 96, 0)], vec![]);
        assert!((rate_of(&base) - 1.0).abs() < 1e-9);
        let mut p = base.clone();
        p.channels[0].misc.root = 72;
        assert!((rate_of(&p) - 0.5).abs() < 1e-9, "root an octave up plays an octave down");
        let mut p = base.clone();
        p.channels[0].misc.fine = 100;
        assert!((rate_of(&p) - 2f64.powf(1.0 / 12.0)).abs() < 1e-6, "fine tune in cents");
        let mut p = base.clone();
        p.main_pitch = Some(1200);
        assert!((rate_of(&p) - 2.0).abs() < 1e-9, "main pitch");
        p.channels[0].misc.main_pitch = false;
        assert!((rate_of(&p) - 1.0).abs() < 1e-9, "the switch cleared: no main pitch");
        let mut p = base.clone();
        p.patterns[0].notes[0].fine_pitch = 220;
        let mut lib = SampleLibrary::new(None);
        let e = Engine::new(&p, &mut lib, 44100, PlayMode::Pattern(1));
        assert!((e.triggers[0].rate - 2f64.powf(1000.0 / 1200.0)).abs() < 1e-9, "fine pitch 220 = +1000 cents");
        assert!((e.channels[0].base_rate / e.channels[0].src_ratio - 1.0).abs() < 1e-9, "the channel is not moved");
    }

    // spec: ui/metronome.md §4
    fn metronome_on() -> std::sync::Arc<crate::metronome::Switch> {
        let sw = std::sync::Arc::new(crate::metronome::Switch::default());
        sw.set_on(true);
        sw.set_sound(crate::metronome::Sound::Beep);
        sw
    }

    /// Frames where sound starts after at least 100 silent ones, with the peak of the next 5 ms.
    fn click_onsets(out: &[f32], rate: u32) -> Vec<(u64, f32)> {
        let left: Vec<f32> = out.chunks_exact(2).map(|f| f[0]).collect();
        let mut v = Vec::new();
        let mut last: Option<usize> = None;
        for (i, &s) in left.iter().enumerate() {
            if s.abs() < 1e-6 {
                continue;
            }
            if last.is_none_or(|l| i - l > 100) {
                let n = (rate / 200) as usize;
                v.push((i as u64, left[i..(i + n).min(left.len())].iter().fold(0.0f32, |m, x| m.max(x.abs()))));
            }
            last = Some(i);
        }
        v
    }

    #[test]
    fn metronome_clicks_every_beat_of_a_playing_pattern_and_accents_the_bar() {
        // An empty pattern in 3/4 loops a bar of three beats: accented, plain, plain, accented, ...
        let mut p = project(vec![], vec![]);
        p.bar = Some((3, 4));
        p.mixer = (0..2).map(os_model::MixerTrack::new).collect();
        let mut lib = SampleLibrary::new(None);
        let rate = 48000;
        let mut e = Engine::new(&p, &mut lib, rate, PlayMode::Pattern(1));
        e.set_looping(true);
        e.set_metronome(metronome_on());
        // Three seconds: seven beats.
        let mut out = vec![0.0; 2 * rate as usize * 3];
        e.process(&mut out);
        let on = click_onsets(&out, rate);
        // 130 BPM: a beat every 0.4615 s. Ticks are whole samples (the loop restarts the clock) and a sine's first
        // sample is 0, so an onset is within a few frames of the beat's exact time.
        let beats: Vec<u64> = (0..7).map(|b| crate::clock::Clock::frame_of_tick(rate, 96, 130.0, 96 * b)).collect();
        assert_eq!(on.len(), beats.len(), "{on:?}");
        for ((f, _), b) in on.iter().zip(&beats) {
            assert!(f.abs_diff(*b) <= 3, "{f} vs beat {b}");
        }
        let accented: Vec<bool> = on.iter().map(|o| o.1 > on[1].1 * 1.3).collect();
        assert_eq!(accented, [true, false, false, true, false, false, true]);
    }

    // spec: ui/tempo-tapper.md §6
    #[test]
    fn a_held_nudge_runs_playback_two_percent_slower_or_faster() {
        use std::sync::atomic::{AtomicI8, Ordering};
        // A pattern long enough not to end or loop (a note at bar 6 on a channel switched off).
        let mut p = project(vec![Note { length: 96, ..Note::step(0, 1904) }], vec![]);
        p.channels[0].enabled = false;
        let ticks = |n: i8| {
            let mut e = Engine::new(&p, &mut SampleLibrary::new(None), 48000, PlayMode::Pattern(1));
            let flag = std::sync::Arc::new(AtomicI8::new(n));
            e.set_nudge(flag.clone());
            let mut out = vec![0.0; 2 * 48000 * 4];
            e.process(&mut out);
            let held = e.tick_position();
            // Let go: the ticks after run at the tempo again.
            flag.store(0, Ordering::Relaxed);
            e.process(&mut out);
            (held, e.tick_position() - held)
        };
        // Four seconds at 130 BPM, PPQ 96: 832 ticks; 2 % fewer or more while nudged (the first tick runs before the
        // nudge takes hold), then 832 again.
        let ((slow, slow_after), (plain, _), (fast, fast_after)) = (ticks(-1), ticks(0), ticks(1));
        assert!((plain - 832.0).abs() < 2.0, "{plain}");
        assert!((slow / plain - 0.98).abs() < 0.003 && (fast / plain - 1.02).abs() < 0.003, "{slow} {plain} {fast}");
        assert!((slow_after - plain).abs() < 2.0 && (fast_after - plain).abs() < 2.0, "{slow_after} {fast_after}");
    }

    /// The clicks of three seconds of `p`'s pattern 1, looping, and which of them are accented.
    fn pattern_clicks(p: &Project, rate: u32) -> (Vec<(u64, f32)>, Vec<bool>) {
        let mut lib = SampleLibrary::new(None);
        let mut e = Engine::new(p, &mut lib, rate, PlayMode::Pattern(1));
        e.set_looping(true);
        e.set_metronome(metronome_on());
        let mut out = vec![0.0; 2 * rate as usize * 3];
        e.process(&mut out);
        let on = click_onsets(&out, rate);
        let accented = on.iter().map(|o| o.1 > on[1].1 * 1.3).collect();
        (on, accented)
    }

    // spec: ui/metronome.md §4, audio/song-structure.md "Time signatures and a pattern's length"
    #[test]
    fn metronome_follows_the_patterns_time_signature_markers() {
        use os_model::TimeMarker;
        let sig = |position: u32, n: u8, d: u8| TimeMarker { kind: TimeMarker::SIGNATURE, numerator: n, denominator: d, ..TimeMarker::new(position, &format!("{n}/{d}")) };
        let rate = 48000;
        let mut p = project(vec![], vec![]);
        p.mixer = (0..2).map(os_model::MixerTrack::new).collect();
        // A 4/4 project whose pattern is in 3/4: the accent comes every third beat, as in a 3/4 project.
        p.patterns[0].markers = vec![sig(0, 3, 4)];
        let (on, accented) = pattern_clicks(&p, rate);
        assert_eq!(on.len(), 7, "{on:?}");
        assert_eq!(accented, [true, false, false, true, false, false, true]);
        // 6/8: a click every eighth (48 ticks at 130 quarters a minute), accented every sixth; its bar loops.
        p.patterns[0].markers = vec![sig(0, 6, 8)];
        let (on, accented) = pattern_clicks(&p, rate);
        let beats: Vec<u64> = (0..13).map(|b| crate::clock::Clock::frame_of_tick(rate, 96, 130.0, 48 * b)).collect();
        assert_eq!(on.len(), beats.len(), "{on:?}");
        for ((f, _), b) in on.iter().zip(&beats) {
            assert!(f.abs_diff(*b) <= 3, "{f} vs beat {b}");
        }
        assert_eq!(accented, (0..13).map(|b| b % 6 == 0).collect::<Vec<_>>());
        // A marker later in the pattern starts a bar there: 4/4 for a bar, then 2/4 (a note to 2:3 on a channel switched off makes it six beats).
        p.patterns[0].markers = vec![sig(384, 2, 4)];
        p.patterns[0].notes = vec![Note { length: 96, ..Note::step(0, 480) }];
        p.channels[0].enabled = false;
        let (_, accented) = pattern_clicks(&p, rate);
        assert_eq!(&accented[..6], [true, false, false, false, true, false]);
        // Arrangement markers play no part in pattern mode.
        p.patterns[0].markers.clear();
        p.arrangements = vec![os_model::Arrangement { markers: vec![sig(0, 3, 4)], ..Default::default() }];
        let (_, accented) = pattern_clicks(&p, rate);
        assert_eq!(&accented[..5], [true, false, false, false, true]);
    }

    // spec: ui/recording.md §3
    #[test]
    fn a_count_in_holds_the_position_and_clicks_with_the_metronome_off() {
        // 3/4, one bar: three clicks (accented, plain, plain) while the position stays at 0, then playback moves on
        // and, the switch being off, clicks no more.
        let mut p = project(vec![], vec![]);
        p.bar = Some((3, 4));
        p.mixer = (0..2).map(os_model::MixerTrack::new).collect();
        let mut lib = SampleLibrary::new(None);
        let rate = 48000;
        let mut e = Engine::new(&p, &mut lib, rate, PlayMode::Pattern(1));
        e.set_looping(true);
        let off = metronome_on();
        off.set_on(false);
        e.set_metronome(off);
        e.set_count_in(1);
        assert_eq!(e.counting_in(), 3 * 96);
        let mut out = vec![0.0; 2 * rate as usize];
        e.process(&mut out);
        assert_eq!(e.tick_position(), 0.0, "held through the count-in");
        let mut more = vec![0.0; 2 * rate as usize * 2];
        e.process(&mut more);
        out.extend(more);
        assert_eq!(e.counting_in(), 0);
        assert!(e.tick_position() > 0.0, "playing again");
        let on = click_onsets(&out, rate);
        let beats: Vec<u64> = (0..3).map(|b| crate::clock::Clock::frame_of_tick(rate, 96, 130.0, 96 * b)).collect();
        assert_eq!(on.len(), 3, "{on:?}");
        for ((f, _), b) in on.iter().zip(&beats) {
            assert!(f.abs_diff(*b) <= 3, "{f} vs beat {b}");
        }
        assert!(on[0].1 > on[1].1 * 1.3 && on[1].1 < on[2].1 * 1.3);
    }

    // spec: ui/recording.md §4
    #[test]
    fn run_on_plays_past_the_loop_end_while_set() {
        let mut p = project(vec![], vec![]);
        p.mixer = (0..2).map(os_model::MixerTrack::new).collect();
        let mut lib = SampleLibrary::new(None);
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Pattern(1));
        e.set_looping(true);
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        e.set_run_on(flag.clone());
        // Three seconds at 130 BPM: past the one-bar pattern's end (1.85 s).
        let mut out = vec![0.0; 2 * 48000 * 3];
        e.process(&mut out);
        assert!(e.tick_position() > 384.0, "ran on: {}", e.tick_position());
        assert!(!e.finished());
        flag.store(false, std::sync::atomic::Ordering::Relaxed);
        let mut out = vec![0.0; 2 * 4800];
        e.process(&mut out);
        assert!(e.tick_position() < 384.0, "back to the start: {}", e.tick_position());
    }

    #[test]
    fn metronome_clicks_every_beat_of_the_bar_at_the_tempo() {
        // 3/8 at 130 BPM: a click on every eighth (48 ticks), still 60 / 130 s apart, accented every three.
        let mut p = project(vec![], vec![]);
        p.bar = Some((3, 2));
        p.mixer = (0..2).map(os_model::MixerTrack::new).collect();
        let mut lib = SampleLibrary::new(None);
        let rate = 48000;
        let mut e = Engine::new(&p, &mut lib, rate, PlayMode::Pattern(1));
        e.set_looping(true);
        e.set_metronome(metronome_on());
        let mut out = vec![0.0; 2 * rate as usize * 3];
        e.process(&mut out);
        let on = click_onsets(&out, rate);
        let beats: Vec<u64> = (0..7).map(|b| (f64::from(rate) * 60.0 / 130.0 * b as f64).round() as u64).collect();
        assert_eq!(on.len(), beats.len(), "{on:?}");
        for ((f, _), b) in on.iter().zip(&beats) {
            assert!(f.abs_diff(*b) <= 3, "{f} vs beat {b}");
        }
        let accented: Vec<bool> = on.iter().map(|o| o.1 > on[1].1 * 1.3).collect();
        assert_eq!(accented, [true, false, false, true, false, false, true]);
    }

    /// spec: audio/timing.md "The tempo counts the bar's beats": a tick lasts `60 / (tempo × steps / 4) / PPQ`
    /// seconds, so in 3/8 (2 steps per beat) a note a quarter note in plays twice as late as in 4/4, and in 3/2 (8
    /// steps) half as late.
    #[test]
    fn notes_play_at_the_bars_beat() {
        for (bar, tempo) in [(None, 130.0), (Some((3, 2)), 65.0), (Some((3, 8)), 260.0)] {
            let mut p = project(vec![Note::step(0, 96), Note::step(0, 192)], vec![]);
            p.bar = bar;
            p.channels[0].kind = ChannelKind::Plugin;
            let mut lib = SampleLibrary::default();
            let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Pattern(1));
            let probe = Arc::new(std::sync::Mutex::new(Probe { log: vec![], held: vec![], at: 0 }));
            e.set_instrument(0, probe.clone());
            let mut out = vec![0.0f32; 512 * 2];
            for _ in 0..400 {
                e.process(&mut out);
            }
            let ons: Vec<u64> = probe.lock().unwrap().log.iter().filter(|l| l.1).map(|l| l.0).collect();
            assert_eq!(ons[0], laws::tick_to_frame(96.0, 96, tempo, 48000), "{bar:?} {ons:?}");
            assert_eq!(ons[1], laws::tick_to_frame(192.0, 96, tempo, 48000), "{bar:?} {ons:?}");
        }
    }

    #[test]
    fn metronome_is_never_in_an_export_and_follows_the_master_track() {
        let mut p = project(vec![], vec![]);
        p.mixer = (0..2).map(os_model::MixerTrack::new).collect();
        let mut lib = SampleLibrary::new(None);
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Pattern(1));
        e.set_metronome(metronome_on());
        let mut out = Vec::new();
        e.render_ticks(400, &mut out);
        assert!(out.iter().all(|&s| s == 0.0), "an export never clicks");
        // The master track muted: playback is silent too.
        p.mixer[0].enabled = false;
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Pattern(1));
        e.set_looping(true);
        e.set_metronome(metronome_on());
        let mut out = vec![0.0; 2 * 48000];
        e.process(&mut out);
        assert!(out.iter().all(|&s| s == 0.0), "muted master");
    }

    /// Settings > Audio "Metronome track": the clicks enter that insert track, so muting it silences them; a track the
    /// project does not have (or the hidden last one) is the master. The count-in and a tap go the same way.
    #[test]
    fn metronome_enters_its_mixer_track() {
        let mut p = project(vec![], vec![]);
        p.mixer = (0..4).map(os_model::MixerTrack::new).collect();
        let mut lib = SampleLibrary::new(None);
        let play = |p: &Project, lib: &mut SampleLibrary, track: u16| {
            let sw = metronome_on();
            sw.set_track(track);
            let mut e = Engine::new(p, lib, 48000, PlayMode::Pattern(1));
            e.set_looping(true);
            e.set_metronome(sw);
            let mut out = vec![0.0; 2 * 48000];
            e.process(&mut out);
            out
        };
        assert!(play(&p, &mut lib, 2).iter().any(|&s| s != 0.0), "insert 2 goes on to the master");
        p.mixer[2].enabled = false;
        assert!(play(&p, &mut lib, 2).iter().all(|&s| s == 0.0), "insert 2 muted");
        assert!(play(&p, &mut lib, 1).iter().any(|&s| s != 0.0), "another track");
        for missing in [3, 9] {
            assert!(play(&p, &mut lib, missing).iter().any(|&s| s != 0.0), "track {missing}: the master");
        }
        // The count-in (the switch off: it clicks anyway) and a tap.
        for (track, heard) in [(2, false), (0, true)] {
            let sw = metronome_on();
            sw.set_on(false);
            sw.set_track(track);
            let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Pattern(1));
            e.set_metronome(sw);
            e.set_count_in(1);
            let mut out = vec![0.0; 2 * 48000];
            e.process(&mut out);
            assert_eq!(out.iter().any(|&s| s != 0.0), heard, "count-in into track {track}");
        }
        let sw = metronome_on();
        sw.set_track(2);
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Pattern(1));
        e.set_metronome(sw.clone());
        e.clear_schedule();
        e.tap_click(true, 0);
        let mut out = vec![0.0; 2 * 2000];
        e.process(&mut out);
        assert!(out.iter().all(|&s| s == 0.0), "a tap into muted insert 2");
        sw.set_track(0);
        e.tap_click(true, 0);
        e.process(&mut out);
        assert!(out.iter().any(|&s| s != 0.0), "a tap into the master");
    }

    // spec: ui/tempo-tapper.md §8
    #[test]
    fn a_tap_clicks_on_a_stream_of_live_notes_and_carries_over_a_rebuild() {
        let p = project(vec![], vec![]);
        let mut lib = SampleLibrary::new(None);
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Pattern(1));
        e.clear_schedule();
        assert!(e.finished());
        e.tap_click(true, 0);
        assert!(!e.finished(), "a click keeps a stream of live notes going");
        let mut out = vec![0.0; 2 * 240];
        e.process(&mut out);
        let on = click_onsets(&out, 48000);
        assert_eq!(on.len(), 1);
        assert!(on[0].0 < 2 && (on[0].1 - 1.0).abs() < 0.05, "accented Tick from the start: {on:?}");
        // The tempo changes while it sounds: the rebuilt engine plays the rest of it.
        let mut p2 = p.clone();
        p2.tempo = Some(90.0);
        let mut e2 = Engine::new(&p2, &mut lib, 48000, PlayMode::Pattern(1));
        e2.clear_schedule();
        e2.continue_from(&mut e);
        let mut rest = vec![0.0; 2 * 2000];
        e2.process(&mut rest);
        assert!(rest.iter().any(|&s| s != 0.0));
        assert!(e2.finished(), "silent again once the click is over");
    }

    /// A device's inputs held at fixed levels.
    struct Held(Vec<f32>);

    impl DeviceInput for Held {
        fn channels(&self) -> usize {
            self.0.len()
        }
        fn read(&mut self, out: &mut [f32]) {
            for f in out.chunks_exact_mut(self.0.len()) {
                f.copy_from_slice(&self.0);
            }
        }
    }

    /// Plays `p` for a second on a device of `pairs` output pairs whose two inputs hold `inputs`; the last frame.
    fn on_device(p: &Project, pairs: usize, inputs: [f32; 2]) -> Vec<f32> {
        let mut lib = SampleLibrary::new(None);
        let mut e = Engine::new(p, &mut lib, 48000, PlayMode::Pattern(1));
        e.set_looping(true);
        e.set_device(pairs, Some(Box::new(Held(inputs.to_vec()))));
        assert_eq!(e.width(), 2 * pairs);
        let mut out = vec![0.0; 2 * pairs * 48000];
        e.process(&mut out);
        out[out.len() - 2 * pairs..].to_vec()
    }

    /// What the armed tracks hand to the recorder: every push, in order.
    #[derive(Default)]
    struct Takes(std::sync::Mutex<Vec<(usize, Vec<f32>)>>);

    impl RecordTap for Takes {
        fn push(&self, track: usize, _: u64, frames: &[f32]) {
            self.0.lock().unwrap().push((track, frames.to_vec()));
        }
    }

    /// Records a second of `p` on a device with `inputs` held: the pushes per track (frames), and the last frame
    /// track 1 took (L, R).
    fn record_takes(p: &Project, inputs: [f32; 2], count_in: u32) -> (Vec<(usize, usize)>, Option<[f32; 2]>) {
        let mut lib = SampleLibrary::new(None);
        let mut e = Engine::new(p, &mut lib, 48000, PlayMode::Pattern(1));
        e.set_looping(true);
        e.set_device(1, Some(Box::new(Held(inputs.to_vec()))));
        let takes = Arc::new(Takes::default());
        e.set_record_tap(Some(takes.clone()));
        e.set_count_in(count_in);
        let mut out = vec![0.0; 2 * 48000];
        e.process(&mut out);
        let got = takes.0.lock().unwrap();
        let mut frames: Vec<(usize, usize)> = Vec::new();
        for (t, f) in got.iter() {
            match frames.iter_mut().find(|(k, _)| k == t) {
                Some(e) => e.1 += f.len() / 2,
                None => frames.push((*t, f.len() / 2)),
            }
        }
        let last = got.iter().rev().find(|(t, _)| *t == 1).map(|(_, f)| [f[f.len() - 2], f[f.len() - 1]]);
        (frames, last)
    }

    // spec: audio/track-io.md §2.5: only armed tracks record, each where its record source says.
    #[test]
    fn armed_tracks_record_at_their_source() {
        let (frames, _) = record_takes(&io_project(0, os_model::MONITOR_ARMED, false), [0.5, 0.25], 0);
        assert!(frames.is_empty(), "nothing armed: {frames:?}");
        // External input only: the input as it came in, heard or not.
        let mut p = io_project(0, os_model::MONITOR_OFF, true);
        p.mixer[1].rec_source = os_model::REC_EXTERNAL;
        let (frames, last) = record_takes(&p, [0.5, 0.25], 0);
        assert_eq!(frames.len(), 1, "track 1 alone: {frames:?}");
        assert!(frames[0].0 == 1 && frames[0].1 >= 47000, "a second of it: {frames:?}");
        assert_eq!(last, Some([0.5, 0.25]));
        // In 2 alone: on both sides.
        p.mixer[1].audio_in = 2;
        assert_eq!(record_takes(&p, [0.5, 0.25], 0).1, Some([0.25, 0.25]));
        // External and mixer input, heard: the input once, not twice.
        let mut p = io_project(0, os_model::MONITOR_ARMED, true);
        p.mixer[1].rec_source = 1;
        assert_eq!(record_takes(&p, [0.5, 0.25], 0).1, Some([0.5, 0.25]));
        // Post level and panning: after the fader.
        p.mixer[1].rec_source = 4;
        p.mixer[1].volume = Some(0.4);
        let [l, r] = record_takes(&p, [0.5, 0.25], 0).1.unwrap();
        assert!(l > 0.0 && l < 0.4 && r < 0.2, "faded: {l} {r}");
        // Post EQ (the default): before the fader.
        p.mixer[1].rec_source = os_model::REC_POST_EQ;
        assert_eq!(record_takes(&p, [0.5, 0.25], 0).1, Some([0.5, 0.25]));
        // No input and Post EQ: what reaches the track (silence here), still a take.
        p.mixer[1].audio_in = -1;
        let (frames, last) = record_takes(&p, [0.5, 0.25], 0);
        assert!(frames.len() == 1 && last == Some([0.0, 0.0]), "{frames:?} {last:?}");
    }

    // spec: audio/track-io.md §2.5: a count-in records nothing.
    #[test]
    fn a_count_in_is_not_recorded() {
        let mut p = io_project(0, os_model::MONITOR_ARMED, true);
        p.tempo = Some(60.0);
        let (frames, _) = record_takes(&p, [0.5, 0.25], 1);
        assert!(frames.is_empty(), "a bar at 60 BPM outlasts the second: {frames:?}");
    }

    fn io_project(input: i32, monitor: u8, armed: bool) -> Project {
        let mut p = project(vec![], vec![]);
        p.mixer = (0..3).map(os_model::MixerTrack::new).collect();
        (p.mixer[1].audio_in, p.mixer[1].monitor, p.mixer[1].armed) = (input, monitor, armed);
        p
    }

    // spec: audio/track-io.md §1 "Signal", §2.1: a pair is heard left / right, one input alone on both sides.
    #[test]
    fn a_monitored_input_reaches_the_master() {
        let f = on_device(&io_project(0, os_model::MONITOR_ON, false), 1, [0.5, 0.0]);
        assert!(f[0] > 0.1 && f[1].abs() < 1e-6, "the pair: {f:?}");
        let f = on_device(&io_project(2, os_model::MONITOR_ON, false), 1, [0.0, 0.5]);
        assert!(f[0] > 0.1 && (f[0] - f[1]).abs() < 1e-6, "In 2 on both sides: {f:?}");
        assert!(on_device(&io_project(-1, os_model::MONITOR_ON, false), 1, [0.5, 0.5]).iter().all(|&s| s == 0.0), "(none)");
        let mut muted = io_project(0, os_model::MONITOR_ON, false);
        muted.mixer[1].enabled = false;
        assert!(on_device(&muted, 1, [0.5, 0.5]).iter().all(|&s| s == 0.0), "muted");
    }

    // spec: audio/track-io.md §1 "Monitor external input".
    #[test]
    fn monitoring_follows_the_setting_and_the_arm() {
        let heard = |monitor, armed| on_device(&io_project(0, monitor, armed), 1, [0.5, 0.5]).iter().any(|&s| s != 0.0);
        assert_eq!([heard(os_model::MONITOR_OFF, false), heard(os_model::MONITOR_OFF, true)], [false, false]);
        assert_eq!([heard(os_model::MONITOR_ARMED, false), heard(os_model::MONITOR_ARMED, true)], [false, true]);
        assert_eq!([heard(os_model::MONITOR_ON, false), heard(os_model::MONITOR_ON, true)], [true, true]);
    }

    // spec: audio/track-io.md §1 "Signal": a direct output plays besides the routing; the master's (none) silences it.
    #[test]
    fn direct_outputs_play_on_their_pair() {
        let mut p = io_project(0, os_model::MONITOR_ON, false);
        p.mixer[1].audio_out = 1;
        let f = on_device(&p, 2, [0.5, 0.5]);
        assert!(f[0] > 0.1 && f[2] > 0.1, "the master on pair 1, the track on pair 2: {f:?}");
        // Both faders at unity: the master hears the track as the pair does.
        assert!((f[0] - f[2]).abs() < 1e-5, "both at unity: {f:?}");
        p.mixer[0].audio_out = -1;
        let f = on_device(&p, 2, [0.5, 0.5]);
        assert!(f[0] == 0.0 && f[1] == 0.0 && f[2] > 0.1, "{f:?}");
        // A pair the device does not have plays nowhere.
        p.mixer[1].audio_out = 5;
        assert!(on_device(&p, 2, [0.5, 0.5]).iter().all(|&s| s == 0.0));
        p.mixer[1].enabled = false;
        p.mixer[1].audio_out = 1;
        assert!(on_device(&p, 2, [0.5, 0.5]).iter().all(|&s| s == 0.0), "muted");
    }

    #[test]
    fn input_numbers_are_pairs_then_single_inputs() {
        assert_eq!((0..7).map(|n| input_channels(n, 4)).collect::<Vec<_>>(), [Some((0, 1)), Some((2, 3)), Some((0, 0)), Some((1, 1)), Some((2, 2)), Some((3, 3)), None]);
        assert_eq!([input_channels(-1, 4), input_channels(0, 0), input_channels(0, 1)], [None, None, Some((0, 0))]);
    }
}
