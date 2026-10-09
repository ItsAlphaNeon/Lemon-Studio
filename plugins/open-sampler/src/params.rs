//! Open Sampler parameters. Every control of the editor is a host-visible parameter, addressed by its id.
//!
// spec: plugins/open-sampler.md (controls per tab, defaults, working vs inert)

use std::sync::{Arc, RwLock};

use nih_plug::prelude::*;
use os_engine::envelope;

pub type Fmt = Arc<dyn Fn(f32) -> String + Send + Sync>;

fn percent() -> Fmt {
    Arc::new(|v| format!("{:.0}%", v * 100.0))
}

fn bipolar() -> Fmt {
    Arc::new(|v| if v.abs() < 0.005 { "Centered".into() } else { format!("{:.0}%", v * 100.0) })
}

#[allow(dead_code)]
fn pan_text() -> Fmt {
    Arc::new(|v| {
        let p = (v * 100.0).round();
        if p == 0.0 { "Centered".into() } else if p < 0.0 { format!("{:.0}% left", -p) } else { format!("{p:.0}% right") }
    })
}

/// A 0..1 knob shown in percent.
pub fn knob(name: &str, default: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Linear { min: 0.0, max: 1.0 }).with_value_to_string(percent())
}

/// A -1..1 knob (centre = 0).
pub fn bi_knob(name: &str, default: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Linear { min: -1.0, max: 1.0 }).with_value_to_string(bipolar())
}

#[allow(dead_code)]
fn pan_knob(name: &str) -> FloatParam {
    FloatParam::new(name, 0.0, FloatRange::Linear { min: -1.0, max: 1.0 })
        .with_smoother(SmoothingStyle::Linear(10.0))
        .with_value_to_string(pan_text())
}

/// A choice list stored as an integer (the label is the value text).
fn choice(name: &str, default: i32, labels: &'static [&'static str]) -> IntParam {
    IntParam::new(name, default, IntRange::Linear { min: 0, max: labels.len() as i32 - 1 })
        .with_value_to_string(Arc::new(move |v| labels.get(v as usize).copied().unwrap_or("?").to_string()))
}

/// A spin box where 0 shows `---` (off / unlimited).
#[allow(dead_code)]
fn spin(name: &str, default: i32, max: i32) -> IntParam {
    IntParam::new(name, default, IntRange::Linear { min: 0, max })
        .with_value_to_string(Arc::new(|v| if v == 0 { "---".into() } else { v.to_string() }))
}

/// Envelope time knobs: field range 100..=65536 of the channel envelope records (spec: audio/channel-envelopes.md).
pub const TIME_MIN: f32 = 100.0;
pub const TIME_MAX: f32 = 65536.0;

pub fn time_field(norm: f32) -> i32 {
    (TIME_MIN + norm.clamp(0.0, 1.0) * (TIME_MAX - TIME_MIN)).round() as i32
}

fn time_norm(field: i32) -> f32 {
    (field as f32 - TIME_MIN) / (TIME_MAX - TIME_MIN)
}

fn seconds_text(falling: bool) -> Fmt {
    Arc::new(move |v| {
        let f = time_field(v);
        let s = if falling { envelope::fall_seconds(f) } else { envelope::knob_seconds(f) };
        if s < 1.0 { format!("{:.0} ms", s * 1000.0) } else { format!("{s:.2} s") }
    })
}

fn time_knob(name: &str, field: i32, falling: bool) -> FloatParam {
    FloatParam::new(name, time_norm(field), FloatRange::Linear { min: 0.0, max: 1.0 }).with_value_to_string(seconds_text(falling))
}

pub const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// FL note naming: key 60 = C5.
pub fn note_name(key: i32) -> String {
    format!("{}{}", NOTE_NAMES[key.rem_euclid(12) as usize], key.div_euclid(12))
}

