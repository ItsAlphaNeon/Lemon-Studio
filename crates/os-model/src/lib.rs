//! OpenStudio project data model: channels, patterns, playlist, mixer, automation.
//! Pure data, no I/O. Populated by `os-flp` and (later) edited by the UI and engine.
//!
//! Units are the project's own: positions and lengths in ticks (see [`Project::ppq`]),
//! levels as normalized `f64`s. File encodings live in `os-flp`, not here.

/// An RGB color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Rgb { r, g, b }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Project {
    /// Version string of the program that saved the file, e.g. `26.1.7.5653`.
    pub saved_by: String,
    /// Ticks per quarter note.
    pub ppq: u16,
    /// Tempo in beats per minute.
    pub tempo: Option<f64>,
    pub info: ProjectInfo,
    /// 1-based number of the pattern selected in the UI.
    pub current_pattern: Option<u16>,
    pub channels: Vec<Channel>,
    pub patterns: Vec<Pattern>,
    pub mixer: Vec<MixerTrack>,
    pub arrangements: Vec<Arrangement>,
    /// Number (event 99 value) of the arrangement the playlist shows (spec: flp/events/playlist.md, event 100).
    pub current_arrangement: Option<u16>,
    /// Automation clip data, one per automation channel (spec: flp/events/automation.md).
    pub automations: Vec<Automation>,
}

impl Project {
    /// Index into `arrangements` of the current arrangement (the first when unset or not found).
    pub fn arrangement_index(&self) -> usize {
        self.current_arrangement.and_then(|n| self.arrangements.iter().position(|a| a.number == n)).unwrap_or(0)
    }

    /// The arrangement the playlist shows and Song mode plays.
    pub fn arrangement(&self) -> Option<&Arrangement> {
        self.arrangements.get(self.arrangement_index())
    }

    pub fn arrangement_mut(&mut self) -> Option<&mut Arrangement> {
        let i = self.arrangement_index();
        self.arrangements.get_mut(i)
    }

    pub fn automation(&self, channel: u16) -> Option<&Automation> {
        self.automations.iter().find(|a| a.channel == channel)
    }

    pub fn automation_mut(&mut self, channel: u16) -> Option<&mut Automation> {
        self.automations.iter_mut().find(|a| a.channel == channel)
    }

    pub fn pattern(&self, number: u16) -> Option<&Pattern> {
        self.patterns.iter().find(|p| p.number == number)
    }

