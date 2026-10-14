// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Lemon Studio project data model: channels, patterns, playlist, mixer, automation.
//! Pure data, no I/O. Populated by `os-flp` and (later) edited by the UI and engine.
//!
//! Units are the project's own: positions and lengths in ticks (see [`Project::ppq`]),
//! levels as normalized `f64`s. File encodings live in `os-flp`, not here.

pub mod curve;

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
    /// Main swing, 0..=128 (spec: flp/events/project-info.md, event 11); `None` = not in the file.
    pub swing: Option<u8>,
    /// Channel rack filter group names in file order (event 231); a channel's [`ChannelRack::group`] indexes it.
    /// FL files always end with "Unsorted", the group of channels in no user group.
    pub groups: Vec<String>,
    /// Current display filter (event 146): a group index, `None` = All.
    pub current_group: Option<u32>,
    /// Time signature (events 17 and 18): numerator, denominator. `None` = not in the file (4/4).
    pub time_signature: Option<(u16, u16)>,
    /// "Play truncated notes" (event 30; spec: audio/song-structure.md). `None` = not in the file (off).
    pub play_truncated: Option<bool>,
    /// Playlist scroll, time zoom and track height (event 242). `None` = not in the file.
    pub playlist_view: Option<PlaylistView>,
    /// Main volume 0..=16000, 12800 = unity (mixer record id 0 at place 0x4000; spec: audio/mixer.md "Main volume").
    /// `None` = not in the file (12800).
    pub main_volume: Option<u16>,
    /// Main pitch in cents, -1200..=1200 (event 80; spec: audio/voice-levels.md). `None` = not in the file (0).
    pub main_pitch: Option<i16>,
}

impl Project {
    /// Default main volume (unity).
    pub const MAIN_VOLUME_DEFAULT: u16 = 12800;
}

/// The playlist view a project stores: scroll, time zoom and track height (event 242, 28 bytes; spec:
/// flp/events/playlist.md "Playlist view"). Pixels are FL's at 100 % UI scale.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PlaylistView {
    pub raw: [u8; 28],
}

impl PlaylistView {
    /// Largest track height code: rows are `16 + 2 × h` px, `h` = 0..=132.
    pub const HEIGHT_MAX: u32 = 132;
    /// The default track height code (48 px rows).
    pub const HEIGHT_DEFAULT: u32 = 16;

    /// The template's view at `ppq`: left edge 0, a 4-beat bar 60 px wide, default track height.
    pub fn new(ppq: u16) -> Self {
        let mut v = PlaylistView::default();
        v.set_ticks_per_px(f64::from(ppq) * 4.0 / 60.0);
        v.set_height(Self::HEIGHT_DEFAULT);
        v
    }

    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        b.try_into().ok().map(|raw| PlaylistView { raw })
    }

    /// Left edge of the view, ticks.
    pub fn left(&self) -> f64 {
        f64::from_le_bytes(self.raw[4..12].try_into().unwrap())
    }

    pub fn set_left(&mut self, ticks: f64) {
        self.raw[4..12].copy_from_slice(&ticks.to_le_bytes());
    }

    /// Time zoom, ticks per pixel.
    pub fn ticks_per_px(&self) -> f64 {
        f64::from_le_bytes(self.raw[12..20].try_into().unwrap())
    }

    pub fn set_ticks_per_px(&mut self, v: f64) {
        self.raw[12..20].copy_from_slice(&v.to_le_bytes());
    }

    /// Vertical scroll, pixels.
    pub fn scroll_y(&self) -> u32 {
        u32::from_le_bytes(self.raw[20..24].try_into().unwrap())
    }

    pub fn set_scroll_y(&mut self, px: u32) {
        self.raw[20..24].copy_from_slice(&px.to_le_bytes());
    }

    /// Track height code `h` (rows are `16 + 2 × h` px).
    pub fn height(&self) -> u32 {
        u32::from_le_bytes(self.raw[24..28].try_into().unwrap())
    }

    pub fn set_height(&mut self, h: u32) {
        self.raw[24..28].copy_from_slice(&h.min(Self::HEIGHT_MAX).to_le_bytes());
    }

    /// Track row pitch in pixels.
    pub fn row_px(&self) -> u32 {
        16 + 2 * self.height().min(Self::HEIGHT_MAX)
    }
}