pub const DECLICK: &[&str] = &["None", "Out only (no bleeding)", "Transient (no bleeding)", "Generic (bleeding)", "Smooth (bleeding)", "Crossfade (no bleeding)"];
pub const STRETCH: &[&str] = &["Resample", "E3 generic", "E3 mono", "Slice stretch", "Slice map", "Auto"];
pub const FILTERS: &[&str] = &["Fast LP", "LP", "BP", "HP", "BS", "LPx2", "SVF LP", "SVF LP x2"];
pub const LFO_SHAPES: &[&str] = &["Sine", "Triangle", "Square"];

/// One envelope / LFO target of the envelope tab (Panning, Volume, Mod X, Mod Y, Pitch).
#[derive(Params)]
pub struct EnvParams {
    #[id = "on"]
    pub on: BoolParam,
    #[id = "delay"]
    pub delay: FloatParam,
    #[id = "att"]
    pub att: FloatParam,
    #[id = "hold"]
    pub hold: FloatParam,
    #[id = "dec"]
    pub dec: FloatParam,
    #[id = "sus"]
    pub sus: FloatParam,
    #[id = "rel"]
    pub rel: FloatParam,
    #[id = "att_t"]
    pub att_t: FloatParam,
    #[id = "dec_t"]
    pub dec_t: FloatParam,
    #[id = "rel_t"]
    pub rel_t: FloatParam,
    /// Envelope amount (pitch: ±2400 cents at full scale). Not on the Volume target.
    #[id = "amt"]
    pub amt: FloatParam,
    #[id = "tempo"]
    pub tempo: BoolParam,
    #[id = "lfo_shape"]
    pub lfo_shape: IntParam,
    #[id = "lfo_delay"]
    pub lfo_delay: FloatParam,
    #[id = "lfo_att"]
    pub lfo_att: FloatParam,
    #[id = "lfo_amt"]
    pub lfo_amt: FloatParam,
    #[id = "lfo_speed"]
    pub lfo_speed: FloatParam,
    #[id = "lfo_tempo"]
    pub lfo_tempo: BoolParam,
    #[id = "lfo_global"]
    pub lfo_global: BoolParam,
}

impl EnvParams {
    fn new(target: &str, rel_tension: i32) -> Self {
        let n = |s: &str| format!("{target} {s}");
        let d = os_model::Envelope::default();
        let tension = |s: &str, t: i32| {
            FloatParam::new(&n(s), t as f32 / 128.0, FloatRange::Linear { min: -1.0, max: 1.0 })
                .with_value_to_string(Arc::new(|v| format!("{:.0}", v * 128.0)))
        };
        EnvParams {
            on: BoolParam::new(n("envelope"), false),
            delay: time_knob(&n("delay"), d.delay, false),
            att: time_knob(&n("attack"), d.attack, false),
            hold: time_knob(&n("hold"), d.hold, false),
            dec: time_knob(&n("decay"), d.decay, true),
            sus: FloatParam::new(n("sustain"), d.sustain as f32 / 128.0, FloatRange::Linear { min: 0.0, max: 1.0 }).with_value_to_string(percent()),
            rel: time_knob(&n("release"), d.release, true),
            att_t: tension("attack tension", 0),
            dec_t: tension("decay tension", 0),
            rel_t: tension("release tension", rel_tension),
            amt: bi_knob(&n("amount"), 0.0),
            tempo: BoolParam::new(n("tempo"), false),
            lfo_shape: choice(&n("LFO shape"), 0, LFO_SHAPES),
            lfo_delay: knob(&n("LFO delay"), 0.0),
            lfo_att: knob(&n("LFO attack"), 0.3),
            lfo_amt: bi_knob(&n("LFO amount"), 0.0),
            lfo_speed: knob(&n("LFO speed"), 0.6),
            lfo_tempo: BoolParam::new(n("LFO tempo"), false),
            lfo_global: BoolParam::new(n("LFO global"), false),
        }
    }

