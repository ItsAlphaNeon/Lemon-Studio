// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The plugin window's Misc tab (a host feature, the same for every channel): levels adjustment, polyphony, time,
//! tracking, cut groups, arpeggiator, echo delay and the root note with its keyboard. The frame draws it with the
//! instrument kit in place of the plugin's editor; values come from the channel (`ChannelInfo::misc`), edits go back
//! to the app as `FrameEvent::Misc`.
//!
//! Coordinates in the tables are FL window pixels of the measured Sampler window (568 x 466 with its title bar), the
//! same frame of reference as Lemon Sampler's pages: the page's top-left is FL (4, 72).
// spec: ui/plugin-window.md (Misc tab), plugins/lemon-sampler.md §9 (Misc page layout)

use os_theme::{Color, Palette};
use os_ui::kit::{inst, State as KS};
use os_ui::tabler as tb;
use os_ui::{Canvas, Font, Glyph, Rect};

/// FL x / y of the page's top-left corner.
const OX: f32 = 4.0;
const OY: f32 = 72.0;
/// Page size (the joined Lemon Sampler editor's size).
pub const PAGE_W: f32 = 560.0;
pub const PAGE_H: f32 = 390.0;

/// The page's values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum M {
    LevelPan,
    LevelVol,
    LevelModX,
    LevelModY,
    PolyMax,
    PolySlide,
    Porta,
    Mono,
    TimeGate,
    TimeShift,
    TimeSwing,
    FullPorta,
    /// Tracking, volume page.
    VolMid,
    VolPan,
    VolModX,
    VolModY,
    /// Tracking, key page.
    KeyMid,
    KeyPan,
    KeyModX,
    KeyModY,
    Cut,
    CutBy,
    ArpDir,
    ArpTime,
    ArpGate,
    ArpRange,
    ArpRepeat,
    ArpSlide,
    ArpChord,
    EchoFeed,
    EchoPan,
    EchoModX,
    EchoModY,
    EchoPitch,
    EchoTime,
    Echoes,
    PingPong,
    FatMode,
    Root,
    MainPitch,
    AddKey,
    Fine,
}

impl M {
    pub const COUNT: usize = M::Fine as usize + 1;
    pub const ALL: [M; M::COUNT] = [
        M::LevelPan,
        M::LevelVol,
        M::LevelModX,
        M::LevelModY,
        M::PolyMax,
        M::PolySlide,
        M::Porta,
        M::Mono,
        M::TimeGate,
        M::TimeShift,
        M::TimeSwing,
        M::FullPorta,
        M::VolMid,
        M::VolPan,
        M::VolModX,
        M::VolModY,
        M::KeyMid,
        M::KeyPan,
        M::KeyModX,
        M::KeyModY,
        M::Cut,
        M::CutBy,
        M::ArpDir,
        M::ArpTime,
        M::ArpGate,
        M::ArpRange,
        M::ArpRepeat,
        M::ArpSlide,
        M::ArpChord,
        M::EchoFeed,
        M::EchoPan,
        M::EchoModX,
        M::EchoModY,
        M::EchoPitch,
        M::EchoTime,
        M::Echoes,
        M::PingPong,
        M::FatMode,
        M::Root,
        M::MainPitch,
        M::AddKey,
        M::Fine,
    ];

    /// Hint label (Lemon Studio wording).
    pub fn label(self) -> &'static str {
        match self {
            M::LevelPan => "Panning adjustment",
            M::LevelVol => "Volume adjustment",
            M::LevelModX => "Mod X adjustment",
            M::LevelModY => "Mod Y adjustment",
            M::PolyMax => "Maximum polyphony",
            M::PolySlide => "Slide time",
            M::Porta => "Portamento",
            M::Mono => "Mono",
            M::TimeGate => "Note gate",
            M::TimeShift => "Note time shift",
            M::TimeSwing => "Note time swing",
            M::FullPorta => "Full portamento",
            M::VolMid | M::KeyMid => "Tracking middle point",
            M::VolPan | M::KeyPan => "Tracking to panning",
            M::VolModX | M::KeyModX => "Tracking to Mod X",
            M::VolModY | M::KeyModY => "Tracking to Mod Y",
            M::Cut => "Cut group",
            M::CutBy => "Cut by group",
            M::ArpDir => "Arpeggio direction",
            M::ArpTime => "Arpeggio time",
            M::ArpGate => "Arpeggio gate",
            M::ArpRange => "Arpeggio range (octaves)",
            M::ArpRepeat => "Arpeggio note repeat",
            M::ArpSlide => "Arpeggio slide",
            M::ArpChord => "Arpeggio chord",
            M::EchoFeed => "Echo feedback",
            M::EchoPan => "Echo panning",
            M::EchoModX => "Echo Mod X",
            M::EchoModY => "Echo Mod Y",
            M::EchoPitch => "Echo pitch",
            M::EchoTime => "Echo time",
            M::Echoes => "Number of echoes",
            M::PingPong => "Ping pong echoes",
            M::FatMode => "Fat mode",
            M::Root => "Root note",
            M::MainPitch => "Affected by the main pitch",
            M::AddKey => "Add root note to the key",
            M::Fine => "Fine tuning",
        }
    }
}

/// Entries of FL 24.1's chords file, which the stored arpeggio chord `k >= 1` names (spec: audio/channel-misc.md
/// "Settings", "The chords file").
pub const CHORD_ENTRIES: i32 = 88;

/// The name of arpeggio chord `k` (`M::ArpChord`): -1 = the held notes, 0 = the note alone, `k >= 1` = entry `k` of
/// the chords file. Only entries 1, 7, 10 and 72 are known (their intervals 4,3 / 4,4 / 4,3,2 / 3,2,2,3); the others
/// show their number.
pub fn chord_name(k: i32) -> String {
    match k {
        -1 => "Held notes".into(),
        0 => "Note alone".into(),
        1 => "Major".into(),
        7 => "Augmented".into(),
        10 => "Major 6".into(),
        72 => "Minor pentatonic".into(),
        k => format!("Chord {k}"),
    }
}

/// Values of the page, indexed by `M`. Knobs are 0..1 (bipolar knobs centred at 0.5), switches 0 / 1, boxes and
/// lists whole numbers (0 = `---` for boxes that can be off), `Root` a MIDI key (60 = C5), `Fine` cents ±100,
/// `*Mid` MIDI keys.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MiscVals {
    pub v: [f32; M::COUNT],
    /// A sampler channel: "Add to key" reaches the sound (Lemon Sampler plays it; the engine has no such setting).
    pub sampler: bool,
}

impl Default for MiscVals {
    /// A new channel (measured on FL's default Sampler channel).
    fn default() -> Self {
        let mut v = [0.0; M::COUNT];
        for m in M::ALL {
            v[m as usize] = default_of(m);
        }
        MiscVals { v, sampler: true }
    }
}

impl MiscVals {
    pub fn get(&self, m: M) -> f32 {
        self.v[m as usize]
    }
    pub fn set(&mut self, m: M, v: f32) {
        self.v[m as usize] = v;
    }
}

/// Whether value `m` reaches the sound today: the level adjustments; polyphony, slide, porta / mono, swing and cut
/// groups (the engine's note rules); root note, fine tuning and main pitch (the channel's pitch); "Add to key" when
/// `v.sampler`. The others (gate, shift, full porta, tracking, arpeggiator, echo delay) are drawn greyed out.
pub fn works(m: M, v: &MiscVals) -> bool {
    match m {
        M::LevelPan | M::LevelVol | M::LevelModX | M::LevelModY => true,
        M::PolyMax | M::PolySlide | M::Porta | M::Mono | M::TimeSwing | M::Cut | M::CutBy => true,
        M::Root | M::Fine | M::MainPitch => true,
        M::AddKey => v.sampler,
        _ => false,
    }
}

