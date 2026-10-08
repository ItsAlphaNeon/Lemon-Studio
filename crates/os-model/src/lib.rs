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
}

impl Note {
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
    pub slots: [Option<String>; 10],
    /// Destination tracks this track sends to.
    pub routes: Vec<u16>,
    /// 0.0..=1.0 fader position; 0.8 = unity (the default).
    pub volume: Option<f64>,
    /// -1.0 ..= 1.0.
    pub pan: Option<f64>,
    /// -1.0 ..= 1.0.
    pub stereo_separation: Option<f64>,
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