/// Name of the group of channels that are in no user filter group (spec: flp/events/project-info.md).
pub const UNSORTED_GROUP: &str = "Unsorted";

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

    /// A new channel order: `order[new] = old` rack position. An old position left out deletes that channel (its
    /// notes, control points, loop lengths and clips go too); a repeated one is a clone (a copy without notes that
    /// keeps the source's [`ChannelRack::origin`]). Every channel reference is renumbered: notes, control points,
    /// loop lengths, automation clips and their channel targets (a deleted target leaves the clip unlinked),
    /// playlist clips, layer children (spec: flp/events/channels.md "Deleting, cloning and moving channels").
    pub fn reorder_channels(&mut self, order: &[usize]) {
        let n = self.channels.len();
        let mut map: Vec<Option<u16>> = vec![None; n];
        for (new, &old) in order.iter().enumerate() {
            if old < n && map[old].is_none() {
                map[old] = Some(new as u16);
            }
        }
        let m = |c: u16| map.get(usize::from(c)).copied().flatten();
        let old = std::mem::take(&mut self.channels);
        self.channels = order
            .iter()
            .filter_map(|&o| old.get(o).cloned())
            .enumerate()
            .map(|(new, mut c)| {
                c.index = new as u16;
                c.layer_children = c.layer_children.iter().filter_map(|&k| m(k)).collect();
                c
            })
            .collect();
        for p in &mut self.patterns {
            p.notes.retain_mut(|x| m(x.channel).map(|c| x.channel = c).is_some());
            // Only channel parameters (0 volume, 1 pan, 4 pitch) hold a channel; other targets stay as read.
            p.controls.retain_mut(|x| {
                !matches!(x.param(), 0 | 1 | 4) || m(x.channel()).map(|c| *x = ControlPoint::new(x.position, c, x.param(), x.value)).is_some()
            });
            p.loops.retain_mut(|l| m(l.0).map(|c| l.0 = c).is_some());
        }
        // A clone of an automation channel copies its points (they live in the channel block), not its link.
        let clones: Vec<Automation> = order
            .iter()
            .enumerate()
            .filter(|&(new, &o)| map.get(o).copied().flatten() != Some(new as u16))
            .filter_map(|(new, &o)| {
                let a = self.automations.iter().find(|a| usize::from(a.channel) == o)?;
                Some(Automation { channel: new as u16, targets: Vec::new(), ..a.clone() })
            })
            .collect();
        self.automations.retain_mut(|a| {
            let Some(c) = m(a.channel) else { return false };
            a.channel = c;
            // Channel targets follow their channel; a deleted channel's targets go (the file loses those links).
            a.targets.retain_mut(|t| {
                if *t >> 16 >= 0x4000 {
                    return true;
                }
                m((*t >> 16) as u16).map(|c| *t = target::channel(c, *t as u16)).is_some()
            });
            true
        });
        self.automations.extend(clones);
        self.automations.sort_by_key(|a| a.channel);
        for arr in &mut self.arrangements {
            arr.clips.retain_mut(|c| match c.source {
                ClipSource::Channel(ch) => m(ch)
                    .map(|n| {
                        c.source = ClipSource::Channel(n);
                        // The stored record holds the item index too (bytes 6..8, spec: flp/events/playlist.md).
                        c.raw[6..8].copy_from_slice(&n.to_le_bytes());
                    })
                    .is_some(),
                ClipSource::Pattern(_) => true,
            });
        }
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

impl ChannelKind {
    /// Sampler and audio-clip channels: both are sampler channels with a sample (an audio clip channel is a sampler
    /// with audio-clip defaults; spec: plugins/lemon-sampler.md §7).
    pub fn is_sampler(self) -> bool {
        matches!(self, ChannelKind::Sampler | ChannelKind::AudioClip)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Channel {
    /// Position in the Channel rack, 0-based.
    pub index: u16,
    pub kind: ChannelKind,
    pub name: Option<String>,
    /// Generator plugin name for [`ChannelKind::Plugin`].
    pub plugin: Option<String>,
    /// The plugin's saved state as the project file stores it (event 213), for plugin channels. `None` = not known
    /// (the file's state is kept as it is). spec: flp/events/plugins.md
    pub plugin_state: Option<Vec<u8>>,
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
    /// Channel rack state saved with the channel.
    pub rack: ChannelRack,
    /// The plugin window's Misc tab settings (polyphony, cut groups, arpeggiator, echo, root note...).
    pub misc: ChannelMisc,
    /// Sound settings the engine reads that have no other place in the model (filter, sampler flags, declick,
    /// stretch).
    pub sound: ChannelSound,
}

/// A channel's filter, sampler and declick settings as stored (spec: flp/engine-events.md "A channel",
/// audio/channel-filter.md, audio/sampler.md, audio/declick.md, audio/audio-clips.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelSound {
    /// Filter cutoff (mod X) and resonance (mod Y), 0..=256; filter type 0..=7 (event 219, ints 3..5).
    pub cutoff: i32,
    pub resonance: i32,
    pub filter_type: i32,
    /// Sampler flags (event 143): `RESAMPLE`, `LOAD_REGIONS`, `LOAD_SLICES`, `USE_LOOP`, `KEEP_ON_DISK`. `None` = not
    /// in the file.
    pub sampler_flags: Option<u32>,
    /// Declick mode 0..=6 (event 215, byte 83; spec: audio/declick.md); 7 and up are invalid in FL.
    pub declick: u8,
    /// Stretch settings as stored (event 215, ints 24..27): time, pitch, multiplier, mode. The time is a single in
    /// 1/192 beat (FL 24.2 on), kept as its bits.
    pub stretch: [u32; 4],
    /// The sample's start and length, loop options and precomputed effects (saving writes them for sampler
    /// channels).
    pub fx: SampleFx,
    /// Loop start and end in sample frames (event 251); -1 / `i64::MAX` = the sample's own ends. `None` = not in the
    /// file (the whole sample).
    pub loop_points: Option<[i64; 2]>,
    /// Sample start offset, a share of the sample (event 215, double at byte 144), kept as its bits.
    pub start_offset_bits: u64,
}

impl ChannelSound {
    /// Sampler flags (event 143) values.
    pub const RESAMPLE: u32 = 1;
    pub const LOAD_REGIONS: u32 = 2;
    pub const LOAD_SLICES: u32 = 4;
    pub const USE_LOOP: u32 = 8;
    pub const KEEP_ON_DISK: u32 = 0x100;
    /// Flags of a channel without event 143 (FL's new sampler channel).
    pub const DEFAULT_FLAGS: u32 = Self::LOAD_REGIONS | Self::USE_LOOP;

    /// The sample start offset (share of the sample).
    pub fn start_offset(&self) -> f64 {
        f64::from_bits(self.start_offset_bits)
    }
}

/// A sample channel's start, length, loop options and precomputed effects as stored, in the file's units (spec:
/// audio/precomputed-effects.md "Settings"). Saving writes it back for sampler channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleFx {
    /// Event 70: `FADE_STEREO`, `REVERSE`, `CLIP`, `SWAP`.
    pub flags: u16,
    /// Event 74, 0..256.
    pub boost: u16,
    /// Events 75 and 76, 0..1024.
    pub fade_out: u16,
    pub fade_in: u16,
    /// Event 85, 0..4096, centre 2048 (above: the left side is late).
    pub stereo_delay: u16,
    /// Event 86, 0..512, centre 256.
    pub pogo: u16,
    /// Event 69: the one-pole feedback, 0..256, centre 128.
    pub feedback: u16,
    /// Events 71 and 83: the low-pass, cutoff 1024 = open, resonance 0..640 (a larger stored value plays as 640).
    pub cutoff: u16,
    pub resonance: u16,
    /// Event 131: ring modulation mix (0..256) and frequency word.
    pub ring_mix: u16,
    pub ring_freq: u16,
    /// Event 139: reverb amount (0..256, 0 = off) and type (0 or 1).
    pub reverb: u16,
    pub reverb_type: u16,
    /// Event 20: a ping-pong loop.
    pub ping_pong: bool,
    /// Event 215 bytes 9, 80, 81.
    pub remove_dc: bool,
    pub normalize: bool,
    pub polarity: bool,
    /// Event 215 bytes 84..87: the crossfade in 1/2048 of the loop's length.
    pub crossfade: u32,
    /// Event 215 bytes 88..91: the trim threshold in 1/32768 of full scale, 0 = off.
    pub trim: u32,
    /// Event 215 bytes 128..135 and 136..143 (doubles, kept as their bits): the start as a fraction of the sample, the
    /// length as a fraction of what follows the start.
    pub start_bits: u64,
    pub length_bits: u64,
    /// Event 215 byte 156: the loop points follow a reversal.
    pub loop_follows_reverse: bool,
}

impl SampleFx {
    pub const FADE_STEREO: u16 = 1;
    pub const REVERSE: u16 = 2;
    pub const CLIP: u16 = 4;
    pub const SWAP: u16 = 256;

    /// The start, a fraction of the sample.
    pub fn start(&self) -> f64 {
        f64::from_bits(self.start_bits)
    }

    /// The length, a fraction of what follows the start.
    pub fn length(&self) -> f64 {
        f64::from_bits(self.length_bits)
    }
}

impl Default for SampleFx {
    /// What a new channel stores (spec: audio/precomputed-effects.md "Settings", the defaults).
    fn default() -> Self {
        SampleFx {
            flags: 0,
            boost: 0,
            fade_out: 0,
            fade_in: 0,
            stereo_delay: 2048,
            pogo: 256,
            feedback: 128,
            cutoff: 1024,
            resonance: 0,
            ring_mix: 0,
            ring_freq: 128,
            reverb: 0,
            reverb_type: 1,
            ping_pong: false,
            remove_dc: false,
            normalize: false,
            polarity: false,
            crossfade: 0,
            trim: 0,
            start_bits: 0f64.to_bits(),
            length_bits: 1f64.to_bits(),
            loop_follows_reverse: false,
        }
    }
}

/// Stretch methods as event 215 int 27 stores them (spec: audio/time-stretch.md "Fields").
pub mod stretch_mode {
    /// Plays faster or slower, pitch and length together.
    pub const RESAMPLE: i32 = 0;
    /// The realtime stretchers: the note and the channel pitch move the pitch, not the length.
    pub const STRETCH: i32 = -1;
    pub const STRETCH_PRO: i32 = -2;
    /// The stretchers that stretch the sample once before it plays.
    pub const E3_GENERIC: i32 = 1;
    pub const E3_MONO: i32 = 2;
    pub const SLICE_STRETCH: i32 = 3;
    pub const SLICE_MAP: i32 = 4;
    /// Plain playback when there is nothing to do, else e3 generic.
    pub const AUTO: i32 = 5;
    pub const E2_GENERIC: i32 = 6;
    pub const E2_TRANSIENT: i32 = 7;
    pub const E2_MONO: i32 = 8;
    pub const E2_SPEECH: i32 = 9;

    /// A realtime method (the voice stretches while it plays).
    pub fn realtime(mode: i32) -> bool {
        matches!(mode, STRETCH | STRETCH_PRO)
    }
}

/// How a sample channel's stretch settings play (spec: audio/time-stretch.md "How each method plays").
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Stretching {
    /// The sample plays as it is.
    None,
    /// Resample: the sample plays `speed` times as fast, pitch and length together.
    Resample { speed: f64 },
    /// Stretched once before it plays to `factor` times its length, its pitch moved by `cents`; notes then play the
    /// result like any sample (the key and the channel pitch resample it).
    Whole { factor: f64, cents: f64 },
    /// Stretched while it plays to `factor` times its length; the stretch's `cents`, the key and the channel pitch move
    /// the pitch, never the length.
    Realtime { factor: f64, cents: f64 },
}

impl ChannelSound {
    /// The stretch multiplier `2^(stored / 10000)` (event 215 int 26), 1.0 when none; at single precision, as FL
    /// keeps it.
    pub fn stretch_multiplier(&self) -> f64 {
        f64::from(2f64.powf(f64::from(self.stretch[2] as i32) / 10000.0) as f32)
    }

    /// The stretch time in beats the sample is fitted to (event 215 int 24, a single in 1/192 beat), `None` when
    /// not set.
    pub fn stretch_time(&self) -> Option<f64> {
        let t = f32::from_bits(self.stretch[0]);
        (t.is_finite() && t >= 1.0).then(|| f64::from(t) / 192.0)
    }

    /// The stretch's own pitch shift in cents (event 215 int 25).
    pub fn stretch_cents(&self) -> f64 {
        f64::from(self.stretch[1] as i32)
    }

    /// The stretch method (event 215 int 27, `stretch_mode`).
    pub fn stretch_mode(&self) -> i32 {
        self.stretch[3] as i32
    }

    /// Stretches by resampling with multiplier `m` and no fitted time (the clip plays `m` times as long).
    pub fn set_stretch_multiplier(&mut self, m: f64) {
        self.stretch[0] = 0;
        self.stretch[2] = (m.log2() * 10000.0).round() as i32 as u32;
        self.stretch[3] = 0;
    }

    /// Fits the sample to `beats` with the multiplier back at 1, the method and pitch kept: what resizing an audio
    /// clip in the playlist's stretch mode does (spec: ui/playlist.md §9b).
    pub fn set_stretch_time(&mut self, beats: f64) {
        self.stretch[0] = ((beats * 192.0) as f32).max(1.0).to_bits();
        self.stretch[2] = 0;
    }

    /// How many times its own length a sample of `duration` seconds lasts once stretched at `bpm`, in any method:
    /// the stretch time (or the sample's length when none) times the multiplier.
    pub fn stretch_factor(&self, duration: f64, bpm: f64) -> f64 {
        let m = self.stretch_multiplier();
        match self.stretch_time() {
            Some(beats) if duration > 0.0 => beats * 60.0 / bpm.max(1.0) * m / duration,
            _ => m,
        }
    }

    /// Whether the channel's pitch still moves the sample: not when a stretch time fits it, except in the realtime
    /// methods (spec: audio/time-stretch.md "Channel pitch").
    pub fn channel_pitch_applies(&self) -> bool {
        self.stretch_time().is_none() || stretch_mode::realtime(self.stretch_mode())
    }

    /// How a sample of `duration` seconds plays at `bpm` with these stretch settings.
    pub fn stretching(&self, duration: f64, bpm: f64) -> Stretching {
        if self.stretch == [0; 4] {
            return Stretching::None;
        }
        let factor = self.stretch_factor(duration, bpm);
        let cents = self.stretch_cents();
        let idle = (factor - 1.0).abs() < 1e-4 && cents == 0.0;
        match self.stretch_mode() {
            stretch_mode::RESAMPLE if (factor - 1.0).abs() < 1e-9 => Stretching::None,
            stretch_mode::RESAMPLE => Stretching::Resample { speed: 1.0 / factor },
            m if stretch_mode::realtime(m) => Stretching::Realtime { factor, cents },
            // Auto, the methods that stretch the sample first, and any value not known: nothing to do is plain playback.
            _ if idle => Stretching::None,
            _ => Stretching::Whole { factor, cents },
        }
    }
}

impl Default for ChannelSound {
    /// A new channel: filter open, no flags read, declick mode 0, not stretched.
    fn default() -> Self {
        ChannelSound {
            cutoff: 256,
            resonance: 0,
            filter_type: 0,
            sampler_flags: None,
            declick: 0,
            stretch: [0; 4],
            fx: SampleFx::default(),
            loop_points: None,
            start_offset_bits: 0,
        }
    }
}

/// A channel's Misc tab settings (spec: ui/plugin-window.md "Misc tab"; storage: flp/events/channels.md "Misc
/// settings"). Knobs are 0.0..=1.0; bipolar knobs (levels, tracking, echo pan / mod / pitch) are -1.0..=1.0 with 0
/// = centre.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChannelMisc {
    /// Levels adjustment: panning, volume, Mod X, Mod Y (bipolar).
    pub levels: [f32; 4],
    /// Maximum polyphony; 0 = unlimited.
    pub max_poly: u32,
    /// Slide time knob: the stored slide time / 1024 (spec: audio/note-rules.md).
    pub slide: f32,
    /// Portamento: every new note slides from the previous one.
    pub porta: bool,
    /// Mono: one voice at a time.
    pub mono: bool,
    /// Note gate, time shift and swing knobs (gate 1.0 = off).
    pub gate: f32,
    pub shift: f32,
    pub swing: f32,
    /// Full portamento.
    pub full_porta: bool,
    /// Velocity tracking (0) and key tracking (1).
    pub tracking: [Tracking; 2],
    /// Cut group this channel cuts, and the group that cuts it; 0 = none.
    pub cut: u32,
    pub cut_by: u32,
    pub arp: Arpeggio,
    pub echo: Echo,
    /// Root note (MIDI key, 60 = C5).
    pub root: u8,
    /// Follows the main (project) pitch.
    pub main_pitch: bool,
    /// Adds the root note to played keys.
    pub add_key: bool,
    /// Fine tuning, cents (-100..=100).
    pub fine: i32,
    /// Key zone: lowest and highest key that play (event 215 ints at bytes 68 and 72); 0 and 256 = no zone.
    pub key_range: [i32; 2],
    /// How a layer channel hands its notes to its children (stored with every channel; only a layer uses it).
    pub layer: LayerSettings,
}

