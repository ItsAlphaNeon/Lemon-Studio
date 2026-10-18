// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Playlist state and editing: tools, clips, tracks, time markers, automation points, undo.
// spec: ui/playlist.md; flp/events/playlist.md (clips, windows, markers); flp/events/automation.md

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use os_model::{
    target, AutoMode, AutoPoint, Arrangement, Automation, Channel, ChannelKind, Clip, ClipSource, PlaylistTrack, Rgb, TimeMarker,
};
use os_ui::Rect;

use crate::app::{App, Drag, State};
use crate::snap::EditorSnap;

/// Track row pitch at the default size (spec: ui/main-window.md §6); a clip fills its row less the 3 px gap.
pub const ROW_H: f32 = 48.0;
/// Track heights Alt+wheel can reach, and the step per wheel notch (§6; FL's `16 + 2 × h`, h = 0..=132, 3 per notch).
pub const ROW_MIN: f32 = 16.0;
pub const ROW_MAX: f32 = 280.0;
pub const ROW_STEP: f32 = 6.0;
/// Title strip of a clip incl. its top border (spec: ui/playlist.md §2).
pub const TITLE_H: f32 = 15.0;
/// Marker lane under the ruler, when the arrangement has time markers (§1).
pub const MARKER_H: f32 = 17.0;
/// Edge grab zone for resizing (OS choice, §3).
pub const EDGE: f32 = 6.0;
/// Number of tracks (flp playlist.md).
pub const TRACKS: u16 = 500;

/// Playlist tools (§3) and their letters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlTool {
    #[default]
    Draw,
    Paint,
    Delete,
    Mute,
    Slip,
    Slice,
    Select,
    Zoom,
    Playback,
}

impl PlTool {
    pub const ALL: [PlTool; 9] =
        [PlTool::Draw, PlTool::Paint, PlTool::Delete, PlTool::Mute, PlTool::Slip, PlTool::Slice, PlTool::Select, PlTool::Zoom, PlTool::Playback];

    pub fn from_letter(c: &str) -> Option<PlTool> {
        Some(match c.to_ascii_lowercase().as_str() {
            "p" => PlTool::Draw,
            "b" => PlTool::Paint,
            "d" => PlTool::Delete,
            "t" => PlTool::Mute,
            "s" => PlTool::Slip,
            "c" => PlTool::Slice,
            "e" => PlTool::Select,
            "z" => PlTool::Zoom,
            "y" => PlTool::Playback,
            _ => return None,
        })
    }

    pub fn hint(self) -> &'static str {
        match self {
            PlTool::Draw => "Draw (P) - click places the current clip, drag moves, edges resize, right-click deletes",
            PlTool::Paint => "Paint (B) - drag paints clips along the track",
            PlTool::Delete => "Delete (D) - click or drag deletes clips",
            PlTool::Mute => "Mute (T) - click or drag mutes / unmutes clips",
            PlTool::Slip => "Slip (S) - drag moves a clip's content inside it",
            PlTool::Slice => "Slice (C) - drag a line to cut clips",
            PlTool::Select => "Select (E) - drag a box to select (Ctrl adds)",
            PlTool::Zoom => "Zoom (Z) - drag a box to zoom, right-click toggles",
            PlTool::Playback => "Playback (Y) - hold to play from the mouse",
        }
    }
}

/// Snap settings (§4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Snap {
    None,
    #[default]
    Line,
    Cell,
    SixthStep,
    QuarterStep,
    ThirdStep,
    HalfStep,
    Step,
    SixthBeat,
    QuarterBeat,
    ThirdBeat,
    HalfBeat,
    Beat,
    Bar,
}

impl Snap {
    pub const MENU: [Snap; 14] = [
        Snap::Line,
        Snap::Cell,
        Snap::None,
        Snap::SixthStep,
        Snap::QuarterStep,
        Snap::ThirdStep,
        Snap::HalfStep,
        Snap::Step,
        Snap::SixthBeat,
        Snap::QuarterBeat,
        Snap::ThirdBeat,
        Snap::HalfBeat,
        Snap::Beat,
        Snap::Bar,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Snap::None => "(none)",
            Snap::Line => "Line",
            Snap::Cell => "Cell",
            Snap::SixthStep => "1/6 step",
            Snap::QuarterStep => "1/4 step",
            Snap::ThirdStep => "1/3 step",
            Snap::HalfStep => "1/2 step",
            Snap::Step => "Step",
            Snap::SixthBeat => "1/6 beat",
            Snap::QuarterBeat => "1/4 beat",
            Snap::ThirdBeat => "1/3 beat",
            Snap::HalfBeat => "1/2 beat",
            Snap::Beat => "Beat",
            Snap::Bar => "Bar",
        }
    }

    /// Snap unit in ticks at the zoom `bar_w` (Line / Cell = the drawn grid line).
    pub fn ticks(self, ppq: u32, bar_w: f32) -> u32 {
        let step = ppq / 4;
        match self {
            Snap::None => 1,
            Snap::Line | Snap::Cell => grid_ticks(ppq, bar_w),
            Snap::SixthStep => step / 6,
            Snap::QuarterStep => step / 4,
            Snap::ThirdStep => step / 3,
            Snap::HalfStep => step / 2,
            Snap::Step => step,
            Snap::SixthBeat => ppq / 6,
            Snap::QuarterBeat => ppq / 4,
            Snap::ThirdBeat => ppq / 3,
            Snap::HalfBeat => ppq / 2,
            Snap::Beat => ppq,
            Snap::Bar => ppq * 4,
        }
        .max(1)
    }
}

/// Ticks between the drawn grid lines: steps when a beat is wide, beats normally, bars when zoomed far out.
pub fn grid_ticks(ppq: u32, bar_w: f32) -> u32 {
    let beat_px = bar_w / 4.0;
    if beat_px >= 64.0 {
        ppq / 4
    } else if beat_px >= 6.0 {
        ppq
    } else {
        ppq * 4
    }
}

/// Picker panel lists (§1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PickKind {
    #[default]
    Pattern,
    Audio,
    Automation,
}