/// The name of a greyed-out control under hit `h` (its hint and click say it is not available yet); `None` for the
/// ones that work. Keyboard keys always play.
pub fn off_name(h: Hit, v: &MiscVals) -> Option<&'static str> {
    match h {
        Hit::Value(m) | Hit::Step(m, _) => (!works(m, v)).then(|| m.label()),
        Hit::ArpDir(_) => Some("Arpeggiator"),
        Hit::TrackTab(_) => Some("Tracking"),
        Hit::Touch => Some("Multi-touch keyboard"),
        Hit::CutSelf | Hit::Reset | Hit::Key(_) | Hit::Keyboard | Hit::ScrollBar => None,
    }
}

/// The default of one value (middle click resets to it).
pub fn default_of(m: M) -> f32 {
    match m {
        M::LevelPan | M::LevelVol | M::LevelModX | M::LevelModY => 0.5,
        M::VolPan | M::VolModX | M::VolModY | M::KeyPan | M::KeyModX | M::KeyModY => 0.5,
        M::EchoPan | M::EchoModX | M::EchoModY | M::EchoPitch => 0.5,
        M::TimeGate | M::TimeSwing | M::ArpGate => 1.0,
        M::ArpTime => os_model::Arpeggio::TIME_DEFAULT,
        M::EchoTime => os_model::Echo::TIME_DEFAULT,
        M::PolySlide => 500.0 / 1024.0,
        M::FullPorta | M::MainPitch => 1.0,
        M::ArpRange | M::ArpRepeat => 1.0,
        M::ArpChord => -1.0,
        M::Echoes => 4.0,
        M::VolMid => 100.0,
        M::Root | M::KeyMid => 60.0,
        _ => 0.0,
    }
}

/// Smallest and largest value of a box / list / note value.
pub fn range_of(m: M) -> (f32, f32) {
    match m {
        M::PolyMax => (0.0, 999.0),
        M::Cut | M::CutBy => (0.0, 999.0),
        // spec: audio/channel-misc.md "Settings" (the echo count's control range was not found; 0 and 10 were exported).
        M::ArpRange => (0.0, 6.0),
        M::ArpRepeat => (1.0, 6.0),
        M::Echoes => (0.0, 10.0),
        M::ArpDir => (0.0, 5.0),
        M::ArpChord => (-1.0, CHORD_ENTRIES as f32),
        M::Root | M::VolMid | M::KeyMid => (0.0, 131.0),
        M::Fine => (-100.0, 100.0),
        _ => (0.0, 1.0),
    }
}

/// Note name ("C5", "F#4"): FL's naming, C5 = MIDI 60, as the settings choose it (`crate::note_names`).
pub fn note_name(key: i32) -> String {
    crate::note_names::note_name(key)
}

/// The hint value text.
pub fn value_text(m: M, v: f32) -> String {
    match kind(m) {
        Kind::Knob { bipolar: true, .. } => format!("{:+.0}%", (v - 0.5) * 200.0),
        Kind::Knob { .. } => format!("{:.0}%", v * 100.0),
        Kind::Radio => (if v >= 0.5 { "on" } else { "off" }).into(),
        Kind::Spin { dash } if dash && v < 0.5 => "none".into(),
        Kind::Spin { .. } => format!("{}", v.round() as i32),
        _ => match m {
            M::ArpChord => chord_name(v.round() as i32),
            M::ArpDir => ARP_DIR_NAMES[v.round().clamp(0.0, 5.0) as usize].into(),
            M::Fine => format!("{:+.0} cents", v),
            M::VolMid => format!("{}", v.round() as i32),
            M::Root | M::KeyMid => note_name(v.round() as i32),
            _ => format!("{v}"),
        },
    }
}

/// Arpeggio directions by stored value (spec: audio/channel-misc.md "Settings").
const ARP_DIR_NAMES: [&str; 6] = ["off", "up", "down", "up and down (bounce)", "up and down (sticky)", "random"];

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    /// FL x / y centre, ball radius, ring radius, dot colour.
    Knob { bipolar: bool },
    Radio,
    /// A number box; `dash` = 0 shows `---`.
    Spin { dash: bool },
    List,
    Slider,
    Other,
}

fn kind(m: M) -> Kind {
    match m {
        M::LevelPan | M::LevelVol | M::LevelModX | M::LevelModY => Kind::Knob { bipolar: true },
        M::VolPan | M::VolModX | M::VolModY | M::KeyPan | M::KeyModX | M::KeyModY => Kind::Knob { bipolar: true },
        M::EchoPan | M::EchoModX | M::EchoModY | M::EchoPitch => Kind::Knob { bipolar: true },
        M::PolySlide | M::TimeGate | M::TimeShift | M::TimeSwing | M::ArpTime | M::ArpGate | M::EchoFeed | M::EchoTime => {
            Kind::Knob { bipolar: false }
        }
        M::Porta | M::Mono | M::FullPorta | M::ArpSlide | M::PingPong | M::FatMode | M::MainPitch | M::AddKey => Kind::Radio,
        M::PolyMax | M::Cut | M::CutBy | M::ArpRepeat => Kind::Spin { dash: true },
        M::ArpRange | M::Echoes => Kind::Spin { dash: false },
        M::ArpChord => Kind::List,
        M::VolMid | M::KeyMid | M::Fine => Kind::Slider,
        M::ArpDir | M::Root => Kind::Other,
    }
}

// ----------------------------------------------------------------------------------------------------------------
// Layout (FL window px)

/// Panels: outline rect (inclusive FL x0, x1, y0, y1), header tab width, title.
const PANELS: [(f32, f32, f32, f32, f32, &str); 8] = [
    (11.0, 188.0, 78.0, 179.0, 106.0, "Levels adjustment"),
    (195.0, 316.0, 78.0, 179.0, 67.0, "Polyphony"),
    (323.0, 434.0, 78.0, 179.0, 40.0, "Time"),
    (441.0, 556.0, 78.0, 179.0, 108.0, ""),
    (11.0, 79.0, 186.0, 309.0, 48.0, "Group"),
    (86.0, 316.0, 186.0, 309.0, 187.0, ""),
    (323.0, 556.0, 186.0, 309.0, 137.0, ""),
    (11.0, 556.0, 316.0, 454.0, 84.0, ""),
];