/// A layer channel's settings besides its children (spec: audio/note-rules.md "Settings", "Layer channels").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LayerSettings {
    /// Event 144: [`LayerSettings::RANDOM`], [`LayerSettings::CROSSFADE`], [`LayerSettings::IN_TURN`].
    pub flags: u32,
    /// Crossfade position, 0..=256 (event 215, int at byte 76).
    pub crossfade: i32,
}

impl LayerSettings {
    /// One child per note, drawn at random.
    pub const RANDOM: u32 = 1;
    /// The children's volumes follow the crossfade position.
    pub const CROSSFADE: u32 = 2;
    /// One child per note, in turn.
    pub const IN_TURN: u32 = 4;
}

impl Default for ChannelMisc {
    /// A new channel's settings.
    fn default() -> Self {
        ChannelMisc {
            levels: [0.0; 4],
            max_poly: 0,
            slide: 500.0 / 1024.0,
            porta: false,
            mono: false,
            gate: 1.0,
            shift: 0.0,
            swing: 1.0,
            full_porta: true,
            tracking: [Tracking::default(); 2],
            cut: 0,
            cut_by: 0,
            arp: Arpeggio::default(),
            echo: Echo::default(),
            root: 60,
            main_pitch: true,
            add_key: false,
            fine: 0,
            key_range: [0, 256],
            layer: LayerSettings::default(),
        }
    }
}

