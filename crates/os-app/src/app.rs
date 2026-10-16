// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Application state, actions and the per-frame paint/hit-test pass.
//!
//! Painting is immediate: every frame repaints from `State` and records hit regions. Hover uses
//! the topmost hit from the previous frame, so occluded controls never highlight.

use std::borrow::Cow;
use std::path::PathBuf;

use os_engine::PlayMode;
use os_model::{Channel, ChannelKind, MixerTrack, Note, Pattern, Project};
use os_theme::{Design, Palette, Theme};
use os_ui::{Canvas, Rect};
use winit::keyboard::KeyCode;

use crate::piano::{self, Geom, Overlay, PianoState, Tool};
use crate::transport::{State as TransportState, Transport};
use crate::views;

/// Child windows. Playlist, Mixer and Piano roll float by default and can be moved, resized,
/// minimized (title bar only) and maximized (filling the workspace); the Channel rack and the
/// channel plugin window float. All share one stacking order (`State::z`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Win {
    Playlist,
    Mixer,
    ChannelRack,
    PianoRoll,
    Browser,
    Plugin,
    /// The Plugin performance monitor (spec: ui/plugin-performance.md).
    Perf,
    /// The Tempo tapper (spec: ui/tempo-tapper.md).
    Tapper,
    /// A docked hosted-plugin window (a native child window; rack row), stacked with the drawn windows.
    Hosted(usize),
}

impl Win {
    /// Index into `State::wins` for the windows with a stored rect, move and resize.
    pub fn slot(self) -> Option<usize> {
        match self {
            Win::Playlist => Some(0),
            Win::Mixer => Some(1),
            Win::PianoRoll => Some(2),
            _ => None,
        }
    }

    /// Smallest outer size (the layouts stay intact down to this).
    pub fn min_size(self) -> (f32, f32) {
        match self {
            Win::Playlist => (520.0, 220.0),
            Win::Mixer => (560.0, 400.0),
            Win::PianoRoll => (540.0, 320.0),
            _ => (0.0, 0.0),
        }
    }
}

/// The three VIEW > Layout close commands (spec: ui/close-windows.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseAll {
    /// Every window (F12).
    All,
    /// Every window but the focused one (Ctrl+F12).
    Unfocused,
    /// The channel settings window and the hosted plugin windows (Alt+F12).
    Plugins,
}

impl CloseAll {
    pub fn from_label(label: &str) -> Option<CloseAll> {
        Some(match label {
            "Close all windows" => CloseAll::All,
            "Close all unfocused windows" => CloseAll::Unfocused,
            "Close all plugin windows" => CloseAll::Plugins,
            _ => return None,
        })
    }
}

/// Resize edge bits for `Action::WinResize`.
pub const EDGE_L: u8 = 1;
pub const EDGE_R: u8 = 2;
pub const EDGE_T: u8 = 4;
pub const EDGE_B: u8 = 8;

/// Where a Playlist / Mixer / Piano roll window sits when floating, and its title-bar states.
#[derive(Clone, Copy)]
pub struct WinState {
    pub rect: Rect,
    /// Fills the workspace.
    pub maxed: bool,
    /// Collapsed to its title bar.
    pub mini: bool,
}

/// Title bar height of the three docked-style windows.
pub const TITLE_H: f32 = 23.0;

/// Toolbar utility buttons whose left and right clicks differ (spec: ui/toolbar-buttons.md §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Util {
    Undo,
    Help,
    Save,
}

/// Mouse cursor shapes the shell should show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    Default,
    EwResize,
    NsResize,
    NwseResize,
    NeswResize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Settings window (spec: ui/settings.md).
    Settings(views::settings::SAct),
    /// A control that is drawn but not functional yet (`Ui::off`): hovering and clicking only say so.
    Placeholder(&'static str),
    /// Typing keyboard to piano switch (toolbar, OPTIONS menu, Ctrl+T).
    Typing,
    /// Metronome switch (toolbar, OPTIONS menu, Ctrl+M); right-click on the button picks the sound.
    Metronome,
    /// Auto scrolling switch (toolbar, OPTIONS menu); right-click on the button for continuous scrolling.
    AutoScroll,
    /// Enable groups switch (toolbar, OPTIONS menu, Shift+Alt+G).
    EnableGroups,
    /// Step edit switch (toolbar, OPTIONS menu, Ctrl+E; spec: ui/step-edit.md).
    StepEdit,
    /// The toolbar's song position slider (spec: ui/song-position-slider.md).
    SongPos,
    /// The time display (song position read-out): a click toggles its mode, right-click opens its menu.
    Clock,
    /// The toolbar's snap combo: the Snap menu; right-click: Stock note lengths; wheel: the next / previous setting
    /// (spec: ui/global-snap.md §1).
    GlobalSnap,
    /// The toolbar's snap magnet: snapping on / off (spec: ui/global-snap.md §1).
    GlobalSnapToggle,
    /// The toolbar's scope / spectrum panel: click switches the display.
    Monitor,
    /// The toolbar's CPU panel: double-click opens the Plugin performance monitor, right-click its menu.
    CpuPanel,
    /// The Plugin performance monitor's controls (spec: ui/plugin-performance.md).
    Perf(crate::perf::PerfAct),
    /// The Tempo tapper's pad and Apply tempo switch (spec: ui/tempo-tapper.md).
    Tapper(crate::tapper::TapAct),
    /// Monitor debug panel: a slider (index into `monitor::PARAMS`), Reset, Save, Close.
    MonParam(usize),
    MonReset,
    MonSave,
    MonClose,
    /// Cable physics debug panel (views/cabledebug.rs).
    CableDebug(crate::cable::DebugAct),
    Minimize,
    Maximize,
    Close,
    DragWindow,
    Play,
    Stop,
    PatSong(bool),
    Tempo,
    Menu(usize),
    MenuItem(usize, usize),
    /// A ▸ entry of a menu bar dropdown (menu, flat item index): opens its submenu (`views::menus::submenu`).
    MenuSub(usize, usize),
    ToggleWin(Win),
    PatternPrev,
    PatternNext,
    PatternAdd,
    Fader(u16),
    /// A mixer control (spec: ui/mixer.md).
    Mix(crate::mixer::MHit),
    ChannelSelect(usize),
    ChannelEnable(usize),
    Step(usize, u32),
    /// Channel rack: activity / select slot, mixer track box (drag), add channel, resize edges
    /// (`views::rack::EDGE_*` bits).
    RackSelect(usize),
    RackMixer(usize),
    RackAdd,
    /// A rotary control (drag vertically, wheel steps).
    Knob(crate::knobs::KnobId),
    /// Toolbar plugin button: the plugin menu (spec: ui/plugin-menu.md §1).
    PickerOpen,
    /// A toolbar utility button (spec: ui/toolbar-buttons.md §2).
    Util(Util),
    /// Show a window maximized and in front (the playlist / piano roll buttons' right click, spec:
    /// ui/toolbar-buttons.md §2).
    ShowMax(Win),
    RackResize(u8),
    /// Step / piano-roll view toggle, and a row's mini piano roll (spec: ui/channel-rack.md §6).
    RackRollView,
    RackRoll(usize),
    /// Floating window title bar (drag to move) and close button.
    WinDrag(Win),
    CloseWin(Win),
    /// Title-bar buttons of Playlist / Mixer / Piano roll, and their resize edges (`EDGE_*` bits).
    MinWin(Win),
    MaxWin(Win),
    WinResize(Win, u8),
    /// Browser tree row (index into `views::browser::rows`); press selects / toggles / starts a drag.
    BrowserRow(usize),
    /// Browser tab 0..5 (spec: ui/browser.md §1).
    BrowserTab(usize),
    /// Browser list background (wheel scrolls).
    BrowserList,
    /// Title-bar up arrow: close every node of the tab.
    BrowserCollapse,
    /// Title-bar curved arrow: Reread structure (spec: ui/browser.md §4, "Title-bar icons").
    BrowserReread,
    /// Title-bar ▸: the browser options menu (spec: ui/browser.md §4).
    BrowserMenu,
    /// The search field (focus it, §5).
    BrowserSearch,
    PlaylistArea,
    MixerArea,
    RackArea,
    Swallow,
    /// The value box of "Type in value..." (spec: ui/type-in-value.md).
    TypeIn(crate::typein::TiAct),
    /// Piano roll: empty grid, a note (index into the pattern's notes), keyboard, velocity panel.
    PianoGrid,
    PianoNote(usize),
    PianoKeys,
    PianoVelocity,
    /// The band between the grid and the event panel (spec: ui/event-panel-splitter.md).
    PianoSplitter,
    /// Event panel "Control" button and its menu entries (index into `Lane::ALL`).
    PianoLaneMenu,
    /// Toolbar "Piano roll tools" button: the Tools menu (spec: ui/piano-roll.md §11).
    PianoToolsMenu,
    /// Toolbar magnet: the Snap menu; right-click: Stock note lengths (spec: ui/piano-snap.md §1).
    PianoSnapMenu,
    /// The channel name in the title: the channel menu; right-click: channel settings; wheel: channels with notes
    /// (spec: ui/piano-target-channel.md).
    PianoTarget,
    PianoLanePick(usize),
    /// Arrow button of the pitch lane's scroll bar: +1 up, -1 down.
    PitchStep(i32),
    PianoUndo,
    /// Piano-roll toolbar: make a tool active (spec: ui/piano-roll.md §9).
    PianoTool(Tool),
    /// Piano-roll time ruler: left-drag moves the play-start marker, right-drag selects a time
    /// range (spec: ui/piano-roll.md §10).
    PianoRuler,
    /// A scroll bar (left-drag scrolls; right-drag scrolls horizontally and zooms vertically).
    Scrollbar(Scroll),
    /// A scroll bar's arrow button: steps the axis by an eighth of its view (-1 back, 1 forward).
    ScrollStep(Scroll, i32),
    /// Playlist (spec: ui/playlist.md): title-bar tools, snap magnet, ▸ options menu, time ruler, marker lane,
    /// track header name area and mute LED, picker tabs and items, popup menu items (top level, submenu).
    PlTool(crate::playlist::PlTool),
    PlSnap,
    PlOptions,
    PlRuler,
    PlMarkers,
    PlHeader(u16),
    PlLed(u16),
    /// A group track's collapse arrow (spec: ui/playlist.md §6a).
    PlGroupArrow(u16),
    /// The arrangement name in the playlist title: the arrangement menu (spec: ui/playlist.md §12).
    PlArrangement,
    /// Stretch mode switch in the playlist title (Shift+M, spec: ui/playlist.md §9b).
    PlStretch,
    PlPickTab(crate::playlist::PickKind),
    PlPick(os_model::ClipSource),
    /// Picker list background / footer "+".
    PlPickList,
    PlPickAdd,
    PlPopup(usize),
    PlPopupSub(usize),
    /// Channel rack title bar (spec: ui/channel-rack.md §7): options ▸, display filter combo, name-width dots,
    /// speaker (play / pause pattern), pattern length box, loop mode, graph editor; per-row loop boxes (§15),
    /// graph editor area and tabs (§14), Advanced fill dialog (§22).
    RackOptions,
    RackFilter,
    RackDots,
    RackSpeaker,
    RackLen,
    RackLoop,
    RackGraph,
    RackLoopBox(usize),
    RackGraphArea,
    RackGraphTab(usize),
    RackFill(crate::rack::FillHit),
    /// Pattern name box, suggestions, swatches, delete confirmation (spec: ui/pattern-menu.md).
    Pat(crate::patterns::Hit),
    /// A message box or the file dialog (spec: ui/dialogs.md).
    Dlg(crate::dialog::DHit),
    /// The Song info box: a click closes it (spec: flp/events/project-info.md "The Song info box").
    SongInfo,
}

/// Scrollable view axes. Scroll values are in content pixels (see `App::scroll_get`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scroll {
    PianoX,
    PianoY,
    /// The channel pitch lane's window (content = ±4800 cents).
    PitchY,
    PlaylistX,
    PlaylistY,
    MixerX,
    RackX,
    RackY,
    /// Playlist picker list.
    PlPick,
}

/// A scroll bar painted this frame.
#[derive(Debug, Clone, Copy)]
pub struct Bar {
    pub target: Scroll,
    pub track: Rect,
    pub vertical: bool,
    /// Largest scroll value (content pixels).
    pub max: f32,
    /// Visible extent along the axis (content pixels).
    pub view: f32,
    /// Thumb length in pixels.
    pub thumb: f32,
}

impl Bar {
    fn len(&self) -> f32 {
        if self.vertical { self.track.h } else { self.track.w }
    }

    /// Content pixels per pixel of thumb travel.
    fn ratio(&self) -> f32 {
        let travel = self.len() - self.thumb;
        if travel > 0.0 { self.max / travel } else { 0.0 }
    }
}

/// What needs repainting before the next present.
#[derive(Clone, PartialEq)]
enum Damage {
    None,
    /// Separate regions (a playhead, the clock and a meter are far apart: one bounding box would cover most of the
    /// frame).
    Rects(Vec<Rect>),
    Full,
}

/// Right-drag sideways on a vertical scroll bar: the rows grow by this factor per [`BAR_ROW_PX`] moved right
/// (the piano roll's Ctrl+middle-drag rate, spec: ui/scroll-bars.md).
const BAR_ROW_STEP: f32 = 1.15;
const BAR_ROW_PX: f32 = 12.0;

/// The window a scroll target scrolls.
pub(crate) fn scroll_win(t: Scroll) -> Win {
    match t {
        Scroll::PianoX | Scroll::PianoY | Scroll::PitchY => Win::PianoRoll,
        Scroll::PlaylistX | Scroll::PlaylistY | Scroll::PlPick => Win::Playlist,
        Scroll::MixerX => Win::Mixer,
        Scroll::RackX | Scroll::RackY => Win::ChannelRack,
    }
}

/// The smallest scroll step whose move is whole device pixels on the playlist's drawing grid (1/256 px) at `scale`:
/// 1 px at 100 %, 2 at 150 %, 4 at 125 % and 175 %. None at a scale off that grid (no move is exact there).
fn blit_step(scale: f32) -> Option<f32> {
    (0..=16).map(|e| 2f32.powi(e - 8)).find(|q| (f64::from(*q) * f64::from(scale)).fract() == 0.0)
}

/// `r` moved by (dx, dy).
fn moved_by(r: Rect, dx: f64, dy: f64) -> Rect {
    Rect::new(r.x + dx as f32, r.y + dy as f32, r.w, r.h)
}

/// The parts of `a` outside `b`: up to four rects (above, below, left and right of their overlap).
fn rect_minus(a: Rect, b: Rect) -> Vec<Rect> {
    let o = a.intersect(&b);
    if o.is_empty() {
        return if a.is_empty() { Vec::new() } else { vec![a] };
    }
    let ltrb = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::new(x0, y0, x1 - x0, y1 - y0);
    [
        ltrb(a.x, a.y, a.right(), o.y),
        ltrb(a.x, o.bottom(), a.right(), a.bottom()),
        ltrb(a.x, o.y, o.x, o.bottom()),
        ltrb(o.right(), o.y, a.right(), o.bottom()),
    ]
    .into_iter()
    .filter(|r| !r.is_empty())
    .collect()
}

/// Something drawn over the playlist (`App::playlist_covers`): what it may paint or hit, the window it is (None for
/// the toolbar) and the part of it its frame fills with opaque colour first, while it floats (`App::win_solid`).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Cover {
    bounds: Rect,
    win: Option<Win>,
    solid: Option<Rect>,
}

/// A canvas area a paint moved instead of repainting it (a playlist scroll): by `dx`, `dy` device pixels right and
/// down, except the pixels wholly inside `keep` (windows over it that did not change, `os_ui::keep_area`).
#[derive(Debug, Clone, PartialEq)]
pub struct Moved {
    pub area: Rect,
    pub dx: i32,
    pub dy: i32,
    pub keep: Vec<Rect>,
}

/// Past this many separate damage rects, the two whose merge adds the least area are merged.
const MAX_DAMAGE_RECTS: usize = 32;

/// Add `r` to a damage list, merging it with rects it overlaps or nearly touches when the merge adds little area.
fn add_damage(v: &mut Vec<Rect>, r: Rect) {
    if r.is_empty() {
        return;
    }
    let area = |r: &Rect| r.w * r.h;
    let mut r = r;
    loop {
        let mut merged = false;
        v.retain(|o| {
            let u = o.union(&r);
            if area(&u) <= area(o) + area(&r) + 2048.0 {
                r = u;
                merged = true;
                false
            } else {
                true
            }
        });
        if !merged {
            break;
        }
    }
    v.push(r);
    while v.len() > MAX_DAMAGE_RECTS {
        let mut best = (f32::MAX, 0, 1);
        for i in 0..v.len() {
            for j in i + 1..v.len() {
                let cost = area(&v[i].union(&v[j])) - area(&v[i]) - area(&v[j]);
                if cost < best.0 {
                    best = (cost, i, j);
                }
            }
        }
        let b = v.swap_remove(best.2);
        v[best.1] = v[best.1].union(&b);
    }
}

/// Regions whose look follows playback; views register them while painting (`Ui::live`) and `App::tick` repaints
/// just them while playing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Live {
    /// The song position display.
    Clock,
    /// Lights for the notes sounding now (rack select slots, piano-roll keys).
    Sounding,
    /// Mixer peak meters.
    Meter,
}

/// The playback state the frame shows. Taken in `App::tick`, where the damage for playheads is worked out, so a paint
/// draws them exactly where the damage expects them.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Shown {
    pub playing: bool,
    pub paused: bool,
    pub tick: f64,
    pub song: bool,
    pub sounding: Vec<(u16, u16)>,
}

/// The hint panel (spec §2): its text changes whenever the hovered control does.
pub(crate) const HINT_RECT: Rect = Rect::new(73.0, 5.0, 289.0, 31.0);
/// The toolbar's output monitor (scope / analyzer and peak bars).
const MONITOR_RECT: Rect = Rect::new(880.0, 4.0, 168.0, 33.0);
/// The toolbar's CPU panel.
pub(crate) const CPU_RECT: Rect = Rect::new(1053.0, 4.0, 149.0, 32.0);
/// What a tempo change repaints in the toolbar (the tempo box and the transport around it).
const TEMPO_RECT: Rect = Rect::new(380.0, 0.0, 200.0, 83.0);

/// Actions whose hover state has no visual feedback (large background regions).
fn hover_is_visible(a: &Action) -> bool {
    !matches!(
        a,
        Action::DragWindow
            | Action::PlaylistArea
            | Action::MixerArea
            | Action::Mix(crate::mixer::MHit::Strip(_) | crate::mixer::MHit::Scale)
            | Action::RackArea
            | Action::Swallow
            | Action::PianoGrid
            | Action::PianoNote(_)
            | Action::PianoKeys
            | Action::PianoVelocity
            | Action::PianoRuler
            | Action::PlRuler
            | Action::PlMarkers
            | Action::PlHeader(_)
            | Action::Scrollbar(_)
            | Action::RackResize(_)
            | Action::RackGraphArea
            | Action::WinDrag(_)
            | Action::WinResize(..)
            | Action::Tapper(crate::tapper::TapAct::Pad)
    )
}

/// Pixels from a note's right end that grab the end for resizing (spec: ui/piano-roll.md §8).
fn resize_zone(note_w: f32) -> f32 {
    (note_w / 3.0).clamp(2.0, 8.0)
}

pub struct Hit {
    pub rect: Rect,
    pub action: Action,
    pub hint: Cow<'static, str>,
    /// The child window the control belongs to (clicking raises it).
    pub win: Option<Win>,
}

/// A part of the frame that paints independently: skipped by a repaint whose damage misses where it drew.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seg {
    Browser,
    Win(Win),
    Toolbar,
    /// The playlist's clip source list (inside `Win(Playlist)`: a scroll of the grid leaves it alone).
    PlPicker,
}

/// What each part registered and where it drew in the last paint, so a partial repaint can keep the hits of the parts
/// it skips (hits are rebuilt every paint; topmost last).
#[derive(Default)]
pub struct Keep {
    /// Last paint's hits / bars, from which skipped parts take theirs back.
    prev: Vec<Hit>,
    prev_bars: Vec<Bar>,
    prev_live: Vec<(Live, Rect)>,
    parts: Vec<Part>,
    next: Vec<Part>,
    /// This paint covers the whole frame (drawn bounds are complete, nothing is skipped).
    full: bool,
}

struct Part {
    seg: Seg,
    hits: std::ops::Range<usize>,
    bars: std::ops::Range<usize>,
    live: std::ops::Range<usize>,
    /// Everything the part drew (logical px), or might draw.
    area: Rect,
}

impl Ui<'_> { // This code is a part of L e m o n S t u d i o. Please don't upload it to ArtCraft.
    /// Paint one part of the frame, or, when it has painted before and `area` (where it paints now) and where it
    /// drew last time both miss the clip, keep last paint's hits for it instead.
    pub fn part(&mut self, seg: Seg, area: Rect, paint: impl FnOnce(&mut Self)) {
        let (h0, b0, l0) = (self.hits.len(), self.bars.len(), self.live.len());
        let old = self.keep.parts.iter().position(|p| p.seg == seg);
        let keep = !self.keep.full
            && old.is_some_and(|i| {
                let p = &self.keep.parts[i];
                p.hits.end <= self.keep.prev.len()
                    && p.bars.end <= self.keep.prev_bars.len()
                    && p.live.end <= self.keep.prev_live.len()
                    && !self.c.visible(p.area.union(&area))
            });
        let area = match old {
            Some(i) if keep => {
                let (hits, bars, live, area) = {
                    let p = &self.keep.parts[i];
                    (p.hits.clone(), p.bars.clone(), p.live.clone(), p.area)
                };
                let dummy = || Hit { rect: Rect::default(), action: Action::Swallow, hint: Cow::Borrowed(""), win: None };
                let prev = &mut self.keep.prev;
                self.hits.extend(hits.map(|k| std::mem::replace(&mut prev[k], dummy())));
                self.bars.extend_from_slice(&self.keep.prev_bars[bars]);
                self.live.extend_from_slice(&self.keep.prev_live[live]);
                area
            }
            _ => {
                // Parts may sit inside another part: that one learns what this one drew too.
                let before = self.c.begin_drawn();
                paint(self);
                let drawn = self.c.end_drawn(before).map_or(area, |d| d.union(&area));
                // A partial paint only saw part of what the view draws: keep last paint's extent as well.
                match old {
                    Some(i) if !self.keep.full => drawn.union(&self.keep.parts[i].area),
                    _ => drawn,
                }
            }
        };
        self.keep.next.push(Part { seg, hits: h0..self.hits.len(), bars: b0..self.bars.len(), live: l0..self.live.len(), area });
    }

    /// Register a region whose look follows playback (repainted while playing, see `Live`).
    pub fn live(&mut self, kind: Live, r: Rect) {
        self.live.push((kind, r));
    }

    /// Paints `paint` at display scale `scale` instead of the frame's (popup menus, spec: ui/ui-scaling.md §1). It works
    /// in logical px of that scale: a frame position `x` is `x / k` there (`k`, handed to it, = `scale` / frame scale);
    /// what it registers (hits, bars, live regions) comes back in the frame's px.
    pub fn scaled(&mut self, scale: f32, paint: impl FnOnce(&mut Self, f32)) {
        let k = scale / self.c.scale;
        if !k.is_finite() || k <= 0.0 || (k - 1.0).abs() < 1e-4 {
            return paint(self, 1.0);
        }
        let (h0, b0, l0, mouse) = (self.hits.len(), self.bars.len(), self.live.len(), self.mouse);
        self.mouse = (mouse.0 / k, mouse.1 / k);
        let old = self.c.rescale(scale);
        paint(self, k);
        self.c.unscale(old);
        self.mouse = mouse;
        let up = |r: Rect| Rect::new(r.x * k, r.y * k, r.w * k, r.h * k);
        for h in &mut self.hits[h0..] {
            h.rect = up(h.rect);
        }
        for b in &mut self.bars[b0..] {
            (b.track, b.thumb) = (up(b.track), b.thumb * k);
        }
        for (_, r) in &mut self.live[l0..] {
            *r = up(*r);
        }
    }
}

/// What views paint with.
pub struct Ui<'a> {
    pub c: &'a mut Canvas,
    pub t: &'a Palette,
    hits: &'a mut Vec<Hit>,
    pub bars: &'a mut Vec<Bar>,
    hover: &'a Option<Action>,
    pressed: &'a Option<Action>,
    pub mouse: (f32, f32),
    /// The window being painted (tags hits).
    pub win: Option<Win>,
    keep: &'a mut Keep,
    live: &'a mut Vec<(Live, Rect)>,
    /// Square toolbar buttons are drawn flat (the Toolbars menu, spec: ui/toolbar-menu.md §3).
    pub flat: bool,
}

impl Ui<'_> {
    /// Whether `action` is held down by the left button with the cursor still over `rect`.
    pub fn down(&self, rect: Rect, action: &Action) -> bool {
        self.pressed.as_ref() == Some(action) && rect.contains(self.mouse.0, self.mouse.1)
    }

    /// Whether `action` is held down by the left button, wherever the cursor is (a drag that began on it).
    pub fn held(&self, action: &Action) -> bool {
        self.pressed.as_ref() == Some(action)
    }

    /// Whether the mouse is over `rect` and last frame's topmost hit there was registered by this window
    /// (for decorations without their own action, e.g. a knob drawn under its hit region).
    pub fn hovering(&self, rect: Rect) -> bool {
        self.hover.is_some() && rect.contains(self.mouse.0, self.mouse.1)
    }

    /// Hits registered so far this frame.
    pub fn hit_count(&self) -> usize {
        self.hits.len()
    }

    /// Register a hit region; returns whether it is hovered (topmost last frame).
    pub fn hit(&mut self, rect: Rect, action: Action, hint: impl Into<Cow<'static, str>>) -> bool {
        let hovered = self.hover.as_ref() == Some(&action) && rect.contains(self.mouse.0, self.mouse.1);
        self.hits.push(Hit { rect, action, hint: hint.into(), win: self.win });
        hovered
    }

    /// A control that is drawn but does nothing yet: its hit only explains that (hint and click message), so it never
    /// lights up, toggles or acts. Draw the control greyed out: `kit::State::disabled`, `menu_pop_dim` text, or `veil`.
    pub fn off(&mut self, rect: Rect, name: &'static str) {
        self.hits.push(Hit { rect, action: Action::Placeholder(name), hint: Cow::Owned(off_text(name)), win: self.win });
    }

    /// Greys out what was just drawn in `rect` (a disabled control without a disabled look of its own): a veil of the
    /// background colour `bg` under it.
    pub fn veil(&mut self, rect: Rect, bg: os_ui::Color) {
        self.c.fill_rect(rect, bg.with_alpha(OFF_VEIL));
    }
}

/// Opacity of `Ui::veil`: a greyed-out control keeps about 40 % of its contrast.
pub const OFF_VEIL: u8 = 155;

/// Hint and message of a control or menu entry that is not implemented yet.
pub fn off_text(name: &str) -> String {
    format!("{name}: not available yet")
}

/// What is not available where plugin windows are not (`cfg(plugin_windows)`: Windows, Linux and macOS so far).
#[cfg(not(plugin_windows))]
pub const PLUGIN_WINDOWS: &str = "Plugin windows on this system";

/// The plugin window's menus, preset list and Misc lists are app popups painted in the main window: off Windows a
/// detached window would cover them (Windows lifts them above it).
#[cfg(all(plugin_windows, not(windows)))]
const PLUGIN_POPUPS: &str = "Menus of detached plugin windows on Linux (dock the window)";

#[derive(Debug, Clone)]
pub enum Drag {
    /// Vertical drag on the pattern name box steps the number (spec: ui/pattern-menu.md §1).
    PatternBox { y0: f32, p0: u16 },
    /// Move a floating window (offset of the mouse from its top-left).
    Window { win: Win, dx: f32, dy: f32 },
    /// Resize a Playlist / Mixer / Piano roll window (`r0` = its rect when the drag began).
    WinResize { win: Win, edges: u8, m0: (f32, f32), r0: Rect },
    /// Resize the channel rack from its right / bottom edges.
    RackResize { edges: u8, m0: (f32, f32), s0: (f32, f32) },
    /// Change a channel's mixer track (spec: ui/channel-rack.md §5).
    RackMixer { ch: usize, y0: f32, v0: u16 },
    Fader { track: u16, y0: f32, v0: f64 },
    /// A press on a track's equaliser graph `g`: band `band` follows the cursor.
    EqGraph { track: u16, band: u8, g: Rect },
    /// A plain press on a mixer strip, held: the strip under the cursor is selected as it slides across, to preview
    /// tracks one after another.
    MixSelect,
    Tempo { y0: f32, t0: f64, step: f64 },
    /// The song position slider's handle (`dx` = grab offset from its centre), or the button held on its track (the
    /// next step is due at `next`) (spec: ui/song-position-slider.md §3).
    SongPos { dx: f32 },
    SongPosHold { next: std::time::Instant },
    /// The piano roll's grid / event panel splitter: `dy` = grab offset from the band's top, `top` = the grid's top,
    /// `h` = the height the grid and the panel share.
    PianoSplit { dy: f32, top: f32, h: f32 },
    /// Monitor debug panel slider.
    MonParam(usize),
    /// Cable physics debug panel slider.
    CableParam(usize),
    /// Piano roll: move notes (originals by index), resize their ends, erase, draw velocities.
    NoteMove { x0: f32, y0: f32, orig: Vec<(usize, Note)> },
    NoteResize { x0: f32, orig: Vec<(usize, Note)> },
    NoteErase,
    /// Mute tool swipe: every note touched gets `on` (the first note's new state, §9).
    NoteMute { on: bool },
    /// Paint tool: notes back to back on `key` from `start`, `len` apart (§9).
    NotePaint { key: i32, start: u32, len: u32 },
    /// Select tool marquee from (x0, y0); `base` = selection kept (Ctrl adds).
    Marquee { x0: f32, y0: f32, base: Vec<usize> },
    /// Slice tool line, Zoom tool box, Playback scrub.
    Slice,
    ZoomBox { x0: f32, y0: f32 },
    Scrub { tick: f32 },
    /// Ruler: move the play-start marker; right-drag a time selection from `t0`.
    Marker,
    /// Ruler in SONG mode over a pattern the playlist plays: the song position follows (see `piano_song_seek`).
    SongRuler,
    TimeSel { t0: u32 },
    /// Painting the event panel (note property or channel control points).
    Velocity { last_x: f32, last_y: f32 },
    /// Right-drag in a channel lane deletes its points (a Lemon Studio choice, spec §7b).
    ControlErase { last_x: f32 },
    Knob { id: crate::knobs::KnobId, y0: f32, v0: f32 },
    /// Left button held on the piano keyboard: each key entered is auditioned (§8).
    PianoKeys { key: i32 },
    /// Scroll axes from a start mouse position: value = start + (mouse - m0) × ratio per axis
    /// (ratio < 0 grabs the content, as with the middle button).
    Scroll { m0: (f32, f32), axes: Vec<(Scroll, bool, f32, f32)> },
    /// Ctrl+middle-drag in the piano roll: vertical movement zooms the key rows around `y0` (up = taller rows,
    /// spec: ui/piano-roll.md §3a).
    PianoRowZoom { y0: f32, row0: f32 },
    /// Right-drag on a horizontal scroll bar: horizontal mouse movement scrolls the bar, vertical movement
    /// zooms its axis around the view centre (up = zoom in).
    BarZoom { t: Scroll, m0: (f32, f32), p0: f32, ratio: f32, z0: f32, center: f32, view: f32 },
    /// Right-drag on a vertical scroll bar: vertical movement scrolls (as in FL), horizontal movement resizes the
    /// rows around the view centre where the bar has rows (right = taller). `last` = mouse at the previous move,
    /// `row` = unrounded row height so slow drags still add up (None: the bar only scrolls). Spec: ui/scroll-bars.md.
    BarRows { t: Scroll, last: (f32, f32), row: Option<f32> },
    /// A playlist drag (spec: ui/playlist.md).
    Pl(crate::playlist::PlDrag),
    /// A browser item pressed (`started` once it moved far enough), spec: ui/browser.md §3.
    Browser { item: views::browser::Item, label: String, m0: (f32, f32), started: bool },
    /// Channel rack (spec: ui/channel-rack.md): step painting along the start row (§9), slot range select (§8),
    /// name width (§7), pattern length (§7), a row's loop length (§15), graph editor values (§14).
    RackPaint { row: usize, on: bool, last: u32 },
    RackSel { from: usize },
    RackDots { x0: f32, p0: u32 },
    RackLen { y0: f32, v0: u32 },
    RackLoopLen { row: usize, y0: f32, v0: u32 },
    RackGraph,
    /// A dialog's caption (offset of the mouse from its top-left), the file list's scroll bar (`ratio` = list pixels
    /// per pixel moved), spec: ui/dialogs.md.
    Dialog { dx: f32, dy: f32 },
    DialogBar { y0: f32, s0: f32, ratio: f32 },
    /// A slider declared in `crate::prefs` follows the mouse along its track (`w` wide from `x`).
    PrefSlider { label: &'static str, x: f32, w: f32 },
    /// General ▸ Display's PPI ruler, pressed at `x0` while it showed `n0` (`crate::ppi::dragged`).
    Ppi { x0: f32, n0: u16 },
}

pub struct State {
    pub project: Project,
    /// The toolbar's scope / analyzer and peak bars (fed from the audio tap in `App::tick`).
    pub monitor: crate::monitor::Monitor,
    /// The plugin database: choices, last scan, the plugin menus' lists, the plugin manager's table (spec:
    /// ui/plugin-manager.md, ui/plugin-menu.md).
    pub plugdb: crate::plugin_db::Db,
    /// The toolbar's CPU panel (read in `App::tick`).
    pub cpu: crate::cpu::Cpu,
    /// The Plugin performance monitor's readings and options (read with the CPU panel while it is shown).
    pub perf: crate::perf::Perf,
    /// The Tempo tapper: where it is, the count of taps, the pad's flash.
    pub tapper: crate::tapper::TapperUi,
    /// Mixer selection, slot requests, meters (spec: ui/mixer.md).
    pub mixer: crate::mixer::MixerUi,
    /// Rack rows played by a hosted plugin.
    pub hosted_rows: Vec<usize>,
    /// Requests for the plugin host, handled on the next tick: (row, `Plugins::available` index) to create, row to open.
    pub pending_plugin: Option<(usize, usize)>,
    pub pending_open: Option<usize>,
    /// Samples to load into rows' Lemon Sampler instances on the next tick (browser send, drops on a rack row).
    pub sample_loads: Vec<(usize, String)>,
    pub file: Option<os_flp::FlpFile>,
    pub path: Option<PathBuf>,
    /// Open child windows, bottom to top.
    pub z: Vec<Win>,
    /// The empty workspace took the last click: no window has the focus (Ctrl+F12 closes them all,
    /// spec: ui/close-windows.md §2).
    pub no_focus: bool,
    pub show_browser: bool,
    pub rack_pos: (f32, f32),
    pub rack_size: (f32, f32),
    /// Horizontal step scroll of the rack, pixels.
    pub rack_scroll_x: f32,
    /// Channel rack selection, filters, view options, graph editor, loops (spec: ui/channel-rack.md §7-22).
    pub rack: crate::rack::RackUi,
    /// Rows show mini piano rolls instead of steps.
    pub rack_roll_view: bool,
    /// Channel shown in the plugin window, and where that window is.
    pub plugin_channel: usize,
    pub plugin_pos: (f32, f32),
    pub selected_channel: usize,
    pub selected_mixer: u16,
    pub pattern: u16,
    pub song_mode: bool,
    pub menu_open: Option<usize>,
    /// The open dropdown's submenu, shown as `playlist.popup`: the flat index of its ▸ entry.
    pub menu_sub: Option<usize>,
    /// Piano roll event panel Control menu.
    pub lane_menu: bool,
    /// Settings window (OPTIONS > ... settings, F10 / F11).
    pub settings: views::settings::SettingsUi,
    /// The `.flstheme` that colours the Classic design, as its file holds it; `None`: none (the stock Dark look, or a
    /// Lemon Studio `theme.txt`). Theme ▸ Options rewrite it (spec: ui/settings-choices.md "Light mode and clip note
    /// colours").
    pub fl_text: Option<String>,
    pub playlist_scroll: (f32, f32),
    /// Playlist horizontal zoom, pixels per bar.
    pub playlist_bar_w: f32,
    pub mixer_scroll: f32,
    pub rack_scroll: f32,
    /// Browser (spec: ui/browser.md): active tab, opened node keys, list scroll (px), selected row key,
    /// and the drag label with the mouse position while an item is dragged.
    pub browser_tab: usize,
    pub browser_open: std::collections::HashSet<String>,
    /// Folders dropped on the window: All-tab roots (spec: ui/browser.md §6).
    pub browser_folders: Vec<PathBuf>,
    /// Their Browser names (File ▸ Browser extra search folders, `crate::search_folders`): a root's label in place of
    /// the folder's name.
    pub browser_names: std::collections::BTreeMap<PathBuf, String>,
    /// Directory listings of dropped folders, until reread.
    pub browser_dirs: std::cell::RefCell<std::collections::HashMap<PathBuf, std::rc::Rc<Vec<views::browser::Entry>>>>,
    /// The mixer's routing cables, hung by the mixer view as it paints and swung on the tick.
    pub cables: std::cell::RefCell<crate::cable::Cables>,
    pub browser_scroll: f32,
    pub browser_sel: Option<String>,
    pub browser_drag: Option<(String, f32, f32)>,
    /// Search text and whether the search box has keyboard focus (spec: ui/browser.md §5).
    pub browser_search: String,
    pub browser_search_focus: bool,
    /// Options-menu toggles that are on (`views::browser::OPT_*`).
    pub browser_opts: std::collections::HashSet<&'static str>,
    /// Navigation history of selected row keys and the position in it.
    pub browser_hist: Vec<String>,
    pub browser_hist_pos: usize,
    /// Plugin parameter lists and the last tweaked parameters (spec: plugins/plugin-parameters.md).
    pub params: crate::params::ParamUi,
    pub tempo: f64,
    /// Where the tempo box's tenths, hundredths and thousandths digits start (x), as last painted: a drag from a digit
    /// steps the tempo by that digit (views/toolbar.rs).
    pub tempo_digits: std::cell::Cell<[f32; 3]>,
    /// The time display's mode and middle unit (spec: ui/time-display.md): app-wide, for the session.
    pub clock: crate::clock::Clock,
    /// The Toolbars menu's switches (spec: ui/toolbar-menu.md §8): app-wide, for the session.
    pub toolbar: views::toolbar::Opts,
    pub message: Option<String>,
    pub toggles: Vec<&'static str>,
    /// MIDI input devices that are open (their notes play the selected channel; spec: ui/midi-input.md).
    pub midi_inputs: Vec<String>,
    pub maximized: bool,
    pub piano: PianoState,
    pub playlist: crate::playlist::PlaylistState,
    /// The project's undo history (spec: ui/settings-choices.md "Undo history: the four checks (2026-10-16)").
    pub history: crate::history::History,
    /// The main toolbar's snap, which the editors' Main follows (spec: ui/global-snap.md).
    pub global_snap: crate::snap::Global,
    pub pat_ui: crate::patterns::PatternUi,
    /// Playlist, Mixer, Piano roll (see `Win::slot`).
    pub wins: [WinState; 3],
    /// Size of the shell window in logical pixels (set each paint).
    pub screen: (f32, f32),
    /// The main window's monitor's own scale (device pixels per logical one, whatever Main GUI scaling says; set at
    /// every window event): General ▸ Display's PPI ruler follows it (`crate::ppi`).
    pub display_scale: f32,
    /// Open message boxes and file dialogs, bottom to top (spec: ui/dialogs.md).
    pub dialogs: Vec<crate::dialog::Modal>,
    /// The working time's clock and the Song info box (Settings > Info, `project_info`).
    pub credits: crate::project_info::Credits,
    /// The current theme's rule for the colours of new audio and automation clips' channels (`App::clip_colors`, kept
    /// up to date by `App::follow_settings`).
    pub clip_colors: crate::clip_colors::ClipColors,
}

impl State {
    /// The applied `.flstheme` (the stock Dark theme when there is none): its options `light_mode` and
    /// `override_clips` ("Audio and automation clips use note colors"; spec: ui/settings-choices.md "Light mode and
    /// clip note colours").
    pub fn fl_theme(&self) -> os_theme::FlTheme {
        self.fl_text.as_deref().map_or_else(os_theme::FlTheme::dark, os_theme::FlTheme::parse)
    }

    pub fn shown(&self, w: Win) -> bool {
        self.z.contains(&w)
    }

    /// Whether `w` is maximized and not collapsed (it fills the workspace).
    pub fn is_max(&self, w: Win) -> bool {
        w.slot().is_some_and(|i| self.wins[i].maxed && !self.wins[i].mini)
    }

    /// The maximized window in front (the one filling the workspace), if any.
    pub fn docked(&self) -> Option<Win> {
        self.z.iter().rev().copied().find(|&w| self.is_max(w))
    }

    /// The workspace: right of the browser, below the toolbar.
    pub fn work_rect(&self) -> Rect {
        let left = if self.show_browser { views::BROWSER_W } else { 0.0 };
        let top = views::work_y(self);
        Rect::new(left, top, (self.screen.0 - left).max(0.0), (self.screen.1 - top).max(0.0))
    }

    /// The window's floating rect, kept at least its minimum size and at most the screen's, with its part in the
    /// workspace (`win_clamp::keep`).
    pub fn float_rect(&self, w: Win) -> Rect {
        let Some(i) = w.slot() else { return Rect::new(0.0, 0.0, 0.0, 0.0) };
        let r = self.wins[i].rect;
        let (mw, mh) = w.min_size();
        let (sw, sh) = self.screen;
        let rw = r.w.min(sw).max(mw);
        let rh = r.h.min(sh - views::work_y(self)).max(mh);
        let (x, y) = crate::win_clamp::keep(r.x, r.y, rw, self.work_rect());
        Rect::new(x, y, rw, rh)
    }

    /// Where window `w` paints and takes clicks: its rect plus, while it floats, the resize zones past its edges.
    pub fn win_area(&self, w: Win) -> Rect {
        let r = self.win_rect(w);
        match w.slot() {
            Some(i) if !self.wins[i].maxed && !self.wins[i].mini => r.outset(4.0),
            _ => r,
        }
    }

    /// Where the window is painted: the workspace when maximized, a title bar when minimized.
    pub fn win_rect(&self, w: Win) -> Rect {
        let Some(i) = w.slot() else { return Rect::new(0.0, 0.0, 0.0, 0.0) };
        let f = self.float_rect(w);
        let ws = self.wins[i];
        if ws.mini {
            Rect::new(f.x, f.y, f.w, TITLE_H)
        } else if ws.maxed {
            self.work_rect()
        } else {
            f
        }
    }

    /// Show `w` and bring it to the front.
    pub fn raise(&mut self, w: Win) {
        self.z.retain(|&o| o != w);
        self.z.push(w);
        self.no_focus = false;
    }

    pub fn hide(&mut self, w: Win) {
        self.z.retain(|&o| o != w);
    }

    /// Whether `w` is in front of everything that could cover it: the top window for floating
    /// windows, the top maximized window for maximized ones (floating windows never cover them fully).
    pub fn in_front(&self, w: Win) -> bool {
        if self.is_max(w) { self.docked() == Some(w) } else { self.z.last() == Some(&w) }
    }
}

pub struct App {
    pub state: State,
    pub plugins: crate::plugins::Plugins,
    pub theme: Theme,
    pub transport: Transport,
    pub hits: Vec<Hit>,
    keep: Keep,
    /// Live regions from the last paint.
    live: Vec<(Live, Rect)>,
    /// Playback state the frame shows (see `Shown`).
    shown: Shown,
    /// Rects of last frame's overlays (menus, popups...): docked plugin windows are clipped under them.
    overlays: Vec<Rect>,
    /// Where the canvas sits in the main window's client area (physical px; the restored window's border).
    pub canvas_off: (i32, i32),
    /// The open popup belongs to this row's detached plugin window: it is shown lifted above that window.
    #[cfg_attr(not(plugin_windows), allow(dead_code))]
    popup_lift: Option<usize>,
    pub bars: Vec<Bar>,
    pub hover: Option<Action>,
    /// Action under the cursor while the left button is held (pressed look of buttons).
    pub pressed: Option<Action>,
    /// A window button pressed and not released yet (`acts_on_release`).
    armed: Option<Action>,
    /// A toolbar button right-pressed and not released yet: (its hit, what the release over it runs).
    armed_right: Option<(Action, Action)>,
    hover_rect: Option<Rect>,
    damage: Damage,
    /// Typing keyboard (spec: ui/typing-keyboard.md): keys held now as (physical key, rack row,
    /// note key), the octave shift, and whether the transport has the project for live notes.
    typing_held: Vec<(KeyCode, usize, u16)>,
    pub typing_octave: i32,
    typing_synced: bool,
    /// The piano-roll key held with the mouse, sounding as a live note until released: (rack row, key).
    mouse_note: Option<(usize, u16)>,
    /// When the mixer's cables were last stepped (`None` while they are still).
    cables_at: Option<std::time::Instant>,
    /// Velocity of the next key the mouse sounds (a keyboard click sets it from the click's x; back to the default
    /// after use).
    key_velocity: u8,
    /// MIDI controller input, and its held notes: (key, rack row they started on).
    pub midi: crate::midi_in::MidiIn,
    midi_held: Vec<(u8, usize)>,
    /// Step edit's chord being collected from the typing keyboard and MIDI notes (`crate::step_edit`).
    pub(crate) step_chord: crate::step_edit::Chord,
    /// Last title-bar press (window, time, position), for double-click maximize / restore.
    pub(crate) title_click: Option<(Win, std::time::Instant, (f32, f32))>,
    /// Last channel-rack press (action, time), for double clicks on slots and mixer boxes (§8, §12).
    rack_click: Option<(Action, std::time::Instant)>,
    /// Last press on the CPU panel, for its double click (the Plugin performance monitor).
    cpu_click: Option<std::time::Instant>,
    pub hint: Cow<'static, str>,
    pub mouse: (f32, f32),
    pub drag: Option<Drag>,
    /// A held click that has not moved yet (Click-and-hold & special gesture functions, `crate::hold`).
    pub hold: Option<crate::hold::Hold>,
    /// Views easing to where the wheel sent them (General ▸ Display ▸ Smoothing, `crate::smooth`).
    pub(crate) smooth: crate::smooth::Eases,
    /// Audio ▸ Auto close: whether the output is closed while another program is in front (`crate::auto_close`).
    pub(crate) auto_close: crate::auto_close::AutoClose,
    pub want_minimize: bool,
    pub want_maximize: bool,
    pub want_close: bool,
    /// The window may close now (unsaved changes were answered): the shell ends the program.
    pub want_exit: bool,
    /// macOS: the answer to a Quit, for AppKit (`macos::answer_quit`).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub quit_answer: Option<bool>,
    /// Linux: while a dialog is open, detached plugin windows are not kept above the main window (they would cover it).
    #[cfg(all(plugin_windows, target_os = "linux"))]
    unpinned: Option<crate::plugin_window::Unpinned>,
    pub want_drag_window: bool,
    /// Show the horizontal-resize cursor (hovering or dragging a note end).
    pub resize_cursor: bool,
    /// Modifier keys held (set by the shell).
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// The right Shift key is held (the playlist's quick slice, ui/playlist.md §3).
    pub rshift: bool,
    /// Playlist regions from the last paint (grid, ruler, marker lane).
    pl_grid: Option<Rect>,
    /// Playlist scrolls since the last paint: how far the content moved (logical px, right and down), see
    /// `playlist_scrolled`.
    scrolled: Option<(f64, f64)>,
    /// Canvas areas the next paint moves before it paints: from `take_damage`.
    shifts: Vec<Moved>,
    /// Areas the last paint moved: the shell moves its window buffer the same way (`take_moved`).
    moved: Vec<Moved>,
    /// The canvas scale at the last paint.
    px_scale: f32,
    /// What covered the playlist at the last paint (`playlist_covers`).
    pl_covers: Vec<Cover>,
    /// Piano roll regions from the last paint (window, grid, velocity panel).
    piano_win: Option<Rect>,
    pub(crate) piano_grid: Option<Rect>,
    piano_panel: Option<Rect>,
    /// Sampler channel rows whose Lemon Sampler instance could not be created (not retried).
    sampler_failed: Vec<usize>,
    /// Mixer slots (`plugins::fx_key`) whose Lemon Parametric EQ could not start (not retried).
    pub(crate) fx_failed: Vec<usize>,
    /// The running plugin scan, and the scan asked for while it runs (spec: ui/plugin-manager.md §4).
    pub(crate) sweep: Option<crate::plugin_scan::Sweep>,
    pub(crate) scan_queued: Option<crate::plugin_db::Mode>,
    /// Wrapped plugin channels (rows) and slots (`fx_key`) whose plugin was not found or did not start (not retried).
    pub(crate) wrap_failed: Vec<usize>,
    /// Plugin channels (rows) and slots (`fx_key`) naming a native plugin that is not installed or did not start
    /// (not retried; spec: plugins/fruity-abi.md "Opened projects").
    pub(crate) native_failed: Vec<usize>,
    /// An opened project is still loading its samplers (spec: ui/project-loading.md): progress shows, input waits.
    loading: Option<crate::loading::Loading>,
    /// A WAV export is rendering (FILE > Export): its file name and progress (0..1); the same panel shows, input waits.
    export: Option<(String, f32)>,
    /// The project and tempo as last opened, saved or created: anything else has unsaved changes.
    pub(crate) saved: (Project, f64),
    /// The Autosave timer and the last backup (spec: ui/settings-choices.md "Backup files").
    pub(crate) backup: crate::backup::Backup,
}

/// Window-level requests the shell must carry out after an event.
impl App {
    pub fn new(path: Option<PathBuf>) -> App {
        let transport = Transport::new(os_engine::SampleLibrary::default_factory_dir());
        // Tests keep the old five-channel template; a new session starts with one Lemon Sampler channel.
        #[cfg(test)]
        let project = test_project();
        #[cfg(not(test))]
        let project = default_project();
        let mut app = App {
            plugins: crate::plugins::Plugins::default(),
            state: State {
                project,
                plugdb: Default::default(),
                hosted_rows: Vec::new(),
                pending_plugin: None,
                pending_open: None,
                sample_loads: Vec::new(),
                file: None,
                path: None,
                z: vec![Win::Playlist, Win::ChannelRack],
                no_focus: false,
                show_browser: true,
                rack_pos: (317.0, 195.0),
                rack_size: views::rack::DEFAULT_SIZE,
                rack_scroll_x: 0.0,
                rack: Default::default(),
                rack_roll_view: false,
                plugin_channel: 0,
                plugin_pos: (560.0, 260.0),
                selected_channel: 0,
                selected_mixer: 1,
                mixer: Default::default(),
                pattern: 1,
                song_mode: false,
                menu_open: None,
                menu_sub: None,
                lane_menu: false,
                settings: views::settings::SettingsUi { native: os_native::settings_shown(), ..Default::default() },
                fl_text: saved_fl_text(),
                playlist_scroll: (0.0, 0.0),
                playlist_bar_w: views::playlist::BAR_W,
                mixer_scroll: 0.0,
                rack_scroll: 0.0,
                browser_tab: 0,
                browser_open: views::browser::default_open(),
                browser_folders: crate::browser::load_folders(),
                browser_names: crate::browser::load_names(),
                params: Default::default(),
                browser_dirs: Default::default(),
                cables: std::cell::RefCell::new({
                    let mut c = crate::cable::Cables::with(load_cable_cfg());
                    c.animate(load_animations());
                    c
                }),
                browser_scroll: 0.0,
                browser_sel: None,
                browser_drag: None,
                browser_search: String::new(),
                browser_search_focus: false,
                browser_opts: views::browser::DEFAULT_OPTS.iter().copied().collect(),
                browser_hist: Vec::new(),
                browser_hist_pos: 0,
                tempo: 130.0,
                tempo_digits: std::cell::Cell::new([f32::INFINITY; 3]),
                message: None,
                monitor: crate::monitor::Monitor::with_cfg(load_monitor_cfg()),
                cpu: crate::cpu::Cpu::saved(),
                perf: crate::perf::Perf::default(),
                tapper: Default::default(),
                toggles: vec!["typing"],
                clock: crate::clock::Clock::load(),
                toolbar: views::toolbar::Opts::load(),
                midi_inputs: Vec::new(),
                maximized: true,
                piano: PianoState::default(),
                playlist: crate::playlist::PlaylistState::default(),
                history: Default::default(),
                global_snap: Default::default(),
                pat_ui: Default::default(),
                wins: [
                    WinState { rect: Rect::new(693.0, 101.0, 1126.0, 378.0), maxed: true, mini: false },
                    WinState { rect: Rect::new(231.0, 565.0, 1552.0, 435.0), maxed: true, mini: false },
                    WinState { rect: Rect::new(480.0, 140.0, 1000.0, 640.0), maxed: true, mini: false },
                ],
                screen: (1920.0, 1080.0),
                display_scale: 1.0,
                dialogs: Vec::new(),
                credits: Default::default(),
                clip_colors: Default::default(),
            },
            theme: load_theme(),
            transport,
            hits: Vec::new(),
            keep: Keep::default(),
            live: Vec::new(),
            shown: Shown::default(),
            overlays: Vec::new(),
            canvas_off: (0, 0),
            popup_lift: None,
            bars: Vec::new(),
            hover: None,
            pressed: None,
            armed: None,
            armed_right: None,
            hover_rect: None,
            damage: Damage::Full,
            typing_held: Vec::new(),
            typing_octave: 0,
            typing_synced: false,
            mouse_note: None,
            cables_at: None,
            key_velocity: TYPING_VELOCITY,
            midi: crate::midi_in::MidiIn::none(),
            midi_held: Vec::new(),
            step_chord: crate::step_edit::Chord::default(),
            title_click: None,
            rack_click: None,
            cpu_click: None,
            hint: Cow::Borrowed(""),
            mouse: (-1.0, -1.0),
            drag: None,
            hold: None,
            smooth: Default::default(),
            auto_close: Default::default(),
            want_minimize: false,
            want_maximize: false,
            want_close: false,
            want_exit: false,
            quit_answer: None,
            #[cfg(all(plugin_windows, target_os = "linux"))]
            unpinned: None,
            want_drag_window: false,
            resize_cursor: false,
            ctrl: false,
            shift: false,
            alt: false,
            rshift: false,
            pl_grid: None,
            scrolled: None,
            shifts: Vec::new(),
            moved: Vec::new(),
            px_scale: 1.0,
            pl_covers: Vec::new(),
            piano_win: None,
            piano_grid: None,
            piano_panel: None,
            sampler_failed: Vec::new(),
            fx_failed: Vec::new(),
            sweep: None,
            scan_queued: None,
            wrap_failed: Vec::new(),
            native_failed: Vec::new(),
            loading: None,
            export: None,
            saved: (Project::default(), 0.0),
            backup: Default::default(),
        };
        // The snap settings are program settings, kept in the user folder (spec: ui/global-snap.md §6).
        let snap = crate::snap::load();
        app.state.global_snap = snap.global;
        app.state.piano.snap = snap.piano;
        app.state.playlist.snap = snap.playlist;
        crate::project_info::created(&mut app.state);
        app.mark_saved();
        if let Some(p) = path {
            app.open(p);
        }
        app
    }

    pub fn title(&self) -> String {
        let s = &self.state;
        let name = s.project.info.title.clone().filter(|t| !t.is_empty()).or_else(|| {
            s.path.as_ref().and_then(|p| p.file_stem()).map(|n| n.to_string_lossy().into_owned())
        });
        match name {
            Some(n) => format!("[{n}]"),
            None => "[untitled]".into(),
        }
    }

    pub fn open(&mut self, path: PathBuf) {
        // A project in the layout used before version 24.2 gets a dialog that says what to do (spec:
        // flp/events/channels.md "Version scope"), not just a line in the hint bar.
        let mut legacy = None;
        let result = std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|b| os_flp::FlpFile::parse(&b).map_err(|e| e.to_string()))
            .and_then(|f| match os_flp::project::load(&f) {
                Ok(p) => Ok((f, p)),
                Err(os_flp::project::ProjectError::LegacyLayout(v)) => {
                    let e = os_flp::project::ProjectError::LegacyLayout(v.clone()).to_string();
                    legacy = Some(v);
                    Err(e)
                }
                Err(e) => Err(e.to_string()),
            });
        if let Some(v) = legacy {
            let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            let saved = if v.trim().is_empty() { "an older version".to_string() } else { format!("version {}", v.trim()) };
            let rest = format!(
                " yet.\n\nIt was saved by {saved}. Projects saved before version 24.2 use an older channel layout that Lemon \
                 Studio does not read yet.\n\nOpen the project in version 24.2 or later, save it again, then open the new copy \
                 here."
            );
            self.tell(crate::dialog::Kind::Info, "Project not supported yet", crate::dialog::spans(&["Lemon Studio cannot open ", &name, &rest]));
        }
        match result {
            Ok((file, project)) => {
                self.transport.stop();
                self.reset_hosted();
                let s = &mut self.state;
                s.tempo = project.tempo.unwrap_or(130.0);
                s.pattern = project.current_pattern.unwrap_or(1).max(1);
                s.selected_channel = 0;
                s.selected_mixer = 1;
                s.mixer = Default::default();
                s.playlist_scroll = (0.0, 0.0);
                s.rack_scroll = 0.0;
                s.rack_scroll_x = 0.0;
                s.rack.reset();
                s.hide(Win::Plugin);
                s.mixer_scroll = 0.0;
                s.song_mode = project.arrangement().is_some_and(|a| !a.clips.is_empty());
                s.project = project;
                crate::project_info::opened(s);
                crate::rack::from_model(s);
                s.file = Some(file);
                s.message = Some(format!("Opened {}", path.display()));
                crate::templates::remember(&path);
                s.path = Some(path);
                s.piano.reset();
                s.playlist.reset();
                s.history.reset();
                crate::playlist::load_view(s);
                s.piano.dirty = true; // live notes use the new project
                piano::center_keys(s, self.piano_grid.map_or(700.0, |g| g.h));
                self.mark_saved();
                self.loading = Some(crate::loading::Loading::start(&self.state.project));
                self.state.playlist.wave_wait = true;
                self.invalidate();
            }
            Err(e) => self.state.message = Some(format!("Could not open {}: {e}", path.display())),
        }
    }

    /// FILE > Save (`choose` = false: to the project's file, when it has one) and Save as.
    fn save(&mut self, choose: bool) {
        if choose {
            self.save_dialog(None);
        } else {
            self.save_then(None);
        }
    }

    /// Saves the project to its file, or asks where (a project without one); then `next`, once it is saved.
    fn save_then(&mut self, next: Option<crate::dialog::Next>) {
        let Some(path) = self.state.path.clone() else { return self.save_dialog(next) };
        self.save_to(path);
        if let Some(n) = next {
            self.go_on_if_saved(n);
        }
    }

    /// The Save project as dialog, starting in the project's folder with its name.
    fn save_dialog(&mut self, next: Option<crate::dialog::Next>) {
        use crate::filedlg::{FileDlg, Mode};
        let path = self.state.path.clone();
        let name = path.as_ref().and_then(|p| p.file_name()).map_or_else(|| "Untitled.flp".to_string(), |n| n.to_string_lossy().into_owned());
        let dlg = FileDlg::new(Mode::Save, "Save project as", "project", crate::dialog::Then::SaveProject(next))
            .filter("FLP project", &["flp"])
            .start(path.as_deref().and_then(|p| p.parent()))
            .name(&name);
        self.file_dialog(dlg);
    }

    fn go_on_if_saved(&mut self, n: crate::dialog::Next) {
        if self.unsaved() { self.held_back(n) } else { self.go_on(n) }
    }

    /// FILE > Export as WAV (Ctrl+R, the toolbar's render button): renders what Play would play (the playlist in SONG
    /// mode, one pass of the current pattern in PAT mode) with every hosted instrument and effect to a 32-bit float WAV
    /// file, at the output device's rate. Playback stops; a progress panel shows until the file is written.
    // spec: audio/export.md
    fn export_wav(&mut self) {
        if self.export.is_some() {
            return;
        }
        let p = crate::rack::play_project(&self.state);
        let song = self.state.song_mode;
        if song && p.arrangement().is_none_or(|a| a.clips.is_empty()) {
            self.state.message = Some("Export: the playlist is empty (switch to PAT to export the current pattern)".into());
            return self.invalidate();
        }
        if !song && p.patterns.iter().find(|q| q.number == self.state.pattern).is_none_or(|q| q.notes.is_empty()) {
            self.state.message = Some(format!("Export: {} has no notes", crate::patterns::name_of(&self.state, self.state.pattern)));
            return self.invalidate();
        }
        let stem = self.state.path.as_ref().and_then(|p| p.file_stem()).map_or_else(|| "Untitled".to_string(), |n| n.to_string_lossy().into_owned());
        let dir = self.state.path.as_ref().and_then(|p| p.parent()).map(std::path::Path::to_path_buf);
        let dlg = crate::filedlg::FileDlg::new(crate::filedlg::Mode::Save, "Export as WAV", "export", crate::dialog::Then::ExportWav)
            .filter("WAV audio", &["wav"])
            .start(dir.as_deref())
            .name(&format!("{stem}.wav"));
        self.file_dialog(dlg);
    }

    /// The export's file was chosen: the render starts.
    fn export_to(&mut self, mut target: PathBuf) {
        if self.export.is_some() {
            return;
        }
        if target.extension().is_none() {
            target.set_extension("wav");
        }
        let p = crate::rack::play_project(&self.state);
        let rate = Transport::device_rate().unwrap_or(44100);
        let name = target.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        self.plugins.set_offline(true);
        self.transport.export(&p, self.play_mode(), rate, target);
        self.export = Some((name, 0.0));
        self.invalidate();
    }

    /// Follows a running export: moves the progress bar, then reports the result.
    fn export_tick(&mut self) {
        let Some((name, _)) = self.export.clone() else { return };
        match self.transport.export_status() {
            crate::transport::Export::Running(p) => {
                self.export = Some((name, p));
                self.damage_rect(crate::loading::panel_rect(self.state.screen).outset(4.0));
            }
            crate::transport::Export::Done(r) => {
                self.export = None;
                self.plugins.set_offline(false);
                self.state.message = Some(match r {
                    Ok(secs) => format!("Exported {}:{:02} to {name}", (secs / 60.0) as u32, (secs % 60.0) as u32),
                    Err(e) => format!("Export failed: {e}"),
                });
                self.invalidate();
            }
            crate::transport::Export::Idle => {}
        }
    }

    /// Saves the project to `target`; a file over the Project data size warning is said (`App::size_notice`).
    pub(crate) fn save_to(&mut self, target: PathBuf) {
        if let Some(bytes) = self.write_project(target) {
            self.size_notice(bytes);
        }
    }

    /// Writes the project to `target`: the size of the file written, None when the save failed.
    fn write_project(&mut self, target: PathBuf) -> Option<u64> {
        // A file saved over is kept as an "overwritten on" backup (spec: ui/settings-choices.md "Backup files").
        crate::backup::keep_overwritten(&target, crate::data_folder::backup_folder(&self.state, Some(&target)));
        // Sampler channels keep their sample and settings in Lemon Sampler: copy them to the channel so the FLP stores
        // them.
        for row in self.state.hosted_rows.clone() {
            if self.state.project.channels.get(row).is_some_and(|c| c.kind.is_sampler()) {
                let lemon = self.plugins.descriptor(row).is_some_and(|d| d.name == crate::plugins::LEMON_SAMPLER);
                if let Some(st) = self.plugins.state(row).filter(|_| lemon) {
                    crate::plugins::store_sampler(&mut self.state.project.channels[row], &st);
                }
                if let Some(p) = self.plugins.sampler_path(row) {
                    // A placeholder path that still names the same file is kept as the project wrote it.
                    let ch = &mut self.state.project.channels[row];
                    if ch.sample_path.as_deref().is_some_and(|old| crate::plugins::resolve_sample(old) == p) {
                        continue;
                    }
                    ch.sample_path = (!p.is_empty()).then_some(p);
                }
            }
        }
        self.store_builtin_states();
        let s = &mut self.state;
        s.project.tempo = Some(s.tempo);
        // Lock, zip, filter groups and loop lengths live in the rack UI (spec: ui/channel-rack.md, Saving).
        crate::rack::to_model(s);
        // The playlist's track height, zoom and scroll (event 242, spec: flp/events/playlist.md).
        crate::playlist::store_view(s);
        // A file Lemon Studio made before the project had a playlist has no arrangement block (FL's files always have
        // one): build it again from the model, which holds everything that file stored.
        let lacks_playlist = s.file.as_ref().and_then(|f| os_flp::project::load(f).ok()).is_some_and(|p| p.arrangements.is_empty())
            && !s.project.arrangements.is_empty();
        // A project that was not opened from a file gets a new FLP built from the model.
        let mut file = if s.file.is_none() || lacks_playlist {
            // The new file's blocks are the model's channels in order.
            for (i, c) in s.project.channels.iter_mut().enumerate() {
                c.rack.origin = Some(i as u16);
            }
            // FL's files always have a playlist, and the mixer is stored after it (spec: flp/events/mixer.md).
            if s.project.arrangements.is_empty() {
                s.project.arrangements.push(os_flp::project::default_arrangement());
            }
            let mut first = s.project.clone();
            crate::project_plugins::store_hosted(&mut first, &self.plugins);
            match os_flp::project::create(&first) {
                Ok(f) => f,
                Err(e) => {
                    s.message = Some(format!("Save failed: {e}"));
                    return None;
                }
            }
        } else {
            s.file.clone().expect("checked")
        };
        // Deleted, cloned and moved channels are written by `apply` (os-flp `restructure`); it edits a copy so a
        // failed save leaves the loaded file as it was.
        let lost = os_flp::project::unsaved_parts(&s.project, &file);
        // Mixer parts the file format mapping does not write yet (spec: ui/mixer.md, Lemon Studio status).
        // Hosted CLAP / VST3 plugins are written as wrapped plugins with their current state (spec: flp/events/plugins.md).
        let mut proj = s.project.clone();
        crate::project_info::stamp(s, &mut proj);
        crate::data_folder::stamp(s, &mut proj);
        crate::project_plugins::store_hosted(&mut proj, &self.plugins);
        // Added mixer tracks are written before the hidden last track (spec: flp/events/mixer.md "Adding tracks").
        let result = os_flp::project::apply(&proj, &mut file)
            .map_err(|e| e.to_string())
            .and_then(|()| file.to_bytes().map_err(|e| e.to_string()))
            .and_then(|b| std::fs::write(&target, &b).map(|()| b.len() as u64).map_err(|e| e.to_string()));
        s.message = Some(match &result {
            Ok(_) if lost.is_empty() => format!("Saved {}", target.display()),
            Ok(_) => format!("Saved {} (not stored yet: {})", target.display(), lost.join(", ")),
            Err(e) => format!("Save failed: {e}"),
        });
        if s.message.as_deref().is_some_and(|m| m.starts_with("Saved")) {
            // The saved file now holds the channels in model order, and the plugins as written.
            for (i, c) in s.project.channels.iter_mut().enumerate() {
                c.rack.origin = Some(i as u16);
            }
            crate::project_plugins::adopt_saved(&mut s.project, &proj);
            s.file = Some(file);
            crate::templates::remember(&target);
            s.path = Some(target);
            self.mark_saved();
        }
        result.ok()
    }

    /// Writes the project to `target` as Save does, as a copy: the project keeps its file, name, unsaved changes and the
    /// message (backups, spec: ui/settings-choices.md "Backup files"). None while a project loads or an export renders.
    pub(crate) fn save_copy(&mut self, target: PathBuf) -> Option<Result<(), String>> {
        if self.loading.is_some() || self.export.is_some() {
            return None;
        }
        let s = &mut self.state;
        let keep = (s.project.clone(), s.path.clone(), s.file.clone(), s.message.take(), self.saved.clone());
        // A copy is not the last used project (spec: ui/settings-choices.md "Miscellaneous: ... Startup project").
        let last = crate::templates::last_kept();
        // A copy never warns about its size.
        self.write_project(target);
        crate::templates::restore_last(last);
        let s = &mut self.state;
        let done = match s.message.take() {
            Some(m) if m.starts_with("Saved") => Ok(()),
            m => Err(m.map_or_else(String::new, |m| m.trim_start_matches("Save failed: ").to_string())),
        };
        (s.project, s.path, s.file, s.message, self.saved) = keep;
        Some(done)
    }

    /// The project as it is now counts as saved (opened, saved, new).
    pub(crate) fn mark_saved(&mut self) {
        self.saved = (self.state.project.clone(), self.state.tempo);
    }

    /// Whether the project changed since it was last opened, saved or created. Settings kept inside hosted plugins are
    /// not compared.
    pub fn unsaved(&self) -> bool {
        self.saved.0 != self.state.project || self.saved.1 != self.state.tempo
    }

    /// Before the project is replaced or the app closes (`doing`: "closing", ...): with unsaved changes, asks to save
    /// them (spec: ui/dialogs.md §1f), and `next` follows once they are saved or not wanted; otherwise right away.
    pub fn ask_discard(&mut self, doing: &str, next: crate::dialog::Next) {
        use crate::dialog::{spans, Answer, Kind, Msg, Then};
        #[cfg(test)]
        let quiet = !crate::dialog::fake::is_live();
        #[cfg(not(test))]
        let quiet = false;
        if !self.unsaved() || quiet {
            return self.go_on(next);
        }
        let name = self.state.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
        let text = match &name {
            Some(n) => spans(&["Save the changes to ", n, &format!(" before {doing}?")]),
            None => spans(&[&format!("Save the changes to the new project before {doing}?")]),
        };
        self.show_msg(Msg::new(Kind::Question, "Confirm", text, &[Answer::Yes, Answer::No, Answer::Cancel], Then::Discard(next)));
    }

    /// The work that waited for unsaved changes goes on.
    fn go_on(&mut self, n: crate::dialog::Next) {
        use crate::dialog::Next;
        match n {
            Next::Close => self.want_exit = true,
            Next::Quit => self.quit_answer = Some(true),
            Next::NewProject => self.new_project(),
            Next::OpenDialog => self.open_dialog(),
            Next::Open(p) => self.open(p),
        }
        self.invalidate();
    }

    /// The work that waited for unsaved changes is called off (cancelled, or the save did not happen).
    fn held_back(&mut self, n: crate::dialog::Next) {
        if n == crate::dialog::Next::Quit {
            self.quit_answer = Some(false);
        }
    }

    /// A dialog was answered: what waited for it (spec: ui/dialogs.md §5).
    pub(crate) fn dialog_done(&mut self, then: crate::dialog::Then, reply: crate::dialog::Reply) {
        use crate::dialog::{Answer, Reply, Then};
        let file = match &reply {
            Reply::File(p) => p.clone(),
            Reply::Button(_) => None,
        };
        match then {
            Then::Nothing | Then::Replace(_) => {}
            Then::Discard(next) => match reply {
                Reply::Button(Answer::Yes) => self.save_then(Some(next)),
                Reply::Button(Answer::No) => self.go_on(next),
                _ => self.held_back(next),
            },
            Then::SaveProject(next) => match (file, next) {
                (Some(p), next) => {
                    self.save_to(p);
                    if let Some(n) = next {
                        self.go_on_if_saved(n);
                    }
                }
                (None, Some(n)) => self.held_back(n),
                (None, None) => {}
            },
            Then::ExportWav => {
                if let Some(p) = file {
                    self.export_to(p);
                }
            }
            Then::OpenProject => {
                if let Some(p) = file {
                    self.open(p);
                }
            }
            Then::LoadTheme => {
                if let Some(p) = file {
                    self.load_theme_file(&p);
                }
            }
            Then::PluginFolder => {
                if let Some(p) = file {
                    self.plugin_folder_chosen(p);
                }
            }
            Then::BrowserFolder(row) => {
                if let Some(p) = file {
                    self.search_folder_chosen(row, p);
                }
            }
            Then::SavePreset(key) => {
                if let Some(p) = file {
                    self.state.message = crate::presets::save_chosen(self, key, p);
                }
            }
            Then::RackSample(row) => {
                if let Some(p) = file {
                    crate::rack::sample_chosen(self, row, p);
                }
            }
            // Cancel and the caption's × change nothing.
            Then::StretchClips(rows, method) => {
                if reply == Reply::Button(Answer::Ok) {
                    crate::rack::set_stretch(self, &rows, method);
                }
            }
            // spec: ui/pattern-time-signature.md §2-3: No leaves the pattern alone; closing the dialog without Accept
            // leaves it without a start marker.
            Then::ReplaceSignature(n) => {
                if reply == Reply::Button(Answer::Yes) {
                    self.replace_signature(n);
                }
            }
            Then::Signature(n, num, den) => {
                if reply == Reply::Button(Answer::Ok) {
                    crate::timesig::set_start(&mut self.state, n, num, den);
                }
            }
        }
        self.invalidate();
    }

    /// A dialog opened or closed: Linux's detached plugin windows step back while one is open.
    pub(crate) fn modal_changed(&mut self) {
        #[cfg(all(plugin_windows, target_os = "linux"))]
        {
            let open = self.modal_open();
            if open != self.unpinned.is_some() {
                self.unpinned = open.then(crate::plugin_window::unpinned);
            }
        }
        self.invalidate();
    }

    /// FILE > New project: the Default template (spec: ui/settings-choices.md "Miscellaneous: seven more checks,
    /// Default template and Startup project"); for Empty, or a template that does not load, the built-in project.
    fn new_project(&mut self) {
        if self.new_from_template() {
            return;
        }
        self.transport.stop();
        self.reset_hosted();
        let s = &mut self.state;
        s.project = default_project();
        crate::project_info::created(s);
        s.file = None;
        s.path = None;
        s.tempo = 130.0;
        s.pattern = 1;
        s.piano.reset();
        s.playlist.reset();
        s.history.reset();
        crate::playlist::load_view(s);
        s.rack.reset();
        crate::rack::from_model(s);
        s.piano.dirty = true;
        s.message = Some("New project".into());
        self.mark_saved();
    }

    /// FILE > Open: the open dialog, starting in the project's folder.
    fn open_dialog(&mut self) {
        let start = self.state.path.as_ref().and_then(|p| p.parent().map(std::path::Path::to_path_buf));
        let dlg = crate::filedlg::FileDlg::new(crate::filedlg::Mode::Open, "Open project", "project", crate::dialog::Then::OpenProject)
            .filter("FLP project", &["flp"])
            .start(start.as_deref());
        self.file_dialog(dlg);
    }

    /// FILE > Save new version (Ctrl+N): saves next to the project as "name_2.flp", "name_3.flp", ... (the first free
    /// number), which becomes the project's file. A project without a file asks for a name.
    fn save_new_version(&mut self) {
        let Some(path) = self.state.path.clone() else { return self.save(true) };
        let stem = path.file_stem().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let (base, n) = match stem.rsplit_once('_') {
            Some((b, d)) if !b.is_empty() && !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()) => (b.to_string(), d.parse::<u32>().unwrap_or(1)),
            _ => (stem.clone(), 1),
        };
        let dir = path.parent().map(PathBuf::from).unwrap_or_default();
        let target = (n + 1..).map(|k| dir.join(format!("{base}_{k}.flp"))).find(|p| !p.exists()).expect("a free name");
        self.save_to(target);
    }

    /// The built-in recreations write their settings into the model as the file's own plugin state: Lemon Parametric
    /// EQ into its slots, Lemon Trio into its channels.
    pub(crate) fn store_builtin_states(&mut self) {
        for key in self.plugins.hosted.keys().copied().filter(|&k| k >= crate::plugins::FX_KEY).collect::<Vec<_>>() {
            let (Some((t, k)), Some(st)) = (crate::plugins::fx_of(key), self.plugins.eq_file_state(key)) else { continue };
            if let Some(slot) = self.state.project.mixer.iter_mut().find(|m| m.index == t).and_then(|m| m.slots[usize::from(k)].as_mut()) {
                if slot.name == crate::plugins::EQ_FILE_NAME {
                    slot.state = st;
                }
            }
        }
        crate::osc3::store(&mut self.state.project, &self.plugins);
    }

    /// Drops every hosted instance (a new or opened project); `sync_samplers` hosts the new project's samplers.
    pub(crate) fn reset_hosted(&mut self) {
        self.plugins.clear();
        self.loading = None;
        self.state.playlist.wave_wait = false;
        self.state.hosted_rows.clear();
        self.sampler_failed.clear();
        self.fx_failed.clear();
        self.wrap_failed.clear();
        self.native_failed.clear();
        self.transport.set_instruments(Vec::new());
        self.transport.set_effects(Vec::new());
    }

    /// Every sampler channel is played and edited by Lemon Sampler (owner, 2026-10-10; spec: plugins/lemon-sampler.md §7):
    /// a channel without an instance gets one, started from the channel's sample and envelopes. Without Lemon Sampler
    /// (plugin not found) sampler channels keep the engine's own sample playback.
    /// With a `deadline` it stops after the instance that passes it; returns whether every sampler row is done.
    fn sync_samplers(&mut self, deadline: Option<std::time::Instant>) -> bool {
        let Some(index) = self.plugins.index_of(crate::plugins::LEMON_SAMPLER) else { return true };
        let mut added = false;
        let mut done = true;
        for row in 0..self.state.project.channels.len() {
            let ch = &self.state.project.channels[row];
            if !ch.kind.is_sampler() || self.state.hosted_rows.contains(&row) || self.sampler_failed.contains(&row) {
                continue;
            }
            if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
                done = false;
                break;
            }
            let st = crate::plugins::sampler_state(ch);
            if let Some(l) = &mut self.loading {
                l.hosted += 1;
                l.label = ch.name.clone().unwrap_or_default();
            }
            match self.plugins.create_with(row, index, Some(&st)) {
                Ok(_) => {
                    self.state.hosted_rows.push(row);
                    added = true;
                    // A channel just added (ADD ▸ Lemon Sampler): its window is about to open (spec:
                    // ui/settings-choices.md "Miscellaneous: naming, detached plugins and startup").
                    if self.state.pending_open == Some(row) {
                        self.plugins.added(row);
                    }
                }
                Err(e) => {
                    self.sampler_failed.push(row);
                    self.state.message = Some(format!("Lemon Sampler: {e}"));
                }
            }
        }
        if added {
            self.transport.set_instruments(self.plugins.instruments());
            self.state.piano.dirty = true;
        }
        done
    }

    /// Project loading (spec: ui/project-loading.md): samples decode on worker threads first, then the sampler
    /// instances start a few per tick (`loading::HOST_BUDGET`) so the progress panel keeps moving. Without a loading
    /// project, new sampler rows are hosted at once.
    fn loading_tick(&mut self) {
        let Some(decoded) = self.loading.as_mut().map(|l| l.decode_done()) else {
            self.sync_samplers(None);
            return;
        };
        if decoded && self.sync_samplers(Some(std::time::Instant::now() + crate::loading::HOST_BUDGET)) {
            self.loading = None;
            self.state.playlist.wave_wait = false;
            return self.invalidate();
        }
        self.damage_rect(crate::loading::panel_rect(self.state.screen).outset(4.0));
    }

    /// A project is loading or an export is rendering: the window shows the progress and takes no input (spec:
    /// ui/project-loading.md §2).
    pub fn loading(&self) -> bool {
        self.loading.is_some() || self.export.is_some()
    }

    /// Mixer slots that hold the project file's parametric EQ are played and edited by Lemon Parametric EQ, started
    /// from the slot's saved state (spec: plugins/lemon-parametric-eq-2.md §8). Without the plugin the engine's own EQ
    /// plays them.
    pub(crate) fn sync_effects(&mut self) {
        use crate::plugins::{fx_key, EQ_FILE_NAME, LEMON_EQ};
        // The installed EQ plays these slots instead (`host_native`).
        if crate::project_plugins::native_first() {
            return;
        }
        let Some(index) = self.plugins.effect_index(LEMON_EQ) else { return };
        let todo: Vec<(u16, u8, Vec<u8>)> = self
            .state
            .project
            .mixer
            .iter()
            .flat_map(|m| m.slots.iter().enumerate().map(move |(k, s)| (m.index, k as u8, s)))
            .filter_map(|(t, k, s)| s.as_ref().filter(|s| s.name == EQ_FILE_NAME).map(|s| (t, k, s.state.clone())))
            .filter(|&(t, k, _)| !self.plugins.hosted.contains_key(&fx_key(t, k)) && !self.fx_failed.contains(&fx_key(t, k)))
            .collect();
        if todo.is_empty() {
            return;
        }
        for (t, k, fl) in todo {
            let st = lemon_parametric_eq_2::params::plugin_state_from_fl(&fl);
            if let Err(e) = self.plugins.create_effect(t, k, index, Some(&st)) {
                self.fx_failed.push(fx_key(t, k));
                self.state.message = Some(format!("{LEMON_EQ}: {e}"));
            }
        }
        self.transport.set_effects(self.plugins.effects());
        self.state.piano.dirty = true;
    }

    /// Plugin host work on the UI thread: create / open requested plugins, apply plugin-window edits to the channel.
    pub(crate) fn plugin_tick(&mut self) {
        // Rows gone without the app (an undone drop that made a channel, `rack::remove_last`): their instruments go.
        let len = self.state.project.channels.len();
        if self.plugins.hosted.range(len..crate::plugins::FX_KEY).next().is_some() {
            self.remap_rows(&|r| (r < len).then_some(r));
        }
        self.scan_tick();
        self.loading_tick();
        self.export_tick();
        self.midi_tick();
        self.mixer_tick();
        self.sync_effects();
        self.params_tick();
        if self.loading.is_none() {
            self.sync_wrapped();
            self.sync_osc3();
            self.sync_native();
        }
        for (row, path) in std::mem::take(&mut self.state.sample_loads) {
            if !self.state.hosted_rows.contains(&row) {
                continue;
            }
            // The root note and stretch settings the load may have read from the file (`rack::read_tags`) go with it.
            let set = self.state.project.channels.get(row).map(crate::plugins::tag_params).unwrap_or_default();
            // A row hosted this tick already started from its channel's sample.
            let new = self.plugins.sampler_path(row).as_deref() != Some(path.as_str());
            if !self.plugins.set_sampler(row, new.then_some(path.as_str()), &set) {
                self.state.message = Some(format!("Lemon Sampler did not take {path}"));
                self.invalidate();
            }
        }
        // A native generator waits for its first check in a child process (`Plugins::instrument_ready`).
        let pending = self.state.pending_plugin;
        if let Some((_, index)) = pending.filter(|&(_, index)| !self.plugins.instrument_ready(index)) {
            let name = self.plugins.available.get(index).map(|a| a.desc.name.clone()).unwrap_or_default();
            let msg = format!("Trying {name} before its first load...");
            if self.state.message.as_deref() != Some(msg.as_str()) {
                self.state.message = Some(msg);
                self.invalidate();
            }
        } else if let Some((row, index)) = pending {
            self.state.pending_plugin = None;
            match self.plugins.create(row, index) {
                Ok(name) => {
                    self.plugins.added(row);
                    self.state.hosted_rows.push(row);
                    self.transport.set_instruments(self.plugins.instruments());
                    self.state.piano.dirty = true;
                    let format = self.plugins.descriptor(row).map_or("", |d| crate::plugins::format_name(d.format));
                    self.state.message = Some(format!("Added {name} (hosted {format} plugin)"));
                    self.state.pending_open = Some(row);
                }
                Err(e) => self.state.message = Some(format!("Plugin: {e}")),
            }
            self.invalidate();
        }
        // Without plugin windows opening one says so.
        #[cfg(not(plugin_windows))]
        if self.state.pending_open.take().is_some_and(|r| self.state.hosted_rows.contains(&r)) {
            self.state.message = Some(off_text(PLUGIN_WINDOWS));
            self.invalidate();
        }
        #[cfg(plugin_windows)]
        if let Some(row) = self.state.pending_open.take().filter(|r| self.state.hosted_rows.contains(r)) {
            if let Some(info) = channel_info(&self.state.project, row) {
                match self.plugins.open_window(row, info) {
                    Ok(()) => {
                        self.state.raise(Win::Hosted(row));
                        self.select_window_track(row);
                    }
                    Err(e) => {
                        self.state.message = Some(format!("Plugin: {e}"));
                        self.invalidate();
                    }
                }
            }
        }
        // Plugin housekeeping on every platform (callbacks, parameter flushes, restarts, rate changes); the plugin
        // windows' requests come with it.
        #[cfg(plugin_windows)]
        let requests = {
            let (project, mixer) = (&self.state.project, &self.state.mixer);
            self.plugins.idle(|row| if row >= crate::plugins::FX_KEY { crate::mixer::fx_info(project, mixer, row) } else { channel_info(project, row) })
        };
        // No plugin windows here, so no requests.
        #[cfg(not(plugin_windows))]
        self.plugins.idle(|_| None);
        // An open instrument editor is processed while nothing plays, so notes from its own keyboard sound.
        let listen = self.plugins.editor_open();
        if listen {
            self.sync_live();
        }
        self.transport.listen(listen);
        // Keys pressed while a plugin window has the focus still play the typing keyboard.
        #[cfg(any(windows, target_os = "macos"))]
        for k in crate::plugin_window::take_keys() {
            use winit::platform::scancode::PhysicalKeyExtScancode;
            if let winit::keyboard::PhysicalKey::Code(code) = winit::keyboard::PhysicalKey::from_scancode(k.scancode) {
                // Space keeps playing / stopping the song (the typing keyboard does not use it).
                if !self.typing_key(code, k.pressed, k.ctrl, k.alt) && k.pressed && code == winit::keyboard::KeyCode::Space {
                    self.key(&winit::keyboard::Key::Named(winit::keyboard::NamedKey::Space), k.ctrl, k.alt);
                }
            }
        }
        #[cfg(plugin_windows)]
        {
            for r in requests {
                use crate::plugins::Request;
                let chans = &mut self.state.project.channels;
                match r {
                    Request::Close(_) => continue,
                    Request::Raise(row) => {
                        // A click in a plugin window closes an open app popup, as a click elsewhere does.
                        if self.state.playlist.popup.take().is_some() {
                            self.invalidate();
                        }
                        self.state.raise(Win::Hosted(row));
                        self.select_window_track(row);
                        continue;
                    }
                    // A native Wayland main window (Linux): the popups cannot be lifted above a plugin window there.
                    #[cfg(not(windows))]
                    Request::Menu(..) | Request::Presets(..) | Request::MiscMenu(..) if self.plugins.owner == 0 => {
                        self.state.message = Some(off_text(PLUGIN_POPUPS));
                        self.invalidate();
                        continue;
                    }
                    Request::Menu(row, x, y) => {
                        let Some((_, detached)) = self.plugins.window_of(row) else { continue };
                        let (lx, ly) = self.screen_to_canvas(x, y);
                        let mut menu = crate::playlist::plugin_window_menu(row, detached, lx, ly);
                        crate::presets::fill_window_menu(self, row, &mut menu);
                        self.state.playlist.popup = Some(menu);
                        self.popup_lift = detached.then_some(row);
                        self.invalidate();
                        continue;
                    }
                    Request::Presets(row, x, y) => {
                        let Some((_, detached)) = self.plugins.window_of(row) else { continue };
                        let (lx, ly) = self.screen_to_canvas(x, y);
                        self.state.playlist.popup = Some(crate::presets::popup(self, row, lx, ly));
                        self.popup_lift = detached.then_some(row);
                        self.invalidate();
                        continue;
                    }
                    Request::PresetStep(row, d) => {
                        self.state.message = crate::presets::run(self, crate::presets::PCmd::Step(row, d));
                        continue;
                    }
                    Request::Misc(row, m, v) => {
                        crate::misc::set(self, row, m, v);
                        continue;
                    }
                    Request::MiscMenu(row, m, x, y) => {
                        let Some((_, detached)) = self.plugins.window_of(row) else { continue };
                        let (lx, ly) = self.screen_to_canvas(x, y);
                        self.state.playlist.popup = Some(crate::misc::menu(self, row, m, lx, ly));
                        self.popup_lift = detached.then_some(row);
                        self.invalidate();
                        continue;
                    }
                    Request::ToggleOn(row) => chans.get_mut(row).into_iter().for_each(|c| c.enabled = !c.enabled),
                    Request::Volume(row, v) => chans.get_mut(row).into_iter().for_each(|c| c.volume = v),
                    Request::Pan(row, v) => chans.get_mut(row).into_iter().for_each(|c| c.pan = v),
                    Request::PitchCents(row, v) => chans.get_mut(row).into_iter().for_each(|c| c.pitch_cents = v),
                    Request::Note(row, key, vel, on) => {
                        // The plugin's own keyboard plays on the live stream, like the typing keyboard.
                        if on {
                            self.sync_live();
                            self.transport.note_on(row, u16::from(key), vel);
                        } else {
                            self.transport.note_off(row, u16::from(key));
                        }
                        continue;
                    }
                    Request::Automate(row, index) => {
                        self.state.message = self.params_automate(row, index);
                        self.invalidate();
                        continue;
                    }
                    Request::NotAvailable(name) => {
                        self.state.message = Some(off_text(name));
                        self.invalidate();
                        continue;
                    }
                    Request::Stopped(_, text) => {
                        self.state.message = Some(text);
                        self.invalidate();
                        continue;
                    }
                }
                self.state.piano.dirty = true;
                self.invalidate();
            }
            self.clip_docked();
        }
    }

    /// Screen point -> canvas logical px (an X11 main window: Linux; macOS).
    #[cfg(all(plugin_windows, not(windows)))]
    fn screen_to_canvas(&self, x: i32, y: i32) -> (f32, f32) {
        let (x, y) = crate::plugin_window::screen_to_main(x, y).unwrap_or((x, y));
        let k = self.plugins.scale;
        ((x - self.canvas_off.0) as f32 / k, (y - self.canvas_off.1) as f32 / k)
    }

    /// Screen point -> canvas logical px.
    #[cfg(windows)]
    fn screen_to_canvas(&self, x: i32, y: i32) -> (f32, f32) {
        let mut p = windows_sys::Win32::Foundation::POINT { x, y };
        // SAFETY: plain coordinate conversion against our own window.
        unsafe { windows_sys::Win32::Graphics::Gdi::ScreenToClient(self.plugins.owner as _, &mut p) };
        let k = self.plugins.scale;
        ((p.x - self.canvas_off.0) as f32 / k, (p.y - self.canvas_off.1) as f32 / k)
    }

    /// Linux and macOS: mouse input on a lifted popup (`sync_lift`) goes to the app as if it happened in the main window.
    #[cfg(all(plugin_windows, not(windows)))]
    fn lift_input(&mut self) {
        let input = crate::plugin_window::take_lift_input();
        let k = self.plugins.scale;
        let (ox, oy) = self.canvas_off;
        for i in &input {
            match *i {
                crate::plugin_window::LiftInput::Move(x, y) => {
                    self.mouse_move((x - ox) as f32 / k, (y - oy) as f32 / k);
                }
                crate::plugin_window::LiftInput::Down(1) => self.mouse_down(),
                crate::plugin_window::LiftInput::Down(2) => self.mouse_down_middle(),
                crate::plugin_window::LiftInput::Down(3) => self.mouse_down_right(),
                crate::plugin_window::LiftInput::Down(_) => {}
                crate::plugin_window::LiftInput::Up => self.mouse_up(),
            }
        }
    }

    /// After a paint: a popup opened from a detached plugin window is shown above that window (it is painted in the
    /// main window, which the detached window covers).
    #[cfg(plugin_windows)]
    pub fn sync_lift(&mut self, canvas: &Canvas) {
        let lifted = self.popup_lift.filter(|_| self.state.playlist.popup.is_some()).and_then(|r| self.plugins.window_of(r));
        let bounds = self.overlays.iter().copied().reduce(|a, b| a.union(&b));
        match (lifted, bounds) {
            (Some((frame, true)), Some(b)) => {
                let (x, y, w, h) = canvas.device_rect(b);
                let stride = canvas.pixmap.width() as usize;
                let px = canvas.pixmap.pixels();
                let mut buf = Vec::with_capacity(w * h);
                for row in y..y + h {
                    for p in &px[row * stride + x..row * stride + x + w] {
                        buf.push(u32::from(p.red()) << 16 | u32::from(p.green()) << 8 | u32::from(p.blue()));
                    }
                }
                let origin = (x as i32 + self.canvas_off.0, y as i32 + self.canvas_off.1);
                crate::plugin_window::lift_show(self.plugins.owner, frame, origin, (w as i32, h as i32), buf);
            }
            _ => {
                self.popup_lift = None;
                crate::plugin_window::lift_hide();
            }
        }
    }

    /// Docked plugin windows (native children) join `State::z` and are clipped to the workspace minus the drawn
    /// windows above them and the overlays (spec: ui/plugin-window.md "Docking"). Detached windows leave `State::z`.
    #[cfg(plugin_windows)]
    fn clip_docked(&mut self) {
        let docked = self.plugins.docked();
        self.state.z.retain(|w| !matches!(w, Win::Hosted(r) if !docked.contains(r)));
        for &r in &docked {
            if !self.state.z.contains(&Win::Hosted(r)) {
                self.state.z.push(Win::Hosted(r));
            }
        }
        let k = self.plugins.scale;
        let (ox, oy) = self.canvas_off;
        let px = |r: Rect| [(r.x * k).floor() as i32 + ox, (r.y * k).floor() as i32 + oy, (r.right() * k).ceil() as i32 + ox, (r.bottom() * k).ceil() as i32 + oy];
        let work = px(self.state.work_rect());
        let over: Vec<[i32; 4]> = self.overlays.iter().map(|&r| px(r.outset(2.0))).collect();
        let z = self.state.z.clone();
        let stacked: Vec<usize> = z.iter().filter_map(|w| if let Win::Hosted(r) = w { Some(*r) } else { None }).collect();
        self.plugins.stack(&stacked);
        let docked_at = z.iter().rposition(|&w| self.state.is_max(w));
        for (i, &w) in z.iter().enumerate() {
            let Win::Hosted(row) = w else { continue };
            let mut covers = over.clone();
            for &a in &z[i + 1..] {
                if !matches!(a, Win::Hosted(_)) {
                    covers.push(px(self.win_bounds(a)));
                }
            }
            // Below a maximized window: covered entirely.
            if docked_at.is_some_and(|d| i < d) {
                covers.push(work);
            }
            self.plugins.clip(row, work, &covers);
        }
    }

    /// Paint the whole frame.
    /// Mark everything dirty (state changed in a way that can affect any view).
    pub fn invalidate(&mut self) {
        self.damage = Damage::Full;
    }

    pub(crate) fn damage_rect(&mut self, r: Rect) {
        if r.is_empty() {
            return;
        }
        match &mut self.damage {
            Damage::Full => {}
            Damage::None => self.damage = Damage::Rects(vec![r]),
            Damage::Rects(v) => add_damage(v, r),
        }
    }

    /// Whether a paint is due: damage, or a playlist scroll waiting to move its pixels (`playlist_scrolled` adds no
    /// damage of its own; without this a scroll-bar drag or middle-drag grab showed nothing until the release).
    pub fn has_damage(&self) -> bool {
        self.damage != Damage::None || self.scrolled.is_some()
    }

    /// Take the pending damage: `None` = nothing to paint, `Some(None)` = whole frame, else the regions to repaint
    /// (all in one `paint_region`).
    /// A playlist scroll is resolved here (`resolve_scroll`): call it right before `paint_region`.
    pub fn take_damage(&mut self) -> Option<Option<Vec<Rect>>> {
        // An input's paint can come before the next tick (Windows paints before `about_to_wait`): what follows the
        // settings is brought up to date first, so the frame shows it.
        self.follow_settings();
        self.resolve_scroll();
        match std::mem::replace(&mut self.damage, Damage::None) {
            Damage::None => None,
            Damage::Full => Some(None),
            Damage::Rects(mut v) => {
                // The Toolbars menu (spec: ui/toolbar-menu.md §2, §4): without separators the rows' pixels move after
                // painting, so the toolbar repaints whole; the floating hint bar repeats the hint panel.
                let o = self.state.toolbar;
                let band = views::toolbar::band(self.state.screen.0);
                if !o.separators && !o.hidden && v.iter().any(|r| !r.intersect(&band).is_empty()) {
                    add_damage(&mut v, band);
                }
                if o.hint_bar && v.iter().any(|r| !r.intersect(&HINT_RECT).is_empty()) {
                    add_damage(&mut v, views::toolbar::hint_bar_rect(&self.state));
                }
                Some(Some(v))
            }
        }
    }

    /// The areas the last paint moved instead of repainting (a playlist scroll; `os_ui::shift_area` rounds the area,
    /// `os_ui::keep_area` the kept rects): the shell moves the same pixels of its window buffer and presents them.
    pub fn take_moved(&mut self) -> Vec<Moved> {
        std::mem::take(&mut self.moved)
    }

    /// The smallest playlist scroll step whose drawn pixels can move (`blit_step` at the last paint's scale).
    pub(crate) fn playlist_blit_step(&self) -> Option<f32> {
        blit_step(self.px_scale)
    }

    /// The playlist view scrolled from `before` (its horizontal and vertical scroll). The next paint moves the pixels
    /// already drawn for it instead of repainting the window when that gives exactly what a repaint would
    /// (`resolve_scroll`). Damage waiting inside the moving parts moves along: those pixels are stale wherever they go.
    pub(crate) fn playlist_scrolled(&mut self, before: (f32, f32)) {
        let now = self.state.playlist_scroll;
        let d = (f64::from(before.0) - f64::from(now.0), f64::from(before.1) - f64::from(now.1));
        if d == (0.0, 0.0) {
            return;
        }
        if let Damage::Rects(v) = &self.damage {
            let stale: Vec<Rect> = self
                .scroll_moves(d)
                .into_iter()
                .flat_map(|(r, sx, sy)| v.iter().map(move |o| moved_by(o.intersect(&r), sx, sy).outset(1.0).intersect(&r)))
                .collect();
            for r in stale {
                self.damage_rect(r);
            }
        }
        let s = self.scrolled.get_or_insert((0.0, 0.0));
        *s = (s.0 + d.0, s.1 + d.1);
    }

    /// The playlist parts a scroll by `d` (logical px) moves, with their move: the grid both ways, the ruler and
    /// marker lane sideways, the track headers up and down (as laid out by the last paint).
    fn scroll_moves(&self, d: (f64, f64)) -> Vec<(Rect, f64, f64)> {
        let Some(grid) = self.pl_grid else { return Vec::new() };
        let p = views::playlist::scroll_parts(&self.state, grid);
        let mut v = vec![(p.grid, d.0, d.1)];
        if d.0 != 0.0 {
            v.push((p.ruler, d.0, 0.0));
            v.extend(p.lane.map(|l| (l, d.0, 0.0)));
        }
        if d.1 != 0.0 {
            v.push((p.heads, 0.0, d.1));
        }
        v
    }

    /// What is drawn over the playlist: the drawn windows above it and the toolbar.
    fn playlist_covers(&self) -> Vec<Cover> {
        let s = &self.state;
        let mut v: Vec<_> = s
            .z
            .iter()
            .skip_while(|&&w| w != Win::Playlist)
            .skip(1)
            .filter(|w| matches!(w, Win::Mixer | Win::PianoRoll | Win::ChannelRack | Win::Plugin | Win::Perf | Win::Tapper))
            .map(|&w| Cover { bounds: self.win_bounds(w).outset(1.0), win: Some(w), solid: self.win_solid(w) })
            .collect();
        if !s.toolbar.hidden {
            v.push(Cover { bounds: Rect::new(0.0, 0.0, s.screen.0, views::WORK_Y + 1.0), win: None, solid: None });
        }
        // The floating hint bar (spec: ui/toolbar-menu.md §4).
        if s.toolbar.hint_bar {
            v.push(Cover { bounds: views::toolbar::hint_bar_rect(s).outset(1.0), win: None, solid: None });
        }
        v
    }

    /// The part of window `w` its frame fills with opaque colour first, when it floats: the pixels there depend
    /// on the window alone, not on what is under it.
    fn win_solid(&self, w: Win) -> Option<Rect> {
        let s = &self.state;
        let r = match w {
            Win::ChannelRack => Some(views::rack::rect(s)),
            Win::Plugin => s.project.channels.get(s.plugin_channel).map(|_| views::channel_win::rect(s)),
            Win::Perf => Some(views::perf::rect(s)),
            Win::Tapper => Some(views::tapper::rect(s)),
            w => {
                let ws = &s.wins[w.slot()?];
                let r = s.win_rect(w);
                (!ws.maxed && !ws.mini && r.h >= 2.0 * TITLE_H).then_some(r)
            }
        }?;
        // Rounded frames (Lemon, spec: ui/lemon-design.md §3): the corners show what is under the window. Leaving out
        // a radius-wide band at the left and right leaves out all four corners.
        let st = self.theme.colors.style;
        let k = st.radius_panel;
        Some(if st.lemon() { Rect::new(r.x + k, r.y, (r.w - 2.0 * k).max(0.0), r.h) } else { r })
    }

    /// Before a paint: turn the pending playlist scroll into canvas moves (`shifts`) and the damage they leave: the
    /// strips scrolled into view, the scroll bars, what covers the playlist (where it is and where its pixels moved
    /// to; a window that did not change keeps the pixels inside its frame, so only its edge and the content it
    /// uncovers paint), the hovered control. The window is repainted instead when moving would not equal a repaint: a
    /// distance off the drawing grid (`PGeom::xs64`) or not whole device pixels, a window moved or resized since the
    /// last paint, an open popup or menu, a drag other than scrolling, a progress panel, the monitor debug panel.
    fn resolve_scroll(&mut self) {
        self.shifts.clear();
        let Some(d) = self.scrolled.take() else { return };
        if self.damage == Damage::Full {
            return;
        }
        let k = f64::from(self.px_scale);
        let exact = |v: f64| (v * 256.0).fract() == 0.0 && (v * k).fract() == 0.0;
        let can = exact(d.0)
            && exact(d.1)
            && self.pl_grid.is_some()
            && views::playlist::grid_of(&self.state) == self.pl_grid
            && self.overlays.is_empty()
            && matches!(self.drag, None | Some(Drag::Scroll { .. }))
            && self.loading.is_none()
            && self.export.is_none()
            && self.state.playlist.overlay.is_none()
            && !self.state.monitor.debug;
        if !can {
            return self.damage_win(Win::Playlist);
        }
        let grid = self.pl_grid.expect("checked");
        // A window that is where it was at the last paint keeps the pixels its frame fills; around them it is
        // painted where it draws (`views::paint_area`: its resize zones are hits only) and where those pixels moved.
        // A window that moved or changed is painted where it was and where it is, with all it may paint.
        let now = self.playlist_covers();
        let mut covers: Vec<(Rect, Option<Rect>)> = now
            .iter()
            .map(|c| match (c.win, c.solid) {
                (Some(w), Some(solid)) if self.pl_covers.contains(c) => (views::paint_area(&self.state, w), Some(solid)),
                _ => (c.bounds, None),
            })
            .collect();
        covers.extend(self.pl_covers.iter().filter(|c| !now.contains(c)).map(|c| (c.bounds, None)));
        for (r, sx, sy) in self.scroll_moves(d) {
            if sx.abs() >= f64::from(r.w) || sy.abs() >= f64::from(r.h) {
                self.damage_rect(r);
                continue;
            }
            // What the move leaves to paint, but for the kept pixels: those show the same as before. (Damage
            // waiting from before stays whole: it is what changed.)
            let mut paint = Vec::new();
            // Covers are painted where they are; the pixels of them that moved are content's.
            for &(c, _) in &covers {
                paint.push(c.intersect(&r));
                paint.push(moved_by(c, sx, sy).intersect(&r));
            }
            let keep: Vec<Rect> = covers.iter().filter_map(|c| c.1).map(|s| s.intersect(&r)).filter(|k| !k.is_empty()).collect();
            self.shifts.push(Moved { area: r, dx: (sx * k) as i32, dy: (sy * k) as i32, keep: keep.clone() });
            let (sx, sy) = (sx as f32, sy as f32);
            // The strips scrolled into view (damage reaches out to whole device pixels, past the moved area's rounded edge).
            if sx != 0.0 {
                let x = if sx > 0.0 { r.x } else { r.right() + sx };
                paint.push(Rect::new(x, r.y, sx.abs(), r.h));
            }
            if sy != 0.0 {
                let y = if sy > 0.0 { r.y } else { r.bottom() + sy };
                paint.push(Rect::new(r.x, y, r.w, sy.abs()));
            }
            // The hovered control's look moved off the mouse.
            if let (Some(a), Some(h)) = (&self.hover, self.hover_rect) {
                if hover_is_visible(a) {
                    paint.push(moved_by(h.outset(3.0), f64::from(sx), f64::from(sy)).intersect(&r));
                }
            }
            for k in &keep {
                let kept = os_ui::inner_rect(*k, self.px_scale);
                paint = paint.into_iter().flat_map(|p| rect_minus(p, kept)).collect();
            }
            for p in paint {
                self.damage_rect(p);
            }
        }
        let parts = views::playlist::scroll_parts(&self.state, grid);
        self.damage_rect(parts.overview);
        self.damage_rect(parts.vbar);
    }

    /// Repaint window `w` only (with its resize zones), or everything when it is not shown.
    // ---------------------------------------------------------------- CPU panel and Plugin performance monitor

    /// The CPU panel's right-click menu (spec: ui/main-window.md §13).
    fn cpu_menu(&mut self, x: f32, y: f32) {
        use crate::cpu::{CpuCmd, Refresh};
        use crate::playlist::{Cmd, Mark, PItem, Popup};
        let c = &self.state.cpu;
        let refresh = Refresh::ALL.iter().map(|&r| PItem::new(r.label(), Cmd::Cpu(CpuCmd::Refresh(r))).mark(Mark::Radio(c.refresh == r))).collect();
        let items = vec![
            PItem::sub("Refresh", refresh),
            PItem::sep(),
            PItem::new("View voice count", Cmd::Cpu(CpuCmd::Fps(false))).mark(Mark::Radio(!c.fps_view)),
            PItem::new("View FPS", Cmd::Cpu(CpuCmd::Fps(true))).mark(Mark::Radio(c.fps_view)),
            PItem::new("Misc", Cmd::Cpu(CpuCmd::Misc)).mark(Mark::Check(c.misc)),
            PItem::sep(),
            PItem::new("Plugin performance monitor", Cmd::Cpu(CpuCmd::Monitor)).mark(Mark::Check(self.state.shown(Win::Perf))),
            PItem::new("Audio settings", Cmd::Cpu(CpuCmd::AudioSettings)),
        ];
        self.state.playlist.popup = Some(Popup { x, y, items, sub: None });
    }

    /// The metronome button's right-click menu: the sound (spec: ui/metronome.md §2).
    fn metronome_menu(&mut self, x: f32, y: f32) {
        use crate::playlist::{Cmd, Mark, PItem, Popup};
        use os_engine::metronome::Sound;
        let now = crate::metronome::sound();
        let mut items = vec![PItem::head("Metronome sound")];
        items.extend(Sound::ALL.iter().map(|&s| PItem::new(s.label(), Cmd::MetronomeSound(s)).mark(Mark::Radio(s == now))));
        self.state.playlist.popup = Some(Popup { x, y, items, sub: None });
    }

    /// Metronome switch (toolbar, OPTIONS menu, Ctrl+M): heard from the next beat, also while playing.
    pub fn toggle_metronome(&mut self) {
        let on = crate::metronome::toggle();
        self.state.message = Some(if on { "Metronome on" } else { "Metronome off" }.into());
        self.invalidate();
    }

    /// The auto-scrolling button's right-click menu (spec: ui/auto-scrolling.md §2).
    fn autoscroll_menu(&mut self, x: f32, y: f32) {
        use crate::playlist::{Cmd, Mark, PItem, Popup};
        let items = vec![PItem::new("Continuous scrolling", Cmd::ContinuousScrolling).mark(Mark::Check(crate::autoscroll::continuous()))];
        self.state.playlist.popup = Some(Popup { x, y, items, sub: None });
    }

    /// Auto scrolling switch (toolbar, OPTIONS menu): followed from the next move of the playhead.
    pub fn toggle_autoscroll(&mut self) {
        let on = crate::autoscroll::toggle();
        self.state.message = Some(if on { "Auto scrolling on" } else { "Auto scrolling off" }.into());
        self.invalidate();
    }

    /// Auto scrolling (spec: ui/auto-scrolling.md §5): the playlist (song) or the piano roll (pattern) scrolls so the
    /// shown playing position stays in view. Left alone while the mouse drags a scroll bar.
    fn auto_scroll(&mut self) {
        let set = crate::autoscroll::get();
        let now = &self.shown;
        if !set.on || !now.playing || now.paused || matches!(self.drag, Some(Drag::Scroll { .. } | Drag::BarZoom { .. } | Drag::BarRows { .. })) {
            return;
        }
        let tick = now.tick as f32;
        let ppq = f32::from(self.state.project.ppq.max(4));
        if now.song {
            let Some(g) = self.pl_grid.filter(|_| self.state.shown(Win::Playlist)) else { return };
            let left = self.state.playlist_scroll.0;
            let x = tick * self.state.playlist_bar_w / (ppq * 4.0);
            if let Some(v) = crate::autoscroll::follow(left, g.w, x, set.continuous) {
                // In whole steps the drawn pixels can move by (`playlist_scrolled`).
                let v = blit_step(self.px_scale).map_or(v, |q| left + ((v - left) / q).round() * q);
                let before = self.state.playlist_scroll;
                self.scroll_set(Scroll::PlaylistX, v);
                self.playlist_scrolled(before);
            }
        } else {
            let Some(g) = self.piano_grid.filter(|_| self.state.shown(Win::PianoRoll)) else { return };
            let left = self.scroll_get(Scroll::PianoX);
            let x = tick * self.state.piano.px_per_beat / ppq;
            if let Some(v) = crate::autoscroll::follow(left, g.w, x, set.continuous) {
                self.scroll_set(Scroll::PianoX, v);
                if self.scroll_get(Scroll::PianoX) != left {
                    self.damage_win(Win::PianoRoll);
                }
            }
        }
    }

    /// A command of the CPU panel's menu.
    pub(crate) fn cpu_cmd(&mut self, c: crate::cpu::CpuCmd) -> Option<String> {
        use crate::cpu::CpuCmd;
        match c {
            CpuCmd::Monitor => self.toggle_perf(),
            CpuCmd::AudioSettings => self.open_settings(views::settings::AUDIO),
            c => self.state.cpu.set(c),
        }
        self.invalidate();
        None
    }

    /// Opens the Plugin performance monitor (or brings it to the front), in the middle of the workspace the first
    /// time (spec: ui/plugin-performance.md §1).
    fn open_perf(&mut self) {
        let s = &mut self.state;
        if !s.perf.placed {
            let w = s.work_rect();
            let (pw, ph) = views::perf::SIZE;
            s.perf.pos = ((w.x + (w.w - pw) / 2.0).max(0.0).round(), (w.y + (w.h - ph) / 2.0).max(views::work_y(s)).round());
            s.perf.placed = true;
        }
        if !s.shown(Win::Perf) {
            s.perf.reset();
            let src = self.perf_sources();
            self.state.perf.read(std::time::Instant::now(), src);
        }
        self.state.raise(Win::Perf);
        self.invalidate();
    }

    /// VIEW > Plugin performance monitor and the panel menu's entry: open it, or close it when it is in front.
    fn toggle_perf(&mut self) {
        if self.state.shown(Win::Perf) && self.state.in_front(Win::Perf) {
            self.close_perf();
        } else {
            self.open_perf();
        }
    }

    /// Closing the monitor resets its measurements.
    fn close_perf(&mut self) {
        self.state.hide(Win::Perf);
        self.state.perf.reset();
        self.state.perf.finding = false;
        self.invalidate();
    }

    /// VIEW > Close all windows (F12), Close all unfocused windows (Ctrl+F12), Close all plugin windows (Alt+F12)
    /// (spec: ui/close-windows.md). The browser is a panel and stays.
    pub(crate) fn close_windows(&mut self, what: CloseAll) {
        let plugin = |w: Win| matches!(w, Win::Plugin | Win::Hosted(_));
        // The focused window is the one in front, unless the empty workspace took the last click.
        let keep = match what {
            CloseAll::Unfocused if !self.state.no_focus => self.state.z.last().copied(),
            _ => None,
        };
        let closing: Vec<Win> = self.state.z.iter().copied().filter(|&w| Some(w) != keep && (what != CloseAll::Plugins || plugin(w))).collect();
        for w in closing {
            match w {
                Win::Perf => self.close_perf(),
                w => self.state.hide(w),
            }
        }
        // Hosted plugin windows (rack rows and mixer slots; detached ones are not in the stack).
        let rows: Vec<usize> = self.plugins.hosted.keys().copied().collect();
        for row in rows.into_iter().filter(|&r| keep != Some(Win::Hosted(r))) {
            self.plugins.close_if_open(row);
        }
        self.invalidate();
    }

    /// Every hosted instance for the monitor, rack rows then mixer slots.
    fn perf_sources(&self) -> Vec<crate::perf::Source> {
        let p = &self.state.project;
        self.plugins
            .hosted
            .iter()
            .map(|(&key, h)| {
                let (synth, place) = match crate::plugins::fx_of(key) {
                    None => (true, p.channels.get(key).map_or_else(|| format!("Channel {}", key + 1), |c| c.name.clone().unwrap_or_else(|| format!("Channel {}", c.index + 1)))),
                    Some((0, slot)) => (false, format!("Master, slot {}", slot + 1)),
                    Some((track, slot)) => (false, format!("Insert {track}, slot {}", slot + 1)),
                };
                crate::perf::Source { key, name: h.name.clone(), patch: self.plugins.preset_name(key), place, synth, stopped: h.stopped(), meter: h.meter.clone() }
            })
            .collect()
    }

    fn perf_act(&mut self, a: crate::perf::PerfAct) {
        use crate::perf::PerfAct;
        let p = &mut self.state.perf;
        match a {
            PerfAct::Sort(c) => p.sort_by(c),
            PerfAct::ResetOnTransport => p.reset_on_transport = !p.reset_on_transport,
            PerfAct::Freeze => p.freeze = !p.freeze,
            PerfAct::Find => p.finding = true,
            PerfAct::List => {}
            PerfAct::Row(i) => {
                let Some(key) = p.shown().get(i).map(|r| r.key) else { return };
                let now = std::time::Instant::now();
                let double = p.last_click.is_some_and(|(k, t)| k == key && now.duration_since(t).as_millis() < 400);
                p.last_click = if double { None } else { Some((key, now)) };
                if double {
                    // The channel's plugin window, or the mixer slot's.
                    match crate::plugins::fx_of(key) {
                        Some((track, slot)) => self.open_fx(track, slot),
                        None => self.state.pending_open = Some(key),
                    }
                }
            }
        }
        self.damage_win(Win::Perf);
    }

    fn perf_wheel(&mut self, notches: f32) {
        let fit = views::perf::rows_fit(&self.state);
        let p = &mut self.state.perf;
        let max = p.shown().len().saturating_sub(fit);
        p.scroll = (p.scroll.min(max) as f32 - (notches * 3.0).round()).clamp(0.0, max as f32) as usize;
    }

    /// Typing into the monitor's Find box. True when the key was taken.
    fn perf_key(&mut self, key: &winit::keyboard::Key, ctrl: bool) -> bool {
        use winit::keyboard::{Key, NamedKey};
        let p = &mut self.state.perf;
        match key {
            Key::Named(NamedKey::Escape) => {
                p.find.clear();
                p.finding = false;
            }
            Key::Named(NamedKey::Enter) => p.finding = false,
            Key::Named(NamedKey::Backspace) => {
                p.find.pop();
            }
            Key::Named(NamedKey::Space) => p.find.push(' '),
            Key::Character(c) if !ctrl => p.find.push_str(c),
            _ => return false,
        }
        p.scroll = 0;
        self.damage_win(Win::Perf);
        true
    }

    /// Play and Stop clear the monitor's peaks when Reset on transport is ticked.
    fn perf_transport(&mut self) {
        if self.state.perf.reset_on_transport {
            self.state.perf.reset_peaks();
            self.damage_win(Win::Perf);
        }
    }

    // ---------------------------------------------------------------- Tempo tapper (spec: ui/tempo-tapper.md)

    /// VIEW > Tempo tapper and Ctrl+F5: open it (or bring it to the front), centred over the main window the first
    /// time in a run and then where it was left; close it when it is in front (§1, §8).
    fn toggle_tapper(&mut self) {
        let s = &mut self.state;
        if s.shown(Win::Tapper) && s.in_front(Win::Tapper) {
            s.hide(Win::Tapper);
        } else {
            if !s.tapper.placed {
                let (w, h) = views::tapper::SIZE;
                s.tapper.pos = (((s.screen.0 - w) / 2.0).max(0.0).round(), ((s.screen.1 - h) / 2.0).max(views::work_y(s)).round());
                s.tapper.placed = true;
            }
            s.raise(Win::Tapper);
        }
        self.invalidate();
    }

    fn tapper_act(&mut self, a: crate::tapper::TapAct) {
        use crate::tapper::TapAct;
        match a {
            TapAct::Pad => {
                let t = self.state.tapper.clock();
                self.tap(t);
            }
            // Turning Apply tempo on writes the tempo on the pad to the project at once (§5).
            TapAct::Apply => {
                if crate::tapper::toggle_apply() {
                    if let Some(bpm) = self.state.tapper.count.tempo() {
                        self.set_tempo(bpm);
                    }
                }
            }
        }
        self.damage_win(Win::Tapper);
    }

    /// A tap at `t` seconds on the tapper's clock: it counts (§2), lights the pad (§4), clicks (the metronome's Tick,
    /// accented on the taps the beat bar shows orange, §8) and, with Apply tempo on, a new tempo becomes the project
    /// tempo (§5).
    fn tap(&mut self, t: f64) {
        let tp = &mut self.state.tapper;
        let bpm = tp.count.tap(t);
        tp.flash_at = Some(std::time::Instant::now());
        tp.flash = 1.0;
        let accent = tp.count.bar().is_some_and(|(_, accent)| accent);
        #[cfg(test)]
        tp.test_clicks.push(accent);
        #[cfg(not(test))]
        self.transport.tap_click(accent);
        if let Some(bpm) = bpm.filter(|_| crate::tapper::get().apply) {
            self.set_tempo(bpm);
        }
        self.damage_win(Win::Tapper);
    }

    /// A key on a MIDI keyboard taps while the tapper is open (its pad's hint, §1); the keys of a chord tap once.
    fn midi_tap(&mut self) {
        let t = self.state.tapper.clock();
        if !self.state.tapper.count.last().is_some_and(|l| t - l < crate::tapper::MIDI_CHORD) {
            self.tap(t);
        }
    }

    /// Sets the project tempo, kept in the tempo box's range (10-522 BPM): playback follows from the next tick, the
    /// toolbar shows it. Returns the tempo set.
    fn set_tempo(&mut self, bpm: f64) -> f64 {
        let t = bpm.clamp(10.0, 522.0);
        if self.state.tempo != t {
            self.state.tempo = t;
            self.state.piano.dirty = true;
            self.damage_rect(TEMPO_RECT);
        }
        t
    }

    pub(crate) fn damage_win(&mut self, w: Win) {
        if !self.state.shown(w) {
            return self.invalidate();
        }
        match w {
            Win::ChannelRack => self.damage_rect(views::rack::bounds(&self.state)),
            Win::Perf => self.damage_rect(views::perf::bounds(&self.state)),
            Win::Tapper => self.damage_rect(views::tapper::bounds(&self.state)),
            Win::Playlist | Win::Mixer | Win::PianoRoll => self.damage_rect(self.state.win_area(w)),
            _ => self.invalidate(),
        }
    }

    /// Repaint the live regions of `kind` from the last paint.
    fn damage_live(&mut self, kind: Live) {
        let rs: Vec<Rect> = self.live.iter().filter(|(k, _)| *k == kind).map(|(_, r)| *r).collect();
        for r in rs {
            self.damage_rect(r);
        }
    }

    /// The playback state now, as the frame should show it.
    fn play_state(&self) -> Shown {
        let playing = self.transport.playing();
        Shown {
            playing,
            paused: self.transport.paused(),
            tick: self.transport.tick(),
            song: self.transport.song(),
            sounding: if playing { self.transport.sounding() } else { Vec::new() },
        }
    }

    /// Damage what playback moved since the shown state, then show the new one: the clock, both playheads (old and
    /// new place), the rack's playing step and the lights of sounding notes. Starting, stopping, pausing and switching
    /// between pattern and song repaint everything.
    fn playback_damage(&mut self) {
        let now = self.play_state();
        self.show_playback(now);
    }

    /// `playback_damage` for a given state (tests and the paint benchmark play without a device).
    pub(crate) fn show_playback(&mut self, now: Shown) {
        let old = std::mem::replace(&mut self.shown, now.clone());
        // The song looped back to its start: the initial values again (spec: ui/settings-choices.md "Advanced:
        // Initialize controls automatically"). The engine sets them itself there; the project follows, so the controls
        // show them.
        let looped = now.playing && old.playing && now.song && old.song && !now.paused && now.tick < old.tick;
        if looped && crate::init::apply(&mut self.state) {
            self.invalidate();
        }
        if now.playing != old.playing || now.paused != old.paused || now.song != old.song {
            self.auto_scroll();
            return self.invalidate();
        }
        if !now.playing {
            return;
        }
        if now.tick != old.tick {
            self.damage_live(Live::Clock);
            let (pl, piano, rack) = (self.state.shown(Win::Playlist), self.state.shown(Win::PianoRoll), self.state.shown(Win::ChannelRack));
            if now.song && pl {
                if let Some(g) = self.pl_grid {
                    let pg = crate::playlist::PGeom::new(&self.state, g);
                    // The playhead line and the ruler's song-position triangle (and the marker lane between them).
                    let top = g.y - 22.0 - crate::playlist::MARKER_H;
                    for t in [old.tick, now.tick] {
                        // The flag's outline reaches x + 10.5 (views/playlist.rs ruler_paint).
                        let x = pg.x_of(t as f32);
                        self.damage_rect(Rect::new(x - 8.0, top, 20.0, g.bottom() - top));
                    }
                }
            }
            if piano {
                if let Some(g) = self.piano_geom() {
                    let grid = g.grid;
                    // In song mode the piano roll's playhead is where its pattern plays, while a clip of it does.
                    let at = |t: f64| if now.song { piano::pattern_tick_in_song(&self.state, t) } else { Some(t) };
                    for t in [at(old.tick), at(now.tick)].into_iter().flatten() {
                        self.damage_rect(Rect::new(g.x_of(t as f32) - 2.0, grid.y, 5.0, grid.h));
                    }
                }
            }
            if !now.song && rack {
                for r in views::rack::step_columns(&self.state, old.tick, now.tick) {
                    self.damage_rect(r);
                }
            }
            self.auto_scroll();
        }
        if now.sounding != old.sounding {
            self.damage_live(Live::Sounding);
        }
    }

    /// Debug panel slider `i` follows mouse x.
    fn set_mon_param(&mut self, i: usize, x: f32) {
        let t = views::mondebug::slider(i);
        let p = &crate::monitor::PARAMS[i];
        let k = ((x - t.x) / t.w).clamp(0.0, 1.0);
        self.state.monitor.cfg.set(i, p.min + (p.max - p.min) * k);
        self.hint = Cow::Owned(format!("{}: {}", p.name, self.state.monitor.cfg.text(i)));
        self.damage_rect(views::mondebug::PANEL);
        self.damage_rect(MONITOR_RECT);
        self.damage_rect(HINT_RECT);
    }

    /// The cable physics panel's controls (views/cabledebug.rs).
    fn cable_debug(&mut self, d: crate::cable::DebugAct) {
        use crate::cable::DebugAct;
        let c = self.state.cables.get_mut();
        match d {
            DebugAct::Param(i) => {
                self.drag = Some(Drag::CableParam(i));
                let x = self.mouse.0;
                return self.set_cable_param(i, x);
            }
            DebugAct::Reset => {
                for i in 0..crate::cable::PARAMS.len() {
                    c.set(i, crate::cable::PARAMS[i].default);
                }
            }
            DebugAct::Save => {
                self.state.message = Some(match save_cable_cfg(&c.tuning) {
                    Ok(p) => format!("Cable physics saved to {}", p.display()),
                    Err(e) => format!("Cable physics not saved: {e}"),
                });
            }
            DebugAct::Close => c.debug = false,
        }
        self.invalidate();
    }

    /// Cable debug panel slider `i` follows mouse x; the cables take the new value at once.
    fn set_cable_param(&mut self, i: usize, x: f32) {
        let t = views::cabledebug::slider(i);
        let p = &crate::cable::PARAMS[i];
        let k = ((x - t.x) / t.w).clamp(0.0, 1.0);
        let c = self.state.cables.get_mut();
        c.set(i, p.min + (p.max - p.min) * k);
        self.hint = Cow::Owned(format!("{}: {}", p.name, c.tuning.text(i)));
        self.damage_rect(views::cabledebug::PANEL);
        self.damage_rect(HINT_RECT);
        self.damage_win(Win::Mixer);
    }

    /// The monitor is moving (a signal or a decay), or the tapper's pad fading: the shell should tick at animation rate.
    pub fn animating(&self) -> bool {
        self.transport.playing()
            || matches!(self.drag, Some(Drag::SongPosHold { .. }))
            || self.state.monitor.active()
            || self.state.mixer.meters.iter().any(|m| m.active())
            || self.loading()
            || self.plugin_windows_busy()
            || self.state.tapper.flash_at.is_some()
            || self.state.cables.borrow().moving()
            || self.hold.is_some()
            || self.smooth.running()
    }

    /// Linux: a plugin window is open (its X11 events and the editor's run loop are served on the tick).
    fn plugin_windows_busy(&self) -> bool {
        #[cfg(target_os = "linux")]
        return crate::plugin_window::busy();
        #[cfg(not(target_os = "linux"))]
        false
    }

    /// General ▸ Miscellaneous checks that follow a change, on every tick and before a frame: "Select first channel with
    /// notes when the pattern changes" (`App::pattern_tick`), "Auto zoom in piano roll" (`piano::open_tick`). Also the
    /// theme's rule for new clips' channel colours, which the code making them reads from the state.
    fn follow_settings(&mut self) {
        self.pattern_tick();
        if crate::piano::open_tick(&mut self.state) {
            self.invalidate();
        }
        let clips = self.clip_colors();
        if self.state.clip_colors != clips {
            self.state.clip_colors = clips;
        }
    }

    /// The current theme's rule for the colours of new audio and automation clips' channels (read-only): the applied
    /// `.flstheme`'s `OverrideClips` ("Audio and automation clips use note colors", which the Theme page toggles; off
    /// for the stock Dark theme used without a file) and its note colours.
    /// Spec: ui/settings-choices.md "Light mode and clip note colours (2026-10-16)".
    pub fn clip_colors(&self) -> crate::clip_colors::ClipColors {
        crate::clip_colors::ClipColors::of(&self.state.fl_theme())
    }

    /// The mixer's cables swing until still: step them and repaint where they were and are.
    fn cables_tick(&mut self, now: std::time::Instant) {
        if !self.state.shown(Win::Mixer) {
            self.state.cables.get_mut().clear();
            self.cables_at = None;
            return;
        }
        let dt = self.cables_at.map_or(0.0, |t| now.duration_since(t).as_secs_f32());
        let cables = self.state.cables.get_mut();
        let moved = cables.advance(dt);
        self.cables_at = cables.moving().then_some(now);
        if let Some(r) = moved {
            self.damage_rect(r);
        }
    }

    /// Periodic housekeeping; returns whether a repaint is needed.
    pub fn tick(&mut self) -> bool {
        self.plugin_tick();
        #[cfg(all(plugin_windows, not(windows)))]
        self.lift_input();
        {
            // Scope / analyzer / peak bars: from the audio tap; repaint just the monitor when it changed.
            // The monitor advances at its own refresh rate, however often we tick.
            let (tp, rate) = (&self.transport, self.transport.sample_rate());
            if self.state.monitor.update(std::time::Instant::now(), rate, || tp.take_peaks(), |b| tp.tap(b)) {
                self.damage_rect(MONITOR_RECT);
            }
        }
        if let Some(e) = self.transport.take_error() {
            self.state.message = Some(format!("Audio: {e}"));
            self.invalidate();
        }
        // CPU panel (spec: ui/main-window.md §13) and the Plugin performance monitor: a reading at the panel's refresh
        // rate.
        let now = std::time::Instant::now();
        self.song_pos_tick(now);
        self.hold_tick(now);
        self.ease_tick(now);
        self.cables_tick(now);
        self.backup_tick(now);
        self.work_tick(now);
        self.auto_close_tick(now);
        self.follow_settings();
        if self.state.cpu.due(now) {
            let tp = &self.transport;
            let load = tp.playing().then(|| tp.load()).filter(|_| !tp.paused());
            // Every stream's engine voices and every hosted instrument's (spec: audio/voices.md §5).
            let voices = tp.engine_voices() + self.plugins.voices();
            let dropouts = tp.underruns();
            if self.state.shown(Win::Perf) {
                let src = self.perf_sources();
                if self.state.perf.read(now, src) {
                    self.damage_win(Win::Perf);
                }
            }
            if self.state.cpu.update(now, load, voices, dropouts) {
                self.damage_rect(CPU_RECT);
                let (x, y) = self.mouse;
                if self.top_hit(x, y).is_some_and(|h| h.action == Action::CpuPanel) {
                    self.hint = Cow::Owned(self.state.cpu.hint());
                    self.damage_rect(HINT_RECT);
                }
                if self.state.settings.open && self.state.settings.tab == views::settings::AUDIO && self.audio_status() {
                    self.invalidate();
                }
            }
        }
        // The tapper's pad fades back after a tap (spec: ui/tempo-tapper.md §4).
        if self.state.tapper.fade(now) && self.state.shown(Win::Tapper) {
            self.damage_rect(views::tapper::pad_rect(&self.state));
        }
        // Mixer meters (spec: ui/mixer.md §9): fall back after playback stops.
        let peaks = self.transport.take_track_peaks();
        if crate::mixer::feed_meters(&mut self.state, &peaks) && self.state.shown(Win::Mixer) {
            self.damage_live(Live::Meter);
        }
        // Audio clips follow their channel's stretch, pitch and the tempo (ui/playlist.md §9c). The lengths are worked
        // out, not edited: the saved copy gets them too, so a fit alone (an opened FL project) is not an unsaved change.
        if let Some(fits) = crate::playlist::audio_fits(&self.state) {
            if crate::playlist::fit_clips(&mut self.state.project.arrangements, &fits) {
                crate::playlist::fit_clips(&mut self.saved.0.arrangements, &fits);
                self.state.piano.dirty = true;
                self.damage_win(Win::Playlist);
            }
        }
        // Edits (notes, steps, mixer, channel volume / pan / mute, tempo) reach playback right away,
        // also during a drag (at most once per tick).
        if self.state.piano.dirty {
            self.state.piano.dirty = false;
            // Also while Play is still starting (not `playing` yet); ignored when stopped.
            {
                self.transport.update(&crate::rack::play_project(&self.state));
            }
        }
        self.playback_damage();
        self.has_damage()
    }

    /// Paint the whole frame (`damage` None) or just the damaged rect.
    pub fn paint(&mut self, canvas: &mut Canvas, damage: Option<Rect>) {
        self.paint_region(canvas, damage.as_ref().map(std::slice::from_ref));
    }

    /// Paint the whole frame (None) or only the given rects, in one pass (see `Canvas::begin_region`).
    pub fn paint_region(&mut self, canvas: &mut Canvas, damage: Option<&[Rect]>) {
        // The CPU panel's View FPS counts painted frames.
        self.state.cpu.frame();
        // A playlist scroll's moves (`take_damage`) go first; the damage they left is painted with the rest.
        debug_assert!(damage.is_none() || self.scrolled.is_none(), "take_damage resolves a scroll before a partial paint");
        self.moved.clear();
        if damage.is_some() && self.px_scale == canvas.scale {
            for m in std::mem::take(&mut self.shifts) {
                canvas.shift(m.area, m.dx, m.dy, &m.keep);
                self.moved.push(m);
            }
        }
        self.shifts.clear();
        if damage.is_none() {
            self.scrolled = None;
        }
        self.px_scale = canvas.scale;
        match damage {
            Some(rects) => canvas.begin_region(rects),
            None => canvas.begin_frame(None),
        }
        self.state.screen = canvas.size();
        // The main window or the workspace shrank: the windows keep their part in it (`win_clamp::keep_all`). Moved
        // during a partial paint, the next frame is painted whole.
        if crate::win_clamp::keep_all(&mut self.state) && damage.is_some() {
            self.invalidate();
        }
        // Plugin editors in this process colour their waveforms as the views do (spec: ui/settings-choices.md
        // "Display: Colorful waveforms (2026-10-16)").
        crate::waves::publish(&self.theme.colors);
        // The widget kit's scroll bars follow High visibility (spec: ui/settings-choices.md "Display: PPI ruler, Force
        // refreshes, Thick lines, High visibility").
        crate::high_visibility::publish();
        // Last paint's hits stay available to the parts this one skips (see `Ui::part`).
        std::mem::swap(&mut self.hits, &mut self.keep.prev);
        std::mem::swap(&mut self.bars, &mut self.keep.prev_bars);
        std::mem::swap(&mut self.live, &mut self.keep.prev_live);
        self.hits.clear();
        self.bars.clear();
        self.live.clear();
        self.keep.full = damage.is_none();
        let title = self.title();
        let mut ui = Ui {
            c: canvas,
            t: &self.theme.colors,
            hits: &mut self.hits,
            bars: &mut self.bars,
            hover: &self.hover,
            pressed: &self.pressed,
            mouse: self.mouse,
            win: None,
            keep: &mut self.keep,
            live: &mut self.live,
            flat: self.state.toolbar.flat,
        };
        let info = views::FrameInfo {
            title,
            hint: self.hint.clone(),
            playing: self.shown.playing,
            paused: self.shown.paused,
            tick: self.shown.tick,
            song: self.shown.song,
            peaks: self.transport.peaks(),
            sounding: self.shown.sounding.clone(),
            song_pos_drag: matches!(self.drag, Some(Drag::SongPos { .. })),
        };
        let overlays = views::paint(&mut ui, &self.state, &info);
        if self.state.cables.get_mut().take_slowed() {
            self.state.message = Some("Mixer cables hang still: the mixer is too slow to redraw them swinging (General settings > Animations)".into());
            self.damage_win(Win::Mixer);
        }
        self.keep.parts = std::mem::take(&mut self.keep.next);
        self.keep.prev.clear();
        self.keep.prev_bars.clear();
        self.keep.prev_live.clear();
        self.overlays = self.hits[overlays.min(self.hits.len())..].iter().map(|h| h.rect).collect();
        // Remember where the piano roll's regions are, for mouse mapping and damage.
        let find = |a: Action| self.hits.iter().find(|h| h.action == a).map(|h| h.rect);
        self.piano_grid = find(Action::PianoGrid);
        self.piano_panel = find(Action::PianoVelocity);
        self.pl_grid = find(Action::PlaylistArea);
        self.pl_covers = self.playlist_covers();
        self.piano_win = match (self.piano_grid, self.piano_panel) {
            // The window below its title bar: overview, ruler, keyboard, grid, velocity panel.
            (Some(g), Some(p)) => Some(Rect::new(g.x - 73.0, g.y - 41.0, g.w + 96.0, p.bottom() - g.y + 41.0)),
            _ => None,
        };
        if let Some(l) = &self.loading {
            crate::loading::paint(canvas, &self.theme.colors, l);
        }
        if let Some((name, p)) = &self.export {
            let status = format!("Rendering {name}   {:.0} %", p * 100.0);
            crate::loading::panel(canvas, &self.theme.colors, "Exporting", &status, *p);
        }
    }

    /// Repaint the piano roll body (note edits change the grid, overview and velocity panel).
    pub(crate) fn damage_piano(&mut self) {
        match self.piano_win {
            Some(r) => self.damage_rect(r),
            None => self.invalidate(),
        }
        // In PAT mode the song position slider shows the play-start marker within the pattern's length.
        if !self.state.song_mode {
            self.damage_rect(crate::song_pos::PANEL);
        }
    }

    pub(crate) fn piano_geom(&self) -> Option<Geom> {
        self.piano_grid.map(|g| Geom::new(&self.state, g))
    }

    /// A press on the piano roll's splitter: drags it; a double click hides / brings back the event panel (spec:
    /// ui/event-panel-splitter.md §2).
    fn piano_split_press(&mut self) {
        let now = std::time::Instant::now();
        let p = &mut self.state.piano;
        let double = p.split_click.is_some_and(|t| now.duration_since(t).as_millis() < 400);
        p.split_click = if double { None } else { Some(now) };
        if double {
            p.split = p.split.toggled();
            crate::piano_split::save(p.split);
            return self.damage_piano();
        }
        let band = self.hits.iter().find(|h| h.action == Action::PianoSplitter).map(|h| h.rect);
        if let (Some(band), Some(g), Some(panel)) = (band, self.piano_grid, self.piano_panel) {
            let h = panel.bottom() - g.y - crate::piano_split::BAND;
            self.drag = Some(Drag::PianoSplit { dy: self.mouse.1 - band.y, top: g.y, h });
        }
    }

    /// The playlist grid from the last paint.
    pub fn pl_grid(&self) -> Option<Rect> {
        self.pl_grid
    }

    /// The playlist takes the editing keys: it is shown and above the piano roll (or the piano roll is hidden),
    /// or its text box is open.
    pub fn pl_focus(&self) -> bool {
        let s = &self.state;
        if s.playlist.edit.is_some() {
            return true;
        }
        let pos = |w: Win| s.z.iter().position(|&x| x == w);
        match (pos(Win::Playlist), pos(Win::PianoRoll)) {
            (Some(p), Some(r)) => p > r,
            (Some(_), None) => true,
            _ => false,
        }
    }

    /// Song playback from `tick` (Song mode; spec: ui/playlist.md §7).
    pub fn play_song_from(&mut self, tick: u32) {
        self.state.song_mode = true;
        if self.state.project.arrangement().is_none_or(|a| a.clips.is_empty()) {
            // Nothing plays in SONG mode, so a pattern that was playing stops too.
            if self.transport.playing() {
                self.transport.stop();
            }
            self.state.message = Some("The playlist is empty: nothing to play in SONG mode".into());
            return self.invalidate();
        }
        // The initial values first (spec: ui/settings-choices.md "Advanced: Initialize controls automatically").
        crate::init::apply(&mut self.state);
        let mut p = self.state.project.clone();
        p.tempo = Some(self.state.tempo);
        self.transport.stop();
        self.transport.play(&p, PlayMode::Song, f64::from(tick));
        self.invalidate();
    }

    /// Clip menu "Preview": play just this clip once (spec: ui/playlist.md §10).
    pub fn preview_clip(&self, c: &os_model::Clip) {
        let mut p = self.state.project.clone();
        p.tempo = Some(self.state.tempo);
        let mut clip = *c;
        clip.position = 0;
        clip.track = 0;
        clip.muted = false;
        p.arrangements = vec![os_model::Arrangement { clips: vec![clip], ..Default::default() }];
        self.transport.preview_song(p);
    }

    /// A file dropped on the window: audio onto the playlist becomes an audio clip; a project opens.
    pub fn drop_file(&mut self, path: PathBuf) {
        let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        if self.modal_open() {
            return self.dialog_drop(path);
        }
        // A drop accepts an Info box or a browser folder cell being edited, as a click does.
        crate::project_info::accept_info(&mut self.state);
        crate::search_folders::commit(&mut self.state);
        if ext == "flp" {
            return self.ask_discard("opening another project", crate::dialog::Next::Open(path));
        }
        if path.is_dir() {
            return self.add_browser_folder(path);
        }
        if ext == "flstheme" || ext == "txt" && path.file_name().is_some_and(|n| n == "theme.txt") {
            self.load_theme_file(&path);
            return;
        }
        let (x, y) = self.mouse;
        if self.top_hit(x, y).and_then(|h| h.win) == Some(Win::ChannelRack) {
            self.sample_to_rack(path.to_string_lossy().into_owned(), y);
            return;
        }
        let Some(grid) = self.pl_grid.filter(|g| g.contains(x, y)) else {
            self.state.message = Some("Drop audio files on the playlist to add audio clips".into());
            return self.invalidate();
        };
        let s = &mut self.state;
        let g = crate::playlist::PGeom::new(s, grid);
        let track = g.track_at(y);
        let tick = crate::playlist::snap_place(s, &g, g.tick_at(x), self.alt);
        let name = path.to_string_lossy().into_owned();
        s.message = Some(match crate::playlist::add_audio_clip(s, &name, track, tick) {
            Some(_) => format!("Added audio clip {}", path.display()),
            None => format!("Could not read {} as audio", path.display()),
        });
        self.invalidate();
    }

    /// A sample dropped on the channel rack at window y `y` (spec: ui/channel-rack.md, New project / drops): onto a
    /// sampler row it replaces that channel's sample (Lemon Sampler loads it) and renames the channel after the file
    /// while "Auto name channels" is on (`rack::auto_name`); elsewhere it adds a Lemon Sampler channel named after the
    /// file. Either channel reads the file's root note and beats (`rack::read_tags`). One history step, "Load channel
    /// sample(s)" (`history::load_sample`).
    pub(crate) fn sample_to_rack(&mut self, path: String, y: f32) {
        let s = &mut self.state;
        let stem = std::path::Path::new(&path).file_stem().map(|n| n.to_string_lossy().into_owned());
        let row = match views::rack::row_at(s, y).filter(|&r| s.project.channels[r].kind.is_sampler()) {
            Some(r) => {
                crate::history::load_sample(s, r);
                s.sample_loads.push((r, path.clone()));
                crate::rack::auto_name(&mut s.project.channels[r], std::path::Path::new(&path));
                r
            }
            None => {
                // The step covers the channel itself: undone, it goes.
                crate::history::load_sample(s, s.project.channels.len());
                let r = add_channel(&mut s.project);
                s.project.channels[r].name = stem.clone();
                r
            }
        };
        let ch = &mut s.project.channels[row];
        crate::rack::read_tags(ch, std::path::Path::new(&path));
        ch.sample_path = Some(path);
        s.selected_channel = row;
        s.rack.select_only(row);
        s.piano.dirty = true;
        s.message = Some(format!("{} loaded in Lemon Sampler", stem.unwrap_or_default()));
        self.invalidate();
    }

    /// A browser sample released over a hosted plugin window (it sits above the main window, which keeps the mouse
    /// capture during the drag): the plugin gets the file as if it came from Explorer (on macOS only Lemon Sampler takes
    /// it, through its state). Returns whether it was one.
    #[cfg(any(windows, target_os = "macos"))]
    pub(crate) fn sample_to_plugin_window(&mut self, path: &str) -> bool {
        #[cfg(windows)]
        let (x, y) = {
            use windows_sys::Win32::Foundation::POINT;
            let mut pt = POINT { x: 0, y: 0 };
            if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt) } == 0 {
                return false;
            }
            (pt.x, pt.y)
        };
        #[cfg(target_os = "macos")]
        let Some((x, y)) = crate::plugin_window::pointer() else { return false };
        let Some(row) = self.plugins.row_at_screen(x, y) else { return false };
        let sampler = self.state.project.channels.get(row).is_some_and(|c| c.kind.is_sampler());
        let name = std::path::Path::new(path).file_stem().map(|n| n.to_string_lossy().into_owned());
        if self.plugins.drop_file(row, x, y, path, sampler) {
            if sampler && row < self.state.project.channels.len() {
                crate::history::load_sample(&mut self.state, row);
            }
            if let Some(ch) = self.state.project.channels.get_mut(row).filter(|_| sampler) {
                crate::rack::auto_name(ch, std::path::Path::new(path));
                crate::rack::read_tags(ch, std::path::Path::new(path));
                ch.sample_path = Some(path.to_string());
                // The plugin loads the file itself: it takes what the load read alongside.
                let set = crate::plugins::tag_params(ch);
                self.plugins.set_sampler(row, None, &set);
            }
            self.state.message = Some(format!("{} loaded in the plugin", name.unwrap_or_default()));
        } else {
            self.state.message = Some("This plugin does not take dropped files".into());
        }
        self.invalidate();
        true
    }

    /// Load a theme: a `.flstheme` (spec ui/flstheme.md) or a Lemon Studio `theme.txt`. A `.flstheme` is also
    /// remembered as `current.flstheme` in the user folder so the next start uses it.
    pub fn load_theme_file(&mut self, path: &std::path::Path) {
        self.load_theme_file_ex(path, true)
    }

    /// `load_theme_file`; `remember` = keep it for the next start.
    pub fn load_theme_file_ex(&mut self, path: &std::path::Path, remember: bool) {
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        match theme_from_file(path) {
            Ok(t) => {
                // Theme files colour the Classic design (Lemon has its own colours).
                let was_lemon = self.design() == Design::Lemon;
                self.theme = t;
                // Theme ▸ Options read and rewrite the applied .flstheme (spec: ui/settings-choices.md "Light mode and
                // clip note colours").
                let fl = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("flstheme"));
                self.state.fl_text = fl.then(|| String::from_utf8_lossy(&std::fs::read(path).unwrap_or_default()).into_owned());
                os_theme::set_current(&self.theme.colors);
                if remember && was_lemon {
                    save_design(Design::Classic);
                }
                if remember && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("flstheme")) {
                    if let Some(d) = os_paths::user_dir() {
                        let _ = std::fs::create_dir_all(&d);
                        let keep = d.join("current.flstheme");
                        if keep.as_path() != path {
                            let _ = std::fs::copy(path, keep);
                        }
                    }
                }
                self.state.settings.preset = usize::MAX;
                self.sync_preset();
                self.state.message = Some(if was_lemon { format!("Theme: {name} (Classic design; Ctrl+Shift+D switches)") } else { format!("Theme: {name}") });
            }
            Err(e) => self.state.message = Some(format!("Could not load theme {name}: {e}")),
        }
        self.invalidate();
    }

    /// The design in use (spec: ui/lemon-design.md).
    pub fn design(&self) -> Design {
        self.theme.colors.style.design
    }

    /// Switch to design `d`: Lemon brings its own colours, Classic the saved theme (or the default look).
    /// `remember` = keep the choice for the next start (`design.txt` in the user folder).
    pub fn set_design(&mut self, d: Design, remember: bool) {
        self.theme = design_theme(d, remember);
        os_theme::set_current(&self.theme.colors);
        if remember {
            save_design(d);
        }
        self.sync_preset();
        self.state.message = Some(format!("Design: {} (Ctrl+Shift+D switches)", d.name()));
        self.invalidate();
    }

    /// Theme ▸ Options: flips "Light mode" (`Lightmode`) or "Audio and automation clips use note colors"
    /// (`OverrideClips`) in the applied theme, the stock Dark theme's text when no `.flstheme` is applied. The Classic
    /// design shows it at once (Lemon has its own colours), and it is kept as `current.flstheme` for the next start
    /// (spec: ui/settings-choices.md "Light mode and clip note colours").
    pub(crate) fn toggle_theme_option(&mut self, label: &str) {
        let th = self.state.fl_theme();
        let (key, on) = if label == views::settings::LIGHT_MODE_CHECK { ("Lightmode", !th.light_mode) } else { ("OverrideClips", !th.override_clips) };
        let text = self.state.fl_text.as_deref().unwrap_or(os_theme::fl::DARK_TEXT);
        let text = os_theme::fl::set_key(text, key, if on { "1" } else { "0" });
        if self.design() == Design::Classic {
            let palette = os_theme::fl::palette(&os_theme::FlTheme::parse(&text));
            self.theme = os_theme::Theme::new(&self.theme.name.clone(), palette, self.theme.adjust);
            os_theme::set_current(&self.theme.colors);
            self.sync_preset();
        }
        if let Some(d) = theme_dir() {
            let _ = std::fs::create_dir_all(&d);
            let _ = std::fs::write(d.join("current.flstheme"), &text);
        }
        self.state.fl_text = Some(text);
        self.state.message = Some(format!("{label}: {}", if on { "on" } else { "off" }));
        self.invalidate();
    }

    /// The Theme preset tile shown as chosen (spec: ui/lemon-design.md §1): (Default) for Lemon, Classic for the plain
    /// Classic look, else the theme file picked from the tiles (none when the theme came from elsewhere).
    fn sync_preset(&mut self) {
        let st = &mut self.state.settings;
        st.preset = if self.theme.colors.style.lemon() {
            0
        } else if self.theme.colors == os_theme::Palette::default() {
            1
        } else if st.preset >= 2 {
            st.preset
        } else {
            usize::MAX
        };
    }

    /// Open the settings window on `tab` (spec: ui/settings.md §1); reads the output devices and theme files.
    pub fn open_settings(&mut self, tab: usize) {
        let st = &mut self.state.settings;
        if st.tab != tab {
            st.scroll = 0.0;
        }
        st.open = true;
        st.tab = tab;
        self.sync_preset();
        self.audio_page();
        let st = &mut self.state.settings;
        // Built-in presets (spec: ui/theme-presets.md), then .flstheme files in the user folder's `themes`.
        st.themes = os_theme::presets::PRESETS.iter().map(|(n, t)| (n.to_string(), t.to_string())).collect();
        if let Some(d) = os_paths::user_file("themes") {
            if let Ok(rd) = std::fs::read_dir(&d) {
                let mut v: Vec<_> = rd
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("flstheme")))
                    .filter_map(|p| Some((p.file_stem()?.to_string_lossy().into_owned(), String::from_utf8_lossy(&std::fs::read(&p).ok()?).into_owned())))
                    .collect();
                v.sort();
                st.themes.extend(v);
            }
        }
        self.invalidate();
    }

    /// Fills the Audio page: the output devices, the audio settings and the status (spec: ui/settings.md "Audio").
    fn audio_page(&mut self) {
        let (devices, default) = crate::audio_cfg::devices();
        let cfg = crate::audio_cfg::AudioCfg::current();
        let st = &mut self.state.settings;
        st.device = match &cfg.device {
            Some(d) if devices.contains(d) => d.clone(),
            Some(d) => format!("{d} (not found: default device)"),
            None => format!("Default ({})", default.as_deref().unwrap_or("no output device")),
        };
        st.devices = devices;
        st.audio = cfg;
        st.rate = crate::audio_cfg::rate();
        self.audio_status();
    }

    /// The Audio page's status lines from the stream that is open; returns whether they changed.
    fn audio_status(&mut self) -> bool {
        let tp = &self.transport;
        let host = cpal::default_host().id().name();
        let first = match tp.opened() {
            Some((name, rate, buffer)) => {
                let block = match tp.block() {
                    0 => String::new(),
                    b => format!(", blocks of {}", views::settings::buffer_text(b, rate)),
                };
                let buffer = buffer.map_or(String::new(), |b| format!(", buffer {}", views::settings::buffer_text(b, rate)));
                format!("Status: {name} open at {rate} Hz{block}{buffer} ({host})")
            }
            None => format!("Status: opens with the first note or Play ({host})"),
        };
        let second = match tp.underruns() {
            0 => "No dropouts so far.".to_string(),
            n => format!("Dropouts so far: {n} (device blocks the engine could not fill in time; a longer buffer helps)"),
        };
        let st = &mut self.state.settings;
        let changed = st.status[0] != first || st.status[1] != second;
        st.status = [first, second];
        changed
    }

    /// The menu of an Audio page combo, under its box (`x`, `y`).
    fn audio_menu(&mut self, label: &str, x: f32, y: f32) {
        use crate::audio_cfg::{Set, BUFFERS, RATES};
        use crate::playlist::{Cmd, Mark, PItem, Popup};
        use views::settings::{ANIMATIONS, BUFFER_LENGTH, OUTPUT_DEVICE, SAMPLE_RATE};
        let still = self.state.cables.borrow().still_cables;
        let st = &self.state.settings;
        let cfg = &st.audio;
        let rate = st.rate.unwrap_or(44100);
        let items: Vec<PItem> = match label {
            // Project ▸ Time settings (spec: ui/settings-choices.md "Project"): the timebase, FL's list.
            views::settings::TIMEBASE => {
                let now = self.state.project.ppq;
                views::settings::TIMEBASES.iter().map(|&v| PItem::new(v.to_string(), Cmd::Timebase(v)).mark(Mark::Radio(v == now))).collect()
            }
            // The "Set as" mode (event 35), in FL's order.
            views::settings::SET_AS => {
                let sig = self.state.project.bar_is_signature();
                vec![
                    PItem::new(views::settings::SET_AS_DIVISION, Cmd::TimeMode(false)).mark(Mark::Radio(!sig)),
                    PItem::new(views::settings::SET_AS_SIGNATURE, Cmd::TimeMode(true)).mark(Mark::Radio(sig)),
                ]
            }
            // Project ▸ Advanced ▸ Panning law (event 23), FL's two choices.
            views::settings::PANNING_LAW => {
                let now = self.state.project.panning_law.unwrap_or(0);
                views::settings::PANNING_LAWS.iter().map(|&(name, v)| PItem::new(name, Cmd::PanningLaw(v)).mark(Mark::Radio(v == now))).collect()
            }
            // One of the time signature's two numbers: a numerator is the beats per bar (the steps per beat stay), a
            // denominator d makes 16 / d steps per beat (events 17, 18).
            views::settings::NUMERATOR | views::settings::DENOMINATOR => {
                let now = self.state.project.bar();
                let numerator = label == views::settings::NUMERATOR;
                let choices = if numerator { views::settings::NUMERATORS } else { views::settings::DENOMINATORS };
                choices
                    .iter()
                    .map(|&v| {
                        let bar = if numerator { (v, now.1) } else { (now.0, 16 / v.max(1)) };
                        PItem::new(v.to_string(), Cmd::Bar(bar.0, bar.1)).mark(Mark::Radio(bar == now))
                    })
                    .collect()
            }
            OUTPUT_DEVICE => std::iter::once(PItem::new("Default device", Cmd::Audio(Set::Device(None))).mark(Mark::Radio(cfg.device.is_none())))
                .chain(st.devices.iter().map(|d| PItem::new(d.clone(), Cmd::Audio(Set::Device(Some(d.clone())))).mark(Mark::Radio(cfg.device.as_ref() == Some(d)))))
                .collect(),
            SAMPLE_RATE => std::iter::once(PItem::new("Device rate", Cmd::Audio(Set::Rate(None))).mark(Mark::Radio(cfg.rate.is_none())))
                .chain(RATES.iter().map(|&r| PItem::new(format!("{r} Hz"), Cmd::Audio(Set::Rate(Some(r)))).mark(Mark::Radio(cfg.rate == Some(r)))))
                .collect(),
            BUFFER_LENGTH => std::iter::once(PItem::new("Device default", Cmd::Audio(Set::Buffer(None))).mark(Mark::Radio(cfg.buffer.is_none())))
                .chain(BUFFERS.iter().map(|&b| {
                    PItem::new(views::settings::buffer_text(b, rate), Cmd::Audio(Set::Buffer(Some(b)))).mark(Mark::Radio(cfg.buffer == Some(b)))
                }))
                .collect(),
            ANIMATIONS => {
                let item = |label: &'static str, on: bool| PItem::new(label, Cmd::Mixer(crate::mixer::MCmd::Animations(on))).mark(Mark::Radio(still != on));
                vec![item("Entertain me", true), item("Off", false)]
            }
            _ => return,
        };
        self.state.playlist.popup = Some(Popup { x, y: y + 1.0, items, sub: None });
        self.invalidate();
    }

    /// Changes one audio setting: kept for the next start, hosted plugins move to the new rate, and the open streams
    /// move to the new output (spec: ui/settings.md "Audio").
    pub fn set_audio(&mut self, s: crate::audio_cfg::Set) {
        let cfg = crate::audio_cfg::AudioCfg::current().with(s);
        cfg.apply();
        if let Some(rate) = crate::audio_cfg::rate() {
            self.plugins.set_rate(rate);
        }
        self.transport.reopen();
        self.audio_page();
        let st = &self.state.settings;
        self.state.message = Some(format!("Audio: {}, {} Hz", st.device, st.rate.unwrap_or(0)));
        self.invalidate();
    }

    /// Theme import (spec: ui/flstheme.md): pick a .flstheme, starting in FL's theme folder when installed.
    fn theme_dialog(&mut self) {
        let fl = std::path::Path::new(r"C:\Program Files\Image-Line\FL Studio 2026\Artwork\Themes");
        let dlg = crate::filedlg::FileDlg::new(crate::filedlg::Mode::Open, "Load theme", "theme", crate::dialog::Then::LoadTheme)
            .filter("Theme", &["flstheme", "txt"])
            .start(Some(fl));
        self.file_dialog(dlg);
    }

    /// Settings > Project ▸ Time settings: the project's bar, beats per bar and steps per beat (events 17, 18; spec:
    /// ui/settings-choices.md "Project", audio/song-structure.md "Time signatures and a pattern's length"), from the
    /// Numerator / Denominator combos or a time division's value boxes. Playback follows at once (pattern lengths, the
    /// metronome's accents, what hosted plugins are told), as do the pattern lengths the editors draw; the project
    /// counts as changed.
    pub fn set_bar(&mut self, beats: u16, steps: u16) -> Option<String> {
        let bar = (beats.max(1), steps.max(1));
        if self.state.project.bar() != bar {
            self.state.project.bar = Some(bar);
            self.state.piano.dirty = true;
        }
        self.invalidate();
        Some(bar_text(&self.state.project))
    }

    /// Settings > Project ▸ Set as time signature / time division (event 35; spec: ui/settings-choices.md "Set as time
    /// division"). Each mode keeps its own last bar while the program runs (4/4 at first): switching keeps the bar of
    /// the mode left and writes the other mode's into the project. A bar is never converted between the modes.
    pub fn set_time_mode(&mut self, signature: bool) -> Option<String> {
        let s = &mut self.state;
        let now = s.project.bar_is_signature();
        if now != signature {
            let memory = &mut s.settings.bar_memory;
            memory[usize::from(now)] = Some(s.project.bar());
            s.project.bar = Some(memory[usize::from(signature)].unwrap_or(Project::BAR_DEFAULT));
            s.project.bar_as_signature = Some(signature);
            s.piano.dirty = true;
        }
        self.invalidate();
        let mode = if signature { views::settings::SET_AS_SIGNATURE } else { views::settings::SET_AS_DIVISION };
        Some(format!("{mode}: {}", bar_text(&self.state.project)))
    }

    /// Settings > Project ▸ a time division's value box (`label`, `settings::LIVE_NUMBERS`) moved by `delta` (the
    /// mouse wheel). It stays in its range; a file's value outside it can only move toward it.
    fn settings_number(&mut self, label: &str, delta: i32) -> Option<String> {
        // The value boxes declared in `crate::prefs` (General ▸ Undo history ▸ Maximum undo levels).
        if let Some(n) = crate::prefs::find_number(label) {
            // Audio ▸ Browser preview track offers "---" and the open project's insert tracks; one past them comes
            // back in range (spec: ui/settings-choices.md "Mixer: Browser preview track").
            let max = if n == &crate::prefs::BROWSER_PREVIEW_TRACK { n.max.min(crate::mixer::inserts(&self.state)) } else { n.max };
            let v = (i32::from(crate::prefs::number(n)) + delta).clamp(i32::from(n.min), i32::from(max.max(n.min))) as u16;
            crate::prefs::set_number(n, v);
            crate::history::trim(&mut self.state);
            self.invalidate();
            return Some(format!("{}: {}", n.label, crate::prefs::number_text(n)));
        }
        let (lo, hi) = views::settings::number_range(label)?;
        let (beats, steps) = self.state.project.bar();
        let beat_box = label == views::settings::BEATS_PER_BAR;
        let now = if beat_box { beats } else { steps };
        let v = (i32::from(now) + delta).clamp(i32::from(lo.min(now)), i32::from(hi.max(now))) as u16;
        if beat_box { self.set_bar(v, steps) } else { self.set_bar(beats, v) }
    }

    /// Settings > Project ▸ Timebase (PPQ): the project's ticks per quarter (the file header's PPQ). Every tick count
    /// of the project is rescaled at once and nothing is asked (spec: ui/settings-choices.md "Project"; the rounding is
    /// `os_model::rescale_tick`'s). Playback stops: its position is in ticks of the old timebase. The editors' positions,
    /// selections, clipboards and undo steps are rescaled with the project, so no undo brings back ticks of the old
    /// timebase; choosing the old timebase again rescales back (exactly, unless ticks were rounded on the way).
    pub fn set_timebase(&mut self, ppq: u16) -> Option<String> {
        let (from, to) = (self.state.project.ppq.max(1), ppq.max(1));
        if from != to {
            self.transport.stop();
            let s = &mut self.state;
            s.project.set_ppq(to);
            crate::piano::rescale(s, from, to);
            crate::playlist::rescale(s, from, to);
            crate::history::rescale(s, from, to);
            if let Some(notes) = &mut s.rack.clipboard {
                notes.iter_mut().for_each(|n| n.rescale(from, to));
            }
            s.piano.dirty = true;
        }
        self.invalidate();
        Some(format!("Timebase: {to} PPQ"))
    }

    /// Settings > Project ▸ Advanced ▸ Panning law (event 23: 0 Circular, 2 Triangular; spec: audio/gain-laws.md
    /// "Panning law: Triangular"): the law of the channel and mixer track pans. Playback follows from its next tick, as
    /// for any pan edit; the project counts as changed. A file without the event is Circular.
    pub fn set_panning_law(&mut self, law: u8) -> Option<String> {
        if self.state.project.panning_law.unwrap_or(0) != law {
            self.state.project.panning_law = Some(law);
            self.state.piano.dirty = true;
        }
        self.invalidate();
        Some(format!("Panning law: {}", views::settings::panning_text(Some(law))))
    }

    /// Settings > Project ▸ Advanced ▸ Play truncated notes in clips (spec: audio/song-structure.md "Play truncated
    /// notes"): switched for the project, and playback follows at once. Off when the file does not say.
    fn toggle_play_truncated(&mut self) {
        let on = !self.state.project.play_truncated;
        self.state.project.play_truncated = on;
        self.state.piano.dirty = true;
        self.state.message = Some(format!("Play truncated notes in clips: {}", if on { "on" } else { "off" }));
    }

    /// Settings > Project ▸ Playlist ▸ Automatic crossfades: switched for the project (event 44), which counts as
    /// changed; off when the file does not say. What it does to audio clips was not measured (spec:
    /// ui/settings-choices.md "Project", the storage table: "(not measured)"), so nothing else follows yet.
    fn toggle_auto_crossfades(&mut self) {
        let on = !self.state.project.auto_crossfades.unwrap_or(false);
        self.state.project.auto_crossfades = Some(on);
        self.state.message = Some(format!("Automatic crossfades: {}", if on { "on" } else { "off" }));
    }

    fn settings_act(&mut self, a: views::settings::SAct) {
        use views::settings::SAct;
        let st = &mut self.state.settings;
        match a {
            SAct::Body => {}
            SAct::Close => st.open = false,
            // The program settings declared in `crate::prefs`.
            SAct::Combo(views::settings::DEFAULT_TEMPLATE, x, y) => return self.template_menu(x, y),
            SAct::Combo(crate::data_folder::COMBO, x, y) => return self.data_folder_menu(x, y),
            SAct::Combo(label, x, y) if crate::prefs::find_combo(label).is_some() => return self.pref_menu(label, x, y),
            SAct::Combo(label, x, y) => return self.audio_menu(label, x, y),
            SAct::Slider(label, x, w) => return self.pref_slider_press(label, x, w),
            SAct::Ppi => return self.ppi_press(),
            SAct::Tab(i) => {
                st.tab = i;
                st.scroll = 0.0;
            }
            // Opt in / out of the experimental native host and scan again so the effect lists follow (spec
            // plugins/fruity-abi.md). Its displayed state comes straight from `os_native::enabled()`.
            SAct::Check(k) if k == views::settings::NATIVE_HOST_CHECK => {
                let on = !os_native::enabled();
                os_native::set_enabled(on);
                self.start_scan(crate::plugin_db::Mode::Changed);
                self.state.message = Some(if on { "Experimental native plugin host on: looking for native plugins".into() } else { "Experimental native plugin host off".into() });
                // The hidden preference applies again (or stops applying) with the host.
                if os_native::prefer_native_set() {
                    self.swap_builtin_overrides();
                }
            }
            // Real native plugins in place of the Lemon recreations (spec plugins/fruity-abi.md "Opened projects").
            SAct::Check(k) if k == views::settings::NATIVE_FIRST_CHECK => {
                let on = !os_native::prefer_native();
                os_native::set_prefer_native(on);
                let n = self.swap_builtin_overrides();
                self.state.message = Some(match (on, n) {
                    (true, 0) => "Opened projects play their own native plugins, not the Lemon recreations".into(),
                    (true, n) => format!("Opened projects play their own native plugins: {n} channels / slots switched from the Lemon recreations"),
                    (false, 0) => "Lemon recreations play their projects' plugins again".into(),
                    (false, n) => format!("Lemon recreations play their projects' plugins again: {n} channels / slots switched back"),
                });
            }
            // The plugin manager (spec: ui/plugin-manager.md §2).
            SAct::Check(k) if views::settings::plugin_check(k) => self.plugin_check(k),
            SAct::Plug(p) => self.plugin_act(p),
            SAct::Check(k) if crate::prefs::find_check(k).is_some() => self.pref_toggle(k),
            SAct::Check(k) if k == views::settings::TRUNCATED_CHECK => self.toggle_play_truncated(),
            SAct::Check(k) if k == views::settings::CROSSFADES_CHECK => self.toggle_auto_crossfades(),
            SAct::Check(k) if k == views::settings::SHOW_INFO_CHECK => self.toggle_show_info(),
            SAct::Check(k) if k == views::settings::LIGHT_MODE_CHECK || k == views::settings::NOTE_COLORS_CHECK => self.toggle_theme_option(k),
            // A time division's value box: the mouse wheel changes it (`wheel_act`).
            SAct::Number(label) => self.state.message = Some(format!("{label}: the mouse wheel changes it")),
            // Checks and buttons outside `settings::LIVE` are drawn greyed out with an `Action::Placeholder` hit instead.
            SAct::Check(k) => self.state.message = Some(off_text(k)),
            SAct::Group(k) => {
                if !st.folded.remove(k) {
                    st.folded.insert(k);
                }
            }
            // Tiles (spec: ui/lemon-design.md §1): 0 = (Default) = the Lemon design, 1 = the Classic look without a
            // theme file, then the theme files (they colour the Classic design).
            SAct::Preset(0) => {
                self.set_design(Design::Lemon, true);
                self.state.message = Some("Theme: default (Lemon)".into());
            }
            SAct::Preset(1) => {
                self.theme = os_theme::Theme::default();
                os_theme::set_current(&self.theme.colors);
                save_design(Design::Classic);
                if let Some(f) = os_paths::user_file("current.flstheme") {
                    let _ = std::fs::remove_file(f);
                }
                self.state.fl_text = None;
                self.sync_preset();
                self.state.message = Some("Theme: Classic".into());
            }
            SAct::Preset(i) => {
                if let Some((name, text)) = st.themes.get(i - 2).cloned() {
                    st.preset = i;
                    let fl = os_theme::FlTheme::parse(&text);
                    self.theme = os_theme::Theme::new(&name, os_theme::fl::palette(&fl), os_theme::Adjust::default());
                    os_theme::set_current(&self.theme.colors);
                    self.state.fl_text = Some(text.clone());
                    save_design(Design::Classic);
                    // Remembered like an imported theme (loaded at the next start).
                    if let Some(d) = os_paths::user_dir() {
                        let _ = std::fs::create_dir_all(&d);
                        let _ = std::fs::write(d.join("current.flstheme"), &text);
                    }
                    self.state.message = Some(format!("Theme: {name}"));
                }
            }
            SAct::Button("Reset") => self.settings_act(SAct::Preset(0)),
            SAct::Button("Load theme...") => self.theme_dialog(),
            SAct::Button("Manage plugins") => self.open_settings(views::settings::PLUGINS),
            SAct::Button("Manage messages") => return self.messages_menu(),
            SAct::Button("Find new plugins") => self.start_scan(crate::plugin_db::Mode::Changed),
            SAct::Button("Rescan all") => self.start_scan(crate::plugin_db::Mode::All),
            SAct::Button("Add folder...") => self.add_plugin_folder(),
            SAct::Button(views::settings::RESET_WORK_TIME) => self.reset_work_time(),
            SAct::Button(views::settings::MANAGE_SHORTCUTS) => return self.open_shortcuts(),
            SAct::Button(b) => self.state.message = Some(off_text(b)),
            // The Info page's boxes (spec: ui/settings.md "Info").
            SAct::Info(f) => return self.info_act(f),
            // File ▸ Browser extra search folders (`crate::search_folders`).
            SAct::FolderPick(i) => return self.search_folder_pick(i),
            SAct::FolderCell(i, col) => return self.search_folder_cell(i, col),
            SAct::Genres(x, y) => return self.genre_menu(x, y),
        }
        self.invalidate();
    }

    /// The automation target, name and current value of a control, for "Create automation clip" (§8).
    pub(crate) fn automation_target(&self, a: &Action) -> Option<(u32, String, f64)> {
        use os_model::target;
        let s = &self.state;
        let chan = |row: usize| s.project.channels.get(row);
        let chan_name = |row: usize| chan(row).and_then(|c| c.name.clone()).unwrap_or_else(|| format!("Channel {}", row + 1));
        let mix_name = |t: u16| {
            s.project.mixer.iter().find(|m| m.index == t).and_then(|m| m.name.clone()).unwrap_or_else(|| if t == 0 { "Master".into() } else { format!("Insert {t}") })
        };
        Some(match *a {
            Action::Knob(crate::knobs::KnobId::ChanVol(row)) => {
                (target::channel(chan(row)?.index, target::CHANNEL_VOLUME), format!("{} - Channel volume", chan_name(row)), chan(row)?.volume)
            }
            Action::Knob(crate::knobs::KnobId::ChanPan(row)) => {
                (target::channel(chan(row)?.index, target::CHANNEL_PAN), format!("{} - Channel panning", chan_name(row)), 0.5 + chan(row)?.pan / 2.0)
            }
            Action::Knob(crate::knobs::KnobId::MixPan(t)) => {
                let pan = s.project.mixer.iter().find(|m| m.index == t).and_then(|m| m.pan).unwrap_or(0.0);
                (target::mixer(t, target::MIXER_PAN), format!("{} - Pan", mix_name(t)), 0.5 + pan / 2.0)
            }
            Action::Fader(t) => {
                let v = s.project.mixer.iter().find(|m| m.index == t).and_then(|m| m.volume).unwrap_or(0.8);
                (target::mixer(t, target::MIXER_VOLUME), format!("{} - Volume", mix_name(t)), v)
            }
            Action::Tempo => (target::TEMPO, "Tempo".into(), crate::playlist::tempo_value(s.tempo)),
            Action::Knob(k @ crate::knobs::KnobId::Eq(t, b, f)) => {
                s.project.mixer.iter().find(|m| m.index == t)?;
                let param = [target::EQ_GAIN, target::EQ_FREQ, target::EQ_WIDTH][usize::from(f.min(2))] + b.min(2);
                let text = s.knob_text(k);
                let label = text.split_once(':').map_or(text.as_str(), |(a, _)| a);
                (target::mixer(t, param), format!("{} - {label}", mix_name(t)), f64::from(s.knob(k)))
            }
            _ => return None,
        })
    }

    /// Repaints the current track's equaliser: graph, faders and knobs (a change to one moves the others).
    fn damage_eq(&mut self) {
        use crate::{knobs::KnobId, mixer::MHit};
        let eq = |a: &Action| matches!(a, Action::Knob(KnobId::Eq(..)) | Action::Mix(MHit::EqGraph));
        match self.hits.iter().filter(|h| eq(&h.action)).map(|h| h.rect).reduce(|a, b| a.union(&b)) {
            Some(r) => self.damage_rect(r.outset(8.0)),
            None => self.invalidate(),
        }
    }

    pub(crate) fn top_hit(&self, x: f32, y: f32) -> Option<&Hit> {
        self.hits.iter().rev().find(|h| h.rect.contains(x, y))
    }

    /// Mouse moved; returns whether a repaint is needed.
    pub fn mouse_move(&mut self, x: f32, y: f32) -> bool {
        self.mouse = (x, y);
        self.hold_moved(x, y);
        if let Some(d) = self.drag.clone() {
            match d {
                Drag::Browser { item, label, m0, started } => self.browser_drag_move(label, m0, started, item),
                Drag::Pl(d) => {
                    if let Some(g) = self.pl_grid {
                        crate::playlist::drag_move(self, d, g);
                    }
                }
                Drag::BarZoom { t, m0, p0, ratio, z0, center, view } => {
                    let z = z0 * 1.012f32.powf(m0.1 - y);
                    self.zoom_set(t, z);
                    let k = self.zoom_get(t).map_or(1.0, |z| z / z0);
                    // Keep the content that was centred in view, then add the horizontal drag.
                    self.scroll_set(t, (center * k - view / 2.0) + (x - m0.0) * ratio * k);
                    let _ = p0;
                    self.damage_win(scroll_win(t));
                }
                Drag::BarRows { t, last, row } => {
                    // Only when it moved up/down: the clamp uses last frame's bar, stale once the rows were resized.
                    if let Some(b) = self.bar(t).filter(|_| y != last.1) {
                        self.scroll_set(t, self.scroll_get(t) + (y - last.1) * b.ratio());
                    }
                    let row = row.map(|r| {
                        let r = r * BAR_ROW_STEP.powf((x - last.0) / BAR_ROW_PX);
                        self.set_rows(t, r);
                        r
                    });
                    self.drag = Some(Drag::BarRows { t, last: (x, y), row });
                    self.damage_win(scroll_win(t));
                }
                Drag::PianoRowZoom { y0, row0 } => {
                    if let Some(g) = self.piano_geom() {
                        let row = row0 * piano::ROW_ZOOM_STEP.powf((y0 - y) / piano::ROW_ZOOM_PX);
                        piano::set_rows(&mut self.state, g.grid, y0, row);
                        self.damage_win(Win::PianoRoll);
                    }
                }
                Drag::Scroll { m0, axes } => {
                    for (t, vertical, p0, ratio) in axes {
                        let d = if vertical { y - m0.1 } else { x - m0.0 };
                        let v = p0 + d * ratio;
                        if matches!(t, Scroll::PlaylistX | Scroll::PlaylistY) {
                            // In whole steps the drawn pixels can move by (`playlist_scrolled`).
                            let before = self.state.playlist_scroll;
                            let cur = self.scroll_get(t);
                            let v = blit_step(self.px_scale).map_or(v, |q| cur + ((v - cur) / q).round() * q);
                            self.scroll_set(t, v);
                            self.playlist_scrolled(before);
                        } else {
                            self.scroll_set(t, v);
                            self.damage_win(scroll_win(t));
                        }
                    }
                }
                Drag::NoteMove { x0, y0, orig } => {
                    self.drag_notes(x0, y0, &orig, false);
                    self.state.piano.dirty = true;
                }
                Drag::NoteResize { x0, orig } => {
                    self.drag_notes(x0, 0.0, &orig, true);
                    self.state.piano.dirty = true;
                }
                Drag::NoteErase => self.erase_at(x, y),
                Drag::NoteMute { on } => self.mute_at(x, y, on),
                Drag::NotePaint { key, start, len } => self.paint_notes(key, start, len),
                Drag::Marquee { x0, y0, base } => {
                    let r = Rect::span(x0.min(x), x0.max(x), y0.min(y), y0.max(y));
                    self.state.piano.overlay = Some(Overlay::Marquee(r));
                    if let Some(g) = self.piano_geom() {
                        let s = &mut self.state;
                        let mut sel = base.clone();
                        sel.extend(piano::notes_in(s, &g, r).into_iter().filter(|i| !base.contains(i)));
                        if let Some(ch) = piano::channel(s) {
                            s.piano.select(s.pattern, ch, sel);
                        }
                    }
                    self.damage_piano();
                }
                Drag::Slice => {
                    if let Some(g) = self.piano_geom() {
                        let t = crate::piano_snap::place(&self.state, &g, g.tick_at(x), self.alt);
                        self.state.piano.overlay = Some(Overlay::Slice(t));
                    }
                    self.damage_piano();
                }
                Drag::ZoomBox { x0, y0 } => {
                    self.state.piano.overlay = Some(Overlay::ZoomBox(Rect::span(x0.min(x), x0.max(x), y0.min(y), y0.max(y))));
                    self.damage_piano();
                }
                Drag::Marker => {
                    if let Some(g) = self.piano_geom() {
                        self.state.piano.marker = crate::piano_snap::place(&self.state, &g, g.tick_at(x), self.alt);
                    }
                    self.damage_piano();
                }
                Drag::SongRuler => {
                    if let Some(g) = self.piano_geom() {
                        let t = crate::piano_snap::place(&self.state, &g, g.tick_at(x), self.alt);
                        self.piano_song_seek(t, false);
                    }
                    self.damage_piano();
                }
                Drag::TimeSel { t0 } => {
                    if let Some(g) = self.piano_geom() {
                        let t = crate::piano_snap::place(&self.state, &g, g.tick_at(x), self.alt);
                        piano::select_range(&mut self.state, t0.min(t), t0.max(t));
                    }
                    self.damage_piano();
                }
                Drag::Scrub { tick } => {
                    if let Some(g) = self.piano_geom() {
                        let t = g.tick_at(x).max(0.0);
                        self.scrub(tick, t);
                        self.drag = Some(Drag::Scrub { tick: t });
                        self.state.piano.overlay = Some(Overlay::Scrub(t));
                    }
                    self.damage_piano();
                }
                Drag::Velocity { last_x, last_y } => {
                    self.state.piano.dirty = true;
                    self.paint_velocity(last_x, last_y, x, y);
                    self.drag = Some(Drag::Velocity { last_x: x, last_y: y });
                }
                Drag::ControlErase { last_x } => {
                    self.erase_control_points(last_x, x);
                    self.drag = Some(Drag::ControlErase { last_x: x });
                }
                Drag::PianoKeys { key } => {
                    if let Some(g) = self.piano_geom() {
                        let k = g.key_at(y).clamp(0, views::piano::TOP_KEY);
                        if k != key {
                            self.drag = Some(Drag::PianoKeys { key: k });
                            self.key_velocity = self.key_click_velocity(x);
                            self.hold_key(Some(k), true);
                        }
                    }
                }
                Drag::Window { win, dx, dy } => {
                    let old = self.win_bounds(win);
                    // The window keeps its part in the workspace, whatever "Don't limit windows to screen" says.
                    crate::win_clamp::drag_to(&mut self.state, win, (x - dx, y - dy));
                    let new = self.win_bounds(win);
                    self.damage_rect(old.union(&new));
                }
                Drag::WinResize { win, edges, m0, r0 } => {
                    let old = self.win_bounds(win);
                    let (dx, dy) = (x - m0.0, y - m0.1);
                    let (mw, mh) = win.min_size();
                    let mut r = r0;
                    if edges & EDGE_L != 0 {
                        r.w = (r0.w - dx).max(mw);
                        r.x = r0.right() - r.w;
                    }
                    if edges & EDGE_R != 0 {
                        r.w = (r0.w + dx).max(mw);
                    }
                    if edges & EDGE_T != 0 {
                        r.h = (r0.h - dy).max(mh).min((r0.bottom() - views::work_y(&self.state)).max(mh));
                        r.y = r0.bottom() - r.h;
                    }
                    if edges & EDGE_B != 0 {
                        r.h = (r0.h + dy).max(mh);
                    }
                    if let Some(i) = win.slot() {
                        self.state.wins[i].rect = r;
                    }
                    let new = self.win_bounds(win);
                    self.damage_rect(old.union(&new));
                }
                Drag::RackResize { edges, m0, s0 } => {
                    let old = views::rack::bounds(&self.state);
                    let s = &mut self.state;
                    let w = if edges & views::rack::EDGE_RIGHT != 0 { s0.0 + x - m0.0 } else { s0.0 };
                    let h = if edges & views::rack::EDGE_BOTTOM != 0 { s0.1 + y - m0.1 } else { s0.1 };
                    s.rack_size = views::rack::snap_size(s, w, h);
                    let new = views::rack::bounds(&self.state);
                    self.damage_rect(old.union(&new));
                }
                Drag::RackMixer { ch, y0, v0 } => {
                    // One insert per 10 px, up = higher (spec: ui/channel-rack.md §12).
                    let v = (i32::from(v0) + ((y0 - y) / crate::rack::MIXER_DRAG).round() as i32).clamp(0, i32::from(crate::mixer::max_insert(&self.state.project))) as u16;
                    let s = &mut self.state;
                    if let Some(c) = s.project.channels.get_mut(ch) {
                        if c.mixer_track != v {
                            c.mixer_track = v;
                            s.piano.dirty = true;
                        }
                    }
                    self.hint = Cow::Owned(format!("Target mixer track: {}", views::rack::insert_name(v)));
                    let r = self.hits.iter().rev().find(|h| h.action == Action::RackMixer(ch)).map(|h| h.rect);
                    match r {
                        Some(r) => self.damage_rect(r.outset(2.0)),
                        None => self.invalidate(),
                    }
                    self.damage_rect(HINT_RECT);
                }
                Drag::RackPaint { row, on, last } => {
                    if let Some(k) = views::rack::step_at(&self.state, x) {
                        if k != last {
                            for j in last.min(k)..=last.max(k) {
                                crate::rack::set_step(&mut self.state, row, j, on);
                            }
                            self.drag = Some(Drag::RackPaint { row, on, last: k });
                            self.invalidate();
                        }
                    }
                }
                Drag::RackSel { from } => {
                    if let Some(r) = views::rack::row_at(&self.state, y) {
                        let shown = crate::rack::shown(&self.state);
                        if let (Some(a), Some(b)) = (shown.iter().position(|&x| x == from), shown.iter().position(|&x| x == r)) {
                            self.state.rack.sel = shown[a.min(b)..=a.max(b)].iter().copied().collect();
                        }
                    }
                    self.invalidate();
                }
                Drag::RackDots { x0, p0 } => {
                    let w = p0 as f32 / 100.0 * crate::rack::NAME_W + (x - x0);
                    let pct = (w / crate::rack::NAME_W * 100.0).round().clamp(30.0, 400.0) as u32;
                    crate::rack::relayout(&mut self.state, |s| s.rack.name_pct = pct);
                    self.hint = Cow::Owned(format!("Channel button width: {pct}%"));
                    self.invalidate();
                }
                Drag::RackLen { y0, v0 } => {
                    let v = (v0 as i32 + ((y0 - y) / crate::rack::LEN_DRAG).round() as i32).clamp(0, crate::rack::max_steps(&self.state) as i32) as u32;
                    crate::rack::set_len(&mut self.state, (v > 0).then_some(v));
                    self.hint = Cow::Owned(if v == 0 { "Pattern length: Auto".into() } else { format!("Pattern length: {v} steps") });
                    self.invalidate();
                }
                Drag::RackLoopLen { row, y0, v0 } => {
                    let v = (v0 as i32 + ((y0 - y) / crate::rack::LEN_DRAG).round() as i32).clamp(1, crate::rack::MAX_STEPS as i32) as u32;
                    self.state.rack.loop_len.insert(row, v);
                    self.state.piano.dirty = true;
                    self.hint = Cow::Owned(format!("Loop point: {v} steps"));
                    self.invalidate();
                }
                Drag::RackGraph => self.rack_graph_at((x, y)),
                Drag::Dialog { dx, dy } => self.dialog_drag(x - dx, y - dy),
                Drag::DialogBar { y0, s0, ratio } => self.dialog_scroll(s0 + (y - y0) * ratio),
                Drag::PatternBox { y0, p0 } => {
                    let (y0, p0): (f32, u16) = (y0.to_owned(), p0.to_owned());
                    let steps = ((y0 - y) / crate::patterns::DRAG_STEP).trunc() as i32;
                    self.state.pattern = (i32::from(p0) + steps).clamp(1, i32::from(crate::patterns::MAX)) as u16;
                    self.invalidate();
                }
                Drag::MixSelect => {
                    // The strip whose column the cursor is in, whatever its height.
                    let under = self.hits.iter().rev().find_map(|h| match h.action {
                        Action::Mix(crate::mixer::MHit::Strip(n)) if x >= h.rect.x && x < h.rect.right() => Some(n),
                        _ => None,
                    });
                    if let Some(n) = under.filter(|&n| n != self.state.selected_mixer) {
                        crate::mixer::select(&mut self.state, n, false);
                        self.damage_win(Win::Mixer);
                    }
                }
                Drag::Fader { track, y0, v0 } => {
                    let per_px = views::mixer::FADER_PER_PX / f64::from(views::mixer::scale(self.state.win_rect(Win::Mixer)));
                    let v = (v0 - f64::from(y - y0) * per_px).clamp(0.0, 1.0);
                    if let Some(t) = self.state.project.mixer.iter_mut().find(|t| t.index == track) {
                        t.volume = Some(v);
                        self.state.piano.dirty = true;
                    }
                    self.hint = Cow::Owned(crate::mixer::level_text("Volume", v));
                    let col = self.hits.iter().rev().find(|h| h.action == Action::Fader(track)).map(|h| h.rect);
                    match col {
                        Some(r) => self.damage_rect(r.outset(10.0)),
                        None => self.invalidate(),
                    }
                    self.damage_rect(HINT_RECT);
                }
                Drag::Knob { id, y0, v0 } => {
                    self.state.set_knob(id, v0 - (y - y0) * id.drag_rate());
                    self.hint = Cow::Owned(self.state.knob_text(id));
                    match self.hits.iter().rev().find(|h| h.action == Action::Knob(id)).map(|h| h.rect) {
                        Some(r) => self.damage_rect(r.outset(6.0)),
                        None => self.invalidate(),
                    }
                    if let crate::knobs::KnobId::Eq(..) = id {
                        self.damage_eq();
                    }
                    self.damage_rect(HINT_RECT);
                }
                Drag::EqGraph { track, band, g } => {
                    crate::mixer::eq_graph_drag(&mut self.state, track, band, g, (x, y));
                    self.hint = Cow::Owned(crate::mixer::eq_graph_hint(&self.state, g, x));
                    self.damage_eq();
                    self.damage_rect(HINT_RECT);
                }
                Drag::MonParam(i) => {
                    self.set_mon_param(i, x);
                    return true;
                }
                Drag::CableParam(i) => {
                    self.set_cable_param(i, x);
                    return true;
                }
                Drag::SongPos { dx } => self.song_pos_drag(dx),
                Drag::PianoSplit { dy, top, h } => {
                    let split = self.state.piano.split.dragged(y - dy - top, h);
                    if split != self.state.piano.split {
                        self.state.piano.split = split;
                        self.damage_piano();
                    }
                }
                Drag::SongPosHold { .. } => {}
                Drag::PrefSlider { label, x: x0, w } => self.pref_slider_drag(label, x0, w),
                Drag::Ppi { x0, n0 } => self.ppi_set(crate::ppi::dragged(n0, x - x0, self.px_scale)),
                Drag::Tempo { y0, t0, step } => {
                    // Rounded to the box's thousandths, so repeated steps don't drift.
                    let t = self.set_tempo(((t0 + f64::from(y0 - y).round() * step) * 1000.0).round() / 1000.0);
                    self.hint = Cow::Owned(format!("Tempo {t:.3} BPM"));
                    self.damage_rect(TEMPO_RECT);
                    self.damage_rect(HINT_RECT);
                }
            }
            return true;
        }
        let (hover, mut hint, rect) = match self.top_hit(x, y) {
            Some(h) => (Some(h.action.clone()), h.hint.clone(), Some(h.rect)),
            None => (None, Cow::Borrowed(""), None),
        };
        if hover == Some(Action::Monitor) {
            // Reading under the cursor: frequency and level of the analyzer column.
            hint = Cow::Owned(self.state.monitor.hint((x - MONITOR_RECT.x - 1.0).max(0.0) as usize));
        }
        if hover == Some(Action::CpuPanel) {
            hint = Cow::Owned(self.state.cpu.hint());
        }
        if let (Some(Action::PianoKeys), Some(g)) = (&hover, self.piano_geom()) {
            // The key under the cursor: "C5 / 60" (spec: ui/settings-choices.md "Note names and Black notes").
            hint = Cow::Owned(crate::note_names::key_hint(g.key_at(y).clamp(0, views::piano::TOP_KEY)));
        }
        if let (Some(Action::Mix(crate::mixer::MHit::EqGraph)), Some(g)) = (&hover, rect) {
            hint = Cow::Owned(crate::mixer::eq_graph_hint(&self.state, g, x));
        }
        let resize = matches!(hover, Some(Action::PianoNote(_)))
            && matches!(self.state.piano.tool, Tool::Draw | Tool::Paint | Tool::Select)
            && rect.is_some_and(|r| x >= r.right() - resize_zone(r.w));
        // Playlist: the clip under the mouse (automation clips show their points) and the resize cursor on clip edges.
        let mut resize = resize;
        if hover == Some(Action::PlaylistArea) {
            if let Some(g) = self.pl_grid {
                let s = &self.state;
                let pg = crate::playlist::PGeom::new(s, g);
                let hit = crate::playlist::clip_at(s, &pg, x, y);
                // Automation clips show their points while hovered, audio clips their fade and gain handles (§9a).
                let auto = hit.filter(|&i| {
                    let c = &crate::playlist::clips(s)[i];
                    matches!(c.source, os_model::ClipSource::Channel(ch) if s.project.automation(ch).is_some()) || crate::playlist::is_audio_clip(s, &pg, c)
                });
                let handle = hit.and_then(|i| crate::playlist::handle_at(s, &pg, i, x, y));
                if auto != self.state.playlist.hover_clip || handle != self.state.playlist.hover_handle {
                    // Repaint only the clips that lost and gained them (or whose lit handle changed).
                    for i in [self.state.playlist.hover_clip, auto].into_iter().flatten() {
                        match crate::playlist::clips(&self.state).get(i) {
                            Some(c) => self.damage_rect(pg.clip_rect(c).outset(6.0)),
                            None => self.invalidate(),
                        }
                    }
                    self.state.playlist.hover_clip = auto;
                    self.state.playlist.hover_handle = handle;
                }
                if let (Some(i), Some(h)) = (hit, handle) {
                    hint = Cow::Owned(crate::playlist::handle_hint(&crate::playlist::clips(&self.state)[i], h));
                } else if let Some(i) = hit {
                    let r = pg.clip_rect(&crate::playlist::clips(&self.state)[i]);
                    let z = crate::playlist::EDGE.min(r.w / 4.0);
                    let tool_resizes = matches!(self.state.playlist.tool, crate::playlist::PlTool::Draw | crate::playlist::PlTool::Paint | crate::playlist::PlTool::Select);
                    resize |= tool_resizes && (x <= r.x + z || x >= r.right() - z);
                    if hint.is_empty() || hint == "Playlist" {
                        hint = Cow::Owned(crate::playlist::source_name(&self.state, crate::playlist::clips(&self.state)[i].source));
                    }
                }
            }
        } else if let Some(i) = self.state.playlist.hover_clip.take() {
            self.state.playlist.hover_handle = None;
            let r = self.pl_grid.zip(crate::playlist::clips(&self.state).get(i)).map(|(g, c)| crate::playlist::PGeom::new(&self.state, g).clip_rect(c));
            match r {
                Some(r) => self.damage_rect(r.outset(6.0)),
                None => self.invalidate(),
            }
        }
        let changed = hover != self.hover || hint != self.hint || resize != self.resize_cursor;
        self.resize_cursor = resize;
        if changed {
            // Repaint the controls that lost / gained hover and the hint panel; large background
            // regions have no hover look.
            let mut dirty = Vec::new();
            for (a, r) in [(&self.hover, self.hover_rect), (&hover, rect)] {
                if let (Some(a), Some(r)) = (a, r) {
                    if hover_is_visible(a) {
                        dirty.push(r.outset(3.0));
                    }
                }
            }
            for r in dirty {
                self.damage_rect(r);
            }
            if hint != self.hint {
                self.damage_rect(HINT_RECT);
            }
        }
        self.hover = hover;
        self.hover_rect = rect;
        self.hint = hint;
        changed
    }

    /// Window buttons act on the release over the same button instead of on the press, like the system's own (owner,
    /// 2026-10-15: acting on the press made accidental closes easy): the main window's minimize, maximize / restore and
    /// exit, the child windows' minimize, maximize and close, the × of a dialog, the Settings window and the Advanced
    /// fill window. The press arms one; moving off it before releasing lets it go.
    fn acts_on_release(a: &Action) -> bool {
        matches!(
            a,
            Action::Minimize
                | Action::Maximize
                | Action::Close
                | Action::MinWin(_)
                | Action::MaxWin(_)
                | Action::CloseWin(_)
                | Action::Dlg(crate::dialog::DHit::Close)
                | Action::Settings(crate::views::settings::SAct::Close)
                | Action::RackFill(crate::rack::FillHit::Close)
        )
    }

    pub fn mouse_down(&mut self) {
        self.invalidate();
        self.hold = None;
        let (x, y) = self.mouse;
        let action = self.top_hit(x, y).map(|h| h.action.clone());
        // A press stops the views easing where they are drawn; a scroll bar's arrow steps on from where its ease is
        // going (`crate::smooth`).
        if !matches!(action, Some(Action::ScrollStep(..))) {
            self.smooth.stop();
        }
        self.pressed = action.clone();
        self.armed = None;
        self.armed_right = None;
        // A dialog takes every click (spec: ui/dialogs.md §3).
        if self.modal_open() {
            // The time signature dialog's value list (the app's popup) takes its own; a click anywhere else closes it.
            if crate::timesig::list_open(&self.state).is_some() {
                match action {
                    Some(a @ (Action::PlPopup(_) | Action::PlPopupSub(_))) => {
                        self.act(a);
                    }
                    _ => self.state.playlist.popup = None,
                }
                return;
            }
            if let Some(Action::Dlg(h)) = action {
                if Self::acts_on_release(&Action::Dlg(h.clone())) {
                    self.armed = Some(Action::Dlg(h));
                } else {
                    self.dialog_click(h);
                }
            }
            return;
        }
        // The value box is modal: only its own parts take clicks (spec: ui/type-in-value.md §1).
        if crate::typein::is_open(&self.state) {
            if let Some(Action::TypeIn(a)) = action {
                crate::typein::click(&mut self.state, a);
            }
            return;
        }
        // Playlist popup menus close on any click outside them; a click elsewhere accepts an open text box.
        if self.state.playlist.popup.is_some() && !matches!(action, Some(Action::PlPopup(_) | Action::PlPopupSub(_))) {
            self.state.playlist.popup = None;
            // A dropdown's submenu: the click goes on to the dropdown (below), but one on the submenu's own ▸ entry only
            // closes it.
            match self.state.menu_sub.take() {
                Some(i) if self.state.menu_open.is_some() && !matches!(action, Some(Action::MenuSub(_, j)) if j == i) => {}
                _ => return,
            }
        }
        // The pattern name box / delete confirmation are modal: a click outside accepts the box.
        if (self.state.pat_ui.name_box.is_some() || self.state.pat_ui.confirm) && !matches!(action, Some(Action::Pat(_))) {
            if self.state.pat_ui.name_box.is_some() {
                crate::patterns::accept(&mut self.state);
            }
            return;
        }
        // The Advanced fill dialog is modal (spec: ui/channel-rack.md §22).
        if self.state.rack.fill.is_some() && !matches!(action, Some(Action::RackFill(_))) {
            return;
        }
        if self.state.playlist.edit.is_some() {
            crate::playlist::commit_edit(&mut self.state);
        }
        // A click anywhere but the Info box being edited accepts it (spec: ui/settings.md "Info").
        if self.state.settings.info_edit.as_ref().is_some_and(|e| action != Some(Action::Settings(views::settings::SAct::Info(e.field)))) {
            crate::project_info::accept_info(&mut self.state);
        }
        // The same for a cell of File ▸ Browser extra search folders.
        if self.state.settings.folder_edit.as_ref().is_some_and(|e| action != Some(Action::Settings(views::settings::SAct::FolderCell(e.row, e.col)))) {
            crate::search_folders::commit(&mut self.state);
        }
        // Clicking anywhere but the search field leaves it (spec: ui/browser.md §5).
        if action != Some(Action::BrowserSearch) {
            self.state.browser_search_focus = false;
        }
        // The same for the monitor's Find box (spec: ui/plugin-performance.md §1).
        if action != Some(Action::Perf(crate::perf::PerfAct::Find)) {
            self.state.perf.finding = false;
        }
        if self.state.lane_menu && !matches!(action, Some(Action::PianoLanePick(_))) {
            self.state.lane_menu = false;
            return;
        }
        // Any click outside an open menu closes it. VIEW's window entries are window toggles: one clicked in the menu
        // closes the menu and toggles its window. Its open submenu takes its own clicks.
        if self.state.menu_open.is_some() && !matches!(action, Some(Action::MenuItem(..) | Action::MenuSub(..) | Action::Menu(_) | Action::PlPopup(_) | Action::PlPopupSub(_))) {
            self.state.menu_open = None;
            if !(matches!(action, Some(Action::ToggleWin(_))) && self.overlays.iter().any(|r| r.contains(x, y))) {
                return;
            }
        }
        self.raise_under_mouse();
        if let Some(a) = action {
            if Self::acts_on_release(&a) {
                self.armed = Some(a);
            } else {
                self.act(a);
            }
        }
        self.rack_pending();
    }

    /// Clicking any part of a child window brings it to the front (spec: ui/channel-rack.md §5).
    fn raise_under_mouse(&mut self) {
        let (x, y) = self.mouse;
        if let Some(w) = self.top_hit(x, y).and_then(|h| h.win) {
            if self.state.z.last() != Some(&w) {
                self.state.raise(w);
                self.invalidate();
            }
            self.state.no_focus = false;
        } else if self.state.work_rect().contains(x, y) {
            self.state.no_focus = true;
        }
    }

    /// Area a floating window paints (for damage).
    fn win_bounds(&self, w: Win) -> Rect {
        match w {
            Win::Plugin => views::channel_win::bounds(&self.state),
            Win::Perf => views::perf::bounds(&self.state),
            Win::Tapper => views::tapper::bounds(&self.state),
            Win::ChannelRack => views::rack::bounds(&self.state),
            w => self.state.win_rect(w).outset(5.0),
        }
    }

    /// Cursor for the current hover / drag.
    pub fn cursor(&self) -> Cursor {
        let win_edges = match (&self.drag, &self.hover) {
            (Some(Drag::WinResize { edges, .. }), _) => *edges,
            (None, Some(Action::WinResize(_, e))) => *e,
            _ => 0,
        };
        if win_edges != 0 {
            return match win_edges {
                EDGE_L | EDGE_R => Cursor::EwResize,
                EDGE_T | EDGE_B => Cursor::NsResize,
                e if e == EDGE_L | EDGE_T || e == EDGE_R | EDGE_B => Cursor::NwseResize,
                _ => Cursor::NeswResize,
            };
        }
        if matches!((&self.drag, &self.hover), (Some(Drag::PianoSplit { .. }), _) | (None, Some(Action::PianoSplitter))) {
            return Cursor::NsResize;
        }
        let edges = match (&self.drag, &self.hover) {
            (Some(Drag::RackResize { edges, .. }), _) => *edges,
            (None, Some(Action::RackResize(e))) => *e,
            _ => 0,
        };
        match edges {
            0 if self.resize_cursor => Cursor::EwResize,
            0 => Cursor::Default,
            views::rack::EDGE_RIGHT => Cursor::EwResize,
            views::rack::EDGE_BOTTOM => Cursor::NsResize,
            _ => Cursor::NwseResize,
        }
    }

    /// True while a drag moves a value rather than the cursor (knobs, the tempo, scroll-bar right-drags): the shell
    /// hides the cursor and keeps it where the drag started, feeding relative movement.
    pub fn cursor_locked(&self) -> bool {
        matches!(self.drag, Some(Drag::BarZoom { .. } | Drag::BarRows { .. } | Drag::Knob { .. } | Drag::Tempo { .. }))
    }

    pub(crate) fn bar(&self, t: Scroll) -> Option<Bar> {
        self.bars.iter().find(|b| b.target == t).copied()
    }

    /// Scroll value of an axis in content pixels.
    pub fn scroll_get(&self, t: Scroll) -> f32 {
        let s = &self.state;
        match t {
            Scroll::PianoX => s.piano.scroll_x * s.piano.px_per_beat / f32::from(s.project.ppq.max(4)),
            Scroll::PianoY => s.piano.scroll_y,
            Scroll::PitchY => piano::PITCH_RANGE as f32 - (s.piano.pitch_center as f32 + 241.0),
            Scroll::PlaylistX => s.playlist_scroll.0,
            Scroll::PlaylistY => s.playlist_scroll.1,
            Scroll::MixerX => s.mixer_scroll,
            Scroll::RackX => s.rack_scroll_x,
            Scroll::RackY => s.rack_scroll,
            Scroll::PlPick => s.playlist.pick_scroll,
        }
    }

    /// Set an axis, clamped to what its scroll bar allows (last frame).
    pub fn scroll_set(&mut self, t: Scroll, v: f32) {
        let unbounded = if matches!(t, Scroll::RackX | Scroll::RackY) { 0.0 } else { f32::MAX };
        let max = self.bar(t).map_or(unbounded, |b| b.max);
        let v = v.clamp(0.0, max.max(0.0));
        let s = &mut self.state;
        match t {
            Scroll::PianoX => s.piano.scroll_x = v / (s.piano.px_per_beat / f32::from(s.project.ppq.max(4))),
            Scroll::PianoY => s.piano.scroll_y = v.round(),
            Scroll::PitchY => s.piano.pitch_center = piano::clamp_pitch_center((piano::PITCH_RANGE as f32 - 241.0 - v).round() as i32),
            Scroll::PlaylistX => s.playlist_scroll.0 = v,
            Scroll::PlaylistY => s.playlist_scroll.1 = v,
            Scroll::MixerX => s.mixer_scroll = v,
            Scroll::RackX => s.rack_scroll_x = v,
            Scroll::RackY => s.rack_scroll = v,
            Scroll::PlPick => s.playlist.pick_scroll = v,
        }
    }

    /// Zoom of an axis in pixels per content unit, where it can zoom.
    fn zoom_get(&self, t: Scroll) -> Option<f32> {
        match t {
            Scroll::PianoX => Some(self.state.piano.px_per_beat),
            Scroll::PlaylistX => Some(self.state.playlist_bar_w),
            _ => None,
        }
    }

    fn zoom_set(&mut self, t: Scroll, z: f32) {
        match t {
            Scroll::PianoX => self.state.piano.px_per_beat = z.clamp(6.0, 1600.0),
            Scroll::PlaylistX => self.state.playlist_bar_w = z.clamp(4.0, 600.0),
            _ => {}
        }
    }

    /// Row height of a vertical scroll bar's content, where its rows can be resized (spec: ui/scroll-bars.md).
    fn rows_get(&self, t: Scroll) -> Option<f32> {
        match t {
            Scroll::PianoY => Some(self.state.piano.row_h),
            Scroll::PlaylistY => Some(self.state.playlist.row_h),
            _ => None,
        }
    }

    /// Resize a vertical scroll bar's rows to `row` (clamped and rounded by the view), keeping the row at the view
    /// centre in place.
    fn set_rows(&mut self, t: Scroll, row: f32) {
        match t {
            Scroll::PianoY => {
                if let Some(g) = self.piano_geom() {
                    let at = g.grid.cy();
                    piano::set_rows(&mut self.state, g.grid, at, row);
                }
            }
            Scroll::PlaylistY => {
                if let Some(g) = self.pl_grid {
                    crate::playlist::set_row_h(&mut self.state, g, g.cy(), row);
                }
            }
            _ => {}
        }
    }

    /// Press on a scroll bar. Left: a press off the thumb first centres the thumb on the mouse,
    /// then dragging scrolls. Right: the cursor is held; on a horizontal bar sideways scrolls and up/down zooms,
    /// on a vertical bar up/down scrolls and sideways resizes the rows (spec: ui/scroll-bars.md).
    fn scrollbar_press(&mut self, t: Scroll, right: bool) {
        let Some(b) = self.bar(t) else { return };
        let (x, y) = self.mouse;
        let along = if b.vertical { y - b.track.y } else { x - b.track.x };
        let pos = self.scroll_get(t);
        let thumb0 = if b.max > 0.0 { pos / b.max * (b.len() - b.thumb) } else { 0.0 };
        if !right && (along < thumb0 || along > thumb0 + b.thumb) {
            self.scroll_set(t, (along - b.thumb / 2.0) * b.ratio());
        }
        let p0 = self.scroll_get(t);
        self.drag = Some(if right && b.vertical {
            Drag::BarRows { t, last: (x, y), row: self.rows_get(t) }
        } else if right {
            let z0 = self.zoom_get(t).unwrap_or(1.0);
            Drag::BarZoom { t, m0: (x, y), p0, ratio: b.ratio(), z0, center: p0 + b.view / 2.0, view: b.view }
        } else {
            Drag::Scroll { m0: (x, y), axes: vec![(t, b.vertical, p0, b.ratio())] }
        });
        self.invalidate();
    }

    /// Middle button: grab-scroll the Piano roll and Playlist (FL manual: middle-drag scrolls
    /// both directions).
    pub fn mouse_down_middle(&mut self) {
        if crate::typein::is_open(&self.state) || self.modal_open() {
            return;
        }
        self.smooth.stop();
        self.raise_under_mouse();
        let (x, y) = self.mouse;
        let action = self.top_hit(x, y).map(|h| h.action.clone());
        if let Some(Action::Mix(h)) = action {
            if crate::mixer::middle(self, h) {
                return self.invalidate();
            }
        }
        if let Some(Action::Knob(id)) = action {
            crate::history::knob(&mut self.state, id, false);
            self.state.set_knob(id, id.default_value());
            self.hint = Cow::Owned(self.state.knob_text(id));
            self.invalidate();
            return;
        }
        if self.ctrl && matches!(action, Some(Action::PianoGrid | Action::PianoNote(_) | Action::PianoKeys)) {
            self.drag = Some(Drag::PianoRowZoom { y0: y, row0: self.state.piano.row_h });
            return;
        }
        let targets: &[Scroll] = match action {
            Some(Action::PianoGrid | Action::PianoNote(_)) => &[Scroll::PianoX, Scroll::PianoY],
            Some(Action::PianoKeys) => &[Scroll::PianoY],
            Some(Action::PianoVelocity) => &[Scroll::PianoX],
            Some(Action::PlaylistArea) => &[Scroll::PlaylistX, Scroll::PlaylistY],
            _ => return,
        };
        let axes = targets.iter().map(|&t| (t, matches!(t, Scroll::PianoY | Scroll::PlaylistY), self.scroll_get(t), -1.0)).collect();
        self.drag = Some(Drag::Scroll { m0: (x, y), axes });
    }

    /// Right button: delete notes in the piano roll (spec: ui/piano-roll.md §8); right-drag on
    /// a scroll bar scrolls.
    pub fn mouse_down_right(&mut self) {
        if crate::typein::is_open(&self.state) || self.modal_open() {
            return;
        }
        self.smooth.stop();
        let (x, y) = self.mouse;
        let action = self.top_hit(x, y).map(|h| h.action.clone());
        if self.state.menu_open.is_some() {
            self.state.menu_open = None;
            // With its submenu.
            if self.state.menu_sub.take().is_some() {
                self.state.playlist.popup = None;
            }
            self.invalidate();
            return;
        }
        if self.state.pat_ui.name_box.is_some() || self.state.pat_ui.confirm {
            return;
        }
        // Pattern selector (spec: ui/pattern-menu.md §1): box = menu at the mouse, "+" = find next empty.
        match &action {
            Some(Action::PatternNext) => {
                self.state.pat_ui.menu_at = Some((x, y));
                self.state.menu_open = Some(views::menus::PATTERNS);
                return self.invalidate();
            }
            Some(Action::PatternAdd) => {
                crate::patterns::run(&mut self.state, crate::patterns::Cmd::FindNext, crate::patterns::PLUS_BOX_AT);
                return self.invalidate();
            }
            Some(Action::CpuPanel) => {
                self.cpu_menu(x, y);
                return self.invalidate();
            }
            Some(Action::Metronome) => {
                self.metronome_menu(x, y);
                return self.invalidate();
            }
            Some(Action::PianoTarget) => {
                // spec: ui/piano-target-channel.md §4
                let row = self.state.selected_channel;
                if row < self.state.project.channels.len() {
                    self.state.playlist.popup = None;
                    self.toggle_channel_window(row);
                }
                return self.invalidate();
            }
            Some(Action::PianoSnapMenu | Action::GlobalSnap) => {
                // spec: ui/piano-snap.md §7, ui/global-snap.md §4
                self.state.playlist.popup = Some(crate::piano_snap::lengths_menu(x, y));
                return self.invalidate();
            }
            Some(Action::AutoScroll) => {
                self.autoscroll_menu(x, y);
                return self.invalidate();
            }
            Some(Action::SongPos) => return self.song_pos_jump(),
            Some(Action::Clock) => {
                self.state.playlist.popup = Some(views::menus::clock_popup(&self.state, x, y));
                return self.invalidate();
            }
            // The Toolbars menu: on the hint panel and on bare toolbar background (spec: ui/toolbar-menu.md §1).
            Some(Action::DragWindow) => {
                self.state.playlist.popup = Some(views::menus::hint_popup(&self.state, x, y));
                return self.invalidate();
            }
            // Toolbar buttons with a right-click action: armed here, run on the release (spec: ui/toolbar-buttons.md §2).
            Some(a) if toolbar_right(a).is_some() => {
                self.state.playlist.popup = None;
                self.armed_right = toolbar_right(a).map(|run| (a.clone(), run));
                return self.invalidate();
            }
            Some(Action::BrowserRow(i)) => {
                if self.browser_remove_root(*i) || self.params_browser_right(*i, x, y) {
                    return;
                }
            }
            _ => {}
        }
        if self.state.playlist.popup.is_some() && !matches!(action, Some(Action::PlPopup(_) | Action::PlPopupSub(_))) {
            self.state.playlist.popup = None;
            self.invalidate();
            if !matches!(action, Some(Action::PlaylistArea)) {
                return;
            }
        }
        if self.state.playlist.edit.is_some() {
            crate::playlist::commit_edit(&mut self.state);
        }
        self.raise_under_mouse();
        if let Some(Action::Scrollbar(t)) = action {
            return self.scrollbar_press(t, true);
        }
        if let Some(a) = action.as_ref() {
            if self.rack_right(a) {
                return self.invalidate();
            }
        }
        if let Some(Action::Mix(h)) = action {
            if crate::mixer::right(self, h) {
                return self.invalidate();
            }
        }
        // Knobs, faders and the tempo: "Create automation clip" (spec: ui/playlist.md §8) and "Type in value..."
        // (spec: ui/type-in-value.md §8).
        if let Some(a) = action.as_ref() {
            let typein = match *a {
                Action::Knob(k) => Some(crate::typein::Target::Knob(k)),
                Action::Fader(t) => Some(crate::typein::Target::Fader(t)),
                _ => None,
            };
            let auto = self.automation_target(a);
            if auto.is_some() || typein.is_some() {
                self.state.playlist.popup = Some(crate::playlist::knob_menu(&self.state, auto, typein, x, y));
                return self.invalidate();
            }
            if self.playlist_act(a, true) {
                return;
            }
        }
        // Right-click the monitor: open the debug panel; on a debug slider: back to its default.
        // Monitor debug panel (views/mondebug.rs) parked 2026-10-09: its values are now the defaults. Uncomment to tune again.
        // if action == Some(Action::Monitor) {
        //     self.state.monitor.debug = !self.state.monitor.debug;
        //     return self.invalidate();
        // }
        if let Some(Action::MonParam(i)) = action {
            let d = crate::monitor::PARAMS[i].default;
            self.state.monitor.cfg.set(i, d);
            return self.damage_rect(views::mondebug::PANEL);
        }
        if let Some(Action::CableDebug(crate::cable::DebugAct::Param(i))) = action {
            self.state.cables.get_mut().set(i, crate::cable::PARAMS[i].default);
            self.damage_rect(views::cabledebug::PANEL);
            return self.damage_win(Win::Mixer);
        }
        if action == Some(Action::PianoRuler) {
            if let Some(g) = self.piano_geom() {
                let t = crate::piano_snap::place(&self.state, &g, g.tick_at(x), self.alt);
                self.state.piano.clear_selection();
                self.drag = Some(Drag::TimeSel { t0: t });
                self.damage_piano();
            }
            return;
        }
        if action == Some(Action::PianoVelocity) && self.state.piano.lane.is_channel() {
            piano::snapshot(&mut self.state);
            self.drag = Some(Drag::ControlErase { last_x: x });
            self.erase_control_points(x, x);
            return;
        }
        if let Some(a @ (Action::PianoNote(_) | Action::PianoGrid)) = action {
            // Select: same as the left button; Zoom: toggle zoomed out / last zoom; the other
            // tools delete while the right button is held (spec: ui/piano-roll.md §9).
            match self.state.piano.tool {
                Tool::Select => self.piano_click(&a),
                Tool::Zoom => self.zoom_toggle(),
                _ => {
                    piano::snapshot_as(&mut self.state, "Piano roll delete note");
                    self.drag = Some(Drag::NoteErase);
                    self.erase_at(x, y);
                }
            }
        }
    }

    pub fn mouse_up(&mut self) {
        self.hold = None;
        self.release();
        // An undo step whose press changed nothing goes (spec: ui/settings-choices.md "Undo history").
        crate::history::settle(&mut self.state);
    }

    fn release(&mut self) {
        // An armed window button acts if the release is over it (`acts_on_release`).
        if let Some(a) = self.armed.take() {
            let (x, y) = self.mouse;
            if self.top_hit(x, y).is_some_and(|h| h.action == a) {
                self.act(a);
                self.invalidate();
            }
        }
        if let Some((hit, run)) = self.armed_right.take() {
            let (x, y) = self.mouse;
            if self.top_hit(x, y).is_some_and(|h| h.action == hit) {
                self.act(run);
                self.invalidate();
            }
        }
        let pressed = self.pressed.take();
        // A click without a drag did its work on the press (painted then): its release only lifts the pressed look.
        if self.drag.is_none() {
            let rect = pressed.as_ref().and_then(|a| self.hits.iter().rev().find(|h| &h.action == a)).map(|h| h.rect);
            match (pressed, rect) {
                (None, _) => {}
                (Some(_), Some(r)) => self.damage_rect(r.outset(3.0)),
                (Some(_), None) => self.invalidate(),
            }
            self.hold_key(None, false);
            if self.state.piano.overlay.take().is_some() {
                self.damage_piano();
            }
            return;
        }
        if pressed.is_some() {
            self.invalidate();
        }
        self.hold_key(None, false);
        if matches!(self.drag, Some(Drag::Velocity { .. })) {
            self.state.piano.dirty = true;
        }
        if let Some(Drag::Browser { item, started, .. }) = self.drag.clone() {
            self.drag = None;
            return self.browser_drop(item, started);
        }
        if let Some(Drag::Pl(d)) = self.drag.clone() {
            self.drag = None;
            if let Some(g) = self.pl_grid {
                crate::playlist::drag_end(self, d, g);
            }
            return self.invalidate();
        }
        let had_overlay = self.state.piano.overlay.take().is_some();
        match self.drag.clone() {
            Some(Drag::Slice) => {
                if let Some(g) = self.piano_geom() {
                    let t = crate::piano_snap::place(&self.state, &g, g.tick_at(self.mouse.0), self.alt);
                    if piano::slice(&mut self.state, t) > 0 {
                        self.state.piano.dirty = true;
                    }
                }
            }
            Some(Drag::ZoomBox { x0, y0 }) => self.zoom_to(x0, y0),
            Some(Drag::SongPos { .. }) => self.song_pos_release(),
            Some(Drag::PianoSplit { .. }) => crate::piano_split::save(self.state.piano.split),
            Some(Drag::Marker) if self.transport.playing() && !self.transport.song() => {
                self.start_playback();
            }
            Some(Drag::SongRuler) => self.song_pos_release(),
            _ => {}
        }
        if had_overlay {
            self.damage_piano();
        }
        if matches!(self.drag, Some(Drag::NoteMove { .. } | Drag::NoteResize { .. } | Drag::NoteErase | Drag::NoteMute { .. } | Drag::NotePaint { .. })) {
            piano::finish_edit(&mut self.state);
            self.state.piano.dirty = true;
            self.damage_piano();
            self.drag = None;
            return;
        }
        self.invalidate();
        self.drag = None;
    }

    /// Move (`resize == false`) or resize the dragged notes from their originals; the distance follows the snap
    /// (spec: ui/piano-snap.md §5, §8), measured from the grabbed note's start, or its end for a resize.
    pub(crate) fn drag_notes(&mut self, x0: f32, y0: f32, orig: &[(usize, Note)], resize: bool) {
        let Some(g) = self.piano_geom() else { return };
        let (x, y) = self.mouse;
        let unit = crate::piano_snap::unit(&self.state).unwrap_or(1);
        let from = orig.first().map_or(0, |&(_, o)| if resize { o.position + o.length.max(unit) } else { o.position });
        let skip: Vec<usize> = orig.iter().map(|&(i, _)| i).collect();
        let mut dt = crate::piano_snap::delta(&self.state, &g, from, (x - x0) / g.ppt, self.alt, &skip);
        let mut held = None;
        let was_held = self.state.piano.held;
        let rects = |s: &State| -> Vec<Rect> {
            let notes = piano::notes(s);
            orig.iter().filter_map(|&(i, _)| notes.get(i).and_then(|n| g.note_rect(n))).collect()
        };
        let mut dirty = rects(&self.state);
        let s = &mut self.state;
        let p = piano::pattern_mut(s);
        if resize {
            for &(i, o) in orig {
                if let Some(n) = p.notes.get_mut(i) {
                    // The shortest note a resize leaves is one unit (spec: ui/piano-snap.md §8).
                    let len = i64::from(o.length.max(unit)) + dt;
                    n.length = len.max(i64::from(unit)) as u32;
                }
            }
            let first = orig.first().and_then(|&(i, _)| p.notes.get(i)).map(|n| n.length);
            if let Some(len) = first {
                s.piano.last_len = len;
            }
        } else {
            let dk = (g.key_at(y) - g.key_at(y0)).clamp(-131, 131);
            // Keep every note at or after tick 0 and inside the key range.
            let min_pos = orig.iter().map(|(_, o)| i64::from(o.position)).min().unwrap_or(0);
            dt = dt.max(-min_pos);
            let lo = orig.iter().map(|(_, o)| i32::from(o.key)).min().unwrap_or(0);
            let hi = orig.iter().map(|(_, o)| i32::from(o.key)).max().unwrap_or(0);
            let dk = dk.clamp(-lo, views::piano::TOP_KEY - hi);
            // The grabbed note (first) is auditioned again whenever its key changes (§8).
            if let Some(&(_, o)) = orig.first() {
                let k = i32::from(o.key) + dk;
                if was_held != Some(k) {
                    held = Some(k);
                }
            }
            for &(i, o) in orig {
                if let Some(n) = p.notes.get_mut(i) {
                    n.position = (i64::from(o.position) + dt) as u32;
                    n.key = (i32::from(o.key) + dk) as u16;
                }
            }
        }
        dirty.extend(rects(&self.state));
        self.damage_notes(&dirty);
        if held.is_some() {
            let play = self.audition();
            self.hold_key(held, play);
        }
    }

    /// Set the key held with the mouse (lit on the keyboard). With `play` it sounds as a live note, like a typing-keyboard
    /// key, until another key is held or the button is released (§8).
    /// Notes clicked or dragged in the piano roll are auditioned, except while playback runs (it is heard already).
    fn audition(&self) -> bool {
        !(self.transport.playing() && !self.transport.paused())
    }

    /// Velocity of a click on the piano roll's keyboard at `x`: soft at the keys' left end, loud at their tips (as FL).
    fn key_click_velocity(&self, x: f32) -> u8 {
        let Some(g) = self.piano_grid else { return TYPING_VELOCITY };
        let (left, w) = (g.x - 73.0, 71.0);
        (1.0 + 126.0 * ((x - left) / w).clamp(0.0, 1.0)).round() as u8
    }

    fn hold_key(&mut self, key: Option<i32>, play: bool) {
        if self.state.piano.held == key {
            return;
        }
        self.state.piano.held = key;
        if let Some((row, k)) = self.mouse_note.take() {
            self.transport.note_off(row, k);
        }
        let row = piano::channel(&self.state).and_then(|ch| self.state.project.channels.iter().position(|c| c.index == ch));
        if let (Some(k), true, Some(row)) = (key, play, row) {
            let k = k.clamp(0, views::piano::TOP_KEY) as u16;
            self.sync_live();
            self.transport.note_on(row, k, std::mem::replace(&mut self.key_velocity, TYPING_VELOCITY));
            self.mouse_note = Some((row, k));
        }
        // The keyboard column.
        match self.piano_grid {
            Some(g) => self.damage_rect(Rect::new(g.x - 73.0, g.y, 73.0, g.h)),
            None => self.invalidate(),
        }
    }

    /// Damage note rects (old and new) and the velocity-panel columns under them. The overview
    /// mini-map is refreshed when the drag ends.
    fn damage_notes(&mut self, rects: &[Rect]) {
        let (Some(grid), Some(panel)) = (self.piano_grid, self.piano_panel) else { return self.invalidate() };
        let Some(first) = rects.first() else { return };
        let u = rects.iter().fold(*first, |a, r| a.union(r)).outset(2.0).intersect(&grid);
        if u.w > 0.0 && u.h > 0.0 {
            self.damage_rect(u);
        }
        let (x0, x1) = rects.iter().fold((f32::MAX, f32::MIN), |(a, b), r| (a.min(r.x), b.max(r.right())));
        self.damage_rect(Rect::new(x0 - 6.0, panel.y, x1 - x0 + 12.0, panel.h).intersect(&panel.outset(1.0)));
    }

    /// Delete this channel's note under (x, y), if any.
    fn erase_at(&mut self, x: f32, y: f32) {
        let Some(g) = self.piano_geom() else { return };
        if !g.grid.contains(x, y) {
            return;
        }
        let ch = piano::channel(&self.state);
        let hit = piano::notes(&self.state)
            .iter()
            .enumerate()
            .rev()
            .find(|(_, n)| Some(n.channel) == ch && g.note_rect(n).is_some_and(|r| r.contains(x, y)))
            .map(|(i, _)| i);
        if let Some(i) = hit {
            piano::delete(&mut self.state, vec![i]);
            self.state.piano.dirty = true;
            self.damage_piano();
        }
    }

    /// Mute tool: give the note under (x, y) the mute state `on`.
    fn mute_at(&mut self, x: f32, y: f32, on: bool) {
        let Some(g) = self.piano_geom() else { return };
        if !g.grid.contains(x, y) {
            return;
        }
        let Some(i) = piano::note_at(&self.state, &g, x, y) else { return };
        let p = piano::pattern_mut(&mut self.state);
        if p.notes[i].muted() != on {
            p.notes[i].set_muted(on);
            let r = g.note_rect(&p.notes[i]);
            self.state.piano.dirty = true;
            match r {
                Some(r) => self.damage_notes(&[r]),
                None => self.damage_piano(),
            }
        }
    }

    /// Paint tool: fill `key`'s row with notes `len` apart from `start` up to the mouse (either
    /// direction), skipping slots that already hold a note of this channel on that key. Each note laid deletes the
    /// other keys' notes under it with "Monophonic painting in piano roll" on (`piano::paint_mono`).
    fn paint_notes(&mut self, key: i32, start: u32, len: u32) {
        let (Some(g), Some(ch)) = (self.piano_geom(), piano::channel(&self.state)) else { return };
        let len = len.max(1);
        let t = g.tick_at(self.mouse.0).max(0.0) as i64;
        let k = (t - i64::from(start)).div_euclid(i64::from(len));
        let (a, b) = (k.min(0), k.max(0));
        let (mut added, mut deleted) = (Vec::new(), false);
        for j in a..=b {
            let pos = i64::from(start) + j * i64::from(len);
            if pos < 0 {
                continue;
            }
            let pos = pos as u32;
            let taken = piano::notes(&self.state).iter().any(|n| n.channel == ch && i32::from(n.key) == key && n.position == pos);
            if !taken {
                let i = piano::add_note(&mut self.state, ch, pos, key);
                piano::pattern_mut(&mut self.state).notes[i].length = len;
                let before = piano::notes(&self.state).len();
                let i = piano::paint_mono(&mut self.state, i);
                deleted |= piano::notes(&self.state).len() < before;
                added.push(piano::notes(&self.state)[i]);
            }
        }
        if deleted {
            self.damage_piano();
        } else if !added.is_empty() {
            self.state.piano.dirty = true;
            let rects: Vec<Rect> = added.iter().filter_map(|n| g.note_rect(n)).collect();
            self.damage_notes(&rects);
        }
    }

    /// Playback tool: sound the notes the scrub line enters between ticks `from` and `to`.
    fn scrub(&mut self, from: f32, to: f32) {
        let Some(ch) = piano::channel(&self.state) else { return };
        let (a, b) = (from.min(to), from.max(to));
        let mut keys: Vec<i32> = piano::notes(&self.state)
            .iter()
            .filter(|n| n.channel == ch && !n.muted())
            .filter(|n| {
                let p = n.position as f32;
                let end = p + piano::shown_len(&self.state, n) as f32;
                if from == to {
                    // Press: every note the line touches sounds.
                    p <= to && to < end
                } else {
                    // Drag: notes whose start the line just passed.
                    p > a && p <= b
                }
            })
            .map(|n| i32::from(n.key))
            .collect();
        keys.sort_unstable();
        keys.dedup();
        for k in keys {
            self.preview(ch, k);
        }
    }

    /// Zoom tool: zoom the time axis to the dragged box (a click zooms in 2x around the mouse).
    fn zoom_to(&mut self, x0: f32, y0: f32) {
        let Some(g) = self.piano_geom() else { return };
        let (x1, y1) = self.mouse;
        let p = &mut self.state.piano;
        let back = (p.px_per_beat, p.scroll_x, p.scroll_y);
        let ppq = f32::from(self.state.project.ppq.max(4));
        if (x1 - x0).abs() < 4.0 {
            let tick = g.tick_at(x1);
            p.px_per_beat = (p.px_per_beat * 2.0).min(1600.0);
            p.scroll_x = (tick - (x1 - g.grid.x) * ppq / p.px_per_beat).max(0.0);
        } else {
            let (t0, t1) = (g.tick_at(x0.min(x1)).max(0.0), g.tick_at(x0.max(x1)).max(0.0));
            p.px_per_beat = (g.grid.w / ((t1 - t0).max(1.0) / ppq)).clamp(6.0, 1600.0);
            p.scroll_x = t0;
            p.scroll_y = (p.scroll_y + y0.min(y1) - g.grid.y).max(0.0);
        }
        p.zoom_back = Some(back);
        piano::clamp_scroll(&mut self.state, g.grid.h);
        self.invalidate();
    }

    /// Zoom tool right-click: fully zoomed out (the normal 96 px/beat view) <-> the last zoom.
    fn zoom_toggle(&mut self) {
        let Some(g) = self.piano_geom() else { return };
        let p = &mut self.state.piano;
        let normal = PianoState::default().px_per_beat;
        if (p.px_per_beat - normal).abs() > 0.01 {
            p.zoom_back = Some((p.px_per_beat, p.scroll_x, p.scroll_y));
            p.px_per_beat = normal;
            p.scroll_x = 0.0;
        } else if let Some((z, sx, sy)) = p.zoom_back {
            p.zoom_back = Some((p.px_per_beat, p.scroll_x, p.scroll_y));
            p.px_per_beat = z;
            p.scroll_x = sx;
            p.scroll_y = sy;
        }
        piano::clamp_scroll(&mut self.state, g.grid.h);
        self.invalidate();
    }

    /// Where Ctrl+V pastes (§9): the playhead when it is in view, else the left edge of the view.
    fn paste_tick(&self) -> u32 {
        let s = &self.state;
        let Some(g) = self.piano_geom() else { return 0 };
        let ppq = f64::from(s.project.ppq.max(1));
        let head = if self.transport.playing() && !self.transport.song() {
            (self.transport.tick() * 96.0 / ppq) as f32
        } else {
            s.piano.marker as f32
        };
        let x = g.x_of(head);
        let tick = if x >= g.grid.x && x < g.grid.right() { head } else { g.scroll_x };
        crate::piano_snap::place(s, &g, tick, false)
    }

    /// Delete the channel lane's control points between two x positions.
    fn erase_control_points(&mut self, last_x: f32, x: f32) {
        let (Some(g), Some(panel)) = (self.piano_geom(), self.piano_panel) else { return };
        let lane = self.state.piano.lane;
        let (Some(ch), Some(param)) = (piano::channel(&self.state), lane.param()) else { return };
        let (a, b) = (last_x.min(x), last_x.max(x));
        let from = crate::piano_snap::cell(&self.state, g.tick_at(a));
        let to = crate::piano_snap::cell(&self.state, g.tick_at(b));
        if piano::erase_controls(&mut self.state, ch, param, from, to) > 0 {
            self.damage_rect(panel);
        }
    }

    /// Set the event panel's property of this channel's notes whose start lies between the last and
    /// current x; in a channel lane, set a control point at every snap step the cursor path crosses.
    fn paint_velocity(&mut self, last_x: f32, last_y: f32, x: f32, y: f32) {
        let (Some(g), Some(panel)) = (self.piano_geom(), self.piano_panel) else { return };
        let lane = self.state.piano.lane;
        if lane.is_channel() {
            let (Some(ch), Some(param)) = (piano::channel(&self.state), lane.param()) else { return };
            let center = self.state.piano.pitch_center;
            let t0 = crate::piano_snap::cell(&self.state, g.tick_at(last_x));
            let t1 = crate::piano_snap::cell(&self.state, g.tick_at(x));
            // A point is set when the cursor enters a snap unit (its value is the path height at the
            // unit's start, as in FL's captured ramps) and on the press; staying inside a unit keeps it.
            // Without a unit, one point at the pointer (spec: ui/piano-snap.md §8).
            let (lo, hi, step) = match crate::piano_snap::unit(&self.state) {
                Some(u) => (t0.min(t1), t0.max(t1), u),
                None => (t1, t1, 1),
            };
            let press = last_x == x && last_y == y;
            let mut t = lo;
            loop {
                if t != t0 || (t0 == t1 && press) {
                    let px = g.x_of(t as f32);
                    let at = if (x - last_x).abs() < 0.5 { 1.0 } else { ((px - last_x) / (x - last_x)).clamp(0.0, 1.0) };
                    let y_at = last_y + (y - last_y) * at;
                    piano::set_control(&mut self.state, ch, param, t, lane.control_at(panel, y_at, center));
                }
                if t >= hi {
                    break;
                }
                t += step;
            }
            self.damage_rect(panel);
            return;
        }
        let v = lane.value_at(panel, y);
        let (ta, tb) = (g.tick_at(last_x.min(x)), g.tick_at(last_x.max(x)));
        // The notes the cursor's path covers: those starting inside it, and the note already playing where the path
        // begins (the latest start at or before it; before the first note, the first one), so a click anywhere above
        // or below a note's line sets it. A selection limits this to the selected notes (FL's manual: select first to
        // reach one of several notes that start together).
        let cand = piano::targets(&self.state);
        let starts = || cand.iter().filter_map(|&i| piano::notes(&self.state).get(i)).map(|n| n.position);
        let anchor = starts().filter(|&p| p as f32 <= ta).max().or_else(|| starts().min());
        let hit: Vec<usize> = cand
            .iter()
            .copied()
            .filter(|&i| piano::notes(&self.state).get(i).is_some_and(|n| Some(n.position) == anchor || (n.position as f32 >= ta && n.position as f32 <= tb)))
            .collect();
        let p = piano::pattern_mut(&mut self.state);
        let mut span: Option<(f32, f32)> = None;
        let mut edited = None;
        for (i, n) in p.notes.iter_mut().enumerate() {
            let nx = g.x_of(n.position as f32);
            if hit.contains(&i) {
                lane.set(n, v);
                edited = Some(*n);
                let end = g.x_of((n.position + n.length.max(g.step)) as f32);
                span = Some(span.map_or((nx, end), |(l, r)| (l.min(nx), r.max(end))));
            }
        }
        // The next notes placed take the edited note's controls.
        if edited.is_some() {
            self.state.piano.last_controls = edited;
        }
        // Only the panel columns of the changed lollipops (the hint is left alone: a damage
        // union with the hint panel would cover most of the window).
        if let Some((l, r)) = span {
            self.damage_rect(Rect::new(l - 6.0, panel.y, r - l + 12.0, panel.h).intersect(&panel.outset(1.0)));
        }
    }

    pub fn wheel(&mut self, dx: f32, dy: f32, shift: bool, ctrl: bool) {
        if self.modal_open() {
            return self.dialog_wheel(dy);
        }
        // Scrolling or zooming one window repaints just that window; scrolling the playlist moves its pixels.
        let before = self.damage.clone();
        let s = &self.state;
        let view = (s.playlist_scroll, s.playlist_bar_w, s.playlist.row_h, s.playlist.pick_scroll);
        self.invalidate();
        // A scroll eases to where the wheel sends it (General ▸ Display ▸ Smoothing, `crate::smooth`).
        if let Some(w) = self.eased(|app| app.wheel_act(dx, dy, shift, ctrl)) {
            self.damage = before;
            let s = &self.state;
            if w == Win::Playlist && (s.playlist_bar_w, s.playlist.row_h, s.playlist.pick_scroll) == (view.1, view.2, view.3) {
                self.playlist_scrolled(view.0);
            } else {
                self.damage_win(w);
            }
        }
    }

    /// The wheel's work; returns the window when only that window's view moved (else everything is repainted).
    fn wheel_act(&mut self, dx: f32, dy: f32, shift: bool, ctrl: bool) -> Option<Win> {
        let (x, y) = self.mouse;
        let area = self.top_hit(x, y).map(|h| h.action.clone());
        if matches!(area, Some(Action::BrowserList | Action::BrowserRow(_) | Action::BrowserTab(_))) {
            self.browser_wheel(dy + dx);
            return None;
        }
        if matches!(area, Some(Action::Settings(views::settings::SAct::Plug(views::settings::Plug::Table | views::settings::Plug::Row(_) | views::settings::Plug::Star(_) | views::settings::Plug::Show(_))))) {
            let n = self.state.plugdb.rows.len();
            let top = self.state.plugdb.top as f32 - (dy + dx) * 3.0;
            self.state.plugdb.top = (top.max(0.0) as usize).min(n.saturating_sub(views::settings::TABLE_ROWS));
            return None;
        }
        // General ▸ Input's scrolling speeds: a notch moves a slider by its step, up for more (spec:
        // ui/settings-choices.md "Input: Vertical and Horizontal scrolling speed").
        if let Some(Action::Settings(views::settings::SAct::Slider(label, ..))) = area {
            if dy + dx != 0.0 {
                self.pref_slider_wheel(label, if dy + dx > 0.0 { 1 } else { -1 });
            }
            return None;
        }
        // General ▸ Display's PPI ruler: 1 PPI a notch, up for more (spec: ui/settings-choices.md "Display: PPI ruler,
        // Force refreshes, Thick lines, High visibility (2026-10-16)").
        if let Some(Action::Settings(views::settings::SAct::Ppi)) = area {
            if dy + dx != 0.0 {
                self.ppi_wheel(if dy + dx > 0.0 { 1 } else { -1 });
            }
            return None;
        }
        // Project ▸ a time division's value box: one per notch, up for more (spec: ui/settings-choices.md "Set as time
        // division").
        if let Some(Action::Settings(views::settings::SAct::Number(label))) = area {
            if dy + dx != 0.0 {
                if let Some(m) = self.settings_number(label, if dy + dx > 0.0 { 1 } else { -1 }) {
                    self.state.message = Some(m);
                }
            }
            return None;
        }
        if matches!(area, Some(Action::Settings(_))) {
            let s = &self.state;
            let max = (views::settings::content_h(s) - views::settings::page_h(s)).max(0.0);
            self.state.settings.scroll = (self.state.settings.scroll - (dy + dx) * 63.0).min(max).max(0.0);
            return None;
        }
        if matches!(area, Some(Action::Perf(crate::perf::PerfAct::List | crate::perf::PerfAct::Row(_)))) {
            self.perf_wheel(dy + dx);
            return Some(Win::Perf);
        }
        if matches!(area, Some(Action::SongPos)) {
            self.song_pos_wheel(dy);
            return None;
        }
        if matches!(area, Some(Action::PianoTarget)) && dy != 0.0 {
            // spec: ui/piano-target-channel.md §3
            crate::piano_tools::cycle_channel(self, if dy < 0.0 { 1 } else { -1 }, true);
            return Some(Win::PianoRoll);
        }
        if matches!(area, Some(Action::GlobalSnap)) && dy != 0.0 {
            // One setting per notch, down toward Bar (spec: ui/global-snap.md §1).
            let notches = (-dy).round() as i32;
            let notches = if notches == 0 { -(dy.signum() as i32) } else { notches };
            self.state.message = Some(crate::snap::wheel(&mut self.state, notches));
            return None;
        }
        if matches!(area, Some(Action::PlPick(_) | Action::PlPickList)) {
            self.scroll_set(Scroll::PlPick, self.scroll_get(Scroll::PlPick) - (dy + dx) * 31.0);
            return Some(Win::Playlist);
        }
        if matches!(area, Some(Action::PatternNext)) {
            let d = if dy + dx > 0.0 { 1 } else { -1 };
            self.state.pattern = (i32::from(self.state.pattern.max(1)) + d).clamp(1, i32::from(crate::patterns::MAX)) as u16;
            return None;
        }
        if let Some(Action::Scrollbar(t)) = area {
            let step = if t == Scroll::PianoY { self.state.piano.row_h * 3.0 } else { 60.0 };
            // An editor's bar scrolls at the scrolling speeds (`crate::scroll_speed`); the picker list keeps its step.
            let (dx, dy) = if shift || t == Scroll::PlPick { (dx, dy) } else { crate::scroll_speed::scaled(dx, dy) };
            self.scroll_set(t, self.scroll_get(t) - (dy + dx) * step);
            return Some(scroll_win(t));
        }
        // Piano roll (spec: ui/piano-roll.md §3a): Alt+wheel over the ruler zooms time; the keyboard takes the wheel
        // like the grid (scroll, Ctrl = time zoom); the keys zoom with Ctrl+middle-drag.
        if matches!(area, Some(Action::PianoRuler)) && self.alt {
            let Some(g) = self.piano_geom() else { return None };
            piano::zoom_time(&mut self.state, &g, x, dy + dx);
            return Some(Win::PianoRoll);
        }
        if matches!(area, Some(Action::PianoGrid | Action::PianoNote(_) | Action::PianoKeys | Action::PianoVelocity)) {
            let Some(g) = self.piano_geom() else { return None };
            if ctrl {
                // Zoom time around the mouse.
                piano::zoom_time(&mut self.state, &g, x, dy + dx);
                piano::clamp_scroll(&mut self.state, g.grid.h);
                return Some(Win::PianoRoll);
            }
            let row = self.state.piano.row_h;
            let p = &mut self.state.piano;
            if shift {
                p.scroll_x = (p.scroll_x - (dy + dx) * 120.0 / g.ppt).max(0.0);
            } else {
                let (dx, dy) = crate::scroll_speed::scaled(dx, dy);
                p.scroll_y -= dy * row * 3.0;
                p.scroll_x = (p.scroll_x - dx * 120.0 / g.ppt).max(0.0);
            }
            piano::clamp_scroll(&mut self.state, g.grid.h);
            return Some(Win::PianoRoll);
        }
        if let Some(Action::Knob(id)) = area {
            let mut v = self.state.knob(id);
            if id == crate::knobs::KnobId::MasterPitch {
                v = (v * 24.0).round() / 24.0; // wheel steps from the nearest semitone
            }
            let v = v + dy.signum() * id.notch() * if dy == 0.0 { 0.0 } else { 1.0 };
            if v.clamp(0.0, 1.0) != self.state.knob(id) {
                crate::history::knob(&mut self.state, id, true);
            }
            self.state.set_knob(id, v);
            self.hint = Cow::Owned(self.state.knob_text(id));
            return None;
        }
        // Channel rack value boxes (spec: ui/channel-rack.md §7, §12, §15): one per notch.
        let d = if dy + dx > 0.0 { 1 } else { -1 };
        match area {
            Some(Action::RackMixer(i)) => {
                let s = &mut self.state;
                let max = crate::mixer::max_insert(&s.project);
                if let Some(c) = s.project.channels.get_mut(i) {
                    c.mixer_track = (i32::from(c.mixer_track) + d).clamp(0, i32::from(max)) as u16;
                    self.hint = Cow::Owned(format!("Target mixer track: {}", views::rack::insert_name(c.mixer_track)));
                    s.piano.dirty = true;
                }
                return None;
            }
            Some(Action::RackLen) => {
                let v = (crate::rack::len_value(&self.state) as i32 + d).clamp(0, crate::rack::max_steps(&self.state) as i32) as u32;
                crate::rack::set_len(&mut self.state, (v > 0).then_some(v));
                return None;
            }
            Some(Action::RackLoopBox(i)) => {
                let v = (crate::rack::row_len(&self.state, i) as i32 + d).clamp(1, crate::rack::MAX_STEPS as i32) as u32;
                self.state.rack.loop_len.insert(i, v);
                self.state.piano.dirty = true;
                return None;
            }
            _ => {}
        }
        let rack_area = self.top_hit(x, y).is_some_and(|h| h.win == Some(Win::ChannelRack));
        if rack_area {
            let (rx, ry) = if shift { (dy + dx, 0.0) } else { crate::scroll_speed::scaled(dx, dy) };
            self.scroll_set(Scroll::RackY, self.scroll_get(Scroll::RackY) - ry * views::rack::ROW_H);
            self.scroll_set(Scroll::RackX, self.scroll_get(Scroll::RackX) - rx * 4.0 * views::rack::STEP_W);
            return Some(Win::ChannelRack);
        }
        let alt = self.alt;
        let pl_grid = self.pl_grid;
        let pl_area = matches!(
            area,
            Some(Action::PlaylistArea | Action::PlRuler | Action::PlMarkers | Action::PlHeader(_) | Action::PlLed(_) | Action::PlGroupArrow(_))
        );
        let s = &mut self.state;
        let (dx, dy) = if shift { (dy, dx) } else { (dx, dy) };
        // What the playlist and the mixer scroll by (`crate::scroll_speed`); zooms take the wheel's own steps.
        let (sx, sy) = if shift { (dx, dy) } else { crate::scroll_speed::scaled(dx, dy) };
        let row_h = s.playlist.row_h;
        match area {
            Some(Action::Fader(_) | Action::Mix(_) | Action::MixerArea) => {
                s.mixer_scroll = (s.mixer_scroll - (sy + sx) * 44.0).max(0.0);
                Some(Win::Mixer)
            }
            // Alt+wheel anywhere in the playlist: track height (spec: ui/playlist.md §3, §6).
            _ if pl_area && alt => {
                let grid = pl_grid.unwrap_or(Rect::new(481.0, 150.0, 1400.0, 800.0));
                crate::playlist::zoom_rows(s, grid, y, dy + dx);
                Some(Win::Playlist)
            }
            // Ctrl+wheel anywhere in the playlist, the plain wheel over the ruler: time zoom around the mouse (§3).
            _ if pl_area && (ctrl || (!shift && matches!(area, Some(Action::PlRuler | Action::PlMarkers)))) => {
                let gx = self.pl_grid.map_or(481.0, |g| g.x);
                let bar = (s.playlist_scroll.0 + x - gx) / s.playlist_bar_w;
                s.playlist_bar_w = (s.playlist_bar_w * 1.25f32.powf(dy + dx)).clamp(4.0, 600.0);
                s.playlist_scroll.0 = (bar * s.playlist_bar_w - (x - gx)).max(0.0);
                Some(Win::Playlist)
            }
            // Shift+wheel over the track headers moves the selected track (spec: ui/playlist.md §6).
            Some(Action::PlHeader(_) | Action::PlLed(_) | Action::PlGroupArrow(_)) if shift => {
                let d = if dx + dy > 0.0 { -1 } else { 1 };
                crate::playlist::snapshot(s);
                crate::playlist::shift_tracks(s, d);
                None
            }
            Some(Action::PlHeader(_) | Action::PlLed(_) | Action::PlGroupArrow(_)) => {
                s.playlist_scroll.1 = (s.playlist_scroll.1 - sy * row_h).max(0.0);
                Some(Win::Playlist)
            }
            Some(Action::PlaylistArea) => {
                s.playlist_scroll.0 = (s.playlist_scroll.0 - sx * 60.0).max(0.0);
                s.playlist_scroll.1 = (s.playlist_scroll.1 - sy * row_h).max(0.0);
                Some(Win::Playlist)
            }
            _ => None,
        }
    }

    pub fn key(&mut self, key: &winit::keyboard::Key, ctrl: bool, alt: bool) {
        self.invalidate();
        // A dialog takes every key.
        if self.modal_open() {
            return self.dialog_key(key, ctrl, alt);
        }
        // So does a box of the settings window's Info page while it is edited, and a cell of File ▸ Browser extra
        // search folders.
        if self.info_key(key, ctrl, alt) || self.folder_cell_key(key, ctrl, alt) {
            return;
        }
        use winit::keyboard::{Key, NamedKey};
        // Ctrl+Shift+D: switch the design (spec: ui/lemon-design.md §1).
        if let Key::Character(c) = key
            && ctrl
            && self.shift
            && c.eq_ignore_ascii_case("d")
        {
            let next = self.design().next();
            self.set_design(next, true);
            return;
        }
        // Shift+Alt+G: Enable groups (spec: ui/enable-groups.md §1), everywhere the main window has the keyboard.
        if let Key::Character(c) = key
            && alt
            && self.shift
            && !ctrl
            && c.eq_ignore_ascii_case("g")
        {
            return self.toggle_groups();
        }
        if self.plugin_find_key(key, ctrl) {
            return;
        }
        if self.state.perf.finding && self.perf_key(key, ctrl) {
            return;
        }
        if crate::patterns::key(&mut self.state, key, ctrl) {
            self.rack_pending();
            return;
        }
        if self.browser_key(key, ctrl, alt) {
            return;
        }
        // Ctrl+Z / Ctrl+Alt+Z: the project's undo history, whichever window is in front (spec: ui/settings-choices.md
        // "Undo history: the four checks (2026-10-16)").
        if let Key::Character(c) = key
            && ctrl
            && c.eq_ignore_ascii_case("z")
        {
            return self.undo_key(alt);
        }
        // The channel rack's keys while it is in front (spec: ui/channel-rack.md §20).
        let s = &self.state;
        if (s.z.last() == Some(&Win::ChannelRack) || s.rack.fill.is_some())
            && s.menu_open.is_none()
            && !s.settings.open
            && s.playlist.edit.is_none()
            && crate::rack::key(self, key, ctrl, alt)
        {
            self.rack_pending();
            return;
        }
        // The mixer's keys while it is in front (spec: ui/mixer.md §2).
        let s = &self.state;
        if s.z.last() == Some(&Win::Mixer) && s.menu_open.is_none() && !s.settings.open && s.playlist.edit.is_none() && s.playlist.popup.is_none() && crate::mixer::key(self, key, ctrl, alt) {
            return;
        }
        // The playlist's editing keys when it is in front (spec: ui/playlist.md §5).
        if self.pl_focus() && crate::playlist::key(self, key, ctrl, alt) {
            return;
        }
        if let Some(cmd) = pattern_shortcut(key, ctrl, self.shift) {
            if let Some(m) = self.pattern_cmd(cmd) {
                self.state.message = Some(m);
            }
            return;
        }
        // The piano roll's editing and tool keys (spec: ui/piano-roll.md §11).
        let s = &self.state;
        if s.shown(Win::PianoRoll) && s.menu_open.is_none() && !s.settings.open && s.playlist.edit.is_none() && crate::piano_tools::key(self, key, ctrl, alt) {
            return;
        }
        match key {
            // Space starts and stops (rewinding); Ctrl+Space starts and pauses, like the Play button (FL's manual,
            // keyboard shortcuts: "Start/Stop Playback", "Start/Pause Playback").
            Key::Named(NamedKey::Space) if ctrl => self.act(Action::Play),
            Key::Named(NamedKey::Space) => self.act(if self.transport.playing() { Action::Stop } else { Action::Play }),
            // spec: ui/tempo-tapper.md §1
            Key::Named(NamedKey::F5) if ctrl => self.act(Action::ToggleWin(Win::Tapper)),
            Key::Named(NamedKey::F5) => self.act(Action::ToggleWin(Win::Playlist)),
            Key::Named(NamedKey::F6) => self.act(Action::ToggleWin(Win::ChannelRack)),
            Key::Named(NamedKey::F7) => self.act(Action::ToggleWin(Win::PianoRoll)),
            // The browser and the pickers: General ▸ Input ▸ Manage shortcuts (spec: ui/settings-choices.md).
            Key::Named(NamedKey::F8) => self.f8_key(ctrl, alt),
            Key::Named(NamedKey::F9) => self.act(Action::ToggleWin(Win::Mixer)),
            Key::Named(NamedKey::F10) => self.open_settings(views::settings::MIDI),
            // Toolbars > Hide (spec: ui/toolbar-menu.md §5).
            Key::Named(NamedKey::F11) if ctrl => self.toolbar_switch(views::toolbar::Switch::Hide),
            Key::Named(NamedKey::F11) => self.open_settings(views::settings::INFO),
            // Monitor debug panel (views/mondebug.rs) parked 2026-10-09: its values are now the defaults. Uncomment to tune again.
            // Key::Named(NamedKey::F12) => self.state.monitor.debug = !self.state.monitor.debug,
            // spec: ui/close-windows.md (Shift+F12, Align all channel editors, is not available yet).
            Key::Named(NamedKey::F12) if !self.shift => self.close_windows(if ctrl {
                CloseAll::Unfocused
            } else if alt {
                CloseAll::Plugins
            } else {
                CloseAll::All
            }),
            Key::Named(NamedKey::Escape) if self.state.settings.open => self.state.settings.open = false,
            Key::Named(NamedKey::Escape) => {
                self.state.menu_open = None;
                self.state.menu_sub = None;
                self.state.playlist.popup = None;
                self.state.piano.clear_selection();
            }
            // Piano roll editing keys (spec: ui/piano-roll.md §8).
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("a") && self.state.shown(Win::PianoRoll) => {
                let s = &mut self.state;
                if let Some(ch) = piano::channel(s) {
                    let all = piano::notes(s).iter().enumerate().filter(|(_, n)| n.channel == ch).map(|(i, _)| i).collect();
                    s.piano.select(s.pattern, ch, all);
                }
            }
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("t") => self.toggle_typing(),
            Key::Character(c) if ctrl && !alt && c.eq_ignore_ascii_case("m") => self.toggle_metronome(),
            Key::Character(c) if ctrl && !alt && c.eq_ignore_ascii_case("e") => self.toggle_step_edit(),
            // Piano-roll tools and selection editing (spec: ui/piano-roll.md §9). Letters switch
            // tools (until a typing keyboard exists, which will take them).
            Key::Character(c) if !ctrl && !alt && self.state.shown(Win::PianoRoll) && Tool::from_letter(c).is_some() => {
                if let Some(t) = Tool::from_letter(c) {
                    self.act(Action::PianoTool(t));
                }
            }
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("c") && self.state.shown(Win::PianoRoll) => {
                let s = &mut self.state;
                let sel = s.piano.selection(s.pattern, piano::channel(s));
                let notes = piano::notes(s);
                s.piano.clipboard = sel.iter().filter_map(|&i| notes.get(i).copied()).collect();
            }
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("v") && self.state.shown(Win::PianoRoll) => {
                let at = self.paste_tick();
                let clip = self.state.piano.clipboard.clone();
                piano::paste(&mut self.state, &clip, at);
                self.state.piano.dirty = true;
            }
            Key::Named(k @ (NamedKey::ArrowLeft | NamedKey::ArrowRight | NamedKey::ArrowUp | NamedKey::ArrowDown))
                if self.state.shown(Win::PianoRoll) =>
            {
                self.piano_arrow(*k, ctrl);
            }
            Key::Named(NamedKey::Delete) if self.state.shown(Win::PianoRoll) => {
                let s = &mut self.state;
                let sel = s.piano.selection(s.pattern, piano::channel(s));
                if !sel.is_empty() {
                    piano::snapshot_as(s, "Piano roll delete note");
                    piano::delete(s, sel);
                }
            }
            // FILE shortcuts (views/menus.rs). Shift picks another command: Shift+Ctrl+S = Save as; the other Shift
            // variants have none, so they do nothing (not Open, Export or a new file).
            Key::Character(c) if ctrl && !self.shift && c.eq_ignore_ascii_case("o") => self.act(Action::MenuItem(0, views::menus::FILE_OPEN)),
            Key::Character(c) if ctrl && !self.shift && c.eq_ignore_ascii_case("s") => self.act(Action::MenuItem(0, views::menus::FILE_SAVE)),
            Key::Character(c) if ctrl && self.shift && c.eq_ignore_ascii_case("s") => self.act(Action::MenuItem(0, views::menus::FILE_SAVE_AS)),
            Key::Character(c) if ctrl && !self.shift && c.eq_ignore_ascii_case("r") => self.act(Action::MenuItem(0, views::menus::FILE_EXPORT)),
            Key::Character(c) if ctrl && !self.shift && c.eq_ignore_ascii_case("n") => self.act(Action::MenuItem(0, views::menus::FILE_SAVE_VERSION)),
            // TOOLS > Browser find... (spec: ui/browser-commands.md §3).
            Key::Character(c) if ctrl && !alt && !self.shift && c.eq_ignore_ascii_case("f") => self.browser_jump(crate::browser::Jump::Find),
            _ => {}
        }
    }

    /// SONG mode, a press or drag in the piano roll's ruler at pattern tick `t`: when the playlist plays this pattern
    /// there, the song position moves to that place, as a click in the playlist ruler would (the clip under the
    /// current position first, else the next one, else the first). A playing song plays on from there (on release
    /// when dragging); a paused one stops there, so Play resumes from that point. False: the pattern does not play
    /// that tick anywhere in the song.
    fn piano_song_seek(&mut self, t: u32, press: bool) -> bool {
        let tp = &self.transport;
        let near = if tp.playing() && tp.song() { tp.tick() as u32 } else { self.state.playlist.pos };
        let Some(at) = piano::song_tick_of(&self.state, t, near) else { return false };
        if press && tp.playing() && tp.song() && tp.paused() {
            tp.stop();
        }
        self.set_song_pos(at, press);
        true
    }

    /// Start playback in the mode the PAT / SONG switch shows. Pattern mode loops the current pattern from the piano
    /// roll's play-start marker (§10); song mode plays the playlist from its start and stops at its end.
    pub(crate) fn start_playback(&mut self) {
        let song = self.state.song_mode;
        if song && self.state.project.arrangement().is_none_or(|a| a.clips.is_empty()) {
            self.state.message = Some("The playlist is empty: nothing to play in SONG mode".into());
            self.invalidate();
            return;
        }
        // The song starts with the project's initial values (spec: ui/settings-choices.md "Advanced: Initialize
        // controls automatically").
        if crate::init::apply(&mut self.state) {
            self.invalidate();
        }
        let p = crate::rack::play_project(&self.state);
        let start = if song { f64::from(self.state.playlist.pos) } else { f64::from(self.state.piano.marker) };
        self.transport.play(&p, self.play_mode(), start);
    }

    fn play_mode(&self) -> PlayMode {
        if self.state.song_mode { PlayMode::Song } else { PlayMode::Pattern(self.state.pattern) }
    }

    /// Play one note of channel `channel` (Note::channel) once (also while playing or paused).
    /// Typing keyboard switch (toolbar, Options menu, Ctrl+T); turning it off releases held notes.
    pub fn toggle_typing(&mut self) {
        let s = &mut self.state;
        if let Some(i) = s.toggles.iter().position(|t| *t == "typing") {
            s.toggles.remove(i);
            self.typing_release_all();
        } else {
            s.toggles.push("typing");
        }
        self.invalidate();
    }

    /// Enable groups switch (toolbar, OPTIONS menu, Shift+Alt+G): the playlist's clips are deselected, as in FL.
    pub fn toggle_groups(&mut self) {
        let on = crate::enable_groups::toggle();
        self.state.playlist.sel.clear();
        self.state.message = Some(if on { "Groups enabled" } else { "Groups disabled" }.into());
        self.invalidate();
    }

    pub fn typing_on(&self) -> bool {
        self.state.toggles.contains(&"typing")
    }

    /// A physical key went down or up. Returns true when the typing keyboard took it (a note key or
    /// the octave keys while the switch is on, or the release of a held note); the caller then
    /// skips the normal shortcut handling. Spec: ui/typing-keyboard.md.
    pub fn typing_key(&mut self, code: KeyCode, pressed: bool, ctrl: bool, alt: bool) -> bool {
        // A playlist text box (track / marker name, point value) takes the letters.
        if pressed && (self.modal_open() || self.state.playlist.edit.is_some() || self.state.plugdb.finding || self.state.pat_ui.name_box.is_some() || self.state.pat_ui.confirm || self.browser_takes_typing() || self.state.perf.finding) {
            return false;
        }
        // So does a box of the settings window's Info page, and a browser folder cell.
        if pressed && (self.info_editing() || self.folder_cell_editing()) {
            return false;
        }
        if !pressed {
            let Some(i) = self.typing_held.iter().position(|h| h.0 == code) else { return false };
            let (_, row, key) = self.typing_held.remove(i);
            self.transport.note_off(row, key);
            self.typing_lit_changed();
            self.step_edit_up(crate::step_edit::Src::Typing(code));
            return true;
        }
        // Ctrl / Alt / Shift combinations stay shortcuts (Shift+D, Shift+Q in the piano roll).
        if !self.typing_on() || ctrl || alt || self.shift {
            return false;
        }
        if matches!(code, KeyCode::NumpadAdd | KeyCode::NumpadSubtract) {
            let d = if code == KeyCode::NumpadAdd { 1 } else { -1 };
            self.typing_octave = (self.typing_octave + d).clamp(-4, 5);
            self.state.message = Some(format!("Typing keyboard octave {:+}", self.typing_octave));
            self.invalidate();
            return true;
        }
        let Some(semi) = typing_semitone(code) else { return false };
        if self.typing_held.iter().any(|h| h.0 == code) {
            return true; // auto-repeat
        }
        let key = 48 + semi + 12 * self.typing_octave;
        let row = self.state.selected_channel;
        if !(0..=127).contains(&key) || row >= self.state.project.channels.len() {
            return true;
        }
        self.sync_live();
        let key = key as u16;
        self.transport.note_on(row, key, TYPING_VELOCITY);
        self.typing_held.push((code, row, key));
        self.step_edit_down(crate::step_edit::Src::Typing(code), key, TYPING_VELOCITY);
        self.typing_lit_changed();
        true
    }

    /// The live stream needs the project before the first live note.
    fn sync_live(&mut self) {
        if !self.typing_synced {
            self.typing_synced = true;
            self.transport.update(&crate::rack::play_project(&self.state));
        }
    }

    /// Release every held typing-keyboard note and the piano-roll key held with the mouse (focus lost, switch off,
    /// rows renumbered): no hanging notes.
    pub fn typing_release_all(&mut self) {
        self.step_chord.clear();
        if let Some((row, key)) = self.mouse_note.take() {
            self.transport.note_off(row, key);
        }
        if self.typing_held.is_empty() {
            return;
        }
        for (_, row, key) in std::mem::take(&mut self.typing_held) {
            self.transport.note_off(row, key);
        }
        self.typing_lit_changed();
    }

    /// MIDI controller notes (spec: ui/midi-input.md): note on plays the selected channel, like a typing-keyboard key;
    /// note off releases the key on the channel it started on.
    fn midi_tick(&mut self) {
        let msgs = self.midi.poll();
        self.midi_notes(msgs);
    }

    /// MIDI notes received, oldest first.
    pub(crate) fn midi_notes(&mut self, msgs: Vec<crate::midi_in::Msg>) {
        if msgs.is_empty() {
            return;
        }
        for m in msgs {
            match m {
                crate::midi_in::Msg::On(key, velocity) => {
                    if self.state.shown(Win::Tapper) {
                        self.midi_tap();
                    }
                    let row = self.state.selected_channel;
                    if row >= self.state.project.channels.len() {
                        continue;
                    }
                    self.sync_live();
                    self.transport.note_on(row, u16::from(key), velocity);
                    self.midi_held.push((key, row));
                    self.step_edit_down(crate::step_edit::Src::Midi(key), u16::from(key), velocity);
                }
                crate::midi_in::Msg::Off(key) => {
                    while let Some(i) = self.midi_held.iter().position(|h| h.0 == key) {
                        let (_, row) = self.midi_held.remove(i);
                        self.transport.note_off(row, u16::from(key));
                    }
                    self.step_edit_up(crate::step_edit::Src::Midi(key));
                }
            }
        }
        self.typing_lit_changed();
    }

    /// Held typing-keyboard keys and MIDI notes light on the piano roll's keyboard (§4).
    fn typing_lit_changed(&mut self) {
        let channels = &self.state.project.channels;
        let midi = self.midi_held.iter().map(|&(key, row)| (row, u16::from(key)));
        self.state.piano.typed = self.typing_held.iter().map(|&(_, row, key)| (row, key)).chain(midi).filter_map(|(row, key)| Some((channels.get(row)?.index, i32::from(key)))).collect();
        match self.piano_grid {
            Some(g) => self.damage_rect(Rect::new(g.x - 73.0, g.y, 73.0, g.h)),
            None => self.invalidate(),
        }
    }

    fn preview(&self, channel: u16, key: i32) {
        let mut p = self.state.project.clone();
        p.tempo = Some(self.state.tempo);
        let note = Note { key: key.clamp(0, views::piano::TOP_KEY) as u16, length: u32::from(p.ppq.max(4)), ..Note::step(channel, 0) };
        p.patterns = vec![Pattern { number: 1, notes: vec![note], ..Pattern::default() }];
        p.arrangements.clear();
        self.transport.preview(p);
    }

    /// Arrow keys in the piano roll (§9): Left/Right scroll; Shift+arrows move the selection (or
    /// every note) one snap unit (a tick without one, spec: ui/piano-snap.md §6) / semitone; Ctrl+Up/Down
    /// move it an octave.
    fn piano_arrow(&mut self, k: winit::keyboard::NamedKey, ctrl: bool) {
        use winit::keyboard::NamedKey::*;
        let Some(g) = self.piano_geom() else { return };
        let step = i64::from(crate::piano_snap::unit(&self.state).unwrap_or(1));
        let (dt, dk) = match (k, ctrl, self.shift) {
            (ArrowUp, true, _) => (0, 12),
            (ArrowDown, true, _) => (0, -12),
            (ArrowLeft | ArrowRight, true, _) => return,
            (ArrowLeft, false, true) => (-step, 0),
            (ArrowRight, false, true) => (step, 0),
            (ArrowUp, false, true) => (0, 1),
            (ArrowDown, false, true) => (0, -1),
            (ArrowLeft | ArrowRight, false, false) => {
                let d = if k == ArrowLeft { -1.0 } else { 1.0 };
                let s = &mut self.state;
                s.piano.scroll_x = (s.piano.scroll_x + d * g.bar as f32).max(0.0);
                return;
            }
            _ => return,
        };
        let idx = piano::targets(&self.state);
        if piano::shift(&mut self.state, &idx, dt, dk) {
            self.state.piano.dirty = true;
        }
    }

    /// Left click in the piano roll (spec: ui/piano-roll.md §8 Draw tool, §9 other tools).
    fn piano_click(&mut self, a: &Action) {
        let (x, y) = self.mouse;
        let Some(g) = self.piano_geom() else { return };
        let Some(ch) = piano::channel(&self.state) else {
            self.state.message = Some("Piano roll: no channel selected".into());
            return;
        };
        let tool = self.state.piano.tool;
        if matches!(a, Action::PianoGrid | Action::PianoNote(_)) && tool != Tool::Draw {
            let s = &mut self.state;
            let note = if let Action::PianoNote(i) = *a { Some(i) } else { None };
            match tool {
                Tool::Delete => {
                    piano::snapshot_as(s, "Piano roll delete note");
                    self.drag = Some(Drag::NoteErase);
                    self.erase_at(x, y);
                    return;
                }
                Tool::Mute => {
                    let Some(i) = note else { return };
                    piano::snapshot(s);
                    let on = !piano::notes(s)[i].muted();
                    self.drag = Some(Drag::NoteMute { on });
                    self.mute_at(x, y, on);
                    return;
                }
                Tool::Paint if note.is_none() => {
                    piano::snapshot_as(s, "Piano roll add note");
                    s.piano.clear_selection();
                    let key = g.key_at(y).clamp(0, views::piano::TOP_KEY);
                    let start = crate::piano_snap::place(s, &g, g.tick_at(x), self.alt);
                    let i = piano::add_note(s, ch, start, key);
                    let i = piano::paint_mono(s, i);
                    let len = piano::notes(s)[i].length;
                    s.piano.dirty = true;
                    self.drag = Some(Drag::NotePaint { key, start, len });
                    let play = self.audition();
                    self.hold_key(Some(key), play);
                    self.damage_piano();
                    return;
                }
                Tool::Slice => {
                    self.drag = Some(Drag::Slice);
                    self.state.piano.overlay = Some(Overlay::Slice(crate::piano_snap::place(&self.state, &g, g.tick_at(x), self.alt)));
                    self.damage_piano();
                    return;
                }
                Tool::Zoom => {
                    self.drag = Some(Drag::ZoomBox { x0: x, y0: y });
                    return;
                }
                Tool::Playback => {
                    let t = g.tick_at(x).max(0.0);
                    self.drag = Some(Drag::Scrub { tick: t });
                    self.scrub(t, t);
                    self.state.piano.overlay = Some(Overlay::Scrub(t));
                    self.damage_piano();
                    return;
                }
                Tool::Select if note.is_none() => {
                    let base = if self.ctrl { s.piano.selection(s.pattern, Some(ch)) } else { Vec::new() };
                    s.piano.select(s.pattern, ch, base.clone());
                    self.drag = Some(Drag::Marquee { x0: x, y0: y, base });
                    self.damage_piano();
                    return;
                }
                Tool::Select => {
                    // Clicking an unselected note selects it (Ctrl adds), then drags as Draw does.
                    let i = note.unwrap_or(0);
                    let mut sel = s.piano.selection(s.pattern, Some(ch));
                    if !sel.contains(&i) {
                        if !self.ctrl {
                            sel.clear();
                        }
                        sel.push(i);
                        s.piano.select(s.pattern, ch, sel);
                    }
                }
                // Paint on a note: drags that one note like Draw (no group move).
                _ => s.piano.clear_selection(),
            }
        }
        let s = &mut self.state;
        match *a {
            Action::PianoGrid => {
                piano::snapshot_as(s, "Piano roll add note");
                s.piano.clear_selection();
                let i = piano::add_note(s, ch, crate::piano_snap::place(s, &g, g.tick_at(x), self.alt), g.key_at(y));
                let i = piano::paint_mono(s, i);
                let n = piano::notes(s)[i];
                s.piano.last_len = n.length;
                self.drag = Some(Drag::NoteMove { x0: x, y0: y, orig: vec![(i, n)] });
                let play = self.audition();
                self.hold_key(Some(i32::from(n.key)), play);
                // Held without a move, the new note's type cycles (Click-and-hold & special gesture functions).
                self.hold_start(crate::hold::Kind::NewNote);
            }
            Action::PianoNote(i) => {
                let Some(&n) = piano::notes(s).get(i) else { return };
                piano::snapshot(s);
                let sel = s.piano.selection(s.pattern, Some(ch));
                let idx = if sel.contains(&i) && tool.moves_groups() {
                    sel
                } else {
                    s.piano.clear_selection();
                    vec![i]
                };
                let notes = piano::notes(s);
                // The clicked note first: it is the one auditioned while dragging.
                let orig: Vec<(usize, Note)> =
                    std::iter::once(i).chain(idx.into_iter().filter(|&j| j != i)).filter_map(|j| notes.get(j).map(|&m| (j, m))).collect();
                s.piano.last_len = n.length.max(g.step);
                s.piano.last_controls = Some(n);
                let nr = g.note_rect(&n).unwrap_or_default();
                self.drag = Some(if x >= nr.right() - resize_zone(nr.w) {
                    self.resize_cursor = true;
                    Drag::NoteResize { x0: x, orig }
                } else {
                    let play = self.audition();
                    self.hold_key(Some(i32::from(n.key)), play);
                    Drag::NoteMove { x0: x, y0: y, orig }
                });
                // Held without a move, the notes are cloned (Click-and-hold & special gesture functions).
                if matches!(self.drag, Some(Drag::NoteMove { .. })) {
                    self.hold_start(crate::hold::Kind::Notes);
                }
            }
            Action::PianoVelocity => {
                piano::snapshot(s);
                self.drag = Some(Drag::Velocity { last_x: x, last_y: y });
                self.paint_velocity(x, y, x, y);
            }
            Action::PianoKeys => {
                let k = g.key_at(y).clamp(0, views::piano::TOP_KEY);
                self.drag = Some(Drag::PianoKeys { key: k });
                self.key_velocity = self.key_click_velocity(x);
                self.hold_key(Some(k), true);
            }
            _ => {}
        }
    }

    /// Playlist actions (spec: ui/playlist.md). `right` = the right button.
    fn playlist_act(&mut self, a: &Action, right: bool) -> bool {
        use crate::playlist as pl;
        let grid = self.pl_grid;
        match *a {
            Action::PlaylistArea => {
                if let Some(g) = grid {
                    if right && self.ctrl {
                        // Ctrl+right-drag zooms to a box (§3).
                        self.drag = Some(Drag::Pl(pl::PlDrag::ZoomBox { x0: self.mouse.0, y0: self.mouse.1 }));
                    } else {
                        pl::press_grid(self, g, right);
                    }
                }
            }
            Action::PlRuler => {
                if let Some(g) = grid {
                    pl::press_ruler(self, g, right);
                }
            }
            Action::PlMarkers => {
                if let Some(g) = grid {
                    pl::press_markers(self, g, right);
                }
            }
            Action::PlHeader(t) => {
                let at = self.top_hit(self.mouse.0, self.mouse.1).map_or(self.mouse, |h| (h.rect.x, h.rect.y));
                pl::press_header(self, t, right, at);
            }
            Action::PlLed(t) => pl::press_led(self, t, right),
            Action::PlGroupArrow(t) => {
                pl::toggle_collapse(&mut self.state, t);
                self.invalidate();
            }
            Action::PlTool(t) if !right => {
                self.state.playlist.tool = t;
                self.state.playlist.overlay = None;
            }
            Action::PlSnap => {
                let (x, y) = self.mouse;
                self.state.playlist.popup = Some(pl::snap_menu(&self.state, x, y + 10.0));
            }
            Action::PlOptions => {
                let (x, y) = self.mouse;
                self.state.playlist.popup = Some(pl::options_menu(&self.state, x - 6.0, y + 10.0));
            }
            Action::PlStretch => {
                self.state.message = pl::run(self, pl::Cmd::StretchMode);
            }
            Action::PlArrangement => {
                let r = self.top_hit(self.mouse.0, self.mouse.1).map(|h| h.rect).unwrap_or(Rect::new(self.mouse.0, self.mouse.1, 1.0, 20.0));
                self.state.playlist.arr_anchor = (r.x - 4.0, r.y);
                self.state.playlist.popup = Some(pl::arrangement_menu(&self.state, r.x, r.bottom() + 2.0));
            }
            Action::PlPickTab(k) => self.state.playlist.picker = k,
            Action::PlPick(src) => {
                self.state.playlist.source = Some(src);
                if let os_model::ClipSource::Pattern(n) = src {
                    self.state.pattern = n;
                }
                if right && matches!(src, os_model::ClipSource::Pattern(_)) {
                    // Right-click on a pattern: the PATTERNS commands for it (spec: ui/pattern-menu.md §3).
                    let (x, y) = self.mouse;
                    self.state.playlist.popup = Some(views::menus::pattern_popup(x, y));
                }
            }
            Action::PlPopup(i) => {
                let Some(p) = self.state.playlist.popup.as_mut() else { return true };
                let Some(item) = p.items.get(i).cloned() else { return true };
                if !item.children.is_empty() {
                    p.sub = if p.sub == Some(i) { None } else { Some(i) };
                } else if item.enabled && !item.head {
                    if let Some(m) = pl::run(self, item.cmd) {
                        self.state.message = Some(m);
                    }
                } else if let pl::Cmd::NotYet(name) = item.cmd {
                    // A greyed-out entry only says so; the menu stays open.
                    self.state.message = Some(off_text(name));
                }
            }
            Action::PlPopupSub(i) => {
                let item = self.state.playlist.popup.as_ref().and_then(|p| p.sub.and_then(|s| p.items.get(s))).and_then(|it| it.children.get(i)).cloned();
                match item.filter(|it| !it.head && !it.sep) {
                    Some(item) if item.enabled => {
                        if let Some(m) = pl::run(self, item.cmd) {
                            self.state.message = Some(m);
                        }
                    }
                    Some(pl::PItem { cmd: pl::Cmd::NotYet(name), .. }) => self.state.message = Some(off_text(name)),
                    _ => {}
                }
            }
            _ => return false,
        }
        // A pick in a dropdown's submenu closes the dropdown with it.
        if matches!(a, Action::PlPopup(_) | Action::PlPopupSub(_)) && self.state.playlist.popup.is_none() && self.state.menu_sub.take().is_some() {
            self.state.menu_open = None;
        }
        self.invalidate();
        true
    }

    pub fn act(&mut self, a: Action) {
        if let Action::Settings(sa) = a {
            return self.settings_act(sa);
        }
        if let Action::Scrollbar(t) = a {
            return self.scrollbar_press(t, false);
        }
        if let Action::ScrollStep(t, dir) = a {
            let step = self.bar(t).map_or(48.0, |b| (b.view / 8.0).max(18.0).round());
            return self.eased(|app| app.scroll_set(t, app.scroll_get(t) + dir as f32 * step));
        }
        self.note_host_tweak(&a);
        if self.playlist_act(&a, false) {
            return;
        }
        if self.browser_act(&a) {
            return;
        }
        if self.rack_act(&a) {
            return;
        }
        if a == Action::PianoSplitter {
            return self.piano_split_press();
        }
        if matches!(a, Action::PianoGrid | Action::PianoNote(_) | Action::PianoVelocity | Action::PianoKeys) {
            self.piano_click(&a);
            return;
        }
        if a == Action::PianoRuler {
            if let Some(g) = self.piano_geom() {
                let t = crate::piano_snap::place(&self.state, &g, g.tick_at(self.mouse.0), self.alt);
                if self.state.song_mode && self.piano_song_seek(t, true) {
                    self.drag = Some(Drag::SongRuler);
                } else {
                    self.state.piano.marker = t;
                    self.drag = Some(Drag::Marker);
                }
            }
            return;
        }
        let s = &mut self.state;
        match a {
            Action::PianoGrid | Action::PianoNote(_) | Action::PianoVelocity | Action::PianoKeys | Action::PianoRuler | Action::PianoSplitter | Action::Scrollbar(_) | Action::ScrollStep(..) => {}
            Action::PianoTool(t) => {
                s.piano.tool = t;
                s.piano.overlay = None;
            }
            // EDIT > Undo, also the toolbar's Undo button (spec: ui/toolbar-buttons.md §2).
            Action::PianoUndo | Action::Util(Util::Undo) => s.message = crate::history::ctrl_z(s),
            Action::Typing => self.toggle_typing(),
            Action::Metronome => self.toggle_metronome(),
            Action::AutoScroll => self.toggle_autoscroll(),
            Action::EnableGroups => self.toggle_groups(),
            Action::StepEdit => self.toggle_step_edit(),
            // spec: ui/main-window.md §13: a double click opens the Plugin performance monitor; one click does nothing.
            Action::CpuPanel => {
                let now = std::time::Instant::now();
                let double = self.cpu_click.is_some_and(|t| now.duration_since(t).as_millis() < 400);
                self.cpu_click = if double { None } else { Some(now) };
                if double {
                    self.open_perf();
                }
            }
            Action::Perf(p) => self.perf_act(p),
            Action::Tapper(a) => self.tapper_act(a),
            Action::Placeholder(name) => s.message = Some(off_text(name)),
            Action::Monitor => {
                s.monitor.toggle();
                self.damage_rect(MONITOR_RECT);
                // The hint (frequency read-out) follows the new mode.
                let (x, _) = self.mouse;
                self.hint = Cow::Owned(self.state.monitor.hint((x - MONITOR_RECT.x - 1.0).max(0.0) as usize));
                self.damage_rect(HINT_RECT);
            }
            Action::Minimize => self.want_minimize = true,
            Action::Maximize => self.want_maximize = true,
            Action::Close => self.want_close = true,
            Action::DragWindow => self.want_drag_window = true,
            Action::Play => {
                self.perf_transport();
                match self.transport.state() {
                    // Play toggles pause (the position is held in place); Stop rewinds.
                    TransportState::Playing => self.transport.pause(),
                    TransportState::Paused => self.transport.resume(),
                    TransportState::Stopped => self.start_playback(),
                }
            }
            Action::Stop => {
                self.perf_transport();
                self.transport.stop();
                // Stop rewinds: the song position slider goes back to the start (of the time selection, if any).
                let s = &mut self.state;
                if s.song_mode {
                    s.playlist.pos = s.playlist.range.map_or(0, |(a, _)| a);
                    // A song position move: the initial values (spec: ui/settings-choices.md "Advanced: Initialize
                    // controls automatically").
                    crate::init::apply(s);
                } else {
                    s.piano.marker = 0;
                }
                self.invalidate();
            }
            Action::PatSong(song) => {
                let changed = s.song_mode != song;
                s.song_mode = song;
                // The running playback follows the switch: it restarts in the new mode (a paused one is stopped),
                // so the playheads always show what is heard.
                if changed && self.transport.playing() {
                    let was_playing = !self.transport.paused();
                    self.transport.stop();
                    if was_playing {
                        self.start_playback();
                    }
                }
            }
            Action::MonParam(i) => {
                self.drag = Some(Drag::MonParam(i));
                let x = self.mouse.0;
                self.set_mon_param(i, x);
            }
            Action::MonReset => {
                s.monitor.cfg = Default::default();
                self.invalidate();
            }
            Action::MonSave => {
                s.message = Some(match save_monitor_cfg(&s.monitor.cfg) {
                    Ok(p) => format!("Monitor settings saved to {}", p.display()),
                    Err(e) => format!("Monitor settings not saved: {e}"),
                });
                self.invalidate();
            }
            Action::MonClose => {
                s.monitor.debug = false;
                self.invalidate();
            }
            Action::CableDebug(d) => self.cable_debug(d),
            Action::Tempo => {
                // The digit under the press sets the step: the whole number, else the decimal place it is over.
                let x = self.mouse.0;
                let n = s.tempo_digits.get().iter().filter(|&&d| x >= d).count();
                let step = [1.0, 0.1, 0.01, 0.001][n];
                self.drag = Some(Drag::Tempo { y0: self.mouse.1, t0: s.tempo, step });
            }
            Action::SongPos => self.song_pos_press(),
            Action::Clock => {
                s.clock.toggle();
                s.clock.save();
                self.invalidate();
            }
            Action::GlobalSnap => {
                // Its top-left at the combo's bottom-left (spec: ui/global-snap.md §2).
                let b = self.hits.iter().rev().find(|h| h.action == Action::GlobalSnap).map_or(Rect::new(self.mouse.0, self.mouse.1, 0.0, 0.0), |h| h.rect);
                s.playlist.popup = Some(crate::snap::menu(s, b.x, b.bottom()));
            }
            Action::GlobalSnapToggle => s.message = Some(crate::snap::toggle(s)),
            Action::Menu(i) => {
                s.pat_ui.menu_at = None;
                s.menu_open = if s.menu_open == Some(i) { None } else { Some(i) };
                s.menu_sub = None;
            }
            Action::MenuItem(m, i) => {
                s.menu_open = None;
                self.menu_item(m, i);
            }
            Action::MenuSub(m, i) => self.open_menu_sub(m, i),
            // Show a hidden window, bring a covered one to the front, hide one already in front
            // (spec: ui/channel-rack.md §5).
            Action::ToggleWin(Win::Browser) => s.show_browser = !s.show_browser,
            Action::ToggleWin(Win::Perf) => self.toggle_perf(),
            Action::ToggleWin(Win::Tapper) => self.toggle_tapper(),
            Action::ToggleWin(w) => {
                if s.shown(w) && s.in_front(w) {
                    s.hide(w);
                } else {
                    s.raise(w);
                }
                if w == Win::PianoRoll && s.shown(Win::PianoRoll) {
                    piano::center_keys(s, self.piano_grid.map_or(700.0, |g| g.h));
                }
            }
            // spec: ui/pattern-menu.md §1
            Action::PatternPrev => {
                s.pat_ui.menu_at = Some((837.0, 75.0));
                s.menu_open = Some(views::menus::PATTERNS);
            }
            Action::PatternNext => self.drag = Some(Drag::PatternBox { y0: self.mouse.1, p0: s.pattern.max(1) }),
            Action::PatternAdd => {
                crate::patterns::run(s, crate::patterns::Cmd::FindFirst, crate::patterns::PLUS_BOX_AT);
            }
            Action::Pat(h) => crate::patterns::hit(s, h),
            Action::Dlg(h) => self.dialog_click(h),
            Action::SongInfo => self.close_song_info(),
            Action::PlPickList => {}
            Action::PlPickAdd => {
                let at = crate::patterns::box_at_mouse(self.mouse);
                crate::patterns::run(s, crate::patterns::Cmd::FindFirst, at);
                s.playlist.source = None;
            }
            Action::Mix(crate::mixer::MHit::EqGraph) => {
                if let Some(g) = self.top_hit(self.mouse.0, self.mouse.1).map(|h| h.rect) {
                    let (track, band) = (self.state.selected_mixer, crate::mixer::eq_band_at(&self.state, g, self.mouse.0));
                    self.drag = Some(Drag::EqGraph { track, band, g });
                    crate::mixer::eq_graph_drag(&mut self.state, track, band, g, self.mouse);
                    self.damage_eq();
                }
            }
            Action::Mix(h) => {
                let plain_strip = matches!(h, crate::mixer::MHit::Strip(_)) && !self.ctrl && !self.shift;
                if let Some(m) = crate::mixer::hit(self, h) {
                    self.state.message = Some(m);
                }
                if plain_strip {
                    self.drag = Some(Drag::MixSelect);
                }
            }
            Action::Fader(t) => {
                crate::mixer::select(s, t, self.ctrl);
                let v0 = s.project.mixer.iter().find(|m| m.index == t).and_then(|m| m.volume).unwrap_or(0.8);
                self.drag = Some(Drag::Fader { track: t, y0: self.mouse.1, v0 });
            }
            Action::ChannelSelect(_) | Action::RackSelect(_) => {}
            Action::RackRollView => s.rack_roll_view = !s.rack_roll_view,
            Action::RackRoll(i) => {
                s.selected_channel = i;
                s.piano.clear_selection();
                s.raise(Win::PianoRoll);
                piano::center_keys(s, self.piano_grid.map_or(700.0, |g| g.h));
            }
            Action::RackMixer(_) => {}
            Action::RackAdd => {
                let at = self.mouse;
                self.open_plugin_menu(crate::plugin_menu::Target::Gen(None), at);
            }
            // Under the button (spec: ui/plugin-menu.md §1).
            Action::PickerOpen => {
                let (x, y) = self.mouse;
                let at = self.top_hit(x, y).map_or((x, y), |h| (h.rect.x, h.rect.bottom() + 2.0));
                self.open_plugin_menu(crate::plugin_menu::Target::All, at);
            }
            Action::Knob(id) => {
                crate::history::knob(s, id, false);
                self.drag = Some(Drag::Knob { id, y0: self.mouse.1, v0: s.knob(id) });
                self.hint = Cow::Owned(s.knob_text(id));
            }
            Action::PianoLaneMenu => s.lane_menu = true,
            Action::PianoToolsMenu => {
                let (x, y) = self.mouse;
                s.playlist.popup = Some(crate::piano_tools::tools_menu(s, x - 6.0, y + 10.0));
            }
            Action::PianoTarget => {
                // Top-left 4 px left of the name, 11 px below the title text's centre (spec: ui/piano-target-channel.md §2).
                let b = self.hits.iter().rev().find(|h| h.action == Action::PianoTarget).map_or(Rect::new(self.mouse.0, self.mouse.1, 0.0, 0.0), |h| h.rect);
                s.playlist.popup = Some(crate::piano_tools::target_menu(s, b.x + 2.0 - 4.0, b.cy() + 11.0));
            }
            Action::PianoSnapMenu => {
                // Top-left 11 px left of and below the button's centre (spec: ui/piano-snap.md §1).
                let b = self.hits.iter().rev().find(|h| h.action == Action::PianoSnapMenu).map_or(Rect::new(self.mouse.0, self.mouse.1, 0.0, 0.0), |h| h.rect);
                s.playlist.popup = Some(crate::piano_snap::snap_menu(s, b.cx() - 11.0, b.cy() + 11.0));
            }
            Action::PitchStep(dir) => {
                s.piano.pitch_center = piano::clamp_pitch_center(s.piano.pitch_center + dir * piano::PITCH_STEP);
            }
            Action::PianoLanePick(i) => {
                s.piano.lane = piano::Lane::ALL[i.min(piano::Lane::ALL.len() - 1)];
                s.lane_menu = false;
            }
            Action::Settings(_) => {}
            Action::RackResize(edges) => {
                self.drag = Some(Drag::RackResize { edges, m0: self.mouse, s0: s.rack_size });
            }
            Action::ChannelEnable(_)
            | Action::Step(..)
            | Action::RackOptions
            | Action::RackFilter
            | Action::RackDots
            | Action::RackSpeaker
            | Action::RackLen
            | Action::RackLoop
            | Action::RackGraph
            | Action::RackLoopBox(_)
            | Action::RackGraphArea
            | Action::RackGraphTab(_)
            | Action::RackFill(_) => {}
            Action::WinDrag(win) => {
                // Double-click on the title bar toggles maximized / floating (like the max button).
                let now = std::time::Instant::now();
                let m = self.mouse;
                let double = matches!(self.title_click, Some((w, t, p)) if w == win
                    && now.duration_since(t).as_millis() < 500
                    && (p.0 - m.0).abs() < 4.0 && (p.1 - m.1).abs() < 4.0);
                if double && win.slot().is_some() {
                    self.title_click = None;
                    self.act(Action::MaxWin(win));
                    return;
                }
                self.title_click = Some((win, now, m));
                let s = &mut self.state;
                if s.is_max(win) {
                    return;
                }
                let pos = match win {
                    Win::Plugin => s.plugin_pos,
                    Win::Perf => s.perf.pos,
                    Win::Tapper => s.tapper.pos,
                    Win::ChannelRack => s.rack_pos,
                    w => {
                        // Start from the rect as shown (kept on screen), then move it.
                        let r = s.float_rect(w);
                        if let Some(i) = w.slot() {
                            s.wins[i].rect = r;
                        }
                        (r.x, r.y)
                    }
                };
                self.drag = Some(Drag::Window { win, dx: self.mouse.0 - pos.0, dy: self.mouse.1 - pos.1 });
            }
            Action::WinResize(win, edges) => {
                let r0 = s.float_rect(win);
                self.drag = Some(Drag::WinResize { win, edges, m0: self.mouse, r0 });
            }
            Action::MinWin(w) => {
                if let Some(i) = w.slot() {
                    s.wins[i].mini = !s.wins[i].mini;
                }
            }
            Action::MaxWin(w) => {
                if let Some(i) = w.slot() {
                    s.wins[i].mini = false;
                    s.wins[i].maxed = !s.wins[i].maxed;
                }
            }
            // A second one leaves it maximized (spec: ui/toolbar-buttons.md §2).
            Action::ShowMax(w) => {
                s.raise(w);
                if let Some(i) = w.slot() {
                    s.wins[i].mini = false;
                    s.wins[i].maxed = true;
                }
                if w == Win::PianoRoll {
                    piano::center_keys(s, self.piano_grid.map_or(700.0, |g| g.h));
                }
            }
            // No help index yet: the left click does nothing.
            Action::Util(Util::Help) => {}
            Action::Util(Util::Save) => self.menu_item(0, views::menus::FILE_SAVE_AS),
            Action::CloseWin(Win::Perf) => self.close_perf(),
            Action::CloseWin(w) => s.hide(w),
            Action::BrowserRow(_) | Action::BrowserTab(_) | Action::BrowserList | Action::BrowserCollapse | Action::BrowserReread | Action::BrowserMenu | Action::BrowserSearch => {}
            Action::PlaylistArea | Action::MixerArea | Action::RackArea | Action::Swallow | Action::TypeIn(_) => {}
            Action::PlTool(_)
            | Action::PlSnap
            | Action::PlOptions
            | Action::PlRuler
            | Action::PlMarkers
            | Action::PlHeader(_)
            | Action::PlLed(_)
            | Action::PlGroupArrow(_)
            | Action::PlArrangement
            | Action::PlStretch
            | Action::PlPickTab(_)
            | Action::PlPick(_)
            | Action::PlPopup(_)
            | Action::PlPopupSub(_) => {}
        }
    }

    /// Make `i` the focused channel (piano roll, typing keyboard, plugin window follow it, spec: ui/channel-rack.md §8).
    pub(crate) fn focus_channel(&mut self, i: usize) {
        let s = &mut self.state;
        if s.selected_channel != i && s.shown(Win::PianoRoll) {
            s.selected_channel = i;
            s.piano.clear_selection();
            piano::center_keys(s, self.piano_grid.map_or(700.0, |g| g.h));
        }
        s.selected_channel = i;
    }

    /// Name button click (spec: ui/channel-rack.md §13): opens the channel's plugin window, or closes it when open.
    fn toggle_channel_window(&mut self, i: usize) {
        if self.state.hosted_rows.contains(&i) {
            if !self.plugins.close_if_open(i) {
                self.state.pending_open = Some(i);
            }
            return;
        }
        let s = &mut self.state;
        let sampler = s.project.channels.get(i).is_some_and(|c| c.kind.is_sampler());
        if !sampler && s.shown(Win::Plugin) && s.plugin_channel == i {
            s.hide(Win::Plugin);
        } else {
            self.open_channel_window(i);
        }
    }

    /// Open rack row `i`'s window (not hosted: Lemon Sampler's for a sampler or audio clip, else the channel window).
    pub(crate) fn open_channel_window(&mut self, i: usize) {
        let s = &mut self.state;
        if s.project.channels.get(i).is_some_and(|c| c.kind.is_sampler()) {
            // Not hosted yet (just added): Lemon Sampler starts on the next tick and its window opens then.
            s.pending_open = Some(i);
            self.sampler_failed.retain(|&r| r != i);
        } else {
            if !s.shown(Win::Plugin) {
                let r = views::rack::rect(s);
                s.plugin_pos = ((r.right() + 8.0).min(1920.0 - 440.0), r.y.min(1080.0 - 210.0));
            }
            s.plugin_channel = i;
            s.raise(Win::Plugin);
        }
    }

    /// Ctrl+Z (`alt` = Ctrl+Alt+Z) and EDIT's entries: a move in the project's undo history (`crate::history`; spec:
    /// ui/settings-choices.md "Undo history: the four checks (2026-10-16)").
    pub(crate) fn undo_key(&mut self, alt: bool) {
        let s = &mut self.state;
        s.message = if alt { crate::history::ctrl_alt_z(s) } else { crate::history::ctrl_z(s) };
        self.invalidate();
    }

    /// ADD > Audio clip / Automation clip (spec: ui/add-clip-channels.md §3): the channel goes last in the rack (kept
    /// in view) and its window opens: an audio clip's settings (Lemon Sampler's window), an automation clip's channel
    /// window.
    pub(crate) fn add_clip_channel(&mut self, kind: ChannelKind) {
        let row = crate::playlist::add_clip_channel(&mut self.state, kind);
        let s = &mut self.state;
        let view_h = views::rack::panel(views::rack::rect(s)).h;
        s.rack_scroll = (views::rack::content_h(s) - view_h).max(0.0);
        self.open_channel_window(row);
        self.invalidate();
    }

    /// Press on a step with the left button, or the `right` one (spec: ui/channel-rack.md §9): selects the row, sets
    /// the step and starts painting. "Use both mouse buttons in step sequencer" (spec: ui/settings-choices.md "Input:
    /// Click-and-hold and both mouse buttons (2026-10-16)"): off, either button toggles the step; on, the left one sets
    /// it and the right one clears it. Either way the drag that follows sets (left) or clears (right) each step it
    /// crosses.
    fn rack_step(&mut self, i: usize, k: u32, right: bool) {
        self.state.rack.select_only(i);
        self.focus_channel(i);
        let s = &mut self.state;
        s.piano.clear_selection();
        // One undo step for the press and the painting that follows.
        crate::rack::snapshot(s);
        // A step past an explicit length grows the pattern to the end of the pattern's bar holding it (§5, §23).
        if let Some(l) = crate::rack::explicit_len(s) {
            if k >= l {
                let ps = crate::rack::per_step(s);
                crate::rack::set_len(s, Some(s.project.pattern_bar_end(s.pattern, (k + 1) * ps).div_ceil(ps)));
            }
        }
        if crate::prefs::check(&crate::prefs::BOTH_MOUSE_BUTTONS) {
            crate::rack::set_step(s, i, k, !right);
        } else {
            crate::rack::toggle_step(s, i, k);
        }
        self.drag = Some(Drag::RackPaint { row: i, on: !right, last: k });
    }

    /// Speaker (spec: ui/channel-rack.md §7): plays the current pattern; while it plays, pauses / resumes.
    fn rack_speaker(&mut self) {
        match self.transport.state() {
            TransportState::Playing if !self.state.song_mode => self.transport.pause(),
            TransportState::Paused if !self.state.song_mode => self.transport.resume(),
            _ => {
                if self.transport.playing() {
                    self.transport.stop();
                }
                self.state.song_mode = false;
                self.start_playback();
            }
        }
    }

    /// Graph editor click / drag at a screen point (spec: ui/channel-rack.md §14).
    fn rack_graph_at(&mut self, (x, y): (f32, f32)) {
        let Some(b) = views::rack::graph_bars(&self.state) else { return };
        let Some(k) = views::rack::step_at(&self.state, x) else { return };
        let v = ((b.bottom() - y) / b.h).clamp(0.0, 1.0);
        if let Some(t) = crate::rack::graph_set(&mut self.state, k, v) {
            self.hint = Cow::Owned(t);
        }
        self.invalidate();
    }

    /// Channel rack left clicks (spec: ui/channel-rack.md §7-15, §22). Returns whether `a` was one.
    fn rack_act(&mut self, a: &Action) -> bool {
        use crate::rack as rk;
        let (shift, ctrl, alt) = (self.shift, self.ctrl, self.alt);
        let now = std::time::Instant::now();
        let double = matches!(&self.rack_click, Some((p, t)) if p == a && now.duration_since(*t).as_millis() < 400);
        if matches!(a, Action::RackSelect(_) | Action::RackMixer(_)) {
            self.rack_click = if double { None } else { Some((a.clone(), now)) };
        }
        let mouse = self.mouse;
        match *a {
            Action::RackSelect(i) => {
                if double {
                    self.state.rack.sel = rk::shown(&self.state).into_iter().collect();
                } else if shift {
                    let sel = &mut self.state.rack.sel;
                    if !sel.remove(&i) {
                        sel.insert(i);
                    }
                } else {
                    self.state.rack.select_only(i);
                    self.focus_channel(i);
                    self.drag = Some(Drag::RackSel { from: i });
                }
            }
            Action::ChannelSelect(i) => {
                if shift {
                    let at = views::rack::name_rect(&self.state, i).map_or(mouse, |r| (r.x, r.y));
                    rk::open_box(&mut self.state, rk::RBox::Channel(i), at);
                } else if !ctrl {
                    self.state.rack.select_only(i);
                    self.focus_channel(i);
                    self.toggle_channel_window(i);
                }
            }
            Action::ChannelEnable(i) => {
                let s = &mut self.state;
                if ctrl || alt {
                    rk::solo(s, i);
                } else if shift {
                    rk::toggle_lock(s, i);
                } else {
                    rk::toggle_mute(s, i);
                }
            }
            Action::Step(i, k) => self.rack_step(i, k, false),
            Action::RackMixer(i) => {
                let s = &mut self.state;
                if double {
                    if let Some(t) = s.project.channels.get(i).map(|c| c.mixer_track) {
                        s.selected_mixer = t;
                        s.raise(Win::Mixer);
                    }
                } else {
                    let v0 = s.project.channels.get(i).map_or(0, |c| c.mixer_track);
                    self.drag = Some(Drag::RackMixer { ch: i, y0: mouse.1, v0 });
                }
            }
            Action::RackOptions => {
                let p = rk::options_menu(&self.state, self.state.rack_pos.0, self.state.rack_pos.1 + 28.0);
                self.state.playlist.popup = Some(p);
            }
            Action::RackFilter => {
                let p = rk::filter_menu(&self.state, mouse.0, mouse.1 + 8.0);
                self.state.playlist.popup = Some(p);
            }
            Action::RackDots => self.drag = Some(Drag::RackDots { x0: mouse.0, p0: self.state.rack.name_pct }),
            Action::RackSpeaker => self.rack_speaker(),
            Action::RackLen => self.drag = Some(Drag::RackLen { y0: mouse.1, v0: rk::len_value(&self.state) }),
            Action::RackLoop => {
                let s = &mut self.state;
                rk::relayout(s, |s| s.rack.loop_mode = !s.rack.loop_mode);
                s.piano.dirty = true;
            }
            Action::RackGraph => self.state.rack.graph_open = !self.state.rack.graph_open,
            Action::RackLoopBox(i) => self.drag = Some(Drag::RackLoopLen { row: i, y0: mouse.1, v0: rk::row_len(&self.state, i) }),
            Action::RackGraphArea => {
                self.rack_graph_at(mouse);
                self.drag = Some(Drag::RackGraph);
            }
            Action::RackGraphTab(i) => self.state.rack.graph_tab = rk::Tab::ALL[i.min(rk::Tab::ALL.len() - 1)],
            Action::RackFill(h) => rk::fill_hit(&mut self.state, h),
            _ => return false,
        }
        self.invalidate();
        true
    }

    /// Channel rack right clicks (spec: ui/channel-rack.md §8-19). Returns whether `a` was one.
    fn rack_right(&mut self, a: &Action) -> bool {
        use crate::knobs::KnobId;
        use crate::rack as rk;
        let (x, y) = self.mouse;
        let s = &self.state;
        let popup = match *a {
            Action::ChannelSelect(i) => rk::channel_menu(s, i, x, y),
            Action::ChannelEnable(i) => rk::led_menu(s, i, x, y),
            Action::RackMixer(i) => rk::mixer_menu(s, i, x, y),
            Action::Knob(k @ (KnobId::ChanVol(_) | KnobId::ChanPan(_) | KnobId::Swing)) => rk::knob_menu(s, k, self.automation_target(a), x, y),
            Action::RackFilter => rk::filter_edit_menu(s, x, y),
            Action::RackLen => rk::length_menu(s, x, y),
            Action::RackLoop => rk::loop_menu(s, x, y),
            Action::RackSelect(i) => {
                let sel = &mut self.state.rack.sel;
                if !sel.remove(&i) {
                    sel.insert(i);
                }
                return true;
            }
            Action::Step(i, k) => {
                self.rack_step(i, k, true);
                return true;
            }
            Action::RackGraphArea => {
                self.rack_graph_at((x, y));
                self.drag = Some(Drag::RackGraph);
                return true;
            }
            _ => return false,
        };
        self.state.playlist.popup = Some(popup);
        true
    }

    /// Channels whose deletion was confirmed go now (spec: ui/channel-rack.md §18).
    pub(crate) fn rack_pending(&mut self) {
        if let Some(rows) = self.state.rack.delete_now.take() {
            crate::rack::delete_rows(self, &rows);
        }
    }

    /// Rack rows were renumbered (`m(old) = new`, `None` = deleted): hosted plugins, their windows and playback follow.
    pub(crate) fn remap_rows(&mut self, m: &dyn Fn(usize) -> Option<usize>) {
        self.typing_release_all();
        self.plugins.remap(m);
        self.sampler_failed = self.sampler_failed.iter().filter_map(|&r| m(r)).collect();
        self.wrap_failed = self.wrap_failed.iter().filter_map(|&k| if k >= crate::plugins::FX_KEY { Some(k) } else { m(k) }).collect();
        self.native_failed = self.native_failed.iter().filter_map(|&k| if k >= crate::plugins::FX_KEY { Some(k) } else { m(k) }).collect();
        self.transport.set_instruments(self.plugins.instruments());
        self.state.piano.dirty = true;
    }

    /// A row's hosted instrument for cloning: (available plugin index, saved state).
    pub(crate) fn plugin_copy(&self, r: usize) -> Option<(usize, Vec<u8>)> {
        let ch = self.state.project.channels.get(r)?;
        let name = match ch.kind {
            ChannelKind::Sampler | ChannelKind::AudioClip => crate::plugins::LEMON_SAMPLER.to_string(),
            // A hosted plugin is found by its identity (a wrapped channel's plugin name is the wrapper's).
            ChannelKind::Plugin => {
                let d = self.plugins.descriptor(r)?;
                let index = self.plugins.index_of_uid(&d.uid())?;
                return Some((index, self.plugins.state(r)?));
            }
            _ => return None,
        };
        let index = self.plugins.index_of(&name)?;
        let blob = self.plugins.hosted.get(&r)?.plugin.lock().ok()?.save_state()?;
        Some((index, blob))
    }

    /// Host a copy of an instrument (`plugin_copy`) on row `row`.
    pub(crate) fn plugin_paste(&mut self, row: usize, index: usize, blob: &[u8]) {
        if self.plugins.create_with(row, index, Some(blob)).is_ok() {
            if !self.state.hosted_rows.contains(&row) {
                self.state.hosted_rows.push(row);
            }
            self.transport.set_instruments(self.plugins.instruments());
        }
    }

    /// A PATTERNS command from the menu bar, the pattern menu or its key: the ones that open dialogs here, the others
    /// in `patterns::run` with a name box at the mouse. Returns a status message.
    pub(crate) fn pattern_cmd(&mut self, cmd: crate::patterns::Cmd) -> Option<String> {
        match cmd {
            crate::patterns::Cmd::TimeSignature => {
                self.ask_time_signature();
                None
            }
            crate::patterns::Cmd::OpenInBrowser => {
                self.browser_jump(crate::browser::Jump::Patterns);
                None
            }
            _ => {
                let at = crate::patterns::box_at_mouse(self.mouse);
                crate::patterns::run(&mut self.state, cmd, at)
            }
        }
    }

    pub(crate) fn menu_item(&mut self, menu: usize, item: usize) {
        use views::menus::*;
        match (menu, item) {
            (0, FILE_NEW) => self.ask_discard("starting a new project", crate::dialog::Next::NewProject),
            // Theme import (spec: ui/flstheme.md): pick a .flstheme, starting in FL's theme folder when installed.
            (OPTIONS_MENU | ADD_MENU, i) if item_at(menu, i).and_then(|it| views::settings::tab_for_menu(it.label)).is_some() => {
                let tab = item_at(menu, i).and_then(|it| views::settings::tab_for_menu(it.label)).expect("checked");
                self.open_settings(tab);
            }
            (0, FILE_OPEN) => self.ask_discard("opening another project", crate::dialog::Next::OpenDialog),
            (0, FILE_SAVE) => self.save(false),
            (0, FILE_EXPORT) => self.export_wav(),
            // ADD > Sampler: a new Lemon Sampler channel at the bottom of the rack (spec: ui/plugin-menu.md §1).
            (ADD_MENU, i) if item_at(ADD_MENU, i).is_some_and(|it| it.label == "Sampler") => {
                if let Some(index) = self.plugins.index_of(crate::plugins::LEMON_SAMPLER) {
                    self.state.message = crate::plugin_menu::add_generator(self, index, None);
                }
            }
            // ADD > Audio clip / Automation clip (spec: ui/add-clip-channels.md §3).
            (ADD_MENU, i) if item_at(ADD_MENU, i).is_some_and(|it| matches!(it.label, "Audio clip" | "Automation clip")) => {
                let audio = item_at(ADD_MENU, i).is_some_and(|it| it.label == "Audio clip");
                self.add_clip_channel(if audio { ChannelKind::AudioClip } else { ChannelKind::Automation });
            }
            (ADD_MENU, i) if item_at(ADD_MENU, i).is_some_and(|it| it.label == "More plugins...") => {
                let at = self.mouse;
                self.open_plugin_menu(crate::plugin_menu::Target::Gen(None), at);
            }
            (ADD_MENU, i) if item_at(ADD_MENU, i).is_some_and(|it| it.label == "Refresh plugin list (fast scan)") => self.start_scan(crate::plugin_db::Mode::Changed),
            // The plugin menus' display mode, as their own radios set it (spec: ui/plugin-menu.md §2-3).
            (ADD_MENU, i) if item_at(ADD_MENU, i).and_then(|it| menu_view(ADD_MENU, it.label)).is_some() => {
                let v = item_at(ADD_MENU, i).and_then(|it| menu_view(ADD_MENU, it.label)).expect("checked");
                self.state.message = crate::plugin_menu::run(self, crate::plugin_menu::PlugCmd::View(v));
            }
            (HELP_MENU, HELP_ABOUT) => {
                self.state.message = Some(concat!("Lemon Studio ", env!("CARGO_PKG_VERSION"), " (pre-alpha), free software under the GNU GPL v3 or later").into());
            }
            (ADD_MENU | TOOLS_MENU, i) if item_at(menu, i).is_some_and(|it| it.label == "Pattern") => {
                let at = crate::patterns::box_at_mouse(self.mouse);
                if let Some(m) = crate::patterns::run(&mut self.state, crate::patterns::Cmd::FindFirst, at) {
                    self.state.message = Some(m);
                }
            }
            (TOOLS_MENU, i) if item_at(TOOLS_MENU, i).is_some_and(|it| it.label == "Stop sound") => {
                self.transport.stop();
                self.state.message = Some("Sound stopped".into());
            }
            // spec: ui/unused-channels.md
            (TOOLS_MENU, i) if item_at(TOOLS_MENU, i).is_some_and(|it| it.label == "Select unused channels") => {
                self.state.message = crate::rack::run(self, crate::rack::RCmd::SelectUnused);
            }
            (TOOLS_MENU, i) if item_at(TOOLS_MENU, i).is_some_and(|it| it.label == "Purge unused audio clips") => {
                self.state.message = crate::rack::run(self, crate::rack::RCmd::Purge);
            }
            (TOOLS_MENU, i) if item_at(TOOLS_MENU, i).is_some_and(|it| it.label == "Reset empty playlist tracks") => {
                if let Some(m) = crate::playlist::run(self, crate::playlist::Cmd::ResetEmptyTracks) {
                    self.state.message = Some(m);
                }
            }
            (0, FILE_SAVE_AS) => self.save(true),
            (0, FILE_SAVE_VERSION) => self.save_new_version(),
            (0, FILE_EXIT) => self.want_close = true,
            (VIEW | ADD_MENU, i) if item_at(menu, i).is_some_and(|it| it.label == "Plugin picker") => {
                let at = self.mouse;
                self.open_plugin_menu(crate::plugin_menu::Target::All, at);
            }
            (PATTERNS, i) if i >= PATTERN_PICK => self.state.pattern = (i - PATTERN_PICK) as u16,
            (PATTERNS, i) if item_at(PATTERNS, i).and_then(|it| crate::patterns::Cmd::from_label(it.label)).is_some() => {
                let cmd = item_at(PATTERNS, i).and_then(|it| crate::patterns::Cmd::from_label(it.label)).expect("checked");
                if let Some(m) = self.pattern_cmd(cmd) {
                    self.state.message = Some(m);
                }
            }
            (VIEW, i) if item_at(VIEW, i).and_then(|it| CloseAll::from_label(it.label)).is_some() => {
                let what = item_at(VIEW, i).and_then(|it| CloseAll::from_label(it.label)).expect("checked");
                self.close_windows(what);
            }
            (VIEW, i) if item_at(VIEW, i).is_some_and(|it| it.label == INTO_WORKSPACE_LABEL) => self.move_into_workspace(),
            (m, i) if item_at(m, i).is_some_and(|it| it.label == TYPING_LABEL) => self.toggle_typing(),
            (m, i) if item_at(m, i).is_some_and(|it| it.label == views::menus::METRONOME_LABEL) => self.toggle_metronome(),
            (m, i) if item_at(m, i).is_some_and(|it| it.label == views::menus::AUTOSCROLL_LABEL) => self.toggle_autoscroll(),
            (m, i) if item_at(m, i).is_some_and(|it| it.label == views::menus::GROUPS_LABEL) => self.toggle_groups(),
            (m, i) if item_at(m, i).is_some_and(|it| it.label == views::menus::STEP_EDIT_LABEL) => self.toggle_step_edit(),
            (m, i) if item_at(m, i).is_some_and(|it| crate::params::menu_handles(it.label)) => {
                let label = item_at(m, i).map_or("", |it| it.label);
                if let Some(msg) = self.params_menu(label) {
                    self.state.message = Some(msg);
                }
            }
            // EDIT > Undo = Ctrl+Z, Redo = Ctrl+Alt+Z: the project's undo history (`undo_key`).
            (EDIT_MENU, i) if item_at(EDIT_MENU, i).is_some_and(|it| it.label == "Undo") => self.undo_key(false),
            (EDIT_MENU, i) if item_at(EDIT_MENU, i).is_some_and(|it| it.label == "Redo") => self.undo_key(true),
            // spec: ui/browser-commands.md
            (m, i) if item_at(m, i).and_then(|it| crate::browser::Jump::from_label(m, it.label)).is_some() => {
                let j = item_at(m, i).and_then(|it| crate::browser::Jump::from_label(m, it.label)).expect("checked");
                self.browser_jump(j);
            }
            (m, i) => {
                let label = item_at(m, i).map_or("item", |it| it.label);
                self.state.message = Some(format!("{label}: not available yet"));
            }
        }
    }

    /// A ▸ entry of the open dropdown (spec: ui/menus.md "Look"): its submenu, the app's own popup, opens to the right
    /// of the row as the popups' submenus do (kept on screen); the dropdown stays open under it.
    fn open_menu_sub(&mut self, menu: usize, i: usize) {
        let Some(fill) = views::menus::item_at(menu, i).and_then(|it| views::menus::submenu(menu, it.label)) else { return };
        let at = self.hits.iter().rev().find(|h| h.action == Action::MenuSub(menu, i)).map_or(Rect::new(self.mouse.0, self.mouse.1, 0.0, 0.0), |h| h.rect);
        let items = fill(&self.state);
        self.state.playlist.popup = Some(crate::playlist::Popup { x: at.right(), y: at.y - 2.0, items, sub: None });
        self.state.menu_sub = Some(i);
    }
}

impl App {
    /// A Toolbars menu switch (spec: ui/toolbar-menu.md §2-5, §8). Hide (Ctrl+F11) takes the toolbar and menu bar away
    /// and moves the browser and the workspace up to the window's top, the floating windows with them; a second Hide
    /// brings everything back.
    pub(crate) fn toolbar_switch(&mut self, sw: views::toolbar::Switch) {
        use views::toolbar::Switch;
        let s = &mut self.state;
        match sw {
            Switch::Separators => s.toolbar.separators = !s.toolbar.separators,
            Switch::Flat => s.toolbar.flat = !s.toolbar.flat,
            Switch::HintBar => s.toolbar.hint_bar = !s.toolbar.hint_bar,
            Switch::Hide => {
                s.toolbar.hidden = !s.toolbar.hidden;
                s.menu_open = None;
                s.menu_sub = None;
                let dy = if s.toolbar.hidden { -views::WORK_Y } else { views::WORK_Y };
                let top = views::work_y(s);
                let lift = |y: &mut f32| *y = (*y + dy).max(top);
                for w in &mut s.wins {
                    lift(&mut w.rect.y);
                }
                for pos in [&mut s.rack_pos, &mut s.plugin_pos, &mut s.perf.pos, &mut s.tapper.pos] {
                    lift(&mut pos.1);
                }
            }
        }
        if sw != Switch::Hide {
            s.toolbar.save();
        }
        self.invalidate();
    }
}

/// What a right click on toolbar button `hit` runs on its release (spec: ui/toolbar-buttons.md §2). The Undo button has
/// none until the browser has a History section.
fn toolbar_right(hit: &Action) -> Option<Action> {
    use views::menus::{FILE_SAVE_VERSION, HELP_ABOUT, HELP_MENU};
    Some(match *hit {
        Action::Util(Util::Save) => Action::MenuItem(0, FILE_SAVE_VERSION),
        Action::Util(Util::Help) => Action::MenuItem(HELP_MENU, HELP_ABOUT),
        Action::ToggleWin(w @ (Win::Playlist | Win::PianoRoll)) => Action::ShowMax(w),
        Action::ToggleWin(Win::Browser) => Action::PickerOpen,
        _ => return None,
    })
}

/// PATTERNS menu shortcuts (spec: ui/pattern-menu.md §3).
fn pattern_shortcut(key: &winit::keyboard::Key, ctrl: bool, shift: bool) -> Option<crate::patterns::Cmd> {
    use crate::patterns::Cmd;
    use winit::keyboard::{Key, NamedKey};
    let k = match key {
        // spec: ui/browser-commands.md §4
        Key::Character(c) if ctrl && shift && c.eq_ignore_ascii_case("p") => return Some(Cmd::OpenInBrowser),
        Key::Named(k) => k,
        _ => return None,
    };
    Some(match (k, ctrl, shift) {
        (NamedKey::F4, false, true) => Cmd::FindFirst,
        (NamedKey::F4, false, false) => Cmd::FindNext,
        (NamedKey::F4, true, false) => Cmd::FindNextNoName,
        (NamedKey::F2, false, false) => Cmd::Rename,
        (NamedKey::Insert, true, true) => Cmd::InsertOne,
        (NamedKey::Delete, true, true) => Cmd::Delete,
        (NamedKey::ArrowUp, true, true) => Cmd::MoveUp,
        (NamedKey::ArrowDown, true, true) => Cmd::MoveDown,
        _ => return None,
    })
}

/// Typing keyboard velocity (fixed, spec: ui/typing-keyboard.md).
const TYPING_VELOCITY: u8 = 100;
const TYPING_LABEL: &str = "Typing keyboard to piano";

/// Semitone above the lower tier's first C (C4) for a physical key (spec: ui/typing-keyboard.md).
fn typing_semitone(code: KeyCode) -> Option<i32> {
    use KeyCode::*;
    Some(match code {
        // Lower tier: white keys on the bottom row, black keys on the row above.
        KeyZ => 0, KeyS => 1, KeyX => 2, KeyD => 3, KeyC => 4, KeyV => 5, KeyG => 6, KeyB => 7,
        KeyH => 8, KeyN => 9, KeyJ => 10, KeyM => 11, Comma => 12, KeyL => 13, Period => 14,
        Semicolon => 15, Slash => 16,
        // Upper tier, one octave up.
        KeyQ => 12, Digit2 => 13, KeyW => 14, Digit3 => 15, KeyE => 16, KeyR => 17, Digit5 => 18,
        KeyT => 19, Digit6 => 20, KeyY => 21, Digit7 => 22, KeyU => 23, KeyI => 24, Digit9 => 25,
        KeyO => 26, Digit0 => 27, KeyP => 28, BracketLeft => 29, Equal => 30, BracketRight => 31,
        _ => return None,
    })
}

/// An empty sampler channel at rack position `i`.
fn sampler_channel(i: u16, name: &str, mixer_track: u16) -> Channel {
    Channel {
        index: i,
        kind: ChannelKind::Sampler,
        name: Some(name.into()),
        plugin: None,
        plugin_state: None,
        color: None,
        enabled: true,
        volume: 10000.0 / 12800.0,
        pan: 0.0,
        pitch_cents: 0,
        mixer_track,
        sample_path: None,
        layer_children: Vec::new(),
        envelopes: Default::default(),
        rack: Default::default(),
        misc: Default::default(), sound: Default::default(),
    }
}

/// What a plugin window's strip shows for rack row `row` (spec: ui/plugin-window.md).
#[cfg(plugin_windows)]
fn channel_info(p: &Project, row: usize) -> Option<crate::plugin_window::ChannelInfo> {
    let ch = p.channels.get(row)?;
    Some(crate::plugin_window::ChannelInfo {
        name: ch.name.clone().unwrap_or_else(|| format!("Channel {}", ch.index + 1)),
        // The mixer track's own name when it has one, else "Master" / "Insert N".
        target: p
            .mixer
            .iter()
            .find(|t| t.index == ch.mixer_track)
            .and_then(|t| t.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| views::rack::insert_name(ch.mixer_track)),
        enabled: ch.enabled,
        volume: ch.volume,
        pan: ch.pan,
        pitch_cents: ch.pitch_cents,
        range: 2,
        track: ch.mixer_track,
        misc: Some(crate::misc::vals_of(ch)),
    })
}

/// A generator-plugin channel played by a hosted instrument, routed like `add_channel`. Returns its rack row.
pub(crate) fn add_plugin_channel(p: &mut Project, name: &str) -> usize {
    let row = add_channel(p);
    let ch = &mut p.channels[row];
    ch.kind = ChannelKind::Plugin;
    // The built-in Lemon Trio is stored as the file's own 3x Osc.
    ch.plugin = Some(crate::osc3::file_name(name).to_string());
    ch.name = Some(name.to_string());
    row
}

/// Add-channel button (spec: ui/channel-rack.md §5): an empty sampler routed to the next insert
/// after the highest one in use. Returns its rack row.
pub(crate) fn add_channel(p: &mut Project) -> usize {
    let index = p.channels.iter().map(|c| c.index + 1).max().unwrap_or(0);
    let track = (p.channels.iter().map(|c| c.mixer_track).max().unwrap_or(0) + 1).min(crate::mixer::max_insert(p));
    p.channels.push(sampler_channel(index, crate::plugins::LEMON_SAMPLER, track));
    p.channels.len() - 1
}

/// A new project (owner's choice, 2026-10-10): one empty sampler channel, played and edited by Lemon Sampler, on
/// insert 1, and pattern 1.
pub fn default_project() -> Project {
    template_project(&[crate::plugins::LEMON_SAMPLER])
}

/// The project's bar as the Project page names it: "Time signature: 3/8" or "Time division: 5 beats of 3 steps".
fn bar_text(p: &Project) -> String {
    let (beats, steps) = p.bar();
    if p.bar_is_signature() {
        format!("Time signature: {beats}/{}", views::settings::denominator_text(steps))
    } else {
        format!("Time division: {beats} beats of {steps} steps")
    }
}

/// A one-channel project that plays `path` once from the start (browser sample preview, spec: ui/browser.md §7).
pub(crate) fn preview_project(path: &str) -> Project {
    let mut p = template_project(&["Preview"]);
    p.channels[0].sample_path = Some(path.to_string());
    p.patterns = vec![Pattern { number: 1, notes: vec![Note { length: u32::from(p.ppq.max(4)), ..Note::step(0, 0) }], ..Pattern::default() }];
    p.arrangements.clear();
    p
}

/// The old five-channel template (tests that need several channels).
#[cfg(test)]
pub fn test_project() -> Project {
    template_project(&["Kick", "Clap", "Hat", "Snare", "Pluck"])
}

/// Sampler channels named `names` (channel i on insert i + 1), 18 mixer tracks (master, Insert 1-16 and the hidden
/// last one, as FL's default template stores them; spec: ui/mixer.md §1) routed to the master, pattern 1.
fn template_project(names: &[&str]) -> Project {
    let channel = |i: u16, name: &str| sampler_channel(i, name, i + 1);
    let mut mixer: Vec<MixerTrack> = (0..18u16).map(MixerTrack::new).collect();
    // The hidden last track is routed nowhere, as FL stores it (spec: flp/events/mixer.md).
    mixer[17].routes.clear();
    Project {
        ppq: 96,
        tempo: Some(130.0),
        current_pattern: Some(1),
        channels: names.iter().enumerate().map(|(i, n)| channel(i as u16, n)).collect(),
        patterns: vec![Pattern { number: 1, ..Pattern::default() }],
        mixer,
        // FL's default template holds event 30 = 1: "Play truncated notes in clips" is on in a new project (spec:
        // flp/engine-events.md "Project", "Event 30").
        play_truncated: true,
        // Automatic crossfades (event 44) as the program setting says, whatever the template holds (spec:
        // ui/settings-choices.md "Project").
        auto_crossfades: Some(crate::prefs::check(&crate::prefs::AUTO_CROSSFADES_NEW)),
        ..Project::default()
    }
}

fn monitor_cfg_path() -> Option<PathBuf> {
    os_paths::user_file("monitor.txt")
}

/// Monitor settings from `monitor.txt` in the user folder (written by the debug panel's Save), else defaults.
fn load_monitor_cfg() -> crate::monitor::Settings {
    let mut cfg = crate::monitor::Settings::default();
    if let Some(text) = monitor_cfg_path().and_then(|p| std::fs::read_to_string(p).ok()) {
        cfg.load(&text);
    }
    cfg
}

fn cable_cfg_path() -> Option<PathBuf> {
    os_paths::user_file("cables.txt")
}

/// Cable physics factors from `cables.txt` in the user folder (written by the cable debug panel's Save), else
/// defaults.
fn load_cable_cfg() -> crate::cable::Tuning {
    let mut t = crate::cable::Tuning::default();
    if let Some(text) = cable_cfg_path().and_then(|p| std::fs::read_to_string(p).ok()) {
        t.load(&text);
    }
    t
}

/// General settings ▸ Animations from `animations.txt` in the user folder: on unless it says "Off". Tests always
/// animate.
fn load_animations() -> bool {
    if cfg!(test) {
        return true;
    }
    os_paths::user_file("animations.txt").and_then(|f| std::fs::read_to_string(f).ok()).is_none_or(|t| t.trim() != "Off")
}

pub fn save_animations(on: bool) {
    if cfg!(test) {
        return;
    }
    if let Some(dir) = os_paths::user_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("animations.txt"), if on { "Entertain me" } else { "Off" });
    }
}

fn save_cable_cfg(t: &crate::cable::Tuning) -> Result<PathBuf, String> {
    let p = cable_cfg_path().ok_or("no user folder")?;
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(&p, t.dump()).map_err(|e| e.to_string())?;
    Ok(p)
}

fn save_monitor_cfg(cfg: &crate::monitor::Settings) -> Result<PathBuf, String> {
    let p = monitor_cfg_path().ok_or("no user folder")?;
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(&p, cfg.dump()).map_err(|e| e.to_string())?;
    Ok(p)
}

/// Read a `.flstheme` (spec ui/flstheme.md) or a Lemon Studio theme text file.
pub fn theme_from_file(path: &std::path::Path) -> Result<Theme, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&bytes);
    let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("flstheme")) {
        let fl = os_theme::FlTheme::parse(&text);
        Ok(Theme::new(&name, os_theme::fl::palette(&fl), os_theme::Adjust::default()))
    } else {
        Theme::parse(&name, &text)
    }
}

/// The theme: the saved design's (spec: ui/lemon-design.md); for Classic `theme.txt` in the user folder
/// (`os_paths::user_dir`), else the last imported `current.flstheme`, else the default.
fn load_theme() -> Theme {
    let t = design_theme(saved_design(), true);
    os_theme::set_current(&t.colors);
    t
}

/// The settings folder from before the rename to Lemon Studio (`OpenStudio`, e.g. `%APPDATA%/OpenStudio`) moves to the
/// user folder (`os_paths::user_dir`, e.g. `%APPDATA%/Lemon Studio`) once, at start, so dropped browser folders, presets,
/// the design and the theme carry over (spec: ui/lemon-design.md §12).
pub fn move_old_settings() {
    if cfg!(test) {
        return;
    }
    let (Some(old), Some(new)) = (os_paths::legacy_dir(), os_paths::user_dir()) else { return };
    if old.is_dir() && !new.exists() {
        let _ = std::fs::rename(&old, &new);
    }
}

/// The design named in `design.txt` in the user folder; Lemon when there is none. Tests always use Classic.
pub fn saved_design() -> Design {
    if cfg!(test) {
        return Design::Classic;
    }
    os_paths::user_file("design.txt")
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|t| Design::parse(&t))
        .unwrap_or(Design::Lemon)
}

fn save_design(d: Design) {
    if cfg!(test) {
        return;
    }
    if let Some(dir) = os_paths::user_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("design.txt"), d.name());
    }
}

/// The theme of design `d`: Lemon's own palette, or for Classic the saved theme files (`saved` = read them).
pub fn design_theme(d: Design, saved: bool) -> Theme {
    match d {
        Design::Lemon => Theme::new("Lemon", os_theme::design::palette(Design::Lemon, None), os_theme::Adjust::default()),
        Design::Classic if saved => load_theme_inner(),
        Design::Classic => Theme::default(),
    }
}

/// The text of the `current.flstheme` the Classic design loads at start (`load_theme_inner`; `None` when `theme.txt`
/// comes first, or there is none). Tests have none.
fn saved_fl_text() -> Option<String> {
    let dir = theme_dir()?;
    if dir.join("theme.txt").exists() {
        return None;
    }
    std::fs::read(dir.join("current.flstheme")).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// The folder `current.flstheme` is kept in: the user folder; in tests this thread's folder, none unless one is set
/// (`THEME_DIR`).
fn theme_dir() -> Option<PathBuf> {
    #[cfg(test)]
    return THEME_DIR.with(|d| d.borrow().clone());
    #[cfg(not(test))]
    os_paths::user_dir()
}

#[cfg(test)]
thread_local! {
    /// Tests: the folder `theme_dir` gives.
    static THEME_DIR: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

fn load_theme_inner() -> Theme {
    if cfg!(test) {
        return Theme::default();
    }
    let Some(dir) = os_paths::user_dir() else { return Theme::default() };
    for f in ["theme.txt", "current.flstheme"] {
        let p = dir.join(f);
        if p.exists() {
            match theme_from_file(&p) {
                Ok(t) => return t,
                Err(e) => eprintln!("theme {f}: {e}; using the default"),
            }
        }
    }
    Theme::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(app: &mut App, c: &mut Canvas, damage: Option<Rect>) {
        app.paint(c, damage);
    }

    /// Paint what `App::take_damage` returned, as the shell does: each rect, or the whole frame.
    fn frame_damage(app: &mut App, c: &mut Canvas, damage: Option<Option<Vec<Rect>>>) {
        match damage {
            Some(Some(rects)) => app.paint_region(c, Some(&rects)),
            Some(None) => app.paint(c, None),
            None => {}
        }
    }

    /// Slots holding the file's parametric EQ get a Lemon Parametric EQ instance and show its name; adding the EQ
    /// stores the file's EQ (spec: plugins/lemon-parametric-eq-2.md §8).
    #[test]
    fn parametric_eq_slots_are_hosted() {
        use crate::plugins::{fx_key, EQ_FILE_NAME, LEMON_EQ};
        let mut a = App::new(None);
        a.state.project = test_project();
        a.reset_hosted();
        a.plugins.effects = crate::plugins::builtin_effects();
        let mut fl = lemon_parametric_eq_2::params::default_state();
        lemon_parametric_eq_2::params::set_field(&mut fl, 1, -600);
        a.state.project.mixer[2].slots[1] = Some(os_model::SlotPlugin { name: EQ_FILE_NAME.into(), state: fl.clone() });
        a.sync_effects();
        assert!(a.plugins.hosted.contains_key(&fx_key(2, 1)));
        assert_eq!(crate::mixer::slot_name(&a.state, 2, 1).as_deref(), Some(LEMON_EQ));
        assert_eq!(a.plugins.eq_file_state(fx_key(2, 1)), Some(fl));
        // A second pass changes nothing.
        let before = a.plugins.hosted.len();
        a.sync_effects();
        assert_eq!(a.plugins.hosted.len(), before);
        assert_eq!(crate::plugins::new_slot(LEMON_EQ).state, lemon_parametric_eq_2::params::default_state());
    }

    /// Writes the toolbar with a live-looking signal to OS_MONITOR_SHOT (a look-and-feel check, not an assertion).
    #[test]
    fn new_project_is_one_empty_lemon_sampler() {
        let p = default_project();
        assert_eq!(p.channels.len(), 1);
        let c = &p.channels[0];
        assert_eq!((c.kind, c.name.as_deref(), c.sample_path.as_deref(), c.mixer_track), (ChannelKind::Sampler, Some("Lemon Sampler"), None, 1));
        // Its starting Lemon Sampler state names no sample and carries the channel's envelope records.
        let st = crate::plugins::sampler_state(c);
        let json = String::from_utf8_lossy(&st[8..]).to_string();
        assert!(json.contains("\"envvol_on\":{\"bool\":false}") && json.contains("\"sample-path\":\"\\\"\\\"\""), "{json}");
    }

    /// The whole window at one scale, for looking at text: `OS_TEXT_SHOT=<file.png> OS_TEXT_SHOT_SCALE=1.25`.
    #[test]
    fn text_shot() {
        let Some(out) = std::env::var_os("OS_TEXT_SHOT") else { return };
        let scale: f32 = std::env::var("OS_TEXT_SHOT_SCALE").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0);
        let Ok(mut c) = Canvas::new((1600.0 * scale) as u32, (900.0 * scale) as u32, scale) else { return };
        let mut app = App::new(None);
        app.invalidate();
        frame(&mut app, &mut c, None);
        c.pixmap.save_png(std::path::Path::new(&out)).unwrap();
    }

    #[test]
    fn monitor_shot() {
        let Some(out) = std::env::var_os("OS_MONITOR_SHOT") else { return };
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        for mode in [crate::monitor::Mode::Spectrum, crate::monitor::Mode::Scope] {
            app.state.monitor = crate::monitor::Monitor::default();
            app.state.monitor.mode = mode;
            app.state.monitor.debug = true;
            let mut seed = 12345u32;
            let mut phase = 0.0f32;
            app.state.monitor.update(std::time::Instant::now(), 48000, || (0.5, 0.42), |b| {
                for v in b.iter_mut() {
                    seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                    let noise = (seed >> 8) as f32 / 8388608.0 - 1.0;
                    phase += 0.05;
                    *v = 0.25 * phase.sin() + 0.1 * (phase * 7.3).sin() + 0.05 * noise;
                }
                9 + mode as u64
            });
            app.invalidate();
            frame(&mut app, &mut c, None);
            let path = std::path::Path::new(&out).join(format!("{mode:?}.png"));
            c.pixmap.save_png(path).unwrap();
        }
    }

    /// The toolbar's analyzer columns and peak bars are solid at 150 %: no device column of the panel shows
    /// through between two 1 px columns (spec: ui/ui-scaling.md §4).
    #[test]
    fn monitor_columns_tile_at_fractional_scales() {
        use crate::monitor::{k, Mode, COLS};
        for scale in [1.0, 1.25, 1.5, 1.75] {
            let Ok(mut c) = Canvas::new((1920.0 * scale) as u32, (1080.0 * scale) as u32, scale) else { return };
            let mut app = App::new(None);
            app.state.monitor.mode = Mode::Spectrum;
            app.state.monitor.bars_db = [app.state.monitor.cfg.v[k::DB_TOP]; COLS];
            app.state.monitor.meter_db = [0.0; 2];
            app.invalidate();
            frame(&mut app, &mut c, None);
            if let Some(out) = std::env::var_os("OS_MONITOR_SHOT") {
                c.pixmap.save_png(std::path::Path::new(&out).join(format!("tiles-{scale}.png"))).unwrap();
            }
            let w = c.pixmap.width() as usize;
            let px = c.pixmap.pixels();
            let row = |y: f32, x0: f32, x1: f32| -> Vec<u8> {
                let (dy, d0, d1) = ((y * scale) as usize, (x0 * scale).round() as usize, (x1 * scale).round() as usize);
                px[dy * w + d0..dy * w + d1].iter().map(|p| p.green()).collect()
            };
            // Analyzer: 84 full-height columns from x 881; peak bars: 75 px from x 969 at y 5 and 16 (body rows).
            for (what, y, x0, x1) in [("analyzer", 30.0, 881.0, 965.0), ("peak L", 9.0, 969.0, 1043.0), ("peak R", 20.0, 969.0, 1043.0)] {
                let got = row(y, x0, x1);
                assert!(got.iter().all(|&g| g > 0x80), "{what} at {scale}: gaps in {got:?}");
            }
            // The tip column is shaded on purpose (0.58): stay clear of it above. Rows tile too.
            let (d0, d1) = ((5.0 * scale).round() as usize, (15.0 * scale).round() as usize);
            let col: Vec<u8> = (d0..d1).map(|y| px[y * w + (1000.0 * scale) as usize].green()).collect();
            assert!(col.iter().all(|&g| g > 0x80), "peak L rows at {scale}: {col:?}");
            let (d0, d1) = ((16.0 * scale).round() as usize, (26.0 * scale).round() as usize);
            let col: Vec<u8> = (d0..d1).map(|y| px[y * w + (1000.0 * scale) as usize].green()).collect();
            assert!(col.iter().all(|&g| g > 0x80), "peak R rows at {scale}: {col:?}");
        }
    }

    /// Knobs drag vertically (1/1024 per px), step with the wheel, and model-backed ones write the project.
    /// A project that was never opened from a file saves as a new FLP that loads back (spec: flp/container.md).
    #[test]
    fn new_project_saves_and_reloads() {
        let mut app = App::new(None);
        app.state.project = test_project();
        app.state.file = None;
        app.state.path = None;
        app.state.tempo = 111.0;
        app.state.project.patterns[0].notes.push(Note::step(2, 48));
        app.state.project.patterns.push(Pattern { number: 2, name: Some("Two".into()), notes: vec![Note::step(0, 0)], ..Pattern::default() });
        let path = std::env::temp_dir().join(format!("openstudio-new-{}.flp", std::process::id()));
        app.save_to(path.clone());
        assert!(app.state.message.as_deref().is_some_and(|m| m.starts_with("Saved") && !m.contains("not stored")), "{:?}", app.state.message);
        let file = os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap();
        if let Some(keep) = std::env::var_os("OS_KEEP_FLP") {
            let _ = std::fs::copy(&path, keep); // for checking the file in FL by hand
        }
        let back = os_flp::project::load(&file).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(back.tempo, Some(111.0));
        assert_eq!(back.channels.len(), 5);
        assert_eq!(back.patterns[0].notes.len(), 1);
        assert_eq!(back.patterns[1].name.as_deref(), Some("Two"));
        // Saving again goes through apply on the same file.
        app.state.project.patterns[0].notes.push(Note::step(1, 96));
        app.save_to(path.clone());
        let back = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(back.patterns[0].notes.len(), 2);
    }

    /// Settings > Project (spec: ui/settings-choices.md "Project", audio/song-structure.md): the Numerator and
    /// Denominator combos list their choices with the project's signature marked, and a pick changes the project's bar
    /// at once (events 17, 18 = beats per bar, 16 / denominator steps per beat; the pattern lengths the editors use
    /// follow; the project counts as changed); "Play truncated notes in clips" shows and switches the project's
    /// setting, on in a new project. Both survive a save and reopen, of a new project and of an opened one.
    #[test]
    fn project_settings_time_signature_and_truncated_notes() {
        use crate::playlist::Mark;
        use crate::views::settings::{SAct, DENOMINATOR, NUMERATOR, PROJECT, TRUNCATED_CHECK};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        (app.state.file, app.state.path) = (None, None);
        // A note ending at tick 300 of the current pattern: one 4/4 bar.
        app.state.project.patterns[0].notes = vec![Note { length: 276, ..Note::step(0, 24) }];
        assert_eq!(crate::piano::pattern_length(&app.state, app.state.pattern), 384);
        app.mark_saved();
        app.open_settings(PROJECT);
        frame(&mut app, &mut c, None);
        let shown = |app: &App, label: &str| {
            views::settings::page(PROJECT, &app.state).into_iter().flat_map(|g| g.rows).find_map(|r| match r {
                views::settings::Row::Combo(v, _, l) if l == label => Some(v),
                _ => None,
            })
        };
        assert_eq!((shown(&app, NUMERATOR).as_deref(), shown(&app, DENOMINATOR).as_deref()), (Some("4"), Some("4")), "4/4 when the file has none");
        let pick = |app: &mut App, c: &mut Canvas, label: &str, choice: &str, count: usize| {
            let r = app.hits.iter().rev().find(|h| matches!(&h.action, Action::Settings(SAct::Combo(l, ..)) if *l == label)).map(|h| h.rect).unwrap_or_else(|| panic!("{label} works"));
            click(app, c, r);
            let p = app.state.playlist.popup.as_ref().unwrap_or_else(|| panic!("{label} opens a menu"));
            assert_eq!(p.items.len(), count, "{label}");
            let now = shown(app, label).unwrap();
            assert!(p.items.iter().all(|it| it.mark == Mark::Radio(it.label == now)), "{label}: the current value is marked");
            let i = p.items.iter().position(|it| it.label == choice).unwrap_or_else(|| panic!("{choice} offered"));
            let r = hit_rect(app, &Action::PlPopup(i));
            click(app, c, r);
        };
        pick(&mut app, &mut c, NUMERATOR, "3", 16);
        assert_eq!(app.state.project.bar, Some((3, 4)), "three beats of four sixteenths");
        assert_eq!(app.state.message.as_deref(), Some("Time signature: 3/4"));
        assert!(app.unsaved(), "a change to the project");
        // Patterns are whole bars of the signature: 288 ticks in 3/4.
        assert_eq!(crate::piano::pattern_length(&app.state, app.state.pattern), 576);
        assert_eq!(crate::playlist::source_len(&app.state, os_model::ClipSource::Pattern(app.state.pattern)), 576);
        pick(&mut app, &mut c, DENOMINATOR, "8", 4);
        assert_eq!(app.state.project.bar, Some((3, 2)), "an eighth is two sixteenths");
        assert_eq!((shown(&app, NUMERATOR).as_deref(), shown(&app, DENOMINATOR).as_deref()), (Some("3"), Some("8")));
        assert_eq!(crate::piano::pattern_length(&app.state, app.state.pattern), 432);
        // Play truncated notes: on in a new project, as in FL's default template (spec: flp/engine-events.md "Event
        // 30"), on File > New too; a click switches it.
        assert!(default_project().play_truncated);
        let r = hit_rect(&app, &Action::Settings(SAct::Group("Advanced")));
        click(&mut app, &mut c, r);
        assert!(!app.hits.iter().any(|h| h.action == Action::Placeholder(TRUNCATED_CHECK)), "it works");
        let r = hit_rect(&app, &Action::Settings(SAct::Check(TRUNCATED_CHECK)));
        let on = |app: &App| views::settings::page(PROJECT, &app.state).into_iter().flat_map(|g| g.rows).find_map(|r| match r {
            views::settings::Row::Check(k, v) if k == TRUNCATED_CHECK => Some(v),
            _ => None,
        });
        assert_eq!((app.state.project.play_truncated, on(&app)), (true, Some(true)));
        for want in [false, true] {
            click(&mut app, &mut c, r);
            assert_eq!((app.state.project.play_truncated, on(&app)), (want, Some(want)));
        }
        // Saved and opened again.
        let path = std::env::temp_dir().join(format!("lemon-project-settings-{}.flp", std::process::id()));
        app.save_to(path.clone());
        assert!(app.state.message.as_deref().is_some_and(|m| m.starts_with("Saved")), "{:?}", app.state.message);
        assert!(!app.unsaved());
        let mut opened = App::new(None);
        opened.open(path.clone());
        assert_eq!((opened.state.project.bar, opened.state.project.play_truncated), (Some((3, 2)), true));
        // Changed in the opened project and saved over it (the file's events patched), then opened again.
        crate::playlist::run(&mut opened, crate::playlist::Cmd::Bar(7, 1));
        opened.settings_act(SAct::Check(TRUNCATED_CHECK));
        assert!(opened.unsaved());
        opened.save_to(path.clone());
        let mut again = App::new(None);
        again.open(path.clone());
        let _ = std::fs::remove_file(&path);
        assert_eq!((again.state.project.bar, again.state.project.play_truncated), (Some((7, 1)), false));
        assert_eq!(again.state.project.signature(), Some((7, 16)));
    }

    /// Theme ▸ Options (spec: ui/settings-choices.md "Light mode and clip note colours"; ui/flstheme.md §4, §5): both
    /// checks show the applied theme's values (the stock Dark theme's when none is applied); a click flips one in that
    /// theme, applies it at once in the Classic design (Light mode off: base-palette surfaces, darker steps; Lemon keeps
    /// its colours) and keeps it as `current.flstheme`, rewriting only its own key. A temp user folder, never the real
    /// one.
    #[test]
    fn theme_options_toggle_the_applied_theme() {
        use crate::views::settings::{SAct, LIGHT_MODE_CHECK, NOTE_COLORS_CHECK, THEME};
        use os_theme::fl::{palette, set_key, DARK_TEXT};
        use os_theme::{FlTheme, Palette};
        let dir = std::env::temp_dir().join(format!("lemon-theme-options-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        THEME_DIR.with(|d| *d.borrow_mut() = Some(dir.clone()));
        let saved = || std::fs::read_to_string(dir.join("current.flstheme")).unwrap_or_default();
        let shown = |app: &App| -> Vec<bool> {
            let rows = views::settings::page(THEME, &app.state).into_iter().flat_map(|g| g.rows);
            rows.filter_map(|r| match r {
                views::settings::Row::Check(k, v) if k == LIGHT_MODE_CHECK || k == NOTE_COLORS_CHECK => Some(v),
                _ => None,
            })
            .collect()
        };
        let light = |p: &Palette, name: &str| p.get(name).unwrap().hsl().2;
        let mut app = App::new(None);
        assert_eq!((app.design(), app.state.fl_text.as_deref()), (Design::Classic, None));
        assert_eq!(shown(&app), [true, false], "the stock Dark theme's values");
        // Light mode off: base-palette surfaces at once, darker step faces; saved.
        app.settings_act(SAct::Check(LIGHT_MODE_CHECK));
        let off = set_key(DARK_TEXT, "Lightmode", "0");
        assert_eq!((saved(), app.state.fl_text.as_deref()), (off.clone(), Some(off.as_str())));
        assert_eq!(shown(&app), [false, false]);
        assert_eq!(app.state.message.as_deref(), Some("Light mode: off"));
        assert_eq!(app.theme.base, palette(&FlTheme::parse(&off)));
        assert!(light(&app.theme.colors, "toolbar") > light(&Palette::default(), "toolbar"), "lighter surfaces");
        assert!(light(&app.theme.colors, "step_a") < light(&Palette::default(), "step_a"), "darker steps");
        // On again: today's look.
        app.settings_act(SAct::Check(LIGHT_MODE_CHECK));
        assert!(saved().contains("Lightmode=1"));
        assert_eq!((shown(&app), &app.theme.colors), (vec![true, false], &Palette::default()));
        // Clip note colours: stored and saved; new clips' channels follow it (`App::clip_colors`); the palette stays.
        app.settings_act(SAct::Check(NOTE_COLORS_CHECK));
        assert!(app.state.fl_theme().override_clips && saved().contains("OverrideClips=1"));
        app.tick();
        assert!(app.clip_colors().note_colors && app.state.clip_colors.note_colors);
        assert_eq!((shown(&app), &app.theme.colors), (vec![true, true], &Palette::default()));
        app.settings_act(SAct::Check(NOTE_COLORS_CHECK));
        assert!(!app.state.fl_theme().override_clips && saved().contains("OverrideClips=0"));
        // An imported theme (a BOM, CRLF lines): a click on the page rewrites only its key.
        let file = dir.join("Night.flstheme");
        let night = "\u{feff}Hue=-18\r\nSaturation=6\r\nLightness=-57\r\nContrast=40\r\nLightmode=1\r\nSelected=-4546441\r\n";
        std::fs::write(&file, night).unwrap();
        app.load_theme_file_ex(&file, false);
        assert_eq!(shown(&app), [true, false]);
        if let Ok(mut c) = Canvas::new(1920, 1080, 1.0) {
            app.open_settings(THEME);
            frame(&mut app, &mut c, None);
            let r = hit_rect(&app, &Action::Settings(SAct::Check(LIGHT_MODE_CHECK)));
            click(&mut app, &mut c, r);
        } else {
            app.settings_act(SAct::Check(LIGHT_MODE_CHECK));
        }
        let want = night.replace("Lightmode=1", "Lightmode=0");
        assert_eq!((saved(), shown(&app)), (want.clone(), vec![false, false]));
        assert_eq!(app.theme.base, palette(&FlTheme::parse(&want)));
        // The Lemon design keeps its colours; the value is still stored and saved.
        app.set_design(Design::Lemon, false);
        let lemon = app.theme.colors.clone();
        app.settings_act(SAct::Check(LIGHT_MODE_CHECK));
        assert_eq!((saved(), shown(&app), &app.theme.colors), (night.to_string(), vec![true, false], &lemon));
        // The next start reads it back.
        assert_eq!(App::new(None).state.fl_text.as_deref(), Some(night));
        THEME_DIR.with(|d| *d.borrow_mut() = None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The rows of the Project page, flattened.
    fn project_rows(app: &App) -> Vec<views::settings::Row> {
        views::settings::page(views::settings::PROJECT, &app.state).into_iter().flat_map(|g| g.rows).collect()
    }

    /// Picks `choice` from the menu of the live Project combo `label` (it must offer `count` choices).
    fn project_pick(app: &mut App, c: &mut Canvas, label: &str, choice: &str, count: usize) {
        use crate::views::settings::SAct;
        let r = app.hits.iter().rev().find(|h| matches!(&h.action, Action::Settings(SAct::Combo(l, ..)) if *l == label)).map(|h| h.rect).unwrap_or_else(|| panic!("{label} works"));
        click(app, c, r);
        let p = app.state.playlist.popup.as_ref().unwrap_or_else(|| panic!("{label} opens a menu"));
        assert_eq!(p.items.len(), count, "{label}");
        let i = p.items.iter().position(|it| it.label == choice).unwrap_or_else(|| panic!("{choice} offered"));
        let r = hit_rect(app, &Action::PlPopup(i));
        click(app, c, r);
    }

    /// Settings > Project ▸ Set as (spec: ui/settings-choices.md "Set as time division", flp/engine-events.md
    /// "Project"): time division swaps the Numerator / Denominator combos for the Beats per bar (1..32) and Steps per
    /// beat (1..16) boxes the mouse wheel changes; each mode keeps its own last bar while the program runs (4/4 at
    /// first), switching writes it into the project (event 35 with it), and a bar is never converted. Saved and opened
    /// again, the page shows the mode and the bar the file holds.
    #[test]
    fn project_settings_set_as_time_division() {
        use crate::playlist::Mark;
        use crate::views::settings::{Row, SAct, BEATS_PER_BAR, DENOMINATOR, NUMERATOR, PROJECT, SET_AS, SET_AS_DIVISION, SET_AS_SIGNATURE, STEPS_PER_BEAT};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        (app.state.file, app.state.path) = (None, None);
        app.mark_saved();
        app.open_settings(PROJECT);
        frame(&mut app, &mut c, None);
        let combo = |app: &App, label: &str| project_rows(app).into_iter().find_map(|r| match r {
            Row::Combo(v, _, l) if l == label => Some(v),
            _ => None,
        });
        let value = |app: &App, label: &str| project_rows(app).into_iter().find_map(|r| match r {
            Row::Value(v, l) if l == label => Some(v),
            _ => None,
        });
        let shown = |app: &App| (combo(app, SET_AS), combo(app, NUMERATOR), combo(app, DENOMINATOR), value(app, BEATS_PER_BAR), value(app, STEPS_PER_BEAT));
        let some = |v: &str| Some(v.to_string());
        assert_eq!(shown(&app), (some(SET_AS_SIGNATURE), some("4"), some("4"), None, None), "a file without event 35 is a time signature");
        // The menu: FL's two choices, the current one marked.
        let r = app.hits.iter().rev().find(|h| matches!(&h.action, Action::Settings(SAct::Combo(l, ..)) if *l == SET_AS)).map(|h| h.rect).expect("Set as works");
        click(&mut app, &mut c, r);
        let items: Vec<_> = app.state.playlist.popup.as_ref().expect("a menu").items.iter().map(|it| (it.label.to_string(), it.mark.clone())).collect();
        assert_eq!(items, vec![(SET_AS_DIVISION.to_string(), Mark::Radio(false)), (SET_AS_SIGNATURE.to_string(), Mark::Radio(true))]);
        app.state.playlist.popup = None;
        frame(&mut app, &mut c, None);
        // 3/8 as a time signature, then time division: 4 and 4 at first.
        project_pick(&mut app, &mut c, NUMERATOR, "3", 16);
        project_pick(&mut app, &mut c, DENOMINATOR, "8", 4);
        project_pick(&mut app, &mut c, SET_AS, SET_AS_DIVISION, 2);
        assert_eq!((app.state.project.bar, app.state.project.bar_as_signature), (Some((4, 4)), Some(false)));
        assert_eq!(shown(&app), (some(SET_AS_DIVISION), None, None, some("4"), some("4")));
        assert!(!app.hits.iter().any(|h| matches!(&h.action, Action::Settings(SAct::Combo(l, ..)) if *l == NUMERATOR)), "no combos");
        // The wheel: up adds one, down takes one, inside the box's range.
        let wheel = |app: &mut App, c: &mut Canvas, label: &'static str, dy: f32, times: usize| {
            let r = hit_rect(app, &Action::Settings(SAct::Number(label)));
            app.mouse_move(r.cx(), r.cy());
            for _ in 0..times {
                app.wheel(0.0, dy, false, false);
            }
            frame(app, c, None);
        };
        wheel(&mut app, &mut c, BEATS_PER_BAR, 1.0, 1);
        wheel(&mut app, &mut c, STEPS_PER_BEAT, -1.0, 1);
        assert_eq!(app.state.project.bar, Some((5, 3)));
        assert_eq!(app.state.message.as_deref(), Some("Time division: 5 beats of 3 steps"));
        assert_eq!((value(&app, BEATS_PER_BAR), value(&app, STEPS_PER_BEAT)), (some("5"), some("3")));
        // Five beats of three sixteenths: 360 ticks, what the editors' pattern lengths use.
        assert_eq!(app.state.project.bar_ticks(), 360);
        wheel(&mut app, &mut c, STEPS_PER_BEAT, -1.0, 5);
        wheel(&mut app, &mut c, BEATS_PER_BAR, 1.0, 40);
        assert_eq!(app.state.project.bar, Some((32, 1)), "1..32 and 1..16");
        wheel(&mut app, &mut c, STEPS_PER_BEAT, 1.0, 20);
        assert_eq!(app.state.project.bar, Some((32, 16)));
        wheel(&mut app, &mut c, BEATS_PER_BAR, -1.0, 27);
        wheel(&mut app, &mut c, STEPS_PER_BEAT, -1.0, 13);
        assert_eq!(app.state.project.bar, Some((5, 3)));
        // Each mode comes back with its own bar; none is converted.
        project_pick(&mut app, &mut c, SET_AS, SET_AS_SIGNATURE, 2);
        assert_eq!((app.state.project.bar, app.state.project.bar_as_signature), (Some((3, 2)), Some(true)));
        assert_eq!(shown(&app), (some(SET_AS_SIGNATURE), some("3"), some("8"), None, None));
        project_pick(&mut app, &mut c, SET_AS, SET_AS_DIVISION, 2);
        assert_eq!(app.state.project.bar, Some((5, 3)));
        assert!(app.unsaved());
        // Saved and opened again: time division, five beats of three steps.
        let path = std::env::temp_dir().join(format!("lemon-project-division-{}.flp", std::process::id()));
        app.save_to(path.clone());
        let mut opened = App::new(None);
        opened.open(path.clone());
        let _ = std::fs::remove_file(&path);
        assert_eq!((opened.state.project.bar, opened.state.project.bar_as_signature), (Some((5, 3)), Some(false)));
        opened.open_settings(PROJECT);
        assert_eq!(shown(&opened), (some(SET_AS_DIVISION), None, None, some("5"), some("3")));
        // A time-signature file whose steps per beat make no signature still shows (and plays) its bar.
        opened.state.project.bar_as_signature = Some(true);
        assert_eq!((combo(&opened, NUMERATOR), combo(&opened, DENOMINATOR)), (some("5"), some("5.33")));
        assert_eq!(opened.state.project.bar_ticks(), 360);
    }

    /// Settings > Project ▸ Timebase (PPQ) (spec: ui/settings-choices.md "Project"): FL's eleven choices, the
    /// project's marked; a pick rescales every tick count of the project at once (notes, pattern controls, clips,
    /// markers; automation data stays) and asks nothing. The editors follow (marker, song position, selections,
    /// clipboards, undo steps), so an undo after it restores the new timebase's ticks. Saved, the file's header holds
    /// the new PPQ; choosing the old one again gives back the same project.
    #[test]
    fn project_settings_timebase_rescales() {
        use crate::playlist::Mark;
        use crate::views::settings::{SAct, PROJECT, TIMEBASE, TIMEBASES};
        use os_model::{Arrangement, Clip, ClipSource, TimeMarker};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        (app.state.file, app.state.path) = (None, None);
        let note = Note { length: 276, ..Note::step(0, 24) };
        app.state.project.patterns[0].notes = vec![note];
        app.state.project.arrangements = vec![Arrangement {
            clips: vec![Clip::new(192, 384, 0, ClipSource::Pattern(1))],
            markers: vec![TimeMarker::new(96, "Verse")],
            ..Default::default()
        }];
        // Editor state in ticks: the piano roll's marker, clipboard and an undo step; the song position and selection.
        app.state.piano.marker = 48;
        app.state.piano.clipboard = vec![note];
        crate::piano::snapshot(&mut app.state);
        app.state.project.patterns[0].notes[0].position = 72;
        app.state.playlist.pos = 96;
        app.state.playlist.range = Some((96, 192));
        app.mark_saved();
        let before = app.state.project.clone();
        app.open_settings(PROJECT);
        frame(&mut app, &mut c, None);
        let r = app.hits.iter().rev().find(|h| matches!(&h.action, Action::Settings(SAct::Combo(l, ..)) if *l == TIMEBASE)).map(|h| h.rect).expect("Timebase works");
        click(&mut app, &mut c, r);
        let items = &app.state.playlist.popup.as_ref().expect("a menu").items;
        assert_eq!(items.iter().map(|it| it.label.to_string()).collect::<Vec<_>>(), TIMEBASES.iter().map(u16::to_string).collect::<Vec<_>>());
        assert!(items.iter().all(|it| it.mark == Mark::Radio(it.label == "96")));
        assert_eq!(TIMEBASES, &[24, 48, 72, 96, 120, 144, 168, 192, 384, 768, 960]);
        app.state.playlist.popup = None;
        frame(&mut app, &mut c, None);
        project_pick(&mut app, &mut c, TIMEBASE, "192", 11);
        let p = &app.state.project;
        assert_eq!(p.ppq, 192);
        assert_eq!((p.patterns[0].notes[0].position, p.patterns[0].notes[0].length), (144, 552));
        let a = &p.arrangements[0];
        assert_eq!((a.clips[0].position, a.clips[0].length, a.markers[0].position), (384, 768, 192));
        assert_eq!(app.state.message.as_deref(), Some("Timebase: 192 PPQ"));
        assert!(app.unsaved());
        assert_eq!((app.state.piano.marker, app.state.piano.clipboard[0].position), (96, 48));
        assert_eq!((app.state.playlist.pos, app.state.playlist.range), (192, Some((192, 384))));
        // The pattern is still one 4/4 bar long, in the new ticks.
        assert_eq!(crate::piano::pattern_length(&app.state, app.state.pattern), 768);
        // The undo step made before the change comes back in the new ticks.
        assert!(crate::piano::undo(&mut app.state));
        assert_eq!(app.state.project.patterns[0].notes[0].position, 48);
        app.state.project.patterns[0].notes[0].position = 144;
        // Back to 96: the same project as before (every tick was doubled, none rounded).
        project_pick(&mut app, &mut c, TIMEBASE, "96", 11);
        assert!(app.state.project == before && !app.unsaved());
        // Saved at 72: the header holds the new PPQ, the ticks are the new ones (rounded to the nearest).
        project_pick(&mut app, &mut c, TIMEBASE, "72", 11);
        let path = std::env::temp_dir().join(format!("lemon-project-timebase-{}.flp", std::process::id()));
        app.save_to(path.clone());
        let back = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!((back.ppq, back.patterns[0].notes[0].position, back.patterns[0].notes[0].length), (72, 54, 207));
        assert_eq!((back.arrangements[0].clips[0].position, back.arrangements[0].markers[0].position), (144, 72));
    }

    /// Settings > Project ▸ Panning law, Fast declick for cut groups, Automatic crossfades (events 23, 40, 44; spec:
    /// flp/engine-events.md "Project") show the project's values; fast declick is not played yet, so greyed.
    #[test]
    fn project_settings_show_the_unplayed_settings() {
        use crate::views::settings::Row;
        let mut app = App::new(None);
        app.state.project.auto_crossfades = None;
        let get = |app: &App| {
            let rows = project_rows(app);
            let pan = rows.iter().find_map(|r| match r {
                Row::Combo(v, _, "Panning law") => Some(v.clone()),
                _ => None,
            });
            let declick = rows.iter().find_map(|r| match r {
                Row::Check("Fast declick for cut groups", v) => Some(*v),
                _ => None,
            });
            let fades = rows.iter().find_map(|r| match r {
                Row::Check2("Automatic crossfades", v, ..) => Some(*v),
                _ => None,
            });
            (pan, declick, fades)
        };
        assert_eq!(get(&app), (Some("Circular".into()), Some(false), Some(false)), "a project without the events");
        let p = &mut app.state.project;
        (p.panning_law, p.fast_declick, p.auto_crossfades) = (Some(2), Some(true), Some(true));
        assert_eq!(get(&app), (Some("Triangular".into()), Some(true), Some(true)));
        assert!(views::settings::PROJECT_COMBOS.contains(&"Panning law"), "the panning law works");
    }

    /// Settings > Project ▸ Playlist (spec: ui/settings-choices.md "Project", the storage table): "Enable automatic
    /// crossfades in new projects" is a program setting, on by default, and a new project's Automatic crossfades (event
    /// 44) takes it, both ways; opening a file keeps the file's 44 (or its lack of one) whatever the setting.
    /// "Automatic crossfades" switches the project's 44, which counts as a change and is saved.
    #[test]
    fn project_settings_automatic_crossfades() {
        use crate::prefs::{self, AUTO_CROSSFADES_NEW};
        use crate::views::settings::{Row, SAct, CROSSFADES_CHECK, PROJECT};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        assert!(prefs::check(&AUTO_CROSSFADES_NEW), "on by default");
        assert_eq!(default_project().auto_crossfades, Some(true));
        let mut app = App::new(None);
        app.open_settings(PROJECT);
        frame(&mut app, &mut c, None);
        // The program setting: a click switches it, and File > New follows it both ways.
        let r = hit_rect(&app, &Action::Settings(SAct::Check(AUTO_CROSSFADES_NEW.label)));
        click(&mut app, &mut c, r);
        assert!(!prefs::check(&AUTO_CROSSFADES_NEW));
        assert_eq!(app.state.message.as_deref(), Some("Enable automatic crossfades in new projects: off"));
        app.new_project();
        assert_eq!(app.state.project.auto_crossfades, Some(false));
        click(&mut app, &mut c, r);
        assert!(prefs::check(&AUTO_CROSSFADES_NEW));
        app.new_project();
        assert_eq!(app.state.project.auto_crossfades, Some(true));
        assert!(!app.unsaved());
        // The project's check shows and switches event 44.
        frame(&mut app, &mut c, None);
        let shown = |app: &App| {
            project_rows(app).into_iter().find_map(|r| match r {
                Row::Check2(CROSSFADES_CHECK, v, ..) => Some(v),
                _ => None,
            })
        };
        assert_eq!(shown(&app), Some(true));
        assert!(!app.hits.iter().any(|h| h.action == Action::Placeholder(CROSSFADES_CHECK)), "it works");
        let r = hit_rect(&app, &Action::Settings(SAct::Check(CROSSFADES_CHECK)));
        click(&mut app, &mut c, r);
        assert_eq!((app.state.project.auto_crossfades, shown(&app)), (Some(false), Some(false)));
        assert_eq!(app.state.message.as_deref(), Some("Automatic crossfades: off"));
        assert!(app.unsaved());
        // Saved (44 = 0) and opened with the setting on: the file's value. Switched on there and saved over it, then
        // opened with the setting off: on.
        let path = std::env::temp_dir().join(format!("lemon-project-crossfades-{}.flp", std::process::id()));
        app.save_to(path.clone());
        let mut opened = App::new(None);
        opened.open(path.clone());
        assert_eq!(opened.state.project.auto_crossfades, Some(false));
        opened.settings_act(SAct::Check(CROSSFADES_CHECK));
        assert!(opened.unsaved());
        opened.save_to(path.clone());
        prefs::set_check(&AUTO_CROSSFADES_NEW, false);
        let mut again = App::new(None);
        again.open(path.clone());
        assert_eq!(again.state.project.auto_crossfades, Some(true));
        // A file without event 44 opens without one (shown off), whatever the setting.
        let mut p = test_project();
        p.auto_crossfades = None;
        std::fs::write(&path, os_flp::project::create(&p).unwrap().to_bytes().unwrap()).unwrap();
        for on in [false, true] {
            prefs::set_check(&AUTO_CROSSFADES_NEW, on);
            let mut app = App::new(None);
            app.open(path.clone());
            assert_eq!((app.state.project.auto_crossfades, shown(&app)), (None, Some(false)), "setting {on}");
        }
        let _ = std::fs::remove_file(&path);
    }

    /// Settings > Audio ▸ Mixer (spec: ui/settings-choices.md "Transport: Play truncated notes and Reset plugins"):
    /// "Play truncated notes on transport" is on and "Reset plugins on transport" off by default; a click switches
    /// each (kept as a program setting), and the next start of playback goes by them.
    #[test]
    fn audio_settings_transport_checks() {
        use crate::prefs::{self, PLAY_TRUNCATED_ON_TRANSPORT as TRUNCATED, RESET_PLUGINS_ON_TRANSPORT as RESET};
        use crate::transport::Starts;
        use crate::views::settings::{SAct, AUDIO};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        assert_eq!((prefs::check(&TRUNCATED), prefs::check(&RESET)), (true, false), "the defaults");
        assert_eq!(Starts::from_prefs(), Starts { truncated: true, reset: false });
        let mut app = App::new(None);
        app.open_settings(AUDIO);
        frame(&mut app, &mut c, None);
        for (check, said) in [(&TRUNCATED, "Play truncated notes on transport: off"), (&RESET, "Reset plugins on transport: on")] {
            assert!(!app.hits.iter().any(|h| h.action == Action::Placeholder(check.label)), "{} works", check.label);
            let r = hit_rect(&app, &Action::Settings(SAct::Check(check.label)));
            click(&mut app, &mut c, r);
            assert_eq!(app.state.message.as_deref(), Some(said));
        }
        assert_eq!((prefs::check(&TRUNCATED), prefs::check(&RESET)), (false, true));
        assert_eq!(Starts::from_prefs(), Starts { truncated: false, reset: true }, "the next start goes by them");
        // And back.
        for check in [&TRUNCATED, &RESET] {
            let r = hit_rect(&app, &Action::Settings(SAct::Check(check.label)));
            click(&mut app, &mut c, r);
        }
        assert_eq!(Starts::from_prefs(), Starts { truncated: true, reset: false });
    }

    // spec: ui/settings-choices.md "Miscellaneous: seven more checks, Default template and Startup project"
    /// Settings > General ▸ Miscellaneous: "Alternate meter scale" (off by default) and "Create automation clips at
    /// the playback position" (on) switch with a click. The alternate scale is live on the mixer: the "C" column's
    /// read-out at the height where the default scale has −45 dB reads −30 dB.
    #[test]
    fn misc_settings_meter_scale_and_automation_clips() {
        use crate::mixer::MHit;
        use crate::prefs::{self, ALTERNATE_METER_SCALE as ALT, AUTOMATION_AT_PLAYBACK_POSITION as AT_POS};
        use crate::views::settings::{SAct, GENERAL};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        assert_eq!((prefs::check(&ALT), prefs::check(&AT_POS)), (false, true), "the defaults");
        let mut app = App::new(None);
        app.open_settings(GENERAL);
        let above = ["Display", "Language", "Input", "Undo history", "Hints and warnings", "System", "Updates and privacy"];
        app.state.settings.folded.extend(above);
        frame(&mut app, &mut c, None);
        for (check, said) in [(&ALT, "Alternate meter scale: on"), (&AT_POS, "Create automation clips at the playback position: off")] {
            assert!(!app.hits.iter().any(|h| h.action == Action::Placeholder(check.label)), "{} works", check.label);
            let r = hit_rect(&app, &Action::Settings(SAct::Check(check.label)));
            click(&mut app, &mut c, r);
            assert_eq!(app.state.message.as_deref(), Some(said));
        }
        assert_eq!((prefs::check(&ALT), prefs::check(&AT_POS)), (true, false));
        // The mixer's level scale reads levels through the alternate scale while it is on.
        app.state.settings.open = false;
        app.state.z = vec![Win::Mixer];
        let floor = app.state.mixer.meter_cfg.v[crate::meters::k::FLOOR_DB];
        let read = |app: &mut App, c: &mut Canvas| -> f32 {
            frame(app, c, None);
            let r = hit_rect(app, &Action::Mix(MHit::Scale));
            app.mouse_move(r.cx(), r.bottom() - crate::meters::frac(-45.0, floor, false) * r.h);
            frame(app, c, None);
            let h = app.hits.iter().rev().find(|h| h.action == Action::Mix(MHit::Scale)).map(|h| h.hint.to_string()).unwrap();
            h.split_whitespace().nth(1).and_then(|v| v.trim_end_matches("dB").parse().ok()).unwrap_or_else(|| panic!("{h}"))
        };
        assert!((read(&mut app, &mut c) + 30.0).abs() < 0.05, "alternate");
        prefs::set_check(&ALT, false);
        assert!((read(&mut app, &mut c) + 45.0).abs() < 0.05, "default");
    }

    // spec: ui/settings-choices.md "Miscellaneous: seven more checks, Default template and Startup project"
    /// The mixer's "C" column: the default scale's +3, 0, 3 … 45; the alternate scale's 0, 3 … 30 with no +3, 0 at
    /// the same height, each mark where the default scale has the level the measured table gives (30 where 45 is).
    #[test]
    fn alternate_meter_scale_marks() {
        use crate::views::mixer::scale_marks;
        let floor = crate::meters::Settings::default().v[crate::meters::k::FLOOR_DB];
        let off = scale_marks(false, floor);
        assert_eq!((off.len(), off[0], off[1], off[16]), (17, ("3", 181.0), ("0", 241.0), ("45", 880.0)));
        let on = scale_marks(true, floor);
        let labels: Vec<&str> = on.iter().map(|m| m.0).collect();
        assert_eq!(labels, ["0", "3", "6", "9", "12", "15", "18", "21", "24", "27", "30"]);
        assert_eq!((on[0].1, on[10].1), (241.0, 880.0), "0 stays, 30 where 45 is");
        // "3" where the default scale has −6.6 dB: a fifth of the way from its "6" (360) to its "9" (419).
        assert!((on[1].1 - 371.8).abs() < 1e-3, "{}", on[1].1);
    }

    // spec: ui/settings-choices.md "Miscellaneous: seven more checks, Default template and Startup project"
    /// "Create automation clips at the playback position", the measured run on an empty playlist: on at bar 5 the clip
    /// covers bars 5–6 (the song is empty: the pattern's length, 1 bar); off, bars 1–6 on the next track; on at bar
    /// 2, bars 2–6 on the track after that. A muted clip does not count for the song's end; a time selection sets
    /// the clip either way.
    #[test]
    fn automation_clips_start_at_the_playback_position() {
        use crate::playlist as pl;
        use crate::prefs::{self, AUTOMATION_AT_PLAYBACK_POSITION as AT_POS};
        let mut app = App::new(None);
        let s = &mut app.state;
        pl::arr_mut(s).clips.clear();
        let current = s.pattern;
        if let Some(p) = s.project.patterns.iter_mut().find(|p| p.number == current) {
            (p.notes, p.length) = (Vec::new(), None);
        }
        let bar = s.project.bar_ticks();
        let last = |s: &State| pl::clips(s).last().map(|c| (c.position / bar, c.end() / bar, c.track)).unwrap();
        s.playlist.pos = 4 * bar;
        pl::create_automation(s, 0, "Auto", 0.5);
        assert_eq!(last(s), (4, 5, 0), "on, at bar 5");
        prefs::set_check(&AT_POS, false);
        pl::create_automation(s, 0, "Auto", 0.5);
        assert_eq!(last(s), (0, 5, 1), "off: from bar 1 to the song's end");
        prefs::set_check(&AT_POS, true);
        s.playlist.pos = bar;
        pl::create_automation(s, 0, "Auto", 0.5);
        assert_eq!(last(s), (1, 5, 2), "on, at bar 2");
        // A muted clip ending at bar 11 does not count; from bar 7 the song has ended: one pattern length.
        pl::arr_mut(s).clips.push(os_model::Clip { muted: true, ..os_model::Clip::new(0, 10 * bar, 6, os_model::ClipSource::Pattern(1)) });
        s.playlist.pos = 6 * bar;
        pl::create_automation(s, 0, "Auto", 0.5);
        assert_eq!(last(s), (6, 7, 7));
        // A time selection sets the clip, whatever the check.
        for on in [true, false] {
            prefs::set_check(&AT_POS, on);
            s.playlist.range = Some((bar, 3 * bar));
            pl::create_automation(s, 0, "Auto", 0.5);
            assert_eq!(last(s).0..last(s).1, 1..3, "time selection, check {on}");
        }
    }

    // spec: ui/settings-choices.md "Miscellaneous: naming, detached plugins and startup"
    /// Settings > General ▸ Miscellaneous: "Auto name channels" (on by default) and "Auto name effect slots" (off)
    /// switch with a click.
    #[test]
    fn misc_settings_auto_name_checks() {
        use crate::prefs::{self, AUTO_NAME_CHANNELS as CHANNELS, AUTO_NAME_EFFECT_SLOTS as SLOTS};
        use crate::views::settings::{SAct, GENERAL};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        assert_eq!((prefs::check(&CHANNELS), prefs::check(&SLOTS)), (true, false), "the defaults");
        let mut app = App::new(None);
        app.open_settings(GENERAL);
        let above = ["Display", "Language", "Input", "Undo history", "Hints and warnings", "System", "Updates and privacy"];
        app.state.settings.folded.extend(above);
        frame(&mut app, &mut c, None);
        for (check, said) in [(&CHANNELS, "Auto name channels: off"), (&SLOTS, "Auto name effect slots: on")] {
            assert!(!app.hits.iter().any(|h| h.action == Action::Placeholder(check.label)), "{} works", check.label);
            let r = hit_rect(&app, &Action::Settings(SAct::Check(check.label)));
            click(&mut app, &mut c, r);
            assert_eq!(app.state.message.as_deref(), Some(said));
        }
        assert_eq!((prefs::check(&CHANNELS), prefs::check(&SLOTS)), (false, true));
    }

    // spec: ui/settings-choices.md "Miscellaneous: naming, detached plugins and startup"
    /// "Auto name channels": a sample dropped on a sampler row, chosen in Load sample... or sent from the browser to
    /// the selected channel renames that channel after the file (without its extension) while the check is on; off,
    /// the sample loads and the channel keeps its name. A channel made new for a dropped sample takes its name either
    /// way.
    #[test]
    fn auto_name_channels_on_every_sample_path() {
        use crate::browser::BCmd;
        use crate::prefs::{self, AUTO_NAME_CHANNELS};
        let dir = std::env::temp_dir().join(format!("os_auto_name_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let wav = |n: &str| {
            let p = dir.join(format!("{n}.wav"));
            os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&p).unwrap(), 44100, &[0.0; 8]).unwrap();
            p.to_string_lossy().into_owned()
        };
        let (kick6, kick12) = (wav("Grv Kick 06"), wav("Grv Kick 12"));
        let mut app = App::new(None);
        let r = (0..app.state.project.channels.len()).find(|&r| app.state.project.channels[r].kind.is_sampler()).unwrap();
        let y = views::rack::name_rect(&app.state, r).unwrap().cy();
        // The browser lists the folder's two samples; row `r` is the selected channel.
        app.state.browser_folders.push(dir.clone());
        app.state.browser_open.insert(crate::views::browser::disk_key(&dir));
        app.state.selected_channel = r;
        let now = |app: &App| {
            let ch = &app.state.project.channels[r];
            (ch.name.clone().unwrap_or_default(), ch.sample_path.clone().unwrap_or_default())
        };
        let had = |name: &str, path: &str| (name.to_string(), path.to_string());
        app.state.project.channels[r].name = Some("808 Snare".into());
        app.sample_to_rack(kick6.clone(), y);
        assert_eq!(now(&app), had("Grv Kick 06", &kick6), "on: a drop on the row");
        crate::rack::sample_chosen(&mut app, r, kick12.clone().into());
        assert_eq!(now(&app), had("Grv Kick 12", &kick12), "on: Load sample...");
        app.browser_cmd(BCmd::Send(1));
        assert_eq!(now(&app), had("Grv Kick 06", &kick6), "on: the browser's send");
        prefs::set_check(&AUTO_NAME_CHANNELS, false);
        app.state.project.channels[r].name = Some("808 HiHat".into());
        app.browser_cmd(BCmd::Send(1));
        assert_eq!(now(&app), had("808 HiHat", &kick12), "off: the browser's send");
        app.sample_to_rack(kick6.clone(), y);
        assert_eq!(now(&app), had("808 HiHat", &kick6), "off: a drop on the row");
        crate::rack::sample_chosen(&mut app, r, kick12.clone().into());
        assert_eq!(now(&app), had("808 HiHat", &kick12), "off: Load sample...");
        // Below the rows the drop makes a new channel, named after the file.
        let n = app.state.project.channels.len();
        app.sample_to_rack(kick12.clone(), views::rack::rect(&app.state).bottom() + 40.0);
        assert_eq!(app.state.project.channels.len(), n + 1);
        assert_eq!(app.state.project.channels[app.state.selected_channel].name.as_deref(), Some("Grv Kick 12"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Settings > Project ▸ Advanced ▸ Panning law (spec: ui/settings-choices.md "Project", audio/gain-laws.md
    /// "Panning law: Triangular"): FL's two choices with the project's marked; a pick changes event 23 at once (the
    /// engine gets the project again, the project counts as changed), saved and opened again, also patched in an
    /// opened file.
    #[test]
    fn project_settings_panning_law() {
        use crate::playlist::Mark;
        use crate::views::settings::{SAct, PANNING_LAW, PROJECT};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        (app.state.file, app.state.path) = (None, None);
        app.mark_saved();
        app.open_settings(PROJECT);
        frame(&mut app, &mut c, None);
        let r = hit_rect(&app, &Action::Settings(SAct::Group("Advanced")));
        click(&mut app, &mut c, r);
        assert!(!app.hits.iter().any(|h| h.action == Action::Placeholder(PANNING_LAW)), "not greyed");
        let r = app.hits.iter().rev().find(|h| matches!(&h.action, Action::Settings(SAct::Combo(l, ..)) if *l == PANNING_LAW)).map(|h| h.rect).expect("Panning law works");
        click(&mut app, &mut c, r);
        let items: Vec<_> = app.state.playlist.popup.as_ref().expect("a menu").items.iter().map(|it| (it.label.to_string(), it.mark.clone())).collect();
        assert_eq!(items, vec![("Circular".to_string(), Mark::Radio(true)), ("Triangular".to_string(), Mark::Radio(false))], "a project without event 23");
        app.state.playlist.popup = None;
        frame(&mut app, &mut c, None);
        app.state.piano.dirty = false;
        project_pick(&mut app, &mut c, PANNING_LAW, "Triangular", 2);
        assert_eq!(app.state.project.panning_law, Some(2));
        assert_eq!(app.state.message.as_deref(), Some("Panning law: Triangular"));
        assert!(app.state.piano.dirty && app.unsaved(), "playback and the title follow");
        let shown = |app: &App| project_rows(app).into_iter().find_map(|r| match r {
            views::settings::Row::Combo(v, _, l) if l == PANNING_LAW => Some(v),
            _ => None,
        });
        assert_eq!(shown(&app).as_deref(), Some("Triangular"));
        // Saved and opened again; back to Circular in the opened project (23 = 0, patched), saved over it.
        let path = std::env::temp_dir().join(format!("lemon-project-panning-{}.flp", std::process::id()));
        app.save_to(path.clone());
        let mut opened = App::new(None);
        opened.open(path.clone());
        assert_eq!(opened.state.project.panning_law, Some(2));
        crate::playlist::run(&mut opened, crate::playlist::Cmd::PanningLaw(0));
        assert!(opened.unsaved());
        opened.save_to(path.clone());
        let back = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(back.panning_law, Some(0));
    }

    #[test]
    fn tempo_drag_locks_the_cursor() {
        let Ok(mut a) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        frame(&mut app, &mut a, None);
        let r = hit_rect(&app, &Action::Tempo);
        let t0 = app.state.tempo;
        // On the whole number.
        let x = app.state.tempo_digits.get()[0] - 4.0;
        app.mouse_move(x, r.cy());
        frame(&mut app, &mut a, None);
        app.mouse_down();
        assert!(app.cursor_locked(), "tempo drags lock the cursor");
        app.mouse_move(x, r.cy() - 10.0);
        assert_eq!(app.state.tempo, t0 + 10.0);
        app.mouse_up();
        assert!(!app.cursor_locked());
    }

    #[test]
    fn tempo_drag_steps_by_the_digit_it_starts_on() {
        let Ok(mut a) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        frame(&mut app, &mut a, None);
        let y = hit_rect(&app, &Action::Tempo).cy();
        let d = app.state.tempo_digits.get();
        assert!(d[0] < d[1] && d[1] < d[2], "{d:?}");
        let whole = d[0] - 4.0;
        for (x, step) in [(whole, 1.0), ((d[0] + d[1]) / 2.0, 0.1), ((d[1] + d[2]) / 2.0, 0.01), (d[2] + 2.0, 0.001)] {
            app.state.tempo = 130.0;
            frame(&mut app, &mut a, None);
            app.mouse_move(x, y);
            app.mouse_down();
            app.mouse_move(x, y - 3.0);
            app.mouse_up();
            let want = 130.0 + 3.0 * step;
            assert!((app.state.tempo - want).abs() < 1e-9, "from x {x}: {} vs {want}", app.state.tempo);
        }
    }

    #[test]
    fn a_held_press_slides_the_mixer_selection_across_strips() {
        use crate::mixer::MHit;
        let Ok(mut a) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.wins.iter_mut().for_each(|w| w.maxed = false);
        app.state.z = vec![Win::Mixer];
        frame(&mut app, &mut a, None);
        let (r2, r3) = (hit_rect(&app, &Action::Mix(MHit::Strip(2))), hit_rect(&app, &Action::Mix(MHit::Strip(3))));
        app.mouse_move(r2.cx(), r2.cy());
        app.mouse_down();
        assert_eq!(app.state.selected_mixer, 2);
        // Into the next strip's column, above the strips: still follows the column.
        app.mouse_move(r3.cx(), r3.y - 30.0);
        assert_eq!(app.state.selected_mixer, 3);
        app.mouse_move(r2.cx(), r2.cy());
        assert_eq!(app.state.selected_mixer, 2);
        app.mouse_up();
        app.mouse_move(r3.cx(), r3.cy());
        assert_eq!(app.state.selected_mixer, 2, "released: hovering selects nothing");
        assert_eq!(app.state.mixer.sel, Vec::<u16>::new());
    }

    #[test]
    fn mixer_eq_fader_and_knobs_edit_the_track() {
        use crate::knobs::KnobId;
        let Ok(mut a) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.wins.iter_mut().for_each(|w| w.maxed = false);
        app.state.z = vec![Win::Mixer];
        app.state.selected_mixer = 1;
        frame(&mut app, &mut a, None);
        let i = app.state.project.mixer.iter().position(|m| m.index == 1).unwrap();
        // Band 2's fader: up 40 px is a quarter of the range, +9 dB.
        let r = hit_rect(&app, &Action::Knob(KnobId::Eq(1, 1, 0)));
        app.mouse_move(r.cx(), r.cy());
        app.mouse_down();
        app.mouse_move(r.cx(), r.cy() - 40.0);
        app.mouse_up();
        assert_eq!(app.state.project.mixer[i].eq[1][0], 900);
        frame(&mut app, &mut a, None);
        let hint = app.hits.iter().find(|h| h.action == Action::Knob(KnobId::Eq(1, 1, 0))).map(|h| h.hint.to_string());
        assert_eq!(hint.as_deref(), Some("EQ band 2 (peaking) level: 9.0dB"));
        let (target, name, v) = app.automation_target(&Action::Knob(KnobId::Eq(1, 1, 0))).unwrap();
        assert_eq!((target, name.as_str()), (os_model::target::mixer(1, os_model::target::EQ_GAIN + 1), "Insert 1 - EQ band 2 (peaking) level"));
        assert!((v - 0.75).abs() < 1e-6);
        // Band 3's frequency knob, when the panel is tall enough to show it.
        if app.hits.iter().any(|h| h.action == Action::Knob(KnobId::Eq(1, 2, 1))) {
            let r = hit_rect(&app, &Action::Knob(KnobId::Eq(1, 2, 1)));
            app.mouse_move(r.cx(), r.cy());
            app.mouse_down();
            app.mouse_move(r.cx(), r.cy() - 20.0);
            app.mouse_up();
            assert!(app.state.project.mixer[i].eq[2][1] > os_model::EQ_DEFAULT[2][1]);
        }
    }

    #[test]
    fn mixer_eq_graph_drags_the_nearest_band() {
        use crate::knobs::KnobId;
        use crate::mixer::MHit;
        let Ok(mut a) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut b = Canvas::new(1920, 1080, 1.0).unwrap();
        let mut app = App::new(None);
        app.state.wins.iter_mut().for_each(|w| w.maxed = false);
        app.state.z = vec![Win::Mixer];
        app.state.selected_mixer = 1;
        frame(&mut app, &mut a, None);
        let _ = app.take_damage();
        let i = app.state.project.mixer.iter().position(|m| m.index == 1).unwrap();
        let g = hit_rect(&app, &Action::Mix(MHit::EqGraph));
        // Each change repaints the graph with the faders and knobs, and that matches a full repaint.
        let check = |app: &mut App, a: &mut Canvas, b: &mut Canvas, what: &str| {
            let d = app.take_damage();
            let Some(Some(rects)) = &d else { panic!("{what}: repaints regions, not the frame") };
            assert!(rects.iter().any(|r| r.contains(g.cx(), g.cy())), "{what}: the graph is repainted");
            frame_damage(app, a, d);
            frame(app, b, None);
            assert_same(a, b, what);
        };
        // A knob: the graph follows.
        let k = hit_rect(&app, &Action::Knob(KnobId::Eq(1, 1, 0)));
        app.mouse_move(k.cx(), k.cy());
        frame(&mut app, &mut a, None);
        let _ = app.take_damage();
        app.mouse_down();
        frame(&mut app, &mut a, None);
        let _ = app.take_damage();
        app.mouse_move(k.cx(), k.cy() - 30.0);
        check(&mut app, &mut a, &mut b, "fader drag");
        app.mouse_up();
        app.state.project.mixer[i].eq = os_model::EQ_DEFAULT;
        frame(&mut app, &mut a, None);
        let _ = app.take_damage();
        // The graph's hint reads the frequency under the cursor and the band a press would take: here band 2 (1500 Hz).
        let x = g.x + 1.0 + (g.w - 2.0) * 0.55;
        app.mouse_move(x, g.cy());
        assert!(app.hint.ends_with("Hz  Tweak band 2"), "{}", app.hint);
        // Pressing there and dragging up and left: band 2 follows the cursor, the others stay.
        app.mouse_down();
        frame(&mut app, &mut a, None);
        let _ = app.take_damage();
        app.mouse_move(x - 10.0, g.cy() - (g.h / 2.0 - 4.0) / 2.0);
        check(&mut app, &mut a, &mut b, "graph drag");
        app.mouse_up();
        let eq = app.state.project.mixer[i].eq;
        assert_eq!(eq[1][0], 900, "half way up is +9 dB");
        let want = (crate::mixer::eq_graph_pos(g, x - 10.0) * 65536.0).round() as i32;
        assert_eq!(eq[1][1], want);
        assert_eq!((eq[0], eq[2]), (os_model::EQ_DEFAULT[0], os_model::EQ_DEFAULT[2]));
        // At the left edge the low shelf is the nearest.
        app.mouse_move(g.x + 3.0, g.cy());
        assert!(app.hint.ends_with("Tweak band 1"), "{}", app.hint);
    }

    #[test]
    fn knobs_drag_and_wheel() {
        use crate::knobs::KnobId;
        let Ok(mut a) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut b = Canvas::new(1920, 1080, 1.0).unwrap();
        let mut app = App::new(None);
        app.state.wins.iter_mut().for_each(|w| w.maxed = false);
        app.state.z = vec![Win::Mixer, Win::ChannelRack];
        frame(&mut app, &mut a, None);
        let _ = app.take_damage();
        for id in [KnobId::ChanVol(0), KnobId::MixPan(1), KnobId::ChanPan(0)] {
            let r = hit_rect(&app, &Action::Knob(id));
            let v0 = app.state.knob(id);
            app.mouse_move(r.cx(), r.cy());
            frame(&mut app, &mut a, None);
            app.mouse_down();
            assert!(app.cursor_locked(), "knob drags lock the cursor");
            app.mouse_move(r.cx(), r.cy() - 100.0);
            let want = (v0 + 100.0 * id.drag_rate()).min(1.0);
            assert!((app.state.knob(id) - want).abs() < 1e-4, "{id:?}: {} vs {want}", app.state.knob(id));
            frame(&mut app, &mut a, None);
            app.mouse_up();
            // Damage repaint of the drag equals a full repaint.
            let d = app.take_damage();
            frame_damage(&mut app, &mut a, d);
            frame(&mut app, &mut b, None);
            assert_same(&a, &b, "after knob drag");
            let before = app.state.knob(id);
            app.mouse_move(r.cx(), r.cy());
            frame(&mut app, &mut a, None);
            app.mouse_down_middle();
            assert_eq!(app.state.knob(id), id.default_value(), "middle click resets {id:?}");
            app.state.set_knob(id, before);
            frame(&mut app, &mut a, None);
            app.wheel(0.0, 1.0, false, false);
            assert!((app.state.knob(id) - (before + id.notch()).min(1.0)).abs() < 1e-4, "wheel {id:?}");
        }
        assert!(app.state.project.mixer.iter().find(|m| m.index == 1).unwrap().pan.unwrap() > 0.0, "mixer pan written to the model");
    }

    /// Holding the mouse on a toolbar window button lights it orange; releasing restores it.
    #[test]
    fn pressed_button_look() {
        let Ok(mut a) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        frame(&mut app, &mut a, None);
        // A window button whose press changes nothing in the toolbar (the plugin picker would cover the screen).
        let act = Action::ToggleWin(Win::Browser);
        let r = hit_rect(&app, &act);
        let px = |c: &Canvas| {
            let mut out = vec![0u32; 1920 * 1080];
            c.copy_to_xrgb(&mut out);
            out[(r.cy() as usize + 9) * 1920 + r.cx() as usize]
        };
        let idle = px(&a);
        app.mouse_move(r.cx(), r.cy());
        frame(&mut app, &mut a, None);
        app.mouse_down();
        frame(&mut app, &mut a, None);
        let held = px(&a);
        let (red, blue) = ((held >> 16) & 0xff, held & 0xff);
        assert!(red > 0xe0 && blue < 0xa8, "orange while held: {held:06x} (idle {idle:06x})");
        app.mouse_up();
        app.mouse_move(900.0, 900.0);
        frame(&mut app, &mut a, None);
        assert_eq!(px(&a), idle, "back to idle after release and moving away");
    }

    /// A damage-region repaint must give the pixels of a full repaint (anti-aliased edges cut by
    /// the damage rect may round by a level or two in tiny-skia's masked pipeline).
    #[test]
    fn partial_repaint_matches_full() {
        let Ok(mut a) = Canvas::new(1920, 1080, 1.0) else { return }; // no system font: skip
        let mut b = Canvas::new(1920, 1080, 1.0).unwrap();
        let mut app = App::new(None);
        app.state.z = vec![Win::Mixer, Win::ChannelRack];
        frame(&mut app, &mut a, None);

        // Move the rack and fade a fader, repainting only the damaged regions.
        let old = views::rack::bounds(&app.state);
        app.state.rack_pos = (700.0, 400.0);
        let new = views::rack::bounds(&app.state);
        if let Some(t) = app.state.project.mixer.iter_mut().find(|t| t.index == 3) {
            t.volume = Some(0.35);
        }
        let col = app.hits.iter().find(|h| h.action == Action::Fader(3)).map(|h| h.rect).unwrap();
        frame(&mut app, &mut a, Some(old.union(&new).union(&col.outset(10.0))));

        frame(&mut app, &mut b, None);
        let diffs: Vec<usize> = a.pixmap.data().iter().zip(b.pixmap.data()).enumerate().filter(|(_, (x, y))| x.abs_diff(**y) > 3).map(|(i, _)| i / 4).collect();
        let w = 1920;
        let bbox = diffs.iter().fold((usize::MAX, usize::MAX, 0, 0), |m, &i| (m.0.min(i % w), m.1.min(i / w), m.2.max(i % w), m.3.max(i / w)));
        if !diffs.is_empty() {
            let _ = a.pixmap.save_png(std::env::temp_dir().join("os_partial.png"));
            let _ = b.pixmap.save_png(std::env::temp_dir().join("os_full.png"));
        }
        assert!(diffs.is_empty(), "{} differing pixels in x {}..{} y {}..{}", diffs.len(), bbox.0, bbox.2, bbox.1, bbox.3);
    }

    /// While playing, only what playback moves is repainted (clock, playheads, the rack's playing step, sounding
    /// lights, meters); each such frame must equal a full repaint of the same state.
    #[test]
    fn playback_repaint_matches_full() {
        let Ok(mut a) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut b = Canvas::new(1920, 1080, 1.0).unwrap();
        let mut app = App::new(None);
        app.state.project = test_project();
        app.state.project.patterns[0].notes = (0..16).step_by(2).map(|k| Note::step(0, k * 24)).collect();
        app.state.project.patterns[0].notes.extend((0..16).step_by(4).map(|k| Note::step(1, k * 24)));
        crate::playlist::place(&mut app.state, 0, 0);
        crate::playlist::place(&mut app.state, 1, 384);
        let play = |tick: f64, song: bool, sounding: Vec<(u16, u16)>| Shown { playing: true, paused: false, tick, song, sounding };
        // Floating windows side by side, so every playback region is on screen.
        for (w, r) in [(Win::Playlist, Rect::new(220.0, 90.0, 1000.0, 480.0)), (Win::PianoRoll, Rect::new(220.0, 90.0, 1000.0, 480.0)), (Win::Mixer, Rect::new(230.0, 590.0, 1500.0, 480.0))] {
            let ws = &mut app.state.wins[w.slot().unwrap()];
            ws.maxed = false;
            ws.rect = r;
        }
        for (z, song) in [(vec![Win::Playlist, Win::Mixer, Win::ChannelRack], true), (vec![Win::Mixer, Win::PianoRoll, Win::ChannelRack], false)] {
            app.state.z = z;
            app.state.rack_pos = (1240.0, 100.0);
            frame(&mut app, &mut a, None);
            let grid = if song { app.pl_grid } else { app.piano_grid }.expect("grid shown");
            // Starting playback repaints everything; then each tick only damages.
            app.show_playback(play(0.0, song, Vec::new()));
            let d = app.take_damage();
            assert_eq!(d, Some(None), "starting repaints the whole frame");
            frame_damage(&mut app, &mut a, d);
            for (i, tick) in [5.0, 23.0, 25.0, 50.0, 120.0, 383.0, 390.0, 700.0, 10.0].into_iter().enumerate() {
                let sounding = if i % 3 == 0 { vec![(0, 60)] } else if i % 3 == 1 { vec![(0, 60), (1, 60)] } else { Vec::new() };
                app.show_playback(play(tick, song, sounding));
                // Meters move too.
                app.state.mixer.meters.resize(18, Default::default());
                app.state.mixer.meters[1].db = [-6.0 - i as f32, -12.0];
                app.state.mixer.meters[2].db = [-3.0 * i as f32, -1.0];
                app.damage_live(Live::Meter);
                let d = app.take_damage();
                let Some(Some(rects)) = &d else { panic!("a tick repaints regions, not the frame") };
                assert!(rects.iter().any(|r| !r.intersect(&grid).is_empty()), "the playhead is repainted");
                frame_damage(&mut app, &mut a, d);
                frame(&mut app, &mut b, None);
                assert_same(&a, &b, &format!("song {song}, tick {tick}"));
            }
            app.show_playback(Shown::default());
            let d = app.take_damage();
            frame_damage(&mut app, &mut a, d);
        }
    }

    /// Wheel scrolls and zooms, scroll-bar drags and clip drags repaint only their window, and match a full repaint.
    #[test]
    fn scroll_repaint_matches_full() {
        for design in Design::ALL {
            scroll_repaint_in(design);
        }
    }

    fn scroll_repaint_in(design: Design) {
        let Ok(mut a) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut b = Canvas::new(1920, 1080, 1.0).unwrap();
        let mut app = App::new(None);
        // Rounded (Lemon) windows: their corners are not part of what scrolling keeps (spec: ui/lemon-design.md §3).
        app.theme = design_theme(design, false);
        app.state.project = test_project();
        app.state.project.patterns[0].notes = (0..16).step_by(2).map(|k| Note::step(0, k * 24)).collect();
        for (track, tick) in [(0, 0), (1, 384), (2, 768), (0, 1536)] {
            crate::playlist::place(&mut app.state, track, tick);
        }
        for (w, r) in [(Win::Playlist, Rect::new(220.0, 90.0, 1000.0, 480.0)), (Win::PianoRoll, Rect::new(300.0, 300.0, 900.0, 450.0))] {
            let ws = &mut app.state.wins[w.slot().unwrap()];
            ws.maxed = false;
            ws.rect = r;
        }
        app.state.z = vec![Win::PianoRoll, Win::Playlist, Win::ChannelRack];
        app.state.rack_pos = (1240.0, 100.0);
        frame(&mut app, &mut a, None);
        let _ = app.take_damage();
        let pl = app.state.win_area(Win::Playlist);
        let check = |app: &mut App, a: &mut Canvas, b: &mut Canvas, what: &str| {
            let d = app.take_damage();
            let Some(Some(rects)) = &d else { panic!("{what}: repaints regions, not the frame") };
            // The hint panel follows the hovered thing; everything else stays inside the playlist.
            let inside = rects.iter().filter(|r| r.intersect(&HINT_RECT) != **r);
            let bound = inside.fold(Rect::default(), |u, r| if u.is_empty() { *r } else { u.union(r) });
            assert!(bound.intersect(&pl) == bound, "{what}: only the playlist is repainted ({bound:?})");
            frame_damage(app, a, d);
            frame(app, b, None);
            assert_same(a, b, what);
        };
        let g = app.pl_grid.unwrap();
        // Hovering an automation clip shows its points: onto it and off it again repaints just that clip.
        crate::playlist::create_automation(&mut app.state, 0, "Auto", 0.5);
        app.state.playlist.sel.clear();
        frame(&mut app, &mut a, None);
        let _ = app.take_damage();
        let pg = crate::playlist::PGeom::new(&app.state, g);
        let auto = *crate::playlist::clips(&app.state).last().unwrap();
        let ar = pg.clip_rect(&auto);
        app.mouse_move(ar.x + 20.0, ar.bottom() - 10.0);
        assert!(app.state.playlist.hover_clip.is_some(), "the automation clip is hovered");
        check(&mut app, &mut a, &mut b, "automation clip hovered");
        app.mouse_move(ar.x + 20.0, ar.bottom() + 60.0);
        check(&mut app, &mut a, &mut b, "automation clip left");
        // A plain click on a tool button: the press repaints everything, the release just the button.
        let tool = hit_rect(&app, &Action::PlTool(crate::playlist::PlTool::Paint));
        app.mouse_move(tool.cx(), tool.cy());
        app.mouse_down();
        let d = app.take_damage();
        frame_damage(&mut app, &mut a, d);
        app.mouse_up();
        check(&mut app, &mut a, &mut b, "tool button released");
        assert_eq!(app.state.playlist.tool, crate::playlist::PlTool::Paint);
        app.state.playlist.tool = crate::playlist::PlTool::Draw;
        app.invalidate();
        frame_damage(&mut app, &mut a, Some(None));
        let _ = app.take_damage();
        // Wheel over the grid: scroll down, then right, each easing over 10 ms frames (General ▸ Display ▸ Smoothing at
        // its default); Ctrl zooms at once.
        app.mouse_move(g.x + 300.0, g.y + 100.0);
        frame(&mut app, &mut a, None);
        let _ = app.take_damage();
        let eases = |app: &mut App, a: &mut Canvas, b: &mut Canvas, what: &str| {
            assert!(app.smooth.running(), "{what}: eases");
            for i in 0..100 {
                app.ease_step(10.0);
                if app.has_damage() {
                    check(app, a, b, &format!("{what}, frame {i}"));
                }
                if !app.smooth.running() {
                    return;
                }
            }
            panic!("{what}: the ease ends");
        };
        app.wheel(0.0, -2.0, false, false);
        eases(&mut app, &mut a, &mut b, "wheel scroll");
        app.wheel(-3.0, 0.0, false, false);
        eases(&mut app, &mut a, &mut b, "wheel sideways");
        app.wheel(0.0, 1.0, false, true);
        assert!(!app.smooth.running(), "a zoom doesn't ease");
        check(&mut app, &mut a, &mut b, "ctrl+wheel zoom");
        // Drag the horizontal scroll bar.
        let bar = app.bar(Scroll::PlaylistX).unwrap();
        app.mouse_move(bar.track.x + 3.0, bar.track.cy());
        app.mouse_down();
        let _ = app.take_damage();
        frame(&mut app, &mut a, None);
        app.mouse_move(bar.track.x + 60.0, bar.track.cy());
        check(&mut app, &mut a, &mut b, "scroll bar drag");
        app.mouse_up();
        frame(&mut app, &mut a, None);
        let _ = app.take_damage();
        // Drag the first clip on screen right and down.
        app.state.playlist_scroll = (0.0, 0.0);
        frame(&mut app, &mut a, None);
        let _ = app.take_damage();
        let pg = crate::playlist::PGeom::new(&app.state, app.pl_grid.unwrap());
        let c0 = pg.clip_rect(&crate::playlist::clips(&app.state)[0]);
        app.mouse_move(c0.x + 10.0, c0.y + 25.0);
        app.mouse_down();
        frame(&mut app, &mut a, None);
        let _ = app.take_damage();
        for (dx, dy) in [(40.0, 0.0), (90.0, 48.0), (150.0, 96.0)] {
            app.mouse_move(c0.x + 10.0 + dx, c0.y + 25.0 + dy);
            if let Damage::Rects(rs) = &app.damage {
                let area: f32 = rs.iter().map(|r| r.w * r.h).sum();
                assert!(area < pl.w * pl.h / 4.0, "a clip drag repaints the clip, not the window ({area})");
            }
            check(&mut app, &mut a, &mut b, "clip drag");
        }
        let moved = crate::playlist::clips(&app.state)[0];
        assert!(moved.position > 0 && moved.track == 2, "the clip was dragged: {} on track {}", moved.position, moved.track);
        app.mouse_up();
        frame(&mut app, &mut a, None);
        let _ = app.take_damage();

        // An audio clip (spec: ui/playlist.md §9a): hovering it shows its handles, a handle under the mouse lights,
        // dragging the fade-in triangle repaints that clip and the hint bar only.
        let wav = std::env::temp_dir().join(format!("os-repaint-clip-{}.wav", std::process::id()));
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &[0.4; 96000]).unwrap();
        crate::playlist::add_audio_clip(&mut app.state, &wav.to_string_lossy(), 4, 384).expect("audio clip");
        app.state.playlist.sel.clear();
        app.state.playlist_bar_w = 240.0;
        frame(&mut app, &mut a, None);
        let _ = app.take_damage();
        let pg = crate::playlist::PGeom::new(&app.state, app.pl_grid.unwrap());
        let ci = crate::playlist::clips(&app.state).len() - 1;
        let body = pg.body(&crate::playlist::clips(&app.state)[ci]);
        app.mouse_move(body.cx() + 30.0, body.cy());
        assert_eq!(app.state.playlist.hover_clip, Some(ci), "the audio clip is hovered");
        check(&mut app, &mut a, &mut b, "audio clip hovered");
        app.mouse_move(body.x + 2.0, body.y + 2.0);
        assert_eq!(app.state.playlist.hover_handle, Some(crate::playlist::Handle::FadeIn));
        check(&mut app, &mut a, &mut b, "fade handle hovered");
        app.mouse_down();
        frame(&mut app, &mut a, None);
        let _ = app.take_damage();
        for dx in [30.0, 70.0] {
            app.mouse_move(body.x + 2.0 + dx, body.y + 2.0);
            if let Damage::Rects(rs) = &app.damage {
                let area: f32 = rs.iter().map(|r| r.w * r.h).sum();
                assert!(area < pl.w * pl.h / 4.0, "a fade drag repaints the clip, not the window ({area})");
            }
            check(&mut app, &mut a, &mut b, "fade drag");
        }
        assert!(crate::playlist::clips(&app.state)[ci].fade_in().is_some(), "the fade was set");
        app.mouse_up();
        frame(&mut app, &mut a, None);
        let _ = app.take_damage();
        // Stretch mode: resizing the audio clip repaints the playlist (every clip of its channel follows).
        crate::playlist::run(&mut app, crate::playlist::Cmd::StretchMode);
        app.invalidate();
        frame_damage(&mut app, &mut a, Some(None));
        let _ = app.take_damage();
        let r = pg.clip_rect(&crate::playlist::clips(&app.state)[ci]);
        app.mouse_move(r.right() - 1.0, r.bottom() - 6.0);
        app.mouse_down();
        frame(&mut app, &mut a, None);
        let _ = app.take_damage();
        app.mouse_move(r.right() + 60.0, r.bottom() - 6.0);
        check(&mut app, &mut a, &mut b, "stretch resize");
        app.mouse_up();
        let _ = std::fs::remove_file(&wav);
    }

    /// Scrolling the playlist moves the pixels drawn for it and paints only what scrolled in (`App::playlist_scrolled`).
    /// Every step equals a full repaint, at 100 % and 150 %: with notes, an audio clip (fades, a waveform changing
    /// every column), an automation clip, markers, a time selection, a track group, a zoom off the pixel grid, song
    /// playback, windows over the playlist, drags, a popup (no move: the window repaints) and random steps.
    #[test]
    fn playlist_scroll_moves_pixels() {
        let wav = std::env::temp_dir().join(format!("os-scroll-move-{}.wav", std::process::id()));
        let audio: Vec<f32> = (0..480_000)
            .map(|i| {
                let t = (i / 2) as f32 / 48000.0;
                (t * 2.0).sin() * (t * 900.0 * (1.0 + t)).sin()
            })
            .collect();
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &audio).unwrap();
        for scale in [1.0, 1.5] {
            for design in Design::ALL {
                scroll_moves_at(&wav, scale, design);
            }
        }
        let _ = std::fs::remove_file(&wav);
    }

    fn scroll_moves_at(wav: &std::path::Path, scale: f32, design: Design) {
        use crate::playlist as pl;
        let (w, h) = ((1920.0 * scale).round() as u32, (1080.0 * scale).round() as u32);
        let Ok(mut a) = Canvas::new(w, h, scale) else { return };
        let mut b = Canvas::new(w, h, scale).unwrap();
        let mut app = App::new(None);
        app.theme = design_theme(design, false);
        app.state.project = test_project();
        app.state.project.patterns[0].notes = (0..32).map(|i| Note { length: 30, key: 60 + (i % 7) as u16, ..Note::step(0, i * 24) }).collect();
        let bar = 4 * u32::from(app.state.project.ppq);
        for t in 0..14u16 {
            for k in 0..8u32 {
                if (u32::from(t) + k) % 3 != 0 {
                    pl::place(&mut app.state, t, (k * 3 + u32::from(t % 2)) * bar);
                }
            }
        }
        let ci = pl::add_audio_clip(&mut app.state, &wav.to_string_lossy(), 4, bar / 2).expect("audio clip");
        let ch = pl::create_automation(&mut app.state, 0, "Auto", 0.5);
        pl::add_point(&mut app.state, ch, 3.0, 0.9);
        pl::add_point(&mut app.state, ch, 7.5, 0.1);
        {
            let arr = pl::arr_mut(&mut app.state);
            arr.clips[ci].set_fade_in(Some((300.0, 0.3)));
            arr.clips[ci].set_fade_out(Some((900.0, -0.4)));
            arr.markers.push(os_model::TimeMarker::new(2 * bar, "Drop"));
            arr.markers.push(os_model::TimeMarker::new(9 * bar + 7, "Break"));
            arr.tracks[2].color = Some(os_model::Rgb::new(0x90, 0x58, 0x87));
            arr.tracks[3].grouped = true;
            arr.tracks[4].grouped = true;
        }
        app.state.playlist.sel = vec![3, 7];
        app.state.playlist.range = Some((bar + 5, 5 * bar));
        // A zoom off the drawing grid, a start between device pixels (on the 1/256 px lattice: whole steps from it stay
        // exact in f32; a step that is not exact repaints, see "off-grid step"), a track height as a user leaves them.
        app.state.playlist_bar_w = 37.37;
        app.state.playlist_scroll = (123.457_031_25, 0.0);
        app.state.playlist.row_h = 46.0;
        let ws = &mut app.state.wins[Win::Playlist.slot().unwrap()];
        ws.maxed = false;
        ws.rect = Rect::new(150.3, 90.0, 1600.0, 900.0);
        app.state.z = vec![Win::Playlist];
        frame(&mut app, &mut a, None);
        let _ = app.take_damage();
        let pl = app.state.win_area(Win::Playlist);
        let g = app.pl_grid.unwrap();
        let what_at = |what: &str| format!("{what} at {scale}");
        // Paint what is pending; `moves`: pixels were moved (else the window repainted); `small`: under a quarter of the
        // playlist painted. Then the result must equal a full repaint.
        let check = |app: &mut App, a: &mut Canvas, b: &mut Canvas, what: &str, moves: Option<bool>, small: bool| -> Vec<Rect> {
            let what = what_at(what);
            let d = app.take_damage();
            let Some(Some(_)) = &d else { panic!("{what}: repaints regions, not the frame") };
            frame_damage(app, a, d);
            let region = a.region();
            let area: f32 = region.iter().map(|r| r.w * r.h).sum();
            let moved = app.take_moved();
            if let Some(m) = moves {
                assert_eq!(!moved.is_empty(), m, "{what}: the drawn pixels moved");
            }
            if small {
                assert!(area < pl.w * pl.h / 4.0, "{what}: painted {area} of {} px²: {region:?}", pl.w * pl.h);
            }
            frame(app, b, None);
            assert_same(a, b, &what);
            region
        };
        let full = |app: &mut App, a: &mut Canvas| {
            app.invalidate();
            let d = app.take_damage();
            frame_damage(app, a, d);
        };
        let over_grid = (g.x + 300.0, g.y + 200.0);
        app.mouse_move(over_grid.0, over_grid.1);
        full(&mut app, &mut a);
        // Wheel steps at once (General ▸ Display ▸ Smoothing 0; eased steps below): down, up, right, left, Shift =
        // sideways, several before one paint.
        crate::prefs::set_slider(&crate::prefs::SMOOTHING, 0);
        app.wheel(0.0, -1.0, false, false);
        check(&mut app, &mut a, &mut b, "wheel down", Some(true), true);
        app.wheel(0.0, -2.0, false, false);
        check(&mut app, &mut a, &mut b, "wheel down two", Some(true), true);
        app.wheel(0.0, 1.0, false, false);
        check(&mut app, &mut a, &mut b, "wheel up", Some(true), true);
        app.wheel(-1.0, 0.0, false, false);
        check(&mut app, &mut a, &mut b, "wheel right", Some(true), true);
        app.wheel(1.0, 0.0, false, false);
        check(&mut app, &mut a, &mut b, "wheel left", Some(true), true);
        app.wheel(0.0, -1.0, true, false);
        check(&mut app, &mut a, &mut b, "shift+wheel", Some(true), true);
        app.wheel(-1.0, 0.0, false, false);
        app.wheel(0.0, -1.0, false, false);
        app.wheel(-1.0, 0.0, false, false);
        check(&mut app, &mut a, &mut b, "three steps", Some(true), true);
        // Eased steps (Smoothing 128, 10 ms frames): every frame of the ease moves the drawn pixels, in whole steps of
        // the drawing grid at 150 %; a notch sideways during the ease too.
        crate::prefs::set_slider(&crate::prefs::SMOOTHING, 128);
        app.wheel(0.0, -2.0, false, false);
        let mut frames = 0;
        while app.smooth.running() {
            if frames == 6 {
                app.wheel(-1.0, 0.0, false, false);
            }
            app.ease_step(10.0);
            frames += 1;
            if app.has_damage() {
                check(&mut app, &mut a, &mut b, &format!("eased frame {frames}"), Some(true), true);
            }
            assert!(frames < 100, "{}: the ease ends", what_at("eased"));
        }
        assert!(frames > 10, "{}: the steps eased over {frames} frames", what_at("eased"));
        crate::prefs::set_slider(&crate::prefs::SMOOTHING, 0);
        // Over the track headers: rows move, the time line stays; the hovered group arrow (lit) moves off the mouse.
        app.state.playlist_scroll.1 = 0.0;
        full(&mut app, &mut a);
        let arrow = hit_rect(&app, &Action::PlGroupArrow(2));
        app.mouse_move(arrow.cx(), arrow.cy());
        full(&mut app, &mut a);
        app.wheel(0.0, -1.0, false, false);
        check(&mut app, &mut a, &mut b, "wheel over the headers", Some(true), true);
        app.mouse_move(over_grid.0, over_grid.1);
        // At the start: a part of a step moves; a step off the drawing grid repaints the window.
        app.state.playlist_scroll.0 = 30.0;
        full(&mut app, &mut a);
        app.wheel(1.0, 0.0, false, false);
        assert_eq!(app.state.playlist_scroll.0, 0.0);
        check(&mut app, &mut a, &mut b, "wheel to the start", Some(true), true);
        app.state.playlist_scroll.0 = 25.3;
        full(&mut app, &mut a);
        app.wheel(1.0, 0.0, false, false);
        check(&mut app, &mut a, &mut b, "off-grid step", Some(false), false);
        app.state.playlist_scroll.0 = 123.457_031_25;
        full(&mut app, &mut a);
        // Song playback: the playhead moves between steps.
        let pg = pl::PGeom::new(&app.state, g);
        let tick = f64::from(pg.tick_at(g.x + 400.0));
        let shown = |tick: f64| Shown { playing: true, paused: false, tick, song: true, sounding: Vec::new() };
        app.show_playback(shown(tick));
        full(&mut app, &mut a);
        // The playhead's damage before a step was taken where it was drawn; its pixels move with the step.
        app.show_playback(shown(tick + 400.0));
        app.wheel(-1.0, 0.0, false, false);
        app.show_playback(shown(tick + 900.0));
        check(&mut app, &mut a, &mut b, "scroll while playing", Some(true), true);
        app.show_playback(shown(tick + 1300.0));
        app.wheel(0.0, -1.0, false, false);
        app.show_playback(shown(tick + 1700.0));
        check(&mut app, &mut a, &mut b, "scroll down while playing", Some(true), true);
        app.show_playback(Shown::default());
        full(&mut app, &mut a);
        // Dragging the overview's thumb, then grabbing the grid with the middle button.
        let bar_x = app.bar(Scroll::PlaylistX).unwrap();
        let pos = app.scroll_get(Scroll::PlaylistX);
        let thumb_x = bar_x.track.x + pos / bar_x.max * (bar_x.track.w - bar_x.thumb) + bar_x.thumb / 2.0;
        app.mouse_move(thumb_x, bar_x.track.cy());
        app.mouse_down();
        full(&mut app, &mut a);
        // The shell paints only when `has_damage` says so: every move of these drags must ask for a frame.
        for dx in [37.0, 81.0, 60.0] {
            app.mouse_move(thumb_x + dx, bar_x.track.cy());
            assert!(app.has_damage(), "scroll bar drag at {scale}: the move asks for a frame");
            check(&mut app, &mut a, &mut b, "scroll bar drag", Some(true), true);
        }
        app.mouse_up();
        full(&mut app, &mut a);
        app.mouse_move(over_grid.0, over_grid.1);
        app.mouse_down_middle();
        full(&mut app, &mut a);
        for (dx, dy) in [(23.0, 31.0), (-50.0, 7.0), (-90.0, -60.0)] {
            app.mouse_move(over_grid.0 + dx, over_grid.1 + dy);
            assert!(app.has_damage(), "middle-drag grab at {scale}: the move asks for a frame");
            check(&mut app, &mut a, &mut b, "middle-drag grab", Some(true), true);
        }
        app.mouse_up();
        full(&mut app, &mut a);
        // An open popup: the window repaints.
        app.mouse_move(over_grid.0, over_grid.1);
        app.state.playlist.popup = Some(pl::options_menu(&app.state, g.x + 900.0, g.y + 100.0));
        full(&mut app, &mut a);
        app.wheel(0.0, -1.0, false, false);
        check(&mut app, &mut a, &mut b, "popup open", Some(false), false);
        app.state.playlist.popup = None;
        full(&mut app, &mut a);

        // Windows over the playlist: a piano roll and the channel rack.
        let ws = &mut app.state.wins[Win::PianoRoll.slot().unwrap()];
        ws.maxed = false;
        ws.rect = Rect::new(g.x - 60.0, g.bottom() - 220.0, 700.0, 400.0);
        app.state.z = vec![Win::Playlist, Win::PianoRoll, Win::ChannelRack];
        app.state.rack_pos = (g.right() - 380.0, g.y + 120.0);
        full(&mut app, &mut a);
        assert_eq!(app.top_hit(over_grid.0, over_grid.1).map(|h| h.action.clone()), Some(Action::PlaylistArea), "the mouse is over the grid");
        // They did not change: the insides of their frames keep their pixels (their edges and what they uncover paint).
        let insides = [views::rack::rect(&app.state).inset(3.0), app.state.win_rect(Win::PianoRoll).inset(3.0)];
        for (what, dx, dy) in [("covered: down", 0.0, -1.0), ("covered: right", -2.0, 0.0), ("covered: up", 0.0, 2.0), ("covered: left", 1.0, 0.0)] {
            app.wheel(dx, dy, false, false);
            let region = check(&mut app, &mut a, &mut b, what, Some(true), false);
            for inside in insides {
                let painted: f32 = region.iter().map(|r| r.intersect(&inside)).map(|r| r.w * r.h).sum();
                assert!(painted < inside.w * inside.h / 10.0, "{what} at {scale}: painted {painted} px² inside {inside:?}: {region:?}");
            }
        }
        // (The press raises the playlist; put the windows back over it.)
        let covered = vec![Win::Playlist, Win::PianoRoll, Win::ChannelRack];
        app.mouse_down_middle();
        app.state.z = covered.clone();
        full(&mut app, &mut a);
        app.mouse_move(over_grid.0 + 41.0, over_grid.1 - 17.0);
        check(&mut app, &mut a, &mut b, "covered: grab", Some(true), false);
        app.mouse_up();
        app.state.z = covered.clone();
        full(&mut app, &mut a);
        // A window over the playlist moving after a step, before the paint: its old pixels moved with the step.
        assert!(app.playlist_covers().len() > 1, "windows over the playlist");
        app.wheel(0.0, -1.0, false, false);
        let old = app.win_bounds(Win::ChannelRack);
        app.state.rack_pos.1 += 90.0;
        app.damage_rect(old);
        app.damage_rect(app.win_bounds(Win::ChannelRack));
        check(&mut app, &mut a, &mut b, "covered: a window moved after a step", Some(true), false);

        // Random steps: one to three wheel steps a frame, the mouse wandering over the grid (hover changes), playback.
        let mut seed = 0x2545_f491_u32;
        let mut rnd = |n: u32| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed % n
        };
        for i in 0..40 {
            if rnd(4) == 0 {
                let (x, y) = (g.x + 20.0 + rnd(700) as f32 * 0.75, g.y + 10.0 + rnd(500) as f32 * 0.5);
                if app.top_hit(x, y).is_some_and(|h| h.action == Action::PlaylistArea) {
                    app.mouse_move(x, y);
                }
            }
            if rnd(5) == 0 {
                app.show_playback(shown(tick + f64::from(i) * 13.0));
                if rnd(2) == 0 {
                    full(&mut app, &mut a);
                }
            }
            for _ in 0..=rnd(3) {
                let step = [-2.0, -1.0, 1.0, 2.0][rnd(4) as usize];
                if rnd(2) == 0 {
                    app.wheel(step, 0.0, false, false);
                } else {
                    app.wheel(0.0, step, false, false);
                }
            }
            if app.has_damage() {
                check(&mut app, &mut a, &mut b, &format!("random step {i}"), None, false);
            }
        }
    }

    fn assert_same(a: &Canvas, b: &Canvas, what: &str) {
        let width = a.pixmap.width() as usize;
        let diffs = a.pixmap.data().iter().zip(b.pixmap.data()).enumerate().filter(|(_, (x, y))| x.abs_diff(**y) > 3).map(|(i, _)| i / 4);
        let (n, bbox) = diffs.fold((0, (usize::MAX, usize::MAX, 0, 0)), |(n, m), i| (n + 1, (m.0.min(i % width), m.1.min(i / width), m.2.max(i % width), m.3.max(i / width))));
        if n > 0 {
            let _ = a.pixmap.save_png(std::env::temp_dir().join("os_partial.png"));
            let _ = b.pixmap.save_png(std::env::temp_dir().join("os_full.png"));
        }
        assert_eq!(n, 0, "{what}: differing pixels in x {}..{} y {}..{}", bbox.0, bbox.2, bbox.1, bbox.3);
    }

    /// Scroll bars: left-drag scrolls one axis, right-drag both (piano roll), middle-drag grabs.
    #[test]
    fn scrollbar_drags() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.z = vec![Win::PianoRoll];
        frame(&mut app, &mut c, None);
        let v = app.bar(Scroll::PianoY).unwrap();
        let h = app.bar(Scroll::PianoX).unwrap();
        // Left-drag the vertical thumb down.
        let y0 = app.scroll_get(Scroll::PianoY);
        let thumb_y = v.track.y + y0 / v.max * (v.track.h - v.thumb) + v.thumb / 2.0;
        app.mouse_move(v.track.cx(), thumb_y);
        app.mouse_down();
        app.mouse_move(v.track.cx(), thumb_y + 50.0);
        app.mouse_up();
        assert!(app.scroll_get(Scroll::PianoY) > y0 + 100.0, "scrolled down");
        assert_eq!(app.scroll_get(Scroll::PianoX), 0.0);
        frame(&mut app, &mut c, None);
        // Right-drag the horizontal bar: sideways scrolls, upward zooms in; vertical scroll stays.
        let (x1, y1, z1) = (app.scroll_get(Scroll::PianoX), app.scroll_get(Scroll::PianoY), app.state.piano.px_per_beat);
        app.mouse_move(h.track.x + 5.0, h.track.cy());
        app.mouse_down_right();
        app.mouse_move(h.track.x + 105.0, h.track.cy());
        app.mouse_up();
        assert!(app.scroll_get(Scroll::PianoX) > x1, "sideways scroll");
        assert_eq!(app.state.piano.px_per_beat, z1);
        frame(&mut app, &mut c, None);
        app.mouse_move(h.track.x + 5.0, h.track.cy());
        app.mouse_down_right();
        app.mouse_move(h.track.x + 5.0, h.track.cy() - 60.0);
        app.mouse_up();
        assert!(app.state.piano.px_per_beat > z1 * 1.5, "zoomed in: {}", app.state.piano.px_per_beat);
        assert_eq!(app.scroll_get(Scroll::PianoY), y1);
        frame(&mut app, &mut c, None);
        // Middle-drag the grid: the content follows the mouse.
        let (x2, y2) = (app.scroll_get(Scroll::PianoX), app.scroll_get(Scroll::PianoY));
        let g = app.piano_grid.unwrap();
        app.mouse_move(g.cx(), g.cy());
        app.mouse_down_middle();
        app.mouse_move(g.cx() + 30.0, g.cy() + 36.0);
        app.mouse_up();
        assert!((app.scroll_get(Scroll::PianoX) - (x2 - 30.0)).abs() < 0.5);
        assert!((app.scroll_get(Scroll::PianoY) - (y2 - 36.0)).abs() < 0.5);
        frame(&mut app, &mut c, None);
        // Right-drag the vertical bar (spec: ui/scroll-bars.md): up/down scrolls with the cursor held, sideways
        // does not scroll.
        let (x3, y3, row) = (app.scroll_get(Scroll::PianoX), app.scroll_get(Scroll::PianoY), app.state.piano.row_h);
        let thumb = |app: &App| {
            let v = app.bar(Scroll::PianoY).unwrap();
            (v.track.cx(), v.track.y + app.scroll_get(Scroll::PianoY) / v.max * (v.track.h - v.thumb) + v.thumb / 2.0)
        };
        let (tx, ty) = thumb(&app);
        app.mouse_move(tx, ty);
        app.mouse_down_right();
        assert!(app.cursor_locked());
        app.mouse_move(tx, ty + 20.0);
        app.mouse_up();
        assert!(app.scroll_get(Scroll::PianoY) > y3 + 20.0, "down scrolls");
        assert_eq!((app.scroll_get(Scroll::PianoX), app.state.piano.row_h), (x3, row));
        frame(&mut app, &mut c, None);
        // Sideways resizes the key rows around the view centre: right = taller, left = shorter.
        let g = app.piano_geom().unwrap().grid;
        let centre_key = |app: &App| (g.cy() - g.y + app.state.piano.scroll_y) / app.state.piano.row_h;
        let k0 = centre_key(&app);
        let (tx, ty) = thumb(&app);
        app.mouse_move(tx, ty);
        app.mouse_down_right();
        for i in 1..=6 {
            app.mouse_move(tx + 4.0 * i as f32, ty);
        }
        app.mouse_up();
        assert_eq!(app.state.piano.row_h, (row * 1.15f32.powi(2)).round(), "24 px right = two steps taller");
        assert!((centre_key(&app) - k0).abs() < 0.5, "centre key kept: {} vs {k0}", centre_key(&app));
        frame(&mut app, &mut c, None);
        let (tx, ty) = thumb(&app);
        app.mouse_move(tx, ty);
        app.mouse_down_right();
        app.mouse_move(tx - 400.0, ty);
        app.mouse_up();
        assert_eq!(app.state.piano.row_h, piano::MIN_ROW);
        // Playlist: the same drag sets the track height in 2 px steps between the limits.
        app.state.z = vec![Win::Playlist];
        frame(&mut app, &mut c, None);
        let v = app.bar(Scroll::PlaylistY).unwrap();
        let (tx, ty) = (v.track.cx(), v.track.y + v.thumb / 2.0);
        let h0 = app.state.playlist.row_h;
        app.mouse_move(tx, ty);
        app.mouse_down_right();
        app.mouse_move(tx + 12.0, ty);
        app.mouse_up();
        let h1 = app.state.playlist.row_h;
        assert!(h1 > h0 && h1 % 2.0 == 0.0, "taller in 2 px steps: {h0} -> {h1}");
        for (dx, want) in [(1000.0, crate::playlist::ROW_MAX), (-1000.0, crate::playlist::ROW_MIN)] {
            frame(&mut app, &mut c, None);
            app.mouse_move(tx, ty);
            app.mouse_down_right();
            app.mouse_move(tx + dx, ty);
            app.mouse_up();
            assert_eq!(app.state.playlist.row_h, want);
        }
        // Bars without rows (channel rack, picker) only scroll.
        assert_eq!(app.rows_get(Scroll::RackY), None);
    }

    fn hit_rect(app: &App, a: &Action) -> Rect {
        app.hits.iter().rev().find(|h| &h.action == a).map(|h| h.rect).unwrap_or_else(|| panic!("no hit for {a:?}"))
    }

    fn click(app: &mut App, c: &mut Canvas, r: Rect) {
        app.mouse_move(r.cx(), r.cy());
        app.mouse_down();
        app.mouse_up();
        frame(app, c, None);
    }

    /// Settings window (spec: ui/settings.md): OPTIONS entries open their tab, tabs switch, checks and
    /// group folds toggle, the General page scrolls, Esc closes; every tab paints.
    #[test]
    fn settings_window() {
        use crate::views::menus::{item_at, OPTIONS_MENU};
        use crate::views::settings::{SAct, FILE, GENERAL, MIDI, NATIVE_HOST_CHECK, PLUGINS, PROJECT, TABS, THEME};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        frame(&mut app, &mut c, None);
        let audio = (0..40).find(|&i| item_at(OPTIONS_MENU, i).is_some_and(|it| it.label == "Audio settings")).unwrap();
        app.menu_item(OPTIONS_MENU, audio);
        assert!(app.state.settings.open);
        assert_eq!(app.state.settings.tab, 1);
        for i in 0..TABS.len() {
            frame(&mut app, &mut c, None);
            let r = hit_rect(&app, &Action::Settings(SAct::Tab(i)));
            click(&mut app, &mut c, r);
            assert_eq!(app.state.settings.tab, i);
        }
        let r = hit_rect(&app, &Action::Settings(SAct::Tab(MIDI)));
        click(&mut app, &mut c, r);
        // Controls that do not work yet are greyed out: no toggling, the click only says so.
        assert!(!app.hits.iter().any(|h| h.action == Action::Settings(SAct::Check("Send master sync"))));
        let r = hit_rect(&app, &Action::Placeholder("Send master sync"));
        click(&mut app, &mut c, r);
        assert_eq!(app.state.message.as_deref(), Some("Send master sync: not available yet"));
        let r = hit_rect(&app, &Action::Settings(SAct::Tab(PROJECT)));
        click(&mut app, &mut c, r);
        assert!(app.hits.iter().any(|h| matches!(h.action, Action::Settings(SAct::Combo("Ask for data folder", ..)))), "Ask for data folder works");
        assert!(app.hits.iter().any(|h| matches!(h.action, Action::Settings(SAct::Combo("Timebase (PPQ)", ..)))), "the PPQ combo works");
        let adv = Action::Settings(SAct::Group("Advanced"));
        let r = hit_rect(&app, &adv);
        click(&mut app, &mut c, r);
        assert!(app.hits.iter().any(|h| h.action == Action::Placeholder("Fast declick for cut groups")), "Advanced unfolded");
        // The native host toggle shows only with `OPENSTUDIO_NATIVE_SETTINGS` (`SettingsUi::native`); the
        // real-native-plugins toggle only while the native host is on too (off in tests).
        let shown = |app: &App, k| app.hits.iter().any(|h| h.action == Action::Settings(SAct::Check(k)));
        let r = hit_rect(&app, &Action::Settings(SAct::Tab(PLUGINS)));
        for native in [false, true] {
            app.state.settings.native = native;
            click(&mut app, &mut c, r);
            assert_eq!(shown(&app, NATIVE_HOST_CHECK), native, "native host toggle shown: {native}");
            if !os_native::enabled() {
                assert!(!shown(&app, crate::views::settings::NATIVE_FIRST_CHECK), "hidden");
            }
        }
        // The working controls keep their actions.
        for (tab, live) in [
            (FILE, SAct::Button("Manage plugins")),
            (PLUGINS, SAct::Button("Add folder...")),
            (PLUGINS, SAct::Button("Find new plugins")),
            (PLUGINS, SAct::Check(crate::views::settings::COMBINE_CHECK)),
            (THEME, SAct::Button("Load theme...")),
            (THEME, SAct::Button("Reset")),
            (THEME, SAct::Preset(0)),
        ] {
            let r = hit_rect(&app, &Action::Settings(SAct::Tab(tab)));
            click(&mut app, &mut c, r);
            assert!(app.hits.iter().any(|h| h.action == Action::Settings(live.clone())), "{live:?} works");
        }
        assert!(app.hits.iter().any(|h| h.action == Action::Placeholder("Save preset...")));
        let r = hit_rect(&app, &Action::Settings(SAct::Tab(GENERAL)));
        click(&mut app, &mut c, r);
        app.wheel(0.0, -3.0, false, false);
        assert!(app.state.settings.scroll > 0.0);
        frame(&mut app, &mut c, None);
        app.key(&winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape), false, false);
        assert!(!app.state.settings.open);
        app.key(&winit::keyboard::Key::Named(winit::keyboard::NamedKey::F10), false, false);
        assert!(app.state.settings.open && app.state.settings.tab == MIDI);
    }

    /// A project in the layout used before version 24.2 is refused with a dialog that says what to do.
    #[test]
    fn older_projects_get_a_dialog() {
        let mut file = os_flp::project::create(&test_project()).expect("create");
        // Channel blocks without their end event: the older layout.
        for c in &mut file.chunks {
            if let os_flp::ChunkBody::Events(v) = &mut c.body {
                v.retain(|e| e.id != 51);
            }
        }
        let path = std::env::temp_dir().join(format!("lemon-legacy-{}.flp", std::process::id()));
        std::fs::write(&path, file.to_bytes().expect("bytes")).expect("write");
        let mut app = App::new(None);
        let before = app.state.project.channels.len();
        app.open(path.clone());
        let _ = std::fs::remove_file(&path);
        assert_eq!(app.state.dialogs.len(), 1);
        let (title, text) = app.top_message().expect("a message box");
        assert_eq!(title, "Project not supported yet");
        assert!(text.contains("before version 24.2") && text.contains("lemon-legacy-"), "{text}");
        assert!(app.state.message.as_deref().is_some_and(|m| m.starts_with("Could not open")));
        assert_eq!(app.state.project.channels.len(), before, "the open project stays");
        // Its Ok closes it (Enter presses the focused button).
        app.key(&winit::keyboard::Key::Named(winit::keyboard::NamedKey::Enter), false, false);
        assert!(!app.modal_open());
    }

    /// Press `k` (a character or a named key) with the modifiers given.
    fn press(app: &mut App, k: winit::keyboard::Key, ctrl: bool, alt: bool) {
        app.key(&k, ctrl, alt);
    }

    fn typed(app: &mut App, s: &str) {
        press(app, winit::keyboard::Key::Character(s.into()), false, false);
    }

    fn named(app: &mut App, k: winit::keyboard::NamedKey) {
        press(app, winit::keyboard::Key::Named(k), false, false);
    }

    fn top_file(app: &App) -> &crate::filedlg::FileDlg {
        match &app.state.dialogs.last().expect("a dialog").body {
            crate::dialog::Body::File(f) => f,
            crate::dialog::Body::Msg(m) => panic!("a message box is on top: {}", m.plain()),
            crate::dialog::Body::Shortcuts(_) => panic!("Manage shortcuts is on top"),
            crate::dialog::Body::TimeSig(_) => panic!("the time signature dialog is on top"),
        }
    }

    /// Unsaved changes are asked about in a message box drawn in the window (spec: ui/dialogs.md §1f, §3): Cancel
    /// stays, No goes on, Yes saves first (a project without a file through the save dialog) and then goes on; the
    /// typing keyboard does not play while it is open; both designs paint it and its buttons take clicks and keys.
    #[test]
    fn unsaved_changes_are_asked_in_the_window() {
        use crate::dialog::{fake, DHit, Next};
        use crate::views::menus::FILE_NEW;
        use winit::keyboard::NamedKey;
        let Ok(mut c) = Canvas::new(1280, 800, 1.0) else { return };
        fake::live(true);
        let dir = std::env::temp_dir().join(format!("os_dialog_unsaved_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for design in [Design::Classic, Design::Lemon] {
            let mut app = App::new(None);
            app.set_design(design, false);
            app.state.tempo = 99.0;
            assert!(app.unsaved());
            app.menu_item(0, FILE_NEW);
            let (title, text) = app.top_message().expect("asked");
            assert_eq!(title, "Confirm");
            assert_eq!(text, "Save the changes to the new project before starting a new project?");
            assert!(!app.typing_key(KeyCode::KeyQ, true, false, false), "the typing keyboard waits");
            frame(&mut app, &mut c, None);
            // Cancel (a click) stays.
            let r = hit_rect(&app, &Action::Dlg(DHit::Button(2)));
            click(&mut app, &mut c, r);
            assert!(!app.modal_open() && app.unsaved() && app.state.tempo == 99.0, "Cancel keeps the project");
            // Clicks behind a dialog do nothing.
            app.menu_item(0, FILE_NEW);
            frame(&mut app, &mut c, None);
            app.mouse_move(5.0, 5.0);
            app.mouse_down();
            app.mouse_up();
            assert!(app.modal_open());
            // N (the accelerator) = No: the new project replaces the changes.
            typed(&mut app, "n");
            assert!(!app.modal_open() && !app.unsaved() && app.state.tempo == 130.0, "No goes on");
            // Yes on a project without a file: the save dialog, then the window may close.
            app.state.tempo = 98.0;
            app.ask_discard("closing", Next::Close);
            frame(&mut app, &mut c, None);
            named(&mut app, NamedKey::Tab);
            named(&mut app, NamedKey::Tab);
            named(&mut app, NamedKey::Tab);
            named(&mut app, NamedKey::Enter);
            assert_eq!(app.state.dialogs.len(), 1, "Tab went round to Yes");
            let f = top_file(&app);
            assert_eq!((f.mode, f.name.text.as_str(), f.name.all), (crate::filedlg::Mode::Save, "Untitled.flp", true));
            frame(&mut app, &mut c, None);
            let target = dir.join(format!("Tune {design:?}"));
            typed(&mut app, &target.display().to_string());
            assert!(!app.want_exit);
            named(&mut app, NamedKey::Enter);
            let saved = dir.join(format!("Tune {design:?}.flp"));
            assert!(saved.exists() && !app.unsaved(), "saved with the type's extension: {:?}", app.state.message);
            assert!(app.want_exit && !app.modal_open(), "then the window closes");
            // Save as onto that file: the replace question starts on No; Yes replaces.
            app.want_exit = false;
            app.state.tempo = 97.0;
            app.save(true);
            frame(&mut app, &mut c, None);
            assert_eq!(top_file(&app).dir, dir, "the dialog starts in the project's folder");
            named(&mut app, NamedKey::Enter);
            assert_eq!(app.state.dialogs.len(), 2);
            assert_eq!(app.top_message().unwrap().1, format!("Tune {design:?}.flp already exists.\nDo you want to replace it?"));
            frame(&mut app, &mut c, None);
            named(&mut app, NamedKey::Enter);
            assert_eq!(app.state.dialogs.len(), 1, "No goes back to the dialog");
            assert!(app.unsaved());
            named(&mut app, NamedKey::Enter);
            typed(&mut app, "y");
            assert!(!app.modal_open() && !app.unsaved(), "replaced");
        }
        fake::live(false);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The file dialog (spec: ui/dialogs.md §4): rows, double clicks, Up / Back, the type combo, the wheel, the caption
    /// drag, typing a name, Esc; in both designs.
    #[test]
    fn file_dialog_browses_and_answers() {
        use crate::dialog::{fake, DHit};
        use crate::filedlg::{Mode, ROW_H};
        use crate::views::menus::FILE_OPEN;
        use winit::keyboard::NamedKey;
        let Ok(mut c) = Canvas::new(1280, 800, 1.0) else { return };
        fake::live(true);
        let dir = std::env::temp_dir().join(format!("os_dialog_browse_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Beats")).unwrap();
        std::fs::write(dir.join("Beats").join("Loop.flp"), b"not a project").unwrap();
        for i in 0..40 {
            std::fs::write(dir.join(format!("Song {i:02}.flp")), b"x").unwrap();
        }
        std::fs::write(dir.join("Zebra.txt"), b"x").unwrap();
        for design in [Design::Classic, Design::Lemon] {
            let mut app = App::new(None);
            app.set_design(design, false);
            app.state.path = Some(dir.join("Song 00.flp"));
            app.menu_item(0, FILE_OPEN);
            frame(&mut app, &mut c, None);
            let f = top_file(&app);
            assert_eq!((f.mode, &f.dir, f.entries.len()), (Mode::Open, &dir, 41), "FLP files and the folder");
            assert_eq!(f.entries[0].name, "Beats");
            // The wheel scrolls the list; the caption drags the dialog.
            app.mouse_move(640.0, 400.0);
            app.wheel(0.0, -2.0, false, false);
            assert_eq!(top_file(&app).scroll, 6.0 * ROW_H);
            app.wheel(0.0, 10.0, false, false);
            assert_eq!(top_file(&app).scroll, 0.0);
            frame(&mut app, &mut c, None);
            let cap = hit_rect(&app, &Action::Dlg(DHit::Caption));
            let at = app.state.dialogs[0].rect.get();
            app.mouse_move(cap.x + 40.0, cap.cy());
            app.mouse_down();
            app.mouse_move(cap.x + 10.0, cap.cy() + 30.0);
            app.mouse_up();
            assert_eq!(app.state.dialogs[0].at, Some((at.x - 30.0, at.y + 30.0)));
            frame(&mut app, &mut c, None);
            // A double click opens the folder; Alt+Up goes back up with it selected.
            let row = hit_rect(&app, &Action::Dlg(DHit::Row(0)));
            click(&mut app, &mut c, row);
            click(&mut app, &mut c, row);
            assert_eq!(top_file(&app).dir, dir.join("Beats"));
            press(&mut app, winit::keyboard::Key::Named(NamedKey::ArrowUp), false, true);
            assert_eq!(top_file(&app).dir, dir);
            assert_eq!(top_file(&app).sel, Some(0));
            // All files lists the text file too.
            frame(&mut app, &mut c, None);
            let r = hit_rect(&app, &Action::Dlg(DHit::Filter));
            click(&mut app, &mut c, r);
            assert!(top_file(&app).filter_open);
            let r = hit_rect(&app, &Action::Dlg(DHit::FilterItem(1)));
            click(&mut app, &mut c, r);
            assert_eq!(top_file(&app).entries.len(), 42);
            // Typing a name that is not there is refused in the status; Backspace edits.
            typed(&mut app, "Nothing.flp");
            named(&mut app, NamedKey::Enter);
            assert!(app.modal_open());
            assert_eq!(top_file(&app).status_text(), "Nothing.flp was not found");
            frame(&mut app, &mut c, None);
            for _ in 0.."Nothing.flp".len() {
                named(&mut app, NamedKey::Backspace);
            }
            assert_eq!(top_file(&app).name.text, "");
            // Down picks a row; the main button opens the file (not a real project: the hint bar says so).
            named(&mut app, NamedKey::ArrowDown);
            assert_eq!(top_file(&app).name.text, "Song 00.flp");
            let r = hit_rect(&app, &Action::Dlg(DHit::Accept));
            click(&mut app, &mut c, r);
            assert!(!app.modal_open());
            assert!(app.state.message.as_deref().is_some_and(|m| m.starts_with("Could not open")), "{:?}", app.state.message);
            // Esc cancels.
            app.menu_item(0, FILE_OPEN);
            named(&mut app, NamedKey::Escape);
            assert!(!app.modal_open());
        }
        fake::live(false);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Look check for the dialogs in both designs: `OS_SHOT_DIR=dir cargo test -p os-app dialog_shots -- --ignored`.
    #[test]
    #[ignore]
    fn dialog_shots() {
        use crate::dialog::{fake, spans, Answer, Kind, Msg, Then};
        let Some(out) = std::env::var_os("OS_SHOT_DIR") else { return };
        let out = std::path::PathBuf::from(out);
        let Ok(mut c) = Canvas::new(1280, 800, 1.0) else { return };
        fake::live(true);
        let dir = std::env::temp_dir().join(format!("os_dialog_shots_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Drums")).unwrap();
        std::fs::create_dir_all(dir.join("Vocals")).unwrap();
        for (n, len) in [("Club mix.flp", 48_213), ("Demo.flp", 9_120), ("Night drive.flp", 131_072), ("Sketch 3.flp", 2_048)] {
            std::fs::write(dir.join(n), vec![0u8; len]).unwrap();
        }
        for design in [Design::Classic, Design::Lemon] {
            let name = format!("{design:?}").to_lowercase();
            let mut app = App::new(None);
            app.set_design(design, false);
            app.state.path = Some(dir.join("Demo.flp"));
            for (kind, title, text, buttons, tag) in [
                (Kind::Question, "Confirm", spans(&["Save the changes to ", "Demo.flp", " before closing?"]), &[Answer::Yes, Answer::No, Answer::Cancel][..], "question"),
                (Kind::Warning, "Confirm save as", spans(&["", "Demo.flp", " already exists.\nDo you want to replace it?"]), &[Answer::Yes, Answer::No][..], "warning"),
                (Kind::Error, "Error", spans(&["The file doesn't exist or has been moved."]), &[Answer::Ok][..], "error"),
                (Kind::Info, "Project not supported yet", spans(&["Lemon Studio cannot open ", "Old.flp", " yet.\n\nIt was saved by version 12.3. Projects saved before version 24.2 use an older channel layout that Lemon Studio does not read yet."]), &[Answer::Ok][..], "info"),
            ] {
                let mut m = Msg::new(kind, title, text, buttons, Then::Nothing);
                // As in the app, the replace question starts on No.
                m.focus = usize::from(tag == "warning");
                app.show_msg(m);
                frame(&mut app, &mut c, None);
                app.mouse_move(-1.0, -1.0);
                frame(&mut app, &mut c, None);
                c.pixmap.save_png(out.join(format!("dialog_{name}_{tag}.png"))).unwrap();
                app.state.dialogs.clear();
                app.modal_changed();
            }
            app.save(true);
            frame(&mut app, &mut c, None);
            let row = hit_rect(&app, &Action::Dlg(crate::dialog::DHit::Row(3)));
            app.mouse_move(row.cx(), row.cy());
            frame(&mut app, &mut c, None);
            c.pixmap.save_png(out.join(format!("dialog_{name}_save.png"))).unwrap();
            app.state.dialogs.clear();
            app.file_dialog(crate::filedlg::FileDlg::new(crate::filedlg::Mode::Folder, "Add a plugin folder", "plugins", Then::PluginFolder).start(Some(&dir)));
            frame(&mut app, &mut c, None);
            c.pixmap.save_png(out.join(format!("dialog_{name}_folder.png"))).unwrap();
            app.state.dialogs.clear();
            app.modal_changed();
        }
        fake::live(false);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The CPU panel opens Settings > Audio; its combos open menus over the window and a choice becomes the audio
    /// settings (spec: ui/main-window.md "CPU panel", ui/settings.md "Audio").
    #[test]
    fn cpu_panel_and_audio_settings() {
        use crate::audio_cfg::AudioCfg;
        use crate::views::settings::{SAct, AUDIO, BUFFER_LENGTH, OUTPUT_DEVICE, SAMPLE_RATE};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        frame(&mut app, &mut c, None);
        let r = hit_rect(&app, &Action::CpuPanel);
        assert!(app.state.cpu.update(std::time::Instant::now(), Some(0.25), 3, 0));
        app.mouse_move(r.cx(), r.cy());
        assert!(app.hint.contains("Audio load 25%") && app.hint.contains("3 voices"), "{}", app.hint);
        // One click does nothing; the menu's Audio settings opens the Audio tab.
        click(&mut app, &mut c, r);
        assert!(!app.state.settings.open);
        app.mouse_move(r.cx(), r.cy());
        app.mouse_down_right();
        frame(&mut app, &mut c, None);
        let i = app.state.playlist.popup.as_ref().expect("the panel's menu").items.iter().position(|it| it.label == "Audio settings").unwrap();
        let item = hit_rect(&app, &Action::PlPopup(i));
        click(&mut app, &mut c, item);
        assert!(app.state.settings.open && app.state.settings.tab == AUDIO);
        assert!(!app.state.settings.status[0].is_empty());
        let combo = |app: &App, label: &str| {
            app.hits.iter().rev().find(|h| matches!(&h.action, Action::Settings(SAct::Combo(l, ..)) if *l == label)).map(|h| h.rect).unwrap_or_else(|| panic!("{label} works"))
        };
        for (label, first) in [(OUTPUT_DEVICE, "Default device"), (SAMPLE_RATE, "Device rate"), (BUFFER_LENGTH, "Device default")] {
            let r = combo(&app, label);
            click(&mut app, &mut c, r);
            let p = app.state.playlist.popup.as_ref().unwrap_or_else(|| panic!("{label} opens a menu"));
            assert_eq!(p.items[0].label, first);
            assert!((p.y - r.bottom()).abs() < 2.0, "under its box");
            // The menu is painted over the settings window: its rows take the clicks.
            assert!(app.top_hit(r.x + 10.0, r.bottom() + 10.0).is_some_and(|h| matches!(h.action, Action::PlPopup(_))));
            app.state.playlist.popup = None;
            frame(&mut app, &mut c, None);
        }
        let r = combo(&app, BUFFER_LENGTH);
        click(&mut app, &mut c, r);
        let i = app.state.playlist.popup.as_ref().unwrap().items.iter().position(|it| it.label.starts_with("1024 frames")).expect("1024 offered");
        let r = hit_rect(&app, &Action::PlPopup(i));
        click(&mut app, &mut c, r);
        assert!(app.state.playlist.popup.is_none());
        assert_eq!(AudioCfg::current().buffer, Some(1024));
        assert_eq!(app.state.settings.audio.buffer, Some(1024));
        // Back to the defaults for the other tests in this process.
        AudioCfg::default().apply();
    }

    /// Metronome (spec: ui/metronome.md §1-2): the toolbar button and OPTIONS > Metronome toggle the shared switch
    /// (the button lit while on); right-click on the button lists the five sounds with the current one marked.
    #[test]
    fn metronome_button_menu_and_sound() {
        use os_engine::metronome::Sound;
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        frame(&mut app, &mut c, None);
        let was = crate::metronome::on();
        let r = hit_rect(&app, &Action::Metronome);
        assert_eq!((r.x, r.y), (580.0, 4.0));
        click(&mut app, &mut c, r);
        assert_eq!(crate::metronome::on(), !was);
        let opt = (0..60).find(|&i| views::menus::item_at(views::menus::OPTIONS_MENU, i).is_some_and(|it| it.label == "Metronome")).unwrap();
        app.menu_item(views::menus::OPTIONS_MENU, opt);
        assert_eq!(crate::metronome::on(), was);
        // The sound menu: a header, then the sounds; picking one keeps the switch.
        app.mouse_move(r.cx(), r.cy());
        app.mouse_down_right();
        frame(&mut app, &mut c, None);
        let labels: Vec<String> = app.state.playlist.popup.as_ref().unwrap().items.iter().map(|it| it.label.to_string()).collect();
        assert_eq!(labels, ["Metronome sound", "Hi-hat", "Tick", "Beep", "Cowbell", "Pulse"]);
        let before = crate::metronome::sound();
        let i = labels.iter().position(|x| x == "Cowbell").unwrap();
        let item = hit_rect(&app, &Action::PlPopup(i));
        click(&mut app, &mut c, item);
        assert_eq!((crate::metronome::sound(), crate::metronome::on()), (Sound::Cowbell, was));
        crate::metronome::set_sound(before);
    }

    /// Auto scrolling (spec: ui/auto-scrolling.md §1-2): the toolbar button and OPTIONS > Auto scrolling toggle the
    /// switch (the button lit while on); right-click on the button toggles continuous scrolling.
    #[test]
    fn auto_scrolling_button_and_menu() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        frame(&mut app, &mut c, None);
        assert!(!crate::autoscroll::on());
        let r = hit_rect(&app, &Action::AutoScroll);
        assert_eq!((r.x, r.y), (613.0, 45.0));
        click(&mut app, &mut c, r);
        assert!(crate::autoscroll::on());
        let opt = (0..60).find(|&i| views::menus::item_at(views::menus::OPTIONS_MENU, i).is_some_and(|it| it.label == "Auto scrolling")).unwrap();
        app.menu_item(views::menus::OPTIONS_MENU, opt);
        assert!(!crate::autoscroll::on());
        app.mouse_move(r.cx(), r.cy());
        app.mouse_down_right();
        frame(&mut app, &mut c, None);
        let labels: Vec<String> = app.state.playlist.popup.as_ref().unwrap().items.iter().map(|it| it.label.to_string()).collect();
        assert_eq!(labels, ["Continuous scrolling"]);
        let item = hit_rect(&app, &Action::PlPopup(0));
        click(&mut app, &mut c, item);
        assert_eq!(crate::autoscroll::get(), crate::autoscroll::Settings { on: false, continuous: false });
    }

    /// Auto scrolling follows the playhead (spec: ui/auto-scrolling.md §5): the playlist in song mode, the piano roll in
    /// pattern mode; continuous at a third of the width, paged by a quarter page at 7/8; not while paused or off. The
    /// playlist's scroll is drawn by moving pixels: each frame must equal a full repaint.
    #[test]
    fn auto_scrolling_follows_the_playhead() {
        let Ok(mut a) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut b = Canvas::new(1920, 1080, 1.0).unwrap();
        let mut app = App::new(None);
        app.state.project = test_project();
        crate::playlist::place(&mut app.state, 0, 0);
        crate::playlist::place(&mut app.state, 1, 1536);
        for w in [Win::Playlist, Win::PianoRoll] {
            let ws = &mut app.state.wins[w.slot().unwrap()];
            ws.maxed = false;
            ws.rect = Rect::new(220.0, 90.0, 1000.0, 480.0);
        }
        let play = |tick: f64, song: bool| Shown { playing: true, paused: false, tick, song, sounding: Vec::new() };
        crate::autoscroll::toggle();

        // Song mode, playlist in front.
        app.state.z = vec![Win::Playlist];
        frame(&mut app, &mut a, None);
        let g = app.pl_grid.expect("playlist shown");
        let ppt = |app: &App| app.state.playlist_bar_w / (f32::from(app.state.project.ppq.max(4)) * 4.0);
        let tick_at = |app: &App, x: f32| f64::from(x / ppt(app));
        app.show_playback(play(0.0, true));
        let d = app.take_damage();
        frame_damage(&mut app, &mut a, d);
        assert_eq!(app.state.playlist_scroll.0, 0.0);
        // Continuous: still up to a third, then held there.
        app.show_playback(play(tick_at(&app, g.w / 3.0 - 2.0), true));
        assert_eq!(app.state.playlist_scroll.0, 0.0);
        for k in [0.5, 0.52, 0.9] {
            let x = g.w * k;
            app.show_playback(play(tick_at(&app, x), true));
            assert!((app.state.playlist_scroll.0 - (x - g.w / 3.0)).abs() <= 0.5, "continuous at {k}");
            let d = app.take_damage();
            frame_damage(&mut app, &mut a, d);
            frame(&mut app, &mut b, None);
            assert_same(&a, &b, &format!("continuous at {k}"));
        }
        // Paged: nothing until 7/8 of the width, then a quarter page.
        crate::autoscroll::toggle_continuous();
        app.state.playlist_scroll.0 = 0.0;
        frame(&mut app, &mut a, None);
        app.show_playback(play(tick_at(&app, g.w * 0.8), true));
        assert_eq!(app.state.playlist_scroll.0, 0.0);
        app.show_playback(play(tick_at(&app, g.w * 0.9), true));
        assert!((app.state.playlist_scroll.0 - g.w * 0.275).abs() <= 0.5, "paged: {}", app.state.playlist_scroll.0);
        let d = app.take_damage();
        frame_damage(&mut app, &mut a, d);
        frame(&mut app, &mut b, None);
        assert_same(&a, &b, "paged step");
        // Back to the start: the view goes with it.
        app.show_playback(play(0.0, true));
        assert_eq!(app.state.playlist_scroll.0, 0.0);
        // Paused: the view stays.
        app.show_playback(Shown { paused: true, ..play(tick_at(&app, g.w * 3.0), true) });
        assert_eq!(app.state.playlist_scroll.0, 0.0);
        // Off: the view stays.
        app.show_playback(play(0.0, true));
        crate::autoscroll::toggle();
        app.show_playback(play(tick_at(&app, g.w * 3.0), true));
        assert_eq!(app.state.playlist_scroll.0, 0.0);
        app.show_playback(Shown::default());

        // Pattern mode, piano roll in front: its own width and zoom.
        crate::autoscroll::toggle();
        app.state.z = vec![Win::PianoRoll];
        frame(&mut app, &mut a, None);
        let g = app.piano_grid.expect("piano roll shown");
        let ppq = f64::from(app.state.project.ppq.max(4));
        let tick = f64::from(g.w) * 0.9 / f64::from(app.state.piano.px_per_beat) * ppq;
        app.show_playback(play(0.0, false));
        app.show_playback(play(tick, false));
        assert!((app.scroll_get(Scroll::PianoX) - g.w * 0.275).abs() <= 0.5, "piano roll paged: {}", app.scroll_get(Scroll::PianoX));
        let d = app.take_damage();
        frame_damage(&mut app, &mut a, d);
        frame(&mut app, &mut b, None);
        assert_same(&a, &b, "piano roll step");
    }

    /// CPU panel (spec: ui/main-window.md §13): the menu's View FPS / Misc / Refresh change what it shows; a double
    /// click opens the Plugin performance monitor (spec: ui/plugin-performance.md), which lists the hosted
    /// instruments, sorts, finds and closes from the VIEW menu.
    #[test]
    fn cpu_panel_menu_and_performance_monitor() {
        use crate::cpu::{CpuCmd, Refresh};
        use crate::perf::{Col, PerfAct};
        use std::time::{Duration, Instant};
        use winit::keyboard::{Key, NamedKey, SmolStr};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        frame(&mut app, &mut c, None);
        let r = hit_rect(&app, &Action::CpuPanel);
        // The menu: Refresh radio items in a submenu, the view pair, Misc.
        app.mouse_move(r.cx(), r.cy());
        app.mouse_down_right();
        frame(&mut app, &mut c, None);
        let labels: Vec<String> = app.state.playlist.popup.as_ref().unwrap().items.iter().map(|it| it.label.to_string()).collect();
        for l in ["Refresh", "View voice count", "View FPS", "Misc", "Plugin performance monitor", "Audio settings"] {
            assert!(labels.iter().any(|x| x == l), "{l} in {labels:?}");
        }
        let i = labels.iter().position(|x| x == "View FPS").unwrap();
        let item = hit_rect(&app, &Action::PlPopup(i));
        click(&mut app, &mut c, item);
        assert!(app.state.cpu.fps_view);
        app.state.cpu.frame();
        app.state.cpu.frame();
        let t0 = Instant::now();
        assert!(app.state.cpu.update(t0, None, 4, 0));
        assert!(app.state.cpu.hint().contains("frames per second"), "{}", app.state.cpu.hint());
        app.cpu_cmd(CpuCmd::Fps(false));
        app.cpu_cmd(CpuCmd::Misc);
        assert!(app.state.cpu.update(t0, None, 4, 0), "a reading comes at once after a change");
        let m = app.state.cpu.now.misc.expect("Misc reads the counts");
        assert!(m.threads > 0 && m.fonts > 0, "{m:?}");
        #[cfg(windows)]
        assert!(m.handles > 0);
        assert!(app.state.cpu.hint().contains("4 voices") && app.state.cpu.hint().contains(" threads"), "{}", app.state.cpu.hint());
        frame(&mut app, &mut c, None);
        app.cpu_cmd(CpuCmd::Misc);
        app.cpu_cmd(CpuCmd::Refresh(Refresh::Slow));
        assert!(app.state.cpu.update(t0, None, 0, 0));
        assert!(!app.state.cpu.due(t0 + Duration::from_millis(500)) && app.state.cpu.due(t0 + Duration::from_millis(1000)));
        // Double click: the monitor, with a row for each hosted instance.
        app.plugins = crate::plugins::Plugins::default();
        app.plugins.available = crate::plugins::builtin();
        let osc = app.plugins.index_of(crate::osc3::LEMON_TRIO).unwrap();
        app.plugins.create_with(0, osc, None).unwrap();
        click(&mut app, &mut c, r);
        assert!(!app.state.shown(Win::Perf), "one click does nothing");
        app.mouse_move(r.cx(), r.cy());
        app.mouse_down();
        app.mouse_up();
        frame(&mut app, &mut c, None);
        assert!(app.state.shown(Win::Perf) && app.state.in_front(Win::Perf));
        let p = &app.state.perf;
        assert_eq!(p.rows.len(), 1);
        assert_eq!((p.rows[0].name.as_str(), p.rows[0].synth, p.rows[0].voices), ("Lemon Trio", true, Some(0)));
        assert!(app.hits.iter().any(|h| h.action == Action::Perf(PerfAct::Row(0))));
        // Sorting by a header; a second click reverses.
        let h = hit_rect(&app, &Action::Perf(PerfAct::Sort(Col::Plugin)));
        click(&mut app, &mut c, h);
        assert_eq!((app.state.perf.sort, app.state.perf.desc), (Col::Plugin, false));
        click(&mut app, &mut c, h);
        assert!(app.state.perf.desc);
        // Find takes the typing (the typing keyboard does not play), Esc clears it.
        let f = hit_rect(&app, &Action::Perf(PerfAct::Find));
        click(&mut app, &mut c, f);
        assert!(app.state.perf.finding);
        for ch in ["z", "z"] {
            app.key(&Key::Character(SmolStr::new(ch)), false, false);
        }
        assert_eq!(app.state.perf.find, "zz");
        assert!(app.state.perf.shown().is_empty());
        frame(&mut app, &mut c, None);
        app.key(&Key::Named(NamedKey::Backspace), false, false);
        app.key(&Key::Named(NamedKey::Escape), false, false);
        assert!(app.state.perf.find.is_empty() && !app.state.perf.finding);
        // Reset on transport and Freeze are toggles.
        let t = hit_rect(&app, &Action::Perf(PerfAct::Freeze));
        click(&mut app, &mut c, t);
        assert!(app.state.perf.freeze);
        // VIEW > Plugin performance monitor closes it while it is in front; closing resets the readings.
        app.act(Action::ToggleWin(Win::Perf));
        assert!(!app.state.shown(Win::Perf) && app.state.perf.rows.is_empty());
        app.act(Action::ToggleWin(Win::Perf));
        assert!(app.state.shown(Win::Perf));
        frame(&mut app, &mut c, None);
        let x = hit_rect(&app, &Action::CloseWin(Win::Perf));
        click(&mut app, &mut c, x);
        assert!(!app.state.shown(Win::Perf));
    }

    /// Tempo tapper (spec: ui/tempo-tapper.md): VIEW > Tempo tapper opens it centred over the main window, ticked in
    /// the menu; clicks on the pad set the project tempo (whole BPM, clamped to the tempo box) while Apply tempo is on,
    /// only show it while off, and turning it on applies the shown tempo; a long pause starts a new count; MIDI keys
    /// tap (a chord once); the title bar moves it, Ctrl+F5 and its close button close it, and it reopens where it was.
    #[test]
    fn tempo_tapper_window() {
        use crate::tapper::TapAct;
        use views::menus::{bar_model, Mark, VIEW};
        use winit::keyboard::{Key, NamedKey};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        frame(&mut app, &mut c, None);
        let view = hit_rect(&app, &Action::Menu(VIEW));
        click(&mut app, &mut c, view);
        let item = hit_rect(&app, &Action::ToggleWin(Win::Tapper));
        click(&mut app, &mut c, item);
        assert!(app.state.shown(Win::Tapper) && app.state.in_front(Win::Tapper));
        assert_eq!(views::tapper::rect(&app.state), Rect::new(882.0, 393.0, 156.0, 294.0), "156 x 294, centred over the main window");
        let row = bar_model(&app.state)[VIEW].rows.iter().find(|r| r.label == "Tempo tapper").cloned().unwrap();
        assert_eq!((row.enabled, row.mark, row.shortcut), (true, Mark::Check(true), "Ctrl+F5"));
        // The hints; the controls that do nothing yet only say so.
        let hint = |app: &App, a: &Action| app.hits.iter().rev().find(|h| &h.action == a).map(|h| h.hint.to_string()).unwrap();
        assert_eq!(hint(&app, &Action::Tapper(TapAct::Pad)), "Tap me, or press any key on your MIDI keyboard");
        for name in [
            "Nudge tempo to align time (Left / pitch bend)",
            "Nudge tempo to align time (Right / pitch bend)",
            "Resynchronize to taps",
            "Mute all audio during first taps (during playback)",
            "Mute now, for a few bars (during playback)",
        ] {
            assert_eq!(hint(&app, &Action::Placeholder(name)), format!("{name}: not available yet"));
        }

        // Apply tempo on (the default): from the second tap, every tempo is the project's.
        let pad = hit_rect(&app, &Action::Tapper(TapAct::Pad));
        let tap = |app: &mut App, c: &mut Canvas, t: f64| {
            app.state.tapper.test_time = Some(t);
            click(app, c, pad);
        };
        assert!(crate::tapper::get().apply);
        assert_eq!(app.state.tempo, 130.0);
        tap(&mut app, &mut c, 10.0);
        assert_eq!((app.state.tapper.count.tempo(), app.state.tempo), (None, 130.0), "one tap: TAP stays");
        assert!(app.state.tapper.flash == 1.0 && app.animating(), "a tap lights the pad, which then fades");
        tap(&mut app, &mut c, 10.5);
        assert_eq!(app.state.tempo, 120.0);
        tap(&mut app, &mut c, 11.0);
        tap(&mut app, &mut c, 11.4);
        assert_eq!(app.state.tempo, 129.0, "the average of every interval, rounded (128.57)");
        assert!(app.unsaved(), "a tapped tempo is a change to the project");
        // Off: the pad shows the tempo, the project keeps its own; turning it on applies the pad's.
        let apply = hit_rect(&app, &Action::Tapper(TapAct::Apply));
        click(&mut app, &mut c, apply);
        assert!(!crate::tapper::get().apply);
        tap(&mut app, &mut c, 11.8);
        assert_eq!((app.state.tapper.count.tempo(), app.state.tempo), (Some(133.0), 129.0));
        click(&mut app, &mut c, apply);
        assert!(crate::tapper::get().apply);
        assert_eq!(app.state.tempo, 133.0);
        // A long pause starts a new count (the tempo stays); the project tempo stays in 10-522.
        tap(&mut app, &mut c, 20.0);
        assert_eq!((app.state.tapper.count.tempo(), app.state.tempo), (None, 133.0));
        tap(&mut app, &mut c, 20.05);
        assert_eq!((app.state.tapper.count.tempo(), app.state.tempo), (Some(1200.0), 522.0));
        // MIDI keys tap while the window is open; a chord's keys are one tap.
        for t in [30.0, 30.03, 30.5] {
            app.state.tapper.test_time = Some(t);
            app.midi_tap();
        }
        assert_eq!(app.state.tempo, 120.0);
        // Every tap clicked, accented on the first of each four of a count (§8); the chord's second key did not.
        let (t, f) = (true, false);
        assert_eq!(app.state.tapper.test_clicks, [t, f, f, f, t, t, f, t, f]);
        frame(&mut app, &mut c, None);

        // The title bar moves it; Ctrl+F5 closes it while it is in front and reopens it where it was left.
        let title = hit_rect(&app, &Action::WinDrag(Win::Tapper));
        app.mouse_move(title.x + 40.0, title.cy());
        app.mouse_down();
        app.mouse_move(title.x + 40.0 - 300.0, title.cy() + 100.0);
        app.mouse_up();
        frame(&mut app, &mut c, None);
        assert_eq!(app.state.tapper.pos, (582.0, 493.0));
        app.key(&Key::Named(NamedKey::F5), true, false);
        assert!(!app.state.shown(Win::Tapper));
        assert!(app.state.shown(Win::Playlist), "Ctrl+F5 is not F5");
        app.key(&Key::Named(NamedKey::F5), true, false);
        assert!(app.state.shown(Win::Tapper) && app.state.tapper.pos == (582.0, 493.0));
        assert_eq!(app.state.tapper.count.tempo(), Some(120.0), "the last tempo stays on the pad");
        frame(&mut app, &mut c, None);
        let x = hit_rect(&app, &Action::CloseWin(Win::Tapper));
        click(&mut app, &mut c, x);
        assert!(!app.state.shown(Win::Tapper));
        let row = bar_model(&app.state)[VIEW].rows.iter().find(|r| r.label == "Tempo tapper").cloned().unwrap();
        assert_eq!(row.mark, Mark::Check(false));
        // The toolbar's kick drum button (the 8th window button) opens and closes it too.
        let b = hit_rect(&app, &Action::ToggleWin(Win::Tapper));
        assert_eq!((b.x, hint(&app, &Action::ToggleWin(Win::Tapper)).as_str()), (973.0 + 33.0 * 7.0, "Tempo tapper (Ctrl+F5)"));
        click(&mut app, &mut c, b);
        assert!(app.state.shown(Win::Tapper) && app.state.in_front(Win::Tapper));
        click(&mut app, &mut c, b);
        assert!(!app.state.shown(Win::Tapper));
    }

    /// The tapper's flash fades on the ticks (spec: ui/tempo-tapper.md §4, §8): each repaints just the pad, which shows
    /// the same as a full paint, in both designs; once out, ticking stops.
    #[test]
    fn tempo_tapper_flash_repaints_the_pad() {
        use std::time::{Duration, Instant};
        for design in Design::ALL {
            let Ok(mut a) = Canvas::new(1920, 1080, 1.0) else { return };
            let mut b = Canvas::new(1920, 1080, 1.0).unwrap();
            let mut app = App::new(None);
            app.theme = design_theme(design, false);
            app.act(Action::ToggleWin(Win::Tapper));
            app.state.tapper.test_time = Some(1.0);
            app.act(Action::Tapper(crate::tapper::TapAct::Pad));
            app.state.tapper.test_time = Some(1.5);
            app.act(Action::Tapper(crate::tapper::TapAct::Pad));
            frame(&mut app, &mut a, None);
            let _ = app.take_damage();
            let pad = views::tapper::pad_rect(&app.state);
            for (ms, what) in [(100, "fading"), (300, "gone")] {
                app.state.tapper.flash_at = Some(Instant::now() - Duration::from_millis(ms));
                app.tick();
                let d = app.take_damage();
                let Some(Some(rects)) = &d else { panic!("{what}: repaints regions, not the frame") };
                assert!(rects.iter().any(|r| r.intersect(&pad) == pad), "{what}: the pad is repainted ({rects:?})");
                frame_damage(&mut app, &mut a, d);
                frame(&mut app, &mut b, None);
                assert_same(&a, &b, &format!("{design:?}: flash {what}"));
            }
            assert!(!app.animating() && app.state.tapper.flash == 0.0);
        }
    }

    /// An empty project with a Lemon Trio played from the typing keyboard while the UI ticks, then in a playing
    /// pattern: the streams keep up (no run of dropouts) and the voices end.
    #[test]
    #[ignore]
    fn playing_lemon_3x_osc_keeps_the_audio_going() {
        use std::time::{Duration, Instant};
        use winit::keyboard::KeyCode;
        let mut app = App::new(None);
        app.plugins = crate::plugins::Plugins::default();
        app.plugins.available = crate::plugins::builtin();
        let row = add_plugin_channel(&mut app.state.project, crate::osc3::LEMON_TRIO);
        let index = app.plugins.index_of(crate::osc3::LEMON_TRIO).unwrap();
        app.plugins.create(row, index).unwrap();
        app.state.hosted_rows.push(row);
        app.transport.set_instruments(app.plugins.instruments());
        app.state.selected_channel = row;
        app.state.toggles.push("typing");
        let run = |app: &mut App, ms: u64| {
            let t0 = Instant::now();
            while t0.elapsed() < Duration::from_millis(ms) {
                app.tick();
                std::thread::sleep(Duration::from_millis(4));
            }
        };
        run(&mut app, 300);
        // Past the live stream's first 4095 ticks (about 18 s at 140 BPM): it used to go silent there.
        let u0 = app.transport.underruns();
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(25) {
            app.typing_key(KeyCode::KeyZ, true, false, false);
            run(&mut app, 200);
            app.typing_key(KeyCode::KeyZ, false, false, false);
            run(&mut app, 300);
        }
        let du = app.transport.underruns() - u0;
        eprintln!("long live: {du} dropouts");
        assert!(du < 5, "long live run: {du} dropouts");
        let keys = [KeyCode::KeyZ, KeyCode::KeyX, KeyCode::KeyC, KeyCode::KeyV, KeyCode::KeyB, KeyCode::KeyN, KeyCode::KeyM, KeyCode::KeyQ, KeyCode::KeyW, KeyCode::KeyE];
        let phase = |app: &mut App, name: &str| {
            let u0 = app.transport.underruns();
            for round in 0..30 {
                for (i, &k) in keys.iter().enumerate() {
                    if (round + i) % 3 != 0 {
                        app.typing_key(k, true, false, false);
                    }
                }
                run(app, 30);
                for &k in &keys {
                    app.typing_key(k, false, false, false);
                }
                run(app, 15);
            }
            run(app, 1500);
            let du = app.transport.underruns() - u0;
            let voices = app.transport.engine_voices() + app.plugins.voices();
            eprintln!("{name}: {du} dropouts, {voices} voices after");
            (du, voices)
        };
        let (du, v) = phase(&mut app, "live");
        assert!(du < 5 && v == 0, "live notes: {du} dropouts, {v} voices");
        // A pattern of the synth playing, with live notes on top, then Stop.
        app.state.project.patterns[0].notes = (0..16).map(|i| os_model::Note { length: 24, key: 48 + (i % 5) as u16 * 3, ..os_model::Note::step(app.state.project.channels[row].index, i * 24) }).collect();
        app.act(Action::Play);
        run(&mut app, 300);
        let (du, _) = phase(&mut app, "playing");
        app.act(Action::Stop);
        run(&mut app, 500);
        let v = app.transport.engine_voices() + app.plugins.voices();
        assert!(du < 5 && v == 0, "while playing: {du} dropouts; {v} voices after Stop");
    }

    /// Browser (spec: ui/browser.md): tabs, opening nodes, dragging a pattern and a generator onto the playlist.
    #[test]
    fn browser_tree_and_drag() {
        use crate::views::browser::{rows, Item};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        app.state.file = None;
        app.state.path = None;
        app.state.playlist.reset();
        app.state.z = vec![Win::Playlist];
        frame(&mut app, &mut c, None);
        // The all tab lists the roots with content, all closed.
        let labels: Vec<String> = rows(&app.state).into_iter().map(|r| r.label).collect();
        assert_eq!(labels, ["Current project", "Plugin database"]);
        // Project tab: Current project opened with its patterns (default like the screenshot), History above them.
        let r0 = hit_rect(&app, &Action::BrowserTab(1));
        click(&mut app, &mut c, r0);
        let r = rows(&app.state);
        assert_eq!(r[0].label, "Current project");
        assert_eq!((r[1].label.as_str(), r[1].open), ("History", Some(false)));
        assert_eq!(r[2].label, "Patterns");
        let pi = r.iter().position(|r| r.item == Item::Pattern(1)).expect("pattern row");
        // Drag pattern 1 onto the playlist grid: a pattern clip appears.
        let n0 = crate::playlist::clips(&app.state).len();
        let src = hit_rect(&app, &Action::BrowserRow(pi));
        let grid = app.pl_grid().expect("grid");
        app.mouse_move(src.cx(), src.cy());
        app.mouse_down();
        app.mouse_move(src.cx() + 30.0, src.cy());
        assert!(app.state.browser_drag.is_some(), "drag label shown");
        app.mouse_move(grid.x + 100.0, grid.y + 10.0);
        app.mouse_up();
        assert!(app.state.browser_drag.is_none());
        let clips = crate::playlist::clips(&app.state);
        assert_eq!(clips.len(), n0 + 1);
        assert_eq!(clips.last().unwrap().source, os_model::ClipSource::Pattern(1));
        frame(&mut app, &mut c, None);
        // Collapsing Patterns hides its rows; a plain click is not a drop.
        let pat_hdr = rows(&app.state).iter().position(|r| r.label == "Patterns").unwrap();
        let r0 = hit_rect(&app, &Action::BrowserRow(pat_hdr));
        click(&mut app, &mut c, r0);
        assert!(rows(&app.state).iter().all(|r| r.item != Item::Pattern(1)));
        assert_eq!(crate::playlist::clips(&app.state).len(), n0 + 1);
        // Plugin database tab: open Generators, drag Lemon Sampler onto the playlist -> a new channel.
        app.plugins = crate::plugins::Plugins::builtins();
        app.state.plugdb.rebuild(&app.plugins);
        let r0 = hit_rect(&app, &Action::BrowserTab(2));
        click(&mut app, &mut c, r0);
        let gi = rows(&app.state).iter().position(|r| r.label == "Generators").unwrap();
        let r0 = hit_rect(&app, &Action::BrowserRow(gi));
        click(&mut app, &mut c, r0);
        let si = rows(&app.state).iter().position(|r| r.item == Item::Generator(0)).expect("sampler row");
        let ch0 = app.state.project.channels.len();
        let src = hit_rect(&app, &Action::BrowserRow(si));
        app.mouse_move(src.cx(), src.cy());
        app.mouse_down();
        app.mouse_move(grid.x + 200.0, grid.y + 60.0);
        app.mouse_move(grid.x + 210.0, grid.y + 60.0);
        app.mouse_up();
        assert_eq!(app.state.project.channels.len(), ch0 + 1);
        // Online / favorites tabs are not available yet: greyed out (no tab hit), their pages empty; wheel never
        // scrolls past the content.
        assert!(!app.hits.iter().any(|h| h.action == Action::BrowserTab(4)));
        app.act(Action::BrowserTab(4));
        assert!(rows(&app.state).is_empty());
        let r0 = hit_rect(&app, &Action::BrowserTab(0));
        click(&mut app, &mut c, r0);
        app.mouse_move(100.0, 300.0);
        app.wheel(0.0, -5.0, false, false);
        assert_eq!(app.state.browser_scroll, 0.0);
    }

    /// Browser options menu and search (spec: ui/browser.md §4, §5).
    /// A clicked browser sample plays from its file (the 5 s cut-off is the transport's job).
    #[test]
    fn browser_sample_preview_project() {
        let path = std::env::temp_dir().join(format!("os_preview_test_{}.wav", std::process::id()));
        let (rate, n) = (44100u32, 44100usize);
        let mut wav = Vec::new();
        wav.extend(b"RIFF");
        wav.extend(&(36 + n as u32 * 2).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(&16u32.to_le_bytes());
        wav.extend(&1u16.to_le_bytes());
        wav.extend(&1u16.to_le_bytes());
        wav.extend(&rate.to_le_bytes());
        wav.extend(&(rate * 2).to_le_bytes());
        wav.extend(&2u16.to_le_bytes());
        wav.extend(&16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend(&(n as u32 * 2).to_le_bytes());
        for i in 0..n {
            let v = ((i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 16000.0) as i16;
            wav.extend(&v.to_le_bytes());
        }
        std::fs::write(&path, wav).unwrap();
        let p = preview_project(&path.to_string_lossy());
        let mut library = os_engine::SampleLibrary::default();
        let mut e = os_engine::Engine::new(&p, &mut library, 48000, os_engine::PlayMode::Pattern(1));
        let mut buf = vec![0.0f32; 48000];
        e.process(&mut buf);
        let _ = std::fs::remove_file(&path);
        assert!(buf.iter().any(|v| v.abs() > 0.05), "preview is silent");
    }

    /// Audio-clip channels (dropped audio, FL's audio clips) are hosted and opened by Lemon Sampler, never the info
    /// window (spec: plugins/lemon-sampler.md §7).
    #[test]
    fn audio_clip_channels_use_lemon_sampler() {
        let mut app = App::new(None);
        app.plugins = crate::plugins::Plugins::default();
        app.plugins.available = crate::plugins::builtin();
        let wav = std::env::temp_dir().join("os-app-audio-clip-channel.wav");
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &[0.5; 9600]).unwrap();
        let path = wav.to_string_lossy().into_owned();
        crate::playlist::add_audio_clip(&mut app.state, &path, 0, 0).expect("audio clip added");
        let row = app.state.project.channels.len() - 1;
        assert_eq!(app.state.project.channels[row].kind, ChannelKind::AudioClip);
        app.plugin_tick();
        assert!(app.state.hosted_rows.contains(&row), "Lemon Sampler hosts the audio clip channel");
        assert_eq!(app.plugins.sampler_path(row).as_deref(), Some(path.as_str()));
        app.toggle_channel_window(row);
        assert!(!app.state.shown(Win::Plugin), "no information window");
        assert_eq!(app.state.pending_open, Some(row), "Lemon Sampler's window opens");
    }

    /// ADD ▸ Lemon Sampler with "Detach all new plugins" on: the new channel's instance opens detached; channels added
    /// before and the project's own keep docked (spec: ui/settings-choices.md "Miscellaneous: naming, detached plugins
    /// and startup").
    #[test]
    fn added_sampler_channels_take_the_detach_check() {
        let mut a = App::new(None);
        a.plugins = crate::plugins::Plugins::default();
        a.plugins.available = crate::plugins::builtin();
        let sampler = a.plugins.index_of(crate::plugins::LEMON_SAMPLER).unwrap();
        crate::plugin_menu::add_generator(&mut a, sampler, None);
        let off = a.state.pending_open.expect("its window opens once hosted");
        a.sync_samplers(None);
        a.state.pending_open = None;
        crate::prefs::set_check(&crate::prefs::DETACH_NEW, true);
        crate::plugin_menu::add_generator(&mut a, sampler, None);
        let on = a.state.pending_open.unwrap();
        a.sync_samplers(None);
        let detached = |r: usize| a.plugins.hosted[&r].detached;
        assert!(detached(on));
        assert!(!detached(off), "added before: keeps its mode");
        assert!(!detached(0), "the project's channel");
    }

    /// Lemon Sampler's settings are saved with the project and come back when it is reopened (spec:
    /// plugins/lemon-sampler.md §7): envelopes and LFOs, the channel filter, declick, the sampler flags, the start
    /// offset, the precomputed settings and the root note page. Untouched settings keep the channel's own values.
    #[test]
    fn sampler_settings_save_and_reopen() {
        use serde_json::json;
        let flp = std::env::temp_dir().join(format!("os_sampler_settings_{}.flp", std::process::id()));
        let mut a = App::new(None);
        a.plugins = crate::plugins::Plugins::default();
        a.plugins.available = crate::plugins::builtin();
        a.sync_samplers(None);
        let row = (0..a.state.project.channels.len()).find(|&r| a.state.project.channels[r].kind.is_sampler()).expect("a sampler channel");
        assert!(a.state.hosted_rows.contains(&row));
        let untouched = a.state.project.channels[row].clone();
        let edits = [
            ("envvol_on", json!({ "bool": true })),
            ("envvol_att", json!({ "f32": 0.5 })),
            ("envvol_tempo", json!({ "bool": true })),
            ("envvol_lfo_amt", json!({ "f32": -0.5 })),
            ("envvol_lfo_shape", json!({ "i32": 2 })),
            ("envpitch_amt", json!({ "f32": 0.25 })),
            ("f_cut", json!({ "f32": 0.25 })),
            ("f_type", json!({ "i32": 3 })),
            ("declick", json!({ "i32": 2 })),
            ("loop_use", json!({ "bool": false })),
            ("fx_reverse", json!({ "bool": true })),
            ("fx_boost", json!({ "f32": 0.5 })),
            ("fx_start", json!({ "f32": 0.25 })),
            ("fx_sdelay", json!({ "f32": -0.5 })),
            ("envpan_lfo_global", json!({ "bool": true })),
            ("keep_disk", json!({ "bool": true })),
            ("resample", json!({ "bool": true })),
            ("start_offset", json!({ "f32": 0.125 })),
            ("root", json!({ "i32": 48 })),
            ("fine", json!({ "i32": -20 })),
            ("main_pitch", json!({ "bool": false })),
            ("add_key", json!({ "bool": true })),
            ("zone_lo", json!({ "i32": 37 })),
            ("zone_hi", json!({ "i32": 60 })),
        ];
        let st = crate::plugins::with_params(&a.plugins.state(row).unwrap(), &edits).unwrap();
        assert!(a.plugins.set_state(row, &st));

        // Saving copies them into the channel, in the file's units.
        a.save_to(flp.clone());
        assert!(a.state.message.as_deref().is_some_and(|m| m.starts_with("Saved")), "{:?}", a.state.message);
        let ch = &a.state.project.channels[row];
        let vol = &ch.envelopes[os_model::Envelope::VOLUME];
        assert!(vol.enabled && vol.attack == 32768 && vol.flags & 1 == 1 && vol.lfo_amount == -64 && vol.lfo_shape == 2);
        // The pitch record's amount is in cents (±2400).
        assert_eq!(ch.envelopes[os_model::Envelope::PITCH].amount, 600);
        assert_eq!(ch.envelopes[os_model::Envelope::PAN].flags & 0x20, 0x20, "global LFO");
        // FL's new channel's flags (Load regions, Use loop points), loop points off, Keep on disk and Resample on.
        assert_eq!((ch.sound.cutoff, ch.sound.filter_type, ch.sound.declick, ch.sound.sampler_flags), (64, 3, 2, Some(0x103)));
        assert_eq!(ch.sound.start_offset(), 0.125);
        let m = ch.misc;
        assert_eq!((m.root, m.fine, m.main_pitch, m.add_key, m.key_range), (48, -20, false, true, [37, 60]));
        let fx = ch.sound.fx;
        assert_eq!((fx.flags, fx.boost, fx.stereo_delay, fx.start()), (os_model::SampleFx::REVERSE, 128, 1024, 0.25));
        // Everything not edited is as it was.
        assert_eq!(ch.envelopes[os_model::Envelope::PAN].attack, untouched.envelopes[os_model::Envelope::PAN].attack);
        assert_eq!((fx.fade_in, fx.pogo, fx.length_bits), (untouched.sound.fx.fade_in, untouched.sound.fx.pogo, untouched.sound.fx.length_bits));
        assert_eq!(ch.sound.resonance, untouched.sound.resonance);

        // Reopened: the file holds them, and the new Lemon Sampler starts with them.
        let saved = ch.clone();
        let mut b = App::new(None);
        b.plugins = crate::plugins::Plugins::default();
        b.plugins.available = crate::plugins::builtin();
        b.open(flp.clone());
        let _ = std::fs::remove_file(&flp);
        let t0 = std::time::Instant::now();
        while !b.state.hosted_rows.contains(&row) && t0.elapsed() < std::time::Duration::from_secs(20) {
            b.plugin_tick();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let reopened = &b.state.project.channels[row];
        assert_eq!((reopened.envelopes, reopened.sound), (saved.envelopes, saved.sound));
        let (m, n) = (reopened.misc, saved.misc);
        assert_eq!((m.root, m.fine, m.main_pitch, m.add_key, m.key_range), (n.root, n.fine, n.main_pitch, n.add_key, n.key_range));
        let st = b.plugins.state(row).expect("Lemon Sampler hosted again");
        let v: serde_json::Value = serde_json::from_slice(&st[8..]).unwrap();
        for (id, want) in &edits {
            let got = &v["params"][*id];
            match (got.get("f32"), want.get("f32")) {
                (Some(g), Some(w)) => assert!((g.as_f64().unwrap() - w.as_f64().unwrap()).abs() < 1e-6, "{id}: {got} != {want}"),
                _ => assert_eq!(got, want, "{id}"),
            }
        }
    }

    /// A Lemon Sampler state read back into the channel it started from changes nothing: every value the file holds,
    /// even one a knob cannot show exactly, is kept while its knob is untouched.
    #[test]
    fn untouched_sampler_settings_store_as_they_were() {
        let mut pl = crate::plugins::Plugins::default();
        pl.available = crate::plugins::builtin();
        let index = pl.index_of(crate::plugins::LEMON_SAMPLER).unwrap();
        let mut ch = test_project().channels.remove(0);
        let vol = &mut ch.envelopes[os_model::Envelope::VOLUME];
        (vol.enabled, vol.attack, vol.sustain, vol.lfo_amount, vol.lfo_shape, vol.flags) = (true, 50, 77, -3, 1, 0x20 | 4 | 2);
        (ch.envelopes[os_model::Envelope::PITCH].amount, ch.envelopes[os_model::Envelope::PITCH].lfo_amount) = (1201, -7);
        (ch.sound.cutoff, ch.sound.resonance, ch.sound.filter_type, ch.sound.declick, ch.sound.sampler_flags) = (300, 64, 5, 6, Some(0x10b));
        (ch.sound.start_offset_bits, ch.sound.loop_points) = (0.3f64.to_bits(), Some([12, 34]));
        (ch.misc.root, ch.misc.fine, ch.misc.main_pitch, ch.misc.add_key, ch.misc.key_range) = (48, -35, false, true, [37, 300]);
        ch.sound.fx = os_model::SampleFx {
            flags: os_model::SampleFx::SWAP,
            boost: 77,
            trim: 32768,
            start_bits: 0.1f64.to_bits(),
            crossfade: 3,
            ..Default::default()
        };
        pl.create_with(0, index, Some(&crate::plugins::sampler_state(&ch))).unwrap();
        let mut back = ch.clone();
        crate::plugins::store_sampler(&mut back, &pl.state(0).unwrap());
        assert_eq!(back, ch);
    }

    /// Loop points are frames of the sample file and Lemon Sampler's loop knobs fractions of its last frame's index
    /// (2000 for this 2001-frame file; spec: plugins/lemon-sampler.md §7): FL's values for the sample's ends show the
    /// whole sample, stored points come back while the knobs are untouched, and a moved knob stores its frame.
    #[test]
    fn sampler_loop_points_are_frames_of_the_file() {
        use serde_json::json;
        let wav = std::env::temp_dir().join(format!("os_loop_points_{}.wav", std::process::id()));
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 44100, &[0.25; 4002]).unwrap();
        let mut pl = crate::plugins::Plugins::default();
        pl.available = crate::plugins::builtin();
        let index = pl.index_of(crate::plugins::LEMON_SAMPLER).unwrap();
        let mut ch = test_project().channels.remove(0);
        ch.sample_path = Some(wav.to_string_lossy().into_owned());
        let knobs = |pl: &crate::plugins::Plugins| {
            let v: serde_json::Value = serde_json::from_slice(&pl.state(0).unwrap()[8..]).unwrap();
            (v["params"]["loop_start"]["f32"].as_f64().unwrap(), v["params"]["loop_end"]["f32"].as_f64().unwrap())
        };
        for (points, want) in [([-1, i64::MAX], (0.0, 1.0)), ([500, 1500], (0.25, 0.75))] {
            ch.sound.loop_points = Some(points);
            pl.create_with(0, index, Some(&crate::plugins::sampler_state(&ch))).unwrap();
            assert_eq!(knobs(&pl), want);
            let mut back = ch.clone();
            crate::plugins::store_sampler(&mut back, &pl.state(0).unwrap());
            assert_eq!(back.sound.loop_points, Some(points), "untouched");
        }
        let st = crate::plugins::with_params(&pl.state(0).unwrap(), &[("loop_start", json!({ "f32": 0.5 }))]).unwrap();
        crate::plugins::store_sampler(&mut ch, &st);
        assert_eq!(ch.sound.loop_points, Some([1000, 1500]));
        // A channel without loop points gets them once a knob moves.
        ch.sound.loop_points = None;
        pl.create_with(0, index, Some(&crate::plugins::sampler_state(&ch))).unwrap();
        let st = crate::plugins::with_params(&pl.state(0).unwrap(), &[("loop_end", json!({ "f32": 0.5 }))]).unwrap();
        crate::plugins::store_sampler(&mut ch, &st);
        let _ = std::fs::remove_file(&wav);
        assert_eq!(ch.sound.loop_points, Some([0, 1000]));
    }

    /// An opened project decodes its samples on worker threads, then starts its samplers behind the progress panel;
    /// loading ends with every sampler channel hosted on the shared decode (spec: ui/project-loading.md).
    #[test]
    fn opened_projects_load_their_samplers_first() {
        let flp = std::env::temp_dir().join(format!("os_loading_{}.flp", std::process::id()));
        let wav = std::env::temp_dir().join(format!("os_loading_{}.wav", std::process::id()));
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &[0.25; 96000]).unwrap();
        let wav = wav.to_string_lossy().into_owned();
        let mut app = App::new(None);
        for c in app.state.project.channels.iter_mut().filter(|c| c.kind.is_sampler()) {
            c.sample_path = Some(wav.clone());
        }
        let rows: Vec<usize> = (0..app.state.project.channels.len()).filter(|&r| app.state.project.channels[r].kind.is_sampler()).collect();
        assert!(rows.len() > 1);
        app.save_to(flp.clone());
        app.plugins = crate::plugins::Plugins::default();
        app.plugins.available = crate::plugins::builtin();
        app.open(flp.clone());
        let _ = std::fs::remove_file(&flp);
        assert!(app.loading(), "opening starts loading");
        assert!(app.animating(), "the panel keeps ticking");
        let mut canvas = Canvas::new(1920, 1080, 1.0).unwrap();
        app.paint(&mut canvas, None);
        let t0 = std::time::Instant::now();
        while app.loading() && t0.elapsed() < std::time::Duration::from_secs(20) {
            app.plugin_tick();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!app.loading(), "loading ends");
        for &r in &rows {
            assert!(app.state.hosted_rows.contains(&r), "row {r} has its Lemon Sampler");
            assert_eq!(app.plugins.sampler_path(r).as_deref(), Some(wav.as_str()));
        }
        let _ = std::fs::remove_file(&wav);
    }

    /// The Misc tab's values are saved with the channel and come back on open (spec: audio/channel-misc.md
    /// "Settings", audio/voice-levels.md, audio/swing.md, audio/note-rules.md), tracking, the arpeggiator and the echo
    /// delay included: the first save adds their events, a later one patches them. (Full portamento is not saved yet:
    /// OpenStudio-specs REQUESTS.md #5.)
    #[test]
    fn channel_misc_save_and_reopen() {
        use crate::misc_page::M;
        let flp = std::env::temp_dir().join(format!("os_channel_misc_{}.flp", std::process::id()));
        let reopen = |a: &mut App| {
            a.save_to(flp.clone());
            assert!(a.state.message.as_deref().is_some_and(|m| m.starts_with("Saved")), "{:?}", a.state.message);
            let mut b = App::new(None);
            b.plugins = crate::plugins::Plugins::default();
            b.open(flp.clone());
            b
        };
        let mut a = App::new(None);
        a.plugins = crate::plugins::Plugins::default();
        let row = 0;
        let untouched = a.state.project.channels[row].misc;
        // Values the file's units hold exactly.
        let first = [
            (M::LevelPan, 0.25),
            (M::LevelVol, 0.75),
            (M::LevelModX, 0.625),
            (M::LevelModY, 0.375),
            (M::PolyMax, 3.0),
            (M::PolySlide, 820.0 / 1024.0),
            (M::Porta, 1.0),
            (M::Mono, 1.0),
            (M::TimeGate, 499.0 / 997.0),
            (M::TimeShift, 0.25),
            (M::TimeSwing, 0.5),
            (M::Cut, 2.0),
            (M::CutBy, 3.0),
            (M::Root, 48.0),
            (M::Fine, -35.0),
            (M::MainPitch, 0.0),
            (M::AddKey, 1.0),
            (M::VolMid, 80.0),
            (M::VolPan, 0.75),
            (M::VolModX, 0.25),
            (M::VolModY, 1.0),
            (M::KeyMid, 72.0),
            (M::KeyPan, 0.0),
            (M::KeyModY, 0.625),
            (M::ArpDir, 4.0),
            (M::ArpTime, 600.0 / 997.0),
            (M::ArpGate, 0.625),
            (M::ArpRange, 0.0),
            (M::ArpRepeat, 3.0),
            (M::ArpSlide, 1.0),
            (M::ArpChord, 7.0),
            (M::EchoFeed, 0.375),
            (M::EchoPan, 0.625),
            (M::EchoModX, 0.25),
            (M::EchoModY, 0.625),
            (M::EchoPitch, 0.625),
            (M::EchoTime, 0.125),
            (M::Echoes, 7.0),
            (M::PingPong, 1.0),
            (M::FatMode, 1.0),
        ];
        for (m, v) in first {
            crate::misc::set(&mut a, row, m, v);
        }
        let want = a.state.project.channels[row].misc;
        assert_ne!(want, untouched);
        let mut b = reopen(&mut a);
        assert_eq!(b.state.project.channels[row].misc, want);

        let second = [
            (M::LevelPan, 0.5),
            (M::LevelVol, 0.25),
            (M::PolyMax, 0.0),
            (M::Mono, 0.0),
            (M::TimeGate, 1.0),
            (M::TimeShift, 0.5),
            (M::TimeSwing, 1.0),
            (M::CutBy, 0.0),
            (M::Root, 72.0),
            (M::Fine, 20.0),
            (M::MainPitch, 1.0),
            (M::VolMid, 100.0),
            (M::KeyPan, 0.625),
            (M::ArpDir, 3.0),
            (M::ArpChord, -1.0),
            (M::ArpRepeat, 1.0),
            (M::EchoFeed, 0.5),
            (M::EchoModY, 0.5),
            (M::FatMode, 0.0),
        ];
        for (m, v) in second {
            crate::misc::set(&mut b, row, m, v);
        }
        let want = b.state.project.channels[row].misc;
        let c = reopen(&mut b);
        let _ = std::fs::remove_file(&flp);
        assert_eq!(c.state.project.channels[row].misc, want);
    }

    /// Opening, loading and saving leave no unsaved changes; an edit does; "Save new version" numbers the file.
    #[test]
    fn unsaved_changes_and_new_versions() {
        let dir = std::env::temp_dir().join(format!("os_versions_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(None);
        assert!(!app.unsaved(), "a fresh project");
        app.save_to(dir.join("Song.flp"));
        app.plugins = crate::plugins::Plugins::default();
        app.plugins.available = crate::plugins::builtin();
        app.open(dir.join("Song.flp"));
        let mut canvas = Canvas::new(1920, 1080, 1.0).unwrap();
        let t0 = std::time::Instant::now();
        while app.loading() && t0.elapsed() < std::time::Duration::from_secs(20) {
            app.plugin_tick();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        for _ in 0..5 {
            app.tick();
            app.paint(&mut canvas, None);
        }
        assert!(!app.unsaved(), "opening and loading change nothing");
        app.state.tempo = 99.0;
        assert!(app.unsaved(), "an edit is unsaved");
        app.act(Action::MenuItem(0, views::menus::FILE_SAVE));
        assert!(!app.unsaved(), "saved");
        app.act(Action::MenuItem(0, views::menus::FILE_SAVE_VERSION));
        assert_eq!(app.state.path.as_deref(), Some(dir.join("Song_2.flp").as_path()));
        app.act(Action::MenuItem(0, views::menus::FILE_SAVE_VERSION));
        assert_eq!(app.state.path.as_deref(), Some(dir.join("Song_3.flp").as_path()));
        assert!(dir.join("Song_2.flp").exists() && dir.join("Song_3.flp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Shift+Ctrl+S is Save as: it asks where and leaves the open file as it was. Ctrl+S saves in place without asking.
    /// Shift+Ctrl+O / R / N have no command: they neither open, export nor write a new version.
    #[test]
    fn shift_ctrl_s_is_save_as() {
        use crate::dialog::fake;
        use winit::keyboard::Key;
        let dir = std::env::temp_dir().join(format!("os_save_as_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(None);
        app.state.project = test_project();
        // Something to export, so Ctrl+R gets as far as its file dialog.
        app.state.project.patterns[0].notes.push(Note::step(0, 0));
        let song = dir.join("Song.flp");
        app.save_to(song.clone());
        let before = std::fs::read(&song).unwrap();
        let key = |app: &mut App, k: &str, shift: bool| {
            app.shift = shift;
            app.key(&Key::Character(k.into()), true, false);
            app.shift = false;
        };

        app.state.tempo = 99.0;
        key(&mut app, "S", true);
        assert_eq!(fake::shown(), (0, 1), "Save as asks where");
        assert_eq!(std::fs::read(&song).unwrap(), before, "cancelled: the open file is not written");
        assert!(app.unsaved());
        let other = dir.join("Other.flp");
        fake::answer(Some(other.clone()));
        key(&mut app, "S", true);
        assert_eq!(app.state.path.as_deref(), Some(other.as_path()));
        assert!(other.exists() && !app.unsaved(), "saved to the chosen file");
        assert_eq!(std::fs::read(&song).unwrap(), before, "the old file is left as it was");

        for k in ["O", "R", "N"] {
            key(&mut app, k, true);
        }
        assert_eq!(fake::shown(), (0, 2), "no open or export dialog");
        assert!(app.export.is_none());
        assert!(!dir.join("Other_2.flp").exists(), "no new version");

        app.state.tempo = 100.0;
        key(&mut app, "s", false);
        assert_eq!(fake::shown(), (0, 2), "Ctrl+S does not ask");
        assert!(!app.unsaved(), "Ctrl+S saved in place");
        key(&mut app, "n", false);
        assert!(dir.join("Other_2.flp").exists(), "Ctrl+N still saves a new version");
        key(&mut app, "o", false);
        key(&mut app, "r", false);
        assert_eq!(fake::shown(), (1, 3), "Ctrl+O and Ctrl+R still ask for a file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An effect dragged from the plugin database onto the mixer goes into the current track's first free slot
    /// (spec: ui/browser.md §3).
    #[test]
    fn browser_effect_drops_into_a_mixer_slot() {
        use crate::views::browser::{rows, Item};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        app.plugins.effects = vec![crate::plugin_db::fake("Test FX", os_host::Kind::Effect)];
        app.state.plugdb.rebuild(&app.plugins);
        app.state.z = vec![Win::Mixer];
        app.state.browser_tab = 2;
        app.state.browser_open.insert("2:Plugin database/Effects".into());
        frame(&mut app, &mut c, None);
        let i = rows(&app.state).iter().position(|r| r.item == Item::Effect(0)).expect("effect row");
        let src = hit_rect(&app, &Action::BrowserRow(i));
        let dst = hit_rect(&app, &Action::MixerArea);
        app.mouse_move(src.cx(), src.cy());
        app.mouse_down();
        app.mouse_move(src.cx() + 30.0, src.cy());
        app.mouse_move(dst.cx(), dst.cy());
        app.mouse_up();
        let t = app.state.selected_mixer;
        assert_eq!(crate::mixer::slot_name(&app.state, t, 0).as_deref(), Some("Test FX"));
    }

    /// The piano roll's scroll-bar arrows step their axis (spec: ui/piano-roll.md §1a).
    #[test]
    fn piano_scroll_arrows_step() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.z = vec![Win::PianoRoll];
        frame(&mut app, &mut c, None);
        for t in [Scroll::PianoY, Scroll::PianoX] {
            let v0 = app.scroll_get(t);
            let r = hit_rect(&app, &Action::ScrollStep(t, 1));
            click(&mut app, &mut c, r);
            // The step eases (General ▸ Display ▸ Smoothing).
            assert!(app.smooth.running(), "{t:?} eases");
            app.ease_to_end();
            assert!(app.scroll_get(t) > v0, "{t:?} steps forward");
            let r = hit_rect(&app, &Action::ScrollStep(t, -1));
            click(&mut app, &mut c, r);
            app.ease_to_end();
            assert!((app.scroll_get(t) - v0).abs() < 0.01, "{t:?} steps back: {v0} -> {}", app.scroll_get(t));
        }
    }

    #[test]
    fn browser_dropped_folder() {
        use crate::views::browser::{rows, Item};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let dir = std::env::temp_dir().join(format!("os_browser_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Kicks")).unwrap();
        std::fs::write(dir.join("Kicks").join("Kick 01.wav"), b"x").unwrap();
        std::fs::write(dir.join("readme.txt"), b"hello").unwrap();
        std::fs::write(dir.join(".hidden"), b"").unwrap();
        let mut app = App::new(None);
        frame(&mut app, &mut c, None);
        app.drop_file(dir.clone());
        let r = rows(&app.state);
        let i = r.iter().position(|r| r.path.as_deref() == Some(dir.as_path())).expect("root row");
        assert_eq!(r[i].open, Some(true));
        // Folders first, then files; hidden files left out.
        assert_eq!(r[i + 1].label, "Kicks");
        assert_eq!(r[i + 2].label, "readme.txt");
        assert!(matches!(r[i + 2].item, Item::File(_)));
        assert_eq!(r.len(), i + 3, "the dropped folder is the last root");
        // Open the subfolder: the sample shows without extension and drags as a sample.
        frame(&mut app, &mut c, None);
        let sub = hit_rect(&app, &Action::BrowserRow(i + 1));
        click(&mut app, &mut c, sub);
        let r = rows(&app.state);
        assert_eq!(r[i + 2].label, "Kick 01");
        assert!(matches!(r[i + 2].item, Item::Sample(_)));
        assert_eq!(r[i + 2].depth, 2);
        // Right-click the root removes it.
        frame(&mut app, &mut c, None);
        let root = hit_rect(&app, &Action::BrowserRow(i));
        app.mouse_move(root.cx(), root.cy());
        app.mouse_down_right();
        assert!(app.state.browser_folders.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn browser_menu_and_search() {
        use crate::views::browser::{rows, Item};
        use winit::keyboard::{Key, NamedKey, SmolStr};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        app.state.project.patterns.push(os_model::Pattern { number: 2, name: Some("Kick + Clap".into()), ..Default::default() });
        app.state.z = vec![Win::Playlist];
        frame(&mut app, &mut c, None);
        // The ▸ opens the menu; its items follow the screenshot.
        let r0 = hit_rect(&app, &Action::BrowserMenu);
        click(&mut app, &mut c, r0);
        let labels: Vec<String> = app.state.playlist.popup.as_ref().expect("menu").items.iter().map(|i| i.label.to_string()).collect();
        assert_eq!(labels[0], "Search");
        assert!(labels.iter().any(|l| l == "Refresh plugin list (fast scan)"));
        // Toggle "Show parent folders in search results".
        let i = labels.iter().position(|l| l == "Show parent folders in search results").unwrap();
        let r0 = hit_rect(&app, &Action::PlPopup(i));
        click(&mut app, &mut c, r0);
        assert!(app.state.playlist.popup.is_none());
        assert!(crate::views::browser::opt(&app.state, "parents"));
        // Click the search box and type: the whole tree is searched, closed nodes included.
        let r0 = hit_rect(&app, &Action::BrowserSearch);
        click(&mut app, &mut c, r0);
        assert!(app.state.browser_search_focus);
        for ch in ["k", "i", "c", "k"] {
            app.key(&Key::Character(SmolStr::new(ch)), false, false);
        }
        assert_eq!(app.state.browser_search, "kick");
        let r = rows(&app.state);
        // Parents kept: Current project > Patterns > Kick + Clap, and Generators > Kick.
        assert_eq!(r[0].label, "Current project");
        assert!(r.iter().any(|r| r.item == Item::Pattern(2)));
        assert!(r.iter().any(|r| r.label == "Kick" && r.depth == 2));
        // Find next selects the first match, then the second.
        app.key(&Key::Named(NamedKey::F3), false, false);
        let first = app.state.browser_sel.clone().unwrap();
        app.key(&Key::Named(NamedKey::F3), false, false);
        let second = app.state.browser_sel.clone().unwrap();
        assert_ne!(second, first);
        app.key(&Key::Named(NamedKey::F2), false, false);
        assert_eq!(app.state.browser_sel.clone().unwrap(), first);
        // Parents off: matches listed flat.
        app.state.browser_opts.remove("parents");
        assert!(rows(&app.state).iter().all(|r| r.depth == 0 && r.label.to_lowercase().contains("kick")));
        // Navigation history: back = the second match (first, second, first), forward = first again.
        app.browser_cmd(crate::browser::BCmd::History(-1));
        assert_eq!(app.state.browser_sel.clone().unwrap(), second);
        app.browser_cmd(crate::browser::BCmd::History(1));
        assert_eq!(app.state.browser_sel.clone().unwrap(), first);
        // Esc clears the search and leaves the box.
        app.key(&Key::Named(NamedKey::Escape), false, false);
        assert!(app.state.browser_search.is_empty() && !app.state.browser_search_focus);
        assert_eq!(rows(&app.state).len(), 2);
        // Search box on top moves the list down.
        app.browser_cmd(crate::browser::BCmd::Toggle("top"));
        frame(&mut app, &mut c, None);
        assert!(app.hits.iter().any(|h| h.action == Action::BrowserSearch && h.rect.y < 200.0));
    }

    /// Channel rack (spec: ui/channel-rack.md §5): stacking with other windows, name button opens
    /// the plugin window, select slot, mixer-track drag, add, resize; drags repaint like full frames.
    #[test]
    fn channel_rack_window() {
        let Ok(mut a) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut b = Canvas::new(1920, 1080, 1.0).unwrap();
        let mut app = App::new(None);
        frame(&mut app, &mut a, None);
        // Clicking the playlist next to the rack puts the rack behind it; F6 brings it back.
        let rack = views::rack::rect(&app.state);
        click(&mut app, &mut a, Rect::new(rack.right() + 50.0, rack.y + 100.0, 2.0, 2.0));
        assert_eq!(app.state.z.last(), Some(&Win::Playlist));
        let over = (rack.right() - 60.0, rack.y + 100.0); // inside both windows
        assert_eq!(app.top_hit(over.0, over.1).and_then(|h| h.win), Some(Win::Playlist), "the playlist covers the rack there");
        app.act(Action::ToggleWin(Win::ChannelRack));
        frame(&mut app, &mut a, None);
        assert_eq!(app.state.z.last(), Some(&Win::ChannelRack));
        app.act(Action::ToggleWin(Win::ChannelRack));
        assert!(!app.state.shown(Win::ChannelRack), "F6 on the front rack hides it");
        app.act(Action::ToggleWin(Win::ChannelRack));
        frame(&mut app, &mut a, None);

        // Name button opens the channel's Lemon Sampler (a sampler row never shows the info window); the slot selects.
        let hr = hit_rect(&app, &Action::ChannelSelect(2));
        click(&mut app, &mut a, hr);
        assert_eq!((app.state.pending_open, app.state.selected_channel), (Some(2), 2));
        assert!(!app.state.shown(Win::Plugin));
        let hr = hit_rect(&app, &Action::RackSelect(3));
        click(&mut app, &mut a, hr);
        assert_eq!(app.state.rack.sel, std::collections::BTreeSet::from([3]));
        assert_eq!(app.state.z.last(), Some(&Win::ChannelRack), "clicking the rack raised it");

        // Drag the mixer box up 20 px: +5 inserts, with damage repaint matching a full frame.
        let _ = app.take_damage();
        let mb = hit_rect(&app, &Action::RackMixer(0));
        let t0 = app.state.project.channels[0].mixer_track;
        let step = |app: &mut App, c: &mut Canvas, f: &dyn Fn(&mut App)| {
            f(app);
            let d = app.take_damage();
            frame_damage(app, c, d);
        };
        step(&mut app, &mut a, &|ap| { ap.mouse_move(mb.cx(), mb.cy()); });
        step(&mut app, &mut a, &|ap| ap.mouse_down());
        step(&mut app, &mut a, &|ap| { ap.mouse_move(mb.cx(), mb.cy() - 20.0); });
        assert_eq!(app.state.project.channels[0].mixer_track, t0 + 2, "10 px per insert (§12)");
        step(&mut app, &mut a, &|ap| ap.mouse_up());

        // Resize from the corner: wider shows more steps, taller more room.
        let r = views::rack::rect(&app.state);
        step(&mut app, &mut a, &|ap| { ap.mouse_move(r.right() - 2.0, r.bottom() - 2.0); });
        assert_eq!(app.cursor(), Cursor::NwseResize);
        step(&mut app, &mut a, &|ap| ap.mouse_down());
        step(&mut app, &mut a, &|ap| { ap.mouse_move(r.right() + 318.0, r.bottom() + 58.0); });
        step(&mut app, &mut a, &|ap| ap.mouse_up());
        assert_eq!(app.state.rack_size, (views::rack::DEFAULT_SIZE.0 + 320.0, 306.0));
        assert_eq!(views::rack::visible_steps(&app.state), 36);
        frame(&mut app, &mut b, None);
        assert_same(&a, &b, "after mixer drag and resize");

        // A dimmed step (past the one-bar pattern) extends the pattern to two bars.
        assert_eq!(crate::rack::pattern_steps(&app.state), 16);
        let hr = hit_rect(&app, &Action::Step(0, 20));
        click(&mut app, &mut a, hr);
        assert_eq!(crate::rack::pattern_steps(&app.state), 32);

        // Sizes snap to whole steps / rows, down to 4 steps and one row (FL's widths plus the wider name button).
        let d = crate::rack::NAME_W - crate::rack::FL_NAME_W;
        assert_eq!(views::rack::snap_size(&app.state, 822.0 + d, 300.0), (819.0 + d, 306.0));
        assert_eq!(views::rack::snap_size(&app.state, 10.0, 10.0), (307.0 + d, 126.0));
        let keep = app.state.rack_size;
        app.state.rack_size = (307.0 + d, 126.0);
        assert_eq!(views::rack::visible_steps(&app.state), 4);
        app.state.rack_size = keep;

        // Piano-roll view toggle: rows become mini rolls; clicking one opens the Piano roll.
        assert!(!app.hits.iter().any(|h| matches!(h.action, Action::RackRoll(_))));
        let hr = hit_rect(&app, &Action::RackRollView);
        click(&mut app, &mut a, hr);
        assert!(app.state.rack_roll_view);
        assert!(!app.hits.iter().any(|h| matches!(h.action, Action::Step(..))), "no steps in roll view");
        let hr = hit_rect(&app, &Action::RackRoll(1));
        click(&mut app, &mut a, hr);
        assert_eq!((app.state.z.last(), app.state.selected_channel), (Some(&Win::PianoRoll), 1));
        app.state.rack_roll_view = false;
        app.act(Action::ToggleWin(Win::ChannelRack));
        frame(&mut app, &mut a, None);

        // Add channels until the rows overflow: a vertical scroll bar appears. "+" opens the generator menu
        // (spec: ui/plugin-menu.md §1); Lemon Sampler adds a sampler channel at the bottom.
        app.plugins = crate::plugins::Plugins::builtins();
        app.state.plugdb.rebuild(&app.plugins);
        let n = app.state.project.channels.len();
        for _ in 0..6 {
            let hr = hit_rect(&app, &Action::RackAdd);
            click(&mut app, &mut a, hr);
            let pick = app.state.playlist.popup.as_ref().and_then(|p| p.items.iter().find(|i| i.label == crate::plugins::LEMON_SAMPLER)).map(|i| i.cmd.clone());
            crate::playlist::run(&mut app, pick.expect("the generator menu lists Lemon Sampler"));
            assert!(app.state.playlist.popup.is_none());
            frame(&mut app, &mut a, None);
        }
        assert_eq!(app.state.project.channels.len(), n + 6);
        assert_eq!(app.state.project.channels.last().unwrap().mixer_track, 11, "after the highest insert in use (5)");
        // Auto resize (minimum height, §7) grows the rack with its rows; with it off the rows overflow into a scroll bar.
        assert!(app.bar(Scroll::RackY).is_none_or(|b| b.max == 0.0), "auto resize fits the rows");
        app.state.rack.auto_resize = crate::rack::AutoResize::Off;
        app.state.rack_size.1 = 200.0;
        frame(&mut app, &mut a, None);
        assert!(app.bar(Scroll::RackY).is_some_and(|b| b.max > 0.0));
    }

    /// Playlist / Mixer / Piano roll windows: drag the title bar, resize from the edges (down to the
    /// minimum size), minimize and maximize; damage repaints match full frames.
    #[test]
    fn window_move_resize_min_max() {
        for design in Design::ALL {
            window_move_resize_in(design);
        }
    }

    fn window_move_resize_in(design: Design) {
        let Ok(mut a) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut b = Canvas::new(1920, 1080, 1.0).unwrap();
        let mut app = App::new(None);
        app.theme = design_theme(design, false);
        app.state.wins.iter_mut().for_each(|w| w.maxed = false);
        app.state.z = vec![Win::Playlist, Win::Mixer];
        frame(&mut app, &mut a, None);
        let _ = app.take_damage();
        let step = |app: &mut App, c: &mut Canvas, f: &dyn Fn(&mut App)| {
            f(app);
            let d = app.take_damage();
            frame_damage(app, c, d);
        };
        let r0 = app.state.win_rect(Win::Mixer);
        assert_eq!(r0, Rect::new(231.0, 565.0, 1552.0, 435.0));

        // Move by the title bar.
        let bar = hit_rect(&app, &Action::WinDrag(Win::Mixer));
        let (bx, by) = (bar.cx(), bar.cy());
        step(&mut app, &mut a, &|ap| { ap.mouse_move(bx, by); });
        step(&mut app, &mut a, &|ap| ap.mouse_down());
        step(&mut app, &mut a, &|ap| { ap.mouse_move(bx - 100.0, by - 50.0); });
        step(&mut app, &mut a, &|ap| ap.mouse_up());
        let r1 = app.state.win_rect(Win::Mixer);
        assert_eq!((r1.x, r1.y, r1.w, r1.h), (r0.x - 100.0, r0.y - 50.0, r0.w, r0.h));

        // Double-click the title bar: maximize, then double-click again: back to r1.
        for want_max in [true, false] {
            let bar = hit_rect(&app, &Action::WinDrag(Win::Mixer));
            let (bx, by) = (bar.x + 150.0, bar.cy());
            step(&mut app, &mut a, &|ap| { ap.mouse_move(bx, by); });
            for _ in 0..2 {
                step(&mut app, &mut a, &|ap| ap.mouse_down());
                step(&mut app, &mut a, &|ap| ap.mouse_up());
            }
            assert_eq!(app.state.is_max(Win::Mixer), want_max);
        }
        assert_eq!(app.state.win_rect(Win::Mixer), r1);

        // Resize from the bottom-right corner: bigger, then clamped to the minimum size.
        let corner = hit_rect(&app, &Action::WinResize(Win::Mixer, EDGE_R | EDGE_B));
        let (cx, cy) = (corner.cx(), corner.cy());
        step(&mut app, &mut a, &|ap| { ap.mouse_move(cx, cy); });
        assert_eq!(app.cursor(), Cursor::NwseResize);
        step(&mut app, &mut a, &|ap| ap.mouse_down());
        step(&mut app, &mut a, &|ap| { ap.mouse_move(cx - 40.0, cy + 30.0); });
        let r2 = app.state.win_rect(Win::Mixer);
        assert_eq!((r2.w, r2.h), (r1.w - 40.0, r1.h + 30.0));
        step(&mut app, &mut a, &|ap| { ap.mouse_move(cx - 3000.0, cy - 3000.0); });
        let r3 = app.state.win_rect(Win::Mixer);
        assert_eq!((r3.x, r3.y, r3.w, r3.h), (r1.x, r1.y, Win::Mixer.min_size().0, Win::Mixer.min_size().1));
        step(&mut app, &mut a, &|ap| ap.mouse_up());
        frame(&mut app, &mut b, None);
        assert_same(&a, &b, "after window move and resize");

        // Top-left resize keeps the opposite corner fixed.
        frame(&mut app, &mut a, None);
        let tl = hit_rect(&app, &Action::WinResize(Win::Mixer, EDGE_L | EDGE_T));
        step(&mut app, &mut a, &|ap| { ap.mouse_move(tl.cx(), tl.cy()); });
        step(&mut app, &mut a, &|ap| ap.mouse_down());
        step(&mut app, &mut a, &|ap| { ap.mouse_move(tl.cx() - 60.0, tl.cy() - 40.0); });
        step(&mut app, &mut a, &|ap| ap.mouse_up());
        let r4 = app.state.win_rect(Win::Mixer);
        assert_eq!((r4.right(), r4.bottom()), (r3.right(), r3.bottom()));
        assert_eq!((r4.w, r4.h), (r3.w + 60.0, r3.h + 40.0));

        // Minimize collapses to the title bar and restores; maximize fills the workspace.
        app.act(Action::MinWin(Win::Mixer));
        frame(&mut app, &mut a, None);
        assert_eq!(app.state.win_rect(Win::Mixer).h, TITLE_H);
        assert!(app.hits.iter().all(|h| !matches!(h.action, Action::WinResize(Win::Mixer, _))), "no resize edges when collapsed");
        app.act(Action::MinWin(Win::Mixer));
        assert_eq!(app.state.win_rect(Win::Mixer), r4);
        app.act(Action::MaxWin(Win::Mixer));
        frame(&mut app, &mut a, None);
        assert_eq!(app.state.win_rect(Win::Mixer), app.state.work_rect());
        assert_eq!(app.state.docked(), Some(Win::Mixer));
        app.act(Action::MaxWin(Win::Mixer));
        assert_eq!(app.state.win_rect(Win::Mixer), r4);

        // The window buttons act on the release over them: not on the press, and not when the mouse slides off
        // before the release.
        frame(&mut app, &mut a, None);
        let close = hit_rect(&app, &Action::CloseWin(Win::Mixer));
        step(&mut app, &mut a, &|ap| { ap.mouse_move(close.cx(), close.cy()); });
        step(&mut app, &mut a, &|ap| ap.mouse_down());
        assert!(app.state.shown(Win::Mixer), "the press does not close");
        step(&mut app, &mut a, &|ap| { ap.mouse_move(close.cx() - 200.0, close.cy() + 200.0); });
        step(&mut app, &mut a, &|ap| ap.mouse_up());
        assert!(app.state.shown(Win::Mixer), "released off the button: nothing");
        step(&mut app, &mut a, &|ap| { ap.mouse_move(close.cx(), close.cy()); });
        step(&mut app, &mut a, &|ap| ap.mouse_down());
        step(&mut app, &mut a, &|ap| ap.mouse_up());
        assert!(!app.state.shown(Win::Mixer), "released over the button: closed");

        // The mixer layout scales with its height only (1 at the measured size).
        assert_eq!(views::mixer::scale(Rect::new(209.0, 83.0, 1711.0, 997.0)), 1.0);
        assert!(views::mixer::scale(Rect::new(0.0, 0.0, 600.0, 435.0)) < 0.3);

        // Dragging a fader works at a squished size: 0 dB stays under the handle's start.
        let mut app = App::new(None);
        app.state.z = vec![Win::Mixer];
        frame(&mut app, &mut a, None);
        let f = hit_rect(&app, &Action::Fader(1));
        app.mouse_move(f.cx(), f.cy());
        app.mouse_down();
        app.mouse_move(f.cx(), f.cy() - 10.0);
        app.mouse_up();
        let v = app.state.project.mixer.iter().find(|m| m.index == 1).and_then(|m| m.volume).unwrap();
        assert!(v > 0.8, "fader moved up: {v}");
    }

    /// Tiny windows (e.g. while restoring after Alt+Tab) must not panic in any view.
    #[test]
    fn tiny_windows_do_not_panic() {
        for (w, h) in [(1, 1), (120, 28), (400, 120), (800, 300), (1920, 140)] {
            let Ok(mut c) = Canvas::new(w, h, 1.0) else { return };
            for z in [vec![Win::Mixer, Win::ChannelRack], vec![Win::Playlist], vec![Win::PianoRoll, Win::Plugin]] {
                let mut app = App::new(None);
                app.state.z = z;
                frame(&mut app, &mut c, None);
                app.mouse_move(w as f32 / 2.0, h as f32 / 2.0);
                app.wheel(0.0, -1.0, false, false);
                frame(&mut app, &mut c, None);
            }
        }
    }

    /// Needs an audio device: `cargo test -p os-app -- --ignored app_deleted_notes`.
    #[test]
    #[ignore]
    fn app_deleted_notes_stop_playing() {
        use winit::keyboard::{Key, NamedKey};
        let mut app = App::new(None);
        app.state.z = vec![Win::PianoRoll];
        app.state.selected_channel = 0;
        let ch = piano::channel(&app.state).unwrap();
        for i in 0..8 {
            piano::add_note(&mut app.state, ch, i * 48, 60);
        }
        app.act(Action::Play);
        for _ in 0..30 {
            std::thread::sleep(std::time::Duration::from_millis(16));
            app.tick();
        }
        assert!(app.transport.playing());
        app.key(&Key::Character("a".into()), true, false);
        app.key(&Key::Named(NamedKey::Delete), false, false);
        assert!(piano::notes(&app.state).is_empty());
        let mut loud = 0;
        for i in 0..120 {
            std::thread::sleep(std::time::Duration::from_millis(16));
            app.tick();
            if i > 20 && !app.transport.sounding().is_empty() {
                loud += 1;
            }
        }
        assert_eq!(loud, 0, "deleted notes still sounding");
    }

    /// Play / pause / stop and the PAT / SONG switch drive one consistent state: the playheads follow the playing
    /// mode and tick, pause holds the tick, stop rewinds (needs an audio device).
    #[test]
    #[ignore]
    fn transport_states_and_modes() {
        use crate::transport::State as T;
        let step = || std::thread::sleep(std::time::Duration::from_millis(250));
        let mut app = App::new(None);
        let ch = app.state.project.channels[0].index;
        app.state.project.patterns = vec![Pattern {
            number: 1,
            notes: (0..8).map(|i| Note { length: 24, key: 60, ..Note::step(ch, i * 48) }).collect(),
            ..Pattern::default()
        }];
        app.state.project.arrangements = vec![os_model::Arrangement {
            number: 1,
            name: None,
            clips: vec![os_model::Clip::new(0, 384 * 4, 0, os_model::ClipSource::Pattern(1))],
            ..Default::default()
        }];
        // Song mode.
        app.act(Action::PatSong(true));
        app.act(Action::Play);
        step();
        assert_eq!(app.transport.state(), T::Playing);
        assert!(app.transport.song() && app.transport.tick() > 10.0, "song tick {}", app.transport.tick());
        // Pause holds the tick in place; Play resumes from it (no restart).
        app.act(Action::Play);
        step();
        let held = app.transport.tick();
        assert_eq!(app.transport.state(), T::Paused);
        step();
        assert_eq!(app.transport.tick(), held, "paused tick holds");
        app.act(Action::Play);
        step();
        assert!(app.transport.tick() > held, "resumed from {held}: {}", app.transport.tick());
        // Switching to PAT restarts in pattern mode, which loops inside one bar.
        app.act(Action::PatSong(false));
        step();
        assert!(app.transport.playing() && !app.transport.song());
        assert!(app.transport.tick() < 384.0 + 1.0);
        // Stop rewinds and stays stopped.
        app.act(Action::Stop);
        step();
        assert_eq!((app.transport.state(), app.transport.tick()), (T::Stopped, 0.0));
        // An empty playlist does not start SONG mode.
        app.state.project.arrangements.clear();
        app.act(Action::PatSong(true));
        app.act(Action::Play);
        step();
        assert_eq!(app.transport.state(), T::Stopped);
    }

    /// A click in the event panel sets the note already playing where it lands (the latest start at or before it),
    /// above or below its line, in a gap after it too; left of the first note, the first note (spec: ui/piano-roll.md
    /// §7, "Drag in the event panel").
    #[test]
    fn event_panel_click_sets_the_note_already_playing() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.z = vec![Win::PianoRoll];
        app.state.selected_channel = 0;
        let ch = piano::channel(&app.state).unwrap();
        let p = piano::pattern_mut(&mut app.state);
        p.notes.retain(|n| n.channel != ch);
        p.notes.extend([96, 192].map(|pos| os_model::Note { length: 48, key: 60, velocity: 64, ..os_model::Note::step(ch, pos) }));
        frame(&mut app, &mut c, None);
        let (g, panel) = (app.piano_geom().unwrap(), app.piano_panel.unwrap());
        let vel = |app: &App| -> Vec<u8> {
            piano::notes(&app.state).iter().filter(|n| n.channel == ch).map(|n| n.velocity).collect()
        };
        let (low, high) = (panel.bottom() - panel.h * 0.1, panel.y + panel.h * 0.1);
        let click = |app: &mut App, tick: f32, y: f32| {
            let x = g.x_of(tick);
            app.paint_velocity(x, y, x, y);
        };
        click(&mut app, 10.0, low);
        let first = vel(&app);
        assert!(first[0] < 20 && first[1] == 64, "left of the first note: {first:?}");
        click(&mut app, 160.0, high);
        let gap = vel(&app);
        assert!(gap[0] > 100 && gap[1] == 64, "a gap: the note before it: {gap:?}");
        click(&mut app, 200.0, low);
        let inside = vel(&app);
        assert!(inside[0] > 100 && inside[1] < 20, "inside the second note: {inside:?}");
    }

    /// SONG mode: a click in the piano roll's ruler moves the song position to where that tick of the pattern plays
    /// (the clip under the current position first, else the next, else the first); the piano roll's playhead maps
    /// the song position back into the pattern while a clip of it plays. A tick no clip shows keeps the old
    /// behaviour (the play-start marker).
    #[test]
    fn piano_ruler_in_song_mode_seeks_the_song() {
        use crate::playlist as pl;
        use os_model::{Clip, ClipSource};
        let mut app = App::new(None);
        app.state.project = test_project();
        app.state.pattern = 1;
        app.state.song_mode = true;
        // Pattern 1 at bar 2 (window 0..384) and at bar 5 showing its second half only (window 192..384).
        let mut cut = Clip::new(4 * 384, 192, 0, ClipSource::Pattern(1));
        cut.window = Some((192.0, 384.0));
        let mut whole = Clip::new(384, 384, 0, ClipSource::Pattern(1));
        whole.window = Some((0.0, 384.0));
        pl::arr_mut(&mut app.state).clips = vec![whole, cut];
        assert_eq!(piano::song_tick_of(&app.state, 96, 0), Some(480), "before both clips: the first that shows it");
        assert_eq!(piano::song_tick_of(&app.state, 96, 1000), Some(480), "only the first clip shows tick 96");
        assert_eq!(piano::song_tick_of(&app.state, 240, 1600), Some(1584), "the clip under the position");
        assert_eq!(piano::song_tick_of(&app.state, 240, 800), Some(1584), "the next clip after the position");
        assert_eq!(piano::pattern_tick_in_song(&app.state, 1584.0), Some(240.0));
        assert_eq!(piano::pattern_tick_in_song(&app.state, 1000.0), None, "between the clips");
        // Stopped: the press moves the song position.
        assert!(app.piano_song_seek(96, true));
        assert_eq!(app.state.playlist.pos, 480);
        // A tick no clip shows: nothing moves.
        pl::arr_mut(&mut app.state).clips.truncate(1);
        assert!(!app.piano_song_seek(500, true));
        assert_eq!(app.state.playlist.pos, 480);
    }

    /// Shortening a pattern clip that plays its whole pattern cuts it: it gets a window and plays only that.
    #[test]
    fn a_resized_pattern_clip_plays_only_its_window() {
        use crate::playlist as pl;
        use os_model::{Clip, ClipSource};
        let mut app = App::new(None);
        app.state.project = test_project();
        pl::arr_mut(&mut app.state).clips = vec![Clip::new(0, 384, 0, ClipSource::Pattern(1))];
        let i = 0;
        let orig = pl::clips(&app.state)[i];
        assert_eq!(orig.window, None, "a new pattern clip plays its whole pattern");
        pl::resize_right(&mut app.state, i, &orig, 48);
        let c = pl::clips(&app.state)[i];
        assert_eq!((c.length, c.window), (48, Some((0.0, 48.0))));
    }

    /// Same with right-drag erasing and note moves while playing (needs an audio device).
    #[test]
    #[ignore]
    fn app_erased_notes_stop_playing() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.z = vec![Win::PianoRoll];
        app.state.selected_channel = 0;
        frame(&mut app, &mut c, None);
        let g = app.piano_geom().unwrap();
        let y = g.y_of(64) + 8.0;
        // Draw notes by clicking, while playing.
        app.act(Action::Play);
        for i in 0..8 {
            app.mouse_move(g.x_of((i * 48) as f32) + 2.0, y);
            app.mouse_down();
            app.mouse_move(g.x_of((i * 48) as f32) + 2.0, y + 18.0 * (i % 3) as f32);
            app.mouse_up();
            frame(&mut app, &mut c, None);
            app.tick();
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        for _ in 0..60 {
            std::thread::sleep(std::time::Duration::from_millis(16));
            app.tick();
        }
        assert!(!app.transport.sounding().is_empty() || app.transport.peaks().0 > 0.0, "playing notes");
        // Right-drag across all rows to erase.
        frame(&mut app, &mut c, None);
        let g = app.piano_geom().unwrap();
        for key in (55..=70).rev() {
            let yy = g.y_of(key) + 8.0;
            if !g.grid.contains(g.grid.x + 2.0, yy) {
                continue;
            }
            app.mouse_move(g.grid.x + 2.0, yy);
            app.mouse_down_right();
            for i in 0..120 {
                app.mouse_move(g.grid.x + 2.0 + i as f32 * 4.0, yy);
                if i % 8 == 0 {
                    app.tick();
                }
            }
            app.mouse_up();
            frame(&mut app, &mut c, None);
        }
        let ch = piano::channel(&app.state);
        assert!(piano::notes(&app.state).iter().all(|n| Some(n.channel) != ch), "{:?}", piano::notes(&app.state));
        let mut loud = 0;
        for i in 0..150 {
            std::thread::sleep(std::time::Duration::from_millis(16));
            app.tick();
            if i > 20 && !app.transport.sounding().is_empty() {
                loud += 1;
            }
        }
        assert_eq!(loud, 0, "erased notes still sounding: {:?}", app.transport.sounding());
    }

    /// The grid / event panel splitter (spec: ui/event-panel-splitter.md): drag with the minimums and collapsing,
    /// double click to hide and bring back the panel, the vertical resize cursor.
    #[test]
    fn event_panel_splitter() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.z = vec![Win::PianoRoll];
        frame(&mut app, &mut c, None);
        let band = hit_rect(&app, &Action::PianoSplitter);
        let (grid, panel) = (app.piano_grid.unwrap(), app.piano_panel.unwrap());
        assert_eq!((panel.h, band.y), (177.0, panel.y - 8.0), "the default layout");
        let h = panel.bottom() - grid.y - 8.0;
        app.mouse_move(band.cx(), band.y + 3.0);
        frame(&mut app, &mut c, None);
        assert_eq!(app.cursor(), Cursor::NsResize);

        // Dragging up 100 px gives the panel 100 px more, keeping the grab offset.
        app.mouse_down();
        app.mouse_move(band.cx(), band.y - 97.0);
        app.mouse_up();
        frame(&mut app, &mut c, None);
        assert_eq!(app.piano_panel.unwrap().h, 277.0);
        assert_eq!(app.piano_grid.unwrap().h + app.piano_panel.unwrap().h, h);
        // Down to 30 px of panel: held at 45; past half of that, collapsed.
        let drag_to = |app: &mut App, c: &mut Canvas, panel_h: f32| {
            let b = hit_rect(app, &Action::PianoSplitter);
            app.state.piano.split_click = None;
            app.mouse_move(b.cx(), b.y + 3.0);
            app.mouse_down();
            app.mouse_move(b.cx(), panel.bottom() - panel_h - 8.0 + 3.0);
            app.mouse_up();
            frame(app, c, None);
            app.piano_panel.unwrap().h
        };
        assert_eq!(drag_to(&mut app, &mut c, 30.0), 45.0);
        assert_eq!(drag_to(&mut app, &mut c, 20.0), 0.0);
        assert!(app.state.piano.split.panel_collapsed());
        assert_eq!(hit_rect(&app, &Action::PianoSplitter).bottom(), panel.bottom(), "the band at the bottom");
        // Open again at 200 px, then a double click collapses it and another brings back 200 px.
        assert_eq!(drag_to(&mut app, &mut c, 200.0), 200.0);
        let b = hit_rect(&app, &Action::PianoSplitter);
        app.state.piano.split_click = None;
        app.mouse_move(b.cx(), b.y + 3.0);
        for _ in 0..2 {
            app.mouse_down();
            app.mouse_up();
        }
        frame(&mut app, &mut c, None);
        assert_eq!(app.piano_panel.unwrap().h, 0.0);
        let b = hit_rect(&app, &Action::PianoSplitter);
        app.mouse_move(b.cx(), b.y + 3.0);
        for _ in 0..2 {
            app.mouse_down();
            app.mouse_up();
        }
        frame(&mut app, &mut c, None);
        assert_eq!(app.piano_panel.unwrap().h, 200.0);
        // Up to under the ruler: the grid collapses and the panel takes the whole height.
        assert_eq!(drag_to(&mut app, &mut c, h - 10.0), h);
        assert_eq!(app.piano_grid.unwrap().h, 0.0);
    }

    /// Event panel: the Repeat lane and the channel control lanes (spec: ui/piano-roll.md §7a, §7b).
    #[test]
    fn event_panel_repeat_and_channel_lanes() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.z = vec![Win::PianoRoll];
        app.state.selected_channel = 0;
        frame(&mut app, &mut c, None);
        let ch = piano::channel(&app.state).unwrap();
        let g = app.piano_geom().unwrap();
        let panel = app.piano_panel.unwrap();

        // A note at tick 0, then the Repeat lane: pressing at its start at 6/14 of the height sets repeat 6.
        app.mouse_move(g.x_of(0.0) + 2.0, g.y_of(60) + 8.0);
        app.mouse_down();
        app.mouse_up();
        app.state.piano.lane = piano::Lane::Repeat;
        frame(&mut app, &mut c, None);
        app.mouse_move(g.x_of(0.0) + 2.0, panel.bottom() - panel.h * 6.0 / 14.0);
        app.mouse_down();
        app.mouse_up();
        let repeat = piano::notes(&app.state).iter().find(|n| n.channel == ch).map(|n| n.repeat);
        assert_eq!(repeat, Some(6));

        // Channel volume: a drag lays one point per snap step along the cursor path.
        app.state.piano.lane = piano::Lane::ChanVolume;
        frame(&mut app, &mut c, None);
        let (y0, y1) = (panel.bottom() - panel.h * 0.25, panel.bottom() - panel.h * 0.75);
        app.mouse_move(g.x_of(24.0) + 1.0, y0);
        app.mouse_down();
        for i in 1..=8 {
            let k = i as f32 / 8.0;
            app.mouse_move(g.x_of(24.0) + 1.0 + (g.x_of(96.0) - g.x_of(24.0)) * k, y0 + (y1 - y0) * k);
        }
        app.mouse_up();
        frame(&mut app, &mut c, None);
        let pts: Vec<(u32, i32)> = piano::controls(&app.state).iter().filter(|p| p.channel() == ch).map(|p| (p.position, p.value)).collect();
        let at: Vec<u32> = pts.iter().map(|p| p.0).collect();
        assert_eq!(at, vec![24, 48, 72, 96], "{pts:?}");
        assert!(pts.windows(2).all(|w| w[0].1 < w[1].1), "values rise with the cursor: {pts:?}");
        assert!((pts[0].1 - 3200).abs() < 400 && (pts[3].1 - 9600).abs() < 400, "{pts:?}");

        // The same drag in the pan lane adds separate points (other target), right-drag erases only this lane.
        app.state.piano.lane = piano::Lane::ChanPan;
        frame(&mut app, &mut c, None);
        app.mouse_move(g.x_of(48.0) + 1.0, y0);
        app.mouse_down();
        app.mouse_up();
        assert_eq!(piano::controls(&app.state).len(), 5);
        app.state.piano.lane = piano::Lane::ChanVolume;
        frame(&mut app, &mut c, None);
        app.mouse_move(g.x_of(48.0) + 1.0, y0);
        app.mouse_down_right();
        app.mouse_move(g.x_of(72.0) + 1.0, y0);
        app.mouse_up();
        let left: Vec<(u16, u32)> = piano::controls(&app.state).iter().map(|p| (p.param(), p.position)).collect();
        assert_eq!(left.len(), 3, "{left:?}");
        assert!(left.contains(&(os_model::ControlPoint::PAN, 48)), "pan point kept: {left:?}");
        assert!(piano::undo(&mut app.state));
        assert_eq!(piano::controls(&app.state).len(), 5, "undo restores the erased points");
    }

    /// The pitch lane's own scroll bar (spec: ui/piano-roll.md §7b): arrows move the window 455 cents, it stops at
    /// ±4800 cents, the bar and the arrows agree, and points drawn after scrolling take the scrolled value.
    #[test]
    fn pitch_lane_scrolls() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.z = vec![Win::PianoRoll];
        app.state.selected_channel = 0;
        app.state.piano.lane = piano::Lane::ChanPitch;
        frame(&mut app, &mut c, None);
        assert_eq!(app.state.piano.pitch_center, 0);
        for _ in 0..3 {
            app.act(Action::PitchStep(1));
        }
        assert_eq!(app.state.piano.pitch_center, 1365);
        for _ in 0..40 {
            app.act(Action::PitchStep(1));
        }
        assert_eq!(app.state.piano.pitch_center, piano::PITCH_RANGE - 241, "stops at the top of the range");
        for _ in 0..40 {
            app.act(Action::PitchStep(-1));
        }
        assert_eq!(app.state.piano.pitch_center, -(piano::PITCH_RANGE - 241));

        // The bar maps to the same window: the thumb at the top is the top of the range.
        app.state.piano.pitch_center = 0;
        frame(&mut app, &mut c, None);
        assert!(app.bar(Scroll::PitchY).is_some(), "the lane has a scroll bar");
        app.scroll_set(Scroll::PitchY, 0.0);
        assert_eq!(app.state.piano.pitch_center, piano::PITCH_RANGE - 241);
        assert_eq!(app.scroll_get(Scroll::PitchY), 0.0);
        app.state.piano.pitch_center = 1365;

        // A press at the middle of the panel sets a point at about the window's centre value.
        frame(&mut app, &mut c, None);
        let g = app.piano_geom().unwrap();
        let panel = app.piano_panel.unwrap();
        app.mouse_move(g.x_of(48.0) + 1.0, panel.y + panel.h / 2.0);
        app.mouse_down();
        app.mouse_up();
        let v = piano::controls(&app.state).iter().find(|p| p.param() == os_model::ControlPoint::PITCH).map(|p| p.value);
        assert!(v.is_some_and(|v| (v - 1365).abs() <= 2), "{v:?}");
    }

    /// Piano-roll tools driven with the mouse and keys (spec: ui/piano-roll.md §9).
    #[test]
    fn piano_tools() {
        use winit::keyboard::{Key, NamedKey};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.z = vec![Win::PianoRoll];
        app.state.selected_channel = 0;
        frame(&mut app, &mut c, None);
        let ch = piano::channel(&app.state);
        let mine = |app: &App| -> Vec<Note> { piano::notes(&app.state).iter().filter(|n| Some(n.channel) == ch).copied().collect() };
        let g = app.piano_geom().unwrap();
        let row = |k: i32| g.y_of(k) + 8.0;
        let click = |app: &mut App, c: &mut Canvas, x: f32, y: f32| {
            app.mouse_move(x, y);
            app.mouse_down();
            app.mouse_up();
            frame(app, c, None);
        };
        let swipe = |app: &mut App, c: &mut Canvas, (x0, y0): (f32, f32), (x1, y1): (f32, f32), right: bool| {
            app.mouse_move(x0, y0);
            if right { app.mouse_down_right() } else { app.mouse_down() }
            for i in 1..=20 {
                let k = i as f32 / 20.0;
                app.mouse_move(x0 + (x1 - x0) * k, y0 + (y1 - y0) * k);
            }
            app.mouse_up();
            frame(app, c, None);
        };
        let n0 = mine(&app).len();

        // Draw three notes on C5 (48 ticks each, a beat apart).
        for t in [0.0, 96.0, 192.0] {
            click(&mut app, &mut c, g.x_of(t) + 2.0, row(60));
        }
        assert_eq!(mine(&app).len(), n0 + 3);

        // Letters switch tools; Select marquee over the first two notes.
        app.key(&Key::Character("e".into()), false, false);
        assert_eq!(app.state.piano.tool, Tool::Select);
        swipe(&mut app, &mut c, (g.x_of(0.0) + 1.0, row(61)), (g.x_of(120.0), row(59)), false);
        assert_eq!(app.state.piano.selection(app.state.pattern, ch).len(), 2);
        assert!(app.state.piano.overlay.is_none(), "marquee gone after release");

        // Shift+Right moves the selection one step; Ctrl+Up an octave; plain Right scrolls.
        app.shift = true;
        app.key(&Key::Named(NamedKey::ArrowRight), false, false);
        app.shift = false;
        app.key(&Key::Named(NamedKey::ArrowUp), true, false);
        let moved: Vec<(u32, u16)> = mine(&app).iter().map(|n| (n.position, n.key)).collect();
        assert!(moved.contains(&(24, 72)) && moved.contains(&(120, 72)) && moved.contains(&(192, 60)), "{moved:?}");
        app.key(&Key::Named(NamedKey::ArrowRight), false, false);
        assert!(app.state.piano.scroll_x > 0.0);
        app.state.piano.scroll_x = 0.0;
        frame(&mut app, &mut c, None);

        // Right button in Select acts like the left button (no delete).
        let before = mine(&app).len();
        app.mouse_move(g.x_of(200.0), row(60));
        app.mouse_down_right();
        app.mouse_up();
        frame(&mut app, &mut c, None);
        assert_eq!(mine(&app).len(), before);

        // Mute: click toggles; a swipe follows the first note's new state.
        app.key(&Key::Character("t".into()), false, false);
        click(&mut app, &mut c, g.x_of(200.0), row(60));
        assert!(mine(&app).iter().any(|n| n.position == 192 && n.muted()));
        swipe(&mut app, &mut c, (g.x_of(30.0), row(72)), (g.x_of(130.0), row(72)), false);
        assert_eq!(mine(&app).iter().filter(|n| n.key == 72 && n.muted()).count(), 2);

        // Paint: drag along D5 from tick 0 to past 288 -> notes 48 apart, key locked.
        app.key(&Key::Character("b".into()), false, false);
        swipe(&mut app, &mut c, (g.x_of(0.0) + 2.0, row(62)), (g.x_of(300.0), row(66)), false);
        let painted: Vec<u32> = mine(&app).iter().filter(|n| n.key == 62).map(|n| n.position).collect();
        assert_eq!(painted, vec![0, 48, 96, 144, 192, 240, 288]);

        // Slice: a line at tick 24 cuts the D5 note at 0 (48 long) in two.
        app.key(&Key::Character("c".into()), false, false);
        swipe(&mut app, &mut c, (g.x_of(24.0), row(70)), (g.x_of(24.0), row(55)), false);
        let d5: Vec<(u32, u32)> = mine(&app).iter().filter(|n| n.key == 62).map(|n| (n.position, n.length)).take(2).collect();
        assert_eq!(d5, vec![(0, 24), (24, 24)]);

        // Delete: swipe across the D5 row removes every note touched.
        app.key(&Key::Character("d".into()), false, false);
        swipe(&mut app, &mut c, (g.x_of(1.0), row(62)), (g.x_of(330.0), row(62)), false);
        assert_eq!(mine(&app).iter().filter(|n| n.key == 62).count(), 0);

        // Right button in another tool deletes while held.
        app.key(&Key::Character("p".into()), false, false);
        app.mouse_move(g.x_of(200.0), row(60));
        app.mouse_down_right();
        app.mouse_up();
        frame(&mut app, &mut c, None);
        assert!(!mine(&app).iter().any(|n| n.position == 192 && n.key == 60));

        // Ctrl+A, Ctrl+C, Ctrl+V: pasted at the playhead (stopped: tick 0), selected.
        let count = mine(&app).len();
        app.key(&Key::Character("a".into()), true, false);
        app.key(&Key::Character("c".into()), true, false);
        app.key(&Key::Character("v".into()), true, false);
        assert_eq!(mine(&app).len(), count * 2);
        assert_eq!(app.state.piano.selection(app.state.pattern, ch).len(), count);
        // Ctrl+B duplicates right after the selection's end.
        app.key(&Key::Character("b".into()), true, false);
        assert_eq!(mine(&app).len(), count * 3);
        assert_eq!(app.state.piano.tool, Tool::Draw, "Ctrl+letters don't switch tools");

        // Ruler (§10): left-drag moves the play-start marker (snapped to steps); right-drag
        // selects the notes starting in a time range, which is the only shaded selection.
        let ruler = app.hits.iter().find(|h| h.action == Action::PianoRuler).map(|h| h.rect).unwrap();
        let ry = ruler.cy();
        swipe(&mut app, &mut c, (g.x_of(0.0) + 1.0, ry), (g.x_of(190.0), ry), false);
        assert_eq!(app.state.piano.marker, 192);
        assert_eq!(app.paste_tick(), 192, "stopped: paste at the marker");
        swipe(&mut app, &mut c, (g.x_of(0.0) + 1.0, ry), (g.x_of(100.0), ry), true);
        assert_eq!(app.state.piano.range, Some((0, 96)));
        let sel = app.state.piano.selection(app.state.pattern, ch);
        let notes = piano::notes(&app.state);
        assert!(!sel.is_empty() && sel.iter().all(|&i| notes[i].position < 96));
        app.key(&Key::Character("a".into()), true, false);
        assert_eq!(app.state.piano.range, None, "other selections don't shade");
    }

    /// The piano roll title's target channel (spec: ui/piano-target-channel.md): hint, the channel menu and a pick,
    /// the wheel through the channels with notes, right-click for the channel's window.
    /// "Type in value..." (spec: ui/type-in-value.md): the knob menu's item opens the value box at the mouse; typed text
    /// sets the knob on Enter, Esc leaves it; the box is modal; the mode buttons switch and the accepted mode is kept.
    #[test]
    fn type_in_value() {
        use crate::knobs::KnobId;
        use crate::playlist::EditTarget;
        use crate::typein::{Target, TiAct};
        use winit::keyboard::{Key, NamedKey, SmolStr};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        app.state.z = vec![Win::ChannelRack];
        frame(&mut app, &mut c, None);
        let typ = |app: &mut App, text: &str| {
            for ch in text.chars() {
                let k = if ch == ' ' { Key::Named(NamedKey::Space) } else { Key::Character(SmolStr::new(ch.to_string())) };
                app.key(&k, false, false);
            }
        };
        let enter = |app: &mut App| app.key(&Key::Named(NamedKey::Enter), false, false);
        // Right-click the first row's volume knob, pick "Type in value...".
        let open = |app: &mut App, c: &mut Canvas, k: KnobId| {
            let r = hit_rect(app, &Action::Knob(k));
            app.mouse_move(r.cx(), r.cy());
            app.mouse_down_right();
            app.mouse_up();
            let menu = app.state.playlist.popup.clone().expect("knob menu");
            let i = menu.items.iter().position(|i| i.label == "Type in value...").expect("item");
            assert!(menu.items[i].enabled);
            app.mouse_move(r.cx() + 40.0, r.cy() + 300.0);
            app.act(Action::PlPopup(i));
            frame(app, c, None);
        };
        let vol = KnobId::ChanVol(0);
        open(&mut app, &mut c, vol);
        let e = app.state.playlist.edit.clone().expect("value box");
        assert_eq!(e.target, EditTarget::TypeIn(Target::Knob(vol), false));
        assert!(e.sel, "the field opens selected");
        assert_eq!(e.text, crate::typein::text(&app.state, Target::Knob(vol), false));
        assert!(e.text.ends_with("dB"), "{}", e.text);
        // Bottom-right corner 10 px right of / below the mouse (§1).
        let (mx, my) = app.mouse;
        assert_eq!((e.at.0 + crate::typein::W, e.at.1 + crate::typein::H), (mx + 10.0, my + 10.0));
        assert_eq!(crate::typein::caption(&app.state, Target::Knob(vol)), "Value for Channel volume");
        // Typing replaces the selected text; Enter sets -6 dB.
        typ(&mut app, "-6 dB");
        assert_eq!(app.state.playlist.edit.as_ref().unwrap().text, "-6 dB");
        enter(&mut app);
        assert!(app.state.playlist.edit.is_none());
        assert!(app.state.knob_text(vol).contains("-6.0dB"), "{}", app.state.knob_text(vol));
        // Esc leaves the value; unreadable text too (OS).
        frame(&mut app, &mut c, None);
        let before = app.state.knob(vol);
        open(&mut app, &mut c, vol);
        typ(&mut app, "-20");
        app.key(&Key::Named(NamedKey::Escape), false, false);
        assert!(app.state.playlist.edit.is_none());
        assert_eq!(app.state.knob(vol), before);
        frame(&mut app, &mut c, None);
        open(&mut app, &mut c, vol);
        typ(&mut app, "abc");
        enter(&mut app);
        assert_eq!(app.state.knob(vol), before);
        // Modal: a click elsewhere keeps the box; the A button switches to normalized, ✓ accepts and the mode stays.
        frame(&mut app, &mut c, None);
        open(&mut app, &mut c, vol);
        app.mouse_move(5.0, 1000.0);
        app.mouse_down();
        app.mouse_up();
        assert!(app.state.playlist.edit.is_some(), "modal");
        frame(&mut app, &mut c, None);
        let a = hit_rect(&app, &Action::TypeIn(TiAct::Mode(true)));
        click(&mut app, &mut c, a);
        let e = app.state.playlist.edit.clone().unwrap();
        assert_eq!(e.target, EditTarget::TypeIn(Target::Knob(vol), true));
        assert!(e.sel);
        typ(&mut app, "0.5");
        let ok = hit_rect(&app, &Action::TypeIn(TiAct::Accept));
        click(&mut app, &mut c, ok);
        assert!(app.state.playlist.edit.is_none());
        assert!((app.state.knob(vol) - 0.5).abs() < 1e-6);
        assert!(app.state.playlist.typein_norm, "the accepted mode is kept");
        // The pan box then opens in A; switch back to the value mode and type a pan.
        let pan = KnobId::ChanPan(0);
        open(&mut app, &mut c, pan);
        assert_eq!(app.state.playlist.edit.as_ref().unwrap().text, "0.5");
        let v = hit_rect(&app, &Action::TypeIn(TiAct::Mode(false)));
        click(&mut app, &mut c, v);
        assert_eq!(app.state.playlist.edit.as_ref().unwrap().text, "Centered");
        typ(&mut app, "25% left");
        enter(&mut app);
        assert_eq!(app.state.knob_text(pan), "Channel panning: 25% left");
        assert!(!app.state.playlist.typein_norm);
        // ✗ closes.
        frame(&mut app, &mut c, None);
        open(&mut app, &mut c, pan);
        typ(&mut app, "50");
        let x = hit_rect(&app, &Action::TypeIn(TiAct::Close));
        click(&mut app, &mut c, x);
        assert!(app.state.playlist.edit.is_none());
        assert_eq!(app.state.knob_text(pan), "Channel panning: 25% left");
        // Accepting the field as it opened keeps the exact value.
        app.state.set_knob(pan, 0.6234);
        frame(&mut app, &mut c, None);
        open(&mut app, &mut c, pan);
        enter(&mut app);
        assert_eq!(app.state.knob(pan), 0.6234);
    }
    /// "Type in value..." from the toolbar's and the mixer's short menus (spec: ui/type-in-value.md §6, §8): master
    /// pitch's Hz box, a mixer fader in dB, the mixer pan's menu keeps its automation part.
    #[test]
    fn type_in_value_toolbar_and_mixer() {
        use crate::knobs::KnobId;
        use crate::playlist::EditTarget;
        use crate::typein::Target;
        use winit::keyboard::{Key, NamedKey, SmolStr};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        app.state.z = vec![Win::Mixer];
        frame(&mut app, &mut c, None);
        let typ = |app: &mut App, text: &str| {
            for ch in text.chars() {
                let k = if ch == ' ' { Key::Named(NamedKey::Space) } else { Key::Character(SmolStr::new(ch.to_string())) };
                app.key(&k, false, false);
            }
            app.key(&Key::Named(NamedKey::Enter), false, false);
        };
        // Right-click `a`, check the menu's labels, pick "Type in value...".
        let open = |app: &mut App, c: &mut Canvas, a: Action, auto: bool| {
            let r = hit_rect(app, &a);
            app.mouse_move(r.cx(), r.cy());
            app.mouse_down_right();
            app.mouse_up();
            let menu = app.state.playlist.popup.clone().expect("knob menu");
            assert_eq!(menu.items.iter().any(|i| i.label == "Create automation clip"), auto, "{a:?}");
            let i = menu.items.iter().position(|i| i.label == "Type in value...").expect("item");
            assert!(menu.items[i].enabled);
            app.act(Action::PlPopup(i));
            frame(app, c, None);
            app.state.playlist.edit.clone().expect("value box")
        };
        let pitch = Target::Knob(KnobId::MasterPitch);
        let e = open(&mut app, &mut c, Action::Knob(KnobId::MasterPitch), false);
        assert_eq!(e.target, EditTarget::TypeIn(pitch, false));
        assert_eq!(e.text, "440");
        assert_eq!(crate::typein::width(pitch), crate::typein::W_PLAIN);
        assert_eq!(crate::typein::caption(&app.state, pitch), "Master pitch (Hz)");
        typ(&mut app, "466.16");
        assert_eq!(app.state.knob_text(KnobId::MasterPitch), "Master pitch: +100 cents");
        frame(&mut app, &mut c, None);
        open(&mut app, &mut c, Action::Knob(KnobId::MasterVol), false);
        typ(&mut app, "-6");
        assert!(app.state.knob_text(KnobId::MasterVol).contains("-6.0dB"), "{}", app.state.knob_text(KnobId::MasterVol));
        // A mixer fader: dB through the fader law, clamped to its top.
        frame(&mut app, &mut c, None);
        let e = open(&mut app, &mut c, Action::Fader(1), true);
        assert_eq!(e.target, EditTarget::TypeIn(Target::Fader(1), false));
        assert_eq!(e.text, "0.0dB");
        assert_eq!(crate::typein::caption(&app.state, Target::Fader(1)), "Value for Insert 1 - Volume");
        typ(&mut app, "-6");
        let vol = |app: &App| crate::mixer::track(&app.state, 1).unwrap().volume.unwrap();
        assert!(crate::mixer::level_text("Volume", vol(&app)).contains("-6.0dB"));
        frame(&mut app, &mut c, None);
        open(&mut app, &mut c, Action::Fader(1), true);
        typ(&mut app, "10");
        assert_eq!(crate::mixer::level_text("Volume", vol(&app)), "Volume: 125%  5.6dB  1.90");
        // The mixer pan and stereo separation.
        frame(&mut app, &mut c, None);
        open(&mut app, &mut c, Action::Knob(KnobId::MixPan(2)), true);
        typ(&mut app, "40 left");
        assert_eq!(app.state.knob_text(KnobId::MixPan(2)), "Panning: 40% left");
        frame(&mut app, &mut c, None);
        open(&mut app, &mut c, Action::Knob(KnobId::MixSep(2)), false);
        typ(&mut app, "50");
        assert!(app.state.knob_text(KnobId::MixSep(2)).ends_with("50% merged"), "{}", app.state.knob_text(KnobId::MixSep(2)));
    }

    #[test]
    fn piano_target_channel() {
        use crate::playlist::Mark;
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        app.state.z = vec![Win::PianoRoll];
        app.state.selected_channel = 0;
        // Notes on Kick, Clap and Hat (rows 0, 1, 2); Snare and Pluck have none.
        let p = piano::pattern_mut(&mut app.state);
        for ch in 0..3u16 {
            p.notes.push(Note::step(ch, u32::from(ch) * 24));
        }
        frame(&mut app, &mut c, None);
        let b = hit_rect(&app, &Action::PianoTarget);
        let hint = app.hits.iter().rev().find(|h| h.action == Action::PianoTarget).map(|h| h.hint.to_string()).unwrap();
        assert!(hint.starts_with("Target channel"), "{hint}");

        // Click: the menu 4 px left of the name, 11 px below the title's centre (§2).
        click(&mut app, &mut c, b);
        let menu = app.state.playlist.popup.clone().expect("channel menu");
        assert_eq!((menu.x, menu.y), (b.x - 2.0, b.cy() + 11.0));
        let labels: Vec<&str> = menu.items.iter().map(|i| i.label.as_ref()).collect();
        assert_eq!(
            labels,
            ["Current pattern", "Kick", "Clap", "Hat", "Unsorted", "Kick", "Clap", "Hat", "Snare", "Pluck", "", "Auto locate"]
        );
        let ticked: Vec<bool> = menu.items.iter().map(|i| i.mark == Mark::Check(true)).collect();
        assert_eq!(ticked, [false, true, true, true, false, true, true, true, false, false, false, false]);
        assert!(!menu.items[11].enabled, "Auto locate is not available yet");

        // Picking Snare retargets the piano roll; the rack follows.
        let i = menu.items.iter().position(|i| i.label == "Snare").unwrap();
        app.act(Action::PlPopup(i));
        assert_eq!(app.state.selected_channel, 3);
        assert!(app.state.rack.sel.contains(&3) && app.state.rack.sel.len() == 1);
        assert_eq!(app.state.message.as_deref(), Some("Piano roll: Snare"));
        assert!(app.state.playlist.popup.is_none());

        // A filter group gets its own section, before Unsorted.
        app.state.rack.groups.push(crate::rack::Group { name: "Drums".into(), members: [0, 3].into_iter().collect() });
        let menu = crate::piano_tools::target_menu(&app.state, 0.0, 0.0);
        let labels: Vec<&str> = menu.items.iter().map(|i| i.label.as_ref()).collect();
        assert_eq!(labels[4..], ["Drums", "Kick", "Snare", "Unsorted", "Clap", "Hat", "Pluck", "", "Auto locate"]);
        app.state.rack.groups.clear();

        // Wheel (§3): from Snare (no notes) down gives Kick (wrapping), up from Snare gives Hat.
        frame(&mut app, &mut c, None);
        let wheel = |app: &mut App, c: &mut Canvas, dy: f32| {
            let b = hit_rect(app, &Action::PianoTarget);
            app.mouse_move(b.cx(), b.cy());
            app.wheel(0.0, dy, false, false);
            frame(app, c, None);
            app.state.selected_channel
        };
        assert_eq!(wheel(&mut app, &mut c, -1.0), 0);
        assert_eq!(wheel(&mut app, &mut c, -1.0), 1);
        assert_eq!(wheel(&mut app, &mut c, -1.0), 2);
        assert_eq!(wheel(&mut app, &mut c, -1.0), 0, "wraps");
        assert_eq!(wheel(&mut app, &mut c, 1.0), 2, "up goes back");
        assert!(app.state.rack.sel.contains(&2));
        app.state.selected_channel = 3;
        assert_eq!(wheel(&mut app, &mut c, 1.0), 2, "up from a channel without notes");
        // With one channel holding notes, every notch stays on it.
        piano::pattern_mut(&mut app.state).notes.retain(|n| n.channel == 1);
        assert_eq!(wheel(&mut app, &mut c, -1.0), 1);
        assert_eq!(wheel(&mut app, &mut c, -1.0), 1);
        assert_eq!(wheel(&mut app, &mut c, 1.0), 1);

        // Right-click: the channel's settings window (§4).
        let b = hit_rect(&app, &Action::PianoTarget);
        app.mouse_move(b.cx(), b.cy());
        app.mouse_down_right();
        assert_eq!(app.state.pending_open, Some(1), "Clap's sampler window opens");
    }

    /// The browser's title-bar arrows (spec: ui/browser.md §4, "Title-bar icons"): Reread keeps open nodes,
    /// Collapse closes them.
    #[test]
    fn browser_title_icons() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        app.state.browser_tab = 2;
        app.state.browser_open.insert("2:Plugin database/Effects".into());
        frame(&mut app, &mut c, None);
        let hint = |app: &App, a: Action| app.hits.iter().rev().find(|h| h.action == a).map(|h| h.hint.to_string()).unwrap();
        assert_eq!(hint(&app, Action::BrowserReread), "Reread structure (Ctrl+R)");
        assert_eq!(hint(&app, Action::BrowserCollapse), "Collapse structure (Ctrl+Up)");
        let r = hit_rect(&app, &Action::BrowserReread);
        click(&mut app, &mut c, r);
        assert!(app.state.browser_open.contains("2:Plugin database/Effects"), "Reread keeps open nodes");
        let r = hit_rect(&app, &Action::BrowserCollapse);
        click(&mut app, &mut c, r);
        assert!(!app.state.browser_open.iter().any(|k| k.starts_with("2:")), "Collapse closes the tab's nodes");
    }

    /// The piano roll's magnet button (spec: ui/piano-snap.md): its two menus, where clicks, moves, resizes and the
    /// snap's commands land under each kind of setting, Alt, the zoom-dependent Line unit, kept over a reset.
    #[test]
    fn piano_snap() {
        use crate::snap::EditorSnap;
        use crate::piano_tools::NCmd;
        use crate::playlist::{Cmd, Mark, Snap};
        use winit::keyboard::{Key, NamedKey};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        app.state.z = vec![Win::PianoRoll];
        app.state.selected_channel = 0;
        frame(&mut app, &mut c, None);
        let ch = piano::channel(&app.state).unwrap();
        let g = app.piano_geom().unwrap();
        assert_eq!(g.ppt, 1.0, "96 px per beat at PPQ 96: Line is a step");
        let row = |k: i32| g.y_of(k) + 8.0;
        let x_of = |app: &App, t: f32| app.piano_geom().unwrap().x_of(t);
        let mine = |app: &App| -> Vec<(u32, u32, u16)> {
            piano::notes(&app.state).iter().filter(|n| n.channel == ch).map(|n| (n.position, n.length, n.key)).collect()
        };
        let put = |app: &mut App, notes: &[(u32, u32, u16)]| {
            let p = piano::pattern_mut(&mut app.state);
            p.notes.retain(|n| n.channel != ch);
            for &(pos, length, key) in notes {
                p.notes.push(Note { length, key, ..Note::step(ch, pos) });
            }
            p.notes.sort_by_key(|n| n.position);
            app.state.piano.clear_selection();
        };
        let click = |app: &mut App, c: &mut Canvas, x: f32, y: f32| {
            app.mouse_move(x, y);
            app.mouse_down();
            app.mouse_up();
            frame(app, c, None);
        };
        // A left-drag along C5's row between two ticks.
        let drag = |app: &mut App, c: &mut Canvas, t0: f32, t1: f32| {
            frame(app, c, None);
            let (x0, x1, y) = (x_of(app, t0), x_of(app, t1), row(60));
            app.mouse_move(x0, y);
            app.mouse_down();
            for i in 1..=10 {
                app.mouse_move(x0 + (x1 - x0) * i as f32 / 10.0, y);
            }
            app.mouse_up();
            frame(app, c, None);
        };
        // A Draw click at tick `t` on C5's empty row; returns where the new note starts (and removes it).
        let place = |app: &mut App, c: &mut Canvas, t: f32| -> u32 {
            frame(app, c, None);
            let x = x_of(app, t);
            click(app, c, x, row(60));
            let p = piano::pattern_mut(&mut app.state);
            let i = p.notes.iter().position(|n| n.channel == ch && n.key == 60).expect("a note was drawn");
            let at = p.notes.remove(i).position;
            frame(app, c, None);
            at
        };
        let pick = |app: &mut App, p: EditorSnap| crate::playlist::run(app, Cmd::Piano(NCmd::Snap(p)));
        let hint = |app: &App| app.hits.iter().rev().find(|h| h.action == Action::PianoSnapMenu).map(|h| h.hint.to_string()).unwrap();
        let key = |app: &mut App, k: &str, ctrl: bool, shift: bool| {
            app.shift = shift;
            app.key(&Key::Character(k.into()), ctrl, false);
            app.shift = false;
        };
        let nudge = |app: &mut App| {
            app.shift = true;
            app.key(&Key::Named(NamedKey::ArrowRight), false, false);
            app.shift = false;
        };

        // The magnet is live: its hint names the setting; a click opens the Snap menu 11 px left of and below its
        // centre, Main marked (§1, §2, §8).
        let b = hit_rect(&app, &Action::PianoSnapMenu);
        assert_eq!(hint(&app), "Snap to grid: Main (right-click: stock note lengths)");
        click(&mut app, &mut c, b.cx(), b.cy());
        let pop = app.state.playlist.popup.take().expect("the Snap menu opens");
        assert_eq!((pop.x, pop.y), (b.cx() - 11.0, b.cy() + 11.0));
        let marked: Vec<&str> = pop.items.iter().filter(|i| i.mark == Mark::Radio(true)).map(|i| &*i.label).collect();
        assert_eq!(marked, vec!["Main"]);
        assert_eq!(pop.items.iter().filter(|i| !i.sep).count(), 17);
        assert_eq!(pick(&mut app, EditorSnap::Grid(Snap::ThirdBeat)).as_deref(), Some("Piano roll snap: 1/3 beat"));
        frame(&mut app, &mut c, None);
        assert_eq!(hint(&app), "Snap to grid: 1/3 beat (right-click: stock note lengths)");
        assert_eq!((place(&mut app, &mut c, 40.0), place(&mut app, &mut c, 50.0)), (32, 64));

        // Placing (§3, §5): the nearest unit, Cell down, (none) the tick under the pointer, Alt never snaps.
        for (p, t, alt, want) in [
            (EditorSnap::Main, 40.0, false, 48),
            (EditorSnap::Grid(Snap::Line), 30.0, false, 24),
            (EditorSnap::Grid(Snap::Step), 40.0, false, 48),
            (EditorSnap::Grid(Snap::HalfBeat), 40.0, false, 48),
            (EditorSnap::Grid(Snap::Cell), 47.0, false, 24),
            (EditorSnap::Grid(Snap::None), 40.0, false, 40),
            (EditorSnap::Main, 40.0, true, 40),
        ] {
            pick(&mut app, p);
            app.alt = alt;
            assert_eq!(place(&mut app, &mut c, t), want, "{p:?} at {t}, alt {alt}");
            app.alt = false;
        }

        // Line follows the zoom (§4): 1/2 beat at 30 px per beat, a step at 600, no snapping from 768 up.
        pick(&mut app, EditorSnap::Main);
        for (ppb, t, want) in [(30.0, 70.0, 48), (30.0, 80.0, 96), (600.0, 40.0, 48), (800.0, 40.3, 40)] {
            app.state.piano.px_per_beat = ppb;
            frame(&mut app, &mut c, None);
            assert_eq!(place(&mut app, &mut c, t), want, "{ppb} px per beat");
        }
        app.state.piano.px_per_beat = 96.0;
        frame(&mut app, &mut c, None);

        // Events: the edges of a note from 240 to 312 catch clicks within 5 px; Markers the same with a marker.
        put(&mut app, &[(240, 72, 64)]);
        pick(&mut app, EditorSnap::Events);
        for (t, want) in [(244.0, 240), (247.0, 247), (309.0, 312), (318.0, 318)] {
            assert_eq!(place(&mut app, &mut c, t), want, "Events at {t}");
        }
        pick(&mut app, EditorSnap::Markers);
        piano::pattern_mut(&mut app.state).markers = vec![os_model::TimeMarker::new(192, "M")];
        for (t, want) in [(187.0, 192), (195.0, 192), (198.0, 198), (244.0, 244)] {
            assert_eq!(place(&mut app, &mut c, t), want, "Markers at {t}");
        }
        piano::pattern_mut(&mut app.state).markers.clear();

        // Moving snaps the distance, so an off-grid note keeps its offset; a resize snaps the change of length,
        // down to one unit (one tick without a unit); Alt moves freely (§5, §8).
        pick(&mut app, EditorSnap::Main);
        put(&mut app, &[(40, 48, 60)]);
        drag(&mut app, &mut c, 50.0, 80.0);
        assert_eq!(mine(&app), vec![(64, 48, 60)]);
        app.alt = true;
        drag(&mut app, &mut c, 74.0, 104.0);
        app.alt = false;
        assert_eq!(mine(&app), vec![(94, 48, 60)]);
        put(&mut app, &[(40, 48, 60)]);
        drag(&mut app, &mut c, 87.0, 104.0);
        assert_eq!(mine(&app), vec![(40, 72, 60)], "the end dragged +17 ticks lands on 112");
        drag(&mut app, &mut c, 111.0, 20.0);
        assert_eq!(mine(&app), vec![(40, 24, 60)], "a resize keeps one unit");
        pick(&mut app, EditorSnap::Grid(Snap::None));
        drag(&mut app, &mut c, 63.0, 20.0);
        assert_eq!(mine(&app), vec![(40, 1, 60)], "one tick without a unit");
        // Events: the dragged start goes to another note's edge within 5 px.
        pick(&mut app, EditorSnap::Events);
        put(&mut app, &[(40, 48, 60), (240, 72, 64)]);
        drag(&mut app, &mut c, 50.0, 247.0);
        assert!(mine(&app).contains(&(240, 48, 60)), "{:?}", mine(&app));

        // Shift+arrows, Discard lengths and Quick chop use the unit; without one a tick, length 0, one-tick pieces.
        // Quick quantize keeps the global snap (Line) (§6).
        for (p, moved, discarded) in [(EditorSnap::Grid(Snap::None), 101, 0), (EditorSnap::Main, 124, 24), (EditorSnap::Grid(Snap::Beat), 196, 96)] {
            pick(&mut app, p);
            put(&mut app, &[(100, 96, 60)]);
            nudge(&mut app);
            key(&mut app, "d", false, true);
            assert_eq!(mine(&app), vec![(moved, discarded, 60)], "{p:?}");
        }
        pick(&mut app, EditorSnap::Grid(Snap::None));
        put(&mut app, &[(96, 96, 60)]);
        key(&mut app, "u", true, false);
        assert_eq!(mine(&app).len(), 96);
        assert!(mine(&app).iter().all(|n| n.1 == 1));
        pick(&mut app, EditorSnap::Grid(Snap::HalfBeat));
        put(&mut app, &[(96, 96, 60)]);
        key(&mut app, "u", true, false);
        assert_eq!(mine(&app), vec![(96, 48, 60), (144, 48, 60)]);
        // Quick quantize uses the piano roll's snap: nothing changes without a unit (spec: ui/global-snap.md §3).
        pick(&mut app, EditorSnap::Grid(Snap::None));
        put(&mut app, &[(30, 40, 60)]);
        key(&mut app, "q", true, false);
        assert_eq!(mine(&app), vec![(30, 40, 60)]);
        pick(&mut app, EditorSnap::Main);
        key(&mut app, "q", true, false);
        assert_eq!(mine(&app), vec![(24, 48, 60)]);
        pick(&mut app, EditorSnap::Grid(Snap::None));

        // Right-click: Stock note lengths at the mouse; Beat makes the next notes 96 ticks, the snap stays (§7).
        put(&mut app, &[]);
        app.mouse_move(b.cx(), b.cy());
        app.mouse_down_right();
        app.mouse_up();
        let pop = app.state.playlist.popup.take().expect("Stock note lengths opens");
        assert_eq!((pop.x, pop.y), (b.cx(), b.cy()));
        assert!(pop.items[0].head && pop.items[0].label == "Stock note lengths");
        assert!(pop.items.iter().any(|i| i.cmd == Cmd::Piano(NCmd::StockLength(Snap::Beat))));
        let m = crate::playlist::run(&mut app, Cmd::Piano(NCmd::StockLength(Snap::Beat)));
        assert_eq!(m.as_deref(), Some("New notes: Beat"));
        assert_eq!(app.state.piano.snap, EditorSnap::Grid(Snap::None));
        frame(&mut app, &mut c, None);
        assert_eq!(place(&mut app, &mut c, 50.0), 50);
        assert_eq!(app.state.piano.last_len, 96, "the drawn note was a beat long");

        // The setting outlives a project change (§8).
        app.state.piano.reset();
        assert_eq!(app.state.piano.snap, EditorSnap::Grid(Snap::None));
    }

    /// The main toolbar's snap panel (spec: ui/global-snap.md): the combo's menu, right-click and wheel, the magnet
    /// and Backspace, and what follows the global snap: the piano roll's and the playlist's Main, Quick quantize.
    #[test]
    fn global_snap() {
        use crate::piano_tools::NCmd;
        use crate::playlist::{self as pl, Cmd, Mark, Snap};
        use crate::snap::EditorSnap;
        use winit::keyboard::{Key, NamedKey};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        app.state.z = vec![Win::PianoRoll];
        app.state.selected_channel = 0;
        let ch = piano::channel(&app.state).unwrap();
        piano::pattern_mut(&mut app.state).notes.retain(|n| n.channel != ch);
        frame(&mut app, &mut c, None);
        let hint = |app: &App, a: Action| app.hits.iter().rev().find(|h| h.action == a).map(|h| h.hint.to_string()).unwrap();
        let now = |app: &App| app.state.global_snap.now;
        let mine = |app: &App| -> Vec<(u32, u32)> { piano::notes(&app.state).iter().filter(|n| n.channel == ch).map(|n| (n.position, n.length)).collect() };
        let put = |app: &mut App, pos: u32, length: u32| {
            let p = piano::pattern_mut(&mut app.state);
            p.notes.retain(|n| n.channel != ch);
            p.notes.push(Note { length, key: 60, ..Note::step(ch, pos) });
            app.state.piano.clear_selection();
        };
        // A Draw click at tick `t` on C5's empty row; returns where the new note starts (and removes it).
        let place = |app: &mut App, c: &mut Canvas, t: f32| -> u32 {
            frame(app, c, None);
            let g = app.piano_geom().unwrap();
            click(app, c, Rect::new(g.x_of(t), g.y_of(60) + 8.0, 0.0, 0.0));
            frame(app, c, None);
            let p = piano::pattern_mut(&mut app.state);
            let i = p.notes.iter().position(|n| n.channel == ch && n.key == 60).expect("a note was drawn");
            p.notes.remove(i).position
        };
        let backspace = |app: &mut App| app.key(&Key::Named(NamedKey::Backspace), false, false);
        let quantize = |app: &mut App| app.key(&Key::Character("q".into()), true, false);

        // The panel is live: Line by default, the magnet left of the combo, the hints of §6.
        let combo = hit_rect(&app, &Action::GlobalSnap);
        let magnet = hit_rect(&app, &Action::GlobalSnapToggle);
        assert_eq!(now(&app), Snap::Line);
        assert!(magnet.right() <= combo.x && magnet.cy() == combo.cy());
        assert_eq!(hint(&app, Action::GlobalSnap), crate::snap::HINT);
        assert_eq!(hint(&app, Action::GlobalSnapToggle), crate::snap::MAGNET_HINT);

        // A click opens the Snap menu under the combo, Line marked (§2); Beat is picked from it.
        click(&mut app, &mut c, combo);
        let pop = app.state.playlist.popup.take().expect("the Snap menu opens");
        assert_eq!((pop.x, pop.y), (combo.x, combo.bottom()));
        assert_eq!(pop.items.iter().filter(|i| !i.sep).count(), 14);
        assert_eq!((pop.items[0].label.as_ref(), pop.items[0].mark), ("Line", Mark::Radio(true)));
        assert!(pop.items.iter().any(|i| i.label == "Beat" && i.cmd == Cmd::GlobalSnap(Snap::Beat)));
        assert_eq!(pl::run(&mut app, Cmd::GlobalSnap(Snap::Beat)).as_deref(), Some("Main snap: Beat"));

        // The piano roll's Main follows it: Draw clicks at ticks 40 and 60 land on the nearest beat (§3).
        assert_eq!(app.state.piano.snap, EditorSnap::Main);
        assert_eq!((place(&mut app, &mut c, 40.0), place(&mut app, &mut c, 60.0)), (0, 96));

        // Quick quantize uses the piano roll's snap: Main reads the global Beat, Step its own unit (§3).
        put(&mut app, 40, 48);
        quantize(&mut app);
        assert_eq!(mine(&app), vec![(0, 96)]);
        pl::run(&mut app, Cmd::Piano(NCmd::Snap(EditorSnap::Grid(Snap::Step))));
        put(&mut app, 40, 48);
        quantize(&mut app);
        assert_eq!(mine(&app), vec![(48, 48)], "the piano roll's Step, not the global Beat");
        pl::run(&mut app, Cmd::Piano(NCmd::Snap(EditorSnap::Main)));

        // The magnet turns snapping off: Main places at the tick, Quick quantize changes nothing (§1, §6).
        click(&mut app, &mut c, magnet);
        assert_eq!(now(&app), Snap::None);
        assert_eq!(app.state.message.as_deref(), Some("Main snap: (none)"));
        assert_eq!(place(&mut app, &mut c, 43.0), 43);
        put(&mut app, 40, 48);
        quantize(&mut app);
        assert_eq!(mine(&app), vec![(40, 48)]);
        // Backspace with the piano roll focused: back to the last setting; it deletes nothing.
        backspace(&mut app);
        assert_eq!(now(&app), Snap::Beat);
        assert_eq!(mine(&app), vec![(40, 48)]);

        // The wheel over the combo: down to Bar, down again wraps to Line, up goes back to Bar (§1).
        frame(&mut app, &mut c, None);
        app.mouse_move(combo.cx(), combo.cy());
        app.wheel(0.0, -1.0, false, false);
        assert_eq!(now(&app), Snap::Bar);
        app.wheel(0.0, -1.0, false, false);
        assert_eq!(now(&app), Snap::Line);
        app.wheel(0.0, 1.0, false, false);
        assert_eq!((now(&app), app.state.message.as_deref()), (Snap::Bar, Some("Main snap: Bar")));
        // Right-click: Stock note lengths at the mouse; the snap stays (§4).
        app.mouse_down_right();
        app.mouse_up();
        let pop = app.state.playlist.popup.take().expect("Stock note lengths opens");
        assert_eq!((pop.x, pop.y), (combo.cx(), combo.cy()));
        assert!(pop.items[0].head && pop.items[0].label == "Stock note lengths");
        assert_eq!(now(&app), Snap::Bar);

        // The playlist's Main follows it too: clicks at 2.78, 4.67 and 7.62 bars land on the nearest bar, and with
        // the global Beat on the nearest beat (§3, ui/playlist.md §4).
        app.state.z = vec![Win::Playlist];
        app.state.playlist.row_h = pl::ROW_H;
        pl::arr_mut(&mut app.state).clips.clear();
        frame(&mut app, &mut c, None);
        let g = pl::PGeom::new(&app.state, app.pl_grid().expect("playlist grid"));
        let ppq = u32::from(app.state.project.ppq);
        assert_eq!(app.state.playlist.snap, EditorSnap::Main);
        let draw = |app: &mut App, c: &mut Canvas, bars: f32, track: u16| -> u32 {
            click(app, c, Rect::new(g.x_of(bars * (4 * ppq) as f32), g.y_of(track) + 30.0, 0.0, 0.0));
            pl::clips(&app.state).iter().find(|c| c.track == track).expect("a clip was placed").position
        };
        assert_eq!(draw(&mut app, &mut c, 2.78, 0), 3 * 4 * ppq);
        assert_eq!(draw(&mut app, &mut c, 4.67, 1), 5 * 4 * ppq);
        assert_eq!(draw(&mut app, &mut c, 7.62, 2), 8 * 4 * ppq);
        app.state.global_snap.set(Snap::Beat);
        assert_eq!(draw(&mut app, &mut c, 2.78, 3), 11 * ppq);
        // Its own menu: Main first and marked; Events and Markers are listed greyed.
        let m = pl::snap_menu(&app.state, 0.0, 0.0);
        assert_eq!((m.items[0].label.as_ref(), m.items[0].mark), ("Main", Mark::Radio(true)));
        assert!(m.items.iter().filter(|i| i.label == "Events" || i.label == "Markers").all(|i| !i.enabled));
        // Backspace with the playlist focused toggles too.
        backspace(&mut app);
        assert_eq!(now(&app), Snap::None);
        backspace(&mut app);
        assert_eq!(now(&app), Snap::Beat);

        // Program settings: opening another project keeps all three (§6).
        app.state.playlist.snap = EditorSnap::Grid(Snap::Cell);
        app.state.piano.reset();
        app.state.playlist.reset();
        assert_eq!((now(&app), app.state.piano.snap, app.state.playlist.snap), (Snap::Beat, EditorSnap::Main, EditorSnap::Grid(Snap::Cell)));
    }

    /// Piano-roll shortcuts (spec: ui/piano-roll.md §3a, §10a, §11): edits, tool menus, ghosts, markers, channel
    /// cycling and the wheel zooms, driven through `App::key` / `App::wheel` like the shell does.
    #[test]
    fn piano_shortcuts() {
        use crate::piano_tools::{Arp, NCmd, Sub};
        use crate::playlist::Cmd;
        use winit::keyboard::{Key, NamedKey};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        app.state.z = vec![Win::PianoRoll];
        app.state.selected_channel = 0;
        frame(&mut app, &mut c, None);
        let ch = piano::channel(&app.state).unwrap();
        let mine = |app: &App| -> Vec<(u32, u32, u16)> {
            piano::notes(&app.state).iter().filter(|n| n.channel == ch).map(|n| (n.position, n.length, n.key)).collect()
        };
        let put = |app: &mut App, notes: &[(u32, u32, u16)]| {
            let p = piano::pattern_mut(&mut app.state);
            p.notes.retain(|n| n.channel != ch);
            for &(pos, length, key) in notes {
                p.notes.push(Note { length, key, ..Note::step(ch, pos) });
            }
            p.notes.sort_by_key(|n| n.position);
            app.state.piano.clear_selection();
        };
        let key = |app: &mut App, k: &str, ctrl: bool, alt: bool, shift: bool| {
            app.shift = shift;
            app.key(&Key::Character(k.into()), ctrl, alt);
            app.shift = false;
        };

        // Ctrl+Q quantizes starts and lengths, Shift+Q only starts (nothing selected = every note).
        put(&mut app, &[(30, 40, 60)]);
        key(&mut app, "q", true, false, false);
        assert_eq!(mine(&app), vec![(24, 48, 60)]);
        put(&mut app, &[(30, 40, 60)]);
        key(&mut app, "Q", false, false, true);
        assert_eq!(mine(&app), vec![(24, 40, 60)]);
        // Ctrl+Z undoes it.
        key(&mut app, "z", true, false, false);
        assert_eq!(mine(&app), vec![(30, 40, 60)]);

        // Shift+D: one step long; Ctrl+U chops at the steps; Ctrl+G glues them back.
        put(&mut app, &[(0, 96, 60)]);
        key(&mut app, "D", false, false, true);
        assert_eq!(mine(&app), vec![(0, 24, 60)]);
        put(&mut app, &[(0, 72, 60)]);
        key(&mut app, "u", true, false, false);
        assert_eq!(mine(&app), vec![(0, 24, 60), (24, 24, 60), (48, 24, 60)]);
        key(&mut app, "g", true, false, false);
        assert_eq!(mine(&app), vec![(0, 72, 60)]);
        // Ctrl+L: legato.
        put(&mut app, &[(0, 10, 60), (48, 10, 62)]);
        key(&mut app, "l", true, false, false);
        assert_eq!(mine(&app), vec![(0, 48, 60), (48, 10, 62)]);
        // A selection limits the edit and follows the results.
        put(&mut app, &[(0, 72, 60), (96, 72, 62)]);
        let first = piano::notes(&app.state).iter().position(|n| n.channel == ch && n.key == 60).unwrap();
        app.state.piano.select(app.state.pattern, ch, vec![first]);
        key(&mut app, "u", true, false, false);
        assert_eq!(mine(&app), vec![(0, 24, 60), (24, 24, 60), (48, 24, 60), (96, 72, 62)]);
        assert_eq!(app.state.piano.selection(app.state.pattern, Some(ch)).len(), 3);
        // Ctrl+X cuts the selection, Ctrl+D deselects.
        key(&mut app, "x", true, false, false);
        assert_eq!(mine(&app), vec![(96, 72, 62)]);
        assert_eq!(app.state.piano.clipboard.len(), 3);

        // Alt+S / A / R / Q / U open their preset menus; picking an entry runs it.
        put(&mut app, &[(0, 96, 60), (0, 96, 64), (0, 96, 67)]);
        for (k, sub) in [("s", Sub::Strum), ("a", Sub::Arpeggiate), ("r", Sub::Randomize), ("q", Sub::Quantize), ("u", Sub::Chop)] {
            key(&mut app, k, false, true, false);
            let pop = app.state.playlist.popup.take().unwrap_or_else(|| panic!("Alt+{k} opens a menu"));
            assert!(pop.items.iter().all(|i| i.sep || matches!(i.cmd, Cmd::Piano(_))), "{sub:?}");
        }
        crate::playlist::run(&mut app, Cmd::Piano(NCmd::Strum { up: true, n: 64 }));
        assert_eq!(mine(&app).iter().map(|n| n.0).collect::<Vec<_>>(), vec![0, 6, 12]);
        put(&mut app, &[(0, 96, 60), (0, 96, 64), (0, 96, 67)]);
        crate::playlist::run(&mut app, Cmd::Piano(NCmd::Arpeggiate(Arp::Up, 16)));
        assert_eq!(mine(&app).iter().map(|n| n.2).collect::<Vec<_>>(), vec![60, 64, 67, 60]);
        frame(&mut app, &mut c, None);

        // The toolbar's tools button opens the Tools menu.
        let r = hit_rect(&app, &Action::PianoToolsMenu);
        app.mouse_move(r.cx(), r.cy());
        app.mouse_down();
        app.mouse_up();
        assert!(app.state.playlist.popup.take().is_some_and(|p| p.items.iter().any(|i| i.cmd == Cmd::Piano(NCmd::Glue))));

        // Alt+V hides and shows the ghost channels.
        assert!(app.state.piano.ghosts);
        key(&mut app, "v", false, true, false);
        assert!(!app.state.piano.ghosts);
        frame(&mut app, &mut c, None);
        key(&mut app, "v", false, true, false);
        assert!(app.state.piano.ghosts);

        // Alt+T adds a time marker at the play-start marker (stopped), snapped to a step.
        app.state.piano.marker = 200;
        key(&mut app, "t", false, true, false);
        assert_eq!(piano::pattern_mut(&mut app.state).markers, vec![os_model::TimeMarker::new(192, "Marker 1")]);
        // It is saved with the pattern and comes back on open.
        let path = std::env::temp_dir().join(format!("os_piano_marker_{}.flp", std::process::id()));
        app.save_to(path.clone());
        assert!(app.state.message.as_deref().is_some_and(|m| m.starts_with("Saved")), "{:?}", app.state.message);
        app.open(path.clone());
        let _ = std::fs::remove_file(&path);
        app.state.z = vec![Win::PianoRoll];
        app.state.selected_channel = 0;
        assert_eq!(piano::pattern_mut(&mut app.state).markers, vec![os_model::TimeMarker::new(192, "Marker 1")]);
        frame(&mut app, &mut c, None);

        // Ctrl+K cycles to the next channel, wrapping; with the typing keyboard off G / K step too.
        let n = app.state.project.channels.len();
        key(&mut app, "k", true, false, false);
        assert_eq!(app.state.selected_channel, 1);
        if app.typing_on() {
            app.toggle_typing();
        }
        key(&mut app, "g", false, false, false);
        key(&mut app, "g", false, false, false);
        assert_eq!(app.state.selected_channel, n - 1);
        // H / J skip channels without notes in the pattern.
        key(&mut app, "j", false, false, false);
        assert!(piano::notes(&app.state).iter().any(|nt| Some(nt.channel) == piano::channel(&app.state)));
        app.state.selected_channel = 0;
        frame(&mut app, &mut c, None);

        // The wheel over the keyboard scrolls like the grid and Ctrl+wheel there zooms time; Ctrl+middle-drag zooms
        // the keys around the press (up = taller); Alt+wheel over the ruler and PgUp / PgDn zoom time.
        let keys = hit_rect(&app, &Action::PianoKeys);
        app.mouse_move(keys.cx(), keys.cy());
        let sy = app.state.piano.scroll_y;
        app.wheel(0.0, 1.0, false, false);
        app.ease_to_end();
        assert!(app.state.piano.scroll_y < sy, "wheel up over the keys scrolls up");
        assert_eq!(app.state.piano.row_h, views::piano::ROW);
        let z = app.state.piano.px_per_beat;
        app.wheel(0.0, 1.0, false, true);
        assert!(app.state.piano.px_per_beat > z, "Ctrl+wheel over the keys zooms time");
        assert_eq!(app.state.piano.row_h, views::piano::ROW);
        frame(&mut app, &mut c, None);
        for area in [Action::PianoKeys, Action::PianoGrid] {
            let r = hit_rect(&app, &area);
            let (x, y) = (r.cx(), r.cy());
            app.state.piano.row_h = views::piano::ROW;
            app.mouse_move(x, y);
            let k0 = app.piano_geom().unwrap().key_at(y);
            app.ctrl = true;
            app.mouse_down_middle();
            app.ctrl = false;
            app.mouse_move(x, y - 60.0);
            assert!(app.state.piano.row_h > views::piano::ROW, "{area:?}: dragging up makes rows taller");
            assert_eq!(app.piano_geom().unwrap().key_at(y), k0, "{area:?}: the key at the press stays");
            app.mouse_move(x, y + 200.0);
            assert_eq!(app.state.piano.row_h, piano::MIN_ROW, "{area:?}: dragging down shrinks to the minimum");
            app.mouse_up();
            frame(&mut app, &mut c, None);
        }
        // Without Ctrl the middle button still pans.
        app.state.piano.row_h = views::piano::ROW;
        let r = hit_rect(&app, &Action::PianoGrid);
        app.mouse_move(r.cx(), r.cy());
        app.mouse_down_middle();
        app.mouse_move(r.cx(), r.cy() - 40.0);
        app.mouse_up();
        assert_eq!(app.state.piano.row_h, views::piano::ROW);
        frame(&mut app, &mut c, None);
        let z = app.state.piano.px_per_beat;
        let ruler = hit_rect(&app, &Action::PianoRuler);
        app.mouse_move(ruler.cx(), ruler.cy());
        app.alt = true;
        app.wheel(0.0, 1.0, false, false);
        app.alt = false;
        assert!(app.state.piano.px_per_beat > z);
        let z = app.state.piano.px_per_beat;
        app.key(&Key::Named(NamedKey::PageDown), false, false);
        assert!(app.state.piano.px_per_beat < z);
        frame(&mut app, &mut c, None);
    }

    /// Piano roll edits repaint only damaged regions; the result must match a full repaint.
    #[test]
    fn piano_damage_matches_full() {
        let Ok(mut a) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut b = Canvas::new(1920, 1080, 1.0).unwrap();
        let mut app = App::new(None);
        app.state.z = vec![Win::PianoRoll, Win::ChannelRack];
        app.state.wins[2].maxed = true;
        app.state.selected_channel = 4;
        app.state.rack_pos = (900.0, 300.0);
        frame(&mut app, &mut a, None);
        let _ = app.take_damage();
        fn step(app: &mut App, c: &mut Canvas, f: impl FnOnce(&mut App)) {
            f(app);
            let d = app.take_damage();
            frame_damage(app, c, d);
        }
        // Add a note (click), drag it up and right, release; resize it; draw a velocity; erase it.
        step(&mut app, &mut a, |ap| { ap.mouse_move(500.0, 500.0); });
        step(&mut app, &mut a, |ap| ap.mouse_down());
        let key0 = piano::notes(&app.state)[0].key;
        for (x, y) in [(530.0, 490.0), (600.0, 450.0), (650.0, 430.0)] {
            step(&mut app, &mut a, move |ap| { ap.mouse_move(x, y); });
        }
        // The dragged note's key is lit on the keyboard while held.
        assert_eq!(app.state.piano.held, Some(i32::from(key0) + 4));
        // It sounds at that key until the button is released, not for a fixed preview length.
        assert_eq!(app.mouse_note.map(|n| n.1), Some(key0 + 4));
        // (The overview mini-map refreshes on release, so compare the keyboard column only.)
        frame(&mut app, &mut b, None);
        let g = app.piano_geom().unwrap();
        let keys = Rect::new(g.grid.x - 73.0, g.grid.y, 73.0, g.grid.h);
        let differs = |x: usize, y: usize| {
            let i = (y * 1920 + x) * 4;
            (0..4).any(|c| a.pixmap.data()[i + c].abs_diff(b.pixmap.data()[i + c]) > 3)
        };
        let bad = (keys.y as usize..keys.bottom() as usize).flat_map(|y| (keys.x as usize..keys.right() as usize).map(move |x| (x, y))).filter(|&(x, y)| differs(x, y)).count();
        assert_eq!(bad, 0, "keyboard while dragging a note (lit key)");
        step(&mut app, &mut a, |ap| ap.mouse_up());
        assert_eq!(app.state.piano.held, None);
        assert_eq!(app.mouse_note, None, "the note stops with the button");
        let n = piano::notes(&app.state)[0];
        assert!(n.position > 0 && n.key == key0 + 4, "note moved right and 4 keys up: {n:?}");
        let r = app.piano_geom().unwrap().note_rect(&n).unwrap();
        step(&mut app, &mut a, |ap| { ap.mouse_move(r.right() - 1.0, r.cy()); });
        step(&mut app, &mut a, |ap| ap.mouse_down());
        step(&mut app, &mut a, |ap| { ap.mouse_move(r.right() + 60.0, r.cy()); });
        step(&mut app, &mut a, |ap| ap.mouse_up());
        assert!(piano::notes(&app.state)[0].length > n.length, "note resized");
        let panel = app.piano_panel.unwrap();
        step(&mut app, &mut a, |ap| { ap.mouse_move(r.x + 1.0, panel.y + 20.0); });
        step(&mut app, &mut a, |ap| ap.mouse_down());
        step(&mut app, &mut a, |ap| { ap.mouse_move(r.x + 1.0, panel.bottom() - 40.0); });
        step(&mut app, &mut a, |ap| ap.mouse_up());
        frame(&mut app, &mut b, None);
        assert_same(&a, &b, "after move/resize/velocity");
        step(&mut app, &mut a, |ap| { ap.mouse_move(r.x + 4.0, r.cy()); });
        step(&mut app, &mut a, |ap| ap.mouse_down_right());
        step(&mut app, &mut a, |ap| ap.mouse_up());
        assert!(piano::notes(&app.state).is_empty(), "right-click erased the note");
        frame(&mut app, &mut b, None);
        assert_same(&a, &b, "after erase");
        // Hold the left button on the keyboard and slide down: each key entered is held (lit).
        let g = app.piano_geom().unwrap();
        let kx = g.grid.x - 30.0;
        step(&mut app, &mut a, |ap| { ap.mouse_move(kx, g.y_of(72) + 8.0); });
        step(&mut app, &mut a, |ap| ap.mouse_down());
        assert_eq!(app.state.piano.held, Some(72));
        assert_eq!(app.mouse_note.map(|n| n.1), Some(72));
        for key in [71, 70, 69] {
            step(&mut app, &mut a, |ap| { ap.mouse_move(kx, g.y_of(key) + 8.0); });
            assert_eq!(app.state.piano.held, Some(key));
            assert_eq!(app.mouse_note.map(|n| i32::from(n.1)), Some(key), "only the key under the mouse sounds");
        }
        frame(&mut app, &mut b, None);
        assert_same(&a, &b, "keyboard drag");
        step(&mut app, &mut a, |ap| ap.mouse_up());
        assert_eq!(app.state.piano.held, None);
        assert_eq!(app.mouse_note, None, "the key stops with the button");
        frame(&mut app, &mut b, None);
        assert_same(&a, &b, "keyboard released");
    }

    fn ruler_app(c: &mut Canvas) -> (App, Rect, crate::playlist::PGeom, u32) {
        let mut app = App::new(None);
        app.state.project = test_project();
        app.state.playlist.reset();
        app.state.z = vec![Win::Playlist];
        frame(&mut app, c, None);
        let g = crate::playlist::PGeom::new(&app.state, app.pl_grid().expect("playlist grid"));
        let ruler = hit_rect(&app, &Action::PlRuler);
        let bar = 4 * u32::from(app.state.project.ppq);
        (app, ruler, g, bar)
    }

    fn ruler_press(app: &mut App, c: &mut Canvas, x0: f32, x1: f32, y: f32, right: bool) {
        app.mouse_move(x0, y);
        if right { app.mouse_down_right() } else { app.mouse_down() }
        app.mouse_move(x1, y);
        app.mouse_up();
        frame(app, c, None);
    }

    /// Moving the song position in the playlist ruler switches to SONG mode without starting playback; a time
    /// selection (right-drag) keeps PAT (spec: ui/playlist.md §7, as FL does).
    #[test]
    fn ruler_click_switches_to_song_mode() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let (mut app, ruler, g, bar) = ruler_app(&mut c);
        assert!(!app.state.song_mode);
        ruler_press(&mut app, &mut c, g.x_of(bar as f32) + 1.0, g.x_of(3.0 * bar as f32), ruler.cy(), true);
        assert!(app.state.playlist.range.is_some(), "a time selection");
        assert!(!app.state.song_mode, "a time selection keeps PAT");
        // The project's initial values follow the move (spec: ui/settings-choices.md "Advanced: Initialize controls
        // automatically").
        app.state.project.init_controls = vec![os_model::InitControl { head: 0, target: os_model::target::TEMPO, value: 140_000 }];
        app.state.tempo = 90.0;
        ruler_press(&mut app, &mut c, g.x_of(4.0 * bar as f32) + 1.0, g.x_of(4.0 * bar as f32) + 1.0, ruler.cy(), false);
        assert!(app.state.song_mode, "a click on the ruler switches to SONG");
        assert_eq!(app.state.playlist.pos, 4 * bar);
        assert!(!app.transport.playing(), "and does not start playback");
        assert_eq!(app.state.tempo, 140.0, "the initial tempo");
    }

    /// Stretch mode (spec: ui/playlist.md §9b): Shift+M on, then dragging an audio clip's end to twice its length
    /// doubles its channel's resample multiplier and keeps the part of the sample it shows; undo restores both; the
    /// multiplier is saved with a new audio clip channel.
    #[test]
    fn playlist_stretch_mode() {
        use crate::playlist as pl;
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let (mut app, _, _, _) = ruler_app(&mut c);
        app.state.playlist_bar_w = 200.0;
        app.state.playlist.snap = crate::snap::EditorSnap::Grid(pl::Snap::None);
        let wav = std::env::temp_dir().join(format!("os-app-stretch-{}.wav", std::process::id()));
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &[0.5; 96000]).unwrap();
        pl::add_audio_clip(&mut app.state, &wav.to_string_lossy(), 0, 0).expect("audio clip added");
        let ci = pl::clips(&app.state).len() - 1;
        let row = app.state.project.channels.len() - 1;
        let before = pl::clips(&app.state)[ci];
        pl::run(&mut app, pl::Cmd::StretchMode);
        assert!(app.state.playlist.stretch);
        frame(&mut app, &mut c, None);
        let g = pl::PGeom::new(&app.state, app.pl_grid().unwrap());
        let r = g.clip_rect(&before);
        let y = r.y + r.h - 6.0;
        app.mouse_move(r.right() - 1.0, y);
        app.mouse_down();
        app.mouse_move(r.right() - 1.0 + r.w, y);
        app.mouse_up();
        let after = pl::clips(&app.state)[ci];
        // The time becomes the whole sample's length (1 s at the project tempo) times two; the multiplier stays at 1.
        let sound = app.state.project.channels[row].sound;
        let want = app.state.tempo / 60.0 * 2.0;
        let beats = sound.stretch_time().expect("a stretch time");
        assert!((beats - want).abs() < 0.02 * want, "time {beats} beats, want {want}");
        assert_eq!((sound.stretch[2], sound.stretch_mode()), (0, 0), "multiplier 1, method kept");
        assert!((f64::from(after.length) / f64::from(before.length) - 2.0).abs() < 0.02, "twice as long");
        assert_eq!(after.window, before.window, "the same part of the sample");
        app.key(&winit::keyboard::Key::Character("z".into()), true, false);
        assert_eq!(app.state.project.channels[row].sound.stretch, [0; 4], "undo restores the channel");
        // Saved with a new project's audio clip channel.
        app.key(&winit::keyboard::Key::Character("y".into()), true, false);
        app.state.project.channels[row].sound.set_stretch_multiplier(2.0);
        let path = std::env::temp_dir().join(format!("os-app-stretch-{}.flp", std::process::id()));
        app.save_to(path.clone());
        let back = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        let ch = back.channels.iter().find(|c| c.kind == ChannelKind::AudioClip).expect("the audio clip channel");
        assert_eq!(ch.sound.stretch, [0, 0, 10000, 0]);
        // And changed in that existing file (its event 215 patched).
        let mut app2 = App::new(Some(path.clone()));
        let row2 = app2.state.project.channels.iter().position(|c| c.kind == ChannelKind::AudioClip).unwrap();
        app2.state.project.channels[row2].sound.set_stretch_multiplier(0.5);
        app2.save_to(path.clone());
        let back = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        assert_eq!(back.channels[row2].sound.stretch, [0, 0, (-10000i32) as u32, 0]);
        let _ = (std::fs::remove_file(&path), std::fs::remove_file(&wav));
    }

    // spec: ui/settings-choices.md "Display: Colorful waveforms (2026-10-16)"
    /// Colorful waveforms colour a playlist audio clip's waveform at once (a 100 Hz sine: Multiband's orange-red), in
    /// the same shape; back to Off, the clip is drawn exactly as before.
    #[test]
    fn playlist_colorful_waveforms() {
        use crate::playlist as pl;
        use crate::prefs::{set_choice, COLORFUL_WAVEFORMS};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let (mut app, _, _, _) = ruler_app(&mut c);
        app.state.playlist_bar_w = 200.0;
        let wav = std::env::temp_dir().join(format!("os-app-colorful-{}.wav", std::process::id()));
        let sine: Vec<f32> = (0..48000)
            .flat_map(|i| {
                let v = (2.0 * std::f32::consts::PI * 100.0 * i as f32 / 48000.0).sin() * 0.3;
                [v, v]
            })
            .collect();
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &sine).unwrap();
        pl::add_audio_clip(&mut app.state, &wav.to_string_lossy(), 0, 0).expect("audio clip added");
        let clip = *pl::clips(&app.state).last().unwrap();
        let (w, h) = (1920, 1080);
        let shot = |app: &mut App, c: &mut Canvas| {
            frame(app, c, None);
            let g = pl::PGeom::new(&app.state, app.pl_grid().unwrap());
            let body = g.body(&clip);
            let mut px = vec![0u32; w * h];
            c.copy_to_xrgb(&mut px);
            let (x0, x1) = (body.x.max(0.0) as usize, (body.right() as usize).min(w));
            let (y0, y1) = (body.y.max(0.0) as usize, (body.bottom() as usize).min(h));
            (y0..y1).flat_map(|y| px[y * w + x0..y * w + x1].to_vec()).collect::<Vec<u32>>()
        };
        let orange = |px: &[u32]| {
            px.iter()
                .filter(|&&p| {
                    let (r, g, b) = ((p >> 16) & 0xff, (p >> 8) & 0xff, p & 0xff);
                    r > g + 60 && r > b + 100
                })
                .count()
        };
        let off = shot(&mut app, &mut c);
        assert!(!off.is_empty());
        set_choice(&COLORFUL_WAVEFORMS, 2);
        let multiband = shot(&mut app, &mut c);
        assert_eq!(multiband.len(), off.len());
        assert!(orange(&multiband) > orange(&off) + 200, "orange-red: {} before, {} after", orange(&off), orange(&multiband));
        // The same shape: only the waveform's own pixels (a band through the middle, at this level) change colour.
        let changed = off.iter().zip(&multiband).filter(|(a, b)| a != b).count();
        assert!(changed > 200 && changed < off.len() / 2, "{changed} of {} pixels", off.len());
        set_choice(&COLORFUL_WAVEFORMS, 1);
        let map = shot(&mut app, &mut c);
        assert_ne!(map, multiband, "Color map has colours of its own");
        set_choice(&COLORFUL_WAVEFORMS, 0);
        assert!(shot(&mut app, &mut c) == off, "Off: as before");
        let _ = std::fs::remove_file(&wav);
    }

    /// Audio clips follow their channel (spec: ui/playlist.md §9c): the length is worked out from the trims and the
    /// channel's stretch, pitch and the tempo, each trim point rounded on its own. The numbers are FL 26's for a 6.4 s
    /// sample at 130 BPM, PPQ 96: untrimmed, trimmed to 0..3200 ms and to 1600..4800 ms.
    #[test]
    fn audio_clips_follow_their_channel() {
        use crate::playlist as pl;
        use os_model::{Clip, ClipSource};
        let mut app = App::new(None);
        app.state.project = test_project();
        app.state.project.ppq = 96;
        app.state.tempo = 130.0;
        app.state.playlist.reset();
        let wav = std::env::temp_dir().join(format!("os-app-fit-{}.wav", std::process::id()));
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 44100, &vec![0.25; 2 * 282240]).unwrap();
        pl::add_audio_clip(&mut app.state, &wav.to_string_lossy(), 0, 0).expect("audio clip added");
        let row = app.state.project.channels.len() - 1;
        let src = ClipSource::Channel(app.state.project.channels[row].index);
        for (track, w) in [(1, (0.0, 3200.0)), (2, (1600.0, 4800.0))] {
            pl::arr_mut(&mut app.state).clips.push(Clip { window: Some(w), ..Clip::new(0, 1, track, src) });
        }
        let mut check = |app: &mut App, stretch: [u32; 4], pitch: i32, tempo: f64, want: [u32; 3], what: &str| {
            app.state.project.channels[row].sound.stretch = stretch;
            app.state.project.channels[row].pitch_cents = pitch;
            app.state.tempo = tempo;
            pl::fit_audio(&mut app.state);
            let got: Vec<u32> = pl::clips(&app.state).iter().filter(|c| c.source == src).map(|c| c.length).collect();
            assert_eq!(got, want, "{what}");
            // An untrimmed clip shows the whole sample.
            if let Some(whole) = pl::clips(&app.state).iter().find(|c| c.source == src && c.window.is_none()).copied() {
                assert!((pl::window(&app.state, &whole).1 - 6400.0).abs() < 6.0, "{what}: {:?}", pl::window(&app.state, &whole));
            }
        };
        let t = |units: f32| units.to_bits();
        check(&mut app, [0; 4], 0, 130.0, [1331, 666, 665], "plain");
        check(&mut app, [0, 0, 2540, 0], 0, 130.0, [1587, 794, 794], "Resample, MUL 119.25 %");
        check(&mut app, [0, 0, 2540, 0], 10, 130.0, [1578, 789, 789], "and the channel pitch +10 cents");
        check(&mut app, [0, 100, 2540, 5], 0, 130.0, [1587, 794, 794], "Auto, PITCH +100: the length stays");
        check(&mut app, [t(2662.0), 0, 2540, (-1i32) as u32], 10, 130.0, [1587, 794, 793], "Stretch: TIME, no channel pitch");
        check(&mut app, [t(488.0), 0, 0, 0], 0, 130.0, [244, 122, 122], "Resample, TIME 0:10:04");
        check(&mut app, [t(2662.0), 0, 0, (-1i32) as u32], 0, 130.0, [1331, 666, 665], "Stretch, TIME 2662: a point on half a tick rounds up");
        check(&mut app, [0; 4], 0, 522.0, [5345, 2673, 2673], "the tempo without a TIME");
        check(&mut app, [t(2662.0), 0, 0, (-1i32) as u32], 0, 522.0, [1331, 666, 665], "the tempo with a TIME");
        // A clip resized in the playlist keeps its new end (it gets a trim end), refitted alike.
        let i = pl::clips(&app.state).iter().position(|c| c.source == src && c.window.is_none()).unwrap();
        check(&mut app, [0; 4], 0, 130.0, [1331, 666, 665], "plain again");
        let orig = pl::clips(&app.state)[i];
        pl::resize_right(&mut app.state, i, &orig, 1000);
        assert!(!pl::fit_audio(&mut app.state), "a resize already fits");
        check(&mut app, [0, 0, 10000, 0], 0, 130.0, [2000, 1331, 1331], "x2 after the resize");
        // The tick fits them. A fit alone is not an unsaved change (a project opens with other lengths); an edit is.
        for c in &mut pl::arr_mut(&mut app.state).clips {
            c.length = 100;
        }
        app.mark_saved();
        app.tick();
        let got: Vec<u32> = pl::clips(&app.state).iter().filter(|c| c.source == src).map(|c| c.length).collect();
        assert_eq!(got, [2000, 1331, 1331], "fitted by the tick");
        assert!(!app.unsaved(), "a fit is not an edit");
        app.state.project.channels[row].sound.stretch = [0; 4];
        app.tick();
        assert!(app.unsaved(), "a stretch edit is");
        let _ = std::fs::remove_file(&wav);
    }

    /// Tools menu (spec: ui/playlist.md §10): quick quantize moves clip starts onto the snap grid (one undo step);
    /// export renders every playing track to its own file, all as long as the song.
    #[test]
    fn playlist_quantize_and_export_tracks() {
        use crate::playlist as pl;
        use os_model::{Clip, ClipSource};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let (mut app, _, _, bar) = ruler_app(&mut c);
        let ppq = u32::from(app.state.project.ppq);
        app.state.playlist.snap = crate::snap::EditorSnap::Grid(pl::Snap::Step);
        let step = ppq / 4;
        let a = pl::arr_mut(&mut app.state);
        a.clips = vec![Clip::new(step + 5, bar, 0, ClipSource::Pattern(1)), Clip::new(2 * bar - 3, bar, 2, ClipSource::Pattern(1))];
        // Each channel plays a note so the tracks make sound.
        let ch = app.state.project.channels[0].index;
        app.state.project.patterns[0].notes = vec![os_model::Note { length: ppq, ..os_model::Note::step(ch, 0) }];
        let m = pl::run(&mut app, pl::Cmd::QuantizeStarts).unwrap();
        assert!(m.contains("moved 2"), "{m}");
        let starts: Vec<u32> = pl::clips(&app.state).iter().map(|c| c.position).collect();
        assert_eq!(starts, vec![step, 2 * bar], "nearest steps");
        app.key(&winit::keyboard::Key::Character("z".into()), true, false);
        assert_eq!(pl::clips(&app.state)[0].position, step + 5, "one undo step");
        // Export: two files (tracks 1 and 3), the same length, each with sound only where its clip is.
        app.state.path = Some(std::env::temp_dir().join(format!("os-export-{}.flp", std::process::id())));
        let m = pl::run(&mut app, pl::Cmd::ExportTracks).unwrap();
        let dir = std::env::temp_dir().join("Rendered").join(format!("os-export-{} tracks", std::process::id()));
        assert!(m.starts_with("Exported 2 tracks"), "{m}");
        let read = |name: &str| os_engine::sample::load(&dir.join(name)).expect(name);
        let (t1, t3) = (read("01 Track 1.wav"), read("03 Track 3.wav"));
        assert!(t1.frames() >= os_engine::laws::tick_to_frame(f64::from(3 * bar - 3), 96, app.state.tempo, 44100) as usize);
        let loud = |s: &os_engine::sample::Sample, a: usize, b: usize| s.left[a..b.min(s.frames())].iter().any(|v| v.abs() > 0.01);
        let f = |t: u32| os_engine::laws::tick_to_frame(f64::from(t), 96, app.state.tempo, 44100) as usize;
        assert!(loud(&t1, f(step), f(step + ppq)) && !loud(&t3, f(step), f(step + ppq)), "track 1's clip only in its file");
        assert!(loud(&t3, f(2 * bar), f(2 * bar + ppq)), "track 3's clip in its file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Arrangements (spec: ui/playlist.md §12): add, rename, clone, merge, delete, switch; saved and read back with
    /// the shown one.
    #[test]
    fn playlist_arrangements() {
        use crate::playlist as pl;
        use os_model::{Clip, ClipSource};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let (mut app, _, _, bar) = ruler_app(&mut c);
        pl::place(&mut app.state, 0, 0);
        let first = app.state.project.arrangement().unwrap().number;
        // The title's arrangement name opens the menu.
        frame(&mut app, &mut c, None);
        let title = hit_rect(&app, &Action::PlArrangement);
        click(&mut app, &mut c, title);
        assert!(app.state.playlist.popup.is_some(), "arrangement menu");
        app.state.playlist.popup = None;
        // Add one, named through the box.
        pl::run(&mut app, pl::Cmd::ArrAdd);
        assert_eq!(app.state.project.arrangements.len(), 2);
        let added = app.state.project.arrangement().unwrap().number;
        assert_ne!(added, first);
        assert!(pl::clips(&app.state).is_empty(), "a new arrangement is empty");
        app.state.playlist.edit.as_mut().unwrap().text = "Verse".into();
        pl::commit_edit(&mut app.state);
        assert_eq!(pl::arrangement_name(&app.state), "Verse");
        app.state.project.arrangement_mut().unwrap().clips.push(Clip::new(bar, bar, 1, ClipSource::Pattern(1)));
        // Clone it, then merge the first arrangement's clips into the clone.
        pl::run(&mut app, pl::Cmd::ArrClone);
        pl::commit_edit(&mut app.state);
        assert_eq!(pl::arrangement_name(&app.state), "Verse #2");
        pl::run(&mut app, pl::Cmd::ArrMerge(first));
        assert_eq!(pl::clips(&app.state).len(), 2, "its own clip and the merged one");
        // Delete the clone: the one before it shows.
        pl::run(&mut app, pl::Cmd::ArrDelete);
        assert_eq!((app.state.project.arrangements.len(), app.state.project.arrangement().unwrap().number), (2, added));
        // Saved and read back with "Verse" shown.
        let path = std::env::temp_dir().join(format!("os-app-arrangements-{}.flp", std::process::id()));
        app.save_to(path.clone());
        let back = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        let names: Vec<_> = back.arrangements.iter().map(|a| a.name.clone().unwrap_or_default()).collect();
        assert_eq!(names, vec!["Arrangement".to_string(), "Verse".to_string()]);
        assert_eq!(back.arrangement().unwrap().name.as_deref(), Some("Verse"));
        // Edits to a file that already has arrangements: add one more, delete the first, save again.
        let mut app2 = App::new(Some(path.clone()));
        pl::run(&mut app2, pl::Cmd::ArrSelect(first));
        pl::run(&mut app2, pl::Cmd::ArrDelete);
        pl::run(&mut app2, pl::Cmd::ArrAdd);
        pl::commit_edit(&mut app2.state);
        app2.save_to(path.clone());
        let back = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        let names: Vec<_> = back.arrangements.iter().map(|a| a.name.clone().unwrap_or_default()).collect();
        assert_eq!(names, vec!["Verse".to_string(), "Arrangement 2".to_string()], "removed and added in an existing file");
        assert!(back.arrangements[0].clips.iter().any(|c| c.position == bar), "the kept arrangement keeps its clips");
        let _ = std::fs::remove_file(&path);
    }

    /// Audio clip handles (spec: ui/playlist.md §9a): dragging the fade triangles sets the fades, the tension ring
    /// bends the curve, the gain half-circle changes the gain; one undo step each.
    #[test]
    fn audio_clip_fade_handles() {
        use crate::playlist as pl;
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let (mut app, _, _, _) = ruler_app(&mut c);
        app.state.playlist_bar_w = 400.0;
        let wav = std::env::temp_dir().join(format!("os-app-clip-fades-{}.wav", std::process::id()));
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &[0.5; 96000]).unwrap();
        pl::add_audio_clip(&mut app.state, &wav.to_string_lossy(), 0, 0).expect("audio clip added");
        let ci = pl::clips(&app.state).len() - 1;
        assert_eq!(pl::clips(&app.state)[ci].gain(), 1.0, "a new audio clip stores gain 1.0");
        frame(&mut app, &mut c, None);
        let g = pl::PGeom::new(&app.state, app.pl_grid().unwrap());
        let body = g.body(&pl::clips(&app.state)[ci]);
        let drag = |app: &mut App, c: &mut Canvas, (x0, y0): (f32, f32), (x1, y1): (f32, f32)| {
            app.mouse_move(x0, y0);
            app.mouse_down();
            for k in 1..=8 {
                let f = k as f32 / 8.0;
                app.mouse_move(x0 + (x1 - x0) * f, y0 + (y1 - y0) * f);
            }
            app.mouse_up();
            frame(app, c, None);
        };
        // Fade-in: the top-left triangle dragged 100 px right.
        drag(&mut app, &mut c, (body.x + 2.0, body.y + 2.0), (body.x + 102.0, body.y + 2.0));
        let clip = pl::clips(&app.state)[ci];
        let ms_per_tick = pl::to_src(&app.state, clip.source, 1.0);
        let ms = |px: f32| f64::from(px / g.ppt) * ms_per_tick;
        let (fin, t) = clip.fade_in().expect("a fade-in");
        // The fade ends under the mouse.
        assert!((f64::from(fin) - ms(102.0)).abs() < 2.0, "fade-in {fin} ms vs {}", ms(102.0));
        assert_eq!(t, 0.0);
        // Fade-out: the top-right triangle dragged 60 px left.
        drag(&mut app, &mut c, (body.right() - 2.0, body.y + 2.0), (body.right() - 62.0, body.y + 2.0));
        let (fout, _) = pl::clips(&app.state)[ci].fade_out().expect("a fade-out");
        assert!((f64::from(fout) - ms(62.0)).abs() < 2.0, "fade-out {fout} ms");
        // Tension: the fade-in's ring dragged up 10 px.
        let (mx, my) = pl::fade_mid(body, body.x, pl::fade_xs(&app.state, &g, &pl::clips(&app.state)[ci]).0, 0.0, false);
        app.mouse_move(mx, my);
        assert_eq!(app.state.playlist.hover_handle, Some(pl::Handle::TensionIn));
        drag(&mut app, &mut c, (mx, my), (mx, my - 10.0));
        let t = pl::clips(&app.state)[ci].fade_in().unwrap().1;
        assert!((t - 2.8).abs() < 0.01, "tension {t}");
        // Gain: the half-circle dragged up 20 px = +3 dB.
        drag(&mut app, &mut c, (body.cx(), body.bottom() - 2.0), (body.cx(), body.bottom() - 22.0));
        let gain = pl::clips(&app.state)[ci].gain();
        assert!((20.0 * gain.log10() - 3.0).abs() < 0.01, "gain {gain}");
        // Each drag is one undo step.
        app.key(&winit::keyboard::Key::Character("z".into()), true, false);
        assert_eq!(pl::clips(&app.state)[ci].gain(), 1.0);
        app.key(&winit::keyboard::Key::Character("z".into()), true, false);
        assert_eq!(pl::clips(&app.state)[ci].fade_in().unwrap().1, 0.0);
        let _ = std::fs::remove_file(&wav);
    }

    /// Track groups (spec: ui/playlist.md §6a): grouped tracks follow their group track; a collapsed group's members
    /// become strips; the group track's arrow expands it again.
    #[test]
    fn playlist_track_groups() {
        use crate::playlist as pl;
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let (mut app, _, _, _) = ruler_app(&mut c);
        pl::run(&mut app, pl::Cmd::TrackGroup(1));
        pl::run(&mut app, pl::Cmd::TrackGroup(2));
        assert!(pl::is_group_track(&app.state, 0));
        assert_eq!((pl::group_track(&app.state, 2), pl::group_track(&app.state, 3)), (Some(0), None));
        assert_eq!(pl::Rows::new(&app.state).top(3), 3.0 * pl::ROW_H);
        // A member collapses its group.
        pl::run(&mut app, pl::Cmd::GroupCollapse(2));
        let rows = pl::Rows::new(&app.state);
        assert!(rows.strip(1) && rows.strip(2) && !rows.strip(0) && !rows.strip(3));
        assert_eq!(rows.top(3), pl::ROW_H + 2.0 * pl::STRIP_H);
        frame(&mut app, &mut c, None);
        let g = pl::PGeom::new(&app.state, app.pl_grid().unwrap());
        assert_eq!(g.track_at(g.y_of(3) + 1.0), 3, "rows below the strips map back to their tracks");
        let arrow = hit_rect(&app, &Action::PlGroupArrow(0));
        click(&mut app, &mut c, arrow);
        assert!(!pl::Rows::new(&app.state).strip(1), "the arrow expands the group");
        // Undo brings the collapse back.
        app.key(&winit::keyboard::Key::Character("z".into()), true, false);
        assert!(pl::Rows::new(&app.state).strip(1));
    }

    /// Double-clicking an audio clip opens its channel's Lemon Sampler and selects the channel in the rack; one
    /// click does not; a pattern clip opens its pattern in the piano roll (spec: ui/playlist.md §9, §2b).
    #[test]
    fn audio_clip_double_click_opens_its_sampler() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let (mut app, ruler, g, bar) = ruler_app(&mut c);
        let wav = std::env::temp_dir().join(format!("os-app-clip-dblclick-{}.wav", std::process::id()));
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &[0.5; 96000]).unwrap();
        crate::playlist::add_audio_clip(&mut app.state, &wav.to_string_lossy(), 1, 0).expect("audio clip added");
        let row = app.state.project.channels.len() - 1;
        app.state.project.arrangements[0].clips.push(os_model::Clip::new(0, bar, 2, os_model::ClipSource::Pattern(1)));
        frame(&mut app, &mut c, None);
        let _ = ruler;
        let tap = |app: &mut App, c: &mut Canvas, track: u16| {
            let (x, y) = (g.x_of(10.0) + 10.0, g.y_of(track) + 30.0);
            app.mouse_move(x, y);
            app.mouse_down();
            app.mouse_up();
            frame(app, c, None);
        };
        tap(&mut app, &mut c, 1);
        assert_eq!(app.state.pending_open, None, "one click only selects");
        tap(&mut app, &mut c, 1);
        assert_eq!(app.state.pending_open, Some(row), "a double click opens the clip's sampler");
        assert_eq!(app.state.selected_channel, row);
        assert_eq!(app.state.rack.sel, std::collections::BTreeSet::from([row]));
        app.state.pending_open = None;
        app.state.pattern = 2;
        tap(&mut app, &mut c, 2);
        tap(&mut app, &mut c, 2);
        assert_eq!(app.state.pending_open, None, "a pattern clip opens no sampler");
        assert!(app.state.shown(Win::PianoRoll) && app.state.pattern == 1, "it opens its pattern in the piano roll");
        let _ = std::fs::remove_file(&wav);
    }

    /// Double-click on audio clips follows General ▸ Input ▸ Manage shortcuts: the row whose modifier is held wins,
    /// Show clip properties does nothing yet, and a row set to "(disabled)" never fires (spec: ui/settings-choices.md
    /// "Input: Manage shortcuts (2026-10-16)").
    #[test]
    fn audio_clip_double_click_follows_manage_shortcuts() {
        use crate::shortcuts::{Form, CLIP_CHANNEL, CLIP_PROPERTIES};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let (mut app, _, g, _) = ruler_app(&mut c);
        let wav = std::env::temp_dir().join(format!("os-app-clip-dblclick-sc-{}.wav", std::process::id()));
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &[0.5; 96000]).unwrap();
        crate::playlist::add_audio_clip(&mut app.state, &wav.to_string_lossy(), 1, 0).expect("audio clip added");
        let row = app.state.project.channels.len() - 1;
        frame(&mut app, &mut c, None);
        let double = |app: &mut App, c: &mut Canvas, alt: bool| {
            let (x, y) = (g.x_of(10.0) + 10.0, g.y_of(1) + 30.0);
            app.alt = alt;
            for _ in 0..2 {
                app.mouse_move(x, y);
                app.mouse_down();
                app.mouse_up();
                frame(app, c, None);
            }
            app.alt = false;
            app.state.pending_open.take()
        };
        // The defaults: Alt is Show clip properties (no window yet), no modifier Show channel settings.
        assert_eq!(double(&mut app, &mut c, true), None, "Show clip properties opens nothing yet");
        assert_eq!(double(&mut app, &mut c, false), Some(row));
        // Show channel settings to Alt (Show clip properties takes "(none)"): only Alt opens the sampler.
        let mut f = Form::applied();
        f.pick(8, 2);
        f.apply();
        assert_eq!((crate::prefs::choice(&CLIP_PROPERTIES), crate::prefs::choice(&CLIP_CHANNEL)), ("(none)", "Alt"));
        assert_eq!(double(&mut app, &mut c, false), None);
        assert_eq!(double(&mut app, &mut c, true), Some(row));
        // Show channel settings "(disabled)": no double-click opens it.
        f.pick(8, 0);
        f.apply();
        assert_eq!(double(&mut app, &mut c, true), None, "(disabled) never fires");
        assert_eq!(double(&mut app, &mut c, false), None);
        let _ = std::fs::remove_file(&wav);
    }

    /// A pattern playing when the ruler is clicked goes on as the song from the clicked position (needs an audio
    /// device).
    #[test]
    #[ignore]
    fn ruler_click_takes_a_playing_pattern_to_the_song() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let (mut app, ruler, g, bar) = ruler_app(&mut c);
        let ch = app.state.project.channels[0].index;
        app.state.project.patterns = vec![Pattern { number: 1, notes: vec![Note { length: 24, key: 60, ..Note::step(ch, 0) }], ..Pattern::default() }];
        app.state.project.arrangements = vec![os_model::Arrangement {
            number: 1,
            clips: vec![os_model::Clip::new(0, 8 * bar, 0, os_model::ClipSource::Pattern(1))],
            ..Default::default()
        }];
        app.act(Action::Play);
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(app.transport.playing() && !app.transport.song());
        ruler_press(&mut app, &mut c, g.x_of(4.0 * bar as f32) + 1.0, g.x_of(4.0 * bar as f32) + 1.0, ruler.cy(), false);
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(app.state.song_mode && app.transport.playing() && app.transport.song(), "the song plays");
        let tick = app.transport.tick();
        assert!(tick >= f64::from(4 * bar) && tick < f64::from(5 * bar), "from the clicked bar: {tick}");
        app.act(Action::Stop);
    }

    /// Playlist (spec: ui/playlist.md): Draw places / moves / resizes clips, undo, Mute tool, right-click delete,
    /// Ctrl+B, time selection + Ctrl+Insert, track LED, automation clip + point, save and reload.
    #[test]
    fn playlist_editing() {
        use crate::playlist::{self as pl, PlTool};
        use winit::keyboard::{Key, NamedKey};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        app.state.file = None;
        app.state.path = None;
        app.state.playlist.reset();
        app.state.z = vec![Win::Playlist];
        frame(&mut app, &mut c, None);
        let grid = app.pl_grid().expect("playlist grid");
        let g = pl::PGeom::new(&app.state, grid);
        let bar = 4 * u32::from(app.state.project.ppq);
        // Clicks land 10 px into a cell: Cell places clips at its start (Line would round to the nearest beat).
        app.state.playlist.snap = crate::snap::EditorSnap::Grid(pl::Snap::Cell);
        let at = |t: u32, track: u16| (g.x_of(t as f32) + 10.0, g.y_of(track) + 30.0);
        let drag = |app: &mut App, c: &mut Canvas, (x0, y0): (f32, f32), (x1, y1): (f32, f32), right: bool| {
            app.mouse_move(x0, y0);
            if right { app.mouse_down_right() } else { app.mouse_down() }
            for i in 1..=10 {
                let k = i as f32 / 10.0;
                app.mouse_move(x0 + (x1 - x0) * k, y0 + (y1 - y0) * k);
            }
            app.mouse_up();
            frame(app, c, None);
        };
        let tap = |app: &mut App, c: &mut Canvas, p: (f32, f32), right: bool| drag(app, c, p, p, right);
        let on_track = |app: &App, t: u16| -> Vec<os_model::Clip> { pl::clips(&app.state).iter().filter(|c| c.track == t).copied().collect() };

        // Draw: a click places the current pattern (one bar when empty).
        tap(&mut app, &mut c, at(0, 0), false);
        tap(&mut app, &mut c, at(2 * bar, 2), false);
        assert_eq!(pl::clips(&app.state).len(), 2);
        let a = on_track(&app, 0)[0];
        assert_eq!((a.position, a.length, a.source), (0, bar, os_model::ClipSource::Pattern(1)));
        // Drag the body moves (snapped); dragging the right edge resizes.
        drag(&mut app, &mut c, at(0, 0), (g.x_of(4.0 * bar as f32) + 10.0, g.y_of(0) + 30.0), false);
        assert_eq!(on_track(&app, 0)[0].position, 4 * bar);
        let b = on_track(&app, 2)[0];
        let end = g.x_of(b.end() as f32) - 2.0;
        drag(&mut app, &mut c, (end, g.y_of(2) + 30.0), (end + g.bar_w, g.y_of(2) + 30.0), false);
        assert_eq!(on_track(&app, 2)[0].length, 2 * bar);
        // Undo restores the length.
        app.key(&Key::Character("z".into()), true, false);
        assert_eq!(on_track(&app, 2)[0].length, bar);
        frame(&mut app, &mut c, None);

        // Mute tool toggles; right-click deletes.
        app.act(Action::PlTool(PlTool::Mute));
        tap(&mut app, &mut c, at(2 * bar, 2), false);
        assert!(on_track(&app, 2)[0].muted);
        app.act(Action::PlTool(PlTool::Draw));
        tap(&mut app, &mut c, at(2 * bar, 2), true);
        assert!(on_track(&app, 2).is_empty());

        // Ctrl+A, Ctrl+B duplicates the selection after itself.
        app.key(&Key::Character("a".into()), true, false);
        app.key(&Key::Character("b".into()), true, false);
        let t0: Vec<u32> = on_track(&app, 0).iter().map(|c| c.position).collect();
        assert_eq!(t0.len(), 2, "{t0:?}");
        assert!(t0.contains(&(5 * bar)), "{t0:?}");
        app.key(&Key::Character("d".into()), true, false);
        frame(&mut app, &mut c, None);

        // Right-drag in the ruler selects time; Ctrl+Insert inserts that much space.
        let ruler = hit_rect(&app, &Action::PlRuler);
        drag(&mut app, &mut c, (g.x_of(0.0) + 1.0, ruler.cy()), (g.x_of(bar as f32), ruler.cy()), true);
        assert_eq!(app.state.playlist.range, Some((0, bar)));
        app.key(&Key::Named(NamedKey::Insert), true, false);
        let mut t0: Vec<u32> = on_track(&app, 0).iter().map(|c| c.position).collect();
        t0.sort_unstable();
        assert_eq!(t0, vec![5 * bar, 6 * bar]);
        frame(&mut app, &mut c, None);

        // The track LED mutes the track.
        let led = hit_rect(&app, &Action::PlLed(0));
        tap(&mut app, &mut c, (led.cx(), led.cy()), false);
        assert!(!pl::track_on(&app.state, 0));

        // An automation clip (tempo), then a left-click on its curve adds a point.
        app.state.playlist.range = None;
        pl::run(&mut app, pl::Cmd::CreateAutomation(os_model::target::TEMPO, "Tempo".into(), 0.5));
        frame(&mut app, &mut c, None);
        let ch = app.state.project.automations.last().unwrap().channel;
        let ac = *pl::clips(&app.state).iter().find(|c| c.source == os_model::ClipSource::Channel(ch)).unwrap();
        let g = pl::PGeom::new(&app.state, app.pl_grid().unwrap());
        let body = g.body(&ac);
        let x = g.x_of((ac.position + ac.length / 4) as f32); // the midpoint is the tension handle
        let n = app.state.project.automation(ch).unwrap().points.len();
        app.mouse_move(x, pl::value_y(body, 0.5));
        app.mouse_down();
        app.mouse_up();
        assert_eq!(app.state.project.automation(ch).unwrap().points.len(), n + 1);

        // Save (new project) and read back.
        let path = std::env::temp_dir().join(format!("openstudio-pl-{}.flp", std::process::id()));
        app.save_to(path.clone());
        let back = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        let arr = back.arrangement().expect("arrangement saved");
        assert_eq!(arr.clips.len(), pl::clips(&app.state).len());
        assert!(!arr.tracks[0].enabled);
        assert_eq!(back.automation(ch).map(|a| a.points.len()), Some(n + 1));
    }

    /// Enable groups (spec: ui/enable-groups.md): the toolbar button, OPTIONS > Enable groups and Shift+Alt+G toggle it
    /// and deselect the clips; while off, a grouped clip selects and moves alone and Shift+G groups only the selection;
    /// the groups themselves are kept.
    #[test]
    fn enable_groups_switch() {
        use crate::playlist as pl;
        use winit::keyboard::Key;
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        app.state.playlist.reset();
        app.state.playlist.row_h = pl::ROW_H;
        app.state.z = vec![Win::Playlist];
        frame(&mut app, &mut c, None);
        let g = pl::PGeom::new(&app.state, app.pl_grid().expect("playlist grid"));
        let bar = 4 * u32::from(app.state.project.ppq);
        // Clicks land 10 px into a cell: Cell places clips at its start (Line would round to the nearest beat).
        app.state.playlist.snap = crate::snap::EditorSnap::Grid(pl::Snap::Cell);
        let at = |t: u32, track: u16| (g.x_of(t as f32) + 10.0, g.y_of(track) + 30.0);
        let drag = |app: &mut App, c: &mut Canvas, (x0, y0): (f32, f32), (x1, y1): (f32, f32)| {
            app.state.playlist.last_clip = None;
            app.mouse_move(x0, y0);
            app.mouse_down();
            for i in 1..=10 {
                let k = i as f32 / 10.0;
                app.mouse_move(x0 + (x1 - x0) * k, y0 + (y1 - y0) * k);
            }
            app.mouse_up();
            frame(app, c, None);
        };
        let pos = |app: &App, t: u16| pl::clips(&app.state).iter().find(|c| c.track == t).map(|c| (c.position, c.group())).unwrap();
        // Clips on tracks 0 and 1 grouped; a loose clip on track 2.
        for t in 0..3 {
            drag(&mut app, &mut c, at(t as u32 * bar, t), at(t as u32 * bar, t));
        }
        app.state.playlist.sel = vec![0, 1];
        app.shift = true;
        app.key(&Key::Character("G".into()), false, false);
        app.shift = false;

        // The toolbar button (lit while on) turns it off and deselects.
        assert!(crate::enable_groups::on());
        let r = hit_rect(&app, &Action::EnableGroups);
        assert_eq!((r.x, r.y), (679.0, 45.0));
        click(&mut app, &mut c, r);
        assert!(!crate::enable_groups::on());
        assert!(app.state.playlist.sel.is_empty(), "toggling deselects, as in FL");
        // Off: the grouped clip moves alone; the group is kept.
        drag(&mut app, &mut c, at(0, 0), at(bar, 0));
        assert_eq!((pos(&app, 0).0, pos(&app, 1).0), (bar, bar), "only the dragged clip moved");
        assert_eq!(app.state.playlist.sel.len(), 1);
        assert!(pos(&app, 0).1 != 0 && pos(&app, 0).1 == pos(&app, 1).1);
        // Off: Shift+G groups just the selection (track 0 and the loose clip), leaving its old mate behind.
        let i0 = pl::clips(&app.state).iter().position(|c| c.track == 0).unwrap();
        let i2 = pl::clips(&app.state).iter().position(|c| c.track == 2).unwrap();
        app.state.playlist.sel = vec![i0, i2];
        app.shift = true;
        app.key(&Key::Character("G".into()), false, false);
        app.shift = false;
        assert_eq!(pos(&app, 0).1, pos(&app, 2).1);
        assert_ne!(pos(&app, 0).1, pos(&app, 1).1);

        // Shift+Alt+G and OPTIONS > Enable groups toggle it too; on again, a click selects the whole group.
        app.shift = true;
        app.key(&Key::Character("G".into()), false, true);
        app.shift = false;
        assert!(crate::enable_groups::on());
        let opt = (0..60).find(|&i| views::menus::item_at(views::menus::OPTIONS_MENU, i).is_some_and(|it| it.label == "Enable groups")).unwrap();
        app.menu_item(views::menus::OPTIONS_MENU, opt);
        assert!(!crate::enable_groups::on());
        app.menu_item(views::menus::OPTIONS_MENU, opt);
        assert!(crate::enable_groups::on());
        drag(&mut app, &mut c, at(2 * bar, 2), at(3 * bar, 2));
        assert_eq!((pos(&app, 0).0, pos(&app, 2).0), (2 * bar, 3 * bar), "the group moved together");
        assert_eq!(app.state.playlist.sel.len(), 2);
    }

    /// VIEW > Close all windows / unfocused / plugin windows and F12, Ctrl+F12, Alt+F12 (spec: ui/close-windows.md):
    /// the browser stays (and the Tempo tapper for Alt+F12); the focused window is the one in front, none after a
    /// click on the empty workspace.
    #[test]
    fn close_all_windows() {
        use winit::keyboard::{Key, NamedKey};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        let all = vec![Win::Playlist, Win::Mixer, Win::Plugin, Win::ChannelRack, Win::Perf, Win::Tapper, Win::PianoRoll];
        let f12 = Key::Named(NamedKey::F12);
        let view = |label: &str| (0..60).find(|&i| views::menus::item_at(views::menus::VIEW, i).is_some_and(|it| it.label == label)).unwrap();
        for label in ["Close all windows", "Close all plugin windows", "Close all unfocused windows"] {
            let it = views::menus::item_at(views::menus::VIEW, view(label)).unwrap();
            assert!(views::menus::works(views::menus::VIEW, it), "{label} is not greyed out");
        }

        // F12 and the menu entry: everything but the browser.
        app.state.z = all.clone();
        app.key(&f12, false, false);
        assert!(app.state.z.is_empty());
        assert!(app.state.show_browser);
        app.state.z = all.clone();
        app.menu_item(views::menus::VIEW, view("Close all windows"));
        assert!(app.state.z.is_empty());

        // Alt+F12: only the channel settings window (and hosted plugin windows).
        app.state.z = all.clone();
        app.key(&f12, false, true);
        assert_eq!(app.state.z, vec![Win::Playlist, Win::Mixer, Win::ChannelRack, Win::Perf, Win::Tapper, Win::PianoRoll], "the Tempo tapper stays too");
        app.state.z = all.clone();
        app.menu_item(views::menus::VIEW, view("Close all plugin windows"));
        assert!(!app.state.shown(Win::Plugin) && app.state.z.len() == all.len() - 1);

        // Ctrl+F12: all but the one in front (the last clicked).
        app.state.z = all.clone();
        app.state.raise(Win::Mixer);
        app.key(&f12, true, false);
        assert_eq!(app.state.z, vec![Win::Mixer]);
        app.state.z = all.clone();
        app.menu_item(views::menus::VIEW, view("Close all unfocused windows"));
        assert_eq!(app.state.z, vec![Win::PianoRoll]);
        // A click on the empty workspace leaves no window focused: Ctrl+F12 closes them all.
        app.state.z = vec![Win::ChannelRack];
        frame(&mut app, &mut c, None);
        let work = app.state.work_rect();
        app.mouse_move(work.right() - 20.0, work.bottom() - 20.0);
        app.mouse_down();
        app.mouse_up();
        assert!(app.state.no_focus);
        app.key(&f12, true, false);
        assert!(app.state.z.is_empty());
        // Opening or clicking a window focuses it again.
        app.state.raise(Win::Playlist);
        app.state.raise(Win::Mixer);
        assert!(!app.state.no_focus);
        app.key(&f12, true, false);
        assert_eq!(app.state.z, vec![Win::Mixer]);
        // Shift+F12 (Align all channel editors) is not available yet: nothing closes.
        app.shift = true;
        app.key(&f12, false, false);
        app.shift = false;
        assert_eq!(app.state.z, vec![Win::Mixer]);
    }

    /// The toolbar's song position slider (spec: ui/song-position-slider.md), with FL's measured numbers: an 8.25-bar
    /// song; a click beside the handle steps 1/88 of it toward the pointer and holding repeats up to the pointer; the
    /// handle drags; right-click jumps; the wheel moves 3/64 (down = later); PAT mode moves through the pattern (the
    /// play-start marker); an empty playlist disables it in SONG mode.
    #[test]
    fn song_position_slider() {
        use crate::song_pos::{handle_x, PANEL};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project.arrangements =
            vec![os_model::Arrangement { number: 1, clips: vec![os_model::Clip::new(0, 3168, 0, os_model::ClipSource::Pattern(1))], ..Default::default() }];
        app.state.song_mode = true;
        app.state.playlist.pos = 0;
        frame(&mut app, &mut c, None);
        assert_eq!(hit_rect(&app, &Action::SongPos), PANEL);
        let y = 62.0;
        let pos = |app: &App| app.state.playlist.pos;

        // A click beside the handle: one step (36 ticks); the release ends it.
        app.mouse_move(480.0, y);
        app.mouse_down();
        app.mouse_up();
        assert_eq!(pos(&app), 36);
        assert_eq!(app.hint, "Song position: 1:02:12");
        // Held: steps every repeat until the handle is under the pointer (1584 ticks), then it stays.
        app.mouse_down();
        assert_eq!(pos(&app), 72);
        let t0 = std::time::Instant::now();
        for i in 1..60 {
            app.song_pos_tick(t0 + std::time::Duration::from_millis(250 * i));
        }
        assert_eq!(pos(&app), 1584, "stopped under the pointer");
        assert!(app.animating(), "the hold ticks at animation rate");
        app.mouse_up();
        assert!(!matches!(app.drag, Some(Drag::SongPos { .. } | Drag::SongPosHold { .. })));
        // The handle drags (keeping the grab offset).
        frame(&mut app, &mut c, None);
        assert_eq!(handle_x(1584, 3168), 480.0);
        app.mouse_move(482.0, y);
        app.mouse_down();
        app.mouse_move(438.0, y);
        assert_eq!(pos(&app), 764);
        app.mouse_up();
        // Right click jumps to the pointer, clamped to the last tick.
        app.mouse_move(565.0, y);
        app.mouse_down_right();
        assert_eq!(pos(&app), 3167);
        // The wheel: up = earlier, down = later, 149 ticks a notch.
        frame(&mut app, &mut c, None);
        app.wheel(0.0, 1.0, false, false);
        assert_eq!(pos(&app), 3018);
        app.wheel(0.0, -2.0, false, false);
        assert_eq!(pos(&app), 3167);

        // PAT mode: through the (empty, one-bar) pattern; right-click at the middle.
        app.state.song_mode = false;
        frame(&mut app, &mut c, None);
        app.mouse_move(480.0, y);
        app.mouse_down_right();
        assert_eq!(app.state.piano.marker, 192);
        assert_eq!(app.hint, "Song position: 1:09:00");

        // SONG mode with an empty playlist: disabled.
        app.state.song_mode = true;
        app.state.project.arrangements[0].clips.clear();
        frame(&mut app, &mut c, None);
        assert!(app.hits.iter().all(|h| h.action != Action::SongPos));
        app.mouse_down_right();
        assert_eq!(pos(&app), 3167);
    }

    /// Playlist navigation and editing shortcuts (spec: ui/playlist.md §3, §3a, §4, §5, §6): Alt+wheel = track
    /// height (FL's 6 px steps and limits), Ctrl+wheel and the plain wheel over the ruler = time zoom, End = zoom on
    /// selection, Shift+G / Alt+G / Alt+U clip groups (lowest free number, move together, duplicates get their own
    /// group, ungroup deselects, saved), Right-Shift+drag slice in Draw, Shift+Tab snap cycle, Ctrl+Insert without a
    /// time selection, Ctrl+P / Ctrl+Shift+P not available yet, F5 raises, the view saved in the project (event 242).
    #[test]
    fn playlist_keybinds() {
        use crate::playlist::{self as pl, Snap};
        use winit::keyboard::{Key, NamedKey};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        app.state.file = None;
        app.state.path = None;
        app.state.playlist.reset();
        app.state.playlist.row_h = pl::ROW_H;
        app.state.z = vec![Win::Playlist];
        frame(&mut app, &mut c, None);
        let grid = app.pl_grid().expect("playlist grid");
        let g = pl::PGeom::new(&app.state, grid);
        let bar = 4 * u32::from(app.state.project.ppq);
        // Clicks land 10 px into a cell: Cell places clips at its start (Line would round to the nearest beat).
        app.state.playlist.snap = crate::snap::EditorSnap::Grid(pl::Snap::Cell);
        let at = |t: u32, track: u16| (g.x_of(t as f32) + 10.0, g.y_of(track) + 30.0);
        let drag = |app: &mut App, c: &mut Canvas, (x0, y0): (f32, f32), (x1, y1): (f32, f32)| {
            app.mouse_move(x0, y0);
            app.mouse_down();
            for i in 1..=10 {
                let k = i as f32 / 10.0;
                app.mouse_move(x0 + (x1 - x0) * k, y0 + (y1 - y0) * k);
            }
            app.mouse_up();
            frame(app, c, None);
        };
        let on_track = |app: &App, t: u16| -> Vec<os_model::Clip> {
            let mut v: Vec<os_model::Clip> = pl::clips(&app.state).iter().filter(|c| c.track == t).copied().collect();
            v.sort_by_key(|c| c.position);
            v
        };

        // Shift+Tab cycles Line, Cell, Beat, Bar, none; from Main (the default) it goes to Line.
        app.state.playlist.snap = crate::snap::EditorSnap::Main;
        app.shift = true;
        let mut seen = Vec::new();
        for _ in 0..6 {
            app.key(&Key::Named(NamedKey::Tab), false, false);
            seen.push(app.state.playlist.snap);
        }
        app.shift = false;
        let grid_of = crate::snap::EditorSnap::Grid;
        assert_eq!(seen, vec![grid_of(Snap::Line), grid_of(Snap::Cell), grid_of(Snap::Beat), grid_of(Snap::Bar), grid_of(Snap::None), grid_of(Snap::Line)]);
        app.state.playlist.snap = grid_of(Snap::Cell);

        // Two clips, grouped with Shift+G: they select and move as one.
        drag(&mut app, &mut c, at(0, 0), at(0, 0));
        drag(&mut app, &mut c, at(bar, 1), at(bar, 1));
        assert_eq!(pl::clips(&app.state).len(), 2);
        app.key(&Key::Character("a".into()), true, false);
        app.shift = true;
        app.key(&Key::Character("G".into()), false, false);
        app.shift = false;
        let groups: Vec<u16> = pl::clips(&app.state).iter().map(|c| c.group()).collect();
        assert!(groups[0] != 0 && groups[0] == groups[1], "{groups:?}");
        app.key(&Key::Character("d".into()), true, false);
        drag(&mut app, &mut c, at(0, 0), (g.x_of(2.0 * bar as f32) + 10.0, g.y_of(0) + 30.0));
        assert_eq!((on_track(&app, 0)[0].position, on_track(&app, 1)[0].position), (2 * bar, 3 * bar), "the group moved together");
        assert_eq!(app.state.playlist.sel.len(), 2, "clicking a grouped clip selects its group");
        // A duplicate is a group of its own; the copy keeps the shape.
        app.key(&Key::Character("b".into()), true, false);
        let (orig, copy) = (on_track(&app, 0)[0], on_track(&app, 0)[1]);
        assert_eq!((orig.group(), copy.group(), on_track(&app, 1)[1].group()), (1, 2, 2), "the copy takes the lowest free number");
        // The group survives saving.
        let path = std::env::temp_dir().join(format!("openstudio-pl-groups-{}.flp", std::process::id()));
        app.save_to(path.clone());
        let back = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        let saved: Vec<u16> = back.arrangement().unwrap().clips.iter().map(|c| c.group()).collect();
        assert_eq!(saved.iter().filter(|&&g| g == orig.group()).count(), 2, "{saved:?}");
        // Alt+G and Alt+U take groups apart.
        app.key(&Key::Character("d".into()), true, false);
        app.state.playlist.last_clip = None; // not a double click
        drag(&mut app, &mut c, at(2 * bar, 0), at(2 * bar, 0));
        app.key(&Key::Character("g".into()), false, true);
        assert_eq!(on_track(&app, 0)[0].group(), 0);
        assert_eq!(on_track(&app, 1)[0].group(), 0);
        assert!(app.state.playlist.sel.is_empty(), "ungrouping deselects the clips, as in FL");
        // A new group takes the lowest number no clip uses: group 2 left, group 1 free again.
        app.key(&Key::Character("a".into()), true, false);
        let loose: Vec<usize> = (0..pl::clips(&app.state).len()).filter(|&i| pl::clips(&app.state)[i].group() == 0).collect();
        app.state.playlist.sel = loose;
        app.shift = true;
        app.key(&Key::Character("G".into()), false, false);
        app.shift = false;
        assert_eq!((on_track(&app, 0)[0].group(), on_track(&app, 1)[0].group()), (1, 1));
        app.key(&Key::Character("a".into()), true, false);
        app.key(&Key::Character("u".into()), false, true);
        assert!(pl::clips(&app.state).iter().all(|c| c.group() == 0));
        app.key(&Key::Character("d".into()), true, false);

        // End zooms on the selection (all clips when none is selected).
        app.state.playlist.sel = vec![pl::clips(&app.state).iter().position(|c| c.track == 0 && c.position == 2 * bar).unwrap()];
        app.key(&Key::Named(NamedKey::End), false, false);
        assert!((app.state.playlist_bar_w - 600.0f32.min(grid.w)).abs() < 0.01, "one bar fills the view: {}", app.state.playlist_bar_w);
        assert!((app.state.playlist_scroll.0 - 2.0 * app.state.playlist_bar_w).abs() < 0.01);
        assert_eq!(app.state.playlist.row_h, pl::ROW_MAX, "one selected track fills the view's height (up to the maximum)");
        app.state.playlist.row_h = pl::ROW_H;
        app.state.playlist_bar_w = crate::views::playlist::BAR_W;
        app.state.playlist_scroll = (0.0, 0.0);
        app.state.playlist.sel.clear();
        frame(&mut app, &mut c, None);

        // Right-Shift+drag across clips from empty grid (Draw tool) slices them; no clip is placed. The left Shift
        // does not start a slice (FL places a clip there).
        let g = pl::PGeom::new(&app.state, grid);
        app.state.project.arrangements[0].clips.push(os_model::Clip::new(0, 2 * bar, 4, os_model::ClipSource::Pattern(1)));
        let n = pl::clips(&app.state).len();
        app.shift = true;
        app.mouse_move(g.x_of(bar as f32), g.y_of(3) + 30.0);
        app.mouse_down();
        assert!(!matches!(app.state.playlist.overlay, Some(pl::Overlay::Slice { .. })), "left Shift: no slice");
        app.mouse_up();
        app.key(&Key::Character("z".into()), true, false);
        assert_eq!(pl::clips(&app.state).len(), n);
        app.rshift = true;
        drag(&mut app, &mut c, (g.x_of(bar as f32), g.y_of(3) + 30.0), (g.x_of(bar as f32), g.y_of(5) + 30.0));
        app.shift = false;
        app.rshift = false;
        assert_eq!(pl::clips(&app.state).len(), n + 1);
        assert_eq!(on_track(&app, 4).iter().map(|c| (c.position, c.length)).collect::<Vec<_>>(), vec![(0, bar), (bar, bar)]);

        // Alt+wheel (headers, ruler or grid) changes the track height by 6 px a notch, 16..280 px; Ctrl+wheel over the
        // headers and the plain wheel over the ruler zoom time.
        let header = hit_rect(&app, &Action::PlHeader(2));
        app.mouse_move(header.cx(), header.cy());
        let w0 = app.state.playlist_bar_w;
        app.alt = true;
        app.wheel(0.0, 2.0, false, false);
        let h = app.state.playlist.row_h;
        assert_eq!((h, app.state.playlist_bar_w), (pl::ROW_H + 12.0, w0), "taller tracks, same zoom");
        frame(&mut app, &mut c, None);
        let g = pl::PGeom::new(&app.state, grid);
        assert_eq!(g.rows.top(3), 3.0 * h);
        assert_eq!(g.clip_rect(&on_track(&app, 4)[0]).h, h - 3.0);
        assert!(hit_rect(&app, &Action::PlLed(0)).bottom() <= g.y_of(0) + h, "the LED stays inside its row");
        for _ in 0..60 {
            app.wheel(0.0, -1.0, false, false);
        }
        assert_eq!(app.state.playlist.row_h, pl::ROW_MIN);
        frame(&mut app, &mut c, None);
        let g = pl::PGeom::new(&app.state, grid);
        assert!(hit_rect(&app, &Action::PlLed(0)).bottom() <= g.y_of(0) + pl::ROW_MIN, "the LED fits the smallest row");
        app.state.playlist_scroll = (0.0, 0.0);
        frame(&mut app, &mut c, None);
        let grid_pt = (g.x_of(20.0 * bar as f32), g.y_of(2) + 5.0);
        app.mouse_move(grid_pt.0, grid_pt.1);
        for _ in 0..60 {
            app.wheel(0.0, 1.0, false, false);
        }
        app.alt = false;
        assert_eq!(app.state.playlist.row_h, pl::ROW_MAX, "Alt+wheel over the grid, up to the maximum");
        app.state.playlist.row_h = pl::ROW_H;
        app.state.playlist_scroll = (0.0, 0.0);
        frame(&mut app, &mut c, None);
        let header = hit_rect(&app, &Action::PlHeader(2));
        app.mouse_move(header.cx(), header.cy());
        app.wheel(0.0, 1.0, false, true);
        assert!(app.state.playlist_bar_w > w0 && app.state.playlist.row_h == pl::ROW_H, "Ctrl+wheel over the headers zooms time");
        app.state.playlist_bar_w = w0;
        app.state.playlist_scroll = (0.0, 0.0);
        frame(&mut app, &mut c, None);
        let ruler = hit_rect(&app, &Action::PlRuler);
        app.mouse_move(ruler.cx(), ruler.cy());
        app.wheel(0.0, 1.0, false, false);
        assert!(app.state.playlist_bar_w > w0, "the plain wheel over the ruler zooms in");
        app.alt = true;
        let w1 = app.state.playlist_bar_w;
        app.wheel(0.0, 1.0, false, false);
        app.alt = false;
        assert_eq!((app.state.playlist_bar_w, app.state.playlist.row_h), (w1, pl::ROW_H + 6.0), "Alt+wheel over the ruler: track height");
        app.state.playlist.row_h = pl::ROW_H;
        app.state.playlist_bar_w = w0;
        app.state.playlist_scroll = (0.0, 0.0);

        // Ctrl+Insert without a time selection inserts one bar at the song position.
        app.state.playlist.range = None;
        app.state.playlist.pos = bar;
        let before: Vec<u32> = on_track(&app, 4).iter().map(|c| c.position).collect();
        app.key(&Key::Named(NamedKey::Insert), true, false);
        let after: Vec<u32> = on_track(&app, 4).iter().map(|c| c.position).collect();
        assert_eq!((before, after), (vec![0, bar], vec![0, 2 * bar]));

        // Performance mode is not built yet: Ctrl+P (FL's key) and Ctrl+Shift+P say so.
        app.state.message = None;
        app.key(&Key::Character("p".into()), true, false);
        assert!(app.state.message.as_deref().is_some_and(|m| m.contains("not available yet")), "{:?}", app.state.message);
        app.state.message = None;
        app.shift = true;
        app.key(&Key::Character("P".into()), true, false);
        app.shift = false;
        assert!(app.state.message.as_deref().is_some_and(|m| m.contains("not available yet")), "{:?}", app.state.message);

        // The track height, time zoom and scroll are saved with the project and come back when it opens.
        app.state.playlist.row_h = 60.0;
        app.state.playlist_bar_w = 90.0;
        app.state.playlist_scroll = (180.0, 24.0);
        let path = std::env::temp_dir().join(format!("openstudio-pl-view-{}.flp", std::process::id()));
        app.save_to(path.clone());
        let back = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        let v = back.playlist_view.expect("view saved");
        assert_eq!((v.row_px(), v.height(), v.scroll_y()), (60, 22, 24));
        assert!((v.ticks_per_px() - f64::from(bar) / 90.0).abs() < 1e-9 && (v.left() - 180.0 * f64::from(bar) / 90.0).abs() < 1e-6);
        app.state.playlist.row_h = pl::ROW_H;
        app.state.playlist_bar_w = w0;
        app.state.playlist_scroll = (0.0, 0.0);
        app.open(path.clone());
        let _ = std::fs::remove_file(&path);
        assert_eq!(app.state.playlist.row_h, 60.0);
        assert!((app.state.playlist_bar_w - 90.0).abs() < 1e-3 && (app.state.playlist_scroll.0 - 180.0).abs() < 1e-2);
        assert_eq!(app.state.playlist_scroll.1, 24.0);
        app.state.z = vec![Win::Playlist];

        // F5 brings a covered playlist to the front.
        app.state.z = vec![Win::Playlist, Win::Mixer];
        app.key(&Key::Named(NamedKey::F5), false, false);
        assert_eq!(app.state.z.last(), Some(&Win::Playlist));
        assert!(app.state.shown(Win::Playlist));
    }

    /// Look check for playlist states (markers, time selection, selected / muted clips, automation points,
    /// options menu with a submenu): `OS_SHOT_DIR=dir cargo test -p os-app playlist_shots -- --ignored`.
    #[test]
    #[ignore]
    fn playlist_shots() {
        use crate::playlist as pl;
        let Some(dir) = std::env::var_os("OS_SHOT_DIR") else { return };
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        app.state.playlist.reset();
        app.state.z = vec![Win::Playlist];
        app.state.project.patterns[0].notes = (0..16).map(|i| Note { length: 24, key: 60 + (i % 5) as u16 * 2, ..Note::step(0, i * 48) }).collect();
        let bar = 4 * u32::from(app.state.project.ppq);
        {
            let a = pl::arr_mut(&mut app.state);
            for k in 0..4 {
                a.clips.push(os_model::Clip::new(k * 2 * bar, 2 * bar, 0, os_model::ClipSource::Pattern(1)));
            }
            a.clips[1].muted = true;
            a.markers.push(os_model::TimeMarker::new(2 * bar, "Drop"));
            a.tracks[1].color = Some(os_model::Rgb::new(0x90, 0x58, 0x87));
            a.tracks[1].name = Some("Vocals".into());
        }
        app.state.playlist.sel = vec![2];
        app.state.playlist.range = Some((bar, 3 * bar));
        pl::run(&mut app, pl::Cmd::CreateAutomation(os_model::target::TEMPO, "Tempo".into(), 0.3));
        let ch = app.state.project.automations.last().unwrap().channel;
        pl::add_point(&mut app.state, ch, 6.0, 0.9);
        pl::add_point(&mut app.state, ch, 11.0, 0.1);
        app.state.playlist.range = None;
        frame(&mut app, &mut c, None);
        app.state.playlist.hover_clip = pl::clips(&app.state).iter().position(|c| c.source == os_model::ClipSource::Channel(ch));
        let mut popup = pl::options_menu(&app.state, 220.0, 105.0);
        popup.sub = Some(0);
        app.state.playlist.popup = Some(popup);
        frame(&mut app, &mut c, None);
        let dir = std::path::PathBuf::from(dir);
        c.pixmap.save_png(dir.join("pl_states.png")).unwrap();
        // Track heights (Alt+wheel): tall and the minimum.
        app.state.playlist.popup = None;
        for (h, name) in [(96.0, "pl_rows_tall.png"), (pl::ROW_MIN, "pl_rows_short.png")] {
            app.state.playlist.row_h = h;
            frame(&mut app, &mut c, None);
            c.pixmap.save_png(dir.join(name)).unwrap();
        }
    }

    /// Look check for the piano roll's vertical zoom, time markers, Tools menu and snap menus (spec: ui/piano-roll.md
    /// §3a, §10a, §11; ui/piano-snap.md): `OS_SHOT_DIR=dir cargo test -p os-app piano_shots -- --ignored`.
    #[test]
    #[ignore]
    fn piano_shots() {
        let Some(dir) = std::env::var_os("OS_SHOT_DIR") else { return };
        let dir = std::path::PathBuf::from(dir);
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        app.state.z = vec![Win::PianoRoll];
        app.state.selected_channel = 0;
        app.state.project.patterns[0].notes = (0..16).map(|i| Note { length: 24, key: 60 + (i % 5) as u16 * 2, ..Note::step(0, i * 48) }).collect();
        app.state.project.patterns[0].notes.push(Note { length: 96, key: 55, ..Note::step(1, 96) });
        crate::piano_tools::add_marker(&mut app.state, 384);
        frame(&mut app, &mut c, None);
        let grid = app.piano_grid.unwrap();
        piano::center_keys(&mut app.state, grid.h);
        for (name, row) in [("piano_row18.png", 18.0), ("piano_row30.png", 30.0), ("piano_row8.png", 8.0)] {
            let y = grid.cy();
            piano::set_rows(&mut app.state, grid, y, row);
            app.state.piano.row_h = row;
            frame(&mut app, &mut c, None);
            c.pixmap.save_png(dir.join(name)).unwrap();
        }
        app.state.piano.row_h = views::piano::ROW;
        piano::center_keys(&mut app.state, grid.h);
        let mut popup = crate::piano_tools::tools_menu(&app.state, 240.0, 120.0);
        popup.sub = popup.items.iter().position(|i| i.label == "Strum");
        app.state.playlist.popup = Some(popup);
        frame(&mut app, &mut c, None);
        c.pixmap.save_png(dir.join("piano_tools_menu.png")).unwrap();
        // The magnet's two menus, opened as a click and a right-click open them.
        app.state.playlist.popup = None;
        app.state.piano.snap = crate::snap::EditorSnap::Grid(crate::playlist::Snap::ThirdBeat);
        frame(&mut app, &mut c, None);
        let b = hit_rect(&app, &Action::PianoSnapMenu);
        app.mouse_move(b.cx(), b.cy());
        app.mouse_down();
        app.mouse_up();
        frame(&mut app, &mut c, None);
        c.pixmap.save_png(dir.join("piano_snap_menu.png")).unwrap();
        app.state.playlist.popup = None;
        frame(&mut app, &mut c, None);
        app.mouse_move(b.cx(), b.cy());
        app.mouse_down_right();
        app.mouse_up();
        frame(&mut app, &mut c, None);
        c.pixmap.save_png(dir.join("piano_snap_lengths.png")).unwrap();
        // The title's target channel, hovered with its menu open (ui/piano-target-channel.md).
        app.state.playlist.popup = None;
        frame(&mut app, &mut c, None);
        let b = hit_rect(&app, &Action::PianoTarget);
        click(&mut app, &mut c, b);
        c.pixmap.save_png(dir.join("piano_target_menu.png")).unwrap();
        // The toolbar's snap panel (ui/global-snap.md): its menu open, the longest label, snapping off.
        app.state.playlist.popup = None;
        frame(&mut app, &mut c, None);
        let combo = hit_rect(&app, &Action::GlobalSnap);
        click(&mut app, &mut c, combo);
        c.pixmap.save_png(dir.join("global_snap_menu.png")).unwrap();
        app.state.playlist.popup = None;
        app.state.global_snap.set(crate::playlist::Snap::SixthStep);
        frame(&mut app, &mut c, None);
        c.pixmap.save_png(dir.join("global_snap_sixth.png")).unwrap();
        app.state.global_snap.toggle();
        frame(&mut app, &mut c, None);
        c.pixmap.save_png(dir.join("global_snap_none.png")).unwrap();
    }

    /// Mixer look with routing, a sidechain, slots, meters and the menus: `OS_SHOT_DIR=dir cargo test -p os-app
    /// mixer_shots -- --ignored` writes mx_states.png, mx_menu.png and mx_slot_menu.png.
    #[test]
    #[ignore]
    fn mixer_shots() {
        use crate::mixer as mx;
        let Some(dir) = std::env::var_os("OS_SHOT_DIR") else { return };
        let dir = std::path::PathBuf::from(dir);
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        app.state.z = vec![Win::Mixer];
        app.state.show_browser = false;
        app.plugins.effects = vec![crate::plugin_db::fake("Test Delay", os_host::Kind::Effect)];
        app.state.plugdb.rebuild(&app.plugins);
        mx::select(&mut app.state, 3, false);
        mx::route(&mut app.state, 5, false, false);
        mx::route(&mut app.state, 7, false, true);
        app.state.project.mixer[4].polarity = true;
        app.state.project.mixer[6].swap = true;
        app.state.project.mixer[2].enabled = false;
        app.state.project.mixer[3].name = Some("Drums".into());
        app.state.project.mixer[3].slots[0] = Some(os_model::SlotPlugin { name: "Test Delay".into(), state: Vec::new() });
        app.state.project.mixer[3].slot_on[1] = false;
        mx::select(&mut app.state, 6, true);
        mx::select(&mut app.state, 3, true);
        app.state.mixer.meters = (0..18)
            .map(|i| {
                let db = [crate::meters::to_db(0.12 * i as f32), crate::meters::to_db(0.1 * i as f32)];
                crate::meters::Meter { db, hold: db.map(|d| if d > crate::meters::SILENT { d + 3.0 } else { d }), held: [0.0; 2] }
            })
            .collect();
        frame(&mut app, &mut c, None);
        c.pixmap.save_png(dir.join("mx_states.png")).unwrap();
        app.state.playlist.popup = Some(mx::track_menu(&app.state, 3, 500.0, 200.0));
        frame(&mut app, &mut c, None);
        c.pixmap.save_png(dir.join("mx_menu.png")).unwrap();
        app.state.playlist.popup = Some(mx::slot_menu(&app.state, 0, 1500.0, 150.0));
        frame(&mut app, &mut c, None);
        c.pixmap.save_png(dir.join("mx_slot_menu.png")).unwrap();
    }

    /// F8 and the toolbar's plugin button open the menu of every plugin: an effect goes into the selected mixer track's
    /// first empty slot, a generator to the bottom of the channel rack; F8 again closes the menu (spec:
    /// ui/plugin-menu.md §1).
    #[test]
    fn plugin_menu_adds_plugins_where_they_belong() {
        use crate::playlist::Cmd;
        use crate::plugin_menu::PlugCmd;
        use winit::keyboard::{Key, NamedKey};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        app.plugins = crate::plugins::Plugins::builtins();
        app.plugins.effects.push(crate::plugin_db::fake("Test FX", os_host::Kind::Effect));
        app.state.plugdb.rebuild(&app.plugins);
        frame(&mut app, &mut c, None);
        let f8 = Key::Named(NamedKey::F8);
        let pick = |app: &App, label: &str| app.state.playlist.popup.as_ref().and_then(|p| p.items.iter().find(|i| i.label == label)).map(|i| i.cmd.clone());
        app.key(&f8, false, false);
        {
            let p = app.state.playlist.popup.as_ref().expect("F8 opens the plugin menu");
            assert!(p.items.iter().any(|i| i.head && i.label == "Generators"));
            assert!(p.items.iter().any(|i| i.head && i.label == "Effects"));
        }
        // An effect: the selected track's first empty slot.
        crate::mixer::select(&mut app.state, 2, false);
        app.state.project.mixer[2].slots[0] = Some(os_model::SlotPlugin { name: "Taken".into(), state: Vec::new() });
        let fx = pick(&app, "Test FX").expect("the effects are listed");
        let index = app.plugins.effects.len() - 1;
        assert_eq!(fx, Cmd::Plugin(PlugCmd::Effect { index, slot: None }));
        crate::playlist::run(&mut app, fx);
        assert!(app.state.playlist.popup.is_none());
        assert_eq!(crate::mixer::slot_name(&app.state, 2, 1).as_deref(), Some("Test FX"));
        // F8 twice: open, then closed.
        app.key(&f8, false, false);
        assert!(crate::plugin_menu::is_open(&app.state));
        app.key(&f8, false, false);
        assert!(app.state.playlist.popup.is_none());
        // The toolbar button opens it under the button; a generator goes to the bottom of the rack and is selected.
        frame(&mut app, &mut c, None);
        let b = hit_rect(&app, &Action::PickerOpen);
        click(&mut app, &mut c, b);
        let p = app.state.playlist.popup.as_ref().expect("the toolbar button opens the plugin menu");
        assert!(p.y >= b.bottom(), "under the button");
        let n = app.state.project.channels.len();
        let osc = pick(&app, crate::osc3::LEMON_TRIO).expect("the generators are listed");
        crate::playlist::run(&mut app, osc);
        assert_eq!(app.state.project.channels.len(), n + 1);
        assert_eq!(app.state.selected_channel, n);
        assert_eq!(app.state.pending_plugin, Some((n, app.plugins.index_of(crate::osc3::LEMON_TRIO).unwrap())));
        // An empty mixer slot's menu lists the effects for that slot.
        let m = crate::mixer::effect_menu(&app.state, 4, 0.0, 0.0);
        assert!(m.items.iter().any(|i| i.cmd == Cmd::Plugin(PlugCmd::Effect { index, slot: Some(4) })));
    }

    /// The menu bar dropdown's ▸ entries (spec: ui/menus.md "Look"): a click opens the submenu, the app's popup, to the
    /// right of the row over the dropdown, which stays open; a click on the entry again closes it, one elsewhere closes
    /// both; a pick closes both. TOOLS > Change declicking for selected channels to ▸ sets the mode of every selected
    /// channel, in the channel and its Lemon Sampler, for playback (spec: ui/channel-rack.md §17, audio/declick.md).
    #[test]
    fn dropdown_submenu_changes_declicking_of_selected_channels() {
        use views::menus::{DECLICK_LABEL, TOOLS_MENU};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        app.plugins = crate::plugins::Plugins::default();
        app.plugins.available = crate::plugins::builtin();
        app.sync_samplers(None);
        app.mark_saved();
        assert!(app.state.hosted_rows.contains(&1), "Lemon Sampler plays the sampler channels");
        app.state.rack.sel = [1, 3].into();
        let entry = views::menus::MENUS[TOOLS_MENU].cols.iter().flat_map(|(_, items)| items.iter()).position(|it| it.label == DECLICK_LABEL).unwrap();
        frame(&mut app, &mut c, None);
        let tools = hit_rect(&app, &Action::Menu(TOOLS_MENU));
        click(&mut app, &mut c, tools);
        let row = hit_rect(&app, &Action::MenuSub(TOOLS_MENU, entry));
        click(&mut app, &mut c, row);
        let p = app.state.playlist.popup.clone().expect("the submenu opens");
        assert_eq!((app.state.menu_open, app.state.menu_sub), (Some(TOOLS_MENU), Some(entry)), "over the open dropdown");
        assert_eq!((p.x, p.y), (row.right(), row.y - 2.0), "right of its row");
        assert_eq!(p.items, crate::rack::declick_items(&app.state));
        // The entry again closes the submenu only; a click elsewhere closes both.
        click(&mut app, &mut c, row);
        assert!(app.state.playlist.popup.is_none() && app.state.menu_open == Some(TOOLS_MENU));
        click(&mut app, &mut c, row);
        assert!(app.state.playlist.popup.is_some());
        click(&mut app, &mut c, Rect::new(1500.0, 900.0, 4.0, 4.0));
        assert!(app.state.playlist.popup.is_none() && app.state.menu_open.is_none());
        // Generic (bleeding) = mode 3 on the selected channels only.
        click(&mut app, &mut c, tools);
        click(&mut app, &mut c, row);
        let item = hit_rect(&app, &Action::PlPopup(3));
        assert!(app.top_hit(item.cx(), item.cy()).is_some_and(|h| h.action == Action::PlPopup(3)), "the submenu is over the dropdown");
        app.state.piano.dirty = false;
        click(&mut app, &mut c, item);
        assert!(app.state.playlist.popup.is_none() && app.state.menu_open.is_none(), "a pick closes both");
        let modes: Vec<u8> = app.state.project.channels.iter().map(|ch| ch.sound.declick).collect();
        assert_eq!(modes, [0, 3, 0, 3, 0]);
        assert!(app.unsaved() && app.state.piano.dirty, "a change to the project, played at once");
        // Lemon Sampler plays the new mode (and a save keeps it): the change reaches it on the next UI tick.
        app.plugin_tick();
        let mut ch = app.state.project.channels[1].clone();
        ch.sound.declick = 0;
        crate::plugins::store_sampler(&mut ch, &app.plugins.state(1).unwrap());
        assert_eq!(ch.sound.declick, 3);
        // Without a selection the modes are greyed out and change nothing.
        app.state.rack.sel.clear();
        assert!(crate::rack::declick_items(&app.state).iter().all(|i| !i.enabled));
        click(&mut app, &mut c, tools);
        click(&mut app, &mut c, row);
        let item = hit_rect(&app, &Action::PlPopup(5));
        click(&mut app, &mut c, item);
        assert_eq!(app.state.project.channels[1].sound.declick, 3);
        // The channel rack's options menu has the same submenu.
        app.state.rack.sel = [0].into();
        let rack = crate::rack::options_menu(&app.state, 0.0, 0.0);
        let sub = rack.items.iter().find(|i| i.label == DECLICK_LABEL).expect("in the rack's options menu");
        assert_eq!(sub.children, crate::rack::declick_items(&app.state));
        crate::playlist::run(&mut app, sub.children[4].cmd.clone());
        assert_eq!(app.state.project.channels[0].sound.declick, 4);
    }

    /// TOOLS / the channel rack's options > Switch all plugin editors to ▸ (spec: ui/channel-rack.md §17, ui/plugin-window.md
    /// "Docking"): every hosted instance's window, the channels' and the mixer effects', docked or detached, kept for a
    /// window that is closed.
    #[cfg(plugin_windows)]
    #[test]
    fn switch_all_plugin_editors() {
        use views::menus::{submenu, EDITORS_LABEL, TOOLS_MENU};
        let mut app = App::new(None);
        app.plugins = crate::plugins::Plugins::default();
        app.plugins.available = crate::plugins::builtin();
        app.plugins.effects = crate::plugins::builtin_effects();
        app.sync_samplers(None);
        app.plugins.create_effect(1, 0, 0, None).expect("an effect in a mixer slot");
        assert!(app.plugins.hosted.len() >= 2 && app.plugins.hosted.values().all(|h| !h.detached));
        let items = submenu(TOOLS_MENU, EDITORS_LABEL).unwrap()(&app.state);
        let rack = crate::rack::options_menu(&app.state, 0.0, 0.0);
        assert_eq!(rack.items.iter().find(|i| i.label == EDITORS_LABEL).map(|i| &i.children), Some(&items), "the rack's is the same");
        crate::playlist::run(&mut app, items[1].cmd.clone());
        assert!(app.plugins.hosted.values().all(|h| h.detached), "Detached");
        crate::playlist::run(&mut app, items[0].cmd.clone());
        assert!(app.plugins.hosted.values().all(|h| !h.detached), "Attached");
    }

    /// ADD > Effect ▸: the effect plugin menu as a submenu; a pick goes into the selected mixer track's first empty
    /// slot as from F8, and a full track takes nothing (spec: ui/plugin-menu.md §1).
    #[test]
    fn add_effect_submenu_fills_the_selected_track() {
        use crate::playlist::Cmd;
        use crate::plugin_menu::PlugCmd;
        use views::menus::ADD_MENU;
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        app.plugins = crate::plugins::Plugins::builtins();
        app.plugins.effects.push(crate::plugin_db::fake("Test FX", os_host::Kind::Effect));
        app.state.plugdb.rebuild(&app.plugins);
        crate::mixer::select(&mut app.state, 2, false);
        let index = app.plugins.effects.len() - 1;
        let entry = views::menus::MENUS[ADD_MENU].cols.iter().flat_map(|(_, items)| items.iter()).position(|it| it.label == "Effect").unwrap();
        let pick = |app: &mut App, c: &mut Canvas| {
            frame(app, c, None);
            let add = hit_rect(app, &Action::Menu(ADD_MENU));
            click(app, c, add);
            let row = hit_rect(app, &Action::MenuSub(ADD_MENU, entry));
            click(app, c, row);
            let p = app.state.playlist.popup.clone().expect("the effect menu opens");
            assert!(!p.items.iter().any(|i| i.label == "Generators"), "effects only");
            assert!(p.items.iter().any(|i| i.label == "Manage plugins..."));
            let i = p.items.iter().position(|i| i.cmd == Cmd::Plugin(PlugCmd::Effect { index, slot: None })).expect("Test FX is listed");
            let item = hit_rect(app, &Action::PlPopup(i));
            click(app, c, item);
        };
        app.state.project.mixer[2].slots[0] = Some(os_model::SlotPlugin { name: "Taken".into(), state: Vec::new() });
        pick(&mut app, &mut c);
        assert_eq!(crate::mixer::slot_name(&app.state, 2, 1).as_deref(), Some("Test FX"));
        assert!(app.state.menu_open.is_none() && app.state.playlist.popup.is_none());
        // Every slot in use: nothing is added.
        for slot in &mut app.state.project.mixer[2].slots {
            slot.get_or_insert(os_model::SlotPlugin { name: "Taken".into(), state: Vec::new() });
        }
        let before = app.state.project.mixer[2].slots.clone();
        pick(&mut app, &mut c);
        assert_eq!(app.state.project.mixer[2].slots, before);
        assert_eq!(app.state.message.as_deref(), Some("Every slot of this track is in use"));
    }

    /// The menu bar's ways into the browser (spec: ui/browser-commands.md): VIEW's Browser entries show the browser on
    /// the Current project tab with only their section open, and selected; Plugin database (ADD > Browse plugin
    /// database, Shift+F8) shows that tab as it was left; Browser find (Ctrl+F) focuses the search field as a click
    /// does. Text fields keep their keys; History and Remote control are not in the tree, their entries stay greyed out.
    #[test]
    fn browser_commands_jump_into_the_browser() {
        use crate::views::browser::{project_root, rows, section_key, PLUGIN_TAB, PROJECT_TAB};
        use views::menus::{item_at, works, ADD_MENU, MENUS, TOOLS_MENU, VIEW};
        use winit::keyboard::{Key, NamedKey};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        let index = |m: usize, label: &str| MENUS[m].cols.iter().flat_map(|(_, items)| items.iter()).position(|it| it.label == label).unwrap();
        let (gens, pats, fx) = (section_key("Generators").unwrap(), section_key("Patterns").unwrap(), section_key("Effects").unwrap());
        let s = &mut app.state;
        s.show_browser = false;
        s.browser_tab = 0;
        s.browser_search = "kick".into();
        s.browser_open.insert(gens.clone());
        s.browser_open.remove(&project_root());
        app.menu_item(VIEW, index(VIEW, "Patterns"));
        let s = &app.state;
        assert!(s.show_browser && s.browser_tab == PROJECT_TAB && s.browser_search.is_empty());
        assert_eq!(s.browser_sel.as_ref(), Some(&pats));
        assert!(s.browser_open.contains(&project_root()) && s.browser_open.contains(&pats) && !s.browser_open.contains(&gens));
        app.menu_item(VIEW, index(VIEW, "Effects in use"));
        let open: Vec<(String, Option<bool>)> = rows(&app.state).into_iter().filter(|r| r.depth == 1).map(|r| (r.label, r.open)).collect();
        assert_eq!(open, [("History".into(), Some(false)), ("Patterns".into(), Some(false)), ("Effects".into(), Some(true)), ("Generators".into(), Some(false)), ("Samples".into(), Some(false))]);
        assert_eq!(app.state.browser_sel.as_ref(), Some(&fx));
        app.menu_item(VIEW, index(VIEW, "Generators in use"));
        assert_eq!(app.state.browser_sel.as_ref(), Some(&gens));
        // The Plugin database tab as it was left.
        let db_fx = format!("{PLUGIN_TAB}:Plugin database/Effects");
        app.state.browser_open.insert(db_fx.clone());
        app.state.show_browser = false;
        app.menu_item(VIEW, index(VIEW, "Plugin database"));
        let s = &app.state;
        assert!(s.show_browser && s.browser_tab == PLUGIN_TAB && s.browser_open.contains(&db_fx) && s.browser_open.contains(&gens));
        app.state.browser_tab = 0;
        app.menu_item(ADD_MENU, index(ADD_MENU, "Browse plugin database"));
        assert_eq!(app.state.browser_tab, PLUGIN_TAB);
        // Shift+F8 (not the plugin menu F8 opens), but not while the search field has the keyboard.
        app.state.browser_tab = 0;
        app.shift = true;
        app.key(&Key::Named(NamedKey::F8), false, false);
        assert!(app.state.browser_tab == PLUGIN_TAB && app.state.playlist.popup.is_none());
        app.state.browser_tab = 0;
        app.state.browser_search_focus = true;
        app.key(&Key::Named(NamedKey::F8), false, false);
        assert_eq!(app.state.browser_tab, 0);
        app.shift = false;
        app.state.browser_search_focus = false;
        // Browser find, clicked in the dropdown with the browser hidden: typing then searches.
        app.state.show_browser = false;
        frame(&mut app, &mut c, None);
        let tools = hit_rect(&app, &Action::Menu(TOOLS_MENU));
        click(&mut app, &mut c, tools);
        let find = hit_rect(&app, &Action::MenuItem(TOOLS_MENU, index(TOOLS_MENU, "Browser find...")));
        click(&mut app, &mut c, find);
        assert!(app.state.show_browser && app.state.browser_search_focus && app.state.menu_open.is_none());
        app.key(&Key::Character("k".into()), false, false);
        assert_eq!(app.state.browser_search, "k");
        // Ctrl+F does the same; a name box keeps the keys.
        app.key(&Key::Named(NamedKey::Escape), false, false);
        app.state.show_browser = false;
        app.key(&Key::Character("f".into()), true, false);
        assert!(app.state.show_browser && app.state.browser_search_focus);
        app.state.browser_search_focus = false;
        crate::patterns::run(&mut app.state, crate::patterns::Cmd::Rename, (0.0, 0.0));
        app.key(&Key::Character("f".into()), true, false);
        assert!(!app.state.browser_search_focus);
        // Greyed out: the section the tree does not list, and the plugin browsers. Undo history works: the tree lists History.
        assert!(works(VIEW, item_at(VIEW, index(VIEW, "Undo history")).unwrap()));
        for (m, label) in [(VIEW, "Remote control"), (ADD_MENU, "Browse all installed plugins"), (ADD_MENU, "Browse presets")] {
            assert!(!works(m, item_at(m, index(m, label)).unwrap()), "{label}");
        }
    }

    /// PATTERNS > Open in project browser (spec: ui/browser-commands.md §4-5): the browser shown on the Current project
    /// tab with Patterns opened and the current pattern's row selected (the section itself when the pattern is not
    /// listed); other open sections stay open. Shift+Ctrl+P and the picker's pattern menu do the same.
    #[test]
    fn open_pattern_in_project_browser() {
        use crate::playlist::{self as pl, Cmd};
        use crate::views::browser::{project_root, rows, section_key, Item, PROJECT_TAB};
        use views::menus::{item_at, works, MENUS, PATTERNS};
        use winit::keyboard::Key;
        let mut app = App::new(None);
        app.state.project = test_project();
        app.state.project.patterns.push(os_model::Pattern { number: 2, name: Some("Drums".into()), ..Default::default() });
        app.state.pattern = 2;
        let (pats, fx) = (section_key("Patterns").unwrap(), section_key("Effects").unwrap());
        let reset = |app: &mut App| {
            let s = &mut app.state;
            s.show_browser = false;
            s.browser_tab = 0;
            s.browser_search = "kick".into();
            s.browser_sel = None;
            s.browser_open.remove(&pats);
            s.browser_open.remove(&project_root());
            s.browser_open.insert(fx.clone());
        };
        let entry = MENUS[PATTERNS].cols.iter().flat_map(|(_, items)| items.iter()).position(|it| it.label == "Open in project browser").unwrap();
        assert!(works(PATTERNS, item_at(PATTERNS, entry).unwrap()));
        let check = |app: &App, want: &str| {
            let s = &app.state;
            assert!(s.show_browser && s.browser_tab == PROJECT_TAB && s.browser_search.is_empty());
            assert!(s.browser_open.contains(&project_root()) && s.browser_open.contains(&pats) && s.browser_open.contains(&fx));
            assert_eq!(s.browser_sel.as_deref(), Some(want));
        };
        let drums = |app: &App| rows(&app.state).into_iter().find(|r| r.item == Item::Pattern(2)).map(|r| r.key);
        reset(&mut app);
        app.menu_item(PATTERNS, entry);
        let drums = drums(&app).expect("pattern 2 listed");
        check(&app, &drums);
        // A pattern the section does not list: the section is selected.
        reset(&mut app);
        app.state.pattern = 7;
        app.menu_item(PATTERNS, entry);
        check(&app, &pats);
        // Shift+Ctrl+P (the playlist in front keeps it for Performance mode, ui/playlist.md).
        app.state.pattern = 2;
        reset(&mut app);
        app.state.z.retain(|&w| w != Win::Playlist);
        app.shift = true;
        app.key(&Key::Character("P".into()), true, false);
        app.shift = false;
        check(&app, &drums);
        // The picker's pattern menu.
        let popup = views::menus::pattern_popup(0.0, 0.0);
        let item = popup.items.iter().find(|i| i.label == "Open in project browser").expect("in the pattern menu");
        assert!(item.enabled && item.cmd == Cmd::Pattern(crate::patterns::Cmd::OpenInBrowser));
        reset(&mut app);
        pl::run(&mut app, Cmd::Pattern(crate::patterns::Cmd::OpenInBrowser));
        check(&app, &drums);
    }

    /// Toolbar utility and window buttons (spec: ui/toolbar-buttons.md §2): "left / right" hints with the key; Undo,
    /// Save as..., About on Help's right click, Save new version on Save's; the playlist and piano roll shown maximized
    /// and the plugin picker on a right click, which acts on the release over the same button.
    #[test]
    fn toolbar_buttons_left_and_right() {
        fn right(app: &mut App, c: &mut Canvas, r: Rect, release: (f32, f32)) {
            app.mouse_move(r.cx(), r.cy());
            app.mouse_down_right();
            app.mouse_move(release.0, release.1);
            app.mouse_up();
            frame(app, c, None);
        }
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        frame(&mut app, &mut c, None);
        let hint = |app: &App, a: &Action| app.hits.iter().rev().find(|h| &h.action == a).map(|h| h.hint.to_string()).unwrap();
        for (a, h) in [
            (Action::Util(Util::Undo), "Undo / Undo history (Ctrl+Z)"),
            (Action::Util(Util::Help), "Help index / About..."),
            (Action::Util(Util::Save), "Save as... / Save new version (Shift+Ctrl+S)"),
            (Action::MenuItem(0, views::menus::FILE_EXPORT), "Export > Wave file... (Ctrl+R)"),
            (Action::MenuItem(views::menus::OPTIONS_MENU, views::menus::OPT_INFO), "Project info (F11)"),
            (Action::ToggleWin(Win::Playlist), "Playlist / Maximize Play list (F5)"),
            (Action::ToggleWin(Win::PianoRoll), "Piano roll / Maximize Piano roll (F7)"),
            (Action::ToggleWin(Win::Browser), "Browser / Plugin picker (Alt+F8)"),
            (Action::PickerOpen, "Plugin picker (F8)"),
        ] {
            assert_eq!(hint(&app, &a), h);
        }
        // Greyed out.
        for name in [
            "Open audio editor / New audio editor",
            "One-click audio recording",
            "Launch content library",
            "Project picker / Plugin picker (Ctrl+F8)",
            "Touch controller (Alt+F7)",
        ] {
            assert_eq!(hint(&app, &Action::Placeholder(name)), format!("{name}: not available yet"));
        }
        // Help: the left click does nothing; the right click is About, on the release over the button only.
        let help = hit_rect(&app, &Action::Util(Util::Help));
        app.state.message = None;
        click(&mut app, &mut c, help);
        assert!(app.state.message.is_none() && app.state.dialogs.is_empty());
        right(&mut app, &mut c, help, (help.cx() + 40.0, help.cy()));
        assert!(app.state.message.is_none());
        right(&mut app, &mut c, help, (help.cx(), help.cy()));
        assert!(app.state.message.as_deref().is_some_and(|m| m.starts_with("Lemon Studio ")), "{:?}", app.state.message);
        // Undo: EDIT > Undo; its right click does nothing yet.
        let undo = hit_rect(&app, &Action::Util(Util::Undo));
        app.state.message = None;
        right(&mut app, &mut c, undo, (undo.cx(), undo.cy()));
        assert!(app.state.message.is_none() && app.state.playlist.popup.is_none());
        click(&mut app, &mut c, undo);
        assert_eq!(app.state.message.as_deref(), Some("Nothing to undo"));
        // Save: Save as... (the save dialog even with a file), Save new version on the right.
        let path = std::env::temp_dir().join(format!("os_toolbar_save_{}_song.flp", std::process::id()));
        let next = path.with_file_name(format!("os_toolbar_save_{}_song_2.flp", std::process::id()));
        let _ = std::fs::remove_file(&next);
        app.save_to(path.clone());
        let save = hit_rect(&app, &Action::Util(Util::Save));
        let saves = crate::dialog::fake::shown().1;
        crate::dialog::fake::answer(None);
        click(&mut app, &mut c, save);
        assert_eq!(crate::dialog::fake::shown().1, saves + 1);
        assert!(!next.exists());
        right(&mut app, &mut c, save, (save.cx(), save.cy()));
        assert!(next.exists(), "{:?}", app.state.message);
        let _ = (std::fs::remove_file(&path), std::fs::remove_file(&next));
        // Playlist and piano roll: shown maximized and in front; a second right click leaves them maximized.
        for w in [Win::Playlist, Win::PianoRoll] {
            app.state.hide(w);
            let slot = w.slot().unwrap();
            app.state.wins[slot].maxed = false;
            frame(&mut app, &mut c, None);
            let b = hit_rect(&app, &Action::ToggleWin(w));
            for _ in 0..2 {
                right(&mut app, &mut c, b, (b.cx(), b.cy()));
                assert!(app.state.shown(w) && app.state.in_front(w) && app.state.wins[slot].maxed && !app.state.wins[slot].mini, "{w:?}");
            }
        }
        // Browser: the plugin picker, the browser itself left as it is.
        let shown = app.state.show_browser;
        let b = hit_rect(&app, &Action::ToggleWin(Win::Browser));
        right(&mut app, &mut c, b, (b.cx(), b.cy()));
        assert!(crate::plugin_menu::is_open(&app.state) && app.state.show_browser == shown);
    }

    /// The time display's right-click menu (spec: ui/time-display.md §1, §3): every mode and middle unit works, one
    /// dot each; a left click goes between Bar and M:S:CS, and from SMPTE to Bar.
    #[test]
    fn time_display_modes() {
        use crate::clock::{Mode, RATES};
        use crate::playlist::Mark;
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        frame(&mut app, &mut c, None);
        let clock = hit_rect(&app, &Action::Clock);
        let pick = |app: &mut App, c: &mut Canvas, label: &str| {
            app.mouse_move(clock.cx(), clock.cy());
            app.mouse_down_right();
            app.mouse_up();
            frame(app, c, None);
            let items = &app.state.playlist.popup.as_ref().expect("the clock menu").items;
            assert!(items.iter().filter(|it| it.label != "Mode" && it.label != "Middle unit").all(|it| it.enabled));
            let i = items.iter().position(|it| it.label == label).unwrap();
            let r = hit_rect(app, &Action::PlPopup(i));
            click(app, c, r);
            assert!(app.state.playlist.popup.is_none());
        };
        let dots = |app: &App| {
            let p = views::menus::clock_popup(&app.state, 0.0, 0.0);
            p.items.into_iter().filter(|it| it.mark == Mark::Radio(true)).map(|it| it.label).collect::<Vec<_>>()
        };
        assert_eq!(dots(&app), ["Bar : Beat/Step : Tick", "Step"]);
        assert_eq!(views::menus::clock_popup(&app.state, 0.0, 0.0).items.len(), 2 + 2 + RATES.len() + 2);
        pick(&mut app, &mut c, "SMPTE 29.97 fps (drop)");
        assert_eq!(app.state.clock.mode, Mode::Smpte(3));
        assert_eq!(dots(&app), ["SMPTE 29.97 fps (drop)", "Step"]);
        assert_eq!(app.hits.iter().find(|h| h.action == Action::Clock).unwrap().hint, "Song position (minute:second:frame)");
        // A left click from SMPTE: Bar; then M:S:CS and back.
        click(&mut app, &mut c, clock);
        assert_eq!(app.state.clock.mode, Mode::Bar);
        click(&mut app, &mut c, clock);
        assert_eq!(app.state.clock.mode, Mode::Msc);
        click(&mut app, &mut c, clock);
        pick(&mut app, &mut c, "Beat");
        assert_eq!(dots(&app), ["Bar : Beat/Step : Tick", "Beat"]);
        assert_eq!(app.hits.iter().find(|h| h.action == Action::Clock).unwrap().hint, "Song position (bar:beat:tick)");
        pick(&mut app, &mut c, "Minute : Second : Centisecond");
        assert!(app.state.clock.mode == Mode::Msc && app.state.clock.beat);
        // Kept for the next start (settings.txt).
        assert_eq!(App::new(None).state.clock, app.state.clock);
    }

    /// The Toolbars switches but Hide, and the time display's mode, are found again at the next start (settings.txt;
    /// spec: ui/toolbar-menu.md §8, ui/time-display.md §3).
    #[test]
    fn header_settings_are_kept() {
        use views::toolbar::{Opts, Switch};
        let mut app = App::new(None);
        assert_eq!((app.state.toolbar, app.state.clock), (Opts::default(), crate::clock::Clock::default()));
        for sw in [Switch::Separators, Switch::Flat, Switch::HintBar, Switch::Hide] {
            app.toolbar_switch(sw);
        }
        app.act(Action::Clock);
        let next = App::new(None);
        assert_eq!(next.state.toolbar, Opts { separators: false, flat: true, hint_bar: true, hidden: false });
        assert_eq!(next.state.clock.mode, crate::clock::Mode::Msc);
    }

    /// Every drop-down entry fits its row in this font: the label is not clipped and stops before its shortcut or its
    /// ▸ (the columns are FL's widths, widened where an entry needs it; macOS test of #61: TOOLS > Change declicking
    /// for selected channels to ▸ was cut at "channels t").
    #[test]
    fn menu_entries_fit_their_columns() {
        use views::menus::{label_now, shortcut_now, Kind, MENUS};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        app.state.rack.sel = [0, 1].into();
        let f = os_ui::Font::new(14);
        for (menu, m) in MENUS.iter().enumerate() {
            app.state.menu_open = Some(menu);
            frame(&mut app, &mut c, None);
            for it in m.cols.iter().flat_map(|(_, items)| items.iter()).filter(|it| matches!(it.kind, Kind::Item)) {
                let Some(h) = app.hits.iter().rev().find(|h| h.hint == it.label || h.action == Action::Placeholder(it.label)) else { continue };
                let label = c.text_width(f, &label_now(&app.state, menu, it));
                let key = if it.shortcut.is_empty() { 0.0 } else { c.text_width(f.italic(), shortcut_now(it)) + 4.0 };
                let end = h.rect.right() - if it.sub { 13.0 } else { 8.0 } - key;
                assert!(h.rect.x + 20.0 + label <= end, "{}: {:?} ends at {} past {end}", m.title, it.label, h.rect.x + 20.0 + label);
            }
        }
    }

    /// The toolbar's pattern box: a short name sits at its right; a name too long for the box keeps its start, cut at
    /// the spin arrow (macOS test of #61: "Lemon Sampler" read "mon Sampler").
    #[test]
    fn pattern_box_keeps_the_start_of_long_names() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        app.state.pattern = 1;
        // The left end of the box's text area, drawn with pattern 1 named `name`.
        let left = |app: &mut App, c: &mut Canvas, name: &str| {
            crate::patterns::pat_mut(&mut app.state, 1).name = Some(name.into());
            frame(app, c, None);
            let pb = hit_rect(app, &Action::PatternNext);
            let (x0, y0) = (pb.x as u32 + 4, pb.y as u32 + 6);
            (x0..x0 + 12).flat_map(|x| (y0..pb.bottom() as u32 - 6).map(move |y| (x, y))).map(|(x, y)| c.pixmap.pixel(x, y).unwrap()).collect::<Vec<_>>()
        };
        let empty = left(&mut app, &mut c, "");
        assert_eq!(left(&mut app, &mut c, "P1"), empty, "a short name is right-aligned");
        // The first letter shows: names differing only there draw differently.
        let (a, b) = (left(&mut app, &mut c, "Lemon Sampler and more"), left(&mut app, &mut c, "Wemon Sampler and more"));
        assert!(a != empty && a != b, "a long name shows its start");
    }

    /// The floating hint bar wraps a hint too long for a line instead of cutting it (macOS test of #61: "...right-click:
    /// channel mer").
    #[test]
    fn hint_bar_wraps_long_hints() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.toolbar.hint_bar = true;
        // The hint bar's second line, with `text` as the message (the mouse over the bar, which has no hint).
        let second = |app: &mut App, c: &mut Canvas, text: &str| {
            app.state.message = Some(text.into());
            let r = views::toolbar::hint_bar_rect(&app.state);
            app.mouse_move(r.cx(), r.bottom() - 4.0);
            frame(app, c, None);
            let y0 = (r.y + 5.0 + c.line_height(os_ui::Font::new(14))) as u32;
            (r.x as u32 + 7..r.x as u32 + 300).flat_map(|x| (y0 + 3..y0 + 12).map(move |y| (x, y))).map(|(x, y)| c.pixmap.pixel(x, y).unwrap()).collect::<Vec<_>>()
        };
        let empty = second(&mut app, &mut c, "Short");
        let long = "Click: plugin window, Shift+click: rename, right-click: channel menu, drag: move the channel to another row";
        assert_ne!(second(&mut app, &mut c, long), empty, "the rest goes on the second line");
    }

    /// The channel rack and time signatures (spec: ui/channel-rack.md §23, FL's numbers): the lit steps, the Auto
    /// length and the mini roll's bars are the pattern's own bars; the step colour groups (a beat) and Set to ▸ N bars
    /// are the project's; a step past an explicit length grows it to the end of the pattern's bar holding it.
    #[test]
    fn rack_follows_time_signatures() {
        use crate::rack::{bar_steps, beat_steps, pattern_bars, pattern_steps, RCmd};
        use os_model::TimeMarker;
        let mut app = App::new(None);
        app.state.project = test_project();
        app.state.pattern = 1;
        crate::patterns::pat_mut(&mut app.state, 1).notes.clear();
        let set_to = |app: &App| -> Vec<u32> {
            let items = crate::rack::length_menu(&app.state, 0.0, 0.0).items;
            items.into_iter().filter_map(|it| if let crate::playlist::Cmd::Rack(RCmd::SetLen(Some(v))) = it.cmd { Some(v) } else { None }).collect()
        };
        // 4/4 and no marker: one 16-step bar, beats of 4.
        let s = &app.state;
        assert_eq!((pattern_steps(s), beat_steps(s), bar_steps(s)), (16, 4, 16));
        // A 3/8 pattern in a 4/4 project: 6 lit steps, 12 with step 8 on; the groups stay at 4; Set to counts 16-step
        // bars; the pattern's bars are 6 steps.
        let sig = TimeMarker { kind: TimeMarker::SIGNATURE, numerator: 3, denominator: 8, ..TimeMarker::new(0, "3/8") };
        crate::patterns::pat_mut(&mut app.state, 1).markers.push(sig);
        assert_eq!((pattern_steps(&app.state), beat_steps(&app.state)), (6, 4));
        crate::rack::set_step(&mut app.state, 0, 7, true);
        assert_eq!(pattern_steps(&app.state), 12);
        assert_eq!(set_to(&app), [16, 32, 64, 128, 256, 512]);
        assert_eq!(pattern_bars(&app.state, 12 * 24), [(0, 144), (144, 288)]);
        crate::rack::set_len(&mut app.state, Some(6));
        app.rack_step(0, 8, false);
        app.drag = None;
        assert_eq!(pattern_steps(&app.state), 12, "grown to the second 3/8 bar");
        // A 6/8 project: beats of 2 steps and bars of 12 (an empty pattern without a marker, Set to's 1 bar); 6/16:
        // beats of 1 and bars of 6.
        app.state.project.bar = Some((6, 2));
        app.state.pattern = 2;
        let s = &app.state;
        assert_eq!((pattern_steps(s), beat_steps(s), bar_steps(s)), (12, 2, 12));
        assert_eq!(set_to(&app)[0], 12);
        app.state.project.bar = Some((6, 1));
        let s = &app.state;
        assert_eq!((pattern_steps(s), beat_steps(s)), (6, 1));

        // Drawn: in the 6/8 project the steps alternate in pairs; the 3/8 pattern's mini roll shades its two bars, each
        // half the width.
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        app.state.project.bar = Some((6, 2));
        app.state.raise(Win::ChannelRack);
        frame(&mut app, &mut c, None);
        let at = |c: &Canvas, x: f32, y: f32| c.pixmap.pixel(x as u32, y as u32).unwrap();
        let face = |app: &App, c: &Canvas, k: u32| {
            let r = hit_rect(app, &Action::Step(1, k));
            at(c, r.x + 6.0, r.y + 12.0)
        };
        let f: Vec<_> = (0..5).map(|k| face(&app, &c, k)).collect();
        assert!(f[0] == f[1] && f[1] != f[2] && f[2] == f[3] && f[3] != f[4], "pairs");
        app.state.pattern = 1;
        app.state.rack_roll_view = true;
        frame(&mut app, &mut c, None);
        let b = hit_rect(&app, &Action::RackRoll(1));
        let shade = |q: f32| at(&c, b.x + b.w * q, b.y + 15.0);
        assert!(shade(0.05) == shade(0.45) && shade(0.45) != shade(0.55) && shade(0.55) == shade(0.95), "two bars over the width");
    }

    /// The Toolbars menu (spec: ui/toolbar-menu.md §8): on the hint panel, on bare toolbar background and as VIEW >
    /// Toolbars ▸; Separators (on), Flat buttons, Hint bar (off) and Hide (Ctrl+F11) work, the rest stays greyed with
    /// our layout checked. Separators off: no lines, row 1 one pixel down and row 2 one up, also after a partial
    /// repaint. Flat: no face on an unlit button. Hint bar: the floating panel, repainted with the hint panel. Hide:
    /// no toolbar, the browser, workspace and floating windows up by its height; Ctrl+F11 brings them back.
    #[test]
    fn toolbars_menu() {
        use crate::playlist::Mark;
        use crate::views::toolbar::{band, hint_bar_rect, Opts};
        use views::menus::{item_at, submenu, toolbar_items, works, MENUS, VIEW};
        use winit::keyboard::{Key, NamedKey};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        frame(&mut app, &mut c, None);
        assert_eq!(app.state.toolbar, Opts { separators: true, flat: false, hint_bar: false, hidden: false });
        // The menu, from the hint panel and from bare background.
        let open = |app: &mut App, c: &mut Canvas, (x, y): (f32, f32)| {
            app.mouse_move(x, y);
            app.mouse_down_right();
            app.mouse_up();
            frame(app, c, None);
            app.state.playlist.popup.clone().expect("the Toolbars menu")
        };
        let bare = (1490.0, 20.0);
        assert_eq!(app.top_hit(bare.0, bare.1).map(|h| h.action.clone()), Some(Action::DragWindow));
        for at in [(200.0, 20.0), bare] {
            let p = open(&mut app, &mut c, at);
            assert_eq!(p.items, toolbar_items(&app.state));
            app.key(&Key::Named(NamedKey::Escape), false, false);
        }
        let items = toolbar_items(&app.state);
        let row = |label: &str| items.iter().find(|i| i.label == label).unwrap_or_else(|| panic!("{label}"));
        for (label, enabled, mark) in [
            ("Edit", false, Mark::Check(false)),
            ("Separators", true, Mark::Check(true)),
            ("Flat buttons", true, Mark::Check(false)),
            ("Show detected MIDI chords near the hint bar", false, Mark::Check(false)),
            ("Hint bar", true, Mark::Check(false)),
            ("Hide", true, Mark::Check(false)),
        ] {
            assert_eq!((row(label).enabled, row(label).mark), (enabled, mark), "{label}");
        }
        assert_eq!(row("Hide").key, "Ctrl+F11");
        let presets: Vec<_> = items.iter().skip_while(|i| i.label != "Presets").skip(1).collect();
        assert!(presets.len() == 9 && presets.iter().all(|i| !i.enabled));
        assert_eq!(presets.iter().filter(|i| i.mark == Mark::Radio(true)).map(|i| i.label.as_ref()).collect::<Vec<&str>>(), ["Classic"]);
        // VIEW > Toolbars ▸ is the same menu.
        let entry = MENUS[VIEW].cols.iter().flat_map(|(_, items)| items.iter()).position(|it| it.label == "Toolbars").unwrap();
        assert!(works(VIEW, item_at(VIEW, entry).unwrap()));
        assert_eq!(submenu(VIEW, "Toolbars").unwrap()(&app.state), items);
        let pick = |app: &mut App, c: &mut Canvas, label: &str| {
            let p = open(app, c, (200.0, 20.0));
            let i = p.items.iter().position(|it| it.label == label).unwrap();
            let r = hit_rect(app, &Action::PlPopup(i));
            click(app, c, r);
            app.mouse_move(1300.0, 600.0);
            frame(app, c, None);
        };
        let px = |c: &Canvas, x: u32, y: u32| c.pixmap.pixel(x, y).unwrap();
        let column = |c: &Canvas, x: u32| (0..83).map(|y| px(c, x, y)).collect::<Vec<_>>();
        // Separators off: the lines go, row 1 moves down a pixel, row 2 up one.
        let (r1, r2) = (column(&c, 595), column(&c, 990));
        let line = px(&c, 379, 20);
        pick(&mut app, &mut c, "Separators");
        assert!(!app.state.toolbar.separators);
        assert_ne!(px(&c, 379, 20), line);
        let (s1, s2) = (column(&c, 595), column(&c, 990));
        // Row 41 (the old line) holds row 2's top pixel: the rows close over it.
        assert_eq!(s1[1..=40], r1[0..=39]);
        assert_eq!(s2[41..80], r2[42..81]);
        // A partial repaint of the toolbar repaints it whole, so the pixels move once.
        app.damage = Damage::None;
        app.damage_rect(CPU_RECT);
        let (d, b) = (app.take_damage(), band(1920.0));
        assert!(matches!(&d, Some(Some(v)) if v.iter().any(|r| r.contains(b.x, b.y) && r.contains(b.right() - 1.0, b.bottom() - 1.0))), "{d:?}");
        frame_damage(&mut app, &mut c, d);
        assert_eq!((column(&c, 595), column(&c, 990)), (s1, s2));
        pick(&mut app, &mut c, "Separators");
        assert!(app.state.toolbar.separators && column(&c, 595) == r1);
        // Flat buttons: an unlit button is just its icon on the toolbar background.
        assert_ne!(px(&c, 580, 20), px(&c, 611, 20));
        pick(&mut app, &mut c, "Flat buttons");
        assert!(app.state.toolbar.flat);
        assert_eq!(px(&c, 580, 20), px(&c, 611, 20));
        pick(&mut app, &mut c, "Flat buttons");
        // Hint bar: the floating panel at the bottom-left, over the windows; it repaints with the hint panel.
        pick(&mut app, &mut c, "Hint bar");
        let hb = hint_bar_rect(&app.state);
        assert_eq!(hb, Rect::new(0.0, 1080.0 - 84.0, 400.0, 82.0));
        assert_eq!(app.top_hit(hb.cx(), hb.cy()).map(|h| h.action.clone()), Some(Action::Swallow));
        let p = px(&c, 2, hb.y as u32 + 2);
        assert!(p.red() < 0x20 && p.green() < 0x20 && p.blue() < 0x20, "{p:?}");
        app.damage = Damage::None;
        app.damage_rect(HINT_RECT);
        assert!(matches!(app.take_damage(), Some(Some(v)) if v.iter().any(|r| r.contains(hb.x, hb.y) && r.contains(hb.right() - 1.0, hb.bottom() - 1.0))));
        pick(&mut app, &mut c, "Hint bar");
        assert!(!app.state.toolbar.hint_bar);
        // Hide (Ctrl+F11): no toolbar; the browser and the workspace from the top, the floating playlist up with them.
        app.state.show_browser = true;
        app.state.wins[0].maxed = false;
        app.state.wins[0].rect = Rect::new(400.0, 200.0, 900.0, 500.0);
        app.state.raise(Win::Playlist);
        frame(&mut app, &mut c, None);
        let tab = hit_rect(&app, &Action::BrowserTab(1));
        app.key(&Key::Named(NamedKey::F11), true, false);
        frame(&mut app, &mut c, None);
        assert!(app.state.toolbar.hidden && !app.state.settings.open);
        assert_eq!(app.state.work_rect().y, 0.0);
        assert!(!app.hits.iter().any(|h| h.action == Action::Clock || h.action == Action::DragWindow));
        assert_eq!(hit_rect(&app, &Action::BrowserTab(1)).y, tab.y - views::WORK_Y);
        assert_eq!(app.state.win_rect(Win::Playlist).y, 200.0 - views::WORK_Y);
        // Ctrl+F11 again: everything as it was.
        app.key(&Key::Named(NamedKey::F11), true, false);
        frame(&mut app, &mut c, None);
        assert!(!app.state.toolbar.hidden && app.hits.iter().any(|h| h.action == Action::Clock));
        assert_eq!((app.state.work_rect().y, app.state.win_rect(Win::Playlist).y), (views::WORK_Y, 200.0));
        assert_eq!(hit_rect(&app, &Action::BrowserTab(1)), tab);
        // From the menu too; F11 alone is still Project info.
        pick(&mut app, &mut c, "Hide");
        assert!(app.state.toolbar.hidden);
        app.key(&Key::Named(NamedKey::F11), true, false);
        app.key(&Key::Named(NamedKey::F11), false, false);
        assert!(!app.state.toolbar.hidden && app.state.settings.open);
    }

    /// ADD > Audio clip / Automation clip (spec: ui/add-clip-channels.md §3): the channel goes last, named and routed to
    /// the master, alone selected, into its filter group (made before Unsorted) with the filter on it, as the
    /// playlist's clip source; its window opens; the automation clip has FL's default points and no target and saves
    /// so; each add is one undo step. Layer stays greyed out.
    #[test]
    fn add_audio_and_automation_clip_channels() {
        use crate::playlist::{self as pl, PickKind};
        use crate::rack::{Filter, Group};
        use os_model::{AutoMode, ClipSource};
        use views::menus::{item_at, works, ADD_MENU};
        let entry = |label: &str| (0..).find(|&i| item_at(ADD_MENU, i).is_some_and(|it| it.label == label)).unwrap();
        let mut app = App::new(None);
        app.state.project = test_project();
        // A file's groups: Drums, then Unsorted.
        app.state.rack.groups = vec![Group { name: "Drums".into(), members: [0, 3].into_iter().collect() }];
        app.state.rack.unsorted_at = Some(1);
        app.state.rack.select_only(1);
        app.state.selected_channel = 1;
        let before = (app.state.playlist.source, app.state.playlist.picker);
        let clips = pl::clips(&app.state).len();
        let names = |app: &App| app.state.rack.groups.iter().map(|g| g.name.clone()).collect::<Vec<_>>();
        // Audio clip.
        app.menu_item(ADD_MENU, entry("Audio clip"));
        let s = &app.state;
        let ch = s.project.channels.last().unwrap();
        assert_eq!((s.project.channels.len(), ch.index, ch.kind, ch.name.as_deref()), (6, 5, ChannelKind::AudioClip, Some("Audio Clip")));
        assert_eq!((ch.mixer_track, ch.sample_path.as_deref(), ch.sound), (0, None, Default::default()));
        assert_eq!((s.rack.sel.iter().copied().collect::<Vec<_>>(), s.selected_channel), (vec![5], 5));
        assert_eq!((names(&app), app.state.rack.unsorted_at, app.state.rack.filter), (vec!["Drums".into(), "Audio".into()], Some(2), Filter::Group(1)));
        let s = &app.state;
        assert_eq!(s.rack.groups[1].members.iter().copied().collect::<Vec<_>>(), [5]);
        assert_eq!((s.playlist.source, s.playlist.picker, pl::clips(s).len()), (Some(ClipSource::Channel(5)), PickKind::Audio, clips));
        assert_eq!(s.pending_open, Some(5), "its settings (Lemon Sampler's window) open");
        assert!(app.unsaved());
        // Automation clip.
        app.state.pending_open = None;
        app.menu_item(ADD_MENU, entry("Automation clip"));
        let s = &app.state;
        let ch = s.project.channels.last().unwrap();
        assert_eq!((ch.index, ch.kind, ch.name.as_deref(), ch.mixer_track), (6, ChannelKind::Automation, Some("Automation Clip"), 0));
        let a = s.project.automation(6).expect("its points").clone();
        let pts: Vec<_> = a.points.iter().map(|p| (p.beat, p.value, p.tension, p.mode)).collect();
        assert_eq!(pts, [(0.0, 0.5, 0.0, AutoMode::Single), (4.0, 0.5, 0.0, AutoMode::Single)]);
        assert!(a.targets.is_empty() && a.header[8] == 1);
        assert_eq!((names(&app), app.state.rack.unsorted_at, app.state.rack.filter), (vec!["Drums".into(), "Audio".into(), "Automation".into()], Some(3), Filter::Group(2)));
        let s = &app.state;
        assert!(s.shown(Win::Plugin) && s.plugin_channel == 6 && s.pending_open.is_none(), "its channel window opens");
        assert_eq!((s.playlist.source, s.playlist.picker), (Some(ClipSource::Channel(6)), PickKind::Automation));
        // Another audio clip: a unique name, the same group.
        app.menu_item(ADD_MENU, entry("Audio clip"));
        let s = &app.state;
        assert_eq!(s.project.channels[7].name.as_deref(), Some("Audio Clip #2"));
        assert_eq!((s.rack.groups.len(), s.rack.filter), (3, Filter::Group(1)));
        assert_eq!(s.rack.groups[1].members.iter().copied().collect::<Vec<_>>(), [5, 7]);
        // Saved as FL stores them: types 4 and 5, the groups before Unsorted, the automation's points and no link.
        let path = std::env::temp_dir().join(format!("os_add_clips_{}.flp", std::process::id()));
        app.save_to(path.clone());
        let back = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(back.groups, ["Drums", "Audio", "Automation", os_model::UNSORTED_GROUP]);
        let kinds: Vec<_> = back.channels[5..].iter().map(|c| (c.kind, c.name.clone().unwrap_or_default(), c.rack.group)).collect();
        assert_eq!(kinds, [(ChannelKind::AudioClip, "Audio Clip".into(), 1), (ChannelKind::Automation, "Automation Clip".into(), 2), (ChannelKind::AudioClip, "Audio Clip #2".into(), 1)]);
        assert_eq!(back.current_group, Some(1));
        let a2 = back.automation(6).expect("saved points");
        assert_eq!((&a2.points, &a2.targets, a2.header[8]), (&a.points, &Vec::new(), 1));
        // Undo (Ctrl+Z), one step each: the channel goes, the groups, filter, selection and clip source come back.
        let undo = |app: &mut App| crate::playlist::run(app, crate::playlist::Cmd::Undo);
        assert_eq!(undo(&mut app).as_deref(), Some("Undo: add audio clip"));
        assert_eq!((app.state.project.channels.len(), app.state.rack.groups[1].members.len()), (7, 1));
        assert_eq!(undo(&mut app).as_deref(), Some("Undo: add automation clip"));
        let s = &app.state;
        assert_eq!((s.project.channels.len(), s.project.automation(6).is_none()), (6, true));
        assert_eq!((names(&app), app.state.rack.filter, app.state.selected_channel), (vec!["Drums".into(), "Audio".into()], Filter::Group(1), 5));
        undo(&mut app);
        let s = &app.state;
        assert_eq!(s.project.channels.len(), 5);
        assert_eq!((names(&app), app.state.rack.unsorted_at, app.state.rack.filter), (vec!["Drums".into()], Some(1), Filter::All));
        let s = &app.state;
        assert_eq!((s.rack.sel.iter().copied().collect::<Vec<_>>(), s.selected_channel, (s.playlist.source, s.playlist.picker)), (vec![1], 1, before));
        assert!(!works(ADD_MENU, item_at(ADD_MENU, entry("Layer")).unwrap()), "Layer stays greyed out");
    }

    /// PATTERNS > Split by channel (spec: ui/split-by-channel.md §2): rack order; the pattern keeps the first
    /// channel's notes under its name, the others get patterns of their own right after it (later ones move up with
    /// their clips) with the original's colour, markers and settings; each clip of the original gets one clip per new
    /// pattern at its place, under it, unselected. One channel: a rename; no notes: nothing. One undo step.
    #[test]
    fn split_by_channel() {
        use crate::patterns::Cmd as PCmd;
        use crate::playlist::{self as pl, Cmd};
        use os_model::{Clip, ClipSource, ControlPoint, TimeMarker};
        use views::menus::{item_at, pattern_popup, works, MENUS, PATTERNS};
        let mut app = App::new(None);
        app.state.project = test_project();
        // Rack order Kick, Snare, Hat, Clap, Pluck (channel indices 0, 3, 2, 1, 4).
        app.state.project.channels.swap(1, 3);
        let step = |ch: u16, k: u32| Note::step(ch, k * 24);
        let sig = TimeMarker { kind: TimeMarker::SIGNATURE, numerator: 3, denominator: 4, ..TimeMarker::new(0, "3/4") };
        let color = Some(os_model::Rgb::new(0x45, 0x94, 0x6a));
        let p1 = Pattern {
            number: 1,
            color,
            notes: vec![step(1, 0), step(0, 1), step(3, 2), step(0, 3), step(1, 4)],
            controls: vec![ControlPoint::new(0, 0, ControlPoint::VOLUME, 9000)],
            loops: vec![(0, 8)],
            markers: vec![sig.clone()],
            ..Pattern::default()
        };
        let p2 = Pattern { number: 2, name: Some("Verse".into()), notes: vec![step(2, 0)], ..Pattern::default() };
        let p3 = Pattern { number: 3, notes: vec![step(2, 0), step(2, 4)], ..Pattern::default() };
        app.state.project.patterns = vec![p1.clone(), p2, p3];
        let mut cut = Clip::new(1536, 192, 2, ClipSource::Pattern(1));
        cut.window = Some((96.0, 288.0));
        let clips = vec![Clip::new(192, 384, 0, ClipSource::Pattern(1)), Clip::new(768, 384, 1, ClipSource::Pattern(2)), cut];
        pl::arr_mut(&mut app.state).clips = clips.clone();
        app.state.playlist.sel = vec![1];
        let before = (app.state.project.patterns.clone(), clips.clone());
        app.state.pattern = 1;
        let entry = MENUS[PATTERNS].cols.iter().flat_map(|(_, items)| items.iter()).position(|it| it.label == "Split by channel").unwrap();
        assert!(works(PATTERNS, item_at(PATTERNS, entry).unwrap()));
        let popup = pattern_popup(0.0, 0.0);
        let it = popup.items.iter().find(|i| i.label == "Split by channel").unwrap();
        assert!(it.enabled && it.cmd == Cmd::Pattern(PCmd::SplitByChannel), "also in the pattern menu");
        app.menu_item(PATTERNS, entry);
        let s = &app.state;
        let pat = |n: u16| s.project.pattern(n).unwrap();
        let summary = |n: u16| (pat(n).name.clone().unwrap_or_default(), pat(n).notes.iter().map(|x| (x.channel, x.position)).collect::<Vec<_>>());
        assert_eq!(summary(1), ("Kick".into(), vec![(0, 24), (0, 72)]));
        assert_eq!(summary(2), ("Snare".into(), vec![(3, 48)]));
        assert_eq!(summary(3), ("Clap".into(), vec![(1, 0), (1, 96)]));
        assert_eq!(summary(4), ("Verse".into(), vec![(2, 0)]), "later patterns move up");
        assert_eq!(pat(5).notes.len(), 2);
        for n in 2..=3 {
            let p = pat(n);
            assert_eq!((p.color, &p.markers, &p.loops, p.length, p.controls.is_empty()), (color, &p1.markers, &p1.loops, None, true), "pattern {n}");
        }
        assert_eq!((pat(1).color, &pat(1).controls, &pat(1).markers), (color, &p1.controls, &p1.markers));
        assert_eq!(s.pattern, 1, "the first stays current");
        // The playlist: one clip per new pattern at each clip of the original, just before it (the original on top).
        let at = |c: &Clip| (c.source, c.position, c.length, c.track, c.window);
        let want: Vec<_> = [(2, 0), (3, 0), (1, 0), (4, 1), (2, 2), (3, 2), (1, 2)]
            .into_iter()
            .map(|(p, k)| at(&Clip { source: ClipSource::Pattern(p), ..clips[k] }))
            .collect();
        assert_eq!(pl::clips(s).iter().map(at).collect::<Vec<_>>(), want);
        assert!(s.playlist.sel.is_empty(), "nothing selected");
        assert!(app.unsaved());
        // One undo step.
        assert_eq!(pl::run(&mut app, Cmd::Undo).as_deref(), Some("Undo: split by channel"));
        assert_eq!((app.state.project.patterns.clone(), pl::clips(&app.state).to_vec()), before);
        // A pattern with one channel's notes is only renamed; one without notes stays as it is.
        app.state.pattern = 3;
        let n = app.state.history.at();
        app.pattern_cmd(PCmd::SplitByChannel);
        let s = &app.state;
        assert_eq!((s.project.patterns.len(), s.project.pattern(3).unwrap().name.as_deref(), pl::clips(s).len()), (3, Some("Hat"), 3));
        assert_eq!(s.history.at(), n + 1);
        app.state.pattern = 7;
        app.pattern_cmd(PCmd::SplitByChannel);
        assert_eq!((app.state.project.patterns.len(), app.state.history.at()), (3, n + 1));
    }

    /// TOOLS / the channel rack's options > Switch all / selected audio clip channels to ▸ (spec: ui/channel-switches.md
    /// §1, §6): the stretch methods in two columns; a pick asks "Confirm" first; Ok sets the method on the audio clip
    /// channels (all, or the selected ones) and their Lemon Sampler, keeping TIME, PITCH and MUL; Cancel changes nothing;
    /// with no channel concerned nothing is asked. Saving keeps the method.
    #[test]
    fn switch_audio_clip_channels_stretch_method() {
        use crate::dialog::DHit;
        use crate::playlist::{self as pl, Cmd, Mark};
        use os_model::stretch_mode as sm;
        use views::menus::{CLIPS_ALL_LABEL, CLIPS_SEL_LABEL, MENUS, TOOLS_MENU};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        let wav = std::env::temp_dir().join(format!("os-app-switch-{}.wav", std::process::id()));
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &[0.5; 9600]).unwrap();
        pl::add_audio_clip(&mut app.state, &wav.to_string_lossy(), 0, 0).expect("audio clip added");
        pl::add_audio_clip(&mut app.state, &wav.to_string_lossy(), 1, 0).expect("audio clip added");
        let n = app.state.project.channels.len();
        let (a, b) = (n - 2, n - 1);
        // TIME 4 beats, PITCH +100 cents, MUL x1.19, Resample.
        let kept = [768f32.to_bits(), 100, 2540];
        app.state.project.channels[a].sound.stretch = [kept[0], kept[1], kept[2], 0];
        app.plugins = crate::plugins::Plugins::default();
        app.plugins.available = crate::plugins::builtin();
        app.sync_samplers(None);
        app.mark_saved();
        assert!(app.state.hosted_rows.contains(&a) && app.state.hosted_rows.contains(&b), "Lemon Sampler plays the audio clips");
        let sampler = |app: &App, row: usize| {
            let p = app.plugins.hosted[&row].plugin.lock().unwrap();
            let mut st = [0u32; 4];
            for i in (0..p.param_count()).filter_map(|i| p.param_info(i)) {
                crate::plugins::stretch_param(&i.name, p.param_value(i.id).unwrap(), &mut st);
            }
            st[3] as i32
        };
        let methods = |app: &App| app.state.project.channels.iter().map(|ch| ch.sound.stretch_mode()).collect::<Vec<_>>();
        // The submenu: the methods as audio/time-stretch.md names them, the second column from Auto.
        let labels: Vec<String> = crate::rack::stretch_items(true).iter().map(|i| if i.col { "|".into() } else { i.label.to_string() }).collect();
        let want = "Realtime Resample Stretch Stretch Pro Elastique e3 generic e3 mono Special Slice stretch Slice map | Auto Legacy e2 generic e2 transient e2 mono e2 speech";
        assert_eq!(labels.join(" "), want);
        let items = crate::rack::stretch_items(true);
        assert!(items.iter().all(|i| i.mark == Mark::None && (i.head || i.col || i.enabled)), "commands, no marks");
        let stored: Vec<i32> = items.iter().filter_map(|i| match i.cmd {
            Cmd::Rack(crate::rack::RCmd::StretchClips(true, m)) => Some(m),
            _ => None,
        }).collect();
        assert_eq!(stored, [0, -1, -2, 1, 2, 3, 4, 5, 6, 7, 8, 9], "event 215 int 27");
        // TOOLS > Switch all audio clip channels to > Stretch Pro asks first.
        let entry = MENUS[TOOLS_MENU].cols.iter().flat_map(|(_, items)| items.iter()).position(|it| it.label == CLIPS_ALL_LABEL).unwrap();
        let pick = |app: &mut App, c: &mut Canvas, label: &str| {
            frame(app, c, None);
            let tools = hit_rect(app, &Action::Menu(TOOLS_MENU));
            click(app, c, tools);
            let row = hit_rect(app, &Action::MenuSub(TOOLS_MENU, entry));
            click(app, c, row);
            let i = app.state.playlist.popup.as_ref().expect("the submenu").items.iter().position(|it| it.label == label).unwrap();
            let item = hit_rect(app, &Action::PlPopup(i));
            click(app, c, item);
        };
        pick(&mut app, &mut c, "Stretch Pro");
        let text = "All audio clip channels will be set to stretch pro mode. Some channels may need manual time adjustment.";
        assert_eq!(app.top_message(), Some(("Confirm".into(), text.into())));
        assert!(app.state.menu_open.is_none() && app.state.playlist.popup.is_none());
        let crate::dialog::Body::Msg(m) = &app.state.dialogs.last().unwrap().body else { panic!("a message box") };
        assert_eq!((m.kind, &m.buttons[..]), (crate::dialog::Kind::Warning, &[crate::dialog::Answer::Ok, crate::dialog::Answer::Cancel][..]));
        // Cancel changes nothing.
        let cancel = hit_rect(&app, &Action::Dlg(DHit::Button(1)));
        click(&mut app, &mut c, cancel);
        assert!(!app.modal_open() && !app.unsaved());
        assert_eq!((methods(&app)[a], methods(&app)[b]), (0, 0));
        // Ok sets it on both audio clip channels, their other stretch settings kept; the sampler channels keep theirs.
        pick(&mut app, &mut c, "Stretch Pro");
        app.state.piano.dirty = false;
        let ok = hit_rect(&app, &Action::Dlg(DHit::Button(0)));
        click(&mut app, &mut c, ok);
        assert!(!app.modal_open());
        assert_eq!(methods(&app), [0, 0, 0, 0, 0, sm::STRETCH_PRO, sm::STRETCH_PRO]);
        assert_eq!(app.state.project.channels[a].sound.stretch[..3], kept);
        assert!(app.unsaved() && app.state.piano.dirty, "a change to the project, played at once");
        assert_eq!((sampler(&app, a), sampler(&app, b), sampler(&app, 0)), (sm::STRETCH_PRO, sm::STRETCH_PRO, 0), "Lemon Sampler follows");
        // The rack's options menu: Switch selected... > e2 speech, on the selected audio clip channel only (Enter = Ok).
        app.state.rack.sel = [0, b].into();
        let rack = crate::rack::options_menu(&app.state, 0.0, 0.0);
        let all = rack.items.iter().find(|i| i.label == CLIPS_ALL_LABEL).expect("in the rack's options menu");
        assert_eq!(all.children, crate::rack::stretch_items(true));
        let sel = rack.items.iter().find(|i| i.label == CLIPS_SEL_LABEL).expect("in the rack's options menu");
        assert_eq!(sel.children, crate::rack::stretch_items(false));
        let speech = sel.children.iter().find(|i| i.label == "e2 speech").unwrap().cmd.clone();
        pl::run(&mut app, speech.clone());
        let text = "Selected audio clip channels will be set to e2 speech mode. Some channels may need manual time adjustment.";
        assert_eq!(app.top_message(), Some(("Confirm".into(), text.into())));
        app.key(&winit::keyboard::Key::Named(winit::keyboard::NamedKey::Enter), false, false);
        assert!(!app.modal_open());
        assert_eq!(methods(&app), [0, 0, 0, 0, 0, sm::STRETCH_PRO, sm::E2_SPEECH]);
        assert_eq!(sampler(&app, b), sm::E2_SPEECH);
        // No audio clip channel selected: nothing is asked, nothing changes.
        app.state.rack.sel = [0, 1].into();
        pl::run(&mut app, speech);
        assert!(!app.modal_open());
        assert_eq!(methods(&app)[a], sm::STRETCH_PRO);
        // A project without audio clip channels: nothing is asked.
        let mut plain = App::new(None);
        plain.state.project = test_project();
        pl::run(&mut plain, Cmd::Rack(crate::rack::RCmd::StretchClips(true, sm::STRETCH)));
        assert!(!plain.modal_open());
        // Saving keeps the methods.
        let path = std::env::temp_dir().join(format!("os-app-switch-{}.flp", std::process::id()));
        app.save_to(path.clone());
        let back = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        let clips: Vec<[u32; 4]> = back.channels.iter().filter(|c| c.kind == ChannelKind::AudioClip).map(|c| c.sound.stretch).collect();
        assert_eq!(clips, [[kept[0], kept[1], kept[2], sm::STRETCH_PRO as u32], [0, 0, 0, sm::E2_SPEECH as u32]]);
        let _ = (std::fs::remove_file(&path), std::fs::remove_file(&wav));
    }

    /// PATTERNS > Set time signature... (spec: ui/pattern-time-signature.md): the "Time signature change" dialog opens at
    /// 4/4; its value boxes open the app's popup lists (Numerator 1..16, Denominator 2/4/8/16, the current value marked);
    /// Accept puts a time-signature marker named after it ("3/8") at the start of the current pattern, which the marker
    /// lanes show once and saving keeps. With one there already the conflict question comes first: No leaves it, Yes
    /// removes it and opens the dialog, which closed without Accept leaves no start marker. The pattern menu has the
    /// command too.
    #[test]
    fn pattern_time_signature() {
        use crate::dialog::{Answer, Body, DHit, Kind};
        use crate::playlist::{self as pl, Cmd, Mark};
        use crate::timesig::{form_open, list_open, SigForm};
        use os_model::TimeMarker;
        use views::menus::{item_at, works, MENUS, PATTERNS};
        use winit::keyboard::{Key, NamedKey};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        app.state.pattern = 1;
        app.state.project.patterns[0].markers = vec![TimeMarker::new(192, "Mid")];
        app.mark_saved();
        let entry = MENUS[PATTERNS].cols.iter().flat_map(|(_, items)| items.iter()).position(|it| it.label == "Set time signature...").unwrap();
        assert!(works(PATTERNS, item_at(PATTERNS, entry).unwrap()));
        let popup = views::menus::pattern_popup(0.0, 0.0);
        let item = popup.items.iter().find(|i| i.label == "Set time signature...").expect("in the pattern menu");
        assert!(item.enabled && item.cmd == Cmd::Pattern(crate::patterns::Cmd::TimeSignature));
        let form = |app: &App| match app.state.dialogs.last().map(|m| &m.body) {
            Some(Body::TimeSig(f)) => Some(*f),
            _ => None,
        };
        let start = |app: &App| app.state.project.pattern(1).unwrap().markers.iter().find(|m| m.position == 0 && m.kind == TimeMarker::SIGNATURE).cloned();
        // The dialog, at 4/4.
        app.menu_item(PATTERNS, entry);
        assert_eq!(form(&app), Some(SigForm { pattern: 1, num: 4, den: 4 }));
        frame(&mut app, &mut c, None);
        // Numerator: 1..16 under the box, 4 marked; 3 picked.
        let num_box = hit_rect(&app, &Action::Dlg(DHit::SigValue(false)));
        click(&mut app, &mut c, num_box);
        assert_eq!(list_open(&app.state), Some(false));
        let p = app.state.playlist.popup.clone().unwrap();
        assert_eq!((p.x, p.y), (num_box.x, num_box.bottom() + 1.0), "under the box");
        let labels: Vec<String> = p.items.iter().map(|i| i.label.to_string()).collect();
        assert_eq!(labels, (1..=16).map(|n: u8| n.to_string()).collect::<Vec<_>>());
        let marked: Vec<String> = p.items.iter().filter(|i| i.mark == Mark::Radio(true)).map(|i| i.label.to_string()).collect();
        assert_eq!(marked, ["4"]);
        let three = hit_rect(&app, &Action::PlPopup(2));
        assert!(app.top_hit(three.cx(), three.cy()).is_some_and(|h| h.action == Action::PlPopup(2)), "the list is over the dialog");
        click(&mut app, &mut c, three);
        assert!(app.state.playlist.popup.is_none() && form_open(&app.state));
        assert_eq!(form(&app).map(|f| (f.num, f.den)), Some((3, 4)));
        // Denominator: 2, 4, 8, 16; 8 picked.
        let den_box = hit_rect(&app, &Action::Dlg(DHit::SigValue(true)));
        click(&mut app, &mut c, den_box);
        let p = app.state.playlist.popup.clone().unwrap();
        assert_eq!(p.items.iter().map(|i| i.label.to_string()).collect::<Vec<_>>(), ["2", "4", "8", "16"]);
        let eight = hit_rect(&app, &Action::PlPopup(2));
        click(&mut app, &mut c, eight);
        assert_eq!(form(&app).map(|f| (f.num, f.den)), Some((3, 8)));
        // A click beside an open list closes it; so does Esc, before it closes the dialog.
        click(&mut app, &mut c, num_box);
        click(&mut app, &mut c, Rect::new(5.0, 5.0, 2.0, 2.0));
        assert!(app.state.playlist.popup.is_none() && form_open(&app.state));
        click(&mut app, &mut c, num_box);
        app.key(&Key::Named(NamedKey::Escape), false, false);
        assert!(app.state.playlist.popup.is_none() && form_open(&app.state));
        assert_eq!(form(&app).map(|f| (f.num, f.den)), Some((3, 8)));
        assert!(start(&app).is_none() && !app.unsaved(), "nothing yet");
        // Accept: a 3/8 marker at the pattern's start, before its other markers.
        app.state.piano.dirty = false;
        frame(&mut app, &mut c, None);
        let accept = hit_rect(&app, &Action::Dlg(DHit::Accept));
        click(&mut app, &mut c, accept);
        assert!(!app.modal_open());
        let want = TimeMarker { kind: TimeMarker::SIGNATURE, numerator: 3, denominator: 8, ..TimeMarker::new(0, "3/8") };
        assert_eq!(app.state.project.pattern(1).unwrap().markers, [want.clone(), TimeMarker::new(192, "Mid")]);
        assert!(app.unsaved() && app.state.piano.dirty);
        assert_eq!(crate::views::playlist::marker_label(&want), "3/8", "shown once in the marker lanes");
        // Saved with the pattern.
        let path = std::env::temp_dir().join(format!("os-app-timesig-{}.flp", std::process::id()));
        app.save_to(path.clone());
        let back = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        assert_eq!(back.pattern(1).unwrap().markers, [want.clone(), TimeMarker::new(192, "Mid")]);
        let _ = std::fs::remove_file(&path);
        // Again, from the pattern menu: the conflict question; No leaves the marker.
        pl::run(&mut app, Cmd::Pattern(crate::patterns::Cmd::TimeSignature));
        let text = "The pattern already has a time signature defined at the start. Do you want replace it?";
        assert_eq!(app.top_message(), Some(("Time signature conflict".into(), text.into())));
        let Some(Body::Msg(m)) = app.state.dialogs.last().map(|m| &m.body) else { panic!("a message box") };
        assert_eq!((m.kind, &m.buttons[..]), (Kind::Question, &[Answer::Yes, Answer::No][..]));
        frame(&mut app, &mut c, None);
        let no = hit_rect(&app, &Action::Dlg(DHit::Button(1)));
        click(&mut app, &mut c, no);
        assert!(!app.modal_open());
        assert_eq!(start(&app), Some(want.clone()));
        // Yes: the marker goes at once and the dialog opens at 4/4; closed with its × it leaves no start marker.
        app.menu_item(PATTERNS, entry);
        app.key(&Key::Character("y".into()), false, false);
        assert_eq!(form(&app), Some(SigForm { pattern: 1, num: 4, den: 4 }));
        assert!(start(&app).is_none());
        frame(&mut app, &mut c, None);
        let close = hit_rect(&app, &Action::Dlg(DHit::Close));
        click(&mut app, &mut c, close);
        assert!(!app.modal_open());
        assert_eq!(app.state.project.pattern(1).unwrap().markers, [TimeMarker::new(192, "Mid")]);
        // No marker: straight to the dialog; Enter accepts. Another pattern gets its own.
        app.state.pattern = 2;
        app.menu_item(PATTERNS, entry);
        assert_eq!(form(&app), Some(SigForm { pattern: 2, num: 4, den: 4 }));
        app.key(&Key::Named(NamedKey::Enter), false, false);
        assert!(!app.modal_open());
        let four = TimeMarker { kind: TimeMarker::SIGNATURE, ..TimeMarker::new(0, "4/4") };
        assert_eq!(app.state.project.pattern(2).unwrap().markers, [four]);
        assert!(start(&app).is_none(), "pattern 1 keeps none");
    }

    /// A pattern's length follows its own time signature (spec: audio/song-structure.md, "Time signatures and a
    /// pattern's length"; ui/pattern-time-signature.md §2, §4-5): the table's rows, the same in the playlist and the
    /// piano roll, the channel rack's length still first. Set time signature... names its marker "3/8", shown once (the
    /// playlist's "4/4" markers too); the pattern's clips that kept their natural length take the new one, and the old
    /// one back when Yes takes the marker away; clips resized or cut by hand keep theirs. The piano roll's bars and
    /// beats follow the pattern's signature: 3/8, a bar every 6 steps.
    #[test]
    fn pattern_length_follows_its_time_signature() {
        use crate::piano::{pattern_length, Geom};
        use crate::playlist::{self as pl, Cmd};
        use os_model::{Clip, ClipSource, TimeMarker};
        use views::playlist::marker_label;
        use winit::keyboard::{Key, NamedKey};
        let mut app = App::new(None);
        app.state.project = test_project();
        app.state.pattern = 1;
        let note = |end: u32| Note { length: 100, ..Note::step(0, end - 100) };
        let sig = |pos: u32, num: u8, den: u8| TimeMarker { kind: TimeMarker::SIGNATURE, numerator: num, denominator: den, ..TimeMarker::new(pos, &format!("{num}/{den}")) };
        let len = |app: &App| {
            let (playlist, piano) = (pl::source_len(&app.state, ClipSource::Pattern(1)), pattern_length(&app.state, 1));
            assert_eq!(playlist, piano, "the playlist and the piano roll agree");
            playlist
        };
        let set = |app: &mut App, project: (u16, u16), arr: Vec<TimeMarker>, own: Vec<TimeMarker>, end: u32| {
            app.state.project.bar = Some((project.0, 16 / project.1));
            pl::arr_mut(&mut app.state).markers = arr;
            let p = &mut app.state.project.patterns[0];
            (p.notes, p.markers, p.length) = (vec![note(end)], own, None);
        };
        // The table: 96 PPQ, the last note ends at 300; the clip at 768.
        let rows = [
            ((4, 4), vec![], vec![], 384),
            ((3, 4), vec![], vec![], 576),
            ((4, 4), vec![sig(0, 3, 4)], vec![], 384),
            ((4, 4), vec![sig(768, 3, 4)], vec![], 384),
            ((4, 4), vec![], vec![sig(0, 3, 4)], 576),
            ((4, 4), vec![], vec![sig(192, 3, 4)], 480),
        ];
        pl::arr_mut(&mut app.state).clips = vec![Clip::new(768, 384, 0, ClipSource::Pattern(1))];
        for (i, (project, arr, own, want)) in rows.into_iter().enumerate() {
            set(&mut app, project, arr, own, 300);
            assert_eq!(len(&app), want, "row {}", i + 1);
        }
        set(&mut app, (4, 4), vec![], vec![sig(0, 3, 4)], 500);
        assert_eq!(len(&app), 576, "a note ending at 500 with the 3/4 marker at 0");
        set(&mut app, (4, 4), vec![], vec![sig(0, 3, 4)], 384);
        assert_eq!(len(&app), 576, "at least one bar");
        set(&mut app, (4, 4), vec![], vec![], 384);
        assert_eq!(len(&app), 384, "a note ending on a bar line");
        app.state.project.patterns[0].length = Some(192);
        assert_eq!(len(&app), 192, "the channel rack's length wins");
        // The piano roll's grid at 3/4 from tick 192: 4/4 bars before it, a bar line at it, 3/4 bars after it.
        set(&mut app, (4, 4), vec![], vec![sig(192, 3, 4)], 300);
        let g = Geom::new(&app.state, Rect::new(0.0, 0.0, 800.0, 400.0));
        assert_eq!(g.bar_lines(1000.0), [(0, 1), (192, 2), (480, 3), (768, 4)]);
        assert_eq!([96, 192, 288, 384, 480].map(|t| g.line_at(t)), [(false, true), (true, true), (false, true), (false, true), (true, true)]);
        // Set time signature... 3/8 on a 4/4 pattern with clips: natural length (in both arrangements), resized and cut
        // by hand, and another pattern's.
        set(&mut app, (4, 4), vec![], vec![], 300);
        let natural = Clip::new(0, 384, 0, ClipSource::Pattern(1));
        let resized = Clip::new(384, 768, 1, ClipSource::Pattern(1));
        let cut = Clip { window: Some((96.0, 480.0)), ..Clip::new(1536, 384, 2, ClipSource::Pattern(1)) };
        let other = Clip::new(0, 384, 3, ClipSource::Pattern(2));
        pl::arr_mut(&mut app.state).clips = vec![natural, resized, cut, other];
        let mut second = app.state.project.arrangements[0].clone();
        second.clips = vec![natural];
        app.state.project.arrangements.push(second);
        let lengths = |app: &App| app.state.project.arrangements.iter().map(|a| a.clips.iter().map(|c| c.length).collect::<Vec<_>>()).collect::<Vec<_>>();
        assert_eq!(len(&app), 384);
        pl::run(&mut app, Cmd::Pattern(crate::patterns::Cmd::TimeSignature));
        app.sig_pick(false, 3);
        app.sig_pick(true, 8);
        app.key(&Key::Named(NamedKey::Enter), false, false);
        assert!(!app.modal_open());
        let marker = &app.state.project.pattern(1).unwrap().markers[0];
        assert_eq!((marker, marker.name.as_str(), marker_label(marker).as_str()), (&sig(0, 3, 8), "3/8", "3/8"));
        assert_eq!(len(&app), 432, "three bars of 144");
        assert_eq!(lengths(&app), [vec![432, 768, 384, 384], vec![432]]);
        let g = Geom::new(&app.state, Rect::new(0.0, 0.0, 800.0, 400.0));
        assert_eq!((g.step, g.beat, g.bar), (24, 48, 144));
        let bars: Vec<u32> = (0..=12).filter(|&i| g.line_at(i * 24).0).collect();
        let beats: Vec<u32> = (0..=12).filter(|&i| g.line_at(i * 24).1).collect();
        assert_eq!((bars, beats), (vec![0, 6, 12], vec![0, 2, 4, 6, 8, 10, 12]), "a bar every 6 steps, a beat every 2");
        assert_eq!(g.bar_lines(500.0), [(0, 1), (144, 2), (288, 3), (432, 4)]);
        // An empty 3/8 pattern is one 3/8 bar (§4).
        app.state.project.patterns[0].notes.clear();
        assert_eq!(len(&app), 144);
        app.state.project.patterns[0].notes = vec![note(300)];
        // Yes to the conflict question takes the marker away at once: back to 4/4 bars and lengths.
        pl::run(&mut app, Cmd::Pattern(crate::patterns::Cmd::TimeSignature));
        app.key(&Key::Character("y".into()), false, false);
        app.key(&Key::Named(NamedKey::Escape), false, false);
        assert!(!app.modal_open() && app.state.project.pattern(1).unwrap().markers.is_empty());
        assert_eq!(len(&app), 384);
        assert_eq!(lengths(&app), [vec![384, 768, 384, 384], vec![384]]);
        // The playlist's own time-signature markers ("4/4") are shown once too; they leave the pattern's length alone.
        pl::run(&mut app, Cmd::AddSignature);
        let m = pl::arr_mut(&mut app.state).markers.last().cloned().unwrap();
        assert_eq!((m.name.as_str(), marker_label(&m).as_str()), ("4/4", "4/4"));
        assert_eq!(len(&app), 384);
        // A named signature marker still shows its name after the signature.
        assert_eq!(marker_label(&TimeMarker { name: "Verse".into(), ..sig(0, 3, 4) }), "3/4 Verse");
        // The piano roll draws the 3/8 grid.
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        crate::timesig::set_start(&mut app.state, 1, 3, 8);
        frame(&mut app, &mut c, None);
    }

    /// Settings > Plugins (spec: ui/plugin-manager.md §2, §4): Add folder... keeps the folder and scans it in the
    /// background; a file that is no plugin shows in the table as an error; ★ and Show change the menus; the Find box
    /// filters; a folder switches off and goes.
    #[test]
    fn plugin_manager_folders_scan_and_table() {
        use crate::views::settings::{Plug, SAct, COMBINE_CHECK, PLUGINS};
        use winit::keyboard::{Key, NamedKey};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.plugins = crate::plugins::Plugins::builtins();
        app.state.plugdb.rebuild(&app.plugins);
        app.open_settings(PLUGINS);
        frame(&mut app, &mut c, None);
        let finish = |app: &mut App| {
            let t0 = std::time::Instant::now();
            while app.sweep.is_some() && t0.elapsed() < std::time::Duration::from_secs(20) {
                std::thread::sleep(std::time::Duration::from_millis(5));
                app.scan_tick();
            }
            assert!(app.sweep.is_none(), "the scan ends");
        };
        let dir = std::env::temp_dir().join(format!("os_plugin_folder_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub").join("Broken.clap"), b"not a plugin").unwrap();
        crate::dialog::fake::answer(Some(dir.clone()));
        let r = hit_rect(&app, &Action::Settings(SAct::Button("Add folder...")));
        click(&mut app, &mut c, r);
        assert!(app.state.plugdb.prefs.folders.iter().any(|f| std::path::Path::new(&f.path) == dir), "the folder is kept");
        assert!(app.sweep.is_some(), "adding a folder scans");
        finish(&mut app);
        frame(&mut app, &mut c, None);
        let row = app.state.plugdb.rows.iter().find(|r| r.file == "Broken.clap").expect("the failed file has a row");
        assert_eq!(row.status, "error");
        assert!(!row.toggles);
        assert!(app.state.message.as_deref().is_some_and(|m| m.contains("1 file with errors")), "{:?}", app.state.message);
        // Star, then hide, an effect (one no scan found: as if listed before the scan).
        app.plugins.effects.push(crate::plugin_db::fake("Test FX", os_host::Kind::Effect));
        app.state.plugdb.rebuild(&app.plugins);
        frame(&mut app, &mut c, None);
        let row_of = |app: &App| app.state.plugdb.rows.iter().position(|r| r.name == "Test FX").unwrap();
        let r = hit_rect(&app, &Action::Settings(SAct::Plug(Plug::Star(row_of(&app)))));
        click(&mut app, &mut c, r);
        assert!(app.state.plugdb.lists.fx.iter().any(|e| e.name == "Test FX" && e.favorite), "starred");
        let r = hit_rect(&app, &Action::Settings(SAct::Plug(Plug::Show(row_of(&app)))));
        click(&mut app, &mut c, r);
        assert!(!app.state.plugdb.lists.fx.iter().any(|e| e.name == "Test FX"), "hidden plugins leave the menus");
        // Combining formats switches off.
        let r = hit_rect(&app, &Action::Settings(SAct::Check(COMBINE_CHECK)));
        click(&mut app, &mut c, r);
        assert!(!app.state.plugdb.prefs.combine);
        // The Find box: typing filters, Esc clears it and keeps the window open.
        let r = hit_rect(&app, &Action::Settings(SAct::Plug(Plug::Find)));
        click(&mut app, &mut c, r);
        for ch in ["b", "R", "o"] {
            app.key(&Key::Character(ch.into()), false, false);
        }
        assert_eq!(app.state.plugdb.rows.len(), 1, "only Broken.clap matches");
        app.key(&Key::Named(NamedKey::Escape), false, false);
        assert!(app.state.plugdb.find.is_empty() && !app.state.plugdb.finding && app.state.settings.open);
        assert!(app.state.plugdb.rows.len() > 1);
        // The folder: off (scans again), then removed.
        frame(&mut app, &mut c, None);
        let k = crate::plugin_db::search_dirs(&app.state.plugdb.prefs).iter().position(|d| d.path == dir).unwrap();
        let r = hit_rect(&app, &Action::Settings(SAct::Plug(Plug::Folder(k))));
        click(&mut app, &mut c, r);
        assert!(app.state.plugdb.prefs.folders.iter().all(|f| !f.on));
        finish(&mut app);
        assert!(!app.state.plugdb.cache.files.iter().any(|f| f.path.ends_with("Broken.clap")), "a skipped folder's files drop out");
        frame(&mut app, &mut c, None);
        let r = hit_rect(&app, &Action::Settings(SAct::Plug(Plug::Remove(k))));
        click(&mut app, &mut c, r);
        assert!(app.state.plugdb.prefs.folders.is_empty());
        finish(&mut app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// VST2 plugins through the app's own paths, with real plugins: `OS_VST2_DIR` is a folder of the DPF-Plugins
    /// release (Kars and 3BandEQ, each also there as VST3 and CLAP), `OS_VST2_DIR32` (optional) the release's 32-bit
    /// folder, `OS_SHOT_DIR` (optional) gets pictures of Settings > Plugins, `OS_VST2_KEEP` (optional) a copy of the saved
    /// project (for opening in other software).
    /// - Settings > Plugins (spec: ui/plugin-manager.md §2, §4): both folders added with Add folder... and scanned, the
    ///   32-bit VST2 plugins listed as failed with that reason, VST2 picked as the preferred format.
    /// - Kars added from the rack's Add one ▸ menu, 3BandEQ from its insert's slot menu, an automation clip made on the
    ///   EQ's master gain and drawn rising: a song export of eight C5 notes has each note at its beat, at its pitch and
    ///   louder than the one before. Played live (with an audio device), then stopped: silent at once.
    /// - Saved, reopened (both found as VST2 with their state, the clip kept) and exported again; then opened with only
    ///   the VST3 and CLAP copies listed: those copies play them, from their own defaults, and the hint bar says so.
    ///   Saved so, the file keeps both VST2 plugins byte for byte, also after an export that played the copies and
    ///   automated the EQ's; once the user edits Kars' copy (the parameter menu's Paste value), it is saved as that copy.
    ///
    /// Loads the plugins in this process, so ignored:
    /// `OS_VST2_DIR=<dir> cargo test -p os-app --bin lemonstudio vst2_plugins_through_the_app -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn vst2_plugins_through_the_app() {
        use crate::playlist::{self as pl, Cmd};
        use crate::plugin_menu::PlugCmd;
        use crate::plugins::fx_key;
        use crate::views::settings::{Plug, SAct, PLUGINS};
        use os_host::Format;
        use std::time::Duration;
        use winit::keyboard::{Key, NamedKey};
        let Some(dir) = std::env::var_os("OS_VST2_DIR").map(PathBuf::from) else { return };
        let dir32 = std::env::var_os("OS_VST2_DIR32").map(PathBuf::from);
        let shots = std::env::var_os("OS_SHOT_DIR").map(PathBuf::from);
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut a = App::new(None);
        a.state.project = test_project();
        a.plugins = crate::plugins::Plugins::builtins();
        let rate = crate::transport::Transport::device_rate().unwrap_or(48000);
        a.state.plugdb.rebuild(&a.plugins);
        // Only the added folders are scanned: the standard ones hold the user's own plugins.
        a.state.plugdb.prefs.standard_off = crate::plugin_db::search_dirs(&a.state.plugdb.prefs).iter().map(|d| d.path.display().to_string()).collect();
        a.open_settings(PLUGINS);
        frame(&mut a, &mut c, None);
        let finish = |a: &mut App| {
            let t0 = std::time::Instant::now();
            while a.sweep.is_some() && t0.elapsed() < std::time::Duration::from_secs(120) {
                std::thread::sleep(std::time::Duration::from_millis(5));
                a.scan_tick();
            }
            assert!(a.sweep.is_none(), "the scan ends");
        };
        for d in std::iter::once(&dir).chain(&dir32) {
            crate::dialog::fake::answer(Some(d.clone()));
            let r = hit_rect(&a, &Action::Settings(SAct::Button("Add folder...")));
            click(&mut a, &mut c, r);
            finish(&mut a);
        }
        println!("scan: {:?}", a.state.message);
        let r = hit_rect(&a, &Action::Settings(SAct::Plug(Plug::Prefer(crate::plugin_db::Prefer::Vst2))));
        click(&mut a, &mut c, r);
        assert_eq!(a.state.plugdb.prefs.prefer, crate::plugin_db::Prefer::Vst2);
        let vst2: Vec<&crate::plugin_db::TableRow> = a.state.plugdb.all_rows.iter().filter(|r| r.format == "VST2" || r.file.ends_with("-vst.dll")).collect();
        for r in &vst2 {
            println!("  {:22} {:5} {:10} {:8} {:30} {}", r.name, r.format, r.kind, r.status, r.file, r.note);
        }
        let ok = |name: &str| vst2.iter().any(|r| r.name.replace(' ', "") == name && r.format == "VST2" && r.status == "ok");
        assert!(ok("Kars") && ok("3BandEQ"), "Kars and 3BandEQ listed as VST2");
        if dir32.is_some() {
            let unsupported = crate::plugin_db::Status::Unsupported.text();
            assert!(vst2.iter().any(|r| r.status == unsupported && r.note.contains("32-bit")), "32-bit plugins listed as not supported, with the reason");
            assert!(!vst2.iter().any(|r| r.status == "error" && r.note.contains("32-bit")), "and not as errors");
        }
        if let Some(d) = &shots {
            frame(&mut a, &mut c, None);
            c.pixmap.save_png(d.join("vst2_settings_plugins.png")).unwrap();
            let r = hit_rect(&a, &Action::Settings(SAct::Plug(Plug::Find)));
            click(&mut a, &mut c, r);
            for ch in "-vst".chars() {
                a.key(&Key::Character(ch.to_string().into()), false, false);
            }
            frame(&mut a, &mut c, None);
            c.pixmap.save_png(d.join("vst2_settings_plugins_vst.png")).unwrap();
            a.key(&Key::Named(NamedKey::Escape), false, false);
        }
        a.state.settings.open = false;

        // Kars from the rack's Add one ▸ menu: with VST2 preferred, its one entry is the VST2 copy.
        let add = crate::rack::add_one_items(&a.state).into_iter().find(|it| it.label == "Kars").expect("Add one lists Kars");
        let row = a.state.project.channels.len();
        pl::run(&mut a, add.cmd);
        for _ in 0..200 {
            a.tick();
            if a.state.pending_plugin.is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(a.state.hosted_rows.contains(&row), "Kars hosted: {:?}", a.state.message);
        assert_eq!(a.plugins.descriptor(row).map(|d| d.format), Some(Format::Vst2));
        // 3BandEQ in the first slot of Kars' insert, from the slot's menu.
        let track = a.state.project.channels[row].mixer_track;
        crate::mixer::select(&mut a.state, track, false);
        let eq = a.plugins.effects.iter().position(|e| e.desc.format == Format::Vst2 && e.desc.name.replace(' ', "") == "3BandEQ").expect("3BandEQ");
        let menu = crate::mixer::effect_menu(&a.state, 0, 0.0, 0.0);
        let cmd = Cmd::Plugin(PlugCmd::Effect { index: eq, slot: Some(0) });
        assert!(menu.items.iter().any(|it| it.cmd == cmd), "the slot menu lists the VST2 copy");
        pl::run(&mut a, cmd);
        let key = fx_key(track, 0);
        for _ in 0..200 {
            a.tick();
            if a.plugins.hosted.contains_key(&key) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(a.plugins.descriptor(key).map(|d| d.format), Some(Format::Vst2), "the EQ is hosted: {:?}", a.state.message);
        // Eight C5 notes, a beat each, on the pattern placed at the song's start.
        let ch = a.state.project.channels[row].index;
        let ppq = u32::from(a.state.project.ppq);
        a.state.project.patterns[0].notes = (0..8).map(|i| os_model::Note { length: ppq, key: 60, ..os_model::Note::step(ch, i * ppq) }).collect();
        a.state.playlist.reset();
        pl::arr_mut(&mut a.state).clips.push(os_model::Clip::new(0, 8 * ppq, 0, os_model::ClipSource::Pattern(1)));
        // The EQ's master gain automated from its lowest to its highest over the eight beats.
        a.params_tick();
        let master = a.state.params.lists[&key].params.iter().position(|p| p.name == "Master").expect("a Master parameter");
        println!("automate: {:?}", a.params_automate(key, master));
        let auto = a.state.project.channels.last().unwrap().index;
        {
            let x = a.state.project.automation_mut(auto).unwrap();
            assert_eq!(x.points.len(), 2);
            x.points[0].value = 0.0;
            x.points[1].value = 1.0;
        }
        let tempo = f64::from(a.state.tempo);
        let export = |a: &mut App, tag: &str| -> Vec<f32> {
            let path = std::env::temp_dir().join(format!("lemon-vst2-{tag}-{}.wav", std::process::id()));
            a.transport.export(&a.state.project, os_engine::PlayMode::Song, rate, path.clone());
            let t0 = std::time::Instant::now();
            let _secs = loop {
                match a.transport.export_status() {
                    crate::transport::Export::Done(r) => break r.expect("export"),
                    _ if t0.elapsed() > Duration::from_secs(60) => panic!("export did not finish"),
                    _ => std::thread::sleep(Duration::from_millis(10)),
                }
            };
            let bytes = std::fs::read(&path).expect("file written");
            let _ = std::fs::remove_file(&path);
            bytes[44..].chunks_exact(8).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
        };
        // Per note: (frames from its beat to where it first reaches half its peak, peak, pitch in Hz by autocorrelation).
        let beat = f64::from(rate) * 60.0 / tempo;
        let notes = |left: &[f32]| -> Vec<(i64, f32, f64)> {
            (0..8)
                .map(|i| {
                    let s = (i as f64 * beat).round() as usize;
                    let w = &left[s..(s + beat as usize).min(left.len())];
                    let peak = w.iter().fold(0.0f32, |m, v| m.max(v.abs()));
                    let from = s.saturating_sub(512);
                    let onset = left[from..].iter().position(|v| v.abs() > 0.5 * peak).map_or(i64::MAX, |k| (from + k) as i64 - s as i64);
                    let x = &w[2400..2400 + 4096];
                    let corr = |lag: usize| x.iter().zip(&x[lag..]).map(|(p, q)| p * q).sum::<f32>();
                    let best = (80..480).map(corr).fold(f32::MIN, f32::max);
                    let lag = (80..480).find(|&l| corr(l) > 0.9 * best && corr(l) >= corr(l - 1) && corr(l) >= corr(l + 1)).unwrap_or(0);
                    (onset, peak, f64::from(rate) / lag.max(1) as f64)
                })
                .collect()
        };
        let check = |label: &str, n: &[(i64, f32, f64)]| {
            println!("{label}: {:?}", n.iter().map(|(o, p, f)| format!("{o:+} fr, {p:.3}, {f:.0} Hz")).collect::<Vec<_>>());
            for (i, &(onset, peak, hz)) in n.iter().enumerate() {
                assert!(onset.abs() <= 64, "{label}: note {i} starts {onset} frames from its beat");
                assert!((hz / 261.63).log2().abs() < 1.0 / 12.0, "{label}: note {i} at {hz:.0} Hz, not C5");
                if i > 0 {
                    assert!(peak > n[i - 1].1 * 1.2, "{label}: note {i} is not louder than the one before");
                }
            }
        };
        a.tick();
        let first = notes(&export(&mut a, "first"));
        check("export", &first);

        // Live: the pattern plays on the device, Stop silences it. The export left the EQ's Master at the top of its
        // clip, far too loud for the speakers, so it plays at Master's default (pattern playback leaves it there).
        if crate::transport::Transport::device_rate().is_some() {
            {
                let mut p = a.plugins.hosted[&key].plugin.lock().unwrap();
                let info = p.param_info(master).unwrap();
                assert!(p.set_param(info.id, info.default));
            }
            a.transport.take_peaks();
            a.transport.play(&a.state.project, os_engine::PlayMode::Pattern(1), 0.0);
            std::thread::sleep(Duration::from_millis(1200));
            let heard = a.transport.take_peaks();
            let tick = a.transport.tick();
            a.transport.stop();
            std::thread::sleep(Duration::from_millis(100));
            a.transport.take_peaks();
            std::thread::sleep(Duration::from_millis(200));
            let after = a.transport.take_peaks();
            println!("live: peaks {heard:?} while playing (at tick {tick:.0}), {after:?} from 100 ms after Stop");
            assert!(heard.0 > 0.01, "heard while playing");
            assert!(heard.0 < 1.0, "played at a level the speakers take, not clipped ({:.2})", heard.0);
            assert_eq!(after, (0.0, 0.0), "silent after Stop");
        }

        // Saved: both plugins stored as wrapped VST2 plugins with their state, the clip aimed at the EQ. Kars' last
        // parameter is first moved off its default, so the state that comes back is known not to be the default one.
        let path = std::env::temp_dir().join(format!("lemon-vst2-{}.flp", std::process::id()));
        let moved = {
            let mut p = a.plugins.hosted[&row].plugin.lock().unwrap();
            let info = p.param_info(p.param_count() - 1).unwrap();
            let v = if p.param_value(info.id).is_some_and(|v| v > 0.5) { 0.25 } else { 0.75 };
            assert!(p.set_param(info.id, v));
            p.idle();
            println!("Kars {} set to {v} ({:?})", info.name, p.param_text(info.id, v));
            (info.id, v)
        };
        let (ch_state, fx_state) = (a.plugins.state(row).unwrap(), a.plugins.state(key).unwrap());
        a.save_to(path.clone());
        if let Some(keep) = std::env::var_os("OS_VST2_KEEP") {
            std::fs::copy(&path, &keep).unwrap();
        }
        assert!(a.state.message.as_deref().is_some_and(|m| m.starts_with("Saved")), "{:?}", a.state.message);
        let saved = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        let r = crate::project_plugins::wrapped_channel(&saved.channels[row]).expect("a wrapped channel");
        assert_eq!((r.format.as_str(), r.id.as_str(), r.name.as_str()), ("vst2", a.plugins.descriptor(row).unwrap().id.as_str(), "Kars"));
        assert_eq!(r.state, ch_state);
        let slot = saved.mixer.iter().find(|m| m.index == track).and_then(|m| m.slots[0].as_ref());
        let s = slot.and_then(crate::project_plugins::wrapped_slot).expect("a wrapped slot");
        assert_eq!((s.format.as_str(), s.state == fx_state), ("vst2", true));
        let target = a.state.project.automations.last().unwrap().target();
        assert!(saved.automations.iter().any(|x| x.target() == target), "the clip is saved");
        println!("saved: Kars state {} bytes ({:?}...), 3BandEQ state {} bytes", ch_state.len(), &ch_state[..ch_state.len().min(25)], fx_state.len());

        // Reopened: found as VST2 again, with the saved state, and sounding the same.
        let lists = |keep: &dyn Fn(Format) -> bool| {
            let mut p = crate::plugins::Plugins::builtins();
            p.available = a.plugins.available.iter().filter(|x| keep(x.desc.format)).cloned().collect();
            p.effects = a.plugins.effects.iter().filter(|x| keep(x.desc.format)).cloned().collect();
            p
        };
        let mut b = App::new(None);
        b.plugins = lists(&|_| true);
        b.open(path.clone());
        b.sync_wrapped();
        assert!(b.state.hosted_rows.contains(&row), "{:?}", b.state.message);
        assert_eq!(b.plugins.descriptor(row).map(|d| d.format), Some(Format::Vst2));
        assert_eq!(b.plugins.descriptor(key).map(|d| d.format), Some(Format::Vst2));
        assert_eq!(b.plugins.state(row).as_deref(), Some(&ch_state[..]));
        assert_eq!(b.plugins.state(key).as_deref(), Some(&fx_state[..]));
        let back = b.plugins.hosted[&row].plugin.lock().unwrap().param_value(moved.0);
        assert!(back.is_some_and(|v| (v - moved.1).abs() < 1e-6), "Kars' moved parameter came back as {back:?}");
        b.tick();
        check("reopened", &notes(&export(&mut b, "reopened")));

        // With no VST2 copy listed, the project's VST2 plugins are played by their copies in another format, from those
        // copies' defaults (the saved state is the VST2 plugin's), and the hint bar names them.
        let mut v = App::new(None);
        v.plugins = lists(&|f| f != Format::Vst2);
        v.open(path.clone());
        v.sync_wrapped();
        let _ = std::fs::remove_file(&path);
        for k in [row, key] {
            assert!(v.plugins.descriptor(k).is_some_and(|d| d.format != Format::Vst2), "{k} played by a copy: {:?}", v.state.message);
        }
        let msg = v.state.message.clone().unwrap_or_default();
        assert!(msg.contains("from its default settings") && msg.contains("Kars (VST2 as") && msg.contains("3 Band EQ (VST2 as"), "{msg}");
        let fresh = {
            let mut f = lists(&|f| f != Format::Vst2);
            let i = v.plugins.available.iter().position(|x| Some(&x.desc) == v.plugins.descriptor(row).as_ref()).unwrap();
            f.create_with(row, i, None).unwrap();
            f.state(row)
        };
        assert_eq!(v.plugins.state(row), fresh, "Kars' copy starts from its defaults");
        // Saved untouched: the copies leave both wrappers as the file had them, so the VST2 plugins and their settings
        // stay in the project (spec: flp/events/plugins.md §4).
        let path2 = std::env::temp_dir().join(format!("lemon-vst2-copies-{}.flp", std::process::id()));
        let reread = |p: &std::path::Path| os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(p).unwrap()).unwrap()).unwrap();
        let wrappers = |p: &os_model::Project| {
            let slot = p.mixer.iter().find(|m| m.index == track).and_then(|m| m.slots[0].clone());
            ((p.channels[row].plugin.clone(), p.channels[row].plugin_state.clone()), slot)
        };
        v.save_to(path2.clone());
        assert!(v.state.message.as_deref().is_some_and(|m| m.starts_with("Saved")), "{:?}", v.state.message);
        assert!(wrappers(&reread(&path2)) == wrappers(&saved), "untouched copies keep the VST2 plugins byte for byte");
        // Played (a song export): playing is not the user's edit, nor is the automation clip moving the EQ copy's
        // Master, so both copies still keep the VST2 plugins.
        v.tick();
        let n = notes(&export(&mut v, "substituted"));
        println!("substituted: {:?}", n.iter().map(|(o, p, f)| format!("{o:+} fr, {p:.3}, {f:.0} Hz")).collect::<Vec<_>>());
        v.params_tick();
        v.save_to(path2.clone());
        assert!(wrappers(&reread(&path2)) == wrappers(&saved), "played and automated copies keep the VST2 plugins");
        // Kars' copy edited by the user (Paste value onto its last parameter, Volume, in the parameter menu): saved as
        // that copy with its state.
        assert_eq!(v.browse_params(row), None);
        let last = v.state.params.lists[&row].params.len() - 1;
        let id = v.state.params.lists[&row].params[last].id;
        let x = v.plugins.hosted[&row].plugin.lock().unwrap().param_value(id).unwrap();
        v.state.params.copied = Some(if x > 0.5 { 0.25 } else { 0.75 });
        assert_eq!(crate::playlist::run(&mut v, crate::playlist::Cmd::Param(crate::params::PCmd::Paste(row, last))), None);
        v.plugins.hosted[&row].plugin.lock().unwrap().idle();
        let kars = v.plugins.state(row).unwrap();
        assert_ne!(Some(&kars), fresh.as_ref(), "Kars' copy changed");
        v.save_to(path2.clone());
        let again = reread(&path2);
        let _ = std::fs::remove_file(&path2);
        let r = crate::project_plugins::wrapped_channel(&again.channels[row]).expect("a wrapped channel");
        let d = v.plugins.descriptor(row).unwrap();
        assert_eq!((r.format.as_str(), r.name.as_str(), r.state == kars), (d.format.as_str(), d.name.as_str(), true));
        println!("Kars' changed copy saved as {} ({} bytes of state)", r.format, kars.len());
    }

    /// "Monophonic painting in piano roll" (spec: ui/settings-choices.md "Miscellaneous: seven more checks, Default
    /// template and Startup project"): on, a draw-tool click and each note of a paint drag delete the notes of other
    /// keys whose time overlaps the new note, in the same channel and pattern, as part of the paint's undo step. Off,
    /// and for other channels or notes beside it, the notes stay.
    #[test]
    fn monophonic_painting_in_piano_roll() {
        use crate::prefs::{set_check, MONO_PAINTING};
        use os_model::Note;
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let note = |ch: u16, key: u16, at: u32, len: u32| Note { key, length: len, ..Note::step(ch, at) };
        let keys = |app: &App, ch: u16| {
            let mut v: Vec<(u16, u32)> = piano::notes(&app.state).iter().filter(|n| n.channel == ch).map(|n| (n.key, n.position)).collect();
            v.sort_unstable();
            v
        };
        for on in [false, true] {
            set_check(&MONO_PAINTING, on);
            let mut app = App::new(None);
            app.state.z = vec![Win::PianoRoll];
            app.state.selected_channel = 0;
            // A draw-tool click: an eighth-long G5 (ppq 96) over channel 1's C5; channel 2's G5 and channel 1's later
            // D5 are not under it. The click's drag then moves the new note a beat on (it still is the one dragged).
            let start = vec![note(0, 60, 0, 96), note(1, 67, 0, 96), note(0, 62, 384, 96)];
            piano::pattern_mut(&mut app.state).notes = start.clone();
            frame(&mut app, &mut c, None);
            let g = app.piano_geom().expect("piano roll shown");
            let at = |g: &Geom, key: i32, tick: u32| (g.x_of(tick as f32) + 2.0, g.y_of(key) + g.row / 2.0);
            let (x, y) = at(&g, 67, 0);
            app.mouse_move(x, y);
            app.mouse_down();
            let (x, y) = at(&g, 67, 96);
            app.mouse_move(x, y);
            app.mouse_up();
            let want: Vec<(u16, u32)> = if on { vec![(62, 384), (67, 96)] } else { vec![(60, 0), (62, 384), (67, 96)] };
            assert_eq!(keys(&app, 0), want, "draw click, check {on}");
            assert_eq!(keys(&app, 1), vec![(67, 0)], "another channel's notes stay");
            assert!(piano::undo(&mut app.state));
            assert_eq!(piano::notes(&app.state), &start[..], "one undo step, check {on}");

            // A paint drag of E5s an eighth apart over a run of C5s: each E5 deletes the C5 under it.
            let run: Vec<Note> = (0..3).map(|k| note(0, 60, 96 * k, 48)).collect();
            piano::pattern_mut(&mut app.state).notes = run.clone();
            app.state.piano.last_len = 48;
            app.act(Action::PianoTool(Tool::Paint));
            frame(&mut app, &mut c, None);
            let g = app.piano_geom().expect("piano roll shown");
            let (x, y) = at(&g, 64, 0);
            app.mouse_move(x, y);
            app.mouse_down();
            let (x, y) = at(&g, 64, 200);
            app.mouse_move(x, y);
            app.mouse_up();
            let painted: Vec<(u16, u32)> = (0..5).map(|k| (64, 48 * k)).collect();
            let kept: Vec<(u16, u32)> = if on { Vec::new() } else { run.iter().map(|n| (60, n.position)).collect() };
            let mut want = [kept, painted].concat();
            want.sort_unstable();
            assert_eq!(keys(&app, 0), want, "paint drag, check {on}");
            assert!(piano::undo(&mut app.state));
            assert_eq!(piano::notes(&app.state), &run[..], "one undo step for the drag, check {on}");
        }
        set_check(&MONO_PAINTING, false);
    }

    /// The header's menus and switches as pictures, for looking (not an assertion): `OS_HEADER_SHOT=<folder>`.
    #[test]
    fn header_shot() {
        use crate::views::toolbar::Switch;
        use views::menus::{ADD_MENU, MENUS, PATTERNS, VIEW};
        let Some(out) = std::env::var_os("OS_HEADER_SHOT") else { return };
        let out = std::path::PathBuf::from(out);
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        app.state.pattern = 1;
        // Kick on every beat, a clap on 2 and 4; two clips of the pattern.
        let note = |ch: u16, at: u32| Note { length: 48, ..Note::step(ch, at) };
        app.state.project.patterns[0].notes = vec![note(0, 0), note(1, 96), note(0, 192), note(1, 288), note(0, 384), note(1, 480)];
        crate::playlist::arr_mut(&mut app.state).clips = vec![
            os_model::Clip::new(0, 768, 0, os_model::ClipSource::Pattern(1)),
            os_model::Clip::new(1536, 768, 2, os_model::ClipSource::Pattern(1)),
        ];
        let save = |app: &mut App, c: &mut Canvas, name: &str| {
            app.invalidate();
            frame(app, c, None);
            c.pixmap.save_png(out.join(format!("{name}.png"))).unwrap();
        };
        let entry = |m: usize, label: &str| MENUS[m].cols.iter().flat_map(|(_, items)| items.iter()).position(|it| it.label == label).unwrap();
        save(&mut app, &mut c, "01_default");
        // The Toolbars menu on the hint panel.
        app.mouse_move(200.0, 20.0);
        app.mouse_down_right();
        app.mouse_up();
        save(&mut app, &mut c, "02_toolbars_menu");
        app.key(&winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape), false, false);
        // VIEW > Toolbars ▸.
        frame(&mut app, &mut c, None);
        let view = hit_rect(&app, &Action::Menu(VIEW));
        click(&mut app, &mut c, view);
        let row = hit_rect(&app, &Action::MenuSub(VIEW, entry(VIEW, "Toolbars")));
        click(&mut app, &mut c, row);
        save(&mut app, &mut c, "03_view_toolbars");
        app.key(&winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape), false, false);
        app.key(&winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape), false, false);
        // Flat buttons, no separators, the hint bar, a button hovered.
        app.toolbar_switch(Switch::Flat);
        app.toolbar_switch(Switch::Separators);
        app.toolbar_switch(Switch::HintBar);
        frame(&mut app, &mut c, None);
        let save_btn = app.hits.iter().find(|h| h.hint.starts_with("Save")).map(|h| h.rect).unwrap();
        app.mouse_move(save_btn.cx(), save_btn.cy());
        save(&mut app, &mut c, "04_flat_noseps_hintbar");
        app.toolbar_switch(Switch::Flat);
        app.toolbar_switch(Switch::Separators);
        app.toolbar_switch(Switch::HintBar);
        app.toolbar_switch(Switch::Hide);
        save(&mut app, &mut c, "05_hidden");
        app.toolbar_switch(Switch::Hide);
        // The clock in SMPTE and in B:B:T, a second and a half in.
        app.state.clock.mode = crate::clock::Mode::Smpte(4);
        save(&mut app, &mut c, "06_clock_smpte");
        app.state.clock.mode = crate::clock::Mode::Bar;
        app.state.clock.beat = true;
        let clock = hit_rect(&app, &Action::Clock);
        app.mouse_move(clock.cx(), clock.cy());
        app.mouse_down_right();
        app.mouse_up();
        save(&mut app, &mut c, "07_clock_menu");
        app.key(&winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape), false, false);
        app.state.clock = Default::default();
        // ADD and PATTERNS menus.
        frame(&mut app, &mut c, None);
        let add = hit_rect(&app, &Action::Menu(ADD_MENU));
        click(&mut app, &mut c, add);
        save(&mut app, &mut c, "08_add_menu");
        app.key(&winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape), false, false);
        frame(&mut app, &mut c, None);
        let pats = hit_rect(&app, &Action::Menu(PATTERNS));
        click(&mut app, &mut c, pats);
        save(&mut app, &mut c, "09_patterns_menu");
        app.key(&winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape), false, false);
        // The time signature dialog, then 3/8 in the piano roll.
        app.menu_item(PATTERNS, entry(PATTERNS, "Set time signature..."));
        save(&mut app, &mut c, "10_timesig_dialog");
        app.state.dialogs.clear();
        crate::timesig::set_start(&mut app.state, 1, 3, 8);
        app.act(Action::ShowMax(Win::PianoRoll));
        save(&mut app, &mut c, "11_piano_3_8");
        app.act(Action::ShowMax(Win::Playlist));
        save(&mut app, &mut c, "11b_playlist_3_8");
        // Split by channel, then the playlist.
        app.menu_item(PATTERNS, entry(PATTERNS, "Split by channel"));
        save(&mut app, &mut c, "12_split_playlist");
        // ADD > Automation clip and Audio clip.
        app.menu_item(ADD_MENU, entry(ADD_MENU, "Automation clip"));
        save(&mut app, &mut c, "13_automation_clip");
        app.menu_item(ADD_MENU, entry(ADD_MENU, "Audio clip"));
        save(&mut app, &mut c, "14_audio_clip");
        // The channel rack with pattern 1 in 3/8 (ui/channel-rack.md §23): lit to its length, grouped by the project's
        // beat; its mini roll over the pattern's bars; then the same in a 6/8 project.
        app.state.pattern = 1;
        let sig = os_model::TimeMarker { kind: os_model::TimeMarker::SIGNATURE, numerator: 3, denominator: 8, ..os_model::TimeMarker::new(0, "3/8") };
        app.state.project.patterns[0].markers = vec![sig];
        app.state.project.patterns[0].length = None;
        crate::rack::run(&mut app, crate::rack::RCmd::SetFilter(crate::rack::Filter::All));
        app.state.rack_size.0 = views::rack::DEFAULT_SIZE.0 + 320.0;
        app.state.raise(Win::ChannelRack);
        save(&mut app, &mut c, "15_rack_38");
        app.state.rack_roll_view = true;
        save(&mut app, &mut c, "16_rack_mini_38");
        app.state.rack_roll_view = false;
        app.state.project.bar = Some((6, 2));
        save(&mut app, &mut c, "17_rack_project_68");
    }
}