/// Knobs: value, FL centre, ball radius, ring radius, dot colour, caption, caption top.
const KNOBS: [(M, f32, f32, f32, f32, u32, &str, f32); 19] = [
    (M::LevelPan, 37.0, 123.5, 15.0, 17.0, 0x43afbd, "PAN", 149.0),
    (M::LevelVol, 79.0, 123.5, 15.0, 17.0, 0x41b57a, "VOL", 149.0),
    (M::LevelModX, 121.0, 123.5, 15.0, 17.0, 0xa0b241, "MOD X", 149.0),
    (M::LevelModY, 163.0, 123.5, 15.0, 17.0, 0xb87c44, "MOD Y", 149.0),
    (M::PolySlide, 293.5, 120.5, 11.0, 13.5, 0xe89a2f, "SLIDE", 139.0),
    (M::TimeGate, 345.0, 121.0, 11.0, 13.5, 0xe89a2f, "GATE", 139.0),
    (M::TimeShift, 379.0, 121.0, 11.0, 13.5, 0xe89a2f, "SHIFT", 139.0),
    (M::TimeSwing, 413.0, 121.0, 11.0, 13.5, 0xe89a2f, "SWING", 139.0),
    // Tracking: the Vol page's values; the Key page swaps in its own (see `track_m`).
    (M::VolPan, 463.0, 121.0, 11.0, 13.5, 0x43afbd, "PAN", 139.0),
    (M::VolModX, 503.5, 121.0, 11.0, 13.5, 0xa0b241, "MOD X", 139.0),
    (M::VolModY, 535.0, 121.0, 11.0, 13.5, 0xb87c44, "MOD Y", 139.0),
    (M::ArpTime, 108.5, 229.5, 11.0, 13.5, 0xe89a2f, "TIME", 247.0),
    (M::ArpGate, 142.5, 229.5, 11.0, 13.5, 0xe89a2f, "GATE", 247.0),
    (M::EchoFeed, 345.0, 229.0, 11.0, 13.5, 0x35aa6f, "FEED", 247.0),
    (M::EchoPan, 376.5, 229.0, 11.0, 13.5, 0x43afbd, "PAN", 247.0),
    (M::EchoModX, 419.0, 229.0, 11.0, 13.5, 0xa0b241, "MOD X", 247.0),
    (M::EchoModY, 451.0, 229.0, 11.0, 13.5, 0xb87c44, "MOD Y", 247.0),
    (M::EchoPitch, 492.5, 229.0, 11.0, 13.5, 0xbb465f, "PITCH", 247.0),
    (M::EchoTime, 534.0, 229.0, 11.0, 13.5, 0xe89a2f, "TIME", 247.0),
];

/// Radios: value, FL centre, label, label left of the radio.
const RADIOS: [(M, f32, f32, &str, bool); 8] = [
    (M::Porta, 209.5, 161.5, "Porta", false),
    (M::Mono, 264.5, 161.5, "Mono", false),
    (M::FullPorta, 337.5, 162.0, "Full porta", false),
    (M::ArpSlide, 99.5, 281.5, "Slide", false),
    (M::PingPong, 542.0, 272.0, "Ping pong", true),
    (M::FatMode, 542.0, 292.0, "Fat mode", true),
    (M::MainPitch, 80.0, 350.0, "Enable main pitch", false),
    (M::AddKey, 200.0, 350.0, "Add to key", false),
];

/// Number boxes: value, rect (inclusive FL x0, x1, y0, y1), label, label x.
const SPINS: [(M, f32, f32, f32, f32, &str, f32); 6] = [
    (M::PolyMax, 230.0, 259.0, 108.0, 130.0, "Max", 204.0),
    (M::Cut, 44.0, 72.0, 216.0, 237.0, "Cut", 20.0),
    (M::CutBy, 44.0, 72.0, 245.0, 266.0, "By", 27.0),
    (M::ArpRange, 203.0, 226.0, 216.0, 238.0, "Range", 166.0),
    (M::ArpRepeat, 287.0, 308.0, 216.0, 238.0, "Repeat", 246.0),
    (M::Echoes, 374.0, 401.0, 272.0, 293.0, "Echoes", 333.0),
];

const CHORD_BOX: (f32, f32, f32, f32) = (203.0, 308.0, 272.0, 292.0);
const CUT_SELF: (f32, f32, f32, f32) = (15.0, 75.0, 276.0, 305.0);
const RESET: (f32, f32, f32, f32) = (13.0, 62.0, 339.0, 361.0);
/// Tracking header words: Vol, Key (FL x0..x1 of their cells, y 80..97).
const TRACK_TABS: [(f32, f32); 2] = [(443.0, 467.0), (468.0, 493.0)];
/// Arpeggio direction marks (centre x, y 196.5).
const ARP_DIRS: [f32; 6] = [96.0, 116.0, 135.0, 153.5, 172.0, 191.5];
/// Tracking Mid slider (x0, x1, cy) and fine-tune slider.
const MID_SLIDER: (f32, f32, f32) = (479.0, 547.0, 162.0);
const FINE_SLIDER: (f32, f32, f32) = (333.0, 436.0, 350.0);
/// Hand button centre (multi-touch keyboard).
const TOUCH: (f32, f32) = (541.0, 350.0);
/// Keyboard: x range, ruler top, keys top, black key bottom, keys bottom, scroll bar bottom.
const KB_X0: f32 = 13.0;
const KB_X1: f32 = 555.0;
const KB_RULER: f32 = 365.0;
const KB_KEYS: f32 = 379.0;
const KB_BLACK: f32 = 418.0;
const KB_BOTTOM: f32 = 438.0;
const KB_BAR: f32 = 453.0;
const OCTAVE: f32 = 91.0;
const WHITE: f32 = 13.0;
const BLACK_W: f32 = 8.0;
/// Black key left offsets in the octave (C#, D#, F#, G#, A#).
const BLACK_OFF: [(i32, f32); 5] = [(1, 8.0), (3, 23.0), (6, 47.0), (8, 61.0), (10, 75.0)];

thread_local! {
    /// Frame position of the page's top-left while painting.
    static ORG: std::cell::Cell<(f32, f32)> = const { std::cell::Cell::new((0.0, 0.0)) };
}

/// FL px -> frame px offsets while painting.
fn dx() -> f32 {
    ORG.with(|o| o.get().0) - OX
}
fn dy() -> f32 {
    ORG.with(|o| o.get().1) - OY
}

/// FL px -> frame px.
fn p(x: f32, y: f32) -> (f32, f32) {
    (x + dx(), y + dy())
}

fn rect(x0: f32, x1: f32, y0: f32, y1: f32) -> Rect {
    Rect::new(x0 + dx(), y0 + dy(), x1 - x0 + 1.0, y1 - y0 + 1.0)
}

/// The tracking value shown at a knob's place for the active tracking page.
fn track_m(m: M, key: bool) -> M {
    if !key {
        return m;
    }
    match m {
        M::VolPan => M::KeyPan,
        M::VolModX => M::KeyModX,
        M::VolModY => M::KeyModY,
        M::VolMid => M::KeyMid,
        other => other,
    }
}

// ----------------------------------------------------------------------------------------------------------------
// Hits and interaction

/// What is under the mouse (page px).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    /// A value control (knob, radio, box, list, slider).
    Value(M),
    /// A number box's up (+1) / down (-1) arrow.
    Step(M, i32),
    ArpDir(u8),
    /// Tracking header: Vol (false) / Key (true).
    TrackTab(bool),
    CutSelf,
    Reset,
    /// The hand button (multi-touch keyboard).
    Touch,
    /// A keyboard key.
    Key(u8),
    /// The keyboard's ruler (wheel scrolls).
    Keyboard,
    /// The keyboard's scroll bar (drag the thumb; wheel scrolls).
    ScrollBar,
}

/// Page UI state kept by the frame.
#[derive(Clone, Debug, PartialEq)]
pub struct MiscUi {
    /// Tracking page: false = Vol, true = Key (FL shows Key).
    pub track_key: bool,
    /// Keyboard scroll: FL x of C0 (C3 sits at x 96 by default).
    pub kb_scroll: f32,
    /// Drag in progress: value, start y (or x for sliders), start value.
    pub drag: Option<(M, f32, f32)>,
    /// Scroll bar thumb drag in progress: start FL x, start scroll (`bar_press`).
    pub bar_drag: Option<(f32, f32)>,
    pub hover: Option<Hit>,
    /// Key held by a left press on the keyboard.
    pub held: Option<u8>,
    /// Hint line for the title bar (`Label: value` of the control under the mouse).
    pub hint: String,
}