/// One tracking page of the Misc tab (velocity or key).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tracking {
    /// Middle point (velocity or MIDI key).
    pub mid: u8,
    /// To panning, Mod X, Mod Y (bipolar).
    pub pan: f32,
    pub mod_x: f32,
    pub mod_y: f32,
}

impl Default for Tracking {
    fn default() -> Self {
        Tracking { mid: 60, pan: 0.0, mod_x: 0.0, mod_y: 0.0 }
    }
}

/// The Misc tab's arpeggiator.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Arpeggio {
    /// 0 off, 1 up, 2 down, 3 up-down sticky, 4 up-down bounce, 5 random.
    pub direction: u8,
    /// Time and gate knobs.
    pub time: f32,
    pub gate: f32,
    /// Range in octaves (1..).
    pub range: u32,
    /// Note repeat; 0 = none.
    pub repeat: u32,
    pub slide: bool,
    /// Chord list index (0 = auto with sustain).
    pub chord: u32,
}

impl Default for Arpeggio {
    fn default() -> Self {
        Arpeggio { direction: 0, time: 0.58, gate: 1.0, range: 1, repeat: 0, slide: false, chord: 0 }
    }
}

/// The Misc tab's echo delay.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Echo {
    /// Feedback and time knobs.
    pub feed: f32,
    pub time: f32,
    /// Panning, Mod X, Mod Y and pitch of each echo (bipolar).
    pub pan: f32,
    pub mod_x: f32,
    pub mod_y: f32,
    pub pitch: f32,
    /// Number of echoes (1..).
    pub echoes: u32,
    pub ping_pong: bool,
    pub fat: bool,
}

