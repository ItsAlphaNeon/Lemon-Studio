//! Open Sampler parameters. Every control of the editor is a host-visible parameter, addressed by its id. Most are
//! stored as the knob position 0..1 (FL's knobs are positions too) and converted where they are used; the value
//! texts follow FL's hint bar.
//!
// spec: plugins/open-sampler.md (§Controls: ids, ranges, defaults, value texts)

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, RwLock};

use nih_plug::prelude::*;
use os_engine::envelope;

pub type Fmt = Arc<dyn Fn(f32) -> String + Send + Sync>;

/// Editor edits the host has not handed back yet (spec: plugins/open-sampler.md §4 "Edits apply at once").
///
/// The editor tells the host about every edit; nih-plug then changes the parameter only when the host passes the edit
/// back (VST3 while processing) or flushes (CLAP), and some hosts do neither. So the sound reads parameters through
/// [`Live::live`]: the editor's newest value wins until the host's own value moves to it (confirmed) or away from the
/// value it had at the edit (automation / undo in the host). Entries are keyed by the parameter's address, unique per
/// plugin instance.
static LIVE: std::sync::Mutex<Vec<LiveEdit>> = std::sync::Mutex::new(Vec::new());

#[derive(Clone, Copy)]
struct LiveEdit {
    addr: usize,
    /// The editor's value (normalised).
    norm: f32,
    /// The host's value when the edit was made.
    base: f32,
}

/// Records an editor edit of the parameter at `addr` whose host value is `host` now.
pub fn live_set(addr: usize, norm: f32, host: f32) {
    if let Ok(mut l) = LIVE.lock() {
        l.retain(|e| e.addr != addr);
        if (norm - host).abs() > 1e-7 {
            l.push(LiveEdit { addr, norm, base: host });
        }
    }
}

/// The normalised value the sound should use for the parameter at `addr` (`host` = its host value), or `None` to use
/// the host's. Never blocks: when the editor holds the list, the host value is used for this block.
pub fn live_norm(addr: usize, host: f32) -> Option<f32> {
    let mut l = LIVE.try_lock().ok()?;
    let i = l.iter().position(|e| e.addr == addr)?;
    let e = l[i];
    if (host - e.base).abs() <= 1e-7 && (host - e.norm).abs() > 1e-7 {
        Some(e.norm)
    } else {
        // Confirmed, or the host changed it since: the host's value from now on.
        l.swap_remove(i);
        None
    }
}

/// Forgets the edits of parameters inside `from..to` (a dropped instance).
fn live_forget(from: usize, to: usize) {
    if let Ok(mut l) = LIVE.lock() {
        l.retain(|e| e.addr < from || e.addr >= to);
    }
}

/// The normalised value the sound uses for `p` (its pending editor edit, else the host's value).
pub fn live_of(p: ParamPtr) -> f32 {
    // SAFETY: callers hold pointers from a live params object.
    let host = unsafe { p.unmodulated_normalized_value() };
    live_norm(param_addr(p), host).unwrap_or(host)
}

/// The address `live_set` keys a parameter by.
pub fn param_addr(p: ParamPtr) -> usize {
    match p {
        ParamPtr::FloatParam(p) => p as usize,
        ParamPtr::IntParam(p) => p as usize,
        ParamPtr::BoolParam(p) => p as usize,
        ParamPtr::EnumParam(p) => p as usize,
    }
}

/// Parameter values for the sound (see [`LIVE`]).
pub trait Live {
    type Plain;
    fn live(&self) -> Self::Plain;
}

impl Live for FloatParam {
    type Plain = f32;
    fn live(&self) -> f32 {
        match live_norm(self as *const Self as usize, self.unmodulated_normalized_value()) {
            Some(n) => self.preview_plain(n),
            None => self.value(),
        }
    }
}

impl Live for IntParam {
    type Plain = i32;
    fn live(&self) -> i32 {
        match live_norm(self as *const Self as usize, self.unmodulated_normalized_value()) {
            Some(n) => self.preview_plain(n),
            None => self.value(),
        }
    }
}