impl Default for MiscUi {
    fn default() -> Self {
        MiscUi { track_key: true, kb_scroll: 96.0 - 3.0 * OCTAVE, drag: None, bar_drag: None, hover: None, held: None, hint: String::new() }
    }
}

pub fn hit(ui: &MiscUi, x: f32, y: f32) -> Option<Hit> {
    let (fx, fy) = (x + OX, y + OY);
    for &(m, cx, cy, _, ring, _, _, _) in &KNOBS {
        if ring > 0.0 && (fx - cx).powi(2) + (fy - cy).powi(2) <= (ring + 2.0).powi(2) {
            return Some(Hit::Value(track_m(m, ui.track_key)));
        }
    }
    for &(m, cx, cy, label, left) in &RADIOS {
        // The radio and its label.
        let lw = label.len() as f32 * 6.5;
        let (x0, x1) = if left { (cx - 10.0 - lw, cx + 8.0) } else { (cx - 8.0, cx + 10.0 + lw) };
        if fx >= x0 && fx <= x1 && (fy - cy).abs() <= 8.0 {
            return Some(Hit::Value(m));
        }
    }
    for &(m, x0, x1, y0, y1, _, _) in &SPINS {
        if fx >= x0 && fx <= x1 && fy >= y0 && fy <= y1 {
            // The arrows column (right 8 px): top half up, bottom half down.
            if fx >= x1 - 8.0 {
                return Some(Hit::Step(m, if fy < (y0 + y1) / 2.0 { 1 } else { -1 }));
            }
            return Some(Hit::Value(m));
        }
    }
    let inside = |r: (f32, f32, f32, f32)| fx >= r.0 && fx <= r.1 && fy >= r.2 && fy <= r.3;
    if inside(CHORD_BOX) {
        return Some(Hit::Value(M::ArpChord));
    }
    if inside(CUT_SELF) {
        return Some(Hit::CutSelf);
    }
    if inside(RESET) {
        return Some(Hit::Reset);
    }
    for (i, &(x0, x1)) in TRACK_TABS.iter().enumerate() {
        if fx >= x0 && fx <= x1 && (80.0..=97.0).contains(&fy) {
            return Some(Hit::TrackTab(i == 1));
        }
    }
    for (i, &cx) in ARP_DIRS.iter().enumerate() {
        if (fx - cx).abs() <= 9.0 && (188.0..=205.0).contains(&fy) {
            return Some(Hit::ArpDir(i as u8));
        }
    }
    let (mx0, mx1, mcy) = MID_SLIDER;
    if fx >= mx0 - 6.0 && fx <= mx1 + 6.0 && (fy - mcy).abs() <= 9.0 {
        return Some(Hit::Value(track_m(M::VolMid, ui.track_key)));
    }
    let (fx0, fx1, fcy) = FINE_SLIDER;
    if fx >= fx0 && fx <= fx1 && (fy - fcy).abs() <= 9.0 {
        return Some(Hit::Value(M::Fine));
    }
    if (fx - TOUCH.0).abs() <= 10.0 && (fy - TOUCH.1).abs() <= 10.0 {
        return Some(Hit::Touch);
    }
    if fx >= KB_X0 && fx <= KB_X1 && fy >= KB_RULER && fy <= KB_BAR {
        if (KB_KEYS..=KB_BOTTOM).contains(&fy) {
            if let Some(k) = key_at(ui, fx, fy) {
                return Some(Hit::Key(k));
            }
        }
        return Some(if fy > KB_BOTTOM { Hit::ScrollBar } else { Hit::Keyboard });
    }
    None
}

/// Keyboard key under FL point (fx, fy).
fn key_at(ui: &MiscUi, fx: f32, fy: f32) -> Option<u8> {
    let rel = fx - ui.kb_scroll;
    let oct = (rel / OCTAVE).floor();
    let inx = rel - oct * OCTAVE;
    if fy <= KB_BLACK {
        for &(n, off) in &BLACK_OFF {
            if inx >= off && inx < off + BLACK_W {
                let k = oct as i32 * 12 + n;
                return (0..=131).contains(&k).then_some(k as u8);
            }
        }
    }
    const WHITE_N: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
    let w = ((inx / WHITE).floor() as usize).min(6);
    let k = oct as i32 * 12 + WHITE_N[w];
    (0..=131).contains(&k).then_some(k as u8)
}

/// Pixels of vertical drag for a knob's full range (spec: plugins/lemon-sampler.md §4), and per box step.
const KNOB_PX: f32 = 256.0;
const SPIN_PX: f32 = 8.0;

/// A drag moved to page point (x, y): the new value of the dragged control (`fine` = Ctrl, ¼ speed).
pub fn drag_value(ui: &MiscUi, x: f32, y: f32, fine: bool) -> Option<(M, f32)> {
    let (m, start, v0) = ui.drag?;
    let f = if fine { 0.25 } else { 1.0 };
    let (lo, hi) = range_of(m);
    Some(match kind(m) {
        Kind::Knob { .. } => (m, (v0 + (start - y) / KNOB_PX * f).clamp(0.0, 1.0)),
        Kind::Spin { .. } => (m, (v0 + ((start - y) / SPIN_PX * f).round()).clamp(lo, hi)),
        Kind::Slider => {
            let (x0, x1) = if m == M::Fine { (FINE_SLIDER.0, FINE_SLIDER.1) } else { (MID_SLIDER.0, MID_SLIDER.1) };
            let per = (hi - lo) / (x1 - x0 - 12.0);
            let v = v0 + (x - start) * per * f;
            (m, if m == M::Fine { v.round() } else { v.round() }.clamp(lo, hi))
        }
        _ => return None,
    })
}

/// Where a drag of `m` starts from (the coordinate `drag_value` compares with).
pub fn drag_start(m: M, x: f32, y: f32) -> f32 {
    if kind(m) == Kind::Slider { x } else { y }
}

/// Whether `m` is dragged (knobs, boxes, sliders) rather than clicked.
pub fn draggable(m: M) -> bool {
    matches!(kind(m), Kind::Knob { .. } | Kind::Spin { .. } | Kind::Slider)
}

/// A click on `m` (not a drag): the new value, if a click changes it (radios toggle).
pub fn click_value(m: M, v: f32) -> Option<f32> {
    (kind(m) == Kind::Radio).then_some(if v >= 0.5 { 0.0 } else { 1.0 })
}

/// The wheel over `m`: new value (`notches` > 0 = up).
pub fn wheel_value(m: M, v: f32, notches: f32) -> Option<f32> {
    let (lo, hi) = range_of(m);
    match kind(m) {
        Kind::Knob { .. } => Some((v + notches * 8.0 / KNOB_PX).clamp(0.0, 1.0)),
        Kind::Spin { .. } | Kind::Slider | Kind::List => Some((v + notches.signum()).clamp(lo, hi)),
        _ => None,
    }
}

/// Whether `m` opens a list on a left click (the chord list) or on a right click (number boxes).
pub fn list_on_click(m: M) -> bool {
    kind(m) == Kind::List
}
pub fn list_on_right(m: M) -> bool {
    matches!(kind(m), Kind::Spin { .. } | Kind::List)
}