impl Default for Echo {
    fn default() -> Self {
        Echo { feed: 0.0, time: 0.45, pan: 0.0, mod_x: 0.0, mod_y: 0.0, pitch: 0.0, echoes: 4, ping_pong: false, fat: false }
    }
}

/// Channel rack settings of one channel (spec: flp/events/channels.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ChannelRack {
    /// Index of the file block this channel was loaded from (`None` = created in Lemon Studio). A clone keeps its
    /// source's origin, so the writer copies the source block (spec: channels.md "Deleting, cloning and moving").
    pub origin: Option<u16>,
    /// Lock active/mute state (event 32).
    pub locked: bool,
    /// Zipped row (event 15).
    pub zipped: bool,
    /// Filter group index into [`Project::groups`] (event 145).
    pub group: u32,
    /// The colour event 128 holds, whether or not it is a custom colour: the playlist draws audio and automation
    /// clips with it (spec: ui/playlist.md §2, "Channel-colour rule"). Display only; saving writes `Channel::color`.
    pub stored_color: Option<Rgb>,
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
    /// Flags (int 0): 1 = envelope times follow the tempo, 2 = LFO times follow the tempo, 4 = the envelope's
    /// value is 1 while it is off (spec: audio/envelopes-and-lfos.md).
    pub flags: i32,
    /// The record's LFO (ints 9..13): delay, attack and speed are time knobs, amount -128..=128, shape 0 sine /
    /// 1 triangle / 2 square.
    pub lfo_delay: i32,
    pub lfo_attack: i32,
    pub lfo_amount: i32,
    pub lfo_speed: i32,
    pub lfo_shape: i32,
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
            flags: 0,
            lfo_delay: 100,
            lfo_attack: 20000,
            lfo_amount: 0,
            lfo_speed: 32950,
            lfo_shape: 0,
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
    /// Length set in the channel rack's pattern length box, in ticks; `None` = Auto (whole bars past the last
    /// note). Stored as metadata event 164 plus a "Length" marker (spec: flp/events/patterns.md).
    pub length: Option<u32>,
    /// Per-channel loop lengths `(channel, steps)` in channel order (events 160/161 after the notes);
    /// steps [`Pattern::LOOP_FOLLOW`] = a channel that only follows "Loop all channels".
    pub loops: Vec<(u16, u32)>,
    /// Loop mode off with loop lengths kept: the loop type value (metadata events 26 = 1 and 162; 3 = loop step
    /// channels, 2 = loop all channels). `None` = loop mode on (or no loops).
    pub loop_off: Option<u32>,
    /// The pattern's own time markers (piano-roll Alt+T markers, time signatures, kinds kept as read), sorted by
    /// position; the "Length" marker is not among them (it follows `length`). Metadata marker groups (spec:
    /// flp/events/patterns.md, metadata block).
    pub markers: Vec<TimeMarker>,
}

impl Pattern {
    /// Stored loop length of channels that follow "Loop all channels" (0xFFFFFFFD).
    pub const LOOP_FOLLOW: u32 = 0xFFFF_FFFD;
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

    /// Flag of a slide note (spec: flp/events/patterns.md, note flags).
    pub const SLIDE: u16 = 0x0008;

    /// A slide note: it makes no sound and glides the channel's sounding voices (spec: audio/song-structure.md
    /// "Slide notes").
    pub fn slide(&self) -> bool {
        self.flags & Self::SLIDE != 0
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
    /// -1.0 ..= 1.0 (stored / 64): above 0 merges toward mono (fully at 1), below 0 widens (spec: audio/mixer.md).
    pub stereo_separation: Option<f64>,
    /// Reverse polarity and swap left / right buttons (event 236 flags 0x01 / 0x02; spec: audio/mixer.md, ui/mixer.md §4).
    pub polarity: bool,
    pub swap: bool,
    /// The track's "effects enabled" switch (event 236 flag 0x04): when off no slot runs.
    pub slots_enabled: bool,
    /// Per slot: enabled (the slot LED) and mix level 0..=1 (spec: ui/mixer.md §7).
    pub slot_on: [bool; 10],
    pub slot_mix: [f64; 10],
    /// The three-band equaliser as stored, per band `[gain, frequency, width]`: gain -1800..=1800 in 0.01 dB,
    /// frequency and width 0..=65536 (spec: audio/mixer.md "Equaliser"). Band 1 low shelf, 2 peaking, 3 high shelf.
    pub eq: [[i32; 3]; 3],
}

/// A new track's equaliser: flat, at the default frequencies and widths.
pub const EQ_DEFAULT: [[i32; 3]; 3] = [[0, 5777, 17500], [0, 33145, 17500], [0, 55825, 17500]];

impl MixerTrack {
    /// A default track: unnamed, enabled, unity volume, centred, empty slots, routed to the master (except the master).
    pub fn new(index: u16) -> Self {
        MixerTrack {
            index,
            name: None,
            color: None,
            enabled: true,
            slots: Default::default(),
            routes: if index == 0 { Vec::new() } else { vec![0] },
            send_levels: Vec::new(),
            volume: Some(0.8),
            pan: Some(0.0),
            stereo_separation: Some(0.0),
            polarity: false,
            swap: false,
            slots_enabled: true,
            slot_on: [true; 10],
            slot_mix: [1.0; 10],
            eq: EQ_DEFAULT,
        }
    }
}

/// A plugin in a mixer slot: its name and its opaque saved state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotPlugin {
    pub name: String,
    pub state: Vec<u8>,
}

