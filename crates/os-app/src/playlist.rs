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

/// Track row pitch and clip height at the default size (spec: ui/main-window.md §6).
pub const ROW_H: f32 = 48.0;
pub const CLIP_H: f32 = 45.0;
/// Title strip of a clip incl. its top border (spec: ui/playlist.md §2).
pub const TITLE_H: f32 = 15.0;
/// Marker lane under the ruler, when the arrangement has time markers (§1).
pub const MARKER_H: f32 = 17.0;
/// Edge grab zone for resizing (OS choice, §3).
pub const EDGE: f32 = 6.0;
/// Number of tracks (flp playlist.md).
pub const TRACKS: u16 = 500;
const UNDO_DEPTH: usize = 200;

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
    NotYet(&'static str),
    Tool(PlTool),
    Snap(Snap),
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
    PointDelete(u16, usize),
    PointMode(u16, usize, AutoMode),
    PointValue(u16, usize),
    TrackRename(u16),
    TrackMute(u16),
    TrackSolo(u16),
    TrackUp(u16),
    TrackDown(u16),
    TrackInsert(u16),
    TrackDelete(u16),
    TrackColor(u16, Option<Rgb>),
    TrackMuteClips(u16, bool),
    /// "Create automation clip" on a control: target, name, current value 0..1.
    CreateAutomation(u32, String, f64),
    ResetEmptyTracks,
    /// Browser options menu (spec: ui/browser.md §4).
    Browser(crate::browser::BCmd),
}

#[derive(Debug, Clone, PartialEq)]
pub struct PItem {
    pub label: Cow<'static, str>,
    pub key: &'static str,
    pub cmd: Cmd,
    pub mark: Mark,
    pub enabled: bool,
    /// Section header (not clickable).
    pub head: bool,
    pub sep: bool,
    pub children: Vec<PItem>,
}