/// Entries of a value's list menu: (label, value).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn list_entries(m: M) -> Vec<(String, f32)> {
    match m {
        M::ArpChord => (-1..=CHORD_ENTRIES).map(|k| (chord_name(k), k as f32)).collect(),
        _ => {
            let (lo, hi) = range_of(m);
            let dash = matches!(kind(m), Kind::Spin { dash: true });
            let top = hi.min(if m == M::PolyMax { 32.0 } else { 16.0 });
            (lo as i32..=top as i32).map(|i| (if dash && i == 0 { "---".to_string() } else { i.to_string() }, i as f32)).collect()
        }
    }
}

// ----------------------------------------------------------------------------------------------------------------
// Painting

/// State of the control under hit `h`: greyed-out controls are disabled and never light up.
fn state_of(ui: &MiscUi, v: &MiscVals, h: Hit) -> KS {
    let off = off_name(h, v).is_some();
    KS::hover(ui.hover == Some(h) && !off).with_disabled(off)
}

/// Veils over what is greyed out: whole panels with nothing working, rounded like the panel in the Lemon design. The
/// single controls (`works`) draw their disabled look; Classic veils them too (FL px rects).
fn veil_off(c: &mut Canvas, t: &Palette, v: &MiscVals) {
    let veil = t.s(0x222525).with_alpha(crate::app::OFF_VEIL);
    let lemon = t.style.lemon();
    // Tracking, arpeggiator, echo delay.
    for i in [3, 5, 6] {
        let (x0, x1, y0, y1, _, _) = PANELS[i];
        if lemon {
            // The corners of the Lemon panel (`inst::panel`).
            c.fill_rrect(rect(x0, x1, y0, y1), 10.0, veil);
        } else {
            c.fill_rect(rect(x0, x1, y0, y1), veil);
        }
    }
    if lemon {
        return;
    }
    // Time: gate, shift and full porta (swing works).
    c.fill_rect(rect(329.0, 396.0, 104.0, 150.0), veil);
    c.fill_rect(rect(328.0, 410.0, 153.0, 171.0), veil);
    // The multi-touch hand, and "Add to key" on a channel that is not a sampler.
    c.fill_rect(rect(TOUCH.0 - 10.0, TOUCH.0 + 10.0, TOUCH.1 - 10.0, TOUCH.1 + 10.0), veil);
    if !v.sampler {
        c.fill_rect(rect(190.0, 270.0, 341.0, 359.0), veil);
    }
}

/// Paints the page with its top-left at (ax, ay) of the frame (frame px).
pub fn paint(c: &mut Canvas, t: &Palette, ax: f32, ay: f32, v: &MiscVals, ui: &MiscUi) {
    ORG.with(|o| o.set((ax, ay)));
    c.push_clip(Rect::new(ax, ay, PAGE_W, PAGE_H));
    c.fill_rect(Rect::new(ax, ay, PAGE_W, PAGE_H), t.s(0x202324));
    for &(x0, x1, y0, y1, tw, title) in &PANELS {
        let r = rect(x0, x1, y0, y1);
        inst::panel(c, t, r);
        inst::panel_tab(c, t, r, tw, title);
    }
    headers(c, t, v, ui);
    let accent = t.a(0xffa830);
    for &(m, cx, cy, r, ring, dot, cap, capy) in &KNOBS {
        if r <= 0.0 {
            continue;
        }
        let mm = track_m(m, ui.track_key);
        let bipolar = matches!(kind(mm), Kind::Knob { bipolar: true });
        let (x, y) = p(cx, cy);
        // SLIDE only acts with Porta / Mono: drawn dimmed otherwise.
        let off = m == M::PolySlide && v.get(M::Porta) < 0.5 && v.get(M::Mono) < 0.5;
        let ring_col = if off { accent.mix(t.s(0x222525), 0.45) } else { accent };
        let st = state_of(ui, v, Hit::Value(mm));
        inst::dial(c, t, x, y, r, ring, v.get(mm), bipolar, ring_col, st);
        // The Classic kit dial colours its dot from the accent; FL's Misc dots have their own colours. The Lemon dial
        // has a pointer line and no dot.
        if !t.style.lemon() && !st.disabled {
            let a = inst::dial_angle(v.get(mm), bipolar);
            let d = r - 3.5;
            let by = if st.hover { y - 1.0 } else { y };
            let (px, py) = (x + d * a.cos(), by + d * a.sin());
            c.fill_rect(Rect::new((px - 1.0).round(), (py - 1.0).round(), 2.0, 2.0), t.a(dot));
        }
        inst::caption(c, t, x, capy + dy(), cap, off || st.disabled);
    }
    for &(m, cx, cy, label, left) in &RADIOS {
        let (x, y) = p(cx, cy);
        let st = state_of(ui, v, Hit::Value(m));
        inst::radio(c, t, x, y, v.get(m) >= 0.5, st);
        if left {
            let w = c.text_width(inst::LABEL, label);
            inst::label(c, t, x - 11.0 - w, y, label, st.disabled);
        } else {
            inst::label(c, t, x + 11.0, y, label, st.disabled);
        }
    }
    for &(m, x0, x1, y0, y1, label, lx) in &SPINS {
        let r = rect(x0, x1, y0, y1);
        let st = state_of(ui, v, Hit::Value(m));
        inst::label(c, t, lx + dx(), r.cy(), label, st.disabled);
        let dash = matches!(kind(m), Kind::Spin { dash: true });
        let val = v.get(m).round() as i32;
        let text = if dash && val == 0 { None } else { Some(val.to_string()) };
        spin_box(c, t, r, text.as_deref(), st);
    }
    // Chord list.
    let (x0, x1, y0, y1) = CHORD_BOX;
    let cr = rect(x0, x1, y0, y1);
    let st = state_of(ui, v, Hit::Value(M::ArpChord));
    inst::label(c, t, 166.0 + dx(), cr.cy(), "Chord", st.disabled);
    let chord = chord_name(v.get(M::ArpChord).round() as i32);
    inst::list_box(c, t, cr, &chord, st);
    // Buttons.
    let (x0, x1, y0, y1) = CUT_SELF;
    dark_button(c, t, rect(x0, x1, y0, y1), "Cut self", state_of(ui, v, Hit::CutSelf).hover);
    let (x0, x1, y0, y1) = RESET;
    dark_button(c, t, rect(x0, x1, y0, y1), "Reset", state_of(ui, v, Hit::Reset).hover);
    // Tracking Mid slider: orange from the left end to the thumb.
    let mid = track_m(M::VolMid, ui.track_key);
    let (mx0, mx1, mcy) = MID_SLIDER;
    let (sx0, scy) = p(mx0, mcy);
    let sx1 = mx1 + dx();
    let st = state_of(ui, v, Hit::Value(mid));
    inst::label(c, t, 455.0 + dx(), scy, "Mid", st.disabled);
    let (lo, hi) = range_of(mid);
    let track = slider_track(c, t, sx0, sx1, scy, (v.get(mid) - lo) / (hi - lo), st, (!st.disabled).then_some(accent));
    let _ = track;
    // Root note row.
    let root = v.get(M::Root).round() as i32;
    let lh = c.line_height(inst::LABEL);
    c.text(inst::LABEL, 15.0 + dx(), (326.0 + dy() - lh / 2.0).round(), t.tx(0xd2d8dc), &format!("Root note: {}", note_name(root)));
    inst::label(c, t, 279.0 + dx(), 350.0 + dy(), "Fine tune", false);
    let (fx0, fx1, fcy) = FINE_SLIDER;
    inst::hslider(c, t, fx0 + dx(), fx1 + dx(), fcy + dy(), (v.get(M::Fine) + 100.0) / 200.0, state_of(ui, v, Hit::Value(M::Fine)));
    let (hx, hy) = p(TOUCH.0, TOUCH.1);
    let hand = if state_of(ui, v, Hit::Touch).disabled { t.text_faint } else { t.tx(0xd2d8dc) };
    c.icon(&tb::HAND_FINGER, Rect::new(hx - 9.0, hy - 9.0, 18.0, 18.0), hand);
    keyboard(c, t, root as u8, ui);
    veil_off(c, t, v);
    c.pop_clip();
}