/// A third-party plugin (CLAP, VST3, VST2) as a project file names it: its format, identity and saved state.
/// spec: flp/events/plugins.md §4
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PluginRef {
    /// `clap`, `vst3` or `vst2`.
    pub format: String,
    /// VST3: the class id as 32 uppercase hex digits; VST2: the unique id as 8 hex digits; CLAP: empty when the file
    /// does not store it (the plugin is then found by path and name).
    pub id: String,
    pub name: String,
    pub vendor: String,
    /// The plugin file.
    pub path: String,
    /// The plugin's own saved state.
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

    /// Clip group stored in the record (bytes 14..16, 0 = none; spec: flp/events/playlist.md).
    pub fn group(&self) -> u16 {
        u16::from_le_bytes([self.raw[14], self.raw[15]])
    }

    pub fn set_group(&mut self, group: u16) {
        self.raw[14..16].copy_from_slice(&group.to_le_bytes());
    }

    fn f32_at(&self, o: usize) -> f32 {
        f32::from_le_bytes([self.raw[o], self.raw[o + 1], self.raw[o + 2], self.raw[o + 3]])
    }

    fn set_f32(&mut self, o: usize, v: f32) {
        self.raw[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }

    /// An audio clip's fade-in: (milliseconds, tension) when it is on (bytes 36..44, flag bit 0 of byte 56; spec:
    /// flp/events/playlist.md).
    pub fn fade_in(&self) -> Option<(f32, f32)> {
        let ms = self.f32_at(36);
        (self.raw[56] & 1 != 0 && ms.is_finite() && ms > 0.0).then(|| (ms, self.f32_at(40)))
    }

    /// An audio clip's fade-out: (milliseconds, tension) when it is on (bytes 44..52, flag bit 1 of byte 56).
    pub fn fade_out(&self) -> Option<(f32, f32)> {
        let ms = self.f32_at(44);
        (self.raw[56] & 2 != 0 && ms.is_finite() && ms > 0.0).then(|| (ms, self.f32_at(48)))
    }

    /// Sets (or with `None` / a length of 0 removes) the fade-in.
    pub fn set_fade_in(&mut self, f: Option<(f32, f32)>) {
        let (ms, tension) = f.filter(|f| f.0 > 0.0).unwrap_or((0.0, self.f32_at(40)));
        self.set_f32(36, ms);
        self.set_f32(40, tension);
        self.raw[56] = if ms > 0.0 { self.raw[56] | 1 } else { self.raw[56] & !1 };
    }

    /// Sets (or removes) the fade-out.
    pub fn set_fade_out(&mut self, f: Option<(f32, f32)>) {
        let (ms, tension) = f.filter(|f| f.0 > 0.0).unwrap_or((0.0, self.f32_at(48)));
        self.set_f32(44, ms);
        self.set_f32(48, tension);
        self.raw[56] = if ms > 0.0 { self.raw[56] | 2 } else { self.raw[56] & !2 };
    }

    /// The clip's gain (bytes 52..56): 1.0 when the record holds no usable value.
    pub fn gain(&self) -> f32 {
        let g = self.f32_at(52);
        if g.is_finite() && g > 0.0 { g } else { 1.0 }
    }

    pub fn set_gain(&mut self, g: f32) {
        self.set_f32(52, g);
    }
}

/// The gain at `x` (0..1) through an audio clip's fade-in with tension `tension`; a fade-out is `fade_curve(1 - x)`
/// (spec: audio/audio-clips.md "Fades and clip gain": a raised cosine at tension 0, measured; the tension warp
/// `x + k x (1 - x)`, `k = t / (|t| + 4.17)`, is a fit).
pub fn fade_curve(x: f64, tension: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    let k = tension / (tension.abs() + 4.17);
    let w = x + k * x * (1.0 - x);
    0.5 - 0.5 * (std::f64::consts::PI * w).cos()
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
    /// Grouped with the track above: a member of the group led by the nearest ungrouped track above it (event 238
    /// byte 46; spec: flp/events/playlist.md, ui/playlist.md §6a).
    pub grouped: bool,
    /// On a group track: its group is collapsed (byte 61 = 0).
    pub collapsed: bool,
    /// Byte 47: FL draws a lock on such a group (meaning not measured; kept and drawn).
    pub locked: bool,
}

impl PlaylistTrack {
    /// Track `number` as a new project has it: unnamed, default colour, on, not grouped.
    pub fn new(number: u32) -> Self {
        PlaylistTrack { number, name: None, color: None, enabled: true, grouped: false, collapsed: false, locked: false }
    }
}

/// The data of an automation clip channel (spec: flp/events/automation.md): its points (event 234) and the
/// targets it moves (one event 227 each).
#[derive(Debug, Clone, PartialEq)]
pub struct Automation {
    /// Channel index of the automation clip channel.
    pub channel: u16,
    /// Target ids in file order (empty = not linked). A clip drives every one of them (spec: audio/automation.md
    /// "The link"); names and value texts follow the first.
    pub targets: Vec<u32>,
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
    /// The channel's filter knobs and its level adjustments (event 229: pan offset, volume multiplier, mod X and mod
    /// Y offsets; spec: audio/voice-levels.md "Values from automation").
    pub const CHANNEL_CUTOFF: u16 = 2;
    pub const CHANNEL_RESONANCE: u16 = 3;
    pub const CHANNEL_PAN_OFFSET: u16 = 0x10;
    pub const CHANNEL_VOLUME_MULT: u16 = 0x11;
    pub const CHANNEL_MOD_X: u16 = 0x13;
    pub const CHANNEL_MOD_Y: u16 = 0x14;
    pub const TEMPO: u32 = 0x4000_0005;
    pub const MIXER_VOLUME: u8 = 0xC0;
    pub const MIXER_PAN: u8 = 0xC1;
    pub const MIXER_SEPARATION: u8 = 0xC2;
    /// Equaliser band `b` (0..3): gain `EQ_GAIN + b`, frequency `EQ_FREQ + b`, width `EQ_WIDTH + b`.
    pub const EQ_GAIN: u8 = 0xD0;
    pub const EQ_FREQ: u8 = 0xD8;
    pub const EQ_WIDTH: u8 = 0xE0;
    /// A slot's switch and mix level, at the slot's place.
    pub const SLOT_ON: u8 = 0;
    pub const SLOT_MIX: u8 = 1;
    /// Send levels from FL 24.2 on: `0x2000 + destination` at the sending track's place.
    pub const SEND: u16 = 0x2000;

    pub fn channel(ch: u16, param: u16) -> u32 {
        u32::from(ch) << 16 | u32::from(param)
    }

    pub fn mixer(track: u16, id: u8) -> u32 {
        u32::from(0x7000 + track * 64) << 16 | 0x1F00 | u32::from(id)
    }

    /// Setting `id` ([`SLOT_ON`], [`SLOT_MIX`]) of mixer track `track`'s slot `slot` (0..9).
    pub fn slot(track: u16, slot: u8, id: u8) -> u32 {
        u32::from(0x7000 + track * 64 + u16::from(slot)) << 16 | 0x1F00 | u32::from(id)
    }

    /// The level of track `track`'s send to track `dest`.
    pub fn send(track: u16, dest: u16) -> u32 {
        u32::from(0x7000 + track * 64) << 16 | u32::from(SEND + dest)
    }

    /// Plugin parameters are `0x8000 + k`, k = the parameter's index in the plugin's list
    /// (spec: plugins/plugin-parameters.md §3).
    pub const PLUGIN_PARAM: u16 = 0x8000;

    /// Parameter `k` of channel `ch`'s plugin.
    pub fn channel_param(ch: u16, k: u16) -> u32 {
        channel(ch, PLUGIN_PARAM | (k & 0x7FFF))
    }

    /// Parameter `k` of the plugin in mixer track `track`'s slot `slot` (0..9).
    pub fn slot_param(track: u16, slot: u8, k: u16) -> u32 {
        u32::from(0x7000 + track * 64 + u16::from(slot)) << 16 | u32::from(PLUGIN_PARAM | (k & 0x7FFF))
    }

    /// What a target id names.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Target {
        ChannelVolume(u16),
        ChannelPan(u16),
        ChannelPitch(u16),
        /// Channel, one of its filter knobs or level adjustments ([`CHANNEL_CUTOFF`], [`CHANNEL_RESONANCE`],
        /// [`CHANNEL_PAN_OFFSET`], [`CHANNEL_VOLUME_MULT`], [`CHANNEL_MOD_X`], [`CHANNEL_MOD_Y`]).
        ChannelLevel(u16, u16),
        MixerVolume(u16),
        MixerPan(u16),
        MixerSeparation(u16),
        /// Mixer track, equaliser band (0..3), setting (0 gain, 1 frequency, 2 width).
        MixerEq(u16, u8, u8),
        /// Mixer track, destination track: the send level.
        MixerSend(u16, u16),
        /// Mixer track, slot: the slot's switch / its mix level.
        SlotOn(u16, u8),
        SlotMix(u16, u8),
        Tempo,
        /// Channel, parameter index of its plugin.
        ChannelParam(u16, u16),
        /// Mixer track, slot, parameter index of the slot's plugin.
        SlotParam(u16, u8, u16),
        Other(u32),
    }

    pub fn decode(t: u32) -> Target {
        if t == TEMPO {
            return Target::Tempo;
        }
        let hi = (t >> 16) as u16;
        let lo = t as u16;
        if (0x7000..0x7000 + 128 * 64).contains(&hi) && lo >= PLUGIN_PARAM {
            let loc = hi - 0x7000;
            if loc % 64 < 10 {
                return Target::SlotParam(loc / 64, (loc % 64) as u8, lo - PLUGIN_PARAM);
            }
        }
        if hi < 0x4000 && lo >= PLUGIN_PARAM {
            return Target::ChannelParam(hi, lo - PLUGIN_PARAM);
        }
        // Mixer places from FL 24.2 on (spec: flp/layouts.md "Mixer places", "Parameter ids in an automation link").
        if (0x7000..0x7000 + 127 * 64).contains(&hi) && (hi - 0x7000) % 64 < 10 {
            let (track, slot) = ((hi - 0x7000) / 64, ((hi - 0x7000) % 64) as u8);
            let id = lo as u8;
            return match lo >> 8 {
                0x1F if id == SLOT_ON => Target::SlotOn(track, slot),
                0x1F if id == SLOT_MIX => Target::SlotMix(track, slot),
                _ if slot != 0 => Target::Other(t),
                // Send levels: 0x2000 + d, and 0x1F00 + 64 + d as in the mixer's records.
                0x20 if lo - SEND < 127 => Target::MixerSend(track, lo - SEND),
                0x1F => match id {
                    64..=190 => Target::MixerSend(track, u16::from(id - 64)),
                    MIXER_VOLUME => Target::MixerVolume(track),
                    MIXER_PAN => Target::MixerPan(track),
                    MIXER_SEPARATION => Target::MixerSeparation(track),
                    EQ_GAIN..=0xD2 => Target::MixerEq(track, id - EQ_GAIN, 0),
                    EQ_FREQ..=0xDA => Target::MixerEq(track, id - EQ_FREQ, 1),
                    EQ_WIDTH..=0xE2 => Target::MixerEq(track, id - EQ_WIDTH, 2),
                    _ => Target::Other(t),
                },
                _ => Target::Other(t),
            };
        }
        if hi < 0x4000 {
            return match lo {
                CHANNEL_VOLUME => Target::ChannelVolume(hi),
                CHANNEL_PAN => Target::ChannelPan(hi),
                CHANNEL_PITCH => Target::ChannelPitch(hi),
                CHANNEL_CUTOFF | CHANNEL_RESONANCE | CHANNEL_PAN_OFFSET | CHANNEL_VOLUME_MULT | CHANNEL_MOD_X | CHANNEL_MOD_Y => {
                    Target::ChannelLevel(hi, lo)
                }
                _ => Target::Other(t),
            };
        }
        Target::Other(t)
    }

    /// Bipolar targets centre at 0.5.
    pub fn bipolar(t: u32) -> bool {
        matches!(
            decode(t),
            Target::ChannelPan(_)
                | Target::ChannelPitch(_)
                | Target::ChannelLevel(_, CHANNEL_PAN_OFFSET | CHANNEL_VOLUME_MULT | CHANNEL_MOD_X | CHANNEL_MOD_Y)
                | Target::MixerPan(_)
                | Target::MixerSeparation(_)
                | Target::MixerEq(_, _, 0)
        )
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        // spec: plugins/plugin-parameters.md §3
        #[test]
        fn plugin_parameter_targets() {
            assert_eq!(channel_param(2, 5), 0x0002_8005);
            assert_eq!(decode(0x0002_8005), Target::ChannelParam(2, 5));
            // Insert 1, slot 0: location 0x7040; slot 9 of the master: 0x7009.
            assert_eq!(slot_param(1, 0, 13), 0x7040_800D);
            assert_eq!(decode(0x7040_800D), Target::SlotParam(1, 0, 13));
            assert_eq!(decode(slot_param(0, 9, 0)), Target::SlotParam(0, 9, 0));
            assert_eq!(decode(slot_param(125, 3, 0x7FFF)), Target::SlotParam(125, 3, 0x7FFF));
            // The mixer track's own controls are not slot parameters.
            assert_eq!(decode(mixer(1, MIXER_VOLUME)), Target::MixerVolume(1));
            assert_eq!(decode(channel(3, CHANNEL_PAN)), Target::ChannelPan(3));
            assert!(!bipolar(slot_param(1, 0, 0)));
        }

        // spec: flp/events/automation.md "Targets"; flp/layouts.md "Parameter ids in an automation link"
        #[test]
        fn mixer_targets() {
            assert_eq!(decode(0x7001_1F01), Target::SlotMix(0, 1), "master slot 1 mix");
            assert_eq!(slot(0, 1, SLOT_MIX), 0x7001_1F01);
            assert_eq!(decode(slot(3, 9, SLOT_ON)), Target::SlotOn(3, 9));
            assert_eq!(decode(slot(3, 0, SLOT_ON)), Target::SlotOn(3, 0));
            assert_eq!(decode(0x7280_1FC0), Target::MixerVolume(10), "Insert 10 volume");
            assert_eq!(decode(mixer(4, MIXER_SEPARATION)), Target::MixerSeparation(4));
            assert_eq!(decode(mixer(2, EQ_GAIN + 1)), Target::MixerEq(2, 1, 0));
            assert_eq!(decode(mixer(2, EQ_FREQ + 2)), Target::MixerEq(2, 2, 1));
            assert_eq!(decode(mixer(2, EQ_WIDTH)), Target::MixerEq(2, 0, 2));
            assert_eq!(decode(mixer(2, 0xD3)), Target::Other(mixer(2, 0xD3)));
            // Send to track 85: 0x2055 at the sending track's place; the records' 0x1F00 + 64 + d form too.
            assert_eq!(decode(send(5, 85)), Target::MixerSend(5, 85));
            assert_eq!(send(5, 85) & 0xFFFF, 0x2055);
            assert_eq!(decode(mixer(5, 64 + 126)), Target::MixerSend(5, 126));
            // Track settings at a slot's place are nothing.
            assert_eq!(decode(slot(1, 2, MIXER_VOLUME)), Target::Other(slot(1, 2, MIXER_VOLUME)));
            assert!(bipolar(mixer(1, MIXER_SEPARATION)) && bipolar(mixer(1, EQ_GAIN)) && !bipolar(mixer(1, EQ_FREQ)));
            // spec: audio/voice-levels.md "Values from automation"
            assert_eq!(decode(channel(3, CHANNEL_CUTOFF)), Target::ChannelLevel(3, 2));
            assert_eq!(decode(channel(3, CHANNEL_MOD_Y)), Target::ChannelLevel(3, 0x14));
            assert_eq!(decode(channel(3, 0x12)), Target::Other(0x0003_0012));
            assert!(bipolar(channel(0, CHANNEL_VOLUME_MULT)) && !bipolar(channel(0, CHANNEL_CUTOFF)));
        }
    }
}

impl Automation {
    /// The first target: the one the clip is named after and whose values the playlist shows.
    pub fn target(&self) -> Option<u32> {
        self.targets.first().copied()
    }

    /// The curve's value at `beat` (clamped to the first / last point), shaped per segment mode as the walk plays it
    /// (spec: audio/automation.md "Segment shapes"). At a point's own beat it is that point's level.
    pub fn value_at(&self, beat: f64) -> Option<f64> {
        let pts = &self.points;
        let first = pts.first()?;
        if beat <= first.beat {
            return Some(f64::from(first.value as f32));
        }
        let i = pts.partition_point(|p| p.beat <= beat);
        let Some(b) = pts.get(i) else { return Some(f64::from(pts.last()?.value as f32)) };
        let a = pts[i - 1];
        let span = b.beat - a.beat;
        if span <= 0.0 {
            return Some(f64::from(b.value as f32));
        }
        let t = ((beat - a.beat) / span).clamp(0.0, 1.0);
        Some(curve::segment_value(pts, i, t))
    }
}