    /// Channel by its index (rack position).
    pub fn channel(&self, index: u16) -> Option<&Channel> {
        self.channels.iter().find(|c| c.index == index)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProjectInfo {
    pub title: Option<String>,
    pub author: Option<String>,
    pub genre: Option<String>,
    /// Lines separated by `\n`.
    pub comments: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelKind {
    Sampler,
    Plugin,
    Layer,
    AudioClip,
    Automation,
    /// A type value not (yet) known.
    Other(u8),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Channel {
    /// Position in the Channel rack, 0-based.
    pub index: u16,
    pub kind: ChannelKind,
    pub name: Option<String>,
    /// Generator plugin name for [`ChannelKind::Plugin`].
    pub plugin: Option<String>,
    /// `None` = default color.
    pub color: Option<Rgb>,
    pub enabled: bool,
    /// 0.0..=1.0 (0.78125 is the default).
    pub volume: f64,
    /// -1.0 (left) ..= 1.0 (right).
    pub pan: f64,
    pub pitch_cents: i32,
    /// Target mixer track.
    pub mixer_track: u16,
    pub sample_path: Option<String>,
    /// Child channel references of a [`ChannelKind::Layer`].
    pub layer_children: Vec<u16>,
    /// The five envelope records (event 218), indexed by [`Envelope::VOLUME`] and friends.
    pub envelopes: [Envelope; 5],
}

/// One channel envelope record, values as stored (spec: audio/channel-envelopes.md). Times are knob
/// values (100 = none), `sustain` is 0..=128, tensions are -128..=128.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Envelope {
    pub enabled: bool,
    pub delay: i32,
    pub attack: i32,
    pub hold: i32,
    pub decay: i32,
    pub sustain: i32,
    pub release: i32,
    /// Pitch record: cents. Cutoff record: -128..=128.
    pub amount: i32,
    pub attack_tension: i32,
    pub decay_tension: i32,
    pub release_tension: i32,
}

impl Envelope {
    /// Record indices.
    pub const PAN: usize = 0;
    pub const VOLUME: usize = 1;
    pub const CUTOFF: usize = 2;
    pub const RESONANCE: usize = 3;
    pub const PITCH: usize = 4;
}

impl Default for Envelope {
    /// A fresh channel's record: off, with the default knob positions.
    fn default() -> Self {
        Envelope {
            enabled: false,
            delay: 100,
            attack: 20000,
            hold: 20000,
            decay: 30000,
            sustain: 50,
            release: 20000,
            amount: 0,
            attack_tension: 0,
            decay_tension: 0,
            release_tension: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Pattern {
    /// 1-based.
    pub number: u16,
    pub name: Option<String>,
    /// `None` = default color.
    pub color: Option<Rgb>,
    /// Sorted by position.
    pub notes: Vec<Note>,
    /// Channel control points drawn in the piano roll's event panel, ordered by position
    /// (spec: flp/events/patterns.md, "Control points").
    pub controls: Vec<ControlPoint>,
}

/// One point of a pattern's channel control lane (event 223 record). `kind` is
/// `channel << 16 | parameter`; only [`ControlPoint::VOLUME`], [`ControlPoint::PAN`] and
/// [`ControlPoint::PITCH`] are edited, other targets are kept as read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlPoint {
    pub position: u32,
    pub kind: u32,
    /// Volume and pan 0..=12800 (pan centre 6400); pitch in signed cents.
    pub value: i32,
}

impl ControlPoint {
    pub const VOLUME: u16 = 0;
    pub const PAN: u16 = 1;
    pub const PITCH: u16 = 4;

    pub fn new(position: u32, channel: u16, param: u16, value: i32) -> Self {
        ControlPoint { position, kind: u32::from(channel) << 16 | u32::from(param), value }
    }

    pub fn channel(&self) -> u16 {
        (self.kind >> 16) as u16
    }

    pub fn param(&self) -> u16 {
        self.kind as u16
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Note {
    pub position: u32,
    /// 0 for a plain step.
    pub length: u32,
    pub channel: u16,
    /// MIDI-style key; 60 = C5.
    pub key: u16,
    /// 120 = centered.
    pub fine_pitch: u8,
    pub release: u8,
    /// 64 = centered.
    pub pan: u8,
    pub velocity: u8,
    pub mod_x: u8,
    pub mod_y: u8,
    /// Record flags (patterns.md: 0x4000 always, 0x0008 = slide); kept as read.
    pub flags: u16,
    /// Byte 19: colour group in the low nibble, `MUTED` bit; other bits kept as read.
    pub group: u8,
    /// Unknown record bytes 14–15, kept as read.
    pub unk14: u16,
    /// Note repeat index 0..=14 (byte 17; 0 = off; spec: flp/events/patterns.md).
    pub repeat: u8,
}

impl Note {
    /// Byte-19 bit of a muted note (spec: flp/events/patterns.md, note flags).
    pub const MUTED: u8 = 0x20;

    pub fn muted(&self) -> bool {
        self.group & Self::MUTED != 0
    }

    /// Highest repeat index (spec: flp/events/patterns.md, note repeat table).
    pub const MAX_REPEAT: u8 = 14;

    /// Spacing of the repeat in ticks at `ppq`, `None` when the note doesn't repeat. The table is the
    /// interval of each repeat name at PPQ 96 (derived from the names; the audible spacing is unverified).
    pub fn repeat_ticks(&self, ppq: u16) -> Option<u32> {
        const AT_96: [u32; 15] = [0, 96, 72, 64, 48, 36, 32, 24, 18, 16, 12, 9, 8, 6, 4];
        let base = *AT_96.get(usize::from(self.repeat))?;
        (base > 0).then(|| (base * u32::from(ppq) / 96).max(1))
    }

    pub fn set_muted(&mut self, on: bool) {
        if on {
            self.group |= Self::MUTED;
        } else {
            self.group &= !Self::MUTED;
        }
    }

    /// A step-sequencer step with default parameters.
    pub fn step(channel: u16, position: u32) -> Self {
        Note {
            position,
            length: 0,
            channel,
            key: 60,
            fine_pitch: 120,
            release: 64,
            pan: 64,
            velocity: 100,
            mod_x: 128,
            mod_y: 128,
            flags: 0x4000,
            group: 0,
            unk14: 0,
            repeat: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MixerTrack {
    /// 0 = Master.
    pub index: u16,
    pub name: Option<String>,
    pub color: Option<Rgb>,
    pub enabled: bool,
    /// Effects by slot (0..10); `None` = empty slot.
    pub slots: [Option<SlotPlugin>; 10],
    /// Destination tracks this track sends to.
    pub routes: Vec<u16>,
    /// Explicit send levels `(destination, level 0..=1)`; routes without an entry send at unity
    /// (0.8). A level of 0 is a sidechain-only route: no audio reaches the destination's mix.
    pub send_levels: Vec<(u16, f64)>,
    /// 0.0..=1.0 fader position; 0.8 = unity (the default).
    pub volume: Option<f64>,
    /// -1.0 ..= 1.0.
    pub pan: Option<f64>,
    /// -1.0 ..= 1.0.
    pub stereo_separation: Option<f64>,
}

/// A plugin in a mixer slot: its name and its opaque saved state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotPlugin {
    pub name: String,
    pub state: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Arrangement {
    pub number: u16,
    pub name: Option<String>,
    /// Sorted by position.
    pub clips: Vec<Clip>,
    pub tracks: Vec<PlaylistTrack>,
    /// Time markers in file order (spec: flp/events/playlist.md "Time markers").
    pub markers: Vec<TimeMarker>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipSource {
    /// 1-based pattern number.
    Pattern(u16),
    /// Channel index of an audio or automation clip.
    Channel(u16),
}

/// Length of a stored clip record (spec: flp/events/playlist.md).
pub const CLIP_RECORD_LEN: usize = 88;

/// A playlist clip. `start` / `end` are the clip's window into its source in the source's unit: pattern ticks,
/// audio milliseconds, automation beats (spec: flp/events/playlist.md "Clip offsets"); `None` = from the
/// source's start for the clip's length.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clip {
    pub position: u32,
    pub length: u32,
    /// 0-based playlist track.
    pub track: u16,
    pub source: ClipSource,
    pub muted: bool,
    pub window: Option<(f64, f64)>,
    /// The stored record (fields not modeled are written back from here; a new clip gets FL's defaults).
    pub raw: [u8; CLIP_RECORD_LEN],
}

impl Clip {
    /// A new clip record as FL 26 writes one (spec: flp/events/playlist.md).
    pub const TEMPLATE: [u8; CLIP_RECORD_LEN] = {
        let mut r = [0u8; CLIP_RECORD_LEN];
        r[4] = 0x00;
        r[5] = 0x50;
        r[16] = 0x78;
        r[18] = 0x40;
        r[20] = 0x40;
        r[21] = 0x64;
        r[22] = 0x80;
        r[23] = 0x80;
        let mut i = 24;
        while i < 32 {
            r[i] = 0xff;
            i += 1;
        }
        r[70] = 0xf0;
        r[71] = 0x3f;
        r[76] = 0xff;
        r[77] = 0xff;
        r[78] = 0xff;
        r[79] = 0xff;
        r
    };

    pub fn new(position: u32, length: u32, track: u16, source: ClipSource) -> Clip {
        Clip { position, length, track, source, muted: false, window: None, raw: Clip::TEMPLATE }
    }

    pub fn end(&self) -> u32 {
        self.position + self.length
    }

    /// Clip id stored in the record (bytes 32..36).
    pub fn id(&self) -> u32 {
        u32::from_le_bytes([self.raw[32], self.raw[33], self.raw[34], self.raw[35]])
    }

    pub fn set_id(&mut self, id: u32) {
        self.raw[32..36].copy_from_slice(&id.to_le_bytes());
    }
}

/// A time marker (spec: flp/events/playlist.md "Time markers").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeMarker {
    pub position: u32,
    /// High byte of the stored position: 0 = plain marker, 8 = time signature, others kept as read.
    pub kind: u8,
    pub name: String,
    pub numerator: u8,
    pub denominator: u8,
}

impl TimeMarker {
    pub const SIGNATURE: u8 = 8;

    pub fn new(position: u32, name: &str) -> Self {
        TimeMarker { position, kind: 0, name: name.into(), numerator: 4, denominator: 4 }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistTrack {
    /// 1-based.
    pub number: u32,
    pub name: Option<String>,
    pub color: Option<Rgb>,
    pub enabled: bool,
}

/// The data of an automation clip channel (spec: flp/events/automation.md): its points (event 234) and the
/// target it moves (event 227).
#[derive(Debug, Clone, PartialEq)]
pub struct Automation {
    /// Channel index of the automation clip channel.
    pub channel: u16,
    /// Target id (`None` = not linked).
    pub target: Option<u32>,
    /// Points with absolute positions in beats from the clip start, sorted.
    pub points: Vec<AutoPoint>,
    /// Stored header (21 bytes incl. the count) and tail of event 234, kept as read.
    pub header: Vec<u8>,
    pub tail: Vec<u8>,
}

/// One automation point. `mode` shapes the segment that ends at this point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoPoint {
    pub beat: f64,
    /// Normalized 0..=1.
    pub value: f64,
    /// -1..=1.
    pub tension: f32,
    pub mode: AutoMode,
    /// Byte 23 of the record, kept as read.
    pub flag: u8,
}

impl AutoPoint {
    pub fn new(beat: f64, value: f64) -> Self {
        AutoPoint { beat, value, tension: 0.0, mode: AutoMode::Single, flag: 2 }
    }
}

/// Segment shapes, by stored value (spec: flp/events/automation.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoMode {
    Single,
    Double,
    Hold,
    Stairs,
    SmoothStairs,
    Pulse,
    Wave,
    Single2,
    Double2,
    HalfSine,
    Smooth,
    Single3,
    Double3,
    Other(u8),
}

impl AutoMode {
    pub fn from_byte(b: u8) -> Self {
        use AutoMode::*;
        match b {
            0 => Single,
            1 => Double,
            2 => Hold,
            3 => Stairs,
            4 => SmoothStairs,
            5 => Pulse,
            6 => Wave,
            7 => Single2,
            8 => Double2,
            9 => HalfSine,
            10 => Smooth,
            11 => Single3,
            12 => Double3,
            v => Other(v),
        }
    }

    pub fn byte(self) -> u8 {
        use AutoMode::*;
        match self {
            Single => 0,
            Double => 1,
            Hold => 2,
            Stairs => 3,
            SmoothStairs => 4,
            Pulse => 5,
            Wave => 6,
            Single2 => 7,
            Double2 => 8,
            HalfSine => 9,
            Smooth => 10,
            Single3 => 11,
            Double3 => 12,
            Other(v) => v,
        }
    }

    /// Modes whose curve the tension bends (byte 23 = 2 for these).
    pub fn has_tension(self) -> bool {
        use AutoMode::*;
        matches!(self, Single | Double | Single2 | Double2 | Single3 | Double3 | HalfSine | Stairs | SmoothStairs | Pulse | Wave)
    }

    /// Menu order (spec: ui/playlist.md §8).
    pub const MENU: [AutoMode; 13] = [
        AutoMode::Smooth,
        AutoMode::Hold,
        AutoMode::Single,
        AutoMode::Single2,
        AutoMode::Single3,
        AutoMode::Double,
        AutoMode::Double2,
        AutoMode::Double3,
        AutoMode::HalfSine,
        AutoMode::Stairs,
        AutoMode::SmoothStairs,
        AutoMode::Pulse,
        AutoMode::Wave,
    ];

    pub fn name(self) -> &'static str {
        use AutoMode::*;
        match self {
            Single => "Single curve",
            Double => "Double curve",
            Hold => "Hold",
            Stairs => "Stairs",
            SmoothStairs => "Smooth stairs",
            Pulse => "Pulse",
            Wave => "Wave",
            Single2 => "Single curve 2",
            Double2 => "Double curve 2",
            HalfSine => "Half sine",
            Smooth => "Smooth",
            Single3 => "Single curve 3",
            Double3 => "Double curve 3",
            Other(_) => "Other",
        }
    }
}

/// Automation targets (spec: flp/events/automation.md "Targets").
pub mod target {
    pub const CHANNEL_VOLUME: u16 = 0;
    pub const CHANNEL_PAN: u16 = 1;
    pub const CHANNEL_PITCH: u16 = 4;
    pub const TEMPO: u32 = 0x4000_0005;
    pub const MIXER_VOLUME: u8 = 0xC0;
    pub const MIXER_PAN: u8 = 0xC1;

    pub fn channel(ch: u16, param: u16) -> u32 {
        u32::from(ch) << 16 | u32::from(param)
    }

    pub fn mixer(track: u16, id: u8) -> u32 {
        u32::from(0x7000 + track * 64) << 16 | 0x1F00 | u32::from(id)
    }

    /// What a target id names.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Target {
        ChannelVolume(u16),
        ChannelPan(u16),
        ChannelPitch(u16),
        MixerVolume(u16),
        MixerPan(u16),
        Tempo,
        Other(u32),
    }

    pub fn decode(t: u32) -> Target {
        if t == TEMPO {
            return Target::Tempo;
        }
        let hi = (t >> 16) as u16;
        let lo = t as u16;
        if (0x7000..0x7000 + 127 * 64).contains(&hi) && (hi - 0x7000) % 64 == 0 && lo >> 8 == 0x1F {
            let track = (hi - 0x7000) / 64;
            return match lo as u8 {
                MIXER_VOLUME => Target::MixerVolume(track),
                MIXER_PAN => Target::MixerPan(track),
                _ => Target::Other(t),
            };
        }
        if hi < 0x4000 {
            return match lo {
                CHANNEL_VOLUME => Target::ChannelVolume(hi),
                CHANNEL_PAN => Target::ChannelPan(hi),
                CHANNEL_PITCH => Target::ChannelPitch(hi),
                _ => Target::Other(t),
            };
        }
        Target::Other(t)
    }

    /// Bipolar targets centre at 0.5.
    pub fn bipolar(t: u32) -> bool {
        matches!(decode(t), Target::ChannelPan(_) | Target::ChannelPitch(_) | Target::MixerPan(_))
    }
}

impl Automation {
    /// The curve's value at `beat` (clamped to the first / last point), shaped per segment mode
    /// (spec: ui/playlist.md §8, OpenStudio's approximations of FL's curves).
    pub fn value_at(&self, beat: f64) -> Option<f64> {
        let pts = &self.points;
        let first = pts.first()?;
        if beat <= first.beat {
            return Some(first.value);
        }
        let i = pts.partition_point(|p| p.beat <= beat);
        let Some(b) = pts.get(i) else { return Some(pts.last()?.value) };
        let a = pts[i - 1];
        let span = b.beat - a.beat;
        if span <= 0.0 {
            return Some(b.value);
        }
        let t = ((beat - a.beat) / span).clamp(0.0, 1.0);
        Some(a.value + (b.value - a.value) * shape(b.mode, f64::from(b.tension), t))
    }
}

/// Fraction 0..1 of the way from one point's value to the next at time fraction `t` (OS approximations).
pub fn shape(mode: AutoMode, tension: f64, t: f64) -> f64 {
    use std::f64::consts::PI;
    let tension = tension.clamp(-1.0, 1.0);
    let power = |t: f64, k: f64| {
        let e = 2f64.powf(-4.0 * k);
        t.powf(e)
    };
    let single = |t: f64, times: i32| {
        let mut v = t;
        for _ in 0..times {
            v = power(v, tension);
        }
        v
    };
    let double = |t: f64, times: i32| {
        if t < 0.5 { single(t * 2.0, times) / 2.0 } else { 1.0 - single((1.0 - t) * 2.0, times) / 2.0 }
    };
    let freq = 1.0 + (16.0 * tension.abs()).round();
    match mode {
        AutoMode::Single | AutoMode::Other(_) => single(t, 1),
        AutoMode::Single2 => single(t, 2),
        AutoMode::Single3 => single(t, 3),
        AutoMode::Double => double(t, 1),
        AutoMode::Double2 => double(t, 2),
        AutoMode::Double3 => double(t, 3),
        AutoMode::Hold => {
            if t >= 1.0 { 1.0 } else { 0.0 }
        }
        AutoMode::Stairs => {
            let n = 4.0 + (12.0 * tension.abs()).round();
            (t * n).floor() / n
        }
        AutoMode::SmoothStairs => {
            let n = 4.0 + (12.0 * tension.abs()).round();
            let k = (t * n).floor();
            let f = t * n - k;
            let ramp = ((f - 0.8) / 0.2).clamp(0.0, 1.0);
            ((k + ramp) / n).min(1.0)
        }
        AutoMode::Pulse => {
            if t >= 1.0 { 1.0 } else if (t * freq).fract() < 0.5 { 0.0 } else { 1.0 }
        }
        AutoMode::Wave => 0.5 - 0.5 * (t * freq * 2.0 * PI).cos(),
        AutoMode::HalfSine => (t * PI / 2.0).sin(),
        AutoMode::Smooth => 0.5 - 0.5 * (t * PI).cos(),
    }
}