impl Live for BoolParam {
    type Plain = bool;
    fn live(&self) -> bool {
        match live_norm(self as *const Self as usize, self.unmodulated_normalized_value()) {
            Some(n) => self.preview_plain(n),
            None => self.value(),
        }
    }
}


fn percent() -> Fmt {
    Arc::new(|v| format!("{:.0}%", v * 100.0))
}

/// "N %" (with a space: SMP START / LENGTH, as FL writes them).
fn percent_sp() -> Fmt {
    Arc::new(|v| format!("{:.0} %", v * 100.0))
}

/// A -1..1 value in percent ("-79%").
fn signed_percent() -> Fmt {
    Arc::new(|v| format!("{:.0}%", v * 100.0))
}

fn side_text() -> Fmt {
    Arc::new(|v| {
        let p = (v * 100.0).round();
        if p == 0.0 { "Centered".into() } else if p < 0.0 { format!("{:.0}% left", -p) } else { format!("{p:.0}% right") }
    })
}

/// A 0..1 knob shown in percent.
/// Poly modulation id of "Sample start offset": per-note sample start.
pub const START_OFFSET_POLY: u32 = 0;

pub fn knob(name: &str, default: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Linear { min: 0.0, max: 1.0 }).with_value_to_string(percent())
}

/// A -1..1 knob (centre = 0) shown in percent.
pub fn bi_knob(name: &str, default: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Linear { min: -1.0, max: 1.0 }).with_value_to_string(signed_percent())
}

/// A choice list stored as an integer (the label is the value text).
fn choice(name: &str, default: i32, labels: &'static [&'static str]) -> IntParam {
    IntParam::new(name, default, IntRange::Linear { min: 0, max: labels.len() as i32 - 1 })
        .with_value_to_string(Arc::new(move |v| labels.get(v as usize).copied().unwrap_or("?").to_string()))
}

/// Envelope / LFO time knobs: the knob position v is the channel-envelope field v·65536 (minimum 100 = none, spec
/// audio/channel-envelopes.md); FL shows the position in percent.
pub fn time_field(norm: f32) -> i32 {
    ((norm.clamp(0.0, 1.0) * 65536.0).round() as i32).max(100)
}

fn field_norm(field: i32) -> f32 {
    field as f32 / 65536.0
}

pub const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// FL note naming: key 60 = C5.
pub fn note_name(key: i32) -> String {
    format!("{}{}", NOTE_NAMES[key.rem_euclid(12) as usize], key.div_euclid(12))
}

pub const DECLICK: &[&str] = &[
    "Out only (no bleeding)",
    "Transient (no bleeding)",
    "Transient (bleeding)",
    "Generic (bleeding)",
    "Smooth (bleeding)",
    "Crossfade (bleeding)",
];
/// Stretch modes in the order of the mode menu (spec §Menus: two columns with headers).
pub const STRETCH: &[&str] =
    &["Resample", "Stretch", "Stretch Pro", "e3 generic", "e3 mono", "Slice stretch", "Slice map", "Auto", "e2 generic", "e2 transient", "e2 mono", "e2 speech"];
pub const STRETCH_AUTO: i32 = 7;
pub const FILTERS: &[&str] = &["Fast LP", "LP", "BP", "HP", "BS", "LPx2", "SVF LP", "SVF LPx2"];
pub const LFO_SHAPES: &[&str] = &["Sine", "Triangle", "Pulse"];
pub const REVERB_TYPES: &[&str] = &["A", "B"];

/// Highest key (C0 = 0 .. B10 = 131).
pub const KEY_MAX: i32 = 131;

/// Time stretch multiplier of a knob position: 25 %..400 %, doubling every quarter (FL: 100 px per octave).
pub fn mul_of(v: f32) -> f64 {
    2f64.powf(4.0 * (f64::from(v) - 0.5))
}

/// Time stretch pitch in cents of a knob position (±1200).
pub fn ts_cents(v: f32) -> f64 {
    (f64::from(v) * 2400.0 - 1200.0).round()
}