/// Header contents that are not plain titles: tracking Vol / Key words, arpeggiator direction marks, echo ▸.
fn headers(c: &mut Canvas, t: &Palette, v: &MiscVals, ui: &MiscUi) {
    let lh = c.line_height(inst::LABEL);
    let ty = (88.5 + dy() - lh / 2.0).round();
    // Tracking: "Vol" and "Key" are tabs; the active one is orange on a dark cell.
    for (i, &(x0, x1)) in TRACK_TABS.iter().enumerate() {
        let on = ui.track_key == (i == 1);
        if on {
            c.fill_rect(rect(x0, x1, 80.0, 97.0), t.s(0x1b1d1e));
        }
        let col = if on { t.a(0xffa830) } else if state_of(ui, v, Hit::TrackTab(i == 1)).hover { t.tx(0xffffff) } else { t.tx(0xd2d8dc) };
        let s = if i == 0 { "Vol" } else { "Key" };
        let w = c.text_width(inst::LABEL, s);
        c.text(inst::LABEL, ((x0 + x1) / 2.0 + dx() - w / 2.0).round(), ty, col, s);
    }
    c.text(inst::LABEL, 497.0 + dx(), ty, t.tx(0xd2d8dc), "Tracking");
    // Arpeggiator: direction marks, the active one on a darker cell.
    let dir = v.get(M::ArpDir).round() as usize;
    let my = 196.5 + dy();
    let ty2 = (196.5 + dy() - lh / 2.0).round();
    for (i, &cx) in ARP_DIRS.iter().enumerate() {
        let x = cx + dx();
        if i == dir {
            c.fill_rect(Rect::new(x - 9.5, 187.0 + dy(), 19.0, 19.0), t.s(0x1b1d1e));
        }
        let col = if state_of(ui, v, Hit::ArpDir(i as u8)).hover { t.tx(0xffffff) } else { t.tx(0xd2d8dc) };
        let tri = |c: &mut Canvas, up: bool, cy: f32, h: f32| {
            let mut g = Glyph::new();
            let s = if up { -1.0 } else { 1.0 };
            g.polygon(&[(x - h, cy - s * h / 2.0), (x + h, cy - s * h / 2.0), (x, cy + s * h / 2.0)]);
            c.glyph(col, &g);
        };
        match i {
            0 => c.icon_stroke(&tb::X, Rect::new(x - 7.0, my - 7.0, 14.0, 14.0), col, 1.6),
            1 => tri(c, true, my, 5.0),
            2 => tri(c, false, my, 5.0),
            3 => {
                tri(c, true, my - 3.5, 3.5);
                tri(c, false, my + 3.5, 3.5);
            }
            4 => {
                c.fill_rect(Rect::new(x - 5.0, my - 7.0, 10.0, 1.0), col);
                tri(c, true, my - 2.5, 3.5);
                tri(c, false, my + 2.5, 3.5);
                c.fill_rect(Rect::new(x - 5.0, my + 6.0, 10.0, 1.0), col);
            }
            _ => {
                let w = c.text_width(Font::new(16), "?");
                let lh = c.line_height(Font::new(16));
                c.text(Font::new(16), (x - w / 2.0).round(), (my - lh / 2.0).round(), col, "?");
            }
        }
    }
    c.text(inst::LABEL, 205.0 + dx(), ty2, t.tx(0xd2d8dc), "Arpeggiator");
    // Echo delay: ▸ then the title.
    let mut g = Glyph::new();
    let (ax, ay) = (330.0 + dx(), 196.5 + dy());
    g.polygon(&[(ax - 2.0, ay - 4.0), (ax + 2.5, ay), (ax - 2.0, ay + 4.0)]);
    c.glyph(t.tx(0xb4bfc7), &g);
    c.text(inst::LABEL, 338.0 + dx(), ty2, t.tx(0xd2d8dc), "Echo delay / fat mode");
}

/// FL's number box: a rounded grey block, brightest across its middle, with the value right-aligned (or `---`) and
/// small up / down arrows at its right edge; a disabled box has a faint value.
fn spin_box(c: &mut Canvas, t: &Palette, r: Rect, text: Option<&str>, st: KS) {
    c.fill_rrect(Rect::new(r.x + 1.0, r.y + 1.5, r.w - 1.0, r.h), 3.0, t.s(0x353a3d).with_alpha(160));
    let lift = if st.hover { 0x0a } else { 0 };
    let (edge, mid) = (t.s(0x4e5356 + lift * 0x010101), t.s(0x5f6467 + lift * 0x010101));
    let half = Rect::new(r.x, r.y, r.w, r.h / 2.0);
    c.fill_rect_gradient(half, edge, mid, 3.0);
    c.fill_rect_gradient(Rect::new(r.x, r.cy(), r.w, r.h / 2.0), mid, edge, 3.0);
    let (ax, cy) = (r.right() - 5.0, r.cy());
    let col = t.s(0x303538);
    let mut g = Glyph::new();
    g.polygon(&[(ax - 2.5, cy - 2.0), (ax + 2.5, cy - 2.0), (ax, cy - 5.0)]);
    g.polygon(&[(ax - 2.5, cy + 2.0), (ax + 2.5, cy + 2.0), (ax, cy + 5.0)]);
    c.glyph(col, &g);
    match text {
        None => c.fill_rect(Rect::new((r.x + r.w / 2.0 - 7.0).round(), cy.round(), 8.0, 1.0), t.tx(0x8f9598)),
        Some(s) => {
            let f = Font::light(19, 0);
            let w = c.text_width(f, s);
            let lh = c.line_height(f);
            let ink = if st.disabled { t.text_faint } else { t.tx(0xcad6da) };
            c.text(f, (r.right() - 9.0 - w).round(), (cy - lh / 2.0).round(), ink, s);
        }
    }
}

/// FL's dark push button (Cut self, Reset).
fn dark_button(c: &mut Canvas, t: &Palette, r: Rect, s: &str, hover: bool) {
    c.fill_rrect(r, 2.0, t.s(0x161718));
    let (a, b) = if hover { (t.s(0x2a2b2d), t.s(0x1f2022)) } else { (t.s(0x202123), t.s(0x18191b)) };
    c.fill_rect_gradient(r.inset(1.0), a, b, 1.5);
    c.fill_rect(Rect::new(r.x + 2.0, r.y + 1.0, r.w - 4.0, 1.0), t.s(0x27282a));
    c.text_center(inst::LABEL, r, if hover { t.tx(0xffffff) } else { t.tx(0xd2d8dc) }, s);
}