/// What a tool drag draws over the grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Overlay {
    Marquee(Rect),
    ZoomBox(Rect),
    /// Slice line at a tick over a span of tracks.
    Slice { tick: u32, t0: u16, t1: u16 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mark {
    None,
    Check(bool),
    Radio(bool),
}

/// Commands of the playlist's popup menus (§10).
#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    Nothing,
    /// Plugin window menu > Detached: rack row, new state (spec: ui/plugin-window.md "Docking and the window menu").
    #[cfg_attr(not(windows), allow(dead_code))]
    PluginDetach(usize, bool),
    NotYet(&'static str),
    Tool(PlTool),
    /// The playlist's magnet setting (§4).
    Snap(EditorSnap),
    /// The main toolbar's global snap (spec: ui/global-snap.md §2).
    GlobalSnap(Snap),
    Undo,
    Cut,
    Copy,
    Paste,
    Duplicate,
    Delete,
    ShiftTime(i32),
    ShiftTracks(i32),
    Mute(bool),
    MakeUnique,
    InsertSpace,
    DeleteSpace,
    /// Shift+G / Alt+G (§3a): group the selected clips / take their groups apart.
    GroupClips,
    UngroupClips,
    /// Shift+Tab: the next common snap setting (§4).
    CycleSnap,
    Consolidate,
    SelectAll,
    Deselect,
    InvertSelection,
    SelectMuted,
    SelectSource,
    SelectTime,
    ZoomIn,
    ZoomOut,
    ZoomAll,
    ZoomSelection,
    AddMarker,
    AddSignature,
    DeleteMarker(usize),
    RenameMarker(usize),
    ClipMute(usize),
    ClipDelete(usize),
    ClipUnique(usize),
    ClipEditSource(usize),
    ClipSelectSimilar(usize),
    ClipRename(usize),
    ClipRandomColor(usize),
    ClipPreview(usize),
    /// A picker item's menu (§10a) on an audio or automation clip source: the command and the source's channel index.
    Pick(PickCmd, u16),
    PointDelete(u16, usize),
    PointMode(u16, usize, AutoMode),
    PointValue(u16, usize),
    TrackRename(u16),
    TrackMute(u16),
    TrackSolo(u16),
    /// Group the track with the one above it, or take it out of its group (§6a).
    TrackGroup(u16),
    /// Arrangements (§12): show arrangement `n`, add an empty one, clone / rename / delete the current one, copy
    /// arrangement `n`'s clips into the current one.
    ArrSelect(u16),
    ArrAdd,
    ArrClone,
    ArrRename,
    ArrDelete,
    ArrMerge(u16),
    /// Tools (§10): render every track to its own file; move clip starts onto the snap grid.
    ExportTracks,
    QuantizeStarts,
    /// Shift+M: resizing audio clips stretches them (§9b).
    StretchMode,
    /// Collapse / expand the group the track leads or belongs to (§6a).
    GroupCollapse(u16),
    TrackUp(u16),
    TrackDown(u16),
    TrackInsert(u16),
    TrackDelete(u16),
    TrackColor(u16, Option<Rgb>),
    TrackMuteClips(u16, bool),
    /// "Create automation clip" on a control: target, name, current value 0..1.
    CreateAutomation(u32, String, f64),
    /// "Init song with this position" on a control: target, current value 0..1; "Delete initial value": target
    /// (`crate::init`).
    InitControl(u32, f64),
    DeleteInit(u32),
    ResetEmptyTracks,
    /// Browser options menu (spec: ui/browser.md §4).
    Browser(crate::browser::BCmd),
    /// Plugin menus (spec: ui/plugin-menu.md).
    Plugin(crate::plugin_menu::PlugCmd),
    /// Channel rack menus (spec: ui/channel-rack.md §17-21).
    Rack(crate::rack::RCmd),
    /// Mixer menus (spec: ui/mixer.md §6-7).
    Mixer(crate::mixer::MCmd),
    /// Plugin parameter menus (spec: plugins/plugin-parameters.md §5-6).
    Param(crate::params::PCmd),
    /// "Type in value..." of a knob or parameter menu (spec: ui/type-in-value.md).
    TypeIn(crate::typein::Target),
    /// Plugin presets (spec: plugins/presets.md).
    #[cfg_attr(not(windows), allow(dead_code))]
    Preset(crate::presets::PCmd),
    /// A Misc tab list entry: rack row, value, new value (spec: ui/plugin-window.md "Misc tab").
    #[cfg_attr(not(windows), allow(dead_code))]
    Misc(usize, crate::misc_page::M, f32),
    /// Piano roll Tools menu (spec: ui/piano-roll.md §11).
    Piano(crate::piano_tools::NCmd),
    /// Settings > Audio menus: device, sample rate, buffer length (spec: ui/settings.md "Audio").
    Audio(crate::audio_cfg::Set),
    /// The toolbar CPU panel's menu (spec: ui/main-window.md §13).
    Cpu(crate::cpu::CpuCmd),
    /// The metronome button's menu: the click sound (spec: ui/metronome.md §2).
    MetronomeSound(os_engine::metronome::Sound),
    /// Settings > Project ▸ Numerator / Denominator: the project's bar, beats per bar and steps per beat (events 17,
    /// 18; spec: ui/settings-choices.md "Project").
    Bar(u16, u16),
    /// Settings > Project ▸ Set as: time signature (true) or time division (event 35).
    TimeMode(bool),
    /// Settings > Project ▸ Timebase (PPQ).
    Timebase(u16),
    /// Settings > Project ▸ Advanced ▸ Panning law: event 23's value (0 Circular, 2 Triangular).
    PanningLaw(u8),
    /// The auto-scrolling button's menu (spec: ui/auto-scrolling.md §2).
    ContinuousScrolling,
    /// A PATTERNS command from a pattern's right-click menu in the picker (spec: ui/pattern-menu.md §3).
    Pattern(crate::patterns::Cmd),
    /// The time display's mode, and its middle unit (Beat when true) (spec: ui/time-display.md §1).
    ClockMode(crate::clock::Mode),
    ClockBeat(bool),
    /// A Toolbars menu switch (spec: ui/toolbar-menu.md §8).
    Toolbar(crate::views::toolbar::Switch),
    /// A value picked in a list of the time signature dialog: denominator (true) or numerator (spec:
    /// ui/pattern-time-signature.md §1).
    TimeSig(bool, u8),
    /// A settings page combo declared in `crate::prefs`: that entry of its choices.
    Pref(&'static crate::prefs::Combo, usize),
    /// Settings > Info ▸ the genre menu: a genre, None for "(none)" (spec: ui/settings-choices.md "Info").
    Genre(Option<&'static str>),
    /// Settings > General ▸ Default template: "Empty" or "<category>/<template>" (`crate::templates`).
    Template(String),
    /// Settings > General ▸ Hints and warnings ▸ the Manage messages menu (`crate::messages`).
    Messages(crate::messages::Pick),
    /// Settings > Project ▸ Data folder's menus and the forms' lists (`crate::data_folder`).
    DataFolder(crate::data_folder::DCmd),
}

#[derive(Debug, Clone, PartialEq)]
pub struct PItem {
    pub label: Cow<'static, str>,
    pub key: &'static str,
    pub cmd: Cmd,
    pub mark: Mark,
    pub enabled: bool,
    /// Drawn dim though it works (Manage messages: an entry that is off).
    pub dim: bool,
    /// Section header (not clickable).
    pub head: bool,
    pub sep: bool,
    /// Column break: the following items start a new column (not drawn).
    pub col: bool,
    pub children: Vec<PItem>,
}

impl PItem {
    pub fn new(label: impl Into<Cow<'static, str>>, cmd: Cmd) -> Self {
        PItem { label: label.into(), key: "", cmd, mark: Mark::None, enabled: true, dim: false, head: false, sep: false, col: false, children: Vec::new() }
    }
    pub fn key(mut self, k: &'static str) -> Self {
        self.key = k;
        self
    }
    pub fn mark(mut self, m: Mark) -> Self {
        self.mark = m;
        self
    }
    pub fn off(mut self) -> Self {
        self.enabled = false;
        self
    }
    pub fn dim(mut self, on: bool) -> Self {
        self.dim = on;
        self
    }
    pub fn head(label: &'static str) -> Self {
        PItem { head: true, ..PItem::new(label, Cmd::Nothing) }
    }
    pub fn sep() -> Self {
        PItem { sep: true, ..PItem::new("", Cmd::Nothing) }
    }
    pub fn col() -> Self {
        PItem { col: true, ..PItem::new("", Cmd::Nothing) }
    }
    pub fn sub(label: &'static str, children: Vec<PItem>) -> Self {
        PItem { children, ..PItem::new(label, Cmd::Nothing) }
    }
    pub fn not_yet(label: &'static str) -> Self {
        PItem::new(label, Cmd::NotYet(label)).off()
    }
}

/// A popup menu (the playlist ▸ menu, the snap menu, clip / point / track / knob menus).
#[derive(Debug, Clone, PartialEq)]
pub struct Popup {
    pub x: f32,
    pub y: f32,
    pub items: Vec<PItem>,
    /// Open submenu: index of its parent item.
    pub sub: Option<usize>,
}

/// What an inline text box edits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EditTarget {
    TrackName(u16),
    MarkerName(usize),
    NewMarker(u32),
    PointValue(u16, usize),
    PatternName(u16),
    /// Mixer track name (spec: ui/mixer.md §2) and a mixer slot's plugin name (§7).
    MixerName(u16),
    SlotName(u16, u8),
    /// The current arrangement's name (§12).
    ArrangementName,
    /// The value box of "Type in value..." and whether it reads normalized values (spec: ui/type-in-value.md).
    TypeIn(crate::typein::Target, bool),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextEdit {
    pub target: EditTarget,
    pub text: String,
    /// Where the box is drawn (screen).
    pub at: (f32, f32),
    /// The whole text is selected: typing replaces it (the value box opens so, spec: ui/type-in-value.md §1).
    pub sel: bool,
}

/// A waveform summary for drawing audio clips: min/max per `per` frames (left channel + right mixed).
#[derive(Debug, Clone)]
pub struct Wave {
    pub rate: u32,
    pub frames: usize,
    pub per: usize,
    pub peaks: Vec<(f32, f32)>,
}

impl Wave {
    pub fn secs(&self) -> f64 {
        self.frames as f64 / f64::from(self.rate.max(1))
    }
}

/// What a playlist undo step keeps (`crate::history`): the shown arrangement's clips, markers and tracks, the
/// automation data and the channels' stretch settings.
pub(crate) type Snapshot = (Vec<Clip>, Vec<TimeMarker>, Vec<PlaylistTrack>, Vec<Automation>, Vec<[u32; 4]>);

pub struct PlaylistState {
    pub tool: PlTool,
    /// The magnet's setting (§4); Main follows the global snap (spec: ui/global-snap.md).
    pub snap: EditorSnap,
    /// The current clip source (what Draw / Paint place); `None` = the current pattern.
    pub source: Option<ClipSource>,
    pub picker: PickKind,
    /// Picker list scroll, pixels.
    pub pick_scroll: f32,
    /// Selected clips (indices into the first arrangement's clips).
    pub sel: Vec<usize>,
    /// Selected track (Ctrl+Up / Down, header click).
    pub track: u16,
    /// Song position (ticks): song playback starts here.
    pub pos: u32,
    /// Time selection (ticks).
    pub range: Option<(u32, u32)>,
    pub clipboard: Vec<Clip>,
    pub overlay: Option<Overlay>,
    pub popup: Option<Popup>,
    pub edit: Option<TextEdit>,
    /// The value box's mode last accepted: normalized (spec: ui/type-in-value.md §3).
    pub typein_norm: bool,
    /// Soloed track: its own state and the others' before the solo.
    pub solo: Option<(u16, Vec<bool>)>,
    /// Zoom tool: the view before "zoom out" (bar width, scroll x, scroll y).
    pub zoom_back: Option<(f32, f32, f32)>,
    /// Automation or audio clip under the mouse (its points / handles are drawn).
    pub hover_clip: Option<usize>,
    /// The audio clip handle under the mouse (drawn highlighted).
    pub hover_handle: Option<Handle>,
    /// Where the arrangement name sits in the title bar (its rename box opens there).
    pub arr_anchor: (f32, f32),
    /// Stretch mode (Shift+M, §9b): resizing an audio clip stretches it; while a resize runs, how many beats each
    /// audio clip's channel's whole sample lasted when it started.
    pub stretch: bool,
    pub stretch_base: Vec<(u16, f64)>,
    /// A resized clip's length, kept for its source: new clips of that source are placed / painted at this length
    /// until a clip of natural length is clicked (OS, as FL does).
    pub lock_len: Option<(ClipSource, u32)>,
    /// Track row pitch (vertical zoom: Alt+wheel; saved in the project's playlist view).
    pub row_h: f32,
    /// Last header click (track, time) for double-click rename.
    pub last_header: Option<(u16, std::time::Instant)>,
    /// Last left click on an audio clip (clip, time): a second one soon after opens its sampler.
    pub last_clip: Option<(usize, std::time::Instant)>,
    /// Waveform cache by sample path (`None` = could not be loaded).
    pub waves: RefCell<HashMap<String, Option<Rc<Wave>>>>,
    /// Tones of the same samples for colourful waveforms, analysed when first needed (`tones`).
    pub tones: RefCell<HashMap<String, Option<Rc<os_dsp::tone::Tones>>>>,
    /// A project is loading and its files decode off the UI thread: waveforms wait (spec: ui/project-loading.md).
    pub wave_wait: bool,
}

impl Default for PlaylistState {
    fn default() -> Self {
        PlaylistState {
            tool: PlTool::Draw,
            snap: EditorSnap::Main,
            source: None,
            picker: PickKind::Pattern,
            pick_scroll: 0.0,
            sel: Vec::new(),
            track: 0,
            pos: 0,
            range: None,
            clipboard: Vec::new(),
            overlay: None,
            popup: None,
            edit: None,
            typein_norm: false,
            solo: None,
            zoom_back: None,
            hover_clip: None,
            hover_handle: None,
            arr_anchor: (300.0, 90.0),
            stretch: false,
            stretch_base: Vec::new(),
            lock_len: None,
            row_h: ROW_H,
            last_header: None,
            last_clip: None,
            waves: RefCell::new(HashMap::new()),
            tones: RefCell::new(HashMap::new()),
            wave_wait: false,
        }
    }
}

impl PlaylistState {
    /// Forget everything tied to the previous project (keeps tool and snap; the track height comes with the
    /// project, `load_view`).
    pub fn reset(&mut self) {
        let (tool, snap) = (self.tool, self.snap);
        *self = PlaylistState { tool, snap, ..PlaylistState::default() };
    }

    /// The waveform of a sample path (loaded and summarised once).
    pub fn wave(&self, path: &str) -> Option<Rc<Wave>> {
        if let Some(w) = self.waves.borrow().get(path) {
            return w.clone();
        }
        if self.wave_wait {
            return None;
        }
        // The decode is shared with the sampler playing the file (decoded once, while the project loaded).
        let file = crate::plugins::resolve_sample(path);
        let w = os_engine::sample::load_shared(std::path::Path::new(&file)).ok().map(|s| {
            let per = 64usize;
            let peaks = s
                .left
                .chunks(per)
                .zip(s.right.chunks(per))
                .map(|(l, r)| {
                    // The chunk's own range (a DC sample is one value, not a band from zero; §9a signed waveforms).
                    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
                    for (a, b) in l.iter().zip(r) {
                        let v = (a + b) * 0.5;
                        lo = lo.min(v);
                        hi = hi.max(v);
                    }
                    (lo, hi)
                })
                .collect();
            Rc::new(Wave { rate: s.rate, frames: s.frames(), per, peaks })
        });
        self.waves.borrow_mut().insert(path.to_string(), w.clone());
        w
    }

    /// The tones of a sample path, block by block (spec: ui/settings-choices.md "Display: Colorful waveforms
    /// (2026-10-16)"): analysed once, the first time a waveform is drawn in colour, and kept with the peaks.
    pub fn tones(&self, path: &str) -> Option<Rc<os_dsp::tone::Tones>> {
        if let Some(t) = self.tones.borrow().get(path) {
            return t.clone();
        }
        if self.wave_wait {
            return None;
        }
        let file = crate::plugins::resolve_sample(path);
        let t = os_engine::sample::load_shared(std::path::Path::new(&file))
            .ok()
            .map(|s| Rc::new(os_dsp::tone::Tones::analyse_stereo(&s.left, &s.right, s.rate)));
        self.tones.borrow_mut().insert(path.to_string(), t.clone());
        t
    }
}

// ---------------------------------------------------------------------------------------------------------
// Geometry

/// Height of a member row of a collapsed group: a thin strip showing its clips as bars (§6a).
pub const STRIP_H: f32 = 6.0;

/// Vertical layout of the tracks (§6a): every track's top in content pixels. Rows are `row_h` tall (`ROW_H` unless
/// zoomed); members of a collapsed group are `STRIP_H` strips.
#[derive(Debug, Clone, PartialEq)]
pub struct Rows {
    top: Vec<f32>,
    pub row_h: f32,
}

impl Rows {
    pub fn new(s: &State) -> Rows {
        let tracks = s.project.arrangement().map(|a| a.tracks.as_slice()).unwrap_or(&[]);
        // On the drawing grid, so tops and heights add up exactly (`PGeom::xs64`).
        let row_h = snap(f64::from(s.playlist.row_h.clamp(ROW_MIN, ROW_MAX)));
        let mut top = Vec::with_capacity(usize::from(TRACKS) + 1);
        let (mut y, mut collapsed) = (0.0, false);
        for t in 0..usize::from(TRACKS) {
            top.push(y);
            let tr = tracks.get(t);
            let grouped = t > 0 && tr.is_some_and(|x| x.grouped);
            if !grouped {
                collapsed = tr.is_some_and(|x| x.collapsed);
            }
            y += if grouped && collapsed { STRIP_H } else { row_h };
        }
        top.push(y);
        Rows { top, row_h }
    }

    /// A clip's height: its row less the 3 px gap.
    pub fn clip_h(&self) -> f32 {
        self.row_h - 3.0
    }

    pub fn top(&self, t: u16) -> f32 {
        self.top[usize::from(t.min(TRACKS))]
    }

    pub fn height(&self, t: u16) -> f32 {
        let i = usize::from(t.min(TRACKS - 1));
        self.top[i + 1] - self.top[i]
    }

    /// A collapsed group's member: drawn as a strip.
    pub fn strip(&self, t: u16) -> bool {
        self.height(t) < self.row_h - 0.5
    }

    /// The track at content height `y`.
    pub fn at(&self, y: f32) -> u16 {
        self.top.partition_point(|&v| v <= y).saturating_sub(1).min(usize::from(TRACKS) - 1) as u16
    }

    pub fn total(&self) -> f32 {
        self.top[usize::from(TRACKS)]
    }
}

/// Mapping between grid pixels and ticks / tracks for one frame.
#[derive(Debug, Clone)]
pub struct PGeom {
    pub grid: Rect,
    /// Pixels per tick.
    pub ppt: f32,
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub ppq: u32,
    pub bar_w: f32,
    pub rows: std::rc::Rc<Rows>,
}

/// `v` on the grid of 1/256 px that scrolled positions are drawn at (`PGeom::xs`).
pub fn snap64(v: f64) -> f64 {
    (v * 256.0 + 0.5).floor() / 256.0
}

/// [`snap64`] as a drawing coordinate.
pub fn snap(v: f64) -> f32 {
    snap64(v) as f32
}

impl PGeom {
    pub fn new(s: &State, grid: Rect) -> PGeom {
        let ppq = u32::from(s.project.ppq.max(4));
        let bar_w = s.playlist_bar_w;
        let rows = std::rc::Rc::new(Rows::new(s));
        PGeom { grid, ppt: bar_w / (ppq * 4) as f32, scroll_x: s.playlist_scroll.0, scroll_y: s.playlist_scroll.1, ppq, bar_w, rows }
    }

    /// Screen x of content x `px` (pixels from the song start), exact on the 1/256 px grid. Every position the
    /// playlist draws is such a point plus a fixed offset, computed in f64: a scroll by whole device pixels then moves
    /// every shape by exactly that much, so moving the drawn pixels (`App::playlist_scrolled`) equals a repaint.
    pub fn xs64(&self, px: f64) -> f64 {
        snap64(f64::from(self.grid.x) - f64::from(self.scroll_x) + px)
    }

    pub fn xs(&self, px: f64) -> f32 {
        self.xs64(px) as f32
    }

    /// Screen x of `tick`, unrounded to f32 (see [`PGeom::xs64`]).
    pub fn x64(&self, tick: f64) -> f64 {
        self.xs64(tick * f64::from(self.ppt))
    }

    pub fn x_of(&self, tick: f32) -> f32 {
        self.x64(f64::from(tick)) as f32
    }

    pub fn tick_at(&self, x: f32) -> f32 {
        (x - self.grid.x + self.scroll_x) / self.ppt
    }

    pub fn y_of(&self, track: u16) -> f32 {
        snap(f64::from(self.grid.y) - f64::from(self.scroll_y) + f64::from(self.rows.top(track)))
    }

    pub fn track_at(&self, y: f32) -> u16 {
        self.rows.at(y - self.grid.y + self.scroll_y)
    }

    /// A clip's rectangle: the row less its 3 px gap, or a thin bar on a collapsed group's strip (§6a).
    pub fn clip_rect(&self, c: &Clip) -> Rect {
        let x = self.x64(f64::from(c.position));
        let w = ((self.x64(f64::from(c.position) + f64::from(c.length)) - x) as f32).max(3.0);
        let x = x as f32;
        if self.rows.strip(c.track) {
            return Rect::new(x, self.y_of(c.track) + 1.0, w, STRIP_H - 2.0);
        }
        Rect::new(x, self.y_of(c.track), w, self.rows.clip_h())
    }

    /// `clip_rect` for drawing: an edge further than 2048 px outside the grid is pulled in to that distance, so every
    /// drawn position stays small and exact on the drawing grid (`PGeom::xs64`); nothing drawn from an edge reaches
    /// that far. Also the clip's left edge in f64, where the content is measured from.
    pub fn clip_draw_rect(&self, c: &Clip) -> (Rect, f64) {
        const FAR: f64 = 2048.0;
        let r = self.clip_rect(c);
        let x = self.x64(f64::from(c.position));
        let x0 = x.max(f64::from(self.grid.x) - FAR);
        let x1 = (x + f64::from(r.w)).min(f64::from(self.grid.right()) + FAR);
        (Rect::new(x0 as f32, r.y, (x1 - x0).max(0.0) as f32, r.h), x)
    }

    /// The body of a clip below its title strip (automation curves, notes, waveforms).
    pub fn body(&self, c: &Clip) -> Rect {
        let r = self.clip_rect(c);
        Rect::new(r.x, r.y + TITLE_H, r.w, (self.rows.clip_h() - TITLE_H - 1.0).max(1.0))
    }
}

// ---------------------------------------------------------------------------------------------------------
// Model helpers

/// The arrangement being edited, created (with FL's 500 default tracks) when the project has none.
pub fn arr_mut(s: &mut State) -> &mut Arrangement {
    if s.project.arrangements.is_empty() {
        s.project.arrangements.push(os_flp::project::default_arrangement());
    }
    let a = s.project.arrangement_mut().expect("not empty");
    while a.tracks.len() < usize::from(TRACKS) {
        let n = a.tracks.len() as u32 + 1;
        a.tracks.push(PlaylistTrack::new(n));
    }
    a
}

pub fn clips(s: &State) -> &[Clip] {
    s.project.arrangement().map(|a| a.clips.as_slice()).unwrap_or(&[])
}

pub fn markers(s: &State) -> &[TimeMarker] {
    s.project.arrangement().map(|a| a.markers.as_slice()).unwrap_or(&[])
}

/// The group track leading track `t`'s group (§6a): the nearest ungrouped track above a grouped track.
pub fn group_track(s: &State, t: u16) -> Option<u16> {
    if t == 0 || !track_info(s, t).is_some_and(|x| x.grouped) {
        return None;
    }
    (0..t).rev().find(|&k| !track_info(s, k).is_some_and(|x| x.grouped))
}

/// Track `t` leads a group: it is not grouped and the track below it is.
pub fn is_group_track(s: &State, t: u16) -> bool {
    !track_info(s, t).is_some_and(|x| x.grouped) && track_info(s, t + 1).is_some_and(|x| x.grouped)
}

pub fn track_info(s: &State, t: u16) -> Option<&PlaylistTrack> {
    s.project.arrangement().and_then(|a| a.tracks.get(usize::from(t)))
}

pub fn track_on(s: &State, t: u16) -> bool {
    track_info(s, t).is_none_or(|t| t.enabled)
}

pub fn track_name(s: &State, t: u16) -> String {
    track_info(s, t).and_then(|t| t.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| format!("Track {}", t + 1))
}

/// The current clip source (the current pattern unless another was picked).
pub fn source(s: &State) -> ClipSource {
    s.playlist.source.unwrap_or(ClipSource::Pattern(s.pattern.max(1)))
}

fn ppq(s: &State) -> u32 {
    u32::from(s.project.ppq.max(4))
}

fn bar(s: &State) -> u32 {
    ppq(s) * 4
}

fn channel_of(s: &State, ch: u16) -> Option<&Channel> {
    s.project.channel(ch)
}

/// The rack row of the channel an audio clip plays (`None` for pattern and automation clips).
fn audio_clip_row(s: &State, c: &Clip) -> Option<usize> {
    let ClipSource::Channel(ch) = c.source else { return None };
    s.project.channels.iter().position(|x| x.index == ch && x.kind.is_sampler())
}

pub fn source_name(s: &State, src: ClipSource) -> String {
    match src {
        ClipSource::Pattern(n) => s.project.pattern(n).and_then(|p| p.name.clone()).unwrap_or_else(|| format!("Pattern {n}")),
        ClipSource::Channel(c) => channel_of(s, c).and_then(|ch| ch.name.clone()).unwrap_or_else(|| format!("Channel {}", c + 1)),
    }
}

pub fn source_color(s: &State, src: ClipSource) -> Option<Rgb> {
    match src {
        ClipSource::Pattern(n) => s.project.pattern(n).and_then(|p| p.color),
        ClipSource::Channel(c) => channel_of(s, c).and_then(|ch| ch.color),
    }
}

/// The colour an audio or automation clip's title strip and waveform take: its channel's colour as the file stores
/// it (custom or not), else a pastel picked from the channel's name (spec: ui/playlist.md §2, "Channel-colour rule").
/// `None` for pattern clips (neutral titles).
pub fn clip_tint(s: &State, src: ClipSource) -> Option<Rgb> {
    let ClipSource::Channel(c) = src else { return None };
    let ch = channel_of(s, c)?;
    ch.color.or(ch.rack.stored_color).or_else(|| Some(pastel(ch.name.as_deref().or(ch.sample_path.as_deref()).unwrap_or(""))))
}

/// A pastel of FL's audio-clip family (one component 0x68, one 0x7e, the third between), its hue from a hash of
/// `seed` (a Lemon Studio choice for clips whose file has no colour).
pub fn pastel(seed: &str) -> Rgb {
    let h = seed.bytes().fold(2_166_136_261u32, |h, b| (h ^ u32::from(b)).wrapping_mul(16_777_619));
    let hue = (h % 360) as f32 / 60.0;
    let (lo, hi) = (f32::from(0x68u8), f32::from(0x7eu8));
    let mid = lo + (hi - lo) * (1.0 - (hue % 2.0 - 1.0).abs());
    let (r, g, b) = match hue as u32 {
        0 => (hi, mid, lo),
        1 => (mid, hi, lo),
        2 => (lo, hi, mid),
        3 => (lo, mid, hi),
        4 => (mid, lo, hi),
        _ => (hi, lo, mid),
    };
    Rgb { r: r.round() as u8, g: g.round() as u8, b: b.round() as u8 }
}

pub fn source_kind(s: &State, src: ClipSource) -> PickKind {
    match src {
        ClipSource::Pattern(_) => PickKind::Pattern,
        ClipSource::Channel(c) => match channel_of(s, c).map(|ch| ch.kind) {
            Some(ChannelKind::Automation) => PickKind::Automation,
            _ => PickKind::Audio,
        },
    }
}

/// How fast an audio clip's channel goes through its sample: its stretch in any method (§9b, audio/time-stretch.md),
/// times the channel's pitch where that moves the sample (no stretch time, not a realtime method), as the engine
/// plays it (audio/audio-clips.md "Length for the song").
pub fn audio_speed(s: &State, src: ClipSource) -> f64 {
    let ClipSource::Channel(c) = src else { return 1.0 };
    let Some(ch) = channel_of(s, c) else { return 1.0 };
    let sound = &ch.sound;
    let stretch = if sound.stretch == [0; 4] {
        1.0
    } else {
        // A fitted time needs the sample's duration (the multiplier alone does not).
        let duration = if sound.stretch_time().is_some() { sample_secs(s, ch) } else { 0.0 };
        1.0 / sound.stretch_factor(duration, s.project.tick_tempo(s.tempo))
    };
    let pitched = sound.channel_pitch_applies() && !os_model::stretch_mode::realtime(sound.stretch_mode());
    if !pitched {
        return stretch;
    }
    let main = ch.misc.main_pitch.then(|| i32::from(s.project.main_pitch.unwrap_or(0)));
    let cents = f64::from(os_engine::laws::channel_pitch(f64::from(ch.pitch_cents), ch.misc.fine, ch.misc.root, main)) + 6000.0;
    stretch * os_engine::laws::cents_rate(cents)
}

/// Works every audio clip's length out again from its trims and its channel's stretch, pitch and the tempo, as FL
/// does (the stored length is not used for an audio clip): each trim point becomes ticks on its own, rounded; an
/// untrimmed clip spans the whole sample (ui/playlist.md §9c, audio/audio-clips.md). Clips whose sample is not
/// loaded keep their length. Returns whether a length changed. (The app's tick uses `audio_fits` + `fit_clips`.)
#[cfg(test)]
pub fn fit_audio(s: &mut State) -> bool {
    audio_fits(s).is_some_and(|fits| fit_clips(&mut s.project.arrangements, &fits))
}

/// Per audio channel, the whole sample in ticks and in ms; `None` for a channel whose sample is not loaded.
pub type Fits = HashMap<u16, Option<(f64, f64)>>;

/// The fits of every channel with clips (`fit_audio`); `None` while waveforms are loading.
pub fn audio_fits(s: &State) -> Option<Fits> {
    if s.playlist.wave_wait {
        return None;
    }
    let mut per = Fits::new();
    for a in &s.project.arrangements {
        for c in &a.clips {
            let ClipSource::Channel(ch) = c.source else { continue };
            if per.contains_key(&ch) {
                continue;
            }
            let fit = (source_kind(s, c.source) == PickKind::Audio)
                .then(|| channel_of(s, ch))
                .flatten()
                .map(|x| (audio_ticks(s, x), sample_secs(s, x) * 1000.0))
                .filter(|&(t, ms)| ms > 0.0 && t > 0.0 && t.is_finite());
            per.insert(ch, fit);
        }
    }
    Some(per)
}

/// Automatic crossfades follow the clips and the tempo (spec: audio/audio-clips.md "Automatic crossfades"): set again
/// from the overlaps on the tick an edit reaches playback (clips moved, resized, added, deleted or split, the tempo,
/// undo and redo, a project opened, the switch turned on), before the next undo step is taken, so they undo and redo
/// with the edit that made them. As with the fits, the saved copy gets them too: an opened project's crossfades are
/// not an unsaved change.
pub fn follow_crossfades(app: &mut App) {
    if !app.state.piano.dirty || !os_model::crossfade::derive(&mut app.state.project, app.state.tempo) {
        return;
    }
    os_model::crossfade::derive(&mut app.saved.0, app.saved.1);
    app.damage_win(crate::app::Win::Playlist);
}

/// The project's automatic crossfades were switched (`App::toggle_auto_crossfades`): turned off, the crossfades'
/// lengths go to 0 and their tensions stay; turned on, they are set from the overlaps on the next tick.
pub fn crossfades_switched(s: &mut State) {
    if !os_model::crossfade::on(&s.project) {
        os_model::crossfade::clear(&mut s.project);
    }
    s.piano.dirty = true;
}

/// Gives the audio clips of `arrangements` their lengths from `fits` (`fit_audio`). Returns whether one changed.
pub fn fit_clips(arrangements: &mut [os_model::Arrangement], fits: &Fits) -> bool {
    let mut changed = false;
    for a in arrangements {
        for c in &mut a.clips {
            let ClipSource::Channel(ch) = c.source else { continue };
            let Some(&Some((t, ms))) = fits.get(&ch) else { continue };
            let (w0, w1) = c.window.unwrap_or((0.0, ms));
            // Scaled from the whole length, so a point that falls on half a tick stays a tie and rounds up as in FL.
            let tick = |w: f64| (w * t / ms).round();
            let len = (tick(w1) - tick(w0)).max(1.0) as u32;
            if c.length != len {
                c.length = len;
                changed = true;
            }
        }
    }
    changed
}

/// The channel's whole sample in ticks, unrounded: the stretch time (whole 1/192 beats) or the sample's duration at
/// the tempo, times the multiplier, over the channel pitch's rate where that moves the sample (`audio_speed`). 0 while
/// the waveform is not loaded.
fn audio_ticks(s: &State, ch: &os_model::Channel) -> f64 {
    let secs = sample_secs(s, ch);
    if secs <= 0.0 {
        return 0.0;
    }
    let sound = &ch.sound;
    let ppq = f64::from(ppq(s));
    let base = match sound.stretch_time() {
        Some(_) => f64::from(f32::from_bits(sound.stretch[0])) * ppq / 192.0,
        None => secs * s.project.tick_tempo(s.tempo.max(1.0)) / 60.0 * ppq,
    };
    let ticks = base * sound.stretch_multiplier();
    if !sound.channel_pitch_applies() || os_model::stretch_mode::realtime(sound.stretch_mode()) {
        return ticks;
    }
    let main = ch.misc.main_pitch.then(|| i32::from(s.project.main_pitch.unwrap_or(0)));
    let cents = f64::from(os_engine::laws::channel_pitch(f64::from(ch.pitch_cents), ch.misc.fine, ch.misc.root, main)) + 6000.0;
    ticks / os_engine::laws::cents_rate(cents)
}

/// Seconds the channel's sample lasts (0 while its waveform is not loaded).
fn sample_secs(s: &State, ch: &os_model::Channel) -> f64 {
    ch.sample_path.as_deref().and_then(|p| s.playlist.wave(p)).map_or(0.0, |w| w.secs())
}

/// Seconds per tick at the project tempo (the ticks run at `Project::tick_tempo`).
fn secs_per_tick(s: &State) -> f64 {
    60.0 / s.project.tick_tempo(s.tempo.max(1.0)) / f64::from(ppq(s))
}

/// Natural length of a source in ticks: a pattern's length (its bars, the engine's rule: `Project::pattern_length`,
/// spec: audio/song-structure.md), a sample's duration, an automation's last point.
pub fn source_len(s: &State, src: ClipSource) -> u32 {
    let bar = bar(s);
    match src {
        ClipSource::Pattern(n) => s.project.pattern_length(n),
        ClipSource::Channel(c) => {
            if let Some(a) = s.project.automation(c) {
                let beats = a.points.last().map_or(0.0, |p| p.beat);
                return ((beats * f64::from(ppq(s))).round() as u32).max(bar / 4);
            }
            // The whole sample at the channel's speed (`fit_audio`).
            let t = channel_of(s, c).map_or(0.0, |ch| audio_ticks(s, ch));
            if t > 0.0 && t.is_finite() { (t.round() as u32).max(1) } else { bar }
        }
    }
}

/// Pattern `n`'s length changed from `old` (its time signature, spec: ui/pattern-time-signature.md §5): its clips that
/// were not resized by hand (uncut, at the old natural length) take the new one, in every arrangement.
pub fn refit_pattern_clips(s: &mut State, n: u16, old: u32) {
    let new = s.project.pattern_length(n);
    for a in &mut s.project.arrangements {
        for c in a.clips.iter_mut().filter(|c| c.source == ClipSource::Pattern(n) && c.window.is_none() && c.length == old) {
            c.length = new;
        }
    }
    s.piano.dirty = true;
}

/// A clip's window in its source's unit (pattern ticks, audio ms, automation beats), filled in when unset.
pub fn window(s: &State, c: &Clip) -> (f64, f64) {
    c.window.unwrap_or_else(|| (0.0, to_src(s, c.source, f64::from(c.length))))
}

/// Ticks to the source's unit.
pub fn to_src(s: &State, src: ClipSource, ticks: f64) -> f64 {
    match source_kind(s, src) {
        PickKind::Pattern => ticks,
        PickKind::Audio => ticks * secs_per_tick(s) * 1000.0 * audio_speed(s, src),
        PickKind::Automation => ticks / f64::from(ppq(s)),
    }
}

/// Undo snapshot of everything the playlist edits: a step of the project's undo history (`crate::history`).
pub fn snapshot(s: &mut State) {
    snapshot_as(s, "Playlist edit");
}

/// `snapshot`, the step named `name` (as the browser's History lists it; spec: ui/settings-choices.md "Undo history:
/// the four checks (2026-10-16)").
pub fn snapshot_as(s: &mut State, name: &'static str) {
    arr_mut(s);
    let snap = crate::history::Snap::Playlist(Box::new(capture(s)));
    crate::history::record(s, name, snap);
}

/// `snapshot_as` for a command that changes more than the playlist: one step named `name`, holding the playlist
/// and `extra`, what the command changes beyond it.
pub fn snapshot_with(s: &mut State, name: &'static str, extra: crate::history::Snap) {
    arr_mut(s);
    let snap = crate::history::Snap::Several(vec![crate::history::Snap::Playlist(Box::new(capture(s))), extra]);
    crate::history::record(s, name, snap);
}

/// The playlist's undo state now (`snapshot`).
pub(crate) fn capture(s: &State) -> Snapshot {
    let (clips, markers, tracks) = s.project.arrangement().map(|a| (a.clips.clone(), a.markers.clone(), a.tracks.clone())).unwrap_or_default();
    let stretch = s.project.channels.iter().map(|c| c.sound.stretch).collect();
    (clips, markers, tracks, s.project.automations.clone(), stretch)
}

/// Puts an undo state back (undo and redo, `crate::history`).
pub(crate) fn restore(s: &mut State, (c, m, t, a, st): Snapshot) {
    let arr = arr_mut(s);
    arr.clips = c;
    arr.markers = m;
    arr.tracks = t;
    s.project.automations = a;
    for (ch, v) in s.project.channels.iter_mut().zip(st) {
        ch.sound.stretch = v;
    }
    s.playlist.sel.clear();
    s.piano.dirty = true;
}

/// Topmost clip under (x, y): the last one in the list (drawn on top).
pub fn clip_at(s: &State, g: &PGeom, x: f32, y: f32) -> Option<usize> {
    clips(s).iter().enumerate().rev().find(|(_, c)| g.clip_rect(c).contains(x, y)).map(|(i, _)| i)
}

/// The playlist's snap setting with Main read as the global snap (§4). Events and Markers are not available here.
pub fn grid_snap(s: &State) -> Snap {
    s.playlist.snap.grid(s.global_snap.now).unwrap_or(Snap::Line)
}

/// Where a click at `tick` lands (§4): the nearest unit (Cell: the start of the cell it is in), the tick under the
/// pointer for (none). Alt: no snapping.
pub fn snap_place(s: &State, g: &PGeom, tick: f32, alt: bool) -> u32 {
    let t = tick.max(0.0);
    let sn = grid_snap(s);
    if alt || sn == Snap::None {
        return t as u32;
    }
    let u = sn.ticks(g.ppq, g.bar_w);
    if sn == Snap::Cell { t as u32 / u * u } else { (t / u as f32).round() as u32 * u }
}

/// The song position set on the ruler: down to the unit (Alt: the tick under the pointer).
fn snap_down(s: &State, g: &PGeom, tick: f32, alt: bool) -> u32 {
    let t = tick.max(0.0) as u32;
    if alt {
        return t;
    }
    let u = grid_snap(s).ticks(g.ppq, g.bar_w);
    t - t % u
}

/// Snap a tick delta to the nearest snap unit.
pub fn snap_delta(s: &State, g: &PGeom, dt: f32, alt: bool) -> i64 {
    if alt {
        return dt.round() as i64;
    }
    let u = grid_snap(s).ticks(g.ppq, g.bar_w) as f32;
    ((dt / u).round() * u) as i64
}

/// Place a clip of the current source on `track` at `tick`; returns its index.
pub fn place(s: &mut State, track: u16, tick: u32) -> usize {
    let src = source(s);
    let len = match s.playlist.lock_len {
        Some((l, n)) if l == src => n,
        _ => source_len(s, src),
    };
    let audio = source_kind(s, src) == PickKind::Audio && matches!(src, ClipSource::Channel(_));
    let a = arr_mut(s);
    let mut c = Clip::new(tick, len, track, src);
    if audio {
        // FL writes gain 1.0 into a new audio clip (spec: flp/events/playlist.md, byte 52).
        c.set_gain(1.0);
    }
    a.clips.push(c);
    s.piano.dirty = true;
    clips(s).len() - 1
}

/// Delete clips by index (selection indices are remapped).
pub fn delete(s: &mut State, mut idx: Vec<usize>) {
    idx.sort_unstable();
    idx.dedup();
    let a = arr_mut(s);
    for &i in idx.iter().rev() {
        if i < a.clips.len() {
            a.clips.remove(i);
        }
    }
    let sel = std::mem::take(&mut s.playlist.sel);
    s.playlist.sel = sel.into_iter().filter(|i| !idx.contains(i)).map(|i| i - idx.iter().filter(|&&d| d < i).count()).collect();
    s.playlist.hover_clip = None;
    s.piano.dirty = true;
}

/// Clips an edit applies to: the selection, else all.
pub fn targets(s: &State) -> Vec<usize> {
    if s.playlist.sel.is_empty() { (0..clips(s).len()).collect() } else { s.playlist.sel.clone() }
}

/// `idx` plus every clip that shares a group with one of them (grouped clips select and move as one, §3a); just `idx`
/// while Enable groups is off (spec: ui/enable-groups.md §3).
pub fn with_groups(s: &State, mut idx: Vec<usize>) -> Vec<usize> {
    if !crate::enable_groups::on() {
        return idx;
    }
    let groups: Vec<u16> = idx.iter().filter_map(|&i| clips(s).get(i)).map(Clip::group).filter(|&g| g != 0).collect();
    if groups.is_empty() {
        return idx;
    }
    for (i, c) in clips(s).iter().enumerate() {
        if groups.contains(&c.group()) && !idx.contains(&i) {
            idx.push(i);
        }
    }
    idx
}

/// Shift+G: put the selected clips (and the groups they belong to) into one new group. Returns how many.
pub fn group_clips(s: &mut State) -> usize {
    let idx = with_groups(s, s.playlist.sel.clone());
    if idx.len() < 2 {
        return 0;
    }
    // The selected clips' own groups count as used (FL: group 1 + loose clips regrouped became group 2).
    let used: Vec<u16> = clips(s).iter().map(Clip::group).collect();
    let g = free_group(&used);
    let arr = arr_mut(s);
    for &i in &idx {
        if let Some(c) = arr.clips.get_mut(i) {
            c.set_group(g);
        }
    }
    s.playlist.sel = idx.clone();
    s.piano.dirty = true;
    idx.len()
}

/// Alt+G / Alt+U: take the selected clips' groups apart and deselect them, as FL does. Returns how many clips
/// left a group.
pub fn ungroup_clips(s: &mut State) -> usize {
    let idx = with_groups(s, s.playlist.sel.clone());
    let arr = arr_mut(s);
    let mut n = 0;
    for &i in &idx {
        if let Some(c) = arr.clips.get_mut(i).filter(|c| c.group() != 0) {
            c.set_group(0);
            n += 1;
        }
    }
    if n > 0 {
        s.playlist.sel.retain(|i| !idx.contains(i));
        s.piano.dirty = true;
    }
    n
}

/// The number a new group gets: the lowest from 1 up that none of `used` holds (§3a, checked in FL).
fn free_group(used: &[u16]) -> u16 {
    (1..=u16::MAX).find(|g| !used.contains(g)).unwrap_or(u16::MAX)
}

/// Copies of grouped clips (`from..` in the clip list) form new groups of their own, numbered as Shift+G numbers
/// a new group (§3a).
fn regroup_copies(s: &mut State, from: usize) {
    let arr = arr_mut(s);
    let mut used: Vec<u16> = arr.clips[..from.min(arr.clips.len())].iter().map(Clip::group).collect();
    let mut map: HashMap<u16, u16> = HashMap::new();
    for c in arr.clips.iter_mut().skip(from) {
        let g = c.group();
        if g != 0 {
            let n = *map.entry(g).or_insert_with(|| {
                let n = free_group(&used);
                used.push(n);
                n
            });
            c.set_group(n);
        }
    }
}

/// Copies of the moved clips `orig` (index, clip as the move began) for a held click's pick-up (spec:
/// ui/settings-choices.md "Input: Click-and-hold and both mouse buttons (2026-10-16)"): the originals go back to where
/// they were and the copies are added after every clip, grouped copies in groups of their own as a Shift+drag's are.
/// Returns the copies.
pub fn clone_clips(s: &mut State, orig: &[(usize, Clip)]) -> Vec<(usize, Clip)> {
    let arr = arr_mut(s);
    for &(i, o) in orig {
        if let Some(c) = arr.clips.get_mut(i) {
            c.position = o.position;
            c.track = o.track;
        }
    }
    let first = arr.clips.len();
    for &(_, o) in orig {
        let mut cp = o;
        cp.set_id(0);
        arr.clips.push(cp);
    }
    regroup_copies(s, first);
    s.piano.dirty = true;
    clips(s).iter().enumerate().skip(first).map(|(i, c)| (i, *c)).collect()
}

/// Alt+wheel: change the track height by FL's 6 px steps around the track under the mouse (§6).
pub fn zoom_rows(s: &mut State, grid: Rect, y: f32, notches: f32) {
    let row = s.playlist.row_h + ROW_STEP * notches.round();
    set_row_h(s, grid, y, row);
}

/// Set the track height (clamped to [`ROW_MIN`]..[`ROW_MAX`], whole 2 px steps like the saved view's 16 + 2h),
/// keeping the track at window y `y` in place.
pub fn set_row_h(s: &mut State, grid: Rect, y: f32, row: f32) {
    let rows = Rows::new(s);
    let cy = s.playlist_scroll.1 + (y - grid.y);
    let t = rows.at(cy.max(0.0));
    let frac = ((cy - rows.top(t)) / rows.height(t).max(1.0)).clamp(0.0, 1.0);
    s.playlist.row_h = ((row / 2.0).round() * 2.0).clamp(ROW_MIN, ROW_MAX);
    let rows = Rows::new(s);
    s.playlist_scroll.1 = (rows.top(t) + frac * rows.height(t) - (y - grid.y)).max(0.0);
}

/// Take the project's stored playlist view (event 242): track height, time zoom and scroll (spec:
/// flp/events/playlist.md "Playlist view"). A project without one gets the default height.
pub fn load_view(s: &mut State) {
    s.playlist.row_h = ROW_H;
    let Some(v) = s.project.playlist_view else { return };
    s.playlist.row_h = (v.row_px() as f32).clamp(ROW_MIN, ROW_MAX);
    let tpp = v.ticks_per_px();
    if tpp.is_finite() && tpp > 0.0 {
        s.playlist_bar_w = (f64::from(bar(s)) / tpp).clamp(4.0, 600.0) as f32;
        let left = v.left();
        let tpp = f64::from(bar(s)) / f64::from(s.playlist_bar_w);
        s.playlist_scroll.0 = if left.is_finite() { (left / tpp).max(0.0) as f32 } else { 0.0 };
    }
    s.playlist_scroll.1 = v.scroll_y() as f32;
}

/// Write the playlist's track height, time zoom and scroll into the project for saving (event 242).
pub fn store_view(s: &mut State) {
    let mut v = s.project.playlist_view.unwrap_or_else(|| os_model::PlaylistView::new(s.project.ppq));
    let tpp = f64::from(bar(s)) / f64::from(s.playlist_bar_w.max(1.0));
    v.set_ticks_per_px(tpp);
    v.set_left(f64::from(s.playlist_scroll.0) * tpp);
    v.set_scroll_y(s.playlist_scroll.1.max(0.0).round() as u32);
    v.set_height(((s.playlist.row_h.clamp(ROW_MIN, ROW_MAX) - ROW_MIN) / 2.0).round() as u32);
    if s.project.playlist_view != Some(v) {
        s.project.playlist_view = Some(v);
    }
}

/// Shift+Tab: the next of the common snap settings (Line, Cell, Beat, Bar, none); from any other setting, Line.
pub fn next_snap(p: EditorSnap) -> EditorSnap {
    const CYCLE: [Snap; 5] = [Snap::Line, Snap::Cell, Snap::Beat, Snap::Bar, Snap::None];
    let at = match p {
        EditorSnap::Grid(sn) => CYCLE.iter().position(|&x| x == sn),
        _ => None,
    };
    EditorSnap::Grid(at.map_or(Snap::Line, |i| CYCLE[(i + 1) % CYCLE.len()]))
}

/// Split clip `i` at `tick` (inside it); the right part keeps the content in place. Returns the new clip's index.
pub fn split(s: &mut State, i: usize, tick: u32) -> Option<usize> {
    let c = *clips(s).get(i)?;
    if tick <= c.position || tick >= c.end() {
        return None;
    }
    let (w0, w1) = window(s, &c);
    let left_len = tick - c.position;
    let cut = to_src(s, c.source, f64::from(left_len));
    let mut left = c;
    left.length = left_len;
    left.window = Some((w0, w0 + cut));
    let mut right = c;
    right.position = tick;
    right.length = c.end() - tick;
    right.window = Some((w0 + cut, w1.max(w0 + cut)));
    right.set_id(0);
    let a = arr_mut(s);
    a.clips[i] = left;
    a.clips.push(right);
    let n = a.clips.len();
    s.piano.dirty = true;
    Some(n - 1)
}

/// Slice every clip crossing `tick` on tracks `t0..=t1`.
pub fn slice(s: &mut State, tick: u32, t0: u16, t1: u16) -> usize {
    let hit: Vec<usize> = clips(s).iter().enumerate().filter(|(_, c)| c.track >= t0 && c.track <= t1 && c.position < tick && c.end() > tick).map(|(i, _)| i).collect();
    hit.iter().filter(|&&i| split(s, i, tick).is_some()).count()
}

/// Move the content of a clip by `dt` ticks inside it (Slip tool).
pub fn slip(s: &mut State, i: usize, orig: &Clip, dt: i64) {
    let (w0, w1) = window(s, orig);
    let d = to_src(s, orig.source, -dt as f64);
    let (mut a, mut b) = (w0 + d, w1 + d);
    if a < 0.0 {
        b -= a;
        a = 0.0;
    }
    if let Some(c) = arr_mut(s).clips.get_mut(i) {
        c.window = Some((a, b));
    }
    s.piano.dirty = true;
}

/// Resize from the right: the window end follows the length.
pub fn resize_right(s: &mut State, i: usize, orig: &Clip, len: u32) {
    let (w0, _) = window(s, orig);
    let end = w0 + to_src(s, orig.source, f64::from(len));
    // An audio clip without trims plays its whole sample (`fit_audio`), and an uncut pattern clip its whole pattern
    // whatever its length (spec: audio/song-structure.md): resized, either gets a window and plays only that. An
    // unset automation window already means "from the start for the clip's length".
    let cuts = match orig.source {
        ClipSource::Pattern(_) => true,
        ClipSource::Channel(_) => source_kind(s, orig.source) == PickKind::Audio,
    };
    if let Some(c) = arr_mut(s).clips.get_mut(i) {
        c.length = len.max(1);
        if orig.window.is_some() || cuts {
            c.window = Some((w0, end));
        }
    }
    s.piano.dirty = true;
}

/// Stretch mode (§9b): an audio clip resized to `len` ticks at `pos` keeps showing the same part of its sample: its
/// channel's stretch time becomes the whole sample's stretched length times the new / old clip length, the multiplier
/// back at 1, the method and pitch kept (FL does this). False when not stretching.
pub fn stretch_clip(s: &mut State, i: usize, orig: &Clip, pos: u32, len: u32) -> bool {
    let ClipSource::Channel(ch) = orig.source else { return false };
    let Some(&(_, base)) = s.playlist.stretch_base.iter().find(|b| b.0 == ch) else { return false };
    // Up to Lemon Sampler's longest TIME.
    let beats = (base * f64::from(len) / f64::from(orig.length.max(1))).clamp(1.0 / 192.0, 4096.0);
    if let Some(c) = s.project.channels.iter_mut().find(|c| c.index == ch) {
        c.sound.set_stretch_time(beats);
    }
    // The length that time allows (clamped), the window unchanged.
    let len = (f64::from(orig.length) * beats / base).round().max(1.0) as u32;
    let end = orig.position + orig.length;
    if let Some(c) = arr_mut(s).clips.get_mut(i) {
        c.length = len;
        c.position = if pos != orig.position { end.saturating_sub(len) } else { pos };
    }
    s.piano.dirty = true;
    true
}

/// Resize from the left (trim start): the content stays in place (§2a).
pub fn resize_left(s: &mut State, i: usize, orig: &Clip, dt: i64) {
    let (w0, w1) = window(s, orig);
    // The window start may not go below the source start.
    let min_dt = -(w0 / to_src(s, orig.source, 1.0)).floor() as i64;
    let dt = dt.max(min_dt).max(-i64::from(orig.position)).min(i64::from(orig.length) - 1);
    let d = to_src(s, orig.source, dt as f64);
    if let Some(c) = arr_mut(s).clips.get_mut(i) {
        c.position = (i64::from(orig.position) + dt) as u32;
        c.length = (i64::from(orig.length) - dt) as u32;
        c.window = Some(((w0 + d).max(0.0), w1));
    }
    s.piano.dirty = true;
}

/// Ctrl+B: duplicate the selection (or all clips) right after itself, or by the time selection's length.
pub fn duplicate(s: &mut State) {
    let idx = targets(s);
    let cs: Vec<Clip> = idx.iter().filter_map(|&i| clips(s).get(i).copied()).collect();
    let (Some(a), Some(b)) = (cs.iter().map(|c| c.position).min(), cs.iter().map(Clip::end).max()) else { return };
    let span = match s.playlist.range {
        Some((r0, r1)) if r1 > r0 => r1 - r0,
        _ => {
            // Whole bars, as FL duplicates sections.
            let bar = bar(s);
            (b - a).div_ceil(bar) * bar
        }
    };
    let arr = arr_mut(s);
    let first = arr.clips.len();
    for c in cs {
        let mut n = c;
        n.position += span;
        n.set_id(0);
        arr.clips.push(n);
    }
    let n = arr.clips.len();
    regroup_copies(s, first);
    s.playlist.sel = (first..n).collect();
    if let Some((r0, r1)) = s.playlist.range {
        s.playlist.range = Some((r0 + span, r1 + span));
    }
    s.piano.dirty = true;
}

pub fn copy(s: &mut State) {
    let idx = if s.playlist.sel.is_empty() { Vec::new() } else { s.playlist.sel.clone() };
    s.playlist.clipboard = idx.iter().filter_map(|&i| clips(s).get(i).copied()).collect();
}

/// Paste at the song position, keeping the clips' tracks and spacing.
pub fn paste(s: &mut State) {
    let cb = s.playlist.clipboard.clone();
    let Some(first) = cb.iter().map(|c| c.position).min() else { return };
    let at = s.playlist.pos;
    let arr = arr_mut(s);
    let start = arr.clips.len();
    for c in cb {
        let mut n = c;
        n.position = at + (c.position - first);
        n.set_id(0);
        arr.clips.push(n);
    }
    let n = arr.clips.len();
    regroup_copies(s, start);
    s.playlist.sel = (start..n).collect();
    s.piano.dirty = true;
}

/// Ctrl+Insert: insert the time selection's length of empty space at its start (all tracks); without a time
/// selection, one bar at the song position (§5, OS).
pub fn insert_space(s: &mut State) -> (u32, u32) {
    let (r0, r1) = s.playlist.range.filter(|r| r.1 > r.0).unwrap_or_else(|| (s.playlist.pos, s.playlist.pos + bar(s)));
    let len = r1 - r0;
    let ids: Vec<usize> = clips(s).iter().enumerate().filter(|(_, c)| c.position < r0 && c.end() > r0).map(|(i, _)| i).collect();
    for i in ids {
        split(s, i, r0);
    }
    let arr = arr_mut(s);
    for c in &mut arr.clips {
        if c.position >= r0 {
            c.position += len;
        }
    }
    for m in &mut arr.markers {
        if m.position >= r0 {
            m.position += len;
        }
    }
    s.piano.dirty = true;
    (r0, r1)
}

/// Ctrl+Delete: remove the time selection and pull later clips left.
pub fn delete_space(s: &mut State) -> bool {
    let Some((r0, r1)) = s.playlist.range.filter(|r| r.1 > r.0) else { return false };
    let len = r1 - r0;
    // Cut clips at both edges, drop what lies inside, move the rest.
    for t in [r0, r1] {
        let ids: Vec<usize> = clips(s).iter().enumerate().filter(|(_, c)| c.position < t && c.end() > t).map(|(i, _)| i).collect();
        for i in ids {
            split(s, i, t);
        }
    }
    let inside: Vec<usize> = clips(s).iter().enumerate().filter(|(_, c)| c.position >= r0 && c.end() <= r1).map(|(i, _)| i).collect();
    delete(s, inside);
    let arr = arr_mut(s);
    for c in &mut arr.clips {
        if c.position >= r1 {
            c.position -= len;
        }
    }
    arr.markers.retain(|m| m.position < r0 || m.position >= r1);
    for m in &mut arr.markers {
        if m.position >= r1 {
            m.position -= len;
        }
    }
    s.playlist.sel.clear();
    s.piano.dirty = true;
    true
}

/// Shift+Up / Down: move the selected clips' tracks (or the selected track) and their clips by `d`, swapping with
/// the neighbours (track names, colours and states go along).
pub fn shift_tracks(s: &mut State, d: i32) {
    let sel = s.playlist.sel.clone();
    if !sel.is_empty() {
        let arr = arr_mut(s);
        let lo = sel.iter().filter_map(|&i| arr.clips.get(i)).map(|c| i32::from(c.track)).min().unwrap_or(0);
        let hi = sel.iter().filter_map(|&i| arr.clips.get(i)).map(|c| i32::from(c.track)).max().unwrap_or(0);
        if lo + d < 0 || hi + d >= i32::from(TRACKS) {
            return;
        }
        for &i in &sel {
            if let Some(c) = arr.clips.get_mut(i) {
                c.track = (i32::from(c.track) + d) as u16;
            }
        }
        s.piano.dirty = true;
        return;
    }
    let t = i32::from(s.playlist.track);
    let u = t + d;
    if u < 0 || u >= i32::from(TRACKS) {
        return;
    }
    swap_tracks(s, t as u16, u as u16);
    s.playlist.track = u as u16;
}

/// Swap two tracks with their clips.
pub fn swap_tracks(s: &mut State, a: u16, b: u16) {
    let arr = arr_mut(s);
    let (ta, tb) = (arr.tracks[usize::from(a)].clone(), arr.tracks[usize::from(b)].clone());
    arr.tracks[usize::from(a)] = PlaylistTrack { number: ta.number, ..tb };
    arr.tracks[usize::from(b)] = PlaylistTrack { number: tb.number, ..ta };
    for c in &mut arr.clips {
        if c.track == a {
            c.track = b;
        } else if c.track == b {
            c.track = a;
        }
    }
    s.piano.dirty = true;
}

/// Insert an empty track at `t` (the rest move down; the last track's clips stay on it).
pub fn insert_track(s: &mut State, t: u16) {
    let arr = arr_mut(s);
    for c in &mut arr.clips {
        if c.track >= t && c.track < TRACKS - 1 {
            c.track += 1;
        }
    }
    let n = arr.tracks.len();
    for k in (usize::from(t) + 1..n).rev() {
        let prev = arr.tracks[k - 1].clone();
        arr.tracks[k] = PlaylistTrack { number: arr.tracks[k].number, ..prev };
    }
    arr.tracks[usize::from(t)] = PlaylistTrack::new(u32::from(t) + 1);
    s.piano.dirty = true;
}

/// Delete track `t` with its clips (the rest move up).
pub fn delete_track(s: &mut State, t: u16) {
    let gone: Vec<usize> = clips(s).iter().enumerate().filter(|(_, c)| c.track == t).map(|(i, _)| i).collect();
    delete(s, gone);
    let arr = arr_mut(s);
    for c in &mut arr.clips {
        if c.track > t {
            c.track -= 1;
        }
    }
    let n = arr.tracks.len();
    for k in usize::from(t)..n - 1 {
        let next = arr.tracks[k + 1].clone();
        arr.tracks[k] = PlaylistTrack { number: arr.tracks[k].number, ..next };
    }
    arr.tracks[n - 1] = PlaylistTrack::new(n as u32);
    s.piano.dirty = true;
}

/// Track LED: mute / unmute. A solo ends first: with "Restore previous state after solo" on, its saved states come
/// back; off, the states stay as they are.
pub fn toggle_track(s: &mut State, t: u16) {
    if crate::prefs::check(&crate::prefs::RESTORE_AFTER_SOLO) {
        end_solo(s);
    } else {
        s.playlist.solo = None;
    }
    let arr = arr_mut(s);
    if let Some(tr) = arr.tracks.get_mut(usize::from(t)) {
        tr.enabled = !tr.enabled;
    }
    s.piano.dirty = true;
}

/// Ctrl+click / right-click on the LED: solo the track (others muted); again ends the solo (see [`end_solo`]). A solo
/// moved to another track keeps the states from before the first one.
pub fn toggle_solo(s: &mut State, t: u16) {
    if let Some((was, _)) = s.playlist.solo {
        end_solo(s);
        if was == t {
            return;
        }
    }
    let arr = arr_mut(s);
    let states: Vec<bool> = arr.tracks.iter().map(|t| t.enabled).collect();
    for (k, tr) in arr.tracks.iter_mut().enumerate() {
        tr.enabled = k == usize::from(t);
    }
    s.playlist.solo = Some((t, states));
    s.piano.dirty = true;
}

/// Unsolo: every track on, or, with "Restore previous state after solo" on, the states from before the solo (spec:
/// ui/settings-choices.md "Miscellaneous").
fn end_solo(s: &mut State) {
    if let Some((_, states)) = s.playlist.solo.take() {
        let back = crate::prefs::check(&crate::prefs::RESTORE_AFTER_SOLO);
        let arr = arr_mut(s);
        for (k, tr) in arr.tracks.iter_mut().enumerate() {
            tr.enabled = !back || states.get(k).copied().unwrap_or(tr.enabled);
        }
        s.piano.dirty = true;
    }
}

/// Make unique: a pattern clip gets its own copy of the pattern (§5).
pub fn make_unique(s: &mut State, i: usize) -> Option<u16> {
    let c = *clips(s).get(i)?;
    let ClipSource::Pattern(n) = c.source else { return None };
    let mut p = s.project.pattern(n).cloned().unwrap_or(os_model::Pattern { number: n, ..Default::default() });
    let next = s.project.patterns.iter().map(|p| p.number).max().unwrap_or(0).max(n) + 1;
    let base = p.name.clone().unwrap_or_else(|| format!("Pattern {n}"));
    p.number = next;
    p.name = Some(format!("{base} #2"));
    s.project.patterns.push(p);
    if let Some(cl) = arr_mut(s).clips.get_mut(i) {
        cl.source = ClipSource::Pattern(next);
    }
    s.playlist.source = Some(ClipSource::Pattern(next));
    s.pattern = next;
    s.piano.dirty = true;
    Some(next)
}

/// First track at or below `from` with no clips overlapping `[a, b)`.
pub fn free_track(s: &State, from: u16, a: u32, b: u32) -> u16 {
    (from..TRACKS).find(|&t| !clips(s).iter().any(|c| c.track == t && c.position < b && c.end() > a)).unwrap_or(from)
}

/// The track below the last one that has clips.
pub fn track_after_used(s: &State) -> u16 {
    clips(s).iter().map(|c| c.track + 1).max().unwrap_or(0).min(TRACKS - 1)
}

/// Create an automation clip for `target` (§8): a new type-5 channel named `name` (its colour from
/// `State::clip_colors`), two flat points at `value`,
/// a clip over the time selection, else `automation_span` (spec: plugins/plugin-parameters.md §5), on the first free
/// track below the last used one.
pub fn create_automation(s: &mut State, tgt: u32, name: &str, value: f64) -> u16 {
    // One undo step, "Create automation clip": the playlist, and the control's initial value while "Initialize controls
    // automatically" is on (spec: ui/settings-choices.md "Advanced: Initialize controls automatically").
    let init = crate::init::automatic(tgt, value);
    arr_mut(s);
    let mut snap = crate::history::Snap::Playlist(Box::new(capture(s)));
    if init.is_some() {
        snap = crate::history::Snap::Several(vec![snap, crate::history::Snap::Init(tgt, crate::init::entry(s, tgt))]);
    }
    crate::history::record(s, crate::history::CREATE_AUTOMATION, snap);
    let name = &unique_channel_name(s, name);
    let index = s.project.channels.iter().map(|c| c.index + 1).max().unwrap_or(0);
    let (a, b) = match s.playlist.range {
        Some((r0, r1)) if r1 > r0 => (r0, r1),
        _ => automation_span(s),
    };
    let beats = f64::from(b - a) / f64::from(ppq(s));
    s.project.channels.push(Channel {
        index,
        kind: ChannelKind::Automation,
        name: Some(name.to_string()),
        plugin: None,
        plugin_state: None,
        // Spec: ui/settings-choices.md "Light mode and clip note colours (2026-10-16)" (`clip_colors`).
        color: Some(s.clip_colors.new_channel(index)),
        enabled: true,
        volume: 1.0,
        pan: 0.0,
        pitch_cents: 0,
        mixer_track: 0,
        sample_path: None,
        layer_children: Vec::new(),
        envelopes: Default::default(), rack: Default::default(), misc: Default::default(), sound: Default::default(),
    });
    s.project.automations.push(os_flp::project::new_automation(index, tgt, value.clamp(0.0, 1.0), beats));
    let track = free_track(s, track_after_used(s), a, b);
    let arr = arr_mut(s);
    arr.clips.push(Clip::new(a, b - a, track, ClipSource::Channel(index)));
    let n = arr.clips.len();
    s.playlist.sel = vec![n - 1];
    s.playlist.source = Some(ClipSource::Channel(index));
    s.playlist.picker = PickKind::Automation;
    if init.is_some() {
        crate::init::put(s, tgt, init);
    }
    s.piano.dirty = true;
    index
}

/// Where a new automation clip goes without a time selection (spec: ui/settings-choices.md "Miscellaneous: seven
/// more checks, Default template and Startup project", "Create automation clips at the playback position"): from the
/// song position with the check on, from bar 1 with it off; to the song's end (the largest end of a clip that is not
/// muted, audio/song-structure.md) when that lies after the start, else for the current pattern's length.
pub fn automation_span(s: &State) -> (u32, u32) {
    let a = if crate::prefs::check(&crate::prefs::AUTOMATION_AT_PLAYBACK_POSITION) { s.playlist.pos } else { 0 };
    let end = clips(s).iter().filter(|c| !c.muted).map(|c| c.end()).max().unwrap_or(0);
    let b = if end > a { end } else { a + source_len(s, ClipSource::Pattern(s.pattern)).max(1) };
    (a, b)
}

/// `name`, or `name #2`, `name #3`... when a channel already has it (spec: plugins/plugin-parameters.md §4).
pub fn unique_channel_name(s: &State, name: &str) -> String {
    let taken = |n: &str| s.project.channels.iter().any(|c| c.name.as_deref() == Some(n));
    if !taken(name) {
        return name.to_string();
    }
    (2..).map(|k| format!("{name} #{k}")).find(|n| !taken(n)).unwrap_or_default()
}

/// ADD > Audio clip and ADD > Automation clip (spec: ui/add-clip-channels.md §3): a channel of `kind` at the end of
/// the rack named "Audio Clip" / "Automation Clip" (made unique), on the master, the only selected channel, in the
/// rack filter group "Audio" / "Automation" with the filter on it, and the playlist's clip source; no clip is
/// placed. An audio clip has no sample and the default stretch and declick settings; an automation clip has FL's
/// two default points and no target. One undo step. Returns the rack row.
pub fn add_clip_channel(s: &mut State, kind: ChannelKind) -> usize {
    let audio = kind == ChannelKind::AudioClip;
    let index = s.project.channels.iter().map(|c| c.index + 1).max().unwrap_or(0);
    let before = crate::history::Snap::Rack(Box::new(crate::history::RackSide::now(s, s.project.channels.len())));
    snapshot_with(s, if audio { "Add audio clip" } else { "Add automation clip" }, before);
    let name = unique_channel_name(s, if audio { "Audio Clip" } else { "Automation Clip" });
    s.project.channels.push(Channel {
        index,
        kind,
        name: Some(name),
        plugin: None,
        plugin_state: None,
        color: (!audio).then(|| Rgb::new(0x5a, 0x74, 0x4c)),
        enabled: true,
        volume: if audio { 0.78125 } else { 1.0 },
        pan: 0.0,
        pitch_cents: 0,
        mixer_track: 0,
        sample_path: None,
        layer_children: Vec::new(),
        envelopes: Default::default(), rack: Default::default(), misc: Default::default(), sound: Default::default(),
    });
    if !audio {
        // Two points, at 0 and at 4 beats (one 4/4 bar), both 0.5, single curve without tension (§2).
        s.project.automations.push(os_flp::project::untargeted_automation(index, 0.5, 4.0));
    }
    let row = s.project.channels.len() - 1;
    s.selected_channel = row;
    s.rack.select_only(row);
    crate::rack::join_group(s, row, if audio { "Audio" } else { "Automation" });
    s.playlist.source = Some(ClipSource::Channel(index));
    s.playlist.picker = if audio { PickKind::Audio } else { PickKind::Automation };
    s.piano.dirty = true;
    row
}

/// Add an audio clip channel for a sample file and place it at `tick` on `track` (§9). The channel's colour comes from
/// `State::clip_colors`.
pub fn add_audio_clip(s: &mut State, path: &str, track: u16, tick: u32) -> Option<usize> {
    s.playlist.wave(path)?;
    snapshot(s);
    let index = s.project.channels.iter().map(|c| c.index + 1).max().unwrap_or(0);
    let name = std::path::Path::new(path).file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Audio".into());
    let mixer_track = (s.project.channels.iter().map(|c| c.mixer_track).max().unwrap_or(0) + 1).min(crate::mixer::max_insert(&s.project));
    s.project.channels.push(Channel {
        index,
        kind: ChannelKind::AudioClip,
        name: Some(name),
        plugin: None,
        plugin_state: None,
        // Spec: ui/settings-choices.md "Light mode and clip note colours (2026-10-16)" (`clip_colors`).
        color: Some(s.clip_colors.new_channel(index)),
        enabled: true,
        volume: 0.78125,
        pan: 0.0,
        pitch_cents: 0,
        mixer_track,
        sample_path: Some(path.to_string()),
        layer_children: Vec::new(),
        envelopes: Default::default(), rack: Default::default(), misc: Default::default(), sound: Default::default(),
    });
    s.playlist.source = Some(ClipSource::Channel(index));
    s.playlist.picker = PickKind::Audio;
    let i = place(s, track, tick);
    s.playlist.sel = vec![i];
    Some(i)
}

// ---------------------------------------------------------------------------------------------------------
// Automation points (§8)

/// Height of a value in a clip body.
pub fn value_y(body: Rect, v: f64) -> f32 {
    snap(f64::from(body.bottom()) - 2.0 - v * f64::from(body.h - 4.0))
}

pub fn value_at_y(body: Rect, y: f32) -> f64 {
    f64::from(((body.bottom() - 2.0 - y) / (body.h - 4.0).max(1.0)).clamp(0.0, 1.0))
}

/// Screen x of an automation point of clip `c`.
pub fn point_x(s: &State, g: &PGeom, c: &Clip, beat: f64) -> f32 {
    let (w0, _) = window(s, c);
    g.x_of(c.position as f32 + ((beat - w0) * f64::from(g.ppq)) as f32)
}

/// Beat in the automation under screen x for clip `c`.
pub fn beat_at(s: &State, g: &PGeom, c: &Clip, x: f32) -> f64 {
    let (w0, _) = window(s, c);
    w0 + f64::from(g.tick_at(x) - c.position as f32) / f64::from(g.ppq)
}

/// The point of an automation clip near (x, y) (8 px).
pub fn point_near(s: &State, g: &PGeom, ci: usize, x: f32, y: f32) -> Option<usize> {
    let c = clips(s).get(ci)?;
    let ClipSource::Channel(ch) = c.source else { return None };
    let a = s.project.automation(ch)?;
    let body = g.body(c);
    a.points
        .iter()
        .enumerate()
        .map(|(i, p)| (i, (point_x(s, g, c, p.beat) - x).hypot(value_y(body, p.value) - y)))
        .filter(|&(_, d)| d <= 8.0)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// The tension handle (the segment's midpoint on the curve) near (x, y): index of the segment's end point.
pub fn handle_near(s: &State, g: &PGeom, ci: usize, x: f32, y: f32) -> Option<usize> {
    let c = clips(s).get(ci)?;
    let ClipSource::Channel(ch) = c.source else { return None };
    let a = s.project.automation(ch)?;
    let body = g.body(c);
    (1..a.points.len()).find(|&i| {
        let (p, q) = (a.points[i - 1], a.points[i]);
        if !q.mode.has_tension() || matches!(q.mode, AutoMode::Hold) {
            return false;
        }
        let mid = (p.beat + q.beat) / 2.0;
        let hx = point_x(s, g, c, mid);
        let hy = value_y(body, a.value_at(mid).unwrap_or(0.0));
        (hx - x).hypot(hy - y) <= 6.0 && (point_x(s, g, c, q.beat) - point_x(s, g, c, p.beat)) > 16.0
    })
}

/// Distance from (x, y) to the curve at x.
pub fn curve_dist(s: &State, g: &PGeom, ci: usize, x: f32, y: f32) -> Option<f32> {
    let c = clips(s).get(ci)?;
    let ClipSource::Channel(ch) = c.source else { return None };
    let a = s.project.automation(ch)?;
    let v = a.value_at(beat_at(s, g, c, x))?;
    Some((value_y(g.body(c), v) - y).abs())
}

/// Add a point to automation channel `ch` at `beat` with `value`; returns its index.
pub fn add_point(s: &mut State, ch: u16, beat: f64, value: f64) -> Option<usize> {
    let a = s.project.automation_mut(ch)?;
    let i = a.points.partition_point(|p| p.beat <= beat);
    // A new point takes the mode / tension of the segment it splits ("new segments use the last tension").
    let (mode, tension) = a.points.get(i).map_or((AutoMode::Single, 0.0), |q| (q.mode, q.tension));
    let flag = if mode.has_tension() { 2 } else { 0 };
    a.points.insert(i, AutoPoint { beat: beat.max(0.0), value: value.clamp(0.0, 1.0), tension, mode, flag });
    s.piano.dirty = true;
    Some(i)
}

/// Move point `i` to (beat, value), kept between its neighbours; the first point stays at beat 0.
pub fn move_point(s: &mut State, ch: u16, i: usize, beat: f64, value: f64) {
    let Some(a) = s.project.automation_mut(ch) else { return };
    let n = a.points.len();
    if i >= n {
        return;
    }
    let lo = if i == 0 { 0.0 } else { a.points[i - 1].beat };
    let hi = if i + 1 < n { a.points[i + 1].beat } else { f64::MAX };
    a.points[i].beat = if i == 0 { 0.0 } else { beat.clamp(lo, hi) };
    a.points[i].value = value.clamp(0.0, 1.0);
    s.piano.dirty = true;
}

pub fn delete_point(s: &mut State, ch: u16, i: usize) {
    let Some(a) = s.project.automation_mut(ch) else { return };
    // The first and a lone second point stay (a clip needs a curve).
    if a.points.len() > 2 && i > 0 && i < a.points.len() {
        a.points.remove(i);
        s.piano.dirty = true;
    }
}

/// The tempo an automation value stands for: `(10000 + Round(single(v) * 512000)) / 1000` BPM, 10 to 522 (spec:
/// audio/timing.md "Tempo that changes").
pub fn auto_tempo(v: f64) -> f64 {
    (10000.0 + (f64::from(v as f32) * 512000.0).round_ties_even()) / 1000.0
}

/// The automation value of a tempo (the inverse of [`auto_tempo`]).
pub fn tempo_value(bpm: f64) -> f64 {
    ((bpm * 1000.0 - 10000.0) / 512000.0).clamp(0.0, 1.0)
}

/// The text shown for an automation value: the target's own unit where known (spec: audio/voice-levels.md "Values
/// from automation").
pub fn value_text(s: &State, ch: u16, v: f64) -> String {
    let tgt = s.project.automation(ch).and_then(|a| a.target()).map(target::decode);
    match tgt {
        Some(target::Target::Tempo) => format!("{:.3} BPM", auto_tempo(v)),
        Some(target::Target::ChannelPan(_) | target::Target::MixerPan(_)) => crate::knobs::pan_text(((v - 0.5) * 2.0) as f32),
        Some(target::Target::ChannelPitch(_)) => format!("{:+} cents", os_engine::laws::auto_pitch_cents(v)),
        Some(target::Target::MixerVolume(_)) => crate::views::mixer::fader_db_text(v),
        Some(t @ (target::Target::ChannelParam(..) | target::Target::SlotParam(..))) => {
            crate::params::value_text(s, t, v).unwrap_or_else(|| format!("{:.0}%", v * 100.0))
        }
        _ => format!("{:.0}%", v * 100.0),
    }
}

/// Parse a typed value (percent, BPM or 0..1) back to 0..1.
pub fn parse_value(s: &State, ch: u16, text: &str) -> Option<f64> {
    let t = text.trim().trim_end_matches('%').trim_end_matches("BPM").trim();
    let v: f64 = t.parse().ok()?;
    let tgt = s.project.automation(ch).and_then(|a| a.target()).map(target::decode);
    Some(match tgt {
        Some(target::Target::Tempo) => tempo_value(v),
        _ if v > 1.0 || text.contains('%') => v / 100.0,
        _ => v,
    })
    .map(|v| v.clamp(0.0, 1.0))
}

// ---------------------------------------------------------------------------------------------------------
// Menus (§10)

pub fn options_menu(s: &State, x: f32, y: f32) -> Popup {
    let snap_items = snap_items(s);
    let tools = PlTool::ALL.iter().map(|&t| PItem::new(format!("{t:?}"), Cmd::Tool(t)).mark(Mark::Radio(s.playlist.tool == t))).collect();
    let items = vec![
        PItem::sub(
            "Edit",
            vec![
                PItem::new("Undo", Cmd::Undo).key("Ctrl+Z"),
                PItem::new("Cut", Cmd::Cut).key("Ctrl+X"),
                PItem::new("Copy", Cmd::Copy).key("Ctrl+C"),
                PItem::new("Paste", Cmd::Paste).key("Ctrl+V"),
                PItem::new("Duplicate", Cmd::Duplicate).key("Ctrl+B"),
                PItem::new("Delete", Cmd::Delete).key("Del"),
                PItem::sep(),
                PItem::new("Shift left", Cmd::ShiftTime(-1)).key("Shift+Left"),
                PItem::new("Shift right", Cmd::ShiftTime(1)).key("Shift+Right"),
                PItem::new("Shift up", Cmd::ShiftTracks(-1)).key("Shift+Up"),
                PItem::new("Shift down", Cmd::ShiftTracks(1)).key("Shift+Down"),
                PItem::new("Make unique", Cmd::MakeUnique).key("Shift+Ctrl+C"),
                PItem::sep(),
                PItem::new("Mute", Cmd::Mute(true)).key("Alt+M"),
                PItem::new("Unmute", Cmd::Mute(false)).key("Shift+Alt+M"),
                PItem::sep(),
                PItem::new("Insert space", Cmd::InsertSpace).key("Ctrl+Ins"),
                PItem::new("Delete space", Cmd::DeleteSpace).key("Ctrl+Del"),
            ],
        ),
        PItem::sub(
            "Tools",
            vec![
                PItem::new("Consolidate playlist selection", Cmd::Consolidate).key("Ctrl+Alt+C"),
                PItem::new("Export all playlist tracks", Cmd::ExportTracks),
                PItem::new("Quick quantize start times", Cmd::QuantizeStarts),
            ],
        ),
        PItem::sub("Tool", tools),
        PItem::sub("Snap", snap_items),
        PItem::sub(
            "Select",
            vec![
                PItem::new("Deselect", Cmd::Deselect).key("Ctrl+D"),
                PItem::new("Select all", Cmd::SelectAll).key("Ctrl+A"),
                PItem::new("Select by selected source", Cmd::SelectSource).key("Shift+C"),
                PItem::new("Select muted", Cmd::SelectMuted),
                PItem::new("Invert selection", Cmd::InvertSelection).key("Shift+I"),
                PItem::sep(),
                PItem::new("Select time around selection", Cmd::SelectTime).key("Ctrl+Enter"),
            ],
        ),
        PItem::sub(
            "Group",
            vec![PItem::new("Group", Cmd::GroupClips).key("Shift+G"), PItem::new("Ungroup", Cmd::UngroupClips).key("Alt+G")],
        ),
        PItem::sub(
            "Zoom",
            vec![
                PItem::new("Zoom in", Cmd::ZoomIn).key("PgUp"),
                PItem::new("Zoom out", Cmd::ZoomOut).key("PgDn"),
                PItem::new("Zoom out far", Cmd::ZoomAll).key("Shift+4"),
                PItem::new("Zoom on selection", Cmd::ZoomSelection).key("Shift+5"),
            ],
        ),
        PItem::sub(
            "Time markers",
            vec![
                PItem::new("Add one...", Cmd::AddMarker).key("Alt+T"),
                PItem::new("Add time signature change...", Cmd::AddSignature).key("Shift+Alt+T"),
            ],
        ),
        PItem::sep(),
        PItem::new("Reset empty playlist tracks", Cmd::ResetEmptyTracks),
    ];
    Popup { x, y, items, sub: None }
}

/// The magnet's Snap menu and the ▸ menu's Snap submenu (§4): Main, Line, Cell, (none) to Bar; Events and Markers
/// are not available yet.
fn snap_items(s: &State) -> Vec<PItem> {
    let now = s.playlist.snap;
    let radio = |p: EditorSnap| PItem::new(p.label(), Cmd::Snap(p)).mark(Mark::Radio(now == p));
    let mut items = vec![radio(EditorSnap::Main), PItem::sep(), radio(EditorSnap::Grid(Snap::Line)), radio(EditorSnap::Grid(Snap::Cell)), PItem::sep()];
    items.extend(Snap::MENU.iter().filter(|sn| !matches!(sn, Snap::Line | Snap::Cell)).map(|&sn| radio(EditorSnap::Grid(sn))));
    items.extend([PItem::sep(), PItem::not_yet("Events"), PItem::not_yet("Markers")]);
    items
}

pub fn snap_menu(s: &State, x: f32, y: f32) -> Popup {
    Popup { x, y, items: snap_items(s), sub: None }
}

pub fn clip_menu(s: &State, i: usize, x: f32, y: f32) -> Popup {
    let c = clips(s)[i];
    let head = match source_kind(s, c.source) {
        PickKind::Pattern => "Pattern clip",
        PickKind::Audio => "Audio clip",
        PickKind::Automation => "Automation clip",
    };
    let mut items = vec![
        PItem::head(head),
        PItem::new("Preview", Cmd::ClipPreview(i)),
        PItem::sep(),
        PItem::new("Muted", Cmd::ClipMute(i)).mark(Mark::Check(c.muted)),
        PItem::sep(),
        PItem::new("Rename...", Cmd::ClipRename(i)),
        PItem::new("Random color", Cmd::ClipRandomColor(i)),
        PItem::sep(),
    ];
    if let ClipSource::Pattern(_) = c.source {
        items.push(PItem::new("Edit pattern", Cmd::ClipEditSource(i)));
        items.push(PItem::new("Make unique", Cmd::ClipUnique(i)));
    }
    items.push(PItem::new("Select all similar clips", Cmd::ClipSelectSimilar(i)));
    items.push(PItem::new("Delete", Cmd::ClipDelete(i)));
    Popup { x, y, items, sub: None }
}

/// The commands of a picker item's menu (§10a).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickCmd {
    Preview,
    SelectInPlaylist,
    Group,
    Rename,
    ChangeColor,
    RandomColor,
    LoadSample,
    Clone,
    Delete,
}

/// Right-click on an audio or automation clip in the picker (§10a): its channel's menu, as FL shows it there.
pub fn pick_menu(s: &State, ch: u16, x: f32, y: f32) -> Popup {
    let audio = source_kind(s, ClipSource::Channel(ch)) == PickKind::Audio;
    let it = |label: &'static str, c: PickCmd| PItem::new(label, Cmd::Pick(c, ch));
    let mut items = Vec::new();
    if audio {
        items.push(it("Preview", PickCmd::Preview));
    }
    items.extend([
        it("Select in playlist", PickCmd::SelectInPlaylist),
        it("Group selected...", PickCmd::Group),
        PItem::sep(),
        it("Rename, color and icon...", PickCmd::Rename),
        it("Change color...", PickCmd::ChangeColor),
        it("Random color", PickCmd::RandomColor),
        PItem::not_yet("Change icon..."),
        PItem::sep(),
    ]);
    if audio {
        items.extend([it("Load sample...", PickCmd::LoadSample), PItem::not_yet("Edit sample...")]);
    }
    items.extend([PItem::not_yet("Cut itself").mark(Mark::Check(true)), PItem::sep(), it("Clone", PickCmd::Clone)]);
    if !audio {
        items.push(it("Clone with links", PickCmd::Clone));
    }
    items.push(it("Delete...", PickCmd::Delete));
    Popup { x, y, items, sub: None }
}

/// Runs a picker item's command on the channel with index `ch` (§10a).
fn pick(app: &mut App, c: PickCmd, ch: u16) -> Option<String> {
    let row = app.state.project.channels.iter().position(|x| x.index == ch)?;
    let src = ClipSource::Channel(ch);
    let at = crate::patterns::box_at_mouse(app.mouse);
    let s = &mut app.state;
    match c {
        PickCmd::Preview => {
            // The source as one clip of its own length (the fitted length, else that of a clip of it).
            let fit = audio_fits(s).and_then(|f| f.get(&ch).copied().flatten()).map(|(t, _)| t.round().max(1.0) as u32);
            let len = fit.or_else(|| clips(s).iter().find(|c| c.source == src).map(|c| c.length))?;
            app.preview_clip(&Clip::new(0, len, 0, src));
        }
        PickCmd::SelectInPlaylist => s.playlist.sel = clips(s).iter().enumerate().filter(|(_, c)| c.source == src).map(|(i, _)| i).collect(),
        PickCmd::Group => crate::rack::open_box(s, crate::rack::RBox::GroupSel, at),
        PickCmd::Rename => crate::rack::open_box(s, crate::rack::RBox::Channel(row), at),
        PickCmd::ChangeColor => {
            crate::rack::open_box(s, crate::rack::RBox::Channel(row), at);
            if let Some(b) = s.pat_ui.name_box.as_mut() {
                b.palette = true;
            }
        }
        PickCmd::RandomColor => {
            crate::rack::random_color(s, row, u64::from(ch));
            s.piano.dirty = true;
        }
        PickCmd::LoadSample => return crate::rack::run(app, crate::rack::RCmd::LoadSample(row)),
        PickCmd::Clone => crate::rack::clone_rows(app, &[row]),
        PickCmd::Delete => crate::rack::ask_delete(s, vec![row]),
    }
    None
}

pub fn point_menu(s: &State, ch: u16, i: usize, x: f32, y: f32) -> Popup {
    let a = s.project.automation(ch);
    let n = a.map_or(0, |a| a.points.len());
    let mode = a.and_then(|a| a.points.get(i)).map(|p| p.mode);
    let title = PItem { head: true, ..PItem::new(format!("Point {}/{}", i + 1, n), Cmd::Nothing) };
    let mut items = vec![title, PItem::new("Delete", Cmd::PointDelete(ch, i))];
    items.push(PItem::head("Mode"));
    for m in AutoMode::MENU {
        items.push(PItem::new(m.name(), Cmd::PointMode(ch, i, m)).mark(Mark::Radio(mode == Some(m))));
    }
    items.push(PItem::sep());
    items.push(PItem::new("Type in value...", Cmd::PointValue(ch, i)));
    Popup { x, y, items, sub: None }
}

pub fn track_menu(s: &State, t: u16, x: f32, y: f32) -> Popup {
    let on = track_on(s, t);
    let colors: Vec<PItem> = [
        ("Default", None),
        ("Red", Some(Rgb::new(0xa0, 0x40, 0x40))),
        ("Orange", Some(Rgb::new(0xb0, 0x6c, 0x34))),
        ("Green", Some(Rgb::new(0x5a, 0x8c, 0x4a))),
        ("Teal", Some(Rgb::new(0x3c, 0x7c, 0x78))),
        ("Blue", Some(Rgb::new(0x48, 0x6c, 0xa4))),
        ("Purple", Some(Rgb::new(0x80, 0x52, 0x84))),
    ]
    .into_iter()
    .map(|(n, c)| PItem::new(n, Cmd::TrackColor(t, c)))
    .collect();
    let items = vec![
        PItem::head("Track"),
        PItem::new("Rename...", Cmd::TrackRename(t)),
        PItem::sub("Color", colors),
        PItem::sep(),
        PItem::new("Mute", Cmd::TrackMute(t)).mark(Mark::Check(!on)),
        PItem::new("Solo", Cmd::TrackSolo(t)).mark(Mark::Check(s.playlist.solo.as_ref().is_some_and(|x| x.0 == t))),
        PItem::new("Mute all clips", Cmd::TrackMuteClips(t, true)),
        PItem::new("Unmute all clips", Cmd::TrackMuteClips(t, false)),
        PItem::sep(),
        PItem::new("Group with above track", Cmd::TrackGroup(t)).mark(Mark::Check(track_info(s, t).is_some_and(|x| x.grouped))),
        PItem::new("Collapse group", Cmd::GroupCollapse(t))
            .mark(Mark::Check(group_track(s, t).or(Some(t)).and_then(|g| track_info(s, g)).is_some_and(|x| x.collapsed))),
        PItem::sep(),
        PItem::new("Insert one", Cmd::TrackInsert(t)),
        PItem::new("Delete one", Cmd::TrackDelete(t)),
        PItem::new("Move up", Cmd::TrackUp(t)),
        PItem::new("Move down", Cmd::TrackDown(t)),
    ];
    Popup { x, y, items, sub: None }
}

pub fn marker_menu(i: usize, x: f32, y: f32) -> Popup {
    Popup { x, y, items: vec![PItem::head("Time marker"), PItem::new("Rename...", Cmd::RenameMarker(i)), PItem::new("Delete", Cmd::DeleteMarker(i))], sub: None }
}

/// The short right-click menu of the toolbar's and the mixer's knobs, faders and the tempo: "Create automation clip"
/// (§8) and the initial value's items (`crate::init::items`) when the control can be automated, "Type in value..." when
/// it takes one (spec: ui/type-in-value.md §8).
pub fn knob_menu(s: &State, auto: Option<(u32, String, f64)>, typein: Option<crate::typein::Target>, x: f32, y: f32) -> Popup {
    let mut items = Vec::new();
    if let Some((tgt, name, value)) = auto {
        let [init, delete] = crate::init::items(s, Some((tgt, value)));
        items.extend([
            PItem::head("Automation"),
            PItem::new("Create automation clip", Cmd::CreateAutomation(tgt, name, value)),
            init,
            delete,
            PItem::not_yet("Edit events"),
            PItem::not_yet("Link to controller..."),
        ]);
    }
    if let Some(t) = typein {
        items.extend([PItem::head("Value"), PItem::new("Type in value...", Cmd::TypeIn(t))]);
    }
    Popup { x, y, items, sub: None }
}

// ---------------------------------------------------------------------------------------------------------
// Mouse

/// Drags inside the playlist.
#[derive(Debug, Clone)]
pub enum PlDrag {
    /// `fresh` = the clip was just placed (its undo step stays even without a move).
    Move { x0: f32, y0: f32, orig: Vec<(usize, Clip)>, fresh: bool },
    ResizeL { x0: f32, orig: Vec<(usize, Clip)> },
    ResizeR { x0: f32, orig: Vec<(usize, Clip)> },
    Slip { x0: f32, i: usize, orig: Clip },
    Erase,
    Mute { on: bool },
    Paint { track: u16, start: u32, len: u32 },
    Marquee { x0: f32, y0: f32, base: Vec<usize> },
    Slice { y0: f32 },
    /// Right button held on empty grid: a horizontal drag turns into a slice (§3).
    RightPending { x0: f32, y0: f32 },
    ZoomBox { x0: f32, y0: f32 },
    Playback,
    SongPos,
    TimeSel { t0: u32 },
    Marker { i: usize, dt: i64, t0: u32 },
    Point { ch: u16, clip: usize, i: usize, x0: f32, y0: f32, beat0: f64, v0: f64, lock: Option<bool> },
    Tension { ch: u16, i: usize, y0: f32, t0: f32 },
    /// An audio clip's fade-in (`out` false) or fade-out handle (§9a).
    Fade { clip: usize, out: bool },
    FadeTension { clip: usize, out: bool, y0: f32, t0: f32 },
    /// An audio clip's gain handle: dB from where the drag started.
    Gain { clip: usize, y0: f32, db0: f32 },
    /// A crossfade's handle (§9a): later clip `b`, earlier clip `a`, `b`'s gain at the middle when the drag started.
    Crossfade { a: usize, b: usize, y0: f32, m0: f64 },
}

/// The handles an audio clip shows while the mouse is over it (§9a).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    FadeIn,
    FadeOut,
    TensionIn,
    TensionOut,
    Gain,
    /// The handle in the middle of a crossfade (on the later clip).
    Crossfade,
}

/// Tension per logical pixel of a tension-handle drag, and dB per pixel of a gain drag (§9a, OS: FL's per-pixel
/// steps at 150 %).
const TENSION_PER_PX: f32 = 0.28;
const DB_PER_PX: f32 = 0.15;
/// Logical pixels of a crossfade-handle drag per unit of the later clip's gain at the middle (§9a, OS: 1/120 per
/// pixel).
const XFADE_PX: f64 = 120.0;

/// An audio clip (not a collapsed group's strip): the clips with fades and gain.
pub fn is_audio_clip(s: &State, g: &PGeom, c: &Clip) -> bool {
    source_kind(s, c.source) == PickKind::Audio && matches!(c.source, ClipSource::Channel(_)) && !g.rows.strip(c.track)
}

/// Pixels of `ms` milliseconds of an audio clip's fades: the song's time at the tempo, as the engine plays them
/// (spec: audio/audio-clips.md "Fades and clip gain"), whatever the channel's pitch or stretch.
pub fn ms_px(s: &State, g: &PGeom, ms: f32) -> f32 {
    (f64::from(ms) / fade_ms_per_tick(s)) as f32 * g.ppt
}

/// Milliseconds of a fade per tick at the tempo.
pub fn fade_ms_per_tick(s: &State) -> f64 {
    secs_per_tick(s) * 1000.0
}

/// Where an audio clip's fades end on screen: (fade-in end x, fade-out start x). The fades the record holds, bits or
/// not: automatic crossfades keep them in step with the clips (`follow_crossfades`), and with the switch off every
/// stored fade plays.
pub fn fade_xs(s: &State, g: &PGeom, c: &Clip) -> (f32, f32) {
    let body = g.body(c);
    let fin = c.stored_fade_in().map_or(0.0, |f| ms_px(s, g, f.0)).min(body.w);
    let fout = c.stored_fade_out().map_or(0.0, |f| ms_px(s, g, f.0)).min(body.w);
    (snap(f64::from(body.x) + f64::from(fin)), snap(f64::from(body.right()) - f64::from(fout)))
}

/// Whether a clip's fade-in (`out` false) or fade-out belongs to the automatic crossfades: the switch is on and the
/// edge's bit is clear. Its length follows the overlaps, and a fade-out's tension too (spec: audio/audio-clips.md
/// "Automatic crossfades"), so it has no tension handle of its own.
pub fn managed(s: &State, c: &Clip, out: bool) -> bool {
    os_model::crossfade::on(&s.project) && !if out { c.fade_out_set() } else { c.fade_in_set() }
}

/// A crossfade as drawn (§9a): later clip `b`'s fade-in and earlier clip `a`'s fade-out across their overlap, from `x0`
/// to `x1` on `b`'s body, and the two tensions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Xfade {
    pub a: usize,
    pub b: usize,
    pub x0: f32,
    pub x1: f32,
    pub t_in: f32,
    pub t_out: f32,
}

/// The crossfade audio clip `b` makes with an earlier clip, when there is one to draw: the switch is on, both edges
/// belong to it and each is the other's longest overlap (`os_model::crossfade::partner_before`), neither clip is muted.
pub fn crossfade(s: &State, g: &PGeom, b: usize) -> Option<Xfade> {
    let all = clips(s);
    let c = all.get(b).filter(|c| !c.muted && is_audio_clip(s, g, c))?;
    let a = os_model::crossfade::partner_before(&s.project, all, b).filter(|&a| !all[a].muted)?;
    let (_, t_in) = c.stored_fade_in()?;
    let t_out = all[a].fade_out_tension();
    Some(Xfade { a, b, x0: g.body(c).x, x1: fade_xs(s, g, c).0, t_in, t_out })
}

/// The crossfade handle (§9a), drawn and grabbed alike: its x at the overlap's middle, the y of the later clip's gain
/// there (the big circle) and of the earlier clip's (the small ring), on `b`'s body.
pub fn xfade_handle(g: &PGeom, b: &Clip, x: &Xfade) -> (f32, f32, f32) {
    let body = g.body(b);
    let y = |v: f64| body.bottom() - v as f32 * body.h;
    ((x.x0 + x.x1) / 2.0, y(os_model::fade_curve(0.5, f64::from(x.t_in))), y(os_model::fade_curve(0.5, f64::from(x.t_out))))
}

/// Radius of the crossfade handle's big circle, and how far from its middle a press grabs it.
pub const XFADE_R: f32 = 4.5;
const XFADE_GRAB: f32 = 6.0;

/// The middle of a fade curve drawn across `body` from `x0` to `x1` (the same height for a fade-in and a fade-out).
pub fn fade_mid(body: Rect, x0: f32, x1: f32, tension: f32, _out: bool) -> (f32, f32) {
    let v = os_model::fade_curve(0.5, f64::from(tension)) as f32;
    ((x0 + x1) / 2.0, body.bottom() - v * body.h)
}

/// The handle of audio clip `i` under (x, y), if any (§9a).
pub fn handle_at(s: &State, g: &PGeom, i: usize, x: f32, y: f32) -> Option<Handle> {
    let c = clips(s).get(i)?;
    if !is_audio_clip(s, g, c) || c.muted || !matches!(s.playlist.tool, PlTool::Draw | PlTool::Paint | PlTool::Select) {
        return None;
    }
    let body = g.body(c);
    let (fx_in, fx_out) = fade_xs(s, g, c);
    let near = |hx: f32, hy: f32, r: f32| (x - hx).abs() <= r && (y - hy).abs() <= r;
    // A crossfade's handle, on the later clip (§9a).
    if let Some(xf) = crossfade(s, g, i).filter(|xf| xf.x1 - xf.x0 > 10.0) {
        let (hx, hy, _) = xfade_handle(g, c, &xf);
        if near(hx, hy, XFADE_GRAB) {
            return Some(Handle::Crossfade);
        }
    }
    if let Some((_, t)) = c.stored_fade_in().filter(|_| !managed(s, c, false)) {
        let (mx, my) = fade_mid(body, body.x, fx_in, t, false);
        if fx_in - body.x > 10.0 && near(mx, my, 5.0) {
            return Some(Handle::TensionIn);
        }
    }
    if let Some((_, t)) = c.stored_fade_out().filter(|_| !managed(s, c, true)) {
        let (mx, my) = fade_mid(body, fx_out, body.right(), t, true);
        if body.right() - fx_out > 10.0 && near(mx, my, 5.0) {
            return Some(Handle::TensionOut);
        }
    }
    if near(fx_in + 3.0, body.y + 3.0, 5.0) {
        return Some(Handle::FadeIn);
    }
    if near(fx_out - 3.0, body.y + 3.0, 5.0) {
        return Some(Handle::FadeOut);
    }
    if near(body.cx(), body.bottom() - 2.0, 6.0) {
        return Some(Handle::Gain);
    }
    None
}

/// Hint text of a clip handle.
pub fn handle_hint(c: &Clip, h: Handle) -> String {
    match h {
        Handle::FadeIn => format!("Fade in: {:.0} ms", c.stored_fade_in().map_or(0.0, |f| f.0)),
        Handle::FadeOut => format!("Fade out: {:.0} ms", c.stored_fade_out().map_or(0.0, |f| f.0)),
        Handle::TensionIn => format!("Fade in tension: {:+.2}", c.fade_in_tension()),
        Handle::TensionOut => format!("Fade out tension: {:+.2}", c.fade_out_tension()),
        Handle::Gain => format!("Clip gain: {:+.1} dB", 20.0 * c.gain().log10()),
        Handle::Crossfade => format!("Crossfade tension: {:+.2}", c.fade_in_tension()),
    }
}

/// Edge of a clip under x: -1 left, 1 right, 0 body.
fn edge(r: Rect, x: f32) -> i32 {
    let z = EDGE.min(r.w / 4.0);
    if x <= r.x + z {
        -1
    } else if x >= r.right() - z {
        1
    } else {
        0
    }
}

/// Mouse press on the grid. `right` = the right button.
pub fn press_grid(app: &mut App, grid: Rect, right: bool) {
    let (x, y) = app.mouse;
    let s = &mut app.state;
    s.playlist.popup = None;
    let g = PGeom::new(s, grid);
    let track = g.track_at(y);
    let tick = g.tick_at(x).max(0.0);
    let hit = clip_at(s, &g, x, y);
    let tool = s.playlist.tool;
    let alt = app.alt;

    // Audio clip handles: fades, fade tensions, gain (§9a).
    if let (Some(ci), false) = (hit, right) {
        if let Some(h) = handle_at(s, &g, ci, x, y) {
            snapshot(s);
            let c = clips(s)[ci];
            app.drag = Some(Drag::Pl(match h {
                Handle::FadeIn => PlDrag::Fade { clip: ci, out: false },
                Handle::FadeOut => PlDrag::Fade { clip: ci, out: true },
                Handle::TensionIn => PlDrag::FadeTension { clip: ci, out: false, y0: y, t0: c.fade_in_tension() },
                Handle::TensionOut => PlDrag::FadeTension { clip: ci, out: true, y0: y, t0: c.fade_out_tension() },
                Handle::Gain => PlDrag::Gain { clip: ci, y0: y, db0: 20.0 * c.gain().log10() },
                Handle::Crossfade => {
                    let a = crossfade(s, &g, ci).map_or(ci, |xf| xf.a);
                    PlDrag::Crossfade { a, b: ci, y0: y, m0: os_model::fade_curve(0.5, f64::from(c.fade_in_tension())) }
                }
            }));
            app.hint = Cow::Owned(handle_hint(&c, h));
            app.invalidate();
            return;
        }
    }

    // Automation points: drag, add, menu (Draw / Paint / Select tools) (§8).
    if let Some(ci) = hit {
        let c = clips(s)[ci];
        // Double-clicking an audio clip opens its channel's sampler and selects that channel in the rack (§9); a
        // pattern clip makes its pattern the current one and opens the piano roll (§2b, as FL does). Which modifier
        // opens what on an audio clip is General ▸ Input ▸ Manage shortcuts' (`crate::shortcuts::clip_double_click`):
        // Show clip properties does nothing yet (no clip properties window); with neither row's modifier (a row set to
        // "(disabled)"), the second click is a plain one.
        if !right && matches!(tool, PlTool::Draw | PlTool::Paint | PlTool::Select) {
            let audio = audio_clip_row(s, &c);
            if audio.is_some() || matches!(c.source, ClipSource::Pattern(_)) {
                let now = std::time::Instant::now();
                let double = s.playlist.last_clip.is_some_and(|(i, t)| i == ci && now.duration_since(t).as_millis() < 450);
                s.playlist.last_clip = (!double).then_some((ci, now));
                let opens = audio.map(|_| crate::shortcuts::clip_double_click(alt));
                if double && opens != Some(None) {
                    if let Some(row) = audio {
                        if opens == Some(Some(crate::shortcuts::ClipOpen::Channel)) {
                            s.rack.select_only(row);
                            s.pending_open = Some(row);
                            app.focus_channel(row);
                        }
                    } else {
                        run(app, Cmd::ClipEditSource(ci));
                    }
                    app.invalidate();
                    return;
                }
            }
        }
        let auto_clip = matches!(c.source, ClipSource::Channel(ch) if s.project.automation(ch).is_some());
        if auto_clip && matches!(tool, PlTool::Draw | PlTool::Paint | PlTool::Select) && !app.ctrl && g.body(&c).contains(x, y) {
            let ClipSource::Channel(ch) = c.source else { return };
            if let Some(i) = point_near(s, &g, ci, x, y) {
                if right {
                    s.playlist.popup = Some(point_menu(s, ch, i, x, y));
                    return;
                }
                snapshot(s);
                let p = s.project.automation(ch).map(|a| a.points[i]).unwrap_or(AutoPoint::new(0.0, 0.0));
                app.drag = Some(Drag::Pl(PlDrag::Point { ch, clip: ci, i, x0: x, y0: y, beat0: p.beat, v0: p.value, lock: None }));
                app.hint = Cow::Owned(value_text(s, ch, p.value));
                return;
            }
            if !right {
                if let Some(i) = handle_near(s, &g, ci, x, y) {
                    snapshot(s);
                    let t0 = s.project.automation(ch).map_or(0.0, |a| a.points[i].tension);
                    app.drag = Some(Drag::Pl(PlDrag::Tension { ch, i, y0: y, t0 }));
                    return;
                }
            }
            if edge(g.clip_rect(&c), x) == 0 && curve_dist(s, &g, ci, x, y).is_some_and(|d| d <= 6.0) {
                // Left-click on the curve adds a point (owner); FL's right-click adds one too.
                snapshot(s);
                let beat = beat_at(s, &g, &c, x);
                let v = value_at_y(g.body(&c), y);
                if let Some(i) = add_point(s, ch, beat, v) {
                    app.drag = Some(Drag::Pl(PlDrag::Point { ch, clip: ci, i, x0: x, y0: y, beat0: beat, v0: v, lock: None }));
                    app.hint = Cow::Owned(value_text(s, ch, v));
                }
                app.invalidate();
                return;
            }
        }
        // The title strip's icon opens the clip menu (§10).
        let r = g.clip_rect(&c);
        if !right && x < r.x + 14.0 && y < r.y + TITLE_H && r.w > 20.0 {
            s.playlist.popup = Some(clip_menu(s, ci, r.x, r.y + TITLE_H));
            return;
        }
    }

    // Ctrl+drag selects in any tool; Ctrl+click toggles a clip (§3).
    if app.ctrl && !right {
        match hit {
            Some(i) => {
                // A grouped clip toggles with its group.
                let group = with_groups(s, vec![i]);
                if s.playlist.sel.contains(&i) {
                    s.playlist.sel.retain(|j| !group.contains(j));
                } else {
                    s.playlist.sel.extend(group.into_iter().filter(|j| !s.playlist.sel.contains(j)).collect::<Vec<_>>());
                }
            }
            None => {
                let base = s.playlist.sel.clone();
                app.drag = Some(Drag::Pl(PlDrag::Marquee { x0: x, y0: y, base }));
            }
        }
        app.invalidate();
        return;
    }

    if right {
        match tool {
            PlTool::Select => {}
            PlTool::Zoom => {
                zoom_toggle(app);
                return;
            }
            // The Slice tool's right button on empty grid still draws a slice line.
            PlTool::Slice if hit.is_none() => {
                app.drag = Some(Drag::Pl(PlDrag::RightPending { x0: x, y0: y }));
                return;
            }
            // Any other tool: the right button is the Delete tool while held (also starting on empty grid).
            _ => {
                snapshot_as(s, "Playlist delete clip");
                app.drag = Some(Drag::Pl(PlDrag::Erase));
                erase_at(app, grid);
                return;
            }
        }
    }

    match tool {
        PlTool::Delete => {
            snapshot_as(s, "Playlist delete clip");
            app.drag = Some(Drag::Pl(PlDrag::Erase));
            erase_at(app, grid);
        }
        PlTool::Mute => {
            let Some(i) = hit else { return };
            snapshot(s);
            let on = !clips(s)[i].muted;
            app.drag = Some(Drag::Pl(PlDrag::Mute { on }));
            mute_at(app, grid, on);
        }
        PlTool::Slip => {
            let Some(i) = hit else { return };
            snapshot(s);
            let orig = clips(s)[i];
            app.drag = Some(Drag::Pl(PlDrag::Slip { x0: x, i, orig }));
        }
        PlTool::Slice => {
            app.drag = Some(Drag::Pl(PlDrag::Slice { y0: y }));
            let t = snap_round(s, &g, tick, alt);
            s.playlist.overlay = Some(Overlay::Slice { tick: t, t0: track, t1: track });
        }
        PlTool::Zoom => app.drag = Some(Drag::Pl(PlDrag::ZoomBox { x0: x, y0: y })),
        PlTool::Playback => {
            let t = snap_down(s, &g, tick, true);
            s.playlist.pos = t;
            app.drag = Some(Drag::Pl(PlDrag::Playback));
            app.play_song_from(t);
        }
        PlTool::Select if hit.is_none() => {
            s.playlist.sel.clear();
            app.drag = Some(Drag::Pl(PlDrag::Marquee { x0: x, y0: y, base: Vec::new() }));
        }
        PlTool::Draw | PlTool::Paint | PlTool::Select => match hit {
            // Right-Shift+drag on empty grid with Draw / Paint: a quick slice line, as the Slice tool (§3).
            None if app.rshift => {
                app.drag = Some(Drag::Pl(PlDrag::Slice { y0: y }));
                let t = snap_round(s, &g, tick, alt);
                s.playlist.overlay = Some(Overlay::Slice { tick: t, t0: track, t1: track });
            }
            None => {
                // FL's name for placing the current source's clip (spec: ui/settings-choices.md "Undo history").
                snapshot_as(s, "Playlist fill paint clip");
                s.playlist.sel.clear();
                let start = snap_place(s, &g, tick, alt);
                let i = place(s, track, start);
                let c = clips(s)[i];
                if tool == PlTool::Paint {
                    app.drag = Some(Drag::Pl(PlDrag::Paint { track, start, len: c.length.max(1) }));
                } else {
                    app.drag = Some(Drag::Pl(PlDrag::Move { x0: x, y0: y, orig: vec![(i, c)], fresh: true }));
                }
            }
            Some(i) => {
                snapshot(s);
                let c = clips(s)[i];
                // Clicking a clip makes its source current (§3).
                s.playlist.source = Some(c.source);
                if let ClipSource::Pattern(n) = c.source {
                    s.pattern = n;
                }
                s.playlist.picker = source_kind(s, c.source);
                // A resized clip lends its length to the clips placed next; a clip of natural length clears it.
                s.playlist.lock_len = (c.length != source_len(s, c.source)).then_some((c.source, c.length));
                // Grouped clips go along (§3a); clicking one selects its whole group.
                let grouped = clips(s)[i].group() != 0;
                let mut group = with_groups(s, if s.playlist.sel.contains(&i) { s.playlist.sel.clone() } else { vec![i] });
                if !s.playlist.sel.contains(&i) {
                    s.playlist.sel = if tool == PlTool::Select || grouped { group.clone() } else { Vec::new() };
                }
                // The clicked clip first.
                group.retain(|&j| j != i);
                group.insert(0, i);
                let r = g.clip_rect(&c);
                let e = edge(r, x);
                if app.shift && e == 0 {
                    // Shift+drag copies: the copies move, the originals stay.
                    let arr = arr_mut(s);
                    let first = arr.clips.len();
                    let copies: Vec<Clip> = group.iter().filter_map(|&j| arr.clips.get(j).copied()).collect();
                    for mut cp in copies {
                        cp.set_id(0);
                        arr.clips.push(cp);
                    }
                    group = (first..arr.clips.len()).collect();
                    regroup_copies(s, first);
                    s.piano.dirty = true;
                }
                let orig: Vec<(usize, Clip)> = group.iter().filter_map(|&j| clips(s).get(j).map(|c| (j, *c))).collect();
                s.playlist.stretch_base = if s.playlist.stretch && e != 0 {
                    orig.iter()
                        .filter_map(|(_, c)| match c.source {
                            ClipSource::Channel(ch) if source_kind(s, c.source) == PickKind::Audio => {
                                // The whole sample's length in beats as the clip shows it (unknown until its waveform is
                                // in): its stretched length, and the channel's pitch where that applies (OS: the clip then
                                // stays under the mouse).
                                let x = channel_of(s, ch)?;
                                let secs = sample_secs(s, x);
                                let k = to_src(s, c.source, 1.0);
                                (secs > 0.0 && k > 0.0).then(|| (ch, secs * 1000.0 / k / f64::from(ppq(s))))
                            }
                            _ => None,
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                app.drag = Some(Drag::Pl(match e {
                    -1 => PlDrag::ResizeL { x0: x, orig },
                    1 => PlDrag::ResizeR { x0: x, orig },
                    _ => PlDrag::Move { x0: x, y0: y, orig, fresh: app.shift },
                }));
                // Held without a move, the clips are cloned (Click-and-hold & special gesture functions).
                if e == 0 && !app.shift {
                    app.hold_start(crate::hold::Kind::Clips);
                }
            }
        },
    }
    app.invalidate();
}

/// Round a tick to the nearest snap line (Slice tool).
fn snap_round(s: &State, g: &PGeom, tick: f32, alt: bool) -> u32 {
    if alt {
        return tick.max(0.0).round() as u32;
    }
    let u = grid_snap(s).ticks(g.ppq, g.bar_w) as f32;
    ((tick / u).round() * u).max(0.0) as u32
}

fn erase_at(app: &mut App, grid: Rect) {
    let (x, y) = app.mouse;
    let g = PGeom::new(&app.state, grid);
    if let Some(i) = clip_at(&app.state, &g, x, y) {
        delete(&mut app.state, vec![i]);
        app.invalidate();
    }
}

fn mute_at(app: &mut App, grid: Rect, on: bool) {
    let (x, y) = app.mouse;
    let g = PGeom::new(&app.state, grid);
    if let Some(i) = clip_at(&app.state, &g, x, y) {
        let s = &mut app.state;
        if let Some(c) = arr_mut(s).clips.get_mut(i) {
            if c.muted != on {
                c.muted = on;
                s.piano.dirty = true;
                app.invalidate();
            }
        }
    }
}

/// Mouse moved during a playlist drag.
pub fn drag_move(app: &mut App, d: PlDrag, grid: Rect) {
    // Drags that only move, resize or select things inside the playlist repaint just the playlist.
    let inside = matches!(
        d,
        PlDrag::Move { .. }
            | PlDrag::ResizeL { .. }
            | PlDrag::ResizeR { .. }
            | PlDrag::Slip { .. }
            | PlDrag::Marquee { .. }
            | PlDrag::ZoomBox { .. }
            | PlDrag::TimeSel { .. }
            | PlDrag::Marker { .. }
            | PlDrag::Point { .. }
            | PlDrag::Tension { .. }
    );
    // Moving, resizing or slipping clips changes only those clips and the overview's miniature; an audio clip's fade,
    // tension or gain handle only that clip (and the hint bar). A stretch-mode resize changes every clip of the
    // stretched channels: the whole window.
    let stretching = !app.state.playlist.stretch_base.is_empty() && matches!(d, PlDrag::ResizeL { .. } | PlDrag::ResizeR { .. });
    let handle = matches!(d, PlDrag::Fade { .. } | PlDrag::FadeTension { .. } | PlDrag::Gain { .. } | PlDrag::Crossfade { .. });
    let moved: Option<Vec<usize>> = match &d {
        _ if stretching => None,
        PlDrag::Move { orig, .. } | PlDrag::ResizeL { orig, .. } | PlDrag::ResizeR { orig, .. } => Some(orig.iter().map(|&(i, _)| i).collect()),
        PlDrag::Slip { i, .. } => Some(vec![*i]),
        PlDrag::Fade { clip, .. } | PlDrag::FadeTension { clip, .. } | PlDrag::Gain { clip, .. } => Some(vec![*clip]),
        PlDrag::Crossfade { a, b, .. } => Some(vec![*a, *b]),
        _ => None,
    };
    let rects = |s: &State, idx: &[usize]| -> Vec<Rect> {
        let g = PGeom::new(s, grid);
        idx.iter().filter_map(|&i| clips(s).get(i)).map(|c| g.clip_rect(c).outset(6.0).intersect(&grid)).collect()
    };
    let before = moved.as_deref().map(|idx| rects(&app.state, idx));
    let (x, y) = app.mouse;
    let alt = app.alt;
    let shift = app.shift;
    let s = &mut app.state;
    let g = PGeom::new(s, grid);
    match d {
        PlDrag::Move { x0, y0, orig, .. } => {
            let mut dt = snap_delta(s, &g, (x - x0) / g.ppt, alt);
            let min_pos = orig.iter().map(|(_, c)| i64::from(c.position)).min().unwrap_or(0);
            dt = dt.max(-min_pos);
            let dk = i32::from(g.track_at(y)) - i32::from(g.track_at(y0));
            let lo = orig.iter().map(|(_, c)| i32::from(c.track)).min().unwrap_or(0);
            let hi = orig.iter().map(|(_, c)| i32::from(c.track)).max().unwrap_or(0);
            let dk = dk.clamp(-lo, i32::from(TRACKS) - 1 - hi);
            let arr = arr_mut(s);
            for &(i, o) in &orig {
                if let Some(c) = arr.clips.get_mut(i) {
                    c.position = (i64::from(o.position) + dt) as u32;
                    c.track = (i32::from(o.track) + dk) as u16;
                }
            }
            s.piano.dirty = true;
        }
        PlDrag::ResizeR { x0, orig } => {
            let dt = snap_delta(s, &g, (x - x0) / g.ppt, alt);
            let min = if alt { 1 } else { grid_snap(s).ticks(g.ppq, g.bar_w) } as i64;
            for &(i, o) in &orig {
                let len = (i64::from(o.length) + dt).max(min) as u32;
                if !stretch_clip(s, i, &o, o.position, len) {
                    resize_right(s, i, &o, len);
                }
            }
        }
        PlDrag::ResizeL { x0, orig } => {
            let dt = snap_delta(s, &g, (x - x0) / g.ppt, alt);
            for &(i, o) in &orig {
                let dt = dt.clamp(-i64::from(o.position), i64::from(o.length) - 1);
                let (pos, len) = ((i64::from(o.position) + dt) as u32, (i64::from(o.length) - dt) as u32);
                if !stretch_clip(s, i, &o, pos, len) {
                    resize_left(s, i, &o, dt);
                }
            }
        }
        PlDrag::Slip { x0, i, orig } => {
            let dt = snap_delta(s, &g, (x - x0) / g.ppt, alt);
            slip(s, i, &orig, dt);
        }
        PlDrag::Erase => return erase_at(app, grid),
        PlDrag::Mute { on } => return mute_at(app, grid, on),
        PlDrag::Paint { track, start, len } => {
            // Clips back to back along the track from the first, up to the mouse (either direction).
            let t = g.tick_at(x).max(0.0) as i64;
            let k = (t - i64::from(start)).div_euclid(i64::from(len));
            let src = source(s);
            for j in k.min(0)..=k.max(0) {
                let pos = i64::from(start) + j * i64::from(len);
                if pos < 0 {
                    continue;
                }
                let pos = pos as u32;
                let taken = clips(s).iter().any(|c| c.track == track && c.position < pos + len && c.end() > pos);
                if !taken {
                    arr_mut(s).clips.push(Clip::new(pos, len, track, src));
                    s.piano.dirty = true;
                }
            }
        }
        PlDrag::Marquee { x0, y0, base } => {
            let r = Rect::span(x0.min(x), x0.max(x), y0.min(y), y0.max(y));
            s.playlist.overlay = Some(Overlay::Marquee(r));
            let mut sel = base.clone();
            for (i, c) in clips(s).iter().enumerate() {
                if !sel.contains(&i) && g.clip_rect(c).intersect(&r).w > 0.0 && g.clip_rect(c).intersect(&r).h > 0.0 {
                    sel.push(i);
                }
            }
            s.playlist.sel = sel;
        }
        PlDrag::Slice { y0 } => {
            let t = snap_round(s, &g, g.tick_at(x), alt);
            let (a, b) = (g.track_at(y0.min(y)), g.track_at(y0.max(y)));
            s.playlist.overlay = Some(Overlay::Slice { tick: t, t0: a, t1: b });
        }
        PlDrag::RightPending { x0, y0 } => {
            if (x - x0).abs() > 4.0 || (y - y0).abs() > 4.0 {
                // From now on it is a slice line that follows the mouse.
                let t = snap_round(s, &g, g.tick_at(x), alt);
                let (a, b) = (g.track_at(y0.min(y)), g.track_at(y0.max(y)));
                s.playlist.overlay = Some(Overlay::Slice { tick: t, t0: a, t1: b });
                app.drag = Some(Drag::Pl(PlDrag::Slice { y0 }));
            }
        }
        PlDrag::ZoomBox { x0, y0 } => {
            s.playlist.overlay = Some(Overlay::ZoomBox(Rect::span(x0.min(x), x0.max(x), y0.min(y), y0.max(y))));
        }
        PlDrag::Playback => {}
        PlDrag::SongPos => {
            s.playlist.pos = snap_down(s, &g, g.tick_at(x), alt);
            crate::init::apply(s);
        }
        PlDrag::TimeSel { t0 } => {
            let t = snap_round(s, &g, g.tick_at(x), alt);
            s.playlist.range = (t != t0).then(|| (t0.min(t), t0.max(t)));
        }
        PlDrag::Marker { i, t0, .. } => {
            let t = snap_round(s, &g, g.tick_at(x), alt);
            let _ = t0;
            if let Some(m) = arr_mut(s).markers.get_mut(i) {
                m.position = t;
            }
        }
        PlDrag::Point { ch, clip, i, x0, y0, beat0, v0, lock } => {
            let Some(c) = clips(s).get(clip).copied() else { return };
            let body = g.body(&c);
            // Shift locks the axis that moved most first (owner, §8).
            let lock = if shift { lock.or_else(|| ((x - x0).abs() > 3.0 || (y - y0).abs() > 3.0).then(|| (x - x0).abs() >= (y - y0).abs())) } else { None };
            app.drag = Some(Drag::Pl(PlDrag::Point { ch, clip, i, x0, y0, beat0, v0, lock }));
            let s = &mut app.state;
            let mut beat = beat0 + f64::from((x - x0) / g.ppt) / f64::from(g.ppq);
            if !alt {
                let u = f64::from(grid_snap(s).ticks(g.ppq, g.bar_w)) / f64::from(g.ppq);
                let (w0, _) = window(s, &c);
                beat = w0 + ((beat - w0) / u).round() * u;
            }
            let mut v = v0 + f64::from((y0 - y) / (body.h - 4.0).max(1.0));
            match lock {
                Some(true) => v = v0,
                Some(false) => beat = beat0,
                None => {}
            }
            move_point(s, ch, i, beat, v);
            let v = s.project.automation(ch).and_then(|a| a.points.get(i)).map_or(v, |p| p.value);
            app.hint = Cow::Owned(value_text(&app.state, ch, v));
        }
        PlDrag::Tension { ch, i, y0, t0 } => {
            if let Some(p) = s.project.automation_mut(ch).and_then(|a| a.points.get_mut(i)) {
                p.tension = (t0 + (y0 - y) / 80.0).clamp(-1.0, 1.0);
                s.piano.dirty = true;
            }
        }
        PlDrag::Fade { clip, out } => {
            let Some(c) = clips(s).get(clip).copied() else { return };
            let body = g.body(&c);
            let (fx_in, fx_out) = fade_xs(s, &g, &c);
            // A fade reaches at most to where the other one starts.
            let px = if out { (body.right() - x).clamp(0.0, body.right() - fx_in) } else { (x - body.x).clamp(0.0, fx_out - body.x) };
            let ms = (f64::from(px / g.ppt) * fade_ms_per_tick(s)) as f32;
            let c = &mut arr_mut(s).clips[clip];
            let on = (px >= 2.0).then_some(ms);
            // A fade set with its handle gets its bit (and leaves the automatic crossfades); dragged to nothing, the
            // edge is theirs again (spec: audio/audio-clips.md "Automatic crossfades").
            if out {
                let t = c.fade_out_tension();
                c.set_fade_out(on.map(|ms| (ms, t)));
            } else {
                let t = c.fade_in_tension();
                c.set_fade_in(on.map(|ms| (ms, t)));
            }
            let c = *c;
            app.hint = Cow::Owned(handle_hint(&c, if out { Handle::FadeOut } else { Handle::FadeIn }));
            app.state.piano.dirty = true;
        }
        PlDrag::FadeTension { clip, out, y0, t0 } => {
            let t = (t0 + (y0 - y) * TENSION_PER_PX).clamp(-40.0, 40.0);
            let c = &mut arr_mut(s).clips[clip];
            // The tension alone: the length and the bit stay (a stored fade without its bit plays with the switch off).
            if out {
                if c.stored_fade_out().is_some() {
                    c.set_fade_out_tension(t);
                }
            } else if c.stored_fade_in().is_some() {
                c.set_fade_in_tension(t);
            }
            let c = *c;
            app.hint = Cow::Owned(handle_hint(&c, if out { Handle::TensionOut } else { Handle::TensionIn }));
            app.state.piano.dirty = true;
        }
        PlDrag::Gain { clip, y0, db0 } => {
            let db = (db0 + (y0 - y) * DB_PER_PX).clamp(-60.0, 24.0);
            let c = &mut arr_mut(s).clips[clip];
            c.set_gain(10f32.powf(db / 20.0));
            let c = *c;
            app.hint = Cow::Owned(handle_hint(&c, Handle::Gain));
            app.state.piano.dirty = true;
        }
        PlDrag::Crossfade { a, b, y0, m0 } => {
            // The later clip's gain at the middle moves 1/120 per logical pixel; its fade-in takes the tension that
            // gives it, the earlier clip's fade-out the negative (§9a).
            let t = os_model::crossfade::tension_for_mid(m0 + f64::from(y0 - y) / XFADE_PX) as f32;
            let arr = arr_mut(s);
            if a < arr.clips.len() && b < arr.clips.len() {
                arr.clips[b].set_fade_in_tension(t);
                arr.clips[a].set_fade_out_tension(os_model::crossfade::opposite(t));
                let c = arr.clips[b];
                app.hint = Cow::Owned(handle_hint(&c, Handle::Crossfade));
                app.state.piano.dirty = true;
            }
        }
    }
    match (moved, before) {
        (Some(idx), Some(mut dirty)) => {
            dirty.extend(rects(&app.state, &idx));
            if handle {
                dirty.push(crate::app::HINT_RECT);
            } else {
                dirty.push(crate::views::playlist::overview_rect(&app.state, grid).outset(1.0));
            }
            for r in dirty {
                app.damage_rect(r);
            }
        }
        _ if inside || stretching => app.damage_win(crate::app::Win::Playlist),
        _ => app.invalidate(),
    }
}

/// Button released.
pub fn drag_end(app: &mut App, d: PlDrag, grid: Rect) {
    let s = &mut app.state;
    let g = PGeom::new(s, grid);
    let overlay = s.playlist.overlay.take();
    // A resized clip's new length is used for the next clips of its source.
    if let PlDrag::ResizeL { orig, .. } | PlDrag::ResizeR { orig, .. } = &d
        && let Some(c) = orig.first().and_then(|&(i, _)| clips(s).get(i))
    {
        s.playlist.lock_len = Some((c.source, c.length));
    }
    match d {
        PlDrag::Slice { .. } => {
            if let Some(Overlay::Slice { tick, t0, t1 }) = overlay {
                snapshot(s);
                if slice(s, tick, t0, t1) == 0 {
                    crate::history::unrecord(s);
                }
            }
        }
        PlDrag::ZoomBox { x0, y0 } => zoom_to(app, grid, x0, y0),
        PlDrag::Playback => app.transport.stop(),
        PlDrag::SongPos => {
            // The song jumps to the new position; a running pattern goes on as the song from there, a paused one
            // stops, as with the PAT / SONG switch (§7, audio/transport.md).
            if app.transport.playing() {
                if app.transport.song() || !app.transport.paused() {
                    let p = app.state.playlist.pos;
                    app.play_song_from(p);
                } else {
                    app.transport.stop();
                }
            }
        }
        PlDrag::ResizeL { .. } | PlDrag::ResizeR { .. } if !s.playlist.stretch_base.is_empty() => {
            // Stretched channels: their Lemon Sampler takes the new stretch settings (§9b).
            for (ch, _) in std::mem::take(&mut s.playlist.stretch_base) {
                let Some(row) = s.project.channels.iter().position(|c| c.index == ch) else { continue };
                app.plugins.set_sampler_stretch(row, &s.project.channels[row].sound);
            }
        }
        PlDrag::Move { orig, fresh: false, .. } | PlDrag::ResizeL { orig, .. } | PlDrag::ResizeR { orig, .. } => {
            // A click that changed nothing leaves no undo step.
            let same = orig.iter().all(|(i, o)| clips(s).get(*i) == Some(o));
            if same && !app.shift {
                crate::history::unrecord(s);
            }
        }
        _ => {}
    }
    let _ = g;
    app.invalidate();
}

/// Zoom tool / Ctrl+right-drag: zoom the time axis to the dragged box (a click zooms in 2x).
fn zoom_to(app: &mut App, grid: Rect, x0: f32, y0: f32) {
    let (x1, y1) = app.mouse;
    let s = &mut app.state;
    let g = PGeom::new(s, grid);
    s.playlist.zoom_back = Some((s.playlist_bar_w, s.playlist_scroll.0, s.playlist_scroll.1));
    let bar = (g.ppq * 4) as f32;
    if (x1 - x0).abs() < 4.0 {
        let t = g.tick_at(x1);
        s.playlist_bar_w = (s.playlist_bar_w * 2.0).min(600.0);
        s.playlist_scroll.0 = (t / bar * s.playlist_bar_w - (x1 - grid.x)).max(0.0);
    } else {
        let (t0, t1) = (g.tick_at(x0.min(x1)).max(0.0), g.tick_at(x0.max(x1)).max(0.0));
        s.playlist_bar_w = (grid.w / ((t1 - t0).max(1.0) / bar)).clamp(4.0, 600.0);
        s.playlist_scroll.0 = t0 / bar * s.playlist_bar_w;
        s.playlist_scroll.1 = (s.playlist_scroll.1 + y0.min(y1) - grid.y).max(0.0);
    }
}

/// Zoom tool right-click: the default zoom <-> the last zoom.
fn zoom_toggle(app: &mut App) {
    let s = &mut app.state;
    let normal = crate::views::playlist::BAR_W;
    if (s.playlist_bar_w - normal).abs() > 0.01 {
        s.playlist.zoom_back = Some((s.playlist_bar_w, s.playlist_scroll.0, s.playlist_scroll.1));
        s.playlist_bar_w = normal;
        s.playlist_scroll.0 = 0.0;
    } else if let Some((z, sx, sy)) = s.playlist.zoom_back {
        s.playlist.zoom_back = Some((s.playlist_bar_w, s.playlist_scroll.0, s.playlist_scroll.1));
        s.playlist_bar_w = z;
        s.playlist_scroll = (sx, sy);
    }
    app.invalidate();
}

/// Zoom so the clips `idx` (or all) fill the view (Shift+Z, "Zoom on selection").
pub fn zoom_on(s: &mut State, grid: (f32, f32), idx: &[usize]) {
    let cs: Vec<Clip> = idx.iter().filter_map(|&i| clips(s).get(i).copied()).collect();
    let (Some(a), Some(b)) = (cs.iter().map(|c| c.position).min(), cs.iter().map(|c| c.end()).max()) else { return };
    let bar = bar(s) as f32;
    let span = ((b - a) as f32 / bar).max(1.0);
    s.playlist_bar_w = (grid.0 / span).clamp(4.0, 600.0);
    s.playlist_scroll.0 = a as f32 / bar * s.playlist_bar_w;
    let top = cs.iter().map(|c| c.track).min().unwrap_or(0);
    // The selected tracks fill the view's height, on FL's 2 px height steps (§5; FL measured only at the maximum).
    let n = f32::from(cs.iter().map(|c| c.track).max().unwrap_or(0) - top + 1);
    let fit = ROW_MIN + ((grid.1 / n - ROW_MIN) / 2.0).floor() * 2.0;
    s.playlist.row_h = fit.clamp(ROW_MIN, ROW_MAX);
    s.playlist_scroll.1 = Rows::new(s).top(top);
}

/// The current arrangement's name.
pub fn arrangement_name(s: &State) -> String {
    s.project.arrangement().and_then(|a| a.name.clone()).unwrap_or_else(|| "Arrangement".into())
}

fn cmd_is_clone(c: &Cmd) -> bool {
    matches!(c, Cmd::ArrClone)
}

/// Shows arrangement number `n` (event 100, spec: flp/events/playlist.md): selection and undo belong to the one
/// left; a playing song continues with the new one.
pub fn show_arrangement(s: &mut State, n: u16) {
    s.project.current_arrangement = Some(n);
    s.playlist.sel.clear();
    s.playlist.hover_clip = None;
    s.playlist.hover_handle = None;
    crate::history::forget_playlist(s);
    s.piano.dirty = true;
}

/// The arrangement menu of the playlist title (§12, as FL 26 shows it): the arrangements (the current one ticked),
/// then Clone, Rename, Delete, Merge with, Add one.
pub fn arrangement_menu(s: &State, x: f32, y: f32) -> Popup {
    let cur = s.project.arrangement().map(|a| a.number);
    let mut items = vec![PItem::head("Arrangements")];
    for a in &s.project.arrangements {
        let name = a.name.clone().unwrap_or_else(|| "Arrangement".into());
        items.push(PItem::new(name, Cmd::ArrSelect(a.number)).mark(Mark::Radio(Some(a.number) == cur)));
    }
    let others: Vec<PItem> = s
        .project
        .arrangements
        .iter()
        .filter(|a| Some(a.number) != cur)
        .map(|a| PItem::new(a.name.clone().unwrap_or_else(|| "Arrangement".into()), Cmd::ArrMerge(a.number)))
        .collect();
    items.push(PItem::head("Current Arrangement"));
    items.push(PItem::new("Clone...", Cmd::ArrClone));
    items.push(PItem::new("Rename...", Cmd::ArrRename));
    let delete = PItem::new("Delete", Cmd::ArrDelete);
    items.push(if s.project.arrangements.len() > 1 { delete } else { delete.off() });
    items.push(if others.is_empty() { PItem::new("Merge with...", Cmd::Nothing).off() } else { PItem::sub("Merge with...", others) });
    items.push(PItem::sep());
    items.push(PItem::new("Add one...", Cmd::ArrAdd));
    Popup { x, y, items, sub: None }
}

/// Collapses or expands the group led by track `t` or by `t`'s group track (§6a); no-op for an ungrouped track
/// without members.
pub fn toggle_collapse(s: &mut State, t: u16) {
    let Some(g) = group_track(s, t).or_else(|| is_group_track(s, t).then_some(t)) else { return };
    snapshot(s);
    let tr = &mut arr_mut(s).tracks[usize::from(g)];
    tr.collapsed = !tr.collapsed;
}

/// Ruler press: left moves the song position (playback follows), right starts a time selection (§7).
pub fn press_ruler(app: &mut App, grid: Rect, right: bool) {
    let x = app.mouse.0;
    let alt = app.alt;
    let s = &mut app.state;
    s.playlist.popup = None;
    let g = PGeom::new(s, grid);
    let t = snap_round(s, &g, g.tick_at(x), alt);
    if right {
        s.playlist.range = None;
        app.drag = Some(Drag::Pl(PlDrag::TimeSel { t0: t }));
    } else {
        s.playlist.pos = snap_down(s, &g, g.tick_at(x), alt);
        // Moving the song position switches to SONG mode; a time selection does not (§7). The initial values follow
        // (spec: ui/settings-choices.md "Advanced: Initialize controls automatically").
        s.song_mode = true;
        crate::init::apply(s);
        app.drag = Some(Drag::Pl(PlDrag::SongPos));
    }
    app.invalidate();
}

/// The marker under x in the marker lane.
pub fn marker_at(s: &State, g: &PGeom, x: f32) -> Option<usize> {
    markers(s).iter().enumerate().rev().find(|(_, m)| {
        let mx = g.x_of(m.position as f32);
        x >= mx - 2.0 && x <= mx + marker_w(m)
    }).map(|(i, _)| i)
}

/// Width of a marker tab.
pub fn marker_w(m: &TimeMarker) -> f32 {
    (m.name.chars().count() as f32 * 7.0 + 18.0).max(24.0)
}

pub fn press_markers(app: &mut App, grid: Rect, right: bool) {
    let x = app.mouse.0;
    let s = &mut app.state;
    s.playlist.popup = None;
    let g = PGeom::new(s, grid);
    let Some(i) = marker_at(s, &g, x) else { return };
    if right {
        s.playlist.popup = Some(marker_menu(i, x, app.mouse.1));
    } else {
        snapshot(s);
        let t0 = markers(s)[i].position;
        app.drag = Some(Drag::Pl(PlDrag::Marker { i, dt: 0, t0 }));
    }
    app.invalidate();
}

/// Track header press (§6): name area selects (double-click renames); right-click opens the track menu.
pub fn press_header(app: &mut App, t: u16, right: bool, at: (f32, f32)) {
    let s = &mut app.state;
    s.playlist.popup = None;
    if right {
        s.playlist.popup = Some(track_menu(s, t, app.mouse.0, app.mouse.1));
        return app.invalidate();
    }
    let now = std::time::Instant::now();
    let double = s.playlist.last_header.is_some_and(|(lt, when)| lt == t && now.duration_since(when).as_millis() < 450);
    s.playlist.track = t;
    s.playlist.last_header = Some((t, now));
    if double {
        start_edit(s, EditTarget::TrackName(t), at);
    }
    app.invalidate();
}

/// Track LED: click mutes, Ctrl+click or right-click solos (§6).
pub fn press_led(app: &mut App, t: u16, right: bool) {
    snapshot(&mut app.state);
    if right || app.ctrl {
        toggle_solo(&mut app.state, t);
    } else {
        toggle_track(&mut app.state, t);
    }
    app.invalidate();
}

pub fn start_edit(s: &mut State, target: EditTarget, at: (f32, f32)) {
    let text = match target {
        EditTarget::TrackName(t) => track_name(s, t),
        EditTarget::ArrangementName => arrangement_name(s),
        EditTarget::MarkerName(i) => markers(s).get(i).map(|m| m.name.clone()).unwrap_or_default(),
        EditTarget::NewMarker(_) => String::new(),
        EditTarget::PointValue(ch, i) => {
            let v = s.project.automation(ch).and_then(|a| a.points.get(i)).map_or(0.0, |p| p.value);
            match s.project.automation(ch).and_then(|a| a.target()).map(target::decode) {
                Some(target::Target::Tempo) => format!("{:.3}", auto_tempo(v)),
                _ => format!("{:.1}%", v * 100.0),
            }
        }
        EditTarget::PatternName(n) => source_name(s, ClipSource::Pattern(n)),
        EditTarget::MixerName(n) => crate::mixer::name(s, n),
        EditTarget::SlotName(t, k) => crate::mixer::slot_name(s, t, k).unwrap_or_default(),
        EditTarget::TypeIn(t, norm) => crate::typein::text(s, t, norm),
    };
    s.playlist.edit = Some(TextEdit { target, text, at, sel: false });
}

/// Enter in the text box.
pub fn commit_edit(s: &mut State) {
    let Some(e) = s.playlist.edit.take() else { return };
    snapshot(s);
    let text = e.text.trim().to_string();
    match e.target {
        EditTarget::ArrangementName => {
            if !text.trim().is_empty() {
                arr_mut(s).name = Some(text.trim().to_string());
            }
        }
        EditTarget::TrackName(t) => {
            let def = format!("Track {}", t + 1);
            if let Some(tr) = arr_mut(s).tracks.get_mut(usize::from(t)) {
                tr.name = (!text.is_empty() && text != def).then_some(text);
            }
        }
        EditTarget::MarkerName(i) => {
            if let Some(m) = arr_mut(s).markers.get_mut(i) {
                m.name = text;
            }
        }
        EditTarget::NewMarker(pos) => {
            let name = if text.is_empty() { format!("Marker {}", markers(s).len() + 1) } else { text };
            arr_mut(s).markers.push(TimeMarker::new(pos, &name));
        }
        EditTarget::PointValue(ch, i) => {
            if let Some(v) = parse_value(s, ch, &text) {
                let beat = s.project.automation(ch).and_then(|a| a.points.get(i)).map_or(0.0, |p| p.beat);
                move_point(s, ch, i, beat, v);
            }
        }
        EditTarget::MixerName(n) => {
            let def = if n == 0 { "Master".to_string() } else { format!("Insert {n}") };
            if let Some(m) = s.project.mixer.iter_mut().find(|m| m.index == n) {
                m.name = (!text.is_empty() && text != def).then_some(text);
            }
        }
        EditTarget::SlotName(t, k) => {
            if text.is_empty() {
                s.mixer.slot_names.remove(&(t, k));
            } else {
                s.mixer.slot_names.insert((t, k), text);
            }
        }
        EditTarget::TypeIn(t, norm) => crate::typein::accept(s, t, norm, &text),
        EditTarget::PatternName(n) => {
            if let Some(p) = s.project.patterns.iter_mut().find(|p| p.number == n) {
                p.name = (!text.is_empty()).then_some(text);
            } else if !text.is_empty() {
                s.project.patterns.push(os_model::Pattern { number: n, name: Some(text), ..Default::default() });
            }
        }
    }
    s.piano.dirty = true;
}

// ---------------------------------------------------------------------------------------------------------
// Commands

/// Run a menu / shortcut command. Returns a status message, if any.
pub fn run(app: &mut App, cmd: Cmd) -> Option<String> {
    let grid = app.pl_grid();
    app.state.playlist.popup = None;
    // Commands that need the whole app.
    match cmd {
        Cmd::Browser(b) => return app.browser_cmd(b),
        Cmd::Plugin(c) => return crate::plugin_menu::run(app, c),
        Cmd::Rack(c) => return crate::rack::run(app, c),
        Cmd::Mixer(c) => return crate::mixer::run(app, c),
        Cmd::Param(c) => return app.params_cmd(c),
        Cmd::Pattern(c) => {
            let m = app.pattern_cmd(c);
            app.invalidate();
            return m;
        }
        Cmd::ClockMode(m) => {
            app.state.clock.mode = m;
            app.state.clock.save();
            app.invalidate();
            return None;
        }
        Cmd::Toolbar(sw) => {
            app.toolbar_switch(sw);
            return None;
        }
        Cmd::ClockBeat(on) => {
            app.state.clock.beat = on;
            app.state.clock.save();
            app.invalidate();
            return None;
        }
        Cmd::TimeSig(den, v) => {
            app.sig_pick(den, v);
            return None;
        }
        Cmd::TypeIn(t) => {
            let at = app.mouse;
            crate::typein::open(&mut app.state, t, at);
            app.invalidate();
            return None;
        }
        Cmd::Preset(c) => return crate::presets::run(app, c),
        Cmd::Piano(c) => {
            let m = crate::piano_tools::run(app, c);
            app.invalidate();
            return m;
        }
        Cmd::Misc(row, m, v) => {
            crate::misc::set(app, row, m, v);
            return None;
        }
        Cmd::Audio(a) => {
            app.set_audio(a);
            return None;
        }
        Cmd::Cpu(c) => return app.cpu_cmd(c),
        Cmd::Pref(c, i) => return app.pref_choose(c, i),
        Cmd::Genre(g) => return app.set_genre(g),
        Cmd::Template(t) => return app.set_template(&t),
        Cmd::Messages(p) => {
            app.invalidate();
            return Some(crate::messages::pick(p));
        }
        Cmd::DataFolder(c) => return app.data_folder_cmd(c),
        Cmd::MetronomeSound(sound) => {
            crate::metronome::set_sound(sound);
            app.invalidate();
            return Some(format!("Metronome sound: {}", sound.label()));
        }
        Cmd::Bar(beats, steps) => return app.set_bar(beats, steps),
        Cmd::TimeMode(signature) => return app.set_time_mode(signature),
        Cmd::Timebase(ppq) => return app.set_timebase(ppq),
        Cmd::PanningLaw(law) => return app.set_panning_law(law),
        Cmd::ContinuousScrolling => {
            let on = crate::autoscroll::toggle_continuous();
            app.invalidate();
            return Some(if on { "Continuous scrolling on" } else { "Continuous scrolling off" }.into());
        }
        #[cfg(plugin_windows)]
        Cmd::PluginDetach(row, on) => {
            app.plugins.set_detached(row, on);
            app.state.raise(crate::app::Win::Hosted(row));
            app.invalidate();
            return None;
        }
        Cmd::Consolidate => {
            let m = consolidate(app);
            app.invalidate();
            return Some(m);
        }
        Cmd::ExportTracks => {
            let m = export_tracks(app);
            app.invalidate();
            return Some(m);
        }
        Cmd::QuantizeStarts => {
            let m = quantize_starts(&mut app.state);
            app.invalidate();
            return Some(m);
        }
        Cmd::PointValue(ch, i) => {
            let at = app.mouse;
            start_edit(&mut app.state, EditTarget::PointValue(ch, i), at);
            app.invalidate();
            return None;
        }
        Cmd::ClipPreview(i) => {
            if let Some(c) = clips(&app.state).get(i).copied() {
                app.preview_clip(&c);
            }
            return None;
        }
        Cmd::Pick(c, ch) => {
            let m = pick(app, c, ch);
            app.invalidate();
            return m;
        }
        _ => {}
    }
    let s = &mut app.state;
    let grid_w = grid.map_or(1400.0, |g| g.w);
    let grid_wh = grid.map_or((1400.0, 800.0), |g| (g.w, g.h));
    let msg = match cmd {
        Cmd::Nothing | Cmd::Pick(..) | Cmd::Browser(_) | Cmd::Plugin(_) | Cmd::Rack(_) | Cmd::Mixer(_) | Cmd::Param(_) | Cmd::Preset(_) | Cmd::Misc(..) | Cmd::PluginDetach(..) | Cmd::Piano(_) | Cmd::Audio(_) | Cmd::Cpu(_) | Cmd::MetronomeSound(_) | Cmd::ContinuousScrolling | Cmd::Pattern(_) | Cmd::ClockMode(_) | Cmd::ClockBeat(_) | Cmd::Toolbar(_) | Cmd::TimeSig(..) | Cmd::Bar(..) | Cmd::PanningLaw(_) | Cmd::TimeMode(_) | Cmd::Timebase(_) => None,
        Cmd::NotYet(name) => Some(format!("{name}: not available yet")),
        Cmd::Tool(t) => {
            s.playlist.tool = t;
            None
        }
        Cmd::Snap(p) => {
            s.playlist.snap = p;
            crate::snap::save(s);
            Some(format!("Snap: {}", p.label()))
        }
        Cmd::GlobalSnap(sn) => Some(crate::snap::set(s, sn)),
        Cmd::Undo => crate::history::ctrl_z(s),
        Cmd::Copy => {
            copy(s);
            None
        }
        Cmd::Cut => {
            copy(s);
            snapshot(s);
            let sel = s.playlist.sel.clone();
            delete(s, sel);
            None
        }
        Cmd::Paste => {
            snapshot(s);
            paste(s);
            None
        }
        Cmd::Duplicate => {
            snapshot(s);
            duplicate(s);
            None
        }
        Cmd::Delete => {
            snapshot(s);
            let sel = s.playlist.sel.clone();
            delete(s, sel);
            None
        }
        Cmd::ShiftTime(d) => {
            snapshot(s);
            let u = grid.map_or(ppq(s), |g| grid_snap(s).ticks(ppq(s), PGeom::new(s, g).bar_w)) as i64;
            let idx = targets(s);
            let arr = arr_mut(s);
            let min = idx.iter().filter_map(|&i| arr.clips.get(i)).map(|c| i64::from(c.position)).min().unwrap_or(0);
            let dt = (i64::from(d) * u).max(-min);
            for &i in &idx {
                if let Some(c) = arr.clips.get_mut(i) {
                    c.position = (i64::from(c.position) + dt) as u32;
                }
            }
            s.piano.dirty = true;
            None
        }
        Cmd::ShiftTracks(d) => {
            snapshot(s);
            shift_tracks(s, d);
            None
        }
        Cmd::Mute(on) => {
            snapshot(s);
            let idx = targets(s);
            let arr = arr_mut(s);
            for i in idx {
                if let Some(c) = arr.clips.get_mut(i) {
                    c.muted = on;
                }
            }
            s.piano.dirty = true;
            None
        }
        Cmd::MakeUnique => {
            let sel = s.playlist.sel.clone();
            snapshot(s);
            let made: Vec<u16> = sel.iter().filter_map(|&i| make_unique(s, i)).collect();
            if made.is_empty() { Some("Make unique: select a pattern clip".into()) } else { None }
        }
        Cmd::InsertSpace => {
            snapshot(s);
            insert_space(s);
            None
        }
        Cmd::GroupClips => {
            snapshot(s);
            match group_clips(s) {
                0 => {
                    crate::history::unrecord(s);
                    Some("Group: select two or more clips first".into())
                }
                n => Some(format!("Grouped {n} clips")),
            }
        }
        Cmd::UngroupClips => {
            snapshot(s);
            match ungroup_clips(s) {
                0 => {
                    crate::history::unrecord(s);
                    Some("Ungroup: no grouped clip is selected".into())
                }
                n => Some(format!("Ungrouped {n} clips")),
            }
        }
        Cmd::CycleSnap => {
            s.playlist.snap = next_snap(s.playlist.snap);
            crate::snap::save(s);
            Some(format!("Snap: {}", s.playlist.snap.label()))
        }
        Cmd::DeleteSpace => {
            snapshot(s);
            (!delete_space(s)).then(|| "Delete space: make a time selection first (right-drag the ruler)".into())
        }
        Cmd::Consolidate | Cmd::ExportTracks | Cmd::QuantizeStarts | Cmd::PointValue(..) | Cmd::ClipPreview(_) | Cmd::TypeIn(_) => None,
        Cmd::StretchMode => {
            s.playlist.stretch = !s.playlist.stretch;
            Some(if s.playlist.stretch { "Stretch mode on: resizing an audio clip stretches it".into() } else { "Stretch mode off".into() })
        }
        Cmd::SelectAll => {
            s.playlist.sel = (0..clips(s).len()).collect();
            None
        }
        Cmd::Deselect => {
            s.playlist.sel.clear();
            s.playlist.range = None;
            None
        }
        Cmd::InvertSelection => {
            let sel = s.playlist.sel.clone();
            s.playlist.sel = (0..clips(s).len()).filter(|i| !sel.contains(i)).collect();
            None
        }
        Cmd::SelectMuted => {
            s.playlist.sel = clips(s).iter().enumerate().filter(|(_, c)| c.muted).map(|(i, _)| i).collect();
            None
        }
        Cmd::SelectSource => {
            let src = source(s);
            s.playlist.sel = clips(s).iter().enumerate().filter(|(_, c)| c.source == src).map(|(i, _)| i).collect();
            None
        }
        Cmd::SelectTime => {
            let idx = targets(s);
            let a = idx.iter().filter_map(|&i| clips(s).get(i)).map(|c| c.position).min();
            let b = idx.iter().filter_map(|&i| clips(s).get(i)).map(Clip::end).max();
            s.playlist.range = a.zip(b);
            None
        }
        Cmd::ZoomIn | Cmd::ZoomOut => {
            let k = if cmd == Cmd::ZoomIn { 1.25 } else { 0.8 };
            let center = (s.playlist_scroll.0 + grid_w / 2.0) / s.playlist_bar_w;
            s.playlist_bar_w = (s.playlist_bar_w * k).clamp(4.0, 600.0);
            s.playlist_scroll.0 = (center * s.playlist_bar_w - grid_w / 2.0).max(0.0);
            None
        }
        Cmd::ZoomAll => {
            let idx: Vec<usize> = (0..clips(s).len()).collect();
            zoom_on(s, grid_wh, &idx);
            None
        }
        Cmd::ZoomSelection => {
            let idx = targets(s);
            zoom_on(s, grid_wh, &idx);
            None
        }
        Cmd::AddMarker | Cmd::AddSignature => {
            let pos = s.playlist.pos;
            let at = grid.map_or((600.0, 200.0), |g| (PGeom::new(s, g).x_of(pos as f32), g.y));
            if cmd == Cmd::AddSignature {
                snapshot(s);
                arr_mut(s).markers.push(TimeMarker { kind: TimeMarker::SIGNATURE, ..TimeMarker::new(pos, "4/4") });
                s.piano.dirty = true;
            } else {
                start_edit(s, EditTarget::NewMarker(pos), at);
            }
            None
        }
        Cmd::DeleteMarker(i) => {
            snapshot(s);
            let arr = arr_mut(s);
            if i < arr.markers.len() {
                arr.markers.remove(i);
            }
            s.piano.dirty = true;
            None
        }
        Cmd::RenameMarker(i) => {
            let at = grid.zip(markers(s).get(i)).map_or((600.0, 200.0), |(g, m)| (PGeom::new(s, g).x_of(m.position as f32), g.y - MARKER_H));
            start_edit(s, EditTarget::MarkerName(i), at);
            None
        }
        Cmd::ClipMute(i) => {
            snapshot(s);
            if let Some(c) = arr_mut(s).clips.get_mut(i) {
                c.muted = !c.muted;
            }
            s.piano.dirty = true;
            None
        }
        Cmd::ClipDelete(i) => {
            snapshot(s);
            delete(s, vec![i]);
            None
        }
        Cmd::ClipUnique(i) => {
            snapshot(s);
            make_unique(s, i).map(|n| format!("Made unique: pattern {n}"))
        }
        Cmd::ClipEditSource(i) => {
            if let Some(ClipSource::Pattern(n)) = clips(s).get(i).map(|c| c.source) {
                s.pattern = n;
                s.raise(crate::app::Win::PianoRoll);
            }
            None
        }
        Cmd::ClipSelectSimilar(i) => {
            if let Some(src) = clips(s).get(i).map(|c| c.source) {
                s.playlist.sel = clips(s).iter().enumerate().filter(|(_, c)| c.source == src).map(|(i, _)| i).collect();
            }
            None
        }
        Cmd::ClipRename(i) => {
            let src = clips(s).get(i).map(|c| c.source);
            let at = grid.zip(clips(s).get(i)).map_or((600.0, 200.0), |(g, c)| {
                let r = PGeom::new(s, g).clip_rect(c);
                (r.x, r.y)
            });
            match src {
                Some(ClipSource::Pattern(n)) => start_edit(s, EditTarget::PatternName(n), at),
                _ => return Some("Rename: only pattern clips can be renamed here".into()),
            }
            None
        }
        Cmd::ClipRandomColor(i) => {
            let Some(src) = clips(s).get(i).map(|c| c.source) else { return None };
            let seed = (clips(s).len() as u32).wrapping_mul(2654435761).wrapping_add(i as u32 * 40503);
            let rgb = hsl_rgb((seed % 360) as f32, 0.45, 0.45);
            match src {
                ClipSource::Pattern(n) => {
                    if let Some(p) = s.project.patterns.iter_mut().find(|p| p.number == n) {
                        p.color = Some(rgb);
                    } else {
                        s.project.patterns.push(os_model::Pattern { number: n, color: Some(rgb), ..Default::default() });
                    }
                }
                ClipSource::Channel(ch) => {
                    if let Some(c) = s.project.channels.iter_mut().find(|c| c.index == ch) {
                        c.color = Some(rgb);
                        c.rack.stored_color = Some(rgb);
                    }
                }
            }
            None
        }
        Cmd::PointDelete(ch, i) => {
            snapshot(s);
            delete_point(s, ch, i);
            None
        }
        Cmd::PointMode(ch, i, m) => {
            snapshot(s);
            if let Some(p) = s.project.automation_mut(ch).and_then(|a| a.points.get_mut(i)) {
                p.mode = m;
                p.flag = if m.has_tension() && !matches!(m, AutoMode::Stairs | AutoMode::SmoothStairs | AutoMode::Pulse | AutoMode::Wave) { 2 } else { 0 };
            }
            s.piano.dirty = true;
            None
        }
        Cmd::TrackRename(t) => {
            let at = grid.map_or((380.0, 160.0), |g| (g.x - 108.0, PGeom::new(s, g).y_of(t)));
            start_edit(s, EditTarget::TrackName(t), at);
            None
        }
        Cmd::TrackMute(t) => {
            snapshot(s);
            toggle_track(s, t);
            None
        }
        Cmd::TrackSolo(t) => {
            snapshot(s);
            toggle_solo(s, t);
            None
        }
        Cmd::TrackGroup(t) if t > 0 => {
            snapshot(s);
            let tr = &mut arr_mut(s).tracks[usize::from(t)];
            tr.grouped = !tr.grouped;
            None
        }
        Cmd::TrackGroup(_) => None,
        Cmd::GroupCollapse(t) => {
            toggle_collapse(s, t);
            None
        }
        Cmd::ArrSelect(n) => {
            show_arrangement(s, n);
            None
        }
        Cmd::ArrAdd | Cmd::ArrClone => {
            let n = s.project.arrangements.iter().map(|a| a.number + 1).max().unwrap_or(0);
            let mut a = if cmd_is_clone(&cmd) { arr_mut(s).clone() } else { os_flp::project::default_arrangement() };
            a.number = n;
            a.name = Some(if cmd_is_clone(&cmd) {
                format!("{} #2", arrangement_name(s))
            } else {
                format!("Arrangement {}", s.project.arrangements.len() + 1)
            });
            s.project.arrangements.push(a);
            show_arrangement(s, n);
            // FL's "Add one..." / "Clone..." ask for the name.
            start_edit(s, EditTarget::ArrangementName, s.playlist.arr_anchor);
            None
        }
        Cmd::ArrRename => {
            start_edit(s, EditTarget::ArrangementName, s.playlist.arr_anchor);
            None
        }
        Cmd::ArrDelete if s.project.arrangements.len() > 1 => {
            let i = s.project.arrangement_index();
            s.project.arrangements.remove(i);
            let n = s.project.arrangements[i.saturating_sub(1)].number;
            show_arrangement(s, n);
            None
        }
        Cmd::ArrDelete => Some("The only arrangement cannot be deleted".into()),
        Cmd::ArrMerge(n) => {
            let Some(other) = s.project.arrangements.iter().find(|a| a.number == n).cloned() else { return None };
            snapshot(s);
            let a = arr_mut(s);
            a.clips.extend(other.clips.iter().copied());
            a.clips.sort_by_key(|c| c.position);
            s.piano.dirty = true;
            Some(format!("Merged the clips of {}", other.name.unwrap_or_else(|| "Arrangement".into())))
        }
        Cmd::TrackUp(t) if t > 0 => {
            snapshot(s);
            swap_tracks(s, t, t - 1);
            s.playlist.track = t - 1;
            None
        }
        Cmd::TrackDown(t) if t + 1 < TRACKS => {
            snapshot(s);
            swap_tracks(s, t, t + 1);
            s.playlist.track = t + 1;
            None
        }
        Cmd::TrackUp(_) | Cmd::TrackDown(_) => None,
        Cmd::TrackInsert(t) => {
            snapshot(s);
            insert_track(s, t);
            None
        }
        Cmd::TrackDelete(t) => {
            snapshot(s);
            delete_track(s, t);
            None
        }
        Cmd::TrackColor(t, c) => {
            snapshot(s);
            if let Some(tr) = arr_mut(s).tracks.get_mut(usize::from(t)) {
                tr.color = c;
                // FL writes the default name when a track is recoloured (flp playlist.md).
                if c.is_some() && tr.name.is_none() {
                    tr.name = Some(format!("Track {}", t + 1));
                }
            }
            s.piano.dirty = true;
            None
        }
        Cmd::TrackMuteClips(t, on) => {
            snapshot(s);
            for c in arr_mut(s).clips.iter_mut().filter(|c| c.track == t) {
                c.muted = on;
            }
            s.piano.dirty = true;
            None
        }
        Cmd::CreateAutomation(tgt, name, v) => {
            create_automation(s, tgt, &name, v);
            s.raise(crate::app::Win::Playlist);
            Some(format!("Created automation clip \"{name}\""))
        }
        Cmd::InitControl(tgt, v) => {
            crate::init::init(s, tgt, v);
            None
        }
        Cmd::DeleteInit(tgt) => {
            crate::init::delete(s, tgt);
            None
        }
        Cmd::ResetEmptyTracks => {
            snapshot(s);
            let used: Vec<u16> = clips(s).iter().map(|c| c.track).collect();
            for (k, tr) in arr_mut(s).tracks.iter_mut().enumerate() {
                if !used.contains(&(k as u16)) {
                    *tr = PlaylistTrack::new(tr.number);
                }
            }
            s.piano.dirty = true;
            None
        }
        // Run above.
        Cmd::Pref(..) => None,
        Cmd::Genre(_) => None,
        Cmd::Template(_) => None,
        Cmd::Messages(_) => None,
        Cmd::DataFolder(_) => None,
    };
    app.invalidate();
    msg
}

/// An RGB colour from hue (degrees), saturation and lightness (0..1).
pub(crate) fn hsl_rgb(h: f32, s: f32, l: f32) -> Rgb {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = (h.rem_euclid(360.0)) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let k = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Rgb::new(k(r), k(g), k(b))
}

/// Ctrl+Alt+C: render the selection (or the time selection) to a WAV file and put it on a new track as an audio
/// clip; the rendered clips are muted so the song sounds the same (OS choice, §5).
/// Tools > Quick quantize start times (§10, OS): every selected clip (every clip when none is selected) starts on the
/// nearest snap step (on the nearest beat when snapping is off); lengths stay. One undo step.
pub fn quantize_starts(s: &mut State) -> String {
    let p = ppq(s);
    let unit = match grid_snap(s).ticks(p, s.playlist_bar_w) {
        0 | 1 => p,
        u => u,
    };
    let idx: Vec<usize> = if s.playlist.sel.is_empty() { (0..clips(s).len()).collect() } else { s.playlist.sel.clone() };
    let moved = idx.iter().filter(|&&i| clips(s).get(i).is_some_and(|c| c.position % unit != 0)).count();
    if moved == 0 {
        return "Quick quantize: every clip already starts on the grid".into();
    }
    snapshot(s);
    let a = arr_mut(s);
    for &i in &idx {
        if let Some(c) = a.clips.get_mut(i) {
            c.position = (c.position + unit / 2) / unit * unit;
        }
    }
    s.piano.dirty = true;
    format!("Quick quantize: moved {moved} clip start{}", if moved == 1 { "" } else { "s" })
}

/// Tools > Export all playlist tracks (§10, OS): every track with unmuted clips on a playing track is rendered alone,
/// from the song's start, into `Rendered/<project> tracks` next to the project file (the temp folder for a new
/// project): "<NN> <track name>.wav", 44.1 kHz float, at least as long as the song so the files line up.
pub fn export_tracks(app: &mut App) -> String {
    let s = &app.state;
    let audible: Vec<Clip> = clips(s).iter().filter(|c| !c.muted && track_on(s, c.track)).copied().collect();
    let Some(end) = audible.iter().map(|c| c.end()).max() else { return "Export tracks: the playlist has no clips to play".into() };
    let mut tracks: Vec<u16> = audible.iter().map(|c| c.track).collect();
    tracks.sort_unstable();
    tracks.dedup();
    let stem = s.path.as_ref().and_then(|p| p.file_stem()).map_or_else(|| "Untitled".to_string(), |n| n.to_string_lossy().into_owned());
    let base = s.path.as_ref().and_then(|p| p.parent()).map(|d| d.join("Rendered")).unwrap_or_else(|| std::env::temp_dir().join("Lemon Studio").join("Rendered"));
    let dir = base.join(format!("{stem} tracks"));
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return format!("Export tracks: {e}");
    }
    let rate = 44100;
    let mut lib = os_engine::SampleLibrary::new(os_engine::SampleLibrary::default_factory_dir());
    let song_frames = os_engine::laws::tick_to_frame(f64::from(end), s.project.ppq, s.project.tick_tempo(s.tempo), rate) as usize;
    let mut written = 0;
    for &t in &tracks {
        let mut p = s.project.clone();
        p.tempo = Some(s.tempo);
        if let Some(arr) = p.arrangement_mut() {
            arr.clips = audible.iter().filter(|c| c.track == t).copied().collect();
            for tr in &mut arr.tracks {
                tr.enabled = true;
            }
        }
        let mut e = os_engine::Engine::new(&p, &mut lib, rate, os_engine::PlayMode::Song);
        let mut audio = e.render_all(512);
        if audio.len() < song_frames * 2 {
            audio.resize(song_frames * 2, 0.0);
        }
        let name: String = track_name(s, t).chars().map(|c| if r#"<>:"/\|?*"#.contains(c) { '_' } else { c }).collect();
        let file = dir.join(format!("{:02} {name}.wav", t + 1));
        let ok = std::fs::File::create(&file).and_then(|f| {
            let mut w = std::io::BufWriter::new(f);
            os_engine::wav::write_f32_stereo(&mut w, rate, &audio)
        });
        if let Err(e) = ok {
            return format!("Export tracks: {}: {e}", file.display());
        }
        written += 1;
    }
    format!("Exported {written} track{} to {}", if written == 1 { "" } else { "s" }, dir.display())
}

pub fn consolidate(app: &mut App) -> String {
    let s = &mut app.state;
    let sel = s.playlist.sel.clone();
    let (a, b) = match (s.playlist.range, sel.is_empty()) {
        (Some((r0, r1)), _) if r1 > r0 => (r0, r1),
        (_, false) => {
            let cs: Vec<&Clip> = sel.iter().filter_map(|&i| clips(s).get(i)).collect();
            (cs.iter().map(|c| c.position).min().unwrap_or(0), cs.iter().map(|c| c.end()).max().unwrap_or(0))
        }
        _ => return "Consolidate: select clips or a time range first".into(),
    };
    if b <= a {
        return "Consolidate: nothing to render".into();
    }
    let use_idx: Vec<usize> = if sel.is_empty() {
        clips(s).iter().enumerate().filter(|(_, c)| c.position < b && c.end() > a).map(|(i, _)| i).collect()
    } else {
        sel.clone()
    };
    // A project with just those clips, shifted to start at 0 (clips crossing the start are cut there).
    let mut p = s.project.clone();
    p.tempo = Some(s.tempo);
    let mut parts = Vec::new();
    for &i in &use_idx {
        let Some(mut c) = clips(s).get(i).copied() else { continue };
        if c.position < a {
            let w = window(s, &c);
            let d = a - c.position;
            c.window = Some((w.0 + to_src(s, c.source, f64::from(d)), w.1));
            c.length -= d.min(c.length);
            c.position = a;
        }
        c.position -= a;
        c.length = c.length.min(b - a - c.position.min(b - a));
        parts.push(c);
    }
    if let Some(arr) = p.arrangement_mut() {
        arr.clips = parts;
        for t in &mut arr.tracks {
            t.enabled = true;
        }
    }
    let rate = 44100;
    let mut lib = os_engine::SampleLibrary::new(os_engine::SampleLibrary::default_factory_dir());
    let mut e = os_engine::Engine::new(&p, &mut lib, rate, os_engine::PlayMode::Song);
    let mut audio = e.render_all(512);
    let want = (os_engine::laws::tick_to_frame(f64::from(b - a), p.ppq, p.tick_tempo(s.tempo), rate) as usize) * 2;
    audio.resize(want.max(2), 0.0);
    let dir = s.path.as_ref().and_then(|p| p.parent()).map(|d| d.join("Rendered")).unwrap_or_else(|| std::env::temp_dir().join("Lemon Studio").join("Rendered"));
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return format!("Consolidate: {e}");
    }
    let n = std::fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0) + 1;
    let file = dir.join(format!("Consolidated {n}.wav"));
    let written = std::fs::File::create(&file).and_then(|f| {
        let mut w = std::io::BufWriter::new(f);
        os_engine::wav::write_f32_stereo(&mut w, rate, &audio)
    });
    if let Err(e) = written {
        return format!("Consolidate: {e}");
    }
    let path = file.to_string_lossy().into_owned();
    let track = free_track(s, track_after_used(s), a, b);
    snapshot(s);
    for &i in &use_idx {
        if let Some(c) = arr_mut(s).clips.get_mut(i) {
            c.muted = true;
        }
    }
    crate::history::unrecord(s); // one undo step for the whole consolidation (add_audio_clip snapshots)
    match add_audio_clip(s, &path, track, a) {
        Some(i) => {
            // The render's own length, exactly.
            if let Some(c) = arr_mut(s).clips.get_mut(i) {
                c.length = b - a;
            }
            format!("Consolidated to {}", file.display())
        }
        None => format!("Consolidate: could not read back {}", file.display()),
    }
}

/// Keys while the playlist is focused. Returns true when handled.
pub fn key(app: &mut App, key: &winit::keyboard::Key, ctrl: bool, alt: bool) -> bool {
    use winit::keyboard::{Key, NamedKey};
    let shift = app.shift;
    // Inline text box first.
    if app.state.playlist.edit.is_some() {
        let e = app.state.playlist.edit.as_mut().expect("checked");
        match key {
            Key::Named(NamedKey::Enter) => commit_edit(&mut app.state),
            Key::Named(NamedKey::Escape) => app.state.playlist.edit = None,
            // A selected text goes as a whole (the value box, spec: ui/type-in-value.md §1, §8).
            Key::Named(NamedKey::Backspace) => {
                if std::mem::take(&mut e.sel) {
                    e.text.clear();
                } else {
                    e.text.pop();
                }
            }
            Key::Named(NamedKey::Space) | Key::Character(_) if !ctrl => {
                if std::mem::take(&mut e.sel) {
                    e.text.clear();
                }
                e.text.push_str(if let Key::Character(c) = key { c } else { " " });
            }
            Key::Character(c) if c.eq_ignore_ascii_case("a") && matches!(e.target, EditTarget::TypeIn(..)) => e.sel = !e.text.is_empty(),
            _ => {}
        }
        app.invalidate();
        return true;
    }
    if app.state.playlist.popup.is_some() && matches!(key, Key::Named(NamedKey::Escape)) {
        app.state.playlist.popup = None;
        app.invalidate();
        return true;
    }
    let cmd = match key {
        Key::Character(c) if ctrl && alt && c.eq_ignore_ascii_case("c") => Cmd::Consolidate,
        Key::Character(c) if ctrl && shift && c.eq_ignore_ascii_case("c") => Cmd::MakeUnique,
        // Performance mode (live clip launching, Ctrl+P in FL's menu) is not built yet.
        Key::Character(c) if ctrl && !alt && c.eq_ignore_ascii_case("p") => Cmd::NotYet("Performance mode"),
        Key::Character(c) if ctrl && c.eq_ignore_ascii_case("a") => Cmd::SelectAll,
        Key::Character(c) if ctrl && c.eq_ignore_ascii_case("d") => Cmd::Deselect,
        Key::Character(c) if ctrl && c.eq_ignore_ascii_case("c") => Cmd::Copy,
        Key::Character(c) if ctrl && c.eq_ignore_ascii_case("x") => Cmd::Cut,
        Key::Character(c) if ctrl && c.eq_ignore_ascii_case("v") => Cmd::Paste,
        Key::Character(c) if ctrl && c.eq_ignore_ascii_case("b") => Cmd::Duplicate,
        Key::Character(c) if ctrl && c.eq_ignore_ascii_case("z") => Cmd::Undo,
        Key::Character(c) if alt && !ctrl && c.eq_ignore_ascii_case("m") => Cmd::Mute(!shift),
        Key::Character(c) if alt && !ctrl && shift && c.eq_ignore_ascii_case("t") => Cmd::AddSignature,
        Key::Character(c) if alt && !ctrl && c.eq_ignore_ascii_case("t") => Cmd::AddMarker,
        Key::Character(c) if shift && !ctrl && !alt && c.eq_ignore_ascii_case("z") => Cmd::ZoomSelection,
        Key::Character(c) if shift && !ctrl && !alt && c.eq_ignore_ascii_case("m") => Cmd::StretchMode,
        Key::Character(c) if shift && !ctrl && !alt && c.eq_ignore_ascii_case("i") => Cmd::InvertSelection,
        Key::Character(c) if shift && !ctrl && !alt && (c == "4" || c == "$") => Cmd::ZoomAll,
        Key::Character(c) if shift && !ctrl && !alt && (c == "5" || c == "%") => Cmd::ZoomSelection,
        Key::Character(c) if shift && !ctrl && !alt && c.eq_ignore_ascii_case("g") => Cmd::GroupClips,
        Key::Character(c) if alt && !ctrl && (c.eq_ignore_ascii_case("g") || c.eq_ignore_ascii_case("u")) => Cmd::UngroupClips,
        Key::Named(NamedKey::End) if !ctrl && !alt => Cmd::ZoomSelection,
        Key::Named(NamedKey::Tab) if shift && !ctrl && !alt => Cmd::CycleSnap,
        // The global snap on / off (spec: ui/global-snap.md §1).
        Key::Named(NamedKey::Backspace) if !ctrl && !alt => {
            app.state.message = Some(crate::snap::toggle(&mut app.state));
            app.invalidate();
            return true;
        }
        Key::Character(c) if !ctrl && !alt && !app.typing_on() && PlTool::from_letter(c).is_some() => {
            Cmd::Tool(PlTool::from_letter(c).expect("checked"))
        }
        Key::Named(NamedKey::Delete) if ctrl => Cmd::DeleteSpace,
        Key::Named(NamedKey::Insert) if ctrl => Cmd::InsertSpace,
        Key::Named(NamedKey::Delete) => Cmd::Delete,
        Key::Named(NamedKey::PageUp) => Cmd::ZoomIn,
        Key::Named(NamedKey::PageDown) => Cmd::ZoomOut,
        Key::Named(NamedKey::ArrowLeft) if shift => Cmd::ShiftTime(-1),
        Key::Named(NamedKey::ArrowRight) if shift => Cmd::ShiftTime(1),
        Key::Named(NamedKey::ArrowUp) if shift => Cmd::ShiftTracks(-1),
        Key::Named(NamedKey::ArrowDown) if shift => Cmd::ShiftTracks(1),
        Key::Named(NamedKey::ArrowUp) if ctrl => {
            let s = &mut app.state;
            s.playlist.track = s.playlist.track.saturating_sub(1);
            app.invalidate();
            return true;
        }
        Key::Named(NamedKey::ArrowDown) if ctrl => {
            let s = &mut app.state;
            s.playlist.track = (s.playlist.track + 1).min(TRACKS - 1);
            app.invalidate();
            return true;
        }
        Key::Named(NamedKey::Enter) if ctrl => {
            // Play from the song start.
            app.state.song_mode = true;
            app.state.playlist.pos = 0;
            app.play_song_from(0);
            return true;
        }
        Key::Named(NamedKey::Escape) => {
            // Also closes menus and the picker (the global Escape).
            run(app, Cmd::Deselect);
            return false;
        }
        _ => return false,
    };
    if let Some(m) = run(app, cmd) {
        app.state.message = Some(m);
    }
    true
}

/// The timebase changed from `from` to `to` ticks per quarter (`Project::set_ppq` rescaled the project): the song
/// position, time selection, clipboard, locked clip length and undo steps follow, so nothing of the old timebase comes
/// back (automation data stays, as in the project).
pub fn rescale(s: &mut State, from: u16, to: u16) {
    use os_model::{rescale_length, rescale_tick};
    let pl = &mut s.playlist;
    pl.pos = rescale_tick(pl.pos, from, to);
    pl.range = pl.range.map(|(a, b)| (rescale_tick(a, from, to), rescale_tick(b, from, to)));
    pl.clipboard.iter_mut().for_each(|c| c.rescale(from, to));
    if let Some((_, len)) = &mut pl.lock_len {
        *len = rescale_length(*len, from, to);
    }
}

/// The plugin window's ▸ menu (spec: ui/plugin-window.md "Docking and the window menu"): FL's entries, only Detached
/// works yet.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn plugin_window_menu(row: usize, detached: bool, x: f32, y: f32) -> Popup {
    let items = vec![
        PItem::not_yet("Piano roll"),
        PItem::not_yet("Replace"),
        PItem::not_yet("Delete..."),
        PItem::head("Presets"),
        PItem::not_yet("Presets"),
        PItem::not_yet("Browse presets"),
        PItem::not_yet("Save preset as..."),
        PItem::not_yet("Add to plugin database (flag as favorite)"),
        PItem::head("Spare state"),
        PItem::not_yet("Store in spare state"),
        PItem::not_yet("Flip with spare state"),
        PItem::head("Parameters"),
        PItem::not_yet("Link all parameters..."),
        PItem::new("Browse parameters", Cmd::Param(crate::params::PCmd::Browse(row))),
        PItem::head("CPU"),
        PItem::not_yet("Allow threaded processing"),
        PItem::not_yet("Smart disable"),
        PItem::sep(),
        PItem::not_yet("Rename, color and icon...").key("F2"),
        PItem::not_yet("Change color..."),
        PItem::not_yet("Random color"),
        PItem::not_yet("Change icon..."),
        PItem::new("Detached", Cmd::PluginDetach(row, !detached)).mark(Mark::Check(detached)),
        PItem::col(),
        PItem::head("Channel"),
        PItem::not_yet("Save channel state as..."),
        PItem::not_yet("Browse channel states"),
        PItem::sep(),
        PItem::new("Route to free mixer track", Cmd::Rack(crate::rack::RCmd::AssignFree(row))).key("Ctrl+L"),
    ];
    Popup { x, y, items, sub: None }
}