/// TIME knob: pixels of drag over its whole range. The first 16 px are 16ths of a beat, then one beat per 36 px up to
/// 16 beats (spec §4).
pub const TIME_PX: f32 = 556.0;

/// TIME knob: 16ths of a beat of a knob position (0 = off).
pub fn time_steps(v: f32) -> i32 {
    let p = v.clamp(0.0, 1.0) * TIME_PX;
    if p < 15.5 {
        p.round() as i32
    } else {
        ((1.0 + ((p - 16.0).max(0.0) / 36.0).floor()) as i32 * 16).min(256)
    }
}

/// Knob position of a TIME value in 16ths of a beat (inverse of `time_steps`).
pub fn time_norm(steps: i32) -> f32 {
    if steps < 16 {
        steps.max(0) as f32 / TIME_PX
    } else {
        (16.0 + (steps / 16 - 1) as f32 * 36.0 + 0.5) / TIME_PX
    }
}

/// "beats:16ths:00" as FL writes the TIME value.
pub fn time_text(steps: i32) -> String {
    format!("{}:{:02}:00", steps / 16, steps % 16)
}

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
    /// Envelope amount (Mod X / Mod Y / Pitch; pitch: ±2400 cents at full scale).
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
        let time = |s: &str, field: i32| knob(&n(s), field_norm(field));
        EnvParams {
            on: BoolParam::new(n("envelope"), false),
            delay: time("delay", d.delay),
            att: time("attack", d.attack),
            hold: time("hold", d.hold),
            dec: time("decay", d.decay),
            sus: knob(&n("sustain"), d.sustain as f32 / 128.0),
            rel: time("release", d.release),
            att_t: bi_knob(&n("attack tension"), 0.0),
            dec_t: bi_knob(&n("decay tension"), 0.0),
            rel_t: bi_knob(&n("release tension"), rel_tension as f32 / 128.0),
            amt: bi_knob(&n("amount"), 0.0),
            tempo: BoolParam::new(n("tempo"), false),
            lfo_shape: choice(&n("LFO shape"), 0, LFO_SHAPES),
            lfo_delay: knob(&n("LFO delay"), 0.0),
            lfo_att: knob(&n("LFO attack"), field_norm(20000)),
            lfo_amt: bi_knob(&n("LFO amount"), 0.0),
            lfo_speed: knob(&n("LFO speed"), field_norm(32950)),
            lfo_tempo: BoolParam::new(n("LFO tempo"), false),
            lfo_global: BoolParam::new(n("LFO global"), false),
        }
    }

    /// The record in channel-envelope units, `None` when off. `amount_scale` = units of the amount at full scale.
    pub fn envelope(&self, amount_scale: f32) -> Option<envelope::Params> {
        let rec = os_model::Envelope {
            enabled: self.on.live(),
            delay: time_field(self.delay.live()),
            attack: time_field(self.att.live()),
            hold: time_field(self.hold.live()),
            decay: time_field(self.dec.live()),
            sustain: (self.sus.live() * 128.0).round() as i32,
            release: time_field(self.rel.live()),
            amount: (self.amt.live() * amount_scale).round() as i32,
            attack_tension: (self.att_t.live() * 128.0).round() as i32,
            decay_tension: (self.dec_t.live() * 128.0).round() as i32,
            release_tension: (self.rel_t.live() * 128.0).round() as i32,
        };
        envelope::Params::new(&rec)
    }
}