impl PItem {
    pub fn new(label: impl Into<Cow<'static, str>>, cmd: Cmd) -> Self {
        PItem { label: label.into(), key: "", cmd, mark: Mark::None, enabled: true, head: false, sep: false, children: Vec::new() }
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
    pub fn head(label: &'static str) -> Self {
        PItem { head: true, ..PItem::new(label, Cmd::Nothing) }
    }
    pub fn sep() -> Self {
        PItem { sep: true, ..PItem::new("", Cmd::Nothing) }
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
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextEdit {
    pub target: EditTarget,
    pub text: String,
    /// Where the box is drawn (screen).
    pub at: (f32, f32),
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

type Snapshot = (Vec<Clip>, Vec<TimeMarker>, Vec<PlaylistTrack>, Vec<Automation>);

pub struct PlaylistState {
    pub tool: PlTool,
    pub snap: Snap,
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
    /// Soloed track: its own state and the others' before the solo.
    pub solo: Option<(u16, Vec<bool>)>,
    /// Zoom tool: the view before "zoom out" (bar width, scroll x, scroll y).
    pub zoom_back: Option<(f32, f32, f32)>,
    /// Automation clip under the mouse (its points are drawn).
    pub hover_clip: Option<usize>,
    /// Last header click (track, time) for double-click rename.
    pub last_header: Option<(u16, std::time::Instant)>,
    /// Waveform cache by sample path (`None` = could not be loaded).
    pub waves: RefCell<HashMap<String, Option<Rc<Wave>>>>,
    library: RefCell<os_engine::SampleLibrary>,
    undo: Vec<Snapshot>,
}

impl Default for PlaylistState {
    fn default() -> Self {
        PlaylistState {
            tool: PlTool::Draw,
            snap: Snap::Line,
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
            solo: None,
            zoom_back: None,
            hover_clip: None,
            last_header: None,
            waves: RefCell::new(HashMap::new()),
            library: RefCell::new(os_engine::SampleLibrary::new(os_engine::SampleLibrary::default_factory_dir())),
            undo: Vec::new(),
        }
    }
}

impl PlaylistState {
    /// Forget everything tied to the previous project (keeps tool and snap).
    pub fn reset(&mut self) {
        let (tool, snap) = (self.tool, self.snap);
        *self = PlaylistState { tool, snap, ..PlaylistState::default() };
    }

    /// The waveform of a sample path (loaded and summarised once).
    pub fn wave(&self, path: &str) -> Option<Rc<Wave>> {
        if let Some(w) = self.waves.borrow().get(path) {
            return w.clone();
        }
        let w = self.library.borrow_mut().get(path).map(|s| {
            let per = 64usize;
            let peaks = s
                .left
                .chunks(per)
                .zip(s.right.chunks(per))
                .map(|(l, r)| {
                    let (mut lo, mut hi) = (0.0f32, 0.0f32);
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
}

// ---------------------------------------------------------------------------------------------------------
// Geometry

/// Mapping between grid pixels and ticks / tracks for one frame.
#[derive(Debug, Clone, Copy)]
pub struct PGeom {
    pub grid: Rect,
    /// Pixels per tick.
    pub ppt: f32,
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub ppq: u32,
    pub bar_w: f32,
}

impl PGeom {
    pub fn new(s: &State, grid: Rect) -> PGeom {
        let ppq = u32::from(s.project.ppq.max(4));
        let bar_w = s.playlist_bar_w;
        PGeom { grid, ppt: bar_w / (ppq * 4) as f32, scroll_x: s.playlist_scroll.0, scroll_y: s.playlist_scroll.1, ppq, bar_w }
    }

    pub fn x_of(&self, tick: f32) -> f32 {
        self.grid.x + tick * self.ppt - self.scroll_x
    }

    pub fn tick_at(&self, x: f32) -> f32 {
        (x - self.grid.x + self.scroll_x) / self.ppt
    }

    pub fn y_of(&self, track: u16) -> f32 {
        self.grid.y + f32::from(track) * ROW_H - self.scroll_y
    }

    pub fn track_at(&self, y: f32) -> u16 {
        (((y - self.grid.y + self.scroll_y) / ROW_H).floor().max(0.0) as u32).min(u32::from(TRACKS) - 1) as u16
    }

    pub fn clip_rect(&self, c: &Clip) -> Rect {
        let x = self.x_of(c.position as f32);
        let w = (c.length as f32 * self.ppt).max(3.0);
        Rect::new(x, self.y_of(c.track), w, CLIP_H)
    }

    /// The body of a clip below its title strip (automation curves, notes, waveforms).
    pub fn body(&self, c: &Clip) -> Rect {
        let r = self.clip_rect(c);
        Rect::new(r.x, r.y + TITLE_H, r.w, (CLIP_H - TITLE_H - 1.0).max(1.0))
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
        a.tracks.push(PlaylistTrack { number: n, name: None, color: None, enabled: true });
    }
    a
}

pub fn clips(s: &State) -> &[Clip] {
    s.project.arrangement().map(|a| a.clips.as_slice()).unwrap_or(&[])
}

pub fn markers(s: &State) -> &[TimeMarker] {
    s.project.arrangement().map(|a| a.markers.as_slice()).unwrap_or(&[])
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

pub fn source_kind(s: &State, src: ClipSource) -> PickKind {
    match src {
        ClipSource::Pattern(_) => PickKind::Pattern,
        ClipSource::Channel(c) => match channel_of(s, c).map(|ch| ch.kind) {
            Some(ChannelKind::Automation) => PickKind::Automation,
            _ => PickKind::Audio,
        },
    }
}

/// Seconds per tick at the project tempo.
fn secs_per_tick(s: &State) -> f64 {
    60.0 / s.tempo.max(1.0) / f64::from(ppq(s))
}

/// Natural length of a source in ticks: a pattern's whole bars, a sample's duration, an automation's last point.
pub fn source_len(s: &State, src: ClipSource) -> u32 {
    let bar = bar(s);
    match src {
        ClipSource::Pattern(n) => {
            let notes = s.project.pattern(n).map(|p| p.notes.as_slice()).unwrap_or(&[]);
            if notes.is_empty() { bar } else { crate::piano::pattern_length(s, notes) }
        }
        ClipSource::Channel(c) => {
            if let Some(a) = s.project.automation(c) {
                let beats = a.points.last().map_or(0.0, |p| p.beat);
                return ((beats * f64::from(ppq(s))).round() as u32).max(bar / 4);
            }
            let secs = channel_of(s, c).and_then(|ch| ch.sample_path.as_deref()).and_then(|p| s.playlist.wave(p)).map_or(0.0, |w| w.secs());
            if secs > 0.0 { ((secs / secs_per_tick(s)).round() as u32).max(1) } else { bar }
        }
    }
}

/// A clip's window in its source's unit (pattern ticks, audio ms, automation beats), filled in when unset.
pub fn window(s: &State, c: &Clip) -> (f64, f64) {
    c.window.unwrap_or_else(|| (0.0, to_src(s, c.source, f64::from(c.length))))
}

/// Ticks to the source's unit.
pub fn to_src(s: &State, src: ClipSource, ticks: f64) -> f64 {
    match source_kind(s, src) {
        PickKind::Pattern => ticks,
        PickKind::Audio => ticks * secs_per_tick(s) * 1000.0,
        PickKind::Automation => ticks / f64::from(ppq(s)),
    }
}

/// Undo snapshot of everything the playlist edits.
pub fn snapshot(s: &mut State) {
    let a = arr_mut(s);
    let snap = (a.clips.clone(), a.markers.clone(), a.tracks.clone(), s.project.automations.clone());
    let u = &mut s.playlist.undo;
    u.push(snap);
    if u.len() > UNDO_DEPTH {
        u.remove(0);
    }
}

pub fn undo(s: &mut State) -> bool {
    let Some((c, m, t, a)) = s.playlist.undo.pop() else { return false };
    let arr = arr_mut(s);
    arr.clips = c;
    arr.markers = m;
    arr.tracks = t;
    s.project.automations = a;
    s.playlist.sel.clear();
    s.piano.dirty = true;
    true
}

/// Topmost clip under (x, y): the last one in the list (drawn on top).
pub fn clip_at(s: &State, g: &PGeom, x: f32, y: f32) -> Option<usize> {
    clips(s).iter().enumerate().rev().find(|(_, c)| g.clip_rect(c).contains(x, y)).map(|(i, _)| i)
}

/// Snap a tick down to the snap unit (Alt = no snap).
pub fn snap_down(s: &State, g: &PGeom, tick: f32, alt: bool) -> u32 {
    let t = tick.max(0.0) as u32;
    if alt {
        return t;
    }
    let u = s.playlist.snap.ticks(g.ppq, g.bar_w);
    t - t % u
}

/// Snap a tick delta to the nearest snap unit.
pub fn snap_delta(s: &State, g: &PGeom, dt: f32, alt: bool) -> i64 {
    if alt {
        return dt.round() as i64;
    }
    let u = s.playlist.snap.ticks(g.ppq, g.bar_w) as f32;
    ((dt / u).round() * u) as i64
}

/// Place a clip of the current source on `track` at `tick`; returns its index.
pub fn place(s: &mut State, track: u16, tick: u32) -> usize {
    let src = source(s);
    let len = source_len(s, src);
    let a = arr_mut(s);
    a.clips.push(Clip::new(tick, len, track, src));
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
    if let Some(c) = arr_mut(s).clips.get_mut(i) {
        c.length = len.max(1);
        // An unset window already means "from the start for the clip's length".
        if orig.window.is_some() {
            c.window = Some((w0, end));
        }
    }
    s.piano.dirty = true;
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
    s.playlist.sel = (start..n).collect();
    s.piano.dirty = true;
}

/// Ctrl+Insert: insert the time selection's length of empty space at its start (all tracks).
pub fn insert_space(s: &mut State) -> bool {
    let Some((r0, r1)) = s.playlist.range.filter(|r| r.1 > r.0) else { return false };
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
    true
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
    arr.tracks[usize::from(t)] = PlaylistTrack { number: u32::from(t) + 1, name: None, color: None, enabled: true };
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
    arr.tracks[n - 1] = PlaylistTrack { number: n as u32, name: None, color: None, enabled: true };
    s.piano.dirty = true;
}

/// Track LED: mute / unmute (a solo ends first).
pub fn toggle_track(s: &mut State, t: u16) {
    end_solo(s);
    let arr = arr_mut(s);
    if let Some(tr) = arr.tracks.get_mut(usize::from(t)) {
        tr.enabled = !tr.enabled;
    }
    s.piano.dirty = true;
}

/// Ctrl+click / right-click on the LED: solo the track (others muted); again restores the states.
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

fn end_solo(s: &mut State) {
    if let Some((_, states)) = s.playlist.solo.take() {
        let arr = arr_mut(s);
        for (tr, on) in arr.tracks.iter_mut().zip(states) {
            tr.enabled = on;
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

/// Create an automation clip for `target` (§8): a new type-5 channel named `name`, two flat points at `value`,
/// a clip over the time selection (else one bar from the song position) on a free track.
pub fn create_automation(s: &mut State, tgt: u32, name: &str, value: f64) -> u16 {
    snapshot(s);
    let index = s.project.channels.iter().map(|c| c.index + 1).max().unwrap_or(0);
    let (a, b) = match s.playlist.range {
        Some((r0, r1)) if r1 > r0 => (r0, r1),
        _ => {
            let bar = bar(s);
            let start = s.playlist.pos - s.playlist.pos % bar;
            (start, start + bar * 4)
        }
    };
    let beats = f64::from(b - a) / f64::from(ppq(s));
    s.project.channels.push(Channel {
        index,
        kind: ChannelKind::Automation,
        name: Some(name.to_string()),
        plugin: None,
        color: Some(Rgb::new(0x5a, 0x74, 0x4c)),
        enabled: true,
        volume: 1.0,
        pan: 0.0,
        pitch_cents: 0,
        mixer_track: 0,
        sample_path: None,
        layer_children: Vec::new(),
        envelopes: Default::default(),
    });
    s.project.automations.push(os_flp::project::new_automation(index, tgt, value.clamp(0.0, 1.0), beats));
    let track = free_track(s, track_after_used(s), a, b);
    let arr = arr_mut(s);
    arr.clips.push(Clip::new(a, b - a, track, ClipSource::Channel(index)));
    let n = arr.clips.len();
    s.playlist.sel = vec![n - 1];
    s.playlist.source = Some(ClipSource::Channel(index));
    s.playlist.picker = PickKind::Automation;
    s.piano.dirty = true;
    index
}

/// Add an audio clip channel for a sample file and place it at `tick` on `track` (§9).
pub fn add_audio_clip(s: &mut State, path: &str, track: u16, tick: u32) -> Option<usize> {
    s.playlist.wave(path)?;
    snapshot(s);
    let index = s.project.channels.iter().map(|c| c.index + 1).max().unwrap_or(0);
    let name = std::path::Path::new(path).file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Audio".into());
    let mixer_track = (s.project.channels.iter().map(|c| c.mixer_track).max().unwrap_or(0) + 1).min(125);
    s.project.channels.push(Channel {
        index,
        kind: ChannelKind::AudioClip,
        name: Some(name),
        plugin: None,
        color: None,
        enabled: true,
        volume: 0.78125,
        pan: 0.0,
        pitch_cents: 0,
        mixer_track,
        sample_path: Some(path.to_string()),
        layer_children: Vec::new(),
        envelopes: Default::default(),
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
    body.bottom() - 2.0 - v as f32 * (body.h - 4.0)
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

/// The text shown for an automation value: the target's own unit where known.
pub fn value_text(s: &State, ch: u16, v: f64) -> String {
    let tgt = s.project.automation(ch).and_then(|a| a.target).map(target::decode);
    match tgt {
        Some(target::Target::Tempo) => format!("{:.3} BPM", 60.0 + 120.0 * v),
        Some(target::Target::ChannelPan(_) | target::Target::MixerPan(_)) => crate::knobs::pan_text(((v - 0.5) * 2.0) as f32),
        Some(target::Target::ChannelPitch(_)) => format!("{:+.0} cents", (v - 0.5) * 400.0),
        Some(target::Target::MixerVolume(_)) => crate::views::mixer::fader_db_text(v),
        _ => format!("{:.0}%", v * 100.0),
    }
}

/// Parse a typed value (percent, BPM or 0..1) back to 0..1.
pub fn parse_value(s: &State, ch: u16, text: &str) -> Option<f64> {
    let t = text.trim().trim_end_matches('%').trim_end_matches("BPM").trim();
    let v: f64 = t.parse().ok()?;
    let tgt = s.project.automation(ch).and_then(|a| a.target).map(target::decode);
    Some(match tgt {
        Some(target::Target::Tempo) => (v - 60.0) / 120.0,
        _ if v > 1.0 || text.contains('%') => v / 100.0,
        _ => v,
    })
    .map(|v| v.clamp(0.0, 1.0))
}

// ---------------------------------------------------------------------------------------------------------
// Menus (§10)

pub fn options_menu(s: &State, x: f32, y: f32) -> Popup {
    let snap_items = Snap::MENU.iter().map(|&sn| PItem::new(sn.label(), Cmd::Snap(sn)).mark(Mark::Radio(s.playlist.snap == sn))).collect();
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
                PItem::not_yet("Export all playlist tracks"),
                PItem::not_yet("Quick quantize start times"),
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
            "Zoom",
            vec![
                PItem::new("Zoom in", Cmd::ZoomIn).key("PgUp"),
                PItem::new("Zoom out", Cmd::ZoomOut).key("PgDn"),
                PItem::new("Zoom out far", Cmd::ZoomAll).key("Shift+4"),
                PItem::new("Zoom on selection", Cmd::ZoomSelection).key("Shift+Z"),
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

pub fn snap_menu(s: &State, x: f32, y: f32) -> Popup {
    let items = Snap::MENU.iter().map(|&sn| PItem::new(sn.label(), Cmd::Snap(sn)).mark(Mark::Radio(s.playlist.snap == sn))).collect();
    Popup { x, y, items, sub: None }
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

pub fn point_menu(s: &State, ch: u16, i: usize, x: f32, y: f32) -> Popup {
    let a = s.project.automation(ch);
    let n = a.map_or(0, |a| a.points.len());
    let mode = a.and_then(|a| a.points.get(i)).map(|p| p.mode);
    let mut items = vec![PItem::head("Point"), PItem::new(format!("Point {}/{}", i + 1, n), Cmd::Nothing).off(), PItem::new("Delete", Cmd::PointDelete(ch, i))];
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

/// A knob's right-click menu with "Create automation clip" (§8).
pub fn knob_menu(tgt: u32, name: String, value: f64, x: f32, y: f32) -> Popup {
    let items = vec![
        PItem::head("Automation"),
        PItem::new("Create automation clip", Cmd::CreateAutomation(tgt, name, value)),
        PItem::not_yet("Edit events"),
        PItem::not_yet("Link to controller..."),
    ];
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

    // Automation points: drag, add, menu (Draw / Paint / Select tools) (§8).
    if let Some(ci) = hit {
        let c = clips(s)[ci];
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
                if let Some(k) = s.playlist.sel.iter().position(|&j| j == i) {
                    s.playlist.sel.remove(k);
                } else {
                    s.playlist.sel.push(i);
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
            _ => {
                if hit.is_some() {
                    snapshot(s);
                    app.drag = Some(Drag::Pl(PlDrag::Erase));
                    erase_at(app, grid);
                } else {
                    app.drag = Some(Drag::Pl(PlDrag::RightPending { x0: x, y0: y }));
                }
                return;
            }
        }
    }

    match tool {
        PlTool::Delete => {
            snapshot(s);
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
            None => {
                snapshot(s);
                s.playlist.sel.clear();
                let start = snap_down(s, &g, tick, alt);
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
                let mut group = if s.playlist.sel.contains(&i) { s.playlist.sel.clone() } else { vec![i] };
                if !s.playlist.sel.contains(&i) {
                    s.playlist.sel = if tool == PlTool::Select { vec![i] } else { Vec::new() };
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
                    s.piano.dirty = true;
                }
                let orig: Vec<(usize, Clip)> = group.iter().filter_map(|&j| clips(s).get(j).map(|c| (j, *c))).collect();
                app.drag = Some(Drag::Pl(match e {
                    -1 => PlDrag::ResizeL { x0: x, orig },
                    1 => PlDrag::ResizeR { x0: x, orig },
                    _ => PlDrag::Move { x0: x, y0: y, orig, fresh: app.shift },
                }));
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
    let u = s.playlist.snap.ticks(g.ppq, g.bar_w) as f32;
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
            let min = if alt { 1 } else { s.playlist.snap.ticks(g.ppq, g.bar_w) } as i64;
            for &(i, o) in &orig {
                let len = (i64::from(o.length) + dt).max(min) as u32;
                resize_right(s, i, &o, len);
            }
        }
        PlDrag::ResizeL { x0, orig } => {
            let dt = snap_delta(s, &g, (x - x0) / g.ppt, alt);
            for &(i, o) in &orig {
                resize_left(s, i, &o, dt);
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
                let u = f64::from(s.playlist.snap.ticks(g.ppq, g.bar_w)) / f64::from(g.ppq);
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
    }
    app.invalidate();
}

/// Button released.
pub fn drag_end(app: &mut App, d: PlDrag, grid: Rect) {
    let s = &mut app.state;
    let g = PGeom::new(s, grid);
    let overlay = s.playlist.overlay.take();
    match d {
        PlDrag::Slice { .. } => {
            if let Some(Overlay::Slice { tick, t0, t1 }) = overlay {
                snapshot(s);
                if slice(s, tick, t0, t1) == 0 {
                    s.playlist.undo.pop();
                }
            }
        }
        PlDrag::ZoomBox { x0, y0 } => zoom_to(app, grid, x0, y0),
        PlDrag::Playback => app.transport.stop(),
        PlDrag::SongPos => {
            if app.transport.playing() && app.transport.song() {
                let p = app.state.playlist.pos;
                app.play_song_from(p);
            }
        }
        PlDrag::Move { orig, fresh: false, .. } | PlDrag::ResizeL { orig, .. } | PlDrag::ResizeR { orig, .. } => {
            // A click that changed nothing leaves no undo step.
            let same = orig.iter().all(|(i, o)| clips(s).get(*i) == Some(o));
            if same && !app.shift {
                s.playlist.undo.pop();
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
pub fn zoom_on(s: &mut State, grid_w: f32, idx: &[usize]) {
    let cs: Vec<Clip> = idx.iter().filter_map(|&i| clips(s).get(i).copied()).collect();
    let (Some(a), Some(b)) = (cs.iter().map(|c| c.position).min(), cs.iter().map(|c| c.end()).max()) else { return };
    let bar = bar(s) as f32;
    let span = ((b - a) as f32 / bar).max(1.0);
    s.playlist_bar_w = (grid_w / span).clamp(4.0, 600.0);
    s.playlist_scroll.0 = a as f32 / bar * s.playlist_bar_w;
    let top = cs.iter().map(|c| c.track).min().unwrap_or(0);
    s.playlist_scroll.1 = f32::from(top) * ROW_H;
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
        EditTarget::MarkerName(i) => markers(s).get(i).map(|m| m.name.clone()).unwrap_or_default(),
        EditTarget::NewMarker(_) => String::new(),
        EditTarget::PointValue(ch, i) => {
            let v = s.project.automation(ch).and_then(|a| a.points.get(i)).map_or(0.0, |p| p.value);
            match s.project.automation(ch).and_then(|a| a.target).map(target::decode) {
                Some(target::Target::Tempo) => format!("{:.3}", 60.0 + 120.0 * v),
                _ => format!("{:.1}%", v * 100.0),
            }
        }
        EditTarget::PatternName(n) => source_name(s, ClipSource::Pattern(n)),
    };
    s.playlist.edit = Some(TextEdit { target, text, at });
}

/// Enter in the text box.
pub fn commit_edit(s: &mut State) {
    let Some(e) = s.playlist.edit.take() else { return };
    snapshot(s);
    let text = e.text.trim().to_string();
    match e.target {
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
        Cmd::Consolidate => {
            let m = consolidate(app);
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
        _ => {}
    }
    let s = &mut app.state;
    let grid_w = grid.map_or(1400.0, |g| g.w);
    let msg = match cmd {
        Cmd::Nothing | Cmd::Browser(_) => None,
        Cmd::NotYet(name) => Some(format!("{name}: not available yet")),
        Cmd::Tool(t) => {
            s.playlist.tool = t;
            None
        }
        Cmd::Snap(sn) => {
            s.playlist.snap = sn;
            Some(format!("Snap: {}", sn.label()))
        }
        Cmd::Undo => (!undo(s)).then(|| "Nothing to undo".into()),
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
            let u = grid.map_or(ppq(s), |g| s.playlist.snap.ticks(ppq(s), PGeom::new(s, g).bar_w)) as i64;
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
            (!insert_space(s)).then(|| "Insert space: make a time selection first (right-drag the ruler)".into())
        }
        Cmd::DeleteSpace => {
            snapshot(s);
            (!delete_space(s)).then(|| "Delete space: make a time selection first (right-drag the ruler)".into())
        }
        Cmd::Consolidate | Cmd::PointValue(..) | Cmd::ClipPreview(_) => None,
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
            zoom_on(s, grid_w, &idx);
            None
        }
        Cmd::ZoomSelection => {
            let idx = targets(s);
            zoom_on(s, grid_w, &idx);
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
        Cmd::ResetEmptyTracks => {
            snapshot(s);
            let used: Vec<u16> = clips(s).iter().map(|c| c.track).collect();
            for (k, tr) in arr_mut(s).tracks.iter_mut().enumerate() {
                if !used.contains(&(k as u16)) {
                    *tr = PlaylistTrack { number: tr.number, name: None, color: None, enabled: true };
                }
            }
            s.piano.dirty = true;
            None
        }
    };
    app.invalidate();
    msg
}

/// An RGB colour from hue (degrees), saturation and lightness (0..1).
fn hsl_rgb(h: f32, s: f32, l: f32) -> Rgb {
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
    let want = (os_engine::laws::tick_to_frame(f64::from(b - a), p.ppq, s.tempo, rate) as usize) * 2;
    audio.resize(want.max(2), 0.0);
    let dir = s.path.as_ref().and_then(|p| p.parent()).map(|d| d.join("Rendered")).unwrap_or_else(|| std::env::temp_dir().join("OpenStudio").join("Rendered"));
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
    s.playlist.undo.pop(); // one undo step for the whole consolidation (add_audio_clip snapshots)
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
            Key::Named(NamedKey::Backspace) => {
                e.text.pop();
            }
            Key::Named(NamedKey::Space) => e.text.push(' '),
            Key::Character(c) if !ctrl => e.text.push_str(c),
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
        Key::Character(c) if shift && !ctrl && !alt && c.eq_ignore_ascii_case("m") => Cmd::NotYet("Audio clip stretch modes (Shift+M)"),
        Key::Character(c) if shift && !ctrl && !alt && c.eq_ignore_ascii_case("i") => Cmd::InvertSelection,
        Key::Character(c) if shift && !ctrl && !alt && (c == "4" || c == "$") => Cmd::ZoomAll,
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

/// A playlist edit changed nothing: drop its undo step (used by tests).
#[cfg(test)]
pub fn undo_len(s: &State) -> usize {
    s.playlist.undo.len()
}