/// The kit slider with an optional fill colour from the left end to the thumb, and the thumb line in that colour.
fn slider_track(c: &mut Canvas, t: &Palette, x0: f32, x1: f32, cy: f32, value: f32, st: KS, fill: Option<Color>) -> Rect {
    let track = inst::hslider(c, t, x0, x1, cy, value, st);
    if let Some(col) = fill {
        let tx = track.x + track.w * value.clamp(0.0, 1.0);
        c.fill_rect(Rect::new(x0 + 1.0, cy - 1.0, (tx - 8.0 - x0).max(0.0), 2.0), col);
        c.line(tx, cy - 6.0, tx, cy + 6.0, 1.5, col);
    }
    track
}

/// Root-note keyboard (spec: plugins/lemon-sampler.md §3.1, same look as Lemon Sampler's).
fn keyboard(c: &mut Canvas, t: &Palette, root: u8, ui: &MiscUi) {
    let area = rect(KB_X0, KB_X1, KB_RULER, KB_BAR);
    c.push_clip(area);
    c.fill_rect(area, t.s(0x1b1d1e));
    c.fill_rect(Rect::new(area.x, KB_RULER - 1.0 + dy(), area.w, 1.0), t.s(0x1c1e21));
    let lh = c.line_height(Font::new(11));
    const WHITE_N: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
    let held = ui.held;
    for oct in 0..11 {
        let ox = ui.kb_scroll + oct as f32 * OCTAVE + dx();
        if ox > area.right() || ox + OCTAVE < area.x {
            continue;
        }
        for (i, &n) in WHITE_N.iter().enumerate() {
            let k = oct * 12 + n;
            if k > 131 {
                break;
            }
            let x = ox + i as f32 * WHITE;
            let kr = Rect::new(x + 1.0, KB_KEYS + dy(), WHITE - 1.0, KB_BOTTOM - KB_KEYS + 1.0);
            let col = key_colour(t, k, root, held);
            if n == 0 && col.is_none() {
                c.fill_rect(kr, t.tx(0xbabec5));
            } else if let Some(cc) = col {
                c.fill_rect(kr, cc);
            } else {
                c.fill_rect_gradient(kr, t.tx(0xbabec5), t.tx(0xced3da), 0.0);
            }
            if k == i32::from(root) {
                // The root's column runs through the ruler.
                c.fill_rect(Rect::new(x + 1.0, KB_RULER + dy(), WHITE - 1.0, KB_KEYS - KB_RULER), t.a(0x77a9e6));
            }
            if n == 0 {
                let s = format!("C{oct}");
                let col = if k == i32::from(root) { t.a(0x225cc3) } else { t.tx(0xd2d8dc) };
                c.text(Font::new(11), x + 2.0, (KB_RULER + 7.0 + dy() - lh / 2.0).round(), col, &s);
            }
        }
        for &(n, off) in &BLACK_OFF {
            let k = oct * 12 + n;
            if k > 131 {
                break;
            }
            let x = ox + off;
            let kr = Rect::new(x, KB_KEYS + dy(), BLACK_W, KB_BLACK - KB_KEYS);
            match key_colour(t, k, root, held) {
                Some(cc) => c.fill_rect(kr, cc),
                None => c.fill_rect_gradient(kr, t.s(0x2a2c30), t.s(0x101114), 0.0),
            }
            if k == i32::from(root) {
                c.fill_rect(Rect::new(x, KB_RULER + dy(), BLACK_W, KB_KEYS - KB_RULER), t.a(0x77a9e6));
            }
            c.fill_rect(Rect::new(x + 1.0, KB_BLACK + 1.0 + dy(), BLACK_W - 2.0, 1.0), t.tx(0xa2a5ab));
        }
    }
    // Scroll bar.
    let bar = Rect::new(area.x, KB_BOTTOM + 1.0 + dy(), area.w, KB_BAR - KB_BOTTOM);
    c.fill_rect(bar, t.s(0x1b1d1e));
    let (tx, tw) = thumb(ui);
    let lit = ui.bar_drag.is_some() || ui.hover == Some(Hit::ScrollBar);
    c.fill_rrect(Rect::new(tx + dx(), bar.y + 4.0, tw, bar.h - 8.0), 2.0, if lit { t.s(0x3c3e40) } else { t.s(0x313334) });
    c.pop_clip();
}

fn key_colour(t: &Palette, k: i32, root: u8, held: Option<u8>) -> Option<Color> {
    if held.is_some_and(|h| i32::from(h) == k) {
        Some(t.h(0xf2ad52))
    } else if k == i32::from(root) {
        Some(t.a(0x77a9e6))
    } else {
        None
    }
}

/// How far the keyboard scrolls (FL px) and the scroll bar's width.
const KB_TOTAL: f32 = 11.0 * OCTAVE;
const KB_VIEW: f32 = KB_X1 - KB_X0;
const BAR_W: f32 = KB_X1 - KB_X0 + 1.0;

/// The scroll bar thumb: its left edge (FL x) and width.
fn thumb(ui: &MiscUi) -> (f32, f32) {
    let start = (KB_X0 - ui.kb_scroll).clamp(0.0, KB_TOTAL - KB_VIEW);
    let tw = BAR_W * KB_VIEW / KB_TOTAL;
    (KB_X0 + (BAR_W - tw) * start / (KB_TOTAL - KB_VIEW), tw)
}

/// A left press on the scroll bar at page x: a press beside the thumb first centres the thumb there (Lemon Studio
/// choice), then the thumb follows the mouse (`bar_drag_to`).
pub fn bar_press(ui: &mut MiscUi, x: f32) {
    let fx = x + OX;
    let (tx, tw) = thumb(ui);
    if fx < tx || fx > tx + tw {
        set_start(ui, (fx - tw / 2.0 - KB_X0) / (BAR_W - tw) * (KB_TOTAL - KB_VIEW));
    }
    ui.bar_drag = Some((fx, KB_X0 - ui.kb_scroll));
}

/// The scroll bar's thumb dragged to page x.
pub fn bar_drag_to(ui: &mut MiscUi, x: f32) -> bool {
    let Some((fx0, start)) = ui.bar_drag else { return false };
    let tw = thumb(ui).1;
    let was = ui.kb_scroll;
    set_start(ui, start + (x + OX - fx0) / (BAR_W - tw) * (KB_TOTAL - KB_VIEW));
    ui.kb_scroll != was
}

/// Scrolls so the keyboard shows from `start` FL px past C0, kept in range.
fn set_start(ui: &mut MiscUi, start: f32) {
    ui.kb_scroll = KB_X0 - start.clamp(0.0, KB_TOTAL - KB_VIEW);
}

