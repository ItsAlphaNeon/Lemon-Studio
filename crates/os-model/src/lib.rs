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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipSource {
    /// 1-based pattern number.
    Pattern(u16),
    /// Channel index of an audio or automation clip.
    Channel(u16),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clip {
    pub position: u32,
    pub length: u32,
    /// 0-based playlist track.
    pub track: u16,
    pub source: ClipSource,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistTrack {
    /// 1-based.
    pub number: u32,
    pub name: Option<String>,
    pub color: Option<Rgb>,
    pub enabled: bool,
}