    /// The record in channel-envelope units, `None` when off.
    pub fn envelope(&self) -> Option<envelope::Params> {
        let rec = os_model::Envelope {
            enabled: self.on.value(),
            delay: time_field(self.delay.value()),
            attack: time_field(self.att.value()),
            hold: time_field(self.hold.value()),
            decay: time_field(self.dec.value()),
            sustain: (self.sus.value() * 128.0).round() as i32,
            release: time_field(self.rel.value()),
            amount: (self.amt.value() * 2400.0).round() as i32,
            attack_tension: (self.att_t.value() * 128.0).round() as i32,
            decay_tension: (self.dec_t.value() * 128.0).round() as i32,
            release_tension: (self.rel_t.value() * 128.0).round() as i32,
        };
        envelope::Params::new(&rec)
    }
}

#[derive(Params)]
pub struct SamplerParams {
    /// The loaded sample's path (empty = none).
    #[persist = "sample-path"]
    pub sample_path: Arc<RwLock<String>>,

    // Tab 1: sample.
    #[id = "keep_disk"]
    pub keep_disk: BoolParam,
    #[id = "resample"]
    pub resample: BoolParam,
    #[id = "load_regions"]
    pub load_regions: BoolParam,
    #[id = "load_slices"]
    pub load_slices: BoolParam,
    #[id = "declick"]
    pub declick: IntParam,
    #[id = "start_offset"]
    pub start_offset: FloatParam,
    #[id = "loop_start"]
    pub loop_start: FloatParam,
    #[id = "loop_end"]
    pub loop_end: FloatParam,
    #[id = "loop_use"]
    pub loop_use: BoolParam,
    #[id = "loop_pingpong"]
    pub loop_pingpong: BoolParam,
    #[id = "ts_pitch"]
    pub ts_pitch: FloatParam,
    #[id = "ts_mul"]
    pub ts_mul: FloatParam,
    #[id = "ts_time"]
    pub ts_time: FloatParam,
    #[id = "ts_mode"]
    pub ts_mode: IntParam,
    #[id = "fx_dc"]
    pub fx_dc: BoolParam,
    #[id = "fx_normalize"]
    pub fx_normalize: BoolParam,
    #[id = "fx_reverse"]
    pub fx_reverse: BoolParam,
    #[id = "fx_polarity"]
    pub fx_polarity: BoolParam,
    #[id = "fx_fade_stereo"]
    pub fx_fade_stereo: BoolParam,
    #[id = "fx_swap"]
    pub fx_swap: BoolParam,
    #[id = "fx_start"]
    pub fx_start: FloatParam,
    #[id = "fx_length"]
    pub fx_length: FloatParam,
    #[id = "fx_in"]
    pub fx_in: FloatParam,
    #[id = "fx_out"]
    pub fx_out: FloatParam,
    #[id = "fx_crossfade"]
    pub fx_crossfade: FloatParam,
    #[id = "fx_trim"]
    pub fx_trim: FloatParam,

    // Tab 2: envelope / instrument.
    #[nested(id_prefix = "envpan", group = "Panning")]
    pub env_pan: EnvParams,
    #[nested(id_prefix = "envvol", group = "Volume")]
    pub env_vol: EnvParams,
    #[nested(id_prefix = "envmodx", group = "Mod X")]
    pub env_modx: EnvParams,
    #[nested(id_prefix = "envmody", group = "Mod Y")]
    pub env_mody: EnvParams,
    #[nested(id_prefix = "envpitch", group = "Pitch")]
    pub env_pitch: EnvParams,
    #[id = "f_cut"]
    pub f_cut: FloatParam,
    #[id = "f_res"]
    pub f_res: FloatParam,
    #[id = "f_type"]
    pub f_type: IntParam,
    #[id = "root"]
    pub root: IntParam,
    #[id = "fine"]
    pub fine: IntParam,
    #[id = "main_pitch"]
    pub main_pitch: BoolParam,
    #[id = "add_key"]
    pub add_key: BoolParam,

}