/// Keyboard scroll after `notches` of the wheel (34 px per notch), kept in range.
pub fn scroll_keyboard(ui: &mut MiscUi, notches: f32) {
    let total = 11.0 * OCTAVE;
    let view = KB_X1 - KB_X0;
    ui.kb_scroll = (ui.kb_scroll + notches * 34.0).clamp(KB_X0 - (total - view), KB_X0);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The keyboard's scroll bar drags (issue #91): the thumb follows the mouse, a press beside it centres it there,
    /// and the keyboard stays in range.
    #[test]
    fn keyboard_scroll_bar_drags() {
        let mut ui = MiscUi::default();
        let at = |ui: &MiscUi, fx: f32, fy: f32| hit(ui, fx - OX, fy - OY);
        let (tx, tw) = thumb(&ui);
        let y = (KB_BOTTOM + KB_BAR) / 2.0;
        assert_eq!(at(&ui, tx + tw / 2.0, y), Some(Hit::ScrollBar));
        assert_eq!(at(&ui, tx + tw / 2.0, KB_RULER + 3.0), Some(Hit::Keyboard), "the ruler is not the bar");
        let s0 = ui.kb_scroll;
        bar_press(&mut ui, tx + tw / 2.0 - OX);
        assert_eq!(ui.kb_scroll, s0, "a press on the thumb does not move it");
        assert!(bar_drag_to(&mut ui, tx + tw / 2.0 + 40.0 - OX));
        assert!(ui.kb_scroll < s0, "dragging right shows higher keys");
        assert!((thumb(&ui).0 - (tx + 40.0)).abs() < 0.01, "the thumb follows the mouse");
        bar_drag_to(&mut ui, 10_000.0);
        assert_eq!(ui.kb_scroll, KB_X0 - (KB_TOTAL - KB_VIEW), "kept in range");
        // A press at the bar's left end jumps there.
        let mut ui = MiscUi::default();
        bar_press(&mut ui, KB_X0 + 1.0 - OX);
        assert_eq!(ui.kb_scroll, KB_X0);
    }

    #[test]
    fn hits_find_the_controls() {
        let ui = MiscUi::default();
        let at = |fx: f32, fy: f32| hit(&ui, fx - OX, fy - OY);
        assert_eq!(at(37.0, 123.0), Some(Hit::Value(M::LevelPan)));
        assert_eq!(at(463.0, 121.0), Some(Hit::Value(M::KeyPan)), "the Key tracking page is showing");
        assert_eq!(at(240.0, 119.0), Some(Hit::Value(M::PolyMax)));
        assert_eq!(at(256.0, 112.0), Some(Hit::Step(M::PolyMax, 1)));
        assert_eq!(at(256.0, 126.0), Some(Hit::Step(M::PolyMax, -1)));
        assert_eq!(at(225.0, 161.0), Some(Hit::Value(M::Porta)));
        assert_eq!(at(520.0, 272.0), Some(Hit::Value(M::PingPong)));
        assert_eq!(at(250.0, 282.0), Some(Hit::Value(M::ArpChord)));
        assert_eq!(at(116.0, 196.0), Some(Hit::ArpDir(1)));
        assert_eq!(at(450.0, 88.0), Some(Hit::TrackTab(false)));
        assert_eq!(at(40.0, 290.0), Some(Hit::CutSelf));
        assert_eq!(at(30.0, 350.0), Some(Hit::Reset));
        // C5 (60) sits 2 octaves right of C3 at x 96: white key at x 278..290.
        assert_eq!(at(284.0, 430.0), Some(Hit::Key(60)));
        assert_eq!(at(278.0 + 8.0 + 3.0, 400.0), Some(Hit::Key(61)));
        assert_eq!(at(541.0, 350.0), Some(Hit::Touch));
    }

    #[test]
    fn controls_that_do_not_work_yet_are_named() {
        let mut v = MiscVals::default();
        assert_eq!(off_name(Hit::Value(M::PolyMax), &v), None);
        assert_eq!(off_name(Hit::Step(M::Cut, 1), &v), None);
        assert_eq!(off_name(Hit::Value(M::TimeSwing), &v), None);
        assert_eq!(off_name(Hit::Value(M::Fine), &v), None);
        assert_eq!(off_name(Hit::Key(60), &v), None);
        assert_eq!(off_name(Hit::Value(M::LevelVol), &v), None, "the engine plays the level adjustments");
        assert_eq!(off_name(Hit::Value(M::MainPitch), &v), None, "the main pitch switch moves the channel's pitch");
        assert_eq!(off_name(Hit::Value(M::AddKey), &v), None);
        assert_eq!(off_name(Hit::Value(M::EchoFeed), &v), Some("Echo feedback"));
        assert_eq!(off_name(Hit::Value(M::TimeGate), &v), Some("Note gate"));
        assert_eq!(off_name(Hit::Value(M::FullPorta), &v), Some("Full portamento"));
        assert_eq!(off_name(Hit::ArpDir(2), &v), Some("Arpeggiator"));
        assert_eq!(off_name(Hit::Touch, &v), Some("Multi-touch keyboard"));
        // Not a sampler: root note and fine tuning still tune the channel; only "Add to key" does nothing.
        v.sampler = false;
        assert_eq!(off_name(Hit::Value(M::Fine), &v), None);
        assert_eq!(off_name(Hit::Reset, &v), None);
        assert_eq!(off_name(Hit::Value(M::AddKey), &v), Some("Add root note to the key"));
        assert_eq!(off_name(Hit::CutSelf, &v), None);
    }

    #[test]
    fn drags_and_wheel() {
        let mut ui = MiscUi::default();
        ui.drag = Some((M::LevelVol, 100.0, 0.5));
        assert_eq!(drag_value(&ui, 0.0, 100.0 - 64.0, false), Some((M::LevelVol, 0.75)));
        assert_eq!(drag_value(&ui, 0.0, 100.0 - 64.0, true), Some((M::LevelVol, 0.5625)));
        ui.drag = Some((M::Echoes, 100.0, 4.0));
        assert_eq!(drag_value(&ui, 0.0, 84.0, false), Some((M::Echoes, 6.0)));
        assert_eq!(drag_value(&ui, 0.0, 400.0, false), Some((M::Echoes, 0.0)), "clamped");
        assert_eq!(click_value(M::Mono, 0.0), Some(1.0));
        assert_eq!(click_value(M::LevelPan, 0.5), None);
        assert_eq!(wheel_value(M::PolyMax, 0.0, 1.0), Some(1.0));
        assert_eq!(list_entries(M::PolyMax)[0].0, "---");
        assert_eq!(value_text(M::LevelPan, 0.75), "+50%");
        assert_eq!(value_text(M::Root, 60.0), "C5");
        // The arpeggiator's values as the file stores them (spec: audio/channel-misc.md "Settings").
        assert_eq!(value_text(M::ArpDir, 3.0), "up and down (bounce)");
        assert_eq!(value_text(M::ArpDir, 4.0), "up and down (sticky)");
        assert_eq!((value_text(M::ArpChord, -1.0), value_text(M::ArpChord, 0.0), value_text(M::ArpChord, 1.0)), ("Held notes".into(), "Note alone".into(), "Major".into()));
        let chords = list_entries(M::ArpChord);
        assert_eq!((chords.len(), chords[0].1, chords[chords.len() - 1].1), (90, -1.0, 88.0));
        assert_eq!((value_text(M::ArpRepeat, 1.0), list_entries(M::ArpRepeat).len()), ("1".into(), 6));
        assert_eq!(value_text(M::VolMid, 100.0), "100");
    }

    #[test]
    fn paints_the_page() {
        let mut c = Canvas::new(600, 420, 1.0).unwrap();
        let t = Palette::default();
        let mut ui = MiscUi::default();
        paint(&mut c, &t, 7.0, 0.0, &MiscVals::default(), &ui);
        ui.track_key = false;
        ui.hover = Some(Hit::Value(M::PolyMax));
        let mut v = MiscVals::default();
        v.set(M::PolyMax, 3.0);
        v.set(M::ArpDir, 4.0);
        paint(&mut c, &t, 7.0, 0.0, &v, &ui);
        v.sampler = false;
        paint(&mut c, &t, 7.0, 0.0, &v, &ui);
        let lemon = os_theme::design::palette(os_theme::Design::Lemon, None);
        paint(&mut c, &lemon, 7.0, 0.0, &v, &ui);
    }
}