#[derive(Params)]
pub struct SamplerParams {
    /// The loaded sample's path (empty = none).
    #[persist = "sample-path"]
    pub sample_path: Arc<RwLock<String>>,
    /// Frames of the loaded sample (loop knob texts are in samples).
    pub frames: Arc<AtomicU32>,

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
    // Precomputed effects, effects page.
    #[id = "fx_boost"]
    pub fx_boost: FloatParam,
    #[id = "fx_eq"]
    pub fx_eq: FloatParam,
    #[id = "fx_rm_mix"]
    pub fx_rm_mix: FloatParam,
    #[id = "fx_rm_freq"]
    pub fx_rm_freq: FloatParam,
    #[id = "fx_cut"]
    pub fx_cut: FloatParam,
    #[id = "fx_res"]
    pub fx_res: FloatParam,
    #[id = "fx_clip"]
    pub fx_clip: BoolParam,
    #[id = "fx_reverb_type"]
    pub fx_reverb_type: IntParam,
    #[id = "fx_reverb"]
    pub fx_reverb: FloatParam,
    #[id = "fx_sdelay"]
    pub fx_sdelay: FloatParam,
    #[id = "fx_pogo"]
    pub fx_pogo: FloatParam,

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
    /// Key zone (ruler selection): lowest and highest key that play; the full range = no zone.
    #[id = "zone_lo"]
    pub zone_lo: IntParam,
    #[id = "zone_hi"]
    pub zone_hi: IntParam,
}

impl Drop for SamplerParams {
    fn drop(&mut self) {
        let from = self as *const Self as usize;
        live_forget(from, from + std::mem::size_of::<Self>());
    }
}