impl Default for SamplerParams {
    fn default() -> Self {
        SamplerParams {
            sample_path: Arc::new(RwLock::new(String::new())),
            keep_disk: BoolParam::new("Keep on disk", false),
            resample: BoolParam::new("Resample", true),
            load_regions: BoolParam::new("Load regions", true),
            load_slices: BoolParam::new("Load slice markers", false),
            declick: choice("Declicking mode", 1, DECLICK),
            start_offset: knob("Start offset", 0.0),
            loop_start: knob("Loop start", 0.0),
            loop_end: knob("Loop end", 0.0),
            loop_use: BoolParam::new("Use loop points", true),
            loop_pingpong: BoolParam::new("Ping pong loop", false),
            ts_pitch: bi_knob("Stretch pitch", 0.0),
            ts_mul: bi_knob("Stretch multiplier", 0.0),
            ts_time: knob("Stretch time", 0.0),
            ts_mode: choice("Stretch mode", 0, STRETCH),
            fx_dc: BoolParam::new("Remove DC offset", false),
            fx_normalize: BoolParam::new("Normalize", false),
            fx_reverse: BoolParam::new("Reverse", false),
            fx_polarity: BoolParam::new("Reverse polarity", false),
            fx_fade_stereo: BoolParam::new("Fade stereo", false),
            fx_swap: BoolParam::new("Swap stereo", false),
            fx_start: knob("Sample start", 0.0),
            fx_length: knob("Length", 1.0),
            fx_in: knob("Fade in", 0.0),
            fx_out: knob("Fade out", 0.0),
            fx_crossfade: knob("Crossfade", 0.0),
            fx_trim: knob("Trim", 0.0),
            env_pan: EnvParams::new("Panning", 0),
            env_vol: EnvParams::new("Volume", -101),
            env_modx: EnvParams::new("Mod X", 0),
            env_mody: EnvParams::new("Mod Y", 0),
            env_pitch: EnvParams::new("Pitch", 0),
            f_cut: knob("Mod X (cutoff)", 1.0),
            f_res: knob("Mod Y (resonance)", 0.0),
            f_type: choice("Filter type", 0, FILTERS),
            root: IntParam::new("Root note", 60, IntRange::Linear { min: 0, max: 131 }).with_value_to_string(Arc::new(note_name)),
            fine: IntParam::new("Fine tune", 0, IntRange::Linear { min: -100, max: 100 }).with_value_to_string(Arc::new(|v| format!("{v} cents"))),
            main_pitch: BoolParam::new("Enable main pitch", true),
            add_key: BoolParam::new("Add to key", false),
        }
    }
}

impl SamplerParams {
    /// The envelope tab's five targets in sub-tab order.
    pub fn env(&self, target: usize) -> &EnvParams {
        [&self.env_pan, &self.env_vol, &self.env_modx, &self.env_mody, &self.env_pitch][target.min(4)]
    }
}

/// Precomputed-effect settings: the processed sample is rebuilt when these change.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FxKey {
    pub dc: bool,
    pub normalize: bool,
    pub reverse: bool,
    pub polarity: bool,
    pub fade_stereo: bool,
    pub swap: bool,
    pub start: f32,
    pub length: f32,
    pub fade_in: f32,
    pub fade_out: f32,
}

impl SamplerParams {
    pub fn fx_key(&self) -> FxKey {
        FxKey {
            dc: self.fx_dc.value(),
            normalize: self.fx_normalize.value(),
            reverse: self.fx_reverse.value(),
            polarity: self.fx_polarity.value(),
            fade_stereo: self.fx_fade_stereo.value(),
            swap: self.fx_swap.value(),
            start: self.fx_start.value(),
            length: self.fx_length.value(),
            fade_in: self.fx_in.value(),
            fade_out: self.fx_out.value(),
        }
    }
}