impl Default for SamplerParams {
    fn default() -> Self {
        let frames = Arc::new(AtomicU32::new(0));
        let samples = |f: Arc<AtomicU32>| -> Fmt {
            Arc::new(move |v| {
                let n = f.load(Ordering::Relaxed);
                format!("{}", (v * n as f32).round() as i64)
            })
        };
        SamplerParams {
            sample_path: Arc::new(RwLock::new(String::new())),
            keep_disk: BoolParam::new("Keep on disk", false),
            resample: BoolParam::new("Resample", true),
            load_regions: BoolParam::new("Load regions", true),
            load_slices: BoolParam::new("Load slice markers", false),
            declick: choice("Declicking mode", 0, DECLICK),
            // A host can start one note further into the sample (a trimmed audio clip, spec §7).
            start_offset: knob("Sample start offset", 0.0).with_poly_modulation_id(START_OFFSET_POLY),
            loop_start: FloatParam::new("Loop start position", 0.0, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_value_to_string(samples(frames.clone())),
            loop_end: FloatParam::new("Loop end position", 1.0, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_value_to_string(samples(frames.clone())),
            loop_use: BoolParam::new("Use loop points", true),
            loop_pingpong: BoolParam::new("Ping pong loop", false),
            ts_pitch: FloatParam::new("Pitch shift", 0.5, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_value_to_string(Arc::new(|v| {
                    let c = ts_cents(v);
                    if c == 0.0 { "0 cents".into() } else { format!("{c:+.0} cents") }
                })),
            ts_mul: FloatParam::new("Time stretch (multiplicator)", 0.5, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_value_to_string(Arc::new(|v| format!("{:.2}%", mul_of(v) * 100.0))),
            ts_time: FloatParam::new("Time stretch (time)", 0.0, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_value_to_string(Arc::new(|v| time_text(time_steps(v)))),
            ts_mode: choice("Stretching method", 0, STRETCH),
            fx_dc: BoolParam::new("Remove DC offset", false),
            fx_normalize: BoolParam::new("Normalize", false),
            fx_reverse: BoolParam::new("Reverse", false),
            fx_polarity: BoolParam::new("Reverse polarity", false),
            fx_fade_stereo: BoolParam::new("Fade stereo", false),
            fx_swap: BoolParam::new("Swap stereo", false),
            fx_start: knob("Sample start", 0.0).with_value_to_string(percent_sp()),
            fx_length: knob("Sample end", 1.0).with_value_to_string(percent_sp()),
            fx_in: knob("Quick fade in", 0.0),
            fx_out: knob("Quick fade out", 0.0),
            fx_crossfade: knob("Crossfade loop", 0.0),
            fx_trim: knob("Silence trimming threshold", 0.0),
            fx_boost: knob("Boost", 0.0),
            fx_eq: bi_knob("EQ", 0.0),
            fx_rm_mix: knob("Ring modulation mix", 0.0),
            fx_rm_freq: knob("Ring modulation frequency", 0.5),
            fx_cut: knob("Filter cutoff", 1.0),
            fx_res: knob("Filter resonance", 0.0),
            fx_clip: BoolParam::new("Clipping", false),
            fx_reverb_type: choice("Reverb type", 1, REVERB_TYPES),
            fx_reverb: knob("Reverb amount", 0.0),
            fx_sdelay: FloatParam::new("Stereo delay", 0.0, FloatRange::Linear { min: -1.0, max: 1.0 }).with_value_to_string(side_text()),
            fx_pogo: bi_knob("Pogo effect", 0.0),
            env_pan: EnvParams::new("Panning", 0),
            env_vol: EnvParams::new("Volume", -101),
            env_modx: EnvParams::new("Mod X", 0),
            env_mody: EnvParams::new("Mod Y", 0),
            env_pitch: EnvParams::new("Pitch", 0),
            f_cut: knob("Filter frequency (modulation X)", 1.0),
            f_res: knob("Filter bandwidth (modulation Y)", 0.0),
            f_type: choice("Filter type", 0, FILTERS),
            root: IntParam::new("Root note", 60, IntRange::Linear { min: 0, max: KEY_MAX }).with_value_to_string(Arc::new(note_name)),
            fine: IntParam::new("Fine tuning", 0, IntRange::Linear { min: -100, max: 100 }).with_value_to_string(Arc::new(|v| format!("{v} cents"))),
            main_pitch: BoolParam::new("Affected by the main pitch", true),
            add_key: BoolParam::new("Add root note to key", false),
            zone_lo: IntParam::new("Zone low key", 0, IntRange::Linear { min: 0, max: KEY_MAX }).with_value_to_string(Arc::new(note_name)).hide(),
            zone_hi: IntParam::new("Zone high key", KEY_MAX, IntRange::Linear { min: 0, max: KEY_MAX })
                .with_value_to_string(Arc::new(note_name))
                .hide(),
            frames,
        }
    }
}

impl SamplerParams {
    /// The envelope tab's five targets in sub-tab order.
    pub fn env(&self, target: usize) -> &EnvParams {
        [&self.env_pan, &self.env_vol, &self.env_modx, &self.env_mody, &self.env_pitch][target.min(4)]
    }

    /// True when the ruler selected a key zone (not the full range).
    pub fn has_zone(&self) -> bool {
        self.zone_lo.live() > 0 || self.zone_hi.live() < KEY_MAX
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
    pub crossfade: f32,
    pub trim: f32,
    pub boost: f32,
    pub eq: f32,
    pub rm_mix: f32,
    pub rm_freq: f32,
    pub cut: f32,
    pub res: f32,
    pub clip: bool,
    pub reverb_b: bool,
    pub reverb: f32,
    pub sdelay: f32,
    pub pogo: f32,
    /// Declicking mode (the precomputed edges of the sample).
    pub declick: i32,
    /// Loop points (0..1 of the length) and whether a loop plays: the crossfade needs them.
    pub loop_on: bool,
    pub loop_start: f32,
    pub loop_end: f32,
    /// Time stretching in a stretch mode (not Resample): MUL ratio, TIME target length in seconds (0 = off) and the
    /// pitch shift in cents.
    pub stretch: bool,
    pub ratio: f32,
    pub time: f32,
    pub shift: f32,
}

impl FxKey {
    /// No effect at all (a plain copy).
    pub fn neutral() -> FxKey {
        FxKey {
            dc: false,
            normalize: false,
            reverse: false,
            polarity: false,
            fade_stereo: false,
            swap: false,
            start: 0.0,
            length: 1.0,
            fade_in: 0.0,
            fade_out: 0.0,
            crossfade: 0.0,
            trim: 0.0,
            boost: 0.0,
            eq: 0.0,
            rm_mix: 0.0,
            rm_freq: 0.5,
            cut: 1.0,
            res: 0.0,
            clip: false,
            reverb_b: true,
            reverb: 0.0,
            sdelay: 0.0,
            pogo: 0.0,
            declick: 0,
            loop_on: false,
            loop_start: 0.0,
            loop_end: 1.0,
            stretch: false,
            ratio: 1.0,
            time: 0.0,
            shift: 0.0,
        }
    }

    /// Stretch ratio for a sample of `secs` seconds: TIME's target length when set, else MUL.
    pub fn ratio_for(&self, secs: f64) -> f64 {
        if self.time > 0.0 && secs > 0.0 {
            f64::from(self.time) / secs
        } else {
            f64::from(self.ratio)
        }
    }
}

impl SamplerParams {
    /// TIME's target length in seconds at `tempo` BPM (`None` when TIME is off).
    pub fn ts_seconds(&self, tempo: f64) -> Option<f64> {
        let steps = time_steps(self.ts_time.live());
        (steps > 0).then(|| f64::from(steps) / 16.0 * 60.0 / tempo.max(1.0))
    }

    /// Precomputed settings at the host's `tempo` (TIME depends on it).
    pub fn fx_key(&self, tempo: f64) -> FxKey {
        let stretch = self.ts_mode.live() != 0;
        let time = if stretch { self.ts_seconds(tempo).unwrap_or(0.0) as f32 } else { 0.0 };
        FxKey {
            dc: self.fx_dc.live(),
            normalize: self.fx_normalize.live(),
            reverse: self.fx_reverse.live(),
            polarity: self.fx_polarity.live(),
            fade_stereo: self.fx_fade_stereo.live(),
            swap: self.fx_swap.live(),
            start: self.fx_start.live(),
            length: self.fx_length.live(),
            fade_in: self.fx_in.live(),
            fade_out: self.fx_out.live(),
            crossfade: self.fx_crossfade.live(),
            trim: self.fx_trim.live(),
            boost: self.fx_boost.live(),
            eq: self.fx_eq.live(),
            rm_mix: self.fx_rm_mix.live(),
            rm_freq: self.fx_rm_freq.live(),
            cut: self.fx_cut.live(),
            res: self.fx_res.live(),
            clip: self.fx_clip.live(),
            reverb_b: self.fx_reverb_type.live() == 1,
            reverb: self.fx_reverb.live(),
            sdelay: self.fx_sdelay.live(),
            pogo: self.fx_pogo.live(),
            declick: self.declick.live(),
            loop_on: self.loop_active(),
            loop_start: self.loop_start.live(),
            loop_end: self.loop_end.live(),
            stretch,
            ratio: if stretch { mul_of(self.ts_mul.live()) as f32 } else { 1.0 },
            time,
            shift: if stretch { ts_cents(self.ts_pitch.live()) as f32 } else { 0.0 },
        }
    }

    /// A loop plays when "Use loop points" is on and the loop knobs mark a loop (OpenStudio rule, spec §Loop).
    pub fn loop_active(&self) -> bool {
        self.loop_use.live() && (self.loop_start.live() > 0.0 || self.loop_end.live() < 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_edits_reach_the_sound_without_the_host() {
        let p = Arc::new(SamplerParams::default());
        let ptr = p.f_cut.as_ptr();
        let host = p.f_cut.unmodulated_normalized_value();
        assert_eq!(p.f_cut.live(), p.f_cut.value());
        // An edit the host has not handed back: the sound uses it.
        live_set(param_addr(ptr), 0.25, host);
        assert!((p.f_cut.live() - 0.25).abs() < 1e-6);
        assert!((live_of(ptr) - 0.25).abs() < 1e-6);
        // Setting it back to the host's value leaves nothing pending.
        live_set(param_addr(ptr), host, host);
        assert_eq!(p.f_cut.live(), p.f_cut.value());
        // Booleans and integers too; a dropped instance forgets its edits.
        live_set(param_addr(p.fx_reverse.as_ptr()), 1.0, 0.0);
        assert!(p.fx_reverse.live());
        let addr = param_addr(p.fx_reverse.as_ptr());
        drop(p);
        assert!(LIVE.lock().unwrap().iter().all(|e| e.addr != addr));
    }
}
