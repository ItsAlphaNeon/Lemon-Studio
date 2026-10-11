//! Application state, actions and the per-frame paint/hit-test pass.
//!
//! Painting is immediate: every frame repaints from `State` and records hit regions. Hover uses
//! the topmost hit from the previous frame, so occluded controls never highlight.

use std::borrow::Cow;
use std::path::PathBuf;

use os_engine::PlayMode;
use os_model::{Channel, ChannelKind, MixerTrack, Note, Pattern, Project};
use os_theme::{Palette, Theme};
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
    /// A control that is designed but not functional yet.
    Placeholder(&'static str),
    /// The toolbar's scope / spectrum panel: click switches the display.
    Monitor,
    /// Monitor debug panel: a slider (index into `monitor::PARAMS`), Reset, Save, Close.
    MonParam(usize),
    MonReset,
    MonSave,
    MonClose,
    Minimize,
    Maximize,
    Close,
    DragWindow,
    Play,
    Stop,
    Record,
    PatSong(bool),
    Tempo,
    Menu(usize),
    MenuItem(usize, usize),
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
    /// Plugin picker: background (click closes), tab, "All" tab, tile (effects?, index).
    PickerBg,
    /// A rotary control (drag vertically, wheel steps).
    Knob(crate::knobs::KnobId),
    /// Toolbar plugin picker button.
    PickerOpen,
    PickerTab(bool, usize),
    PickerAll,
    PickerTile(bool, usize),
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
    /// Title-bar ▸: the browser options menu (spec: ui/browser.md §4).
    BrowserMenu,
    /// The search field (focus it, §5).
    BrowserSearch,
    PlaylistArea,
    MixerArea,
    RackArea,
    Swallow,
    /// Piano roll: empty grid, a note (index into the pattern's notes), keyboard, velocity panel.
    PianoGrid,
    PianoNote(usize),
    PianoKeys,
    PianoVelocity,
    /// Event panel "Control" button and its menu entries (index into `Lane::ALL`).
    PianoLaneMenu,
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
    /// Playlist (spec: ui/playlist.md): title-bar tools, snap magnet, ▸ options menu, time ruler, marker lane,
    /// track header name area and mute LED, picker tabs and items, popup menu items (top level, submenu).
    PlTool(crate::playlist::PlTool),
    PlSnap,
    PlOptions,
    PlRuler,
    PlMarkers,
    PlHeader(u16),
    PlLed(u16),
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
#[derive(Clone, Copy, PartialEq)]
enum Damage {
    None,
    Rect(Rect),
    Full,
}

/// The hint panel (spec §2): its text changes whenever the hovered control does.
const HINT_RECT: Rect = Rect::new(73.0, 5.0, 289.0, 31.0);
/// The toolbar's output monitor (scope / analyzer and peak bars).
const MONITOR_RECT: Rect = Rect::new(880.0, 4.0, 168.0, 33.0);

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
}

impl Ui<'_> {
    /// Whether `action` is held down by the left button with the cursor still over `rect`.
    pub fn down(&self, rect: Rect, action: &Action) -> bool {
        self.pressed.as_ref() == Some(action) && rect.contains(self.mouse.0, self.mouse.1)
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
}

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
    Tempo { y0: f32, t0: f64 },
    /// Monitor debug panel slider.
    MonParam(usize),
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
    TimeSel { t0: u32 },
    /// Painting the event panel (note property or channel control points).
    Velocity { last_x: f32, last_y: f32 },
    /// Right-drag in a channel lane deletes its points (an OpenStudio choice, spec §7b).
    ControlErase { last_x: f32 },
    Knob { id: crate::knobs::KnobId, y0: f32, v0: f32 },
    /// Left button held on the piano keyboard: each key entered is auditioned (§8).
    PianoKeys { key: i32 },
    /// Scroll axes from a start mouse position: value = start + (mouse - m0) × ratio per axis
    /// (ratio < 0 grabs the content, as with the middle button).
    Scroll { m0: (f32, f32), axes: Vec<(Scroll, bool, f32, f32)> },
    /// Right-drag on a scroll bar: horizontal mouse movement scrolls the bar, vertical movement
    /// zooms its axis around the view centre (up = zoom in).
    BarZoom { t: Scroll, m0: (f32, f32), p0: f32, ratio: f32, z0: f32, center: f32, view: f32 },
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
}

pub struct State {
    pub project: Project,
    /// The toolbar's scope / analyzer and peak bars (fed from the audio tap in `App::tick`).
    pub monitor: crate::monitor::Monitor,
    /// Names of the hosted instruments offered after the built-in generators in the picker.
    pub hosted_generators: Vec<&'static str>,
    /// Names of the effect plugins found (mixer slots, picker effects side).
    pub hosted_effects: Vec<&'static str>,
    /// Mixer selection, slot requests, meters (spec: ui/mixer.md).
    pub mixer: crate::mixer::MixerUi,
    /// Rack rows played by a hosted plugin.
    pub hosted_rows: Vec<usize>,
    /// Requests for the plugin host, handled on the next tick: (row, available index) to create, row to open.
    pub pending_plugin: Option<(usize, usize)>,
    pub pending_open: Option<usize>,
    /// Samples to load into rows' Open Sampler instances on the next tick (browser send, drops on a rack row).
    pub sample_loads: Vec<(usize, String)>,
    pub file: Option<os_flp::FlpFile>,
    pub path: Option<PathBuf>,
    /// Open child windows, bottom to top.
    pub z: Vec<Win>,
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
    /// Values of knobs that are not backed by the project (`knobs.rs`).
    pub knobs: std::collections::HashMap<crate::knobs::KnobId, f32>,
    /// Piano roll event panel Control menu.
    pub lane_menu: bool,
    pub picker_open: bool,
    /// Settings window (OPTIONS > ... settings, F10 / F11).
    pub settings: views::settings::SettingsUi,
    /// Plugin picker category filter: (effects half?, category index).
    pub picker_cat: Option<(bool, usize)>,
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
    /// Directory listings of dropped folders, until reread.
    pub browser_dirs: std::cell::RefCell<std::collections::HashMap<PathBuf, std::rc::Rc<Vec<views::browser::Entry>>>>,
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
    pub tempo: f64,
    pub message: Option<String>,
    pub toggles: Vec<&'static str>,
    pub maximized: bool,
    pub piano: PianoState,
    pub playlist: crate::playlist::PlaylistState,
    pub pat_ui: crate::patterns::PatternUi,
    /// Playlist, Mixer, Piano roll (see `Win::slot`).
    pub wins: [WinState; 3],
    /// Size of the shell window in logical pixels (set each paint).
    pub screen: (f32, f32),
}

impl State {
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
        Rect::new(left, views::WORK_Y, (self.screen.0 - left).max(0.0), (self.screen.1 - views::WORK_Y).max(0.0))
    }

    /// The window's floating rect, kept at least its minimum size and inside the screen.
    pub fn float_rect(&self, w: Win) -> Rect {
        let Some(i) = w.slot() else { return Rect::new(0.0, 0.0, 0.0, 0.0) };
        let r = self.wins[i].rect;
        let (mw, mh) = w.min_size();
        let (sw, sh) = self.screen;
        let rw = r.w.min(sw).max(mw);
        let rh = r.h.min(sh - views::WORK_Y).max(mh);
        let x = r.x.min(sw - rw).max(0.0);
        let y = r.y.min(sh - rh).max(views::WORK_Y);
        Rect::new(x, y, rw, rh)
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
    /// Rects of last frame's overlays (menus, popups...): docked plugin windows are clipped under them.
    overlays: Vec<Rect>,
    /// Where the canvas sits in the main window's client area (physical px; the restored window's border).
    pub canvas_off: (i32, i32),
    /// The open popup belongs to this row's detached plugin window: it is shown lifted above that window.
    popup_lift: Option<usize>,
    pub bars: Vec<Bar>,
    pub hover: Option<Action>,
    /// Action under the cursor while the left button is held (pressed look of buttons).
    pub pressed: Option<Action>,
    hover_rect: Option<Rect>,
    damage: Damage,
    last_playing: bool,
    /// Typing keyboard (spec: ui/typing-keyboard.md): keys held now as (physical key, rack row,
    /// note key), the octave shift, and whether the transport has the project for live notes.
    typing_held: Vec<(KeyCode, usize, u16)>,
    pub typing_octave: i32,
    typing_synced: bool,
    /// Last title-bar press (window, time, position), for double-click maximize / restore.
    title_click: Option<(Win, std::time::Instant, (f32, f32))>,
    /// Last channel-rack press (action, time), for double clicks on slots and mixer boxes (§8, §12).
    rack_click: Option<(Action, std::time::Instant)>,
    pub hint: Cow<'static, str>,
    pub mouse: (f32, f32),
    pub drag: Option<Drag>,
    pub want_minimize: bool,
    pub want_maximize: bool,
    pub want_close: bool,
    pub want_drag_window: bool,
    /// Show the horizontal-resize cursor (hovering or dragging a note end).
    pub resize_cursor: bool,
    /// Modifier keys held (set by the shell).
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// Playlist regions from the last paint (grid, ruler, marker lane).
    pl_grid: Option<Rect>,
    /// Piano roll regions from the last paint (window, grid, velocity panel).
    piano_win: Option<Rect>,
    piano_grid: Option<Rect>,
    piano_panel: Option<Rect>,
    /// Sampler channel rows whose Open Sampler instance could not be created (not retried).
    sampler_failed: Vec<usize>,
    /// Mixer slots (`plugins::fx_key`) whose Open Parametric EQ 2 could not start (not retried).
    fx_failed: Vec<usize>,
    /// An opened project is still loading its samplers (spec: ui/project-loading.md): progress shows, input waits.
    loading: Option<crate::loading::Loading>,
}

/// Window-level requests the shell must carry out after an event.
impl App {
    pub fn new(path: Option<PathBuf>) -> App {
        let transport = Transport::new(os_engine::SampleLibrary::default_factory_dir());
        // Tests keep the old five-channel template; a new session starts with one Open Sampler channel.
        #[cfg(test)]
        let project = test_project();
        #[cfg(not(test))]
        let project = default_project();
        let mut app = App {
            plugins: crate::plugins::Plugins::default(),
            state: State {
                project,
                hosted_generators: Vec::new(),
                hosted_rows: Vec::new(),
                pending_plugin: None,
                pending_open: None,
                sample_loads: Vec::new(),
                file: None,
                path: None,
                z: vec![Win::Playlist, Win::ChannelRack],
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
                hosted_effects: Vec::new(),
                pattern: 1,
                song_mode: false,
                menu_open: None,
                knobs: Default::default(),
                lane_menu: false,
                picker_open: false,
                settings: Default::default(),
                picker_cat: None,
                playlist_scroll: (0.0, 0.0),
                playlist_bar_w: views::playlist::BAR_W,
                mixer_scroll: 0.0,
                rack_scroll: 0.0,
                browser_tab: 0,
                browser_open: views::browser::default_open(),
                browser_folders: crate::browser::load_folders(),
                browser_dirs: Default::default(),
                browser_scroll: 0.0,
                browser_sel: None,
                browser_drag: None,
                browser_search: String::new(),
                browser_search_focus: false,
                browser_opts: views::browser::DEFAULT_OPTS.iter().copied().collect(),
                browser_hist: Vec::new(),
                browser_hist_pos: 0,
                tempo: 130.0,
                message: None,
                monitor: crate::monitor::Monitor::with_cfg(load_monitor_cfg()),
                toggles: vec!["typing", "multilink"],
                maximized: true,
                piano: PianoState::default(),
                playlist: crate::playlist::PlaylistState::default(),
                pat_ui: Default::default(),
                wins: [
                    WinState { rect: Rect::new(693.0, 101.0, 1126.0, 378.0), maxed: true, mini: false },
                    WinState { rect: Rect::new(231.0, 565.0, 1552.0, 435.0), maxed: true, mini: false },
                    WinState { rect: Rect::new(480.0, 140.0, 1000.0, 640.0), maxed: true, mini: false },
                ],
                screen: (1920.0, 1080.0),
            },
            theme: load_theme(),
            transport,
            hits: Vec::new(),
            overlays: Vec::new(),
            canvas_off: (0, 0),
            popup_lift: None,
            bars: Vec::new(),
            hover: None,
            pressed: None,
            hover_rect: None,
            damage: Damage::Full,
            last_playing: false,
            typing_held: Vec::new(),
            typing_octave: 0,
            typing_synced: false,
            title_click: None,
            rack_click: None,
            hint: Cow::Borrowed(""),
            mouse: (-1.0, -1.0),
            drag: None,
            want_minimize: false,
            want_maximize: false,
            want_close: false,
            want_drag_window: false,
            resize_cursor: false,
            ctrl: false,
            shift: false,
            alt: false,
            pl_grid: None,
            piano_win: None,
            piano_grid: None,
            piano_panel: None,
            sampler_failed: Vec::new(),
            fx_failed: Vec::new(),
            loading: None,
        };
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
        let result = std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|b| os_flp::FlpFile::parse(&b).map_err(|e| e.to_string()))
            .and_then(|f| os_flp::project::load(&f).map(|p| (f, p)).map_err(|e| e.to_string()));
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
                crate::rack::from_model(s);
                s.file = Some(file);
                s.message = Some(format!("Opened {}", path.display()));
                s.path = Some(path);
                s.piano.reset();
                s.playlist.reset();
                s.piano.dirty = true; // live notes use the new project
                piano::center_keys(s, self.piano_grid.map_or(700.0, |g| g.h));
                self.loading = Some(crate::loading::Loading::start(&self.state.project));
                self.state.playlist.wave_wait = true;
                self.invalidate();
            }
            Err(e) => self.state.message = Some(format!("Could not open {}: {e}", path.display())),
        }
    }

    fn save(&mut self, choose: bool) {
        let s = &self.state;
        let target = if choose || s.path.is_none() {
            rfd::FileDialog::new().add_filter("FLP project", &["flp"]).save_file()
        } else {
            s.path.clone()
        };
        if let Some(target) = target {
            self.save_to(target);
        }
    }

    pub(crate) fn save_to(&mut self, target: PathBuf) {
        // Sampler channels keep their sample in Open Sampler: copy it to the channel so the FLP stores it.
        for row in self.state.hosted_rows.clone() {
            if self.state.project.channels.get(row).is_some_and(|c| c.kind.is_sampler()) {
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
        // Open Parametric EQ 2 instances hold their slot's settings: convert them back to the file's EQ state.
        for key in self.plugins.hosted.keys().copied().filter(|&k| k >= crate::plugins::FX_KEY).collect::<Vec<_>>() {
            let (Some((t, k)), Some(st)) = (crate::plugins::fx_of(key), self.plugins.eq_file_state(key)) else { continue };
            if let Some(slot) = self.state.project.mixer.iter_mut().find(|m| m.index == t).and_then(|m| m.slots[usize::from(k)].as_mut()) {
                if slot.name == crate::plugins::EQ_FILE_NAME {
                    slot.state = st;
                }
            }
        }
        let s = &mut self.state;
        s.project.tempo = Some(s.tempo);
        if s.hosted_rows.iter().any(|&r| s.project.channels.get(r).is_some_and(|c| c.kind == ChannelKind::Plugin)) {
            // spec: plugins/plugin-formats.md (open question: FLP storage of hosted plugins)
            s.message = Some("Save failed: projects with hosted plugin channels (Open Sampler, CLAP) cannot be saved to FLP yet".into());
            return;
        }
        // Lock, zip, filter groups and loop lengths live in the rack UI (spec: ui/channel-rack.md, Saving).
        crate::rack::to_model(s);
        // A file OpenStudio made before the project had a playlist has no arrangement block (FL's files always have
        // one): build it again from the model, which holds everything that file stored.
        let lacks_playlist = s.file.as_ref().and_then(|f| os_flp::project::load(f).ok()).is_some_and(|p| p.arrangements.is_empty())
            && !s.project.arrangements.is_empty();
        // A project that was not opened from a file gets a new FLP built from the model.
        let mut file = if s.file.is_none() || lacks_playlist {
            // The new file's blocks are the model's channels in order.
            for (i, c) in s.project.channels.iter_mut().enumerate() {
                c.rack.origin = Some(i as u16);
            }
            match os_flp::project::create(&s.project) {
                Ok(f) => f,
                Err(e) => {
                    s.message = Some(format!("Save failed: {e}"));
                    return;
                }
            }
        } else {
            s.file.clone().expect("checked")
        };
        // Deleted, cloned and moved channels are written by `apply` (os-flp `restructure`); it edits a copy so a
        // failed save leaves the loaded file as it was.
        let mut lost = os_flp::project::unsaved_parts(&s.project, &file);
        // Mixer parts the file format mapping does not write yet (spec: ui/mixer.md, OpenStudio status).
        let mut proj = s.project.clone();
        if let Some(n) = os_flp::project::load(&file).ok().map(|p| p.mixer.len()).filter(|&n| n > 0 && n < proj.mixer.len()) {
            proj.mixer.truncate(n);
            for m in proj.mixer.iter_mut() {
                m.routes.retain(|&r| usize::from(r) < n);
                m.send_levels.retain(|&(d, _)| usize::from(d) < n);
            }
            lost.push("added mixer tracks");
        }
        if self.plugins.hosted.keys().any(|&k| k >= crate::plugins::FX_KEY) {
            lost.push("mixer effect plugins");
        }
        if s.project.mixer.iter().any(|m| m.polarity || m.swap) {
            lost.push("polarity / swap buttons");
        }
        let result = os_flp::project::apply(&proj, &mut file)
            .map_err(|e| e.to_string())
            .and_then(|()| file.to_bytes().map_err(|e| e.to_string()))
            .and_then(|b| std::fs::write(&target, b).map_err(|e| e.to_string()));
        s.message = Some(match result {
            Ok(()) if lost.is_empty() => format!("Saved {}", target.display()),
            Ok(()) => format!("Saved {} (not stored yet: {})", target.display(), lost.join(", ")),
            Err(e) => format!("Save failed: {e}"),
        });
        if s.message.as_deref().is_some_and(|m| m.starts_with("Saved")) {
            // The saved file now holds the channels in model order.
            for (i, c) in s.project.channels.iter_mut().enumerate() {
                c.rack.origin = Some(i as u16);
            }
            s.file = Some(file);
            s.path = Some(target);
        }
    }

    /// Scans the plugin folders and offers the instruments found in the picker.
    pub fn scan_plugins(&mut self) {
        let mut p = crate::plugins::Plugins::scan();
        (p.owner, p.scale) = (self.plugins.owner, self.plugins.scale);
        self.plugins = p;
        // Picker names live for the whole run (`Plugin::name` is static).
        // Open Sampler is the picker's own "Sampler" tile (it plays every sampler channel), not a separate entry.
        self.state.hosted_generators = self
            .plugins
            .names()
            .into_iter()
            .filter(|n| n != crate::plugins::OPEN_SAMPLER)
            .map(|n| &*Box::leak(n.into_boxed_str()))
            .collect();
        self.state.hosted_effects = self.plugins.effects.iter().map(|a| &*Box::leak(a.desc.name.clone().into_boxed_str())).collect();
    }

    /// Drops every hosted instance (a new or opened project); `sync_samplers` hosts the new project's samplers.
    fn reset_hosted(&mut self) {
        self.plugins.clear();
        self.loading = None;
        self.state.playlist.wave_wait = false;
        self.state.hosted_rows.clear();
        self.sampler_failed.clear();
        self.fx_failed.clear();
        self.transport.set_instruments(Vec::new());
        self.transport.set_effects(Vec::new());
    }

    /// Every sampler channel is played and edited by Open Sampler (owner, 2026-10-10; spec: plugins/open-sampler.md §7):
    /// a channel without an instance gets one, started from the channel's sample and envelopes. Without Open Sampler
    /// (plugin not found) sampler channels keep the engine's own sample playback.
    /// With a `deadline` it stops after the instance that passes it; returns whether every sampler row is done.
    fn sync_samplers(&mut self, deadline: Option<std::time::Instant>) -> bool {
        let Some(index) = self.plugins.index_of(crate::plugins::OPEN_SAMPLER) else { return true };
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
                }
                Err(e) => {
                    self.sampler_failed.push(row);
                    self.state.message = Some(format!("Open Sampler: {e}"));
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

    /// A project is loading: the window shows the progress and takes no input (spec: ui/project-loading.md §2).
    pub fn loading(&self) -> bool {
        self.loading.is_some()
    }

    /// Mixer slots that hold the project file's parametric EQ are played and edited by Open Parametric EQ 2, started
    /// from the slot's saved state (spec: plugins/open-parametric-eq-2.md §8). Without the plugin the engine's own EQ
    /// plays them.
    fn sync_effects(&mut self) {
        use crate::plugins::{fx_key, EQ_FILE_NAME, OPEN_EQ};
        let Some(index) = self.plugins.effect_index(OPEN_EQ) else { return };
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
            let st = open_parametric_eq_2::params::plugin_state_from_fl(&fl);
            if let Err(e) = self.plugins.create_effect(t, k, index, Some(&st)) {
                self.fx_failed.push(fx_key(t, k));
                self.state.message = Some(format!("{OPEN_EQ}: {e}"));
            }
        }
        self.transport.set_effects(self.plugins.effects());
        self.state.piano.dirty = true;
    }

    /// Plugin host work on the UI thread: create / open requested plugins, apply plugin-window edits to the channel.
    fn plugin_tick(&mut self) {
        self.loading_tick();
        self.mixer_tick();
        self.sync_effects();
        for (row, path) in std::mem::take(&mut self.state.sample_loads) {
            // A row hosted this tick already started from its channel's sample.
            if self.state.hosted_rows.contains(&row) && self.plugins.sampler_path(row).as_deref() != Some(path.as_str()) {
                if !self.plugins.load_sample(row, &path) {
                    self.state.message = Some(format!("Open Sampler did not take {path}"));
                    self.invalidate();
                }
            }
        }
        if let Some((row, pick)) = self.state.pending_plugin.take() {
            // `pick` indexes the picker's hosted generators (Open Sampler is left out there).
            let index = self.state.hosted_generators.get(pick).and_then(|n| self.plugins.index_of(n)).unwrap_or(usize::MAX);
            match self.plugins.create(row, index) {
                Ok(name) => {
                    self.state.hosted_rows.push(row);
                    self.transport.set_instruments(self.plugins.instruments());
                    self.state.piano.dirty = true;
                    self.state.message = Some(format!("Added {name} (hosted CLAP plugin)"));
                    self.state.pending_open = Some(row);
                }
                Err(e) => self.state.message = Some(format!("Plugin: {e}")),
            }
            self.invalidate();
        }
        #[cfg(windows)]
        if let Some(row) = self.state.pending_open.take().filter(|r| self.state.hosted_rows.contains(r)) {
            if let Some(info) = channel_info(&self.state.project, row) {
                match self.plugins.open_window(row, info) {
                    Ok(()) => self.state.raise(Win::Hosted(row)),
                    Err(e) => {
                        self.state.message = Some(format!("Plugin: {e}"));
                        self.invalidate();
                    }
                }
            }
        }
        #[cfg(windows)]
        {
            // Keys pressed while a plugin window has the focus still play the typing keyboard.
            for k in crate::plugin_window::take_keys() {
                use winit::platform::scancode::PhysicalKeyExtScancode;
                if let winit::keyboard::PhysicalKey::Code(code) = winit::keyboard::PhysicalKey::from_scancode(k.scancode) {
                    // Space keeps playing / stopping the song (the typing keyboard does not use it).
                    if !self.typing_key(code, k.pressed, k.ctrl, k.alt) && k.pressed && code == winit::keyboard::KeyCode::Space {
                        self.key(&winit::keyboard::Key::Named(winit::keyboard::NamedKey::Space), k.ctrl, k.alt);
                    }
                }
            }
            let (project, mixer) = (&self.state.project, &self.state.mixer);
            let requests = self.plugins.idle(|row| if row >= crate::plugins::FX_KEY { crate::mixer::fx_info(project, mixer, row) } else { channel_info(project, row) });
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
                        continue;
                    }
                    Request::Menu(row, x, y) => {
                        let Some((_, detached)) = self.plugins.window_of(row) else { continue };
                        let (lx, ly) = self.screen_to_canvas(x, y);
                        self.state.playlist.popup = Some(crate::playlist::plugin_window_menu(row, detached, lx, ly));
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
                            if !self.typing_synced {
                                self.typing_synced = true;
                                self.transport.update(&crate::rack::play_project(&self.state));
                            }
                            self.transport.note_on(row, u16::from(key), vel);
                        } else {
                            self.transport.note_off(row, u16::from(key));
                        }
                        continue;
                    }
                }
                self.state.piano.dirty = true;
                self.invalidate();
            }
            self.clip_docked();
        }
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

    /// After a paint: a popup opened from a detached plugin window is shown above that window (it is painted in the
    /// main window, which the detached window covers).
    #[cfg(windows)]
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
    /// windows above them and the overlays (spec: ui/plugin-window.md "Docking").
    #[cfg(windows)]
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
        self.damage = match self.damage {
            Damage::Full => Damage::Full,
            Damage::None => Damage::Rect(r),
            Damage::Rect(o) => Damage::Rect(o.union(&r)),
        };
    }

    pub fn has_damage(&self) -> bool {
        self.damage != Damage::None
    }

    /// Take the pending damage: `None` = nothing to paint, `Some(None)` = whole frame.
    pub fn take_damage(&mut self) -> Option<Option<Rect>> {
        match std::mem::replace(&mut self.damage, Damage::None) {
            Damage::None => None,
            Damage::Full => Some(None),
            Damage::Rect(r) => Some(Some(r)),
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

    /// The monitor is moving (a signal or a decay): the shell should tick at animation rate.
    pub fn animating(&self) -> bool {
        self.transport.playing() || self.state.monitor.active() || self.state.mixer.meters.iter().any(|m| m.active()) || self.loading.is_some()
    }

    /// Periodic housekeeping; returns whether a repaint is needed.
    pub fn tick(&mut self) -> bool {
        self.plugin_tick();
        let playing = self.transport.playing();
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
        // Mixer meters (spec: ui/mixer.md §9): fall back after playback stops.
        let peaks = self.transport.take_track_peaks();
        if crate::mixer::feed_meters(&mut self.state, &peaks) && self.state.shown(Win::Mixer) {
            self.invalidate();
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
        if playing || playing != self.last_playing {
            self.last_playing = playing;
            self.invalidate();
        }
        self.has_damage()
    }

    pub fn paint(&mut self, canvas: &mut Canvas, damage: Option<Rect>) {
        canvas.begin_frame(damage);
        self.state.screen = canvas.size();
        self.hits.clear();
        self.bars.clear();
        let title = self.title();
        let mut ui = Ui { c: canvas, t: &self.theme.colors, hits: &mut self.hits, bars: &mut self.bars, hover: &self.hover, pressed: &self.pressed, mouse: self.mouse, win: None };
        let info = views::FrameInfo {
            title,
            hint: self.hint.clone(),
            playing: self.transport.playing(),
            paused: self.transport.paused(),
            tick: self.transport.tick(),
            song: self.transport.song(),
            peaks: self.transport.peaks(),
            sounding: if self.transport.playing() { self.transport.sounding() } else { Vec::new() },
        };
        let overlays = views::paint(&mut ui, &self.state, &info);
        self.overlays = self.hits[overlays.min(self.hits.len())..].iter().map(|h| h.rect).collect();
        // Remember where the piano roll's regions are, for mouse mapping and damage.
        let find = |a: Action| self.hits.iter().find(|h| h.action == a).map(|h| h.rect);
        self.piano_grid = find(Action::PianoGrid);
        self.piano_panel = find(Action::PianoVelocity);
        self.pl_grid = find(Action::PlaylistArea);
        self.piano_win = match (self.piano_grid, self.piano_panel) {
            // The window below its title bar: overview, ruler, keyboard, grid, velocity panel.
            (Some(g), Some(p)) => Some(Rect::new(g.x - 73.0, g.y - 41.0, g.w + 96.0, p.bottom() - g.y + 41.0)),
            _ => None,
        };
        if let Some(l) = &self.loading {
            crate::loading::paint(canvas, &self.theme.colors, l);
        }
    }

    /// Repaint the piano roll body (note edits change the grid, overview and velocity panel).
    fn damage_piano(&mut self) {
        match self.piano_win {
            Some(r) => self.damage_rect(r),
            None => self.invalidate(),
        }
    }

    fn piano_geom(&self) -> Option<Geom> {
        self.piano_grid.map(|g| Geom::new(&self.state, g))
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
        let mut p = self.state.project.clone();
        p.tempo = Some(self.state.tempo);
        if p.arrangement().is_none_or(|a| a.clips.is_empty()) {
            self.state.message = Some("The playlist is empty: nothing to play in SONG mode".into());
            return self.invalidate();
        }
        self.state.song_mode = true;
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
        if ext == "flp" {
            return self.open(path);
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
        let tick = crate::playlist::snap_down(s, &g, g.tick_at(x), self.alt);
        let name = path.to_string_lossy().into_owned();
        s.message = Some(match crate::playlist::add_audio_clip(s, &name, track, tick) {
            Some(_) => format!("Added audio clip {}", path.display()),
            None => format!("Could not read {} as audio", path.display()),
        });
        self.invalidate();
    }

    /// A sample dropped on the channel rack at window y `y` (spec: ui/channel-rack.md, New project / drops): onto a
    /// sampler row it replaces that channel's sample (Open Sampler loads it); elsewhere it adds an Open Sampler channel
    /// named after the file.
    pub(crate) fn sample_to_rack(&mut self, path: String, y: f32) {
        let s = &mut self.state;
        let stem = std::path::Path::new(&path).file_stem().map(|n| n.to_string_lossy().into_owned());
        let row = match views::rack::row_at(s, y).filter(|&r| s.project.channels[r].kind.is_sampler()) {
            Some(r) => {
                s.sample_loads.push((r, path.clone()));
                r
            }
            None => add_channel(&mut s.project),
        };
        let ch = &mut s.project.channels[row];
        ch.name = stem.clone();
        ch.sample_path = Some(path);
        s.selected_channel = row;
        s.rack.select_only(row);
        s.piano.dirty = true;
        s.message = Some(format!("{} loaded in Open Sampler", stem.unwrap_or_default()));
        self.invalidate();
    }

    /// A browser sample released over a hosted plugin window (it sits above the main window, which keeps the mouse
    /// capture during the drag): the plugin gets the file as if it came from Explorer. Returns whether it was one.
    #[cfg(windows)]
    pub(crate) fn sample_to_plugin_window(&mut self, path: &str) -> bool {
        use windows_sys::Win32::Foundation::POINT;
        let mut pt = POINT { x: 0, y: 0 };
        if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt) } == 0 {
            return false;
        }
        let Some(row) = self.plugins.row_at_screen(pt.x, pt.y) else { return false };
        let sampler = self.state.project.channels.get(row).is_some_and(|c| c.kind.is_sampler());
        let name = std::path::Path::new(path).file_stem().map(|n| n.to_string_lossy().into_owned());
        if self.plugins.drop_file(row, pt.x, pt.y, path, sampler) {
            if let Some(ch) = self.state.project.channels.get_mut(row).filter(|_| sampler) {
                ch.name = name.clone();
                ch.sample_path = Some(path.to_string());
            }
            self.state.message = Some(format!("{} loaded in the plugin", name.unwrap_or_default()));
        } else {
            self.state.message = Some("This plugin does not take dropped files".into());
        }
        self.invalidate();
        true
    }

    /// Load a theme: a `.flstheme` (spec ui/flstheme.md) or an OpenStudio `theme.txt`. A `.flstheme` is also
    /// remembered as `%APPDATA%/OpenStudio/current.flstheme` so the next start uses it.
    pub fn load_theme_file(&mut self, path: &std::path::Path) {
        self.load_theme_file_ex(path, true)
    }

    /// `load_theme_file`; `remember` = keep it for the next start.
    pub fn load_theme_file_ex(&mut self, path: &std::path::Path, remember: bool) {
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        match theme_from_file(path) {
            Ok(t) => {
                self.theme = t;
                os_theme::set_current(&self.theme.colors);
                if remember && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("flstheme")) {
                    if let Some(d) = std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("OpenStudio")) {
                        let _ = std::fs::create_dir_all(&d);
                        let keep = d.join("current.flstheme");
                        if keep.as_path() != path {
                            let _ = std::fs::copy(path, keep);
                        }
                    }
                }
                self.state.message = Some(format!("Theme: {name}"));
            }
            Err(e) => self.state.message = Some(format!("Could not load theme {name}: {e}")),
        }
        self.invalidate();
    }

    /// Open the settings window on `tab` (spec: ui/settings.md §1); reads the output devices and theme files.
    pub fn open_settings(&mut self, tab: usize) {
        use cpal::traits::{DeviceTrait, HostTrait};
        let st = &mut self.state.settings;
        if st.tab != tab {
            st.scroll = 0.0;
        }
        st.open = true;
        st.tab = tab;
        let host = cpal::default_host();
        let name = |d: &cpal::Device| d.description().map(|d| d.name().to_string()).unwrap_or_else(|_| "(unknown device)".into());
        st.device = host.default_output_device().map(|d| name(&d)).unwrap_or_else(|| "(no output device)".into());
        st.devices = host.output_devices().map(|ds| ds.map(|d| name(&d)).collect()).unwrap_or_default();
        // Built-in presets (spec: ui/theme-presets.md), then .flstheme files in %APPDATA%/OpenStudio/themes.
        st.themes = os_theme::presets::PRESETS.iter().map(|(n, t)| (n.to_string(), t.to_string())).collect();
        if let Some(d) = std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("OpenStudio").join("themes")) {
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

    /// Theme import (spec: ui/flstheme.md): pick a .flstheme, starting in FL's theme folder when installed.
    fn theme_dialog(&mut self) {
        let mut dlg = rfd::FileDialog::new().add_filter("Theme", &["flstheme", "txt"]).set_title("Load theme");
        let fl = std::path::Path::new(r"C:\Program Files\Image-Line\FL Studio 2026\Artwork\Themes");
        if fl.is_dir() {
            dlg = dlg.set_directory(fl);
        }
        if let Some(p) = dlg.pick_file() {
            self.load_theme_file(&p);
        }
    }

    fn settings_act(&mut self, a: views::settings::SAct) {
        use views::settings::SAct;
        let st = &mut self.state.settings;
        match a {
            SAct::Body => {}
            SAct::Close => st.open = false,
            SAct::Tab(i) => {
                st.tab = i;
                st.scroll = 0.0;
            }
            SAct::Check(k) => {
                if !st.flipped.remove(k) {
                    st.flipped.insert(k);
                }
            }
            SAct::Group(k) => {
                if !st.folded.remove(k) {
                    st.folded.insert(k);
                }
            }
            SAct::Preset(0) => {
                st.preset = 0;
                self.theme = os_theme::Theme::default();
                os_theme::set_current(&self.theme.colors);
                if let Some(d) = std::env::var_os("APPDATA") {
                    let _ = std::fs::remove_file(PathBuf::from(d).join("OpenStudio").join("current.flstheme"));
                }
                self.state.message = Some("Theme: default".into());
            }
            SAct::Preset(i) => {
                if let Some((name, text)) = st.themes.get(i - 1).cloned() {
                    st.preset = i;
                    let fl = os_theme::FlTheme::parse(&text);
                    self.theme = os_theme::Theme::new(&name, os_theme::fl::palette(&fl), os_theme::Adjust::default());
                    os_theme::set_current(&self.theme.colors);
                    // Remembered like an imported theme (loaded at the next start).
                    if let Some(d) = std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("OpenStudio")) {
                        let _ = std::fs::create_dir_all(&d);
                        let _ = std::fs::write(d.join("current.flstheme"), &text);
                    }
                    self.state.message = Some(format!("Theme: {name}"));
                }
            }
            SAct::Button("Reset") => self.settings_act(SAct::Preset(0)),
            SAct::Button("Load theme...") => self.theme_dialog(),
            SAct::Button("Manage plugins") => {
                self.scan_plugins();
                self.state.message = Some(format!("Plugin scan: {} instruments found", self.state.hosted_generators.len()));
            }
            SAct::Button(b) => self.state.message = Some(format!("{b}: not available yet")),
            SAct::Combo(k) => self.state.message = Some(format!("{k}: not available yet")),
        }
        self.invalidate();
    }

    /// The automation target, name and current value of a control, for "Create automation clip" (§8).
    fn automation_target(&self, a: &Action) -> Option<(u32, String, f64)> {
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
            Action::Tempo => (target::TEMPO, "Tempo".into(), ((s.tempo - 60.0) / 120.0).clamp(0.0, 1.0)),
            _ => return None,
        })
    }

    pub(crate) fn top_hit(&self, x: f32, y: f32) -> Option<&Hit> {
        self.hits.iter().rev().find(|h| h.rect.contains(x, y))
    }

    /// Mouse moved; returns whether a repaint is needed.
    pub fn mouse_move(&mut self, x: f32, y: f32) -> bool {
        self.mouse = (x, y);
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
                    self.invalidate();
                }
                Drag::Scroll { m0, axes } => {
                    for (t, vertical, p0, ratio) in axes {
                        let d = if vertical { y - m0.1 } else { x - m0.0 };
                        self.scroll_set(t, p0 + d * ratio);
                    }
                    self.invalidate();
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
                        let t = g.snap_down(g.tick_at(x) + g.step as f32 / 2.0);
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
                        self.state.piano.marker = g.snap_down(g.tick_at(x) + g.step as f32 / 2.0);
                    }
                    self.damage_piano();
                }
                Drag::TimeSel { t0 } => {
                    if let Some(g) = self.piano_geom() {
                        let t = g.snap_down(g.tick_at(x) + g.step as f32 / 2.0);
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
                            self.hold_key(Some(k), true);
                        }
                    }
                }
                Drag::Window { win, dx, dy } => {
                    let old = self.win_bounds(win);
                    let pos = ((x - dx).max(0.0), (y - dy).max(views::WORK_Y));
                    let (sw, sh) = self.state.screen;
                    match win {
                        Win::ChannelRack => self.state.rack_pos = pos,
                        Win::Plugin => self.state.plugin_pos = pos,
                        w => {
                            if let Some(i) = w.slot() {
                                let r = &mut self.state.wins[i].rect;
                                r.x = pos.0.min(sw - 100.0).max(0.0);
                                r.y = pos.1.min(sh - 30.0).max(views::WORK_Y);
                            }
                        }
                    }
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
                        r.h = (r0.h - dy).max(mh).min((r0.bottom() - views::WORK_Y).max(mh));
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
                    let v = (v0 as i32 + ((y0 - y) / crate::rack::LEN_DRAG).round() as i32).clamp(0, crate::rack::MAX_STEPS as i32) as u32;
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
                Drag::PatternBox { y0, p0 } => {
                    let (y0, p0): (f32, u16) = (y0.to_owned(), p0.to_owned());
                    let steps = ((y0 - y) / crate::patterns::DRAG_STEP).trunc() as i32;
                    self.state.pattern = (i32::from(p0) + steps).clamp(1, i32::from(crate::patterns::MAX)) as u16;
                    self.invalidate();
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
                    self.damage_rect(HINT_RECT);
                }
                Drag::MonParam(i) => {
                    self.set_mon_param(i, x);
                    return true;
                }
                Drag::Tempo { y0, t0 } => {
                    let t = (t0 + f64::from(y0 - y).round()).clamp(10.0, 522.0);
                    if self.state.tempo != t {
                        self.state.tempo = t;
                        self.state.piano.dirty = true;
                    }
                    self.hint = Cow::Owned(format!("Tempo {t:.3} BPM"));
                    self.damage_rect(Rect::new(380.0, 0.0, 200.0, 83.0));
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
                let auto = hit.filter(|&i| matches!(crate::playlist::clips(s)[i].source, os_model::ClipSource::Channel(c) if s.project.automation(c).is_some()));
                if auto != self.state.playlist.hover_clip {
                    self.state.playlist.hover_clip = auto;
                    self.invalidate();
                }
                if let Some(i) = hit {
                    let r = pg.clip_rect(&crate::playlist::clips(&self.state)[i]);
                    let z = crate::playlist::EDGE.min(r.w / 4.0);
                    let tool_resizes = matches!(self.state.playlist.tool, crate::playlist::PlTool::Draw | crate::playlist::PlTool::Paint | crate::playlist::PlTool::Select);
                    resize |= tool_resizes && (x <= r.x + z || x >= r.right() - z);
                    if hint.is_empty() || hint == "Playlist" {
                        hint = Cow::Owned(crate::playlist::source_name(&self.state, crate::playlist::clips(&self.state)[i].source));
                    }
                }
            }
        } else if self.state.playlist.hover_clip.is_some() {
            self.state.playlist.hover_clip = None;
            self.invalidate();
        }
        let changed = hover != self.hover || hint != self.hint || resize != self.resize_cursor;
        self.resize_cursor = resize;
        if changed && self.state.picker_open {
            self.invalidate();
        }
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

    pub fn mouse_down(&mut self) {
        self.invalidate();
        let (x, y) = self.mouse;
        let action = self.top_hit(x, y).map(|h| h.action.clone());
        self.pressed = action.clone();
        // Playlist popup menus close on any click outside them; a click elsewhere accepts an open text box.
        if self.state.playlist.popup.is_some() && !matches!(action, Some(Action::PlPopup(_) | Action::PlPopupSub(_))) {
            self.state.playlist.popup = None;
            return;
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
        // Clicking anywhere but the search field leaves it (spec: ui/browser.md §5).
        if action != Some(Action::BrowserSearch) {
            self.state.browser_search_focus = false;
        }
        if self.state.lane_menu && !matches!(action, Some(Action::PianoLanePick(_))) {
            self.state.lane_menu = false;
            return;
        }
        // Any click outside an open menu closes it.
        if self.state.menu_open.is_some() && !matches!(action, Some(Action::MenuItem(..)) | Some(Action::Menu(_))) {
            self.state.menu_open = None;
            return;
        }
        self.raise_under_mouse();
        if let Some(a) = action {
            self.act(a);
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
        }
    }

    /// Area a floating window paints (for damage).
    fn win_bounds(&self, w: Win) -> Rect {
        match w {
            Win::Plugin => views::channel_win::bounds(&self.state),
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

    /// True while a right-drag on a scroll bar runs: the shell hides the cursor and keeps it
    /// where the drag started, feeding relative movement.
    pub fn cursor_locked(&self) -> bool {
        matches!(self.drag, Some(Drag::BarZoom { .. } | Drag::Knob { .. }))
    }

    fn bar(&self, t: Scroll) -> Option<Bar> {
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

    /// Press on a scroll bar. Left: a press off the thumb first centres the thumb on the mouse,
    /// then dragging scrolls. Right (two-dimensional views): drag scrolls both axes.
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
        self.drag = Some(if right {
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
        self.raise_under_mouse();
        let (x, y) = self.mouse;
        let action = self.top_hit(x, y).map(|h| h.action.clone());
        if let Some(Action::Mix(h)) = action {
            if crate::mixer::middle(self, h) {
                return self.invalidate();
            }
        }
        if let Some(Action::Knob(id)) = action {
            self.state.set_knob(id, id.default_value());
            self.hint = Cow::Owned(self.state.knob_text(id));
            self.invalidate();
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
        let (x, y) = self.mouse;
        let action = self.top_hit(x, y).map(|h| h.action.clone());
        if self.state.menu_open.is_some() {
            self.state.menu_open = None;
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
            Some(Action::BrowserRow(i)) => {
                if self.browser_remove_root(*i) {
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
        // Knobs, faders and the tempo: "Create automation clip" (spec: ui/playlist.md §8).
        if let Some(a) = action.as_ref() {
            if let Some((tgt, name, v)) = self.automation_target(a) {
                self.state.playlist.popup = Some(crate::playlist::knob_menu(tgt, name, v, x, y));
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
        if action == Some(Action::PianoRuler) {
            if let Some(g) = self.piano_geom() {
                let t = g.snap_down(g.tick_at(x) + g.step as f32 / 2.0);
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
                    piano::snapshot(&mut self.state);
                    self.drag = Some(Drag::NoteErase);
                    self.erase_at(x, y);
                }
            }
        }
    }

    pub fn mouse_up(&mut self) {
        if self.pressed.take().is_some() {
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
                    let t = g.snap_down(g.tick_at(self.mouse.0) + g.step as f32 / 2.0);
                    if piano::slice(&mut self.state, t) > 0 {
                        self.state.piano.dirty = true;
                    }
                }
            }
            Some(Drag::ZoomBox { x0, y0 }) => self.zoom_to(x0, y0),
            Some(Drag::Marker) if self.transport.playing() && !self.transport.song() => {
                self.start_playback();
            }
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

    /// Move (`resize == false`) or resize the dragged notes from their originals, snapped to steps.
    fn drag_notes(&mut self, x0: f32, y0: f32, orig: &[(usize, Note)], resize: bool) {
        let Some(g) = self.piano_geom() else { return };
        let (x, y) = self.mouse;
        let mut dt = g.snap_delta((x - x0) / g.ppt);
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
                    let len = i64::from(o.length.max(g.step)) + dt;
                    n.length = len.max(i64::from(g.step)) as u32;
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
            self.hold_key(held, true);
        }
    }

    /// Set the key held with the mouse (lit on the keyboard), auditioning it if `play`.
    fn hold_key(&mut self, key: Option<i32>, play: bool) {
        if self.state.piano.held == key {
            return;
        }
        self.state.piano.held = key;
        if let (Some(k), true, Some(ch)) = (key, play, piano::channel(&self.state)) {
            self.preview(ch, k);
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
    /// direction), skipping slots that already hold a note of this channel on that key.
    fn paint_notes(&mut self, key: i32, start: u32, len: u32) {
        let (Some(g), Some(ch)) = (self.piano_geom(), piano::channel(&self.state)) else { return };
        let len = len.max(1);
        let t = g.tick_at(self.mouse.0).max(0.0) as i64;
        let k = (t - i64::from(start)).div_euclid(i64::from(len));
        let (a, b) = (k.min(0), k.max(0));
        let mut added = Vec::new();
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
                added.push(i);
            }
        }
        if !added.is_empty() {
            self.state.piano.dirty = true;
            let notes = piano::notes(&self.state);
            let rects: Vec<Rect> = added.iter().filter_map(|&i| g.note_rect(&notes[i])).collect();
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
        g.snap_down(tick)
    }

    /// Delete the channel lane's control points between two x positions.
    fn erase_control_points(&mut self, last_x: f32, x: f32) {
        let (Some(g), Some(panel)) = (self.piano_geom(), self.piano_panel) else { return };
        let lane = self.state.piano.lane;
        let (Some(ch), Some(param)) = (piano::channel(&self.state), lane.param()) else { return };
        let (a, b) = (last_x.min(x), last_x.max(x));
        let from = g.snap_down(g.tick_at(a).max(0.0));
        let to = g.snap_down(g.tick_at(b).max(0.0));
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
            let t0 = g.snap_down(g.tick_at(last_x).max(0.0));
            let t1 = g.snap_down(g.tick_at(x).max(0.0));
            let (lo, hi) = (t0.min(t1), t0.max(t1));
            let step = g.step.max(1);
            // A point is set when the cursor enters a snap step (its value is the path height at the
            // step's start, as in FL's captured ramps) and on the press; staying inside a step keeps it.
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
        let (a, b) = (last_x.min(x) - 3.0, last_x.max(x) + 3.0);
        let ch = piano::channel(&self.state);
        let p = piano::pattern_mut(&mut self.state);
        let mut span: Option<(f32, f32)> = None;
        for n in p.notes.iter_mut().filter(|n| Some(n.channel) == ch) {
            let nx = g.x_of(n.position as f32);
            if nx >= a && nx <= b {
                lane.set(n, v);
                let end = g.x_of((n.position + n.length.max(g.step)) as f32);
                span = Some(span.map_or((nx, end), |(l, r)| (l.min(nx), r.max(end))));
            }
        }
        // Only the panel columns of the changed lollipops (the hint is left alone: a damage
        // union with the hint panel would cover most of the window).
        if let Some((l, r)) = span {
            self.damage_rect(Rect::new(l - 6.0, panel.y, r - l + 12.0, panel.h).intersect(&panel.outset(1.0)));
        }
    }

    pub fn wheel(&mut self, dx: f32, dy: f32, shift: bool, ctrl: bool) {
        self.invalidate();
        let (x, y) = self.mouse;
        let area = self.top_hit(x, y).map(|h| h.action.clone());
        if matches!(area, Some(Action::BrowserList | Action::BrowserRow(_) | Action::BrowserTab(_))) {
            return self.browser_wheel(dy + dx);
        }
        if matches!(area, Some(Action::Settings(_))) {
            let s = &self.state;
            let max = (views::settings::content_h(s) - views::settings::page_h(s)).max(0.0);
            self.state.settings.scroll = (self.state.settings.scroll - (dy + dx) * 63.0).min(max).max(0.0);
            return;
        }
        if matches!(area, Some(Action::PlPick(_) | Action::PlPickList)) {
            self.scroll_set(Scroll::PlPick, self.scroll_get(Scroll::PlPick) - (dy + dx) * 31.0);
            return;
        }
        if matches!(area, Some(Action::PatternNext)) {
            let d = if dy + dx > 0.0 { 1 } else { -1 };
            self.state.pattern = (i32::from(self.state.pattern.max(1)) + d).clamp(1, i32::from(crate::patterns::MAX)) as u16;
            return;
        }
        if let Some(Action::Scrollbar(t)) = area {
            let step = if t == Scroll::PianoY { views::piano::ROW * 3.0 } else { 60.0 };
            self.scroll_set(t, self.scroll_get(t) - (dy + dx) * step);
            return;
        }
        if matches!(area, Some(Action::PianoGrid | Action::PianoNote(_) | Action::PianoKeys | Action::PianoVelocity)) {
            let Some(g) = self.piano_geom() else { return };
            let p = &mut self.state.piano;
            if ctrl {
                // Zoom time around the mouse.
                let tick = g.tick_at(x);
                p.px_per_beat = (p.px_per_beat * 1.25f32.powf(dy + dx)).clamp(6.0, 1600.0);
                let ppt = p.px_per_beat / f32::from(self.state.project.ppq.max(4));
                p.scroll_x = (tick - (x - g.grid.x) / ppt).max(0.0);
            } else if shift {
                p.scroll_x = (p.scroll_x - (dy + dx) * 120.0 / g.ppt).max(0.0);
            } else {
                p.scroll_y -= dy * views::piano::ROW * 3.0;
                p.scroll_x = (p.scroll_x - dx * 120.0 / g.ppt).max(0.0);
            }
            piano::clamp_scroll(&mut self.state, g.grid.h);
            return;
        }
        if let Some(Action::Knob(id)) = area {
            let mut v = self.state.knob(id);
            if id == crate::knobs::KnobId::MasterPitch {
                v = (v * 24.0).round() / 24.0; // wheel steps from the nearest semitone
            }
            let v = v + dy.signum() * id.notch() * if dy == 0.0 { 0.0 } else { 1.0 };
            self.state.set_knob(id, v);
            self.hint = Cow::Owned(self.state.knob_text(id));
            return;
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
                return;
            }
            Some(Action::RackLen) => {
                let v = (crate::rack::len_value(&self.state) as i32 + d).clamp(0, crate::rack::MAX_STEPS as i32) as u32;
                crate::rack::set_len(&mut self.state, (v > 0).then_some(v));
                return;
            }
            Some(Action::RackLoopBox(i)) => {
                let v = (crate::rack::row_len(&self.state, i) as i32 + d).clamp(1, crate::rack::MAX_STEPS as i32) as u32;
                self.state.rack.loop_len.insert(i, v);
                self.state.piano.dirty = true;
                return;
            }
            _ => {}
        }
        let rack_area = self.top_hit(x, y).is_some_and(|h| h.win == Some(Win::ChannelRack));
        if rack_area {
            let (rx, ry) = if shift { (dy + dx, 0.0) } else { (dx, dy) };
            self.scroll_set(Scroll::RackY, self.scroll_get(Scroll::RackY) - ry * views::rack::ROW_H);
            self.scroll_set(Scroll::RackX, self.scroll_get(Scroll::RackX) - rx * 4.0 * views::rack::STEP_W);
            return;
        }
        let s = &mut self.state;
        let (dx, dy) = if shift { (dy, dx) } else { (dx, dy) };
        match area {
            Some(Action::Fader(_) | Action::Mix(_) | Action::MixerArea) => {
                s.mixer_scroll = (s.mixer_scroll - (dy + dx) * 44.0).max(0.0);
            }
            Some(Action::PlaylistArea | Action::PlRuler | Action::PlMarkers) if ctrl => {
                let gx = self.pl_grid.map_or(481.0, |g| g.x);
                let bar = (s.playlist_scroll.0 + x - gx) / s.playlist_bar_w;
                s.playlist_bar_w = (s.playlist_bar_w * 1.25f32.powf(dy + dx)).clamp(4.0, 600.0);
                s.playlist_scroll.0 = (bar * s.playlist_bar_w - (x - gx)).max(0.0);
            }
            // Shift+wheel over the track headers moves the selected track (spec: ui/playlist.md §6).
            Some(Action::PlHeader(_) | Action::PlLed(_)) if shift => {
                let d = if dx + dy > 0.0 { -1 } else { 1 };
                crate::playlist::snapshot(s);
                crate::playlist::shift_tracks(s, d);
            }
            Some(Action::PlHeader(_) | Action::PlLed(_)) => {
                s.playlist_scroll.1 = (s.playlist_scroll.1 - dy * 48.0).max(0.0);
            }
            Some(Action::PlaylistArea) => {
                s.playlist_scroll.0 = (s.playlist_scroll.0 - dx * 60.0).max(0.0);
                s.playlist_scroll.1 = (s.playlist_scroll.1 - dy * 48.0).max(0.0);
            }
            _ => {}
        }
    }

    pub fn key(&mut self, key: &winit::keyboard::Key, ctrl: bool, alt: bool) {
        self.invalidate();
        use winit::keyboard::{Key, NamedKey};
        if crate::patterns::key(&mut self.state, key, ctrl) {
            self.rack_pending();
            return;
        }
        if self.browser_key(key, ctrl, alt) {
            return;
        }
        // The channel rack's keys while it is in front (spec: ui/channel-rack.md §20).
        let s = &self.state;
        if (s.z.last() == Some(&Win::ChannelRack) || s.rack.fill.is_some())
            && !s.picker_open
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
        if s.z.last() == Some(&Win::Mixer) && !s.picker_open && s.menu_open.is_none() && !s.settings.open && s.playlist.edit.is_none() && s.playlist.popup.is_none() && !ctrl && crate::mixer::key(self, key, alt) {
            return;
        }
        // The playlist's editing keys when it is in front (spec: ui/playlist.md §5).
        if self.pl_focus() && crate::playlist::key(self, key, ctrl, alt) {
            return;
        }
        if let Some(cmd) = pattern_shortcut(key, ctrl, self.shift) {
            let at = crate::patterns::box_at_mouse(self.mouse);
            if let Some(m) = crate::patterns::run(&mut self.state, cmd, at) {
                self.state.message = Some(m);
            }
            return;
        }
        match key {
            Key::Named(NamedKey::Space) => self.act(if self.transport.playing() { Action::Stop } else { Action::Play }),
            Key::Named(NamedKey::F5) => self.act(Action::ToggleWin(Win::Playlist)),
            Key::Named(NamedKey::F6) => self.act(Action::ToggleWin(Win::ChannelRack)),
            Key::Named(NamedKey::F7) => self.act(Action::ToggleWin(Win::PianoRoll)),
            Key::Named(NamedKey::F8) if !alt && !ctrl => {
                self.state.picker_open = !self.state.picker_open;
                self.state.picker_cat = None;
            }
            Key::Named(NamedKey::F8) if alt => self.act(Action::ToggleWin(Win::Browser)),
            Key::Named(NamedKey::F9) => self.act(Action::ToggleWin(Win::Mixer)),
            Key::Named(NamedKey::F10) => self.open_settings(views::settings::MIDI),
            Key::Named(NamedKey::F11) => self.open_settings(views::settings::INFO),
            // Monitor debug panel (views/mondebug.rs) parked 2026-10-09: its values are now the defaults. Uncomment to tune again.
            // Key::Named(NamedKey::F12) => self.state.monitor.debug = !self.state.monitor.debug,
            Key::Named(NamedKey::Escape) if self.state.settings.open => self.state.settings.open = false,
            Key::Named(NamedKey::Escape) => {
                self.state.menu_open = None;
                self.state.picker_open = false;
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
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("z") => self.act(Action::PianoUndo),
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("t") => self.toggle_typing(),
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
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("b") && self.state.shown(Win::PianoRoll) => {
                let s = &mut self.state;
                let idx = piano::targets(s);
                let notes = piano::notes(s);
                let clip: Vec<Note> = idx.iter().filter_map(|&i| notes.get(i).copied()).collect();
                let start = clip.iter().map(|n| n.position).min();
                let end = clip.iter().map(|n| n.position + piano::shown_len(s, n)).max();
                if let (Some(a), Some(b)) = (start, end) {
                    piano::paste(s, &clip, b.max(a + 1));
                    s.piano.dirty = true;
                }
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
                    piano::snapshot(s);
                    piano::delete(s, sel);
                }
            }
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("o") => self.act(Action::MenuItem(0, views::menus::FILE_OPEN)),
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("s") => self.act(Action::MenuItem(0, views::menus::FILE_SAVE)),
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("n") => self.act(Action::MenuItem(0, views::menus::FILE_NEW)),
            _ => {}
        }
    }

    /// Start playback in the mode the PAT / SONG switch shows. Pattern mode loops the current pattern from the piano
    /// roll's play-start marker (§10); song mode plays the playlist from its start and stops at its end.
    fn start_playback(&mut self) {
        let p = crate::rack::play_project(&self.state);
        let song = self.state.song_mode;
        if song && p.arrangement().is_none_or(|a| a.clips.is_empty()) {
            self.state.message = Some("The playlist is empty: nothing to play in SONG mode".into());
            self.invalidate();
            return;
        }
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

    pub fn typing_on(&self) -> bool {
        self.state.toggles.contains(&"typing")
    }

    /// A physical key went down or up. Returns true when the typing keyboard took it (a note key or
    /// the octave keys while the switch is on, or the release of a held note); the caller then
    /// skips the normal shortcut handling. Spec: ui/typing-keyboard.md.
    pub fn typing_key(&mut self, code: KeyCode, pressed: bool, ctrl: bool, alt: bool) -> bool {
        // A playlist text box (track / marker name, point value) takes the letters.
        if pressed && (self.state.playlist.edit.is_some() || self.state.pat_ui.name_box.is_some() || self.state.pat_ui.confirm || self.browser_takes_typing()) {
            return false;
        }
        if !pressed {
            let Some(i) = self.typing_held.iter().position(|h| h.0 == code) else { return false };
            let (_, row, key) = self.typing_held.remove(i);
            self.transport.note_off(row, key);
            self.typing_lit_changed();
            return true;
        }
        if !self.typing_on() || ctrl || alt {
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
        if !self.typing_synced {
            // The live stream needs the project before the first note.
            self.typing_synced = true;
            self.transport.update(&crate::rack::play_project(&self.state));
        }
        let key = key as u16;
        self.transport.note_on(row, key, TYPING_VELOCITY);
        self.typing_held.push((code, row, key));
        self.typing_lit_changed();
        true
    }

    /// Release every held typing-keyboard note (focus lost, switch off): no hanging notes.
    pub fn typing_release_all(&mut self) {
        if self.typing_held.is_empty() {
            return;
        }
        for (_, row, key) in std::mem::take(&mut self.typing_held) {
            self.transport.note_off(row, key);
        }
        self.typing_lit_changed();
    }

    /// Held typing-keyboard keys light on the piano roll's keyboard (§4).
    fn typing_lit_changed(&mut self) {
        let channels = &self.state.project.channels;
        self.state.piano.typed = self.typing_held.iter().filter_map(|&(_, row, key)| Some((channels.get(row)?.index, i32::from(key)))).collect();
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
    /// every note) one snap unit / semitone; Ctrl+Up/Down move it an octave.
    fn piano_arrow(&mut self, k: winit::keyboard::NamedKey, ctrl: bool) {
        use winit::keyboard::NamedKey::*;
        let Some(g) = self.piano_geom() else { return };
        let step = g.step as i64;
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
                    piano::snapshot(s);
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
                    piano::snapshot(s);
                    s.piano.clear_selection();
                    let key = g.key_at(y).clamp(0, views::piano::TOP_KEY);
                    let start = g.snap_down(g.tick_at(x));
                    let i = piano::add_note(s, ch, start, key);
                    let len = piano::notes(s)[i].length;
                    s.piano.dirty = true;
                    self.drag = Some(Drag::NotePaint { key, start, len });
                    self.hold_key(Some(key), true);
                    self.damage_piano();
                    return;
                }
                Tool::Slice => {
                    self.drag = Some(Drag::Slice);
                    self.state.piano.overlay = Some(Overlay::Slice(g.snap_down(g.tick_at(x) + g.step as f32 / 2.0)));
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
                piano::snapshot(s);
                s.piano.clear_selection();
                let i = piano::add_note(s, ch, g.snap_down(g.tick_at(x)), g.key_at(y));
                let n = piano::notes(s)[i];
                s.piano.last_len = n.length;
                self.drag = Some(Drag::NoteMove { x0: x, y0: y, orig: vec![(i, n)] });
                self.hold_key(Some(i32::from(n.key)), true);
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
                let nr = g.note_rect(&n).unwrap_or_default();
                self.drag = Some(if x >= nr.right() - resize_zone(nr.w) {
                    self.resize_cursor = true;
                    Drag::NoteResize { x0: x, orig }
                } else {
                    self.hold_key(Some(i32::from(n.key)), true);
                    Drag::NoteMove { x0: x, y0: y, orig }
                });
            }
            Action::PianoVelocity => {
                piano::snapshot(s);
                self.drag = Some(Drag::Velocity { last_x: x, last_y: y });
                self.paint_velocity(x, y, x, y);
            }
            Action::PianoKeys => {
                let k = g.key_at(y).clamp(0, views::piano::TOP_KEY);
                self.drag = Some(Drag::PianoKeys { key: k });
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
            Action::PlPickTab(k) => self.state.playlist.picker = k,
            Action::PlPick(src) => {
                self.state.playlist.source = Some(src);
                if let os_model::ClipSource::Pattern(n) = src {
                    self.state.pattern = n;
                }
                if right {
                    // Right-click on a pattern: rename it (OS choice).
                    if let os_model::ClipSource::Pattern(n) = src {
                        let at = self.top_hit(self.mouse.0, self.mouse.1).map_or(self.mouse, |h| (h.rect.x, h.rect.y));
                        pl::start_edit(&mut self.state, pl::EditTarget::PatternName(n), at);
                    }
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
                }
            }
            Action::PlPopupSub(i) => {
                let item = self.state.playlist.popup.as_ref().and_then(|p| p.sub.and_then(|s| p.items.get(s))).and_then(|it| it.children.get(i)).cloned();
                if let Some(item) = item.filter(|it| it.enabled && !it.head) {
                    if let Some(m) = pl::run(self, item.cmd) {
                        self.state.message = Some(m);
                    }
                }
            }
            _ => return false,
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
        if self.playlist_act(&a, false) {
            return;
        }
        if self.browser_act(&a) {
            return;
        }
        if self.rack_act(&a) {
            return;
        }
        if matches!(a, Action::PianoGrid | Action::PianoNote(_) | Action::PianoVelocity | Action::PianoKeys) {
            self.piano_click(&a);
            return;
        }
        if a == Action::PianoRuler {
            if let Some(g) = self.piano_geom() {
                self.state.piano.marker = g.snap_down(g.tick_at(self.mouse.0) + g.step as f32 / 2.0);
                self.drag = Some(Drag::Marker);
            }
            return;
        }
        let s = &mut self.state;
        match a {
            Action::PianoGrid | Action::PianoNote(_) | Action::PianoVelocity | Action::PianoKeys | Action::PianoRuler | Action::Scrollbar(_) => {}
            Action::PianoTool(t) => {
                s.piano.tool = t;
                s.piano.overlay = None;
            }
            Action::PianoUndo => {
                if !piano::undo(s) {
                    s.message = Some("Nothing to undo".into());
                }
            }
            Action::Placeholder("typing") => self.toggle_typing(),
            Action::Placeholder(name) => {
                if let Some(i) = s.toggles.iter().position(|t| *t == name) {
                    s.toggles.remove(i);
                } else if views::toolbar::is_toggle(name) {
                    s.toggles.push(name);
                }
                s.message = Some(format!("{name}: not available yet"));
            }
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
            Action::Play => match self.transport.state() {
                // Play toggles pause (the position is held in place); Stop rewinds.
                TransportState::Playing => self.transport.pause(),
                TransportState::Paused => self.transport.resume(),
                TransportState::Stopped => self.start_playback(),
            },
            Action::Stop => self.transport.stop(),
            Action::Record => s.message = Some("Recording: not available yet".into()),
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
            Action::Tempo => self.drag = Some(Drag::Tempo { y0: self.mouse.1, t0: s.tempo }),
            Action::Menu(i) => {
                s.pat_ui.menu_at = None;
                s.menu_open = if s.menu_open == Some(i) { None } else { Some(i) };
            }
            Action::MenuItem(m, i) => {
                s.menu_open = None;
                self.menu_item(m, i);
            }
            // Show a hidden window, bring a covered one to the front, hide one already in front
            // (spec: ui/channel-rack.md §5).
            Action::ToggleWin(Win::Browser) => s.show_browser = !s.show_browser,
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
            Action::PlPickList => {}
            Action::PlPickAdd => {
                let at = crate::patterns::box_at_mouse(self.mouse);
                crate::patterns::run(s, crate::patterns::Cmd::FindFirst, at);
                s.playlist.source = None;
            }
            Action::Mix(h) => {
                if let Some(m) = crate::mixer::hit(self, h) {
                    self.state.message = Some(m);
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
                s.picker_open = true;
                s.picker_cat = None;
            }
            Action::PickerOpen => {
                s.picker_open = true;
                s.picker_cat = None;
            }
            Action::Knob(id) => {
                self.drag = Some(Drag::Knob { id, y0: self.mouse.1, v0: s.knob(id) });
                self.hint = Cow::Owned(s.knob_text(id));
            }
            Action::PianoLaneMenu => s.lane_menu = true,
            Action::PitchStep(dir) => {
                s.piano.pitch_center = piano::clamp_pitch_center(s.piano.pitch_center + dir * piano::PITCH_STEP);
            }
            Action::PianoLanePick(i) => {
                s.piano.lane = piano::Lane::ALL[i.min(piano::Lane::ALL.len() - 1)];
                s.lane_menu = false;
            }
            Action::Settings(_) => {}
            Action::PickerBg => s.picker_open = false,
            Action::PickerAll => s.picker_cat = None,
            Action::PickerTab(e, i) => s.picker_cat = if s.picker_cat == Some((e, i)) { None } else { Some((e, i)) },
            Action::PickerTile(true, i) => {
                s.picker_open = false;
                // Into the slot the picker was opened for, else the current track's first empty slot (spec: ui/mixer.md §7).
                let t = s.selected_mixer;
                let slot = s.mixer.picker_slot.take().or_else(|| {
                    s.project.mixer.iter().find(|m| m.index == t).and_then(|m| m.slots.iter().position(Option::is_none)).map(|k| k as u8)
                });
                match slot {
                    Some(k) => {
                        if let Some(m) = crate::mixer::run(self, crate::mixer::MCmd::AddEffect(k, i)) {
                            self.state.message = Some(m);
                        }
                    }
                    None => s.message = Some("Every slot of this track is in use".into()),
                }
                return;
            }
            Action::PickerTile(false, i) if i >= views::picker::GENERATORS.len() => {
                s.picker_open = false;
                let name = s.hosted_generators.get(i - views::picker::GENERATORS.len()).copied().unwrap_or("Plugin");
                let ch = add_plugin_channel(&mut s.project, name);
                s.pending_plugin = Some((ch, i - views::picker::GENERATORS.len()));
                s.selected_channel = ch;
                s.rack.select_only(ch);
                crate::rack::autosize(s);
                let r = views::rack::rect(s);
                let view_h = views::rack::panel(r).h;
                s.rack_scroll = (views::rack::content_h(s) - view_h).max(0.0);
            }
            Action::PickerTile(false, _) => {
                s.picker_open = false;
                let ch = add_channel(&mut s.project);
                s.piano.dirty = true;
                s.selected_channel = ch;
                s.rack.select_only(ch);
                crate::rack::autosize(s);
                // Hosted by Open Sampler on the next tick (`sync_samplers`), then its window opens.
                s.pending_open = Some(ch);
                s.message = Some("Added a sampler channel (Open Sampler)".into());
                // Keep the new row in view.
                let r = views::rack::rect(s);
                let view_h = views::rack::panel(r).h;
                s.rack_scroll = (views::rack::content_h(s) - view_h).max(0.0);
            }
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
            Action::CloseWin(w) => s.hide(w),
            Action::BrowserRow(_) | Action::BrowserTab(_) | Action::BrowserList | Action::BrowserCollapse | Action::BrowserMenu | Action::BrowserSearch => {}
            Action::PlaylistArea | Action::MixerArea | Action::RackArea | Action::Swallow => {}
            Action::PlTool(_)
            | Action::PlSnap
            | Action::PlOptions
            | Action::PlRuler
            | Action::PlMarkers
            | Action::PlHeader(_)
            | Action::PlLed(_)
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
        if s.project.channels.get(i).is_some_and(|c| c.kind.is_sampler()) {
            // Not hosted yet (just added): Open Sampler starts on the next tick and its window opens then.
            s.pending_open = Some(i);
            self.sampler_failed.retain(|&r| r != i);
        } else if s.shown(Win::Plugin) && s.plugin_channel == i {
            s.hide(Win::Plugin);
        } else {
            if !s.shown(Win::Plugin) {
                let r = views::rack::rect(s);
                s.plugin_pos = ((r.right() + 8.0).min(1920.0 - 440.0), r.y.min(1080.0 - 210.0));
            }
            s.plugin_channel = i;
            s.raise(Win::Plugin);
        }
    }

    /// Press on a step (spec: ui/channel-rack.md §9): selects the row, toggles the step, starts painting.
    fn rack_step(&mut self, i: usize, k: u32) {
        self.state.rack.select_only(i);
        self.focus_channel(i);
        let s = &mut self.state;
        s.piano.clear_selection();
        // A step past an explicit length grows the pattern to the bar holding it (§5).
        if let Some(l) = crate::rack::explicit_len(s) {
            if k >= l {
                crate::rack::set_len(s, Some((k / 16 + 1) * 16));
            }
        }
        let on = crate::rack::toggle_step(s, i, k);
        self.drag = Some(Drag::RackPaint { row: i, on, last: k });
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
            Action::Step(i, k) => self.rack_step(i, k),
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
                self.rack_step(i, k);
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
    fn rack_pending(&mut self) {
        if let Some(rows) = self.state.rack.delete_now.take() {
            crate::rack::delete_rows(self, &rows);
        }
    }

    /// Rack rows were renumbered (`m(old) = new`, `None` = deleted): hosted plugins, their windows and playback follow.
    pub(crate) fn remap_rows(&mut self, m: &dyn Fn(usize) -> Option<usize>) {
        self.typing_release_all();
        self.plugins.remap(m);
        self.sampler_failed = self.sampler_failed.iter().filter_map(|&r| m(r)).collect();
        self.transport.set_instruments(self.plugins.instruments());
        self.state.piano.dirty = true;
    }

    /// A row's hosted instrument for cloning: (available plugin index, saved state).
    pub(crate) fn plugin_copy(&self, r: usize) -> Option<(usize, Vec<u8>)> {
        let ch = self.state.project.channels.get(r)?;
        let name = match ch.kind {
            ChannelKind::Sampler | ChannelKind::AudioClip => crate::plugins::OPEN_SAMPLER.to_string(),
            ChannelKind::Plugin => ch.plugin.clone()?,
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

    fn menu_item(&mut self, menu: usize, item: usize) {
        use views::menus::*;
        match (menu, item) {
            (0, FILE_NEW) => {
                self.transport.stop();
                self.reset_hosted();
                let s = &mut self.state;
                s.project = default_project();
                s.file = None;
                s.path = None;
                s.tempo = 130.0;
                s.pattern = 1;
                s.piano.reset();
                s.playlist.reset();
                s.rack.reset();
                crate::rack::from_model(s);
                s.piano.dirty = true;
                s.message = Some("New project".into());
            }
            // Theme import (spec: ui/flstheme.md): pick a .flstheme, starting in FL's theme folder when installed.
            (OPTIONS_MENU, i) if item_at(OPTIONS_MENU, i).and_then(|it| views::settings::tab_for_menu(it.label)).is_some() => {
                let tab = item_at(OPTIONS_MENU, i).and_then(|it| views::settings::tab_for_menu(it.label)).expect("checked");
                self.open_settings(tab);
            }
            (0, FILE_OPEN) => {
                let start = self.state.path.as_ref().and_then(|p| p.parent().map(|d| d.to_path_buf()));
                let mut dlg = rfd::FileDialog::new().add_filter("FLP project", &["flp"]);
                if let Some(d) = start {
                    dlg = dlg.set_directory(d);
                }
                if let Some(p) = dlg.pick_file() {
                    self.open(p);
                }
            }
            (0, FILE_SAVE) => self.save(false),
            (0, FILE_SAVE_AS) => self.save(true),
            (0, FILE_EXIT) => self.want_close = true,
            (VIEW, i) if item_at(VIEW, i).is_some_and(|it| it.label == "Plugin picker") => {
                self.state.picker_open = true;
                self.state.picker_cat = None;
            }
            (PATTERNS, i) if i >= PATTERN_PICK => self.state.pattern = (i - PATTERN_PICK) as u16,
            (PATTERNS, i) if item_at(PATTERNS, i).and_then(|it| crate::patterns::Cmd::from_label(it.label)).is_some() => {
                let cmd = item_at(PATTERNS, i).and_then(|it| crate::patterns::Cmd::from_label(it.label)).expect("checked");
                let at = crate::patterns::box_at_mouse(self.mouse);
                if let Some(m) = crate::patterns::run(&mut self.state, cmd, at) {
                    self.state.message = Some(m);
                }
            }
            (m, i) if item_at(m, i).is_some_and(|it| it.label == TYPING_LABEL) => self.toggle_typing(),
            // EDIT > Undo = Ctrl+Z (piano-roll undo).
            (EDIT_MENU, i) if item_at(EDIT_MENU, i).is_some_and(|it| it.label == "Undo") => self.act(Action::PianoUndo),
            (m, i) => {
                let label = item_at(m, i).map_or("item", |it| it.label);
                self.state.message = Some(format!("{label}: not available yet"));
            }
        }
    }
}

/// PATTERNS menu shortcuts (spec: ui/pattern-menu.md §3).
fn pattern_shortcut(key: &winit::keyboard::Key, ctrl: bool, shift: bool) -> Option<crate::patterns::Cmd> {
    use crate::patterns::Cmd;
    use winit::keyboard::{Key, NamedKey};
    let Key::Named(k) = key else { return None };
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
    }
}

/// What a plugin window's strip shows for rack row `row` (spec: ui/plugin-window.md).
#[cfg(windows)]
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
    })
}

/// A generator-plugin channel played by a hosted instrument, routed like `add_channel`. Returns its rack row.
pub(crate) fn add_plugin_channel(p: &mut Project, name: &str) -> usize {
    let row = add_channel(p);
    let ch = &mut p.channels[row];
    ch.kind = ChannelKind::Plugin;
    ch.plugin = Some(name.to_string());
    ch.name = Some(name.to_string());
    row
}

/// Add-channel button (spec: ui/channel-rack.md §5): an empty sampler routed to the next insert
/// after the highest one in use. Returns its rack row.
pub(crate) fn add_channel(p: &mut Project) -> usize {
    let index = p.channels.iter().map(|c| c.index + 1).max().unwrap_or(0);
    let track = (p.channels.iter().map(|c| c.mixer_track).max().unwrap_or(0) + 1).min(crate::mixer::max_insert(p));
    p.channels.push(sampler_channel(index, crate::plugins::OPEN_SAMPLER, track));
    p.channels.len() - 1
}

/// A new project (owner's choice, 2026-10-10): one empty sampler channel, played and edited by Open Sampler, on
/// insert 1, and pattern 1.
pub fn default_project() -> Project {
    template_project(&[crate::plugins::OPEN_SAMPLER])
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
    let mixer = (0..18u16)
        .map(MixerTrack::new)
        .collect();
    Project {
        ppq: 96,
        tempo: Some(130.0),
        current_pattern: Some(1),
        channels: names.iter().enumerate().map(|(i, n)| channel(i as u16, n)).collect(),
        patterns: vec![Pattern { number: 1, ..Pattern::default() }],
        mixer,
        ..Project::default()
    }
}

fn monitor_cfg_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("OpenStudio").join("monitor.txt"))
}

/// Monitor settings from `%APPDATA%/OpenStudio/monitor.txt` (written by the debug panel's Save), else defaults.
fn load_monitor_cfg() -> crate::monitor::Settings {
    let mut cfg = crate::monitor::Settings::default();
    if let Some(text) = monitor_cfg_path().and_then(|p| std::fs::read_to_string(p).ok()) {
        cfg.load(&text);
    }
    cfg
}

fn save_monitor_cfg(cfg: &crate::monitor::Settings) -> Result<PathBuf, String> {
    let p = monitor_cfg_path().ok_or("no APPDATA folder")?;
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(&p, cfg.dump()).map_err(|e| e.to_string())?;
    Ok(p)
}

/// Read a `.flstheme` (spec ui/flstheme.md) or an OpenStudio theme text file.
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

/// The theme: `%APPDATA%/OpenStudio/theme.txt`, else the last imported `current.flstheme`, else the default.
fn load_theme() -> Theme {
    let t = load_theme_inner();
    os_theme::set_current(&t.colors);
    t
}

fn load_theme_inner() -> Theme {
    if cfg!(test) {
        return Theme::default();
    }
    let Some(dir) = std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("OpenStudio")) else { return Theme::default() };
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

    /// Slots holding the file's parametric EQ get an Open Parametric EQ 2 instance and show its name; adding the EQ
    /// stores the file's EQ (spec: plugins/open-parametric-eq-2.md §8).
    #[test]
    fn parametric_eq_slots_are_hosted() {
        use crate::plugins::{fx_key, EQ_FILE_NAME, OPEN_EQ};
        let mut a = App::new(None);
        a.state.project = test_project();
        a.reset_hosted();
        a.plugins.effects = crate::plugins::builtin_effects();
        let mut fl = open_parametric_eq_2::params::default_state();
        open_parametric_eq_2::params::set_field(&mut fl, 1, -600);
        a.state.project.mixer[2].slots[1] = Some(os_model::SlotPlugin { name: EQ_FILE_NAME.into(), state: fl.clone() });
        a.sync_effects();
        assert!(a.plugins.hosted.contains_key(&fx_key(2, 1)));
        assert_eq!(crate::mixer::slot_name(&a.state, 2, 1).as_deref(), Some(OPEN_EQ));
        assert_eq!(a.plugins.eq_file_state(fx_key(2, 1)), Some(fl));
        // A second pass changes nothing.
        let before = a.plugins.hosted.len();
        a.sync_effects();
        assert_eq!(a.plugins.hosted.len(), before);
        assert_eq!(crate::plugins::new_slot(OPEN_EQ).state, open_parametric_eq_2::params::default_state());
    }

    /// Writes the toolbar with a live-looking signal to OS_MONITOR_SHOT (a look-and-feel check, not an assertion).
    #[test]
    fn new_project_is_one_empty_open_sampler() {
        let p = default_project();
        assert_eq!(p.channels.len(), 1);
        let c = &p.channels[0];
        assert_eq!((c.kind, c.name.as_deref(), c.sample_path.as_deref(), c.mixer_track), (ChannelKind::Sampler, Some("Open Sampler"), None, 1));
        // Its starting Open Sampler state names no sample and carries the channel's envelope records.
        let st = crate::plugins::sampler_state(c);
        let json = String::from_utf8_lossy(&st[8..]).to_string();
        assert!(json.contains("\"envvol_on\":{\"bool\":false}") && json.contains("\"sample-path\":\"\\\"\\\"\""), "{json}");
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
        for id in [KnobId::MasterVol, KnobId::MixPan(1), KnobId::ChanPan(0)] {
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
            frame(&mut app, &mut a, d.flatten());
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
        let act = Action::Placeholder("audio editor");
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

    fn assert_same(a: &Canvas, b: &Canvas, what: &str) {
        let diffs = a.pixmap.data().iter().zip(b.pixmap.data()).enumerate().filter(|(_, (x, y))| x.abs_diff(**y) > 3).map(|(i, _)| i / 4);
        let (n, bbox) = diffs.fold((0, (usize::MAX, usize::MAX, 0, 0)), |(n, m), i| (n + 1, (m.0.min(i % 1920), m.1.min(i / 1920), m.2.max(i % 1920), m.3.max(i / 1920))));
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
        use crate::views::settings::{SAct, GENERAL, MIDI, PROJECT, TABS};
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
        let k = Action::Settings(SAct::Check("Send master sync"));
        let r = hit_rect(&app, &k);
        click(&mut app, &mut c, r);
        assert!(app.state.settings.flipped.contains("Send master sync"));
        let r = hit_rect(&app, &Action::Settings(SAct::Tab(PROJECT)));
        click(&mut app, &mut c, r);
        let adv = Action::Settings(SAct::Group("Advanced"));
        let r = hit_rect(&app, &adv);
        click(&mut app, &mut c, r);
        assert!(app.hits.iter().any(|h| h.action == Action::Settings(SAct::Check("Fast declick for cut groups"))), "Advanced unfolded");
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
        // The all tab lists the 26 roots, all closed.
        assert_eq!(rows(&app.state).len(), 26);
        // Project tab: Current project opened with its patterns (default like the screenshot).
        let r0 = hit_rect(&app, &Action::BrowserTab(1));
        click(&mut app, &mut c, r0);
        let r = rows(&app.state);
        assert_eq!(r[0].label, "Current project");
        assert_eq!(r[1].label, "History");
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
        // Plugin database tab: open Generators, drag the Sampler onto the playlist -> a new channel.
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
        // Online / favorites tabs are empty; wheel never scrolls past the content.
        let r0 = hit_rect(&app, &Action::BrowserTab(4));
        click(&mut app, &mut c, r0);
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

    /// Audio-clip channels (dropped audio, FL's audio clips) are hosted and opened by Open Sampler, never the info
    /// window (spec: plugins/open-sampler.md §7).
    #[test]
    fn audio_clip_channels_use_open_sampler() {
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
        assert!(app.state.hosted_rows.contains(&row), "Open Sampler hosts the audio clip channel");
        assert_eq!(app.plugins.sampler_path(row).as_deref(), Some(path.as_str()));
        app.toggle_channel_window(row);
        assert!(!app.state.shown(Win::Plugin), "no information window");
        assert_eq!(app.state.pending_open, Some(row), "Open Sampler's window opens");
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
            assert!(app.state.hosted_rows.contains(&r), "row {r} has its Open Sampler");
            assert_eq!(app.plugins.sampler_path(r).as_deref(), Some(wav.as_str()));
        }
        let _ = std::fs::remove_file(&wav);
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
        assert_eq!(r[i + 3].depth, 0, "next root follows");
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
        assert_eq!(rows(&app.state).len(), 26);
        // Search box on top moves the list down.
        app.browser_cmd(crate::browser::BCmd::Toggle("top"));
        frame(&mut app, &mut c, None);
        assert!(hit_rect(&app, &Action::BrowserSearch).y < 200.0);
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

        // Name button opens the channel's Open Sampler (a sampler row never shows the info window); the slot selects.
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
            if let Some(d) = app.take_damage() {
                frame(app, c, d);
            }
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
        assert_eq!(app.state.rack_size, (819.0, 306.0));
        assert_eq!(views::rack::visible_steps(&app.state), 36);
        frame(&mut app, &mut b, None);
        assert_same(&a, &b, "after mixer drag and resize");

        // A dimmed step (past the one-bar pattern) extends the pattern to two bars.
        assert_eq!(crate::rack::pattern_steps(&app.state), 16);
        let hr = hit_rect(&app, &Action::Step(0, 20));
        click(&mut app, &mut a, hr);
        assert_eq!(crate::rack::pattern_steps(&app.state), 32);

        // Sizes snap to whole steps / rows, down to 4 steps and one row.
        assert_eq!(views::rack::snap_size(&app.state, 822.0, 300.0), (819.0, 306.0));
        assert_eq!(views::rack::snap_size(&app.state, 10.0, 10.0), (307.0, 126.0));
        let keep = app.state.rack_size;
        app.state.rack_size = (307.0, 126.0);
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

        // Add channels until the rows overflow: a vertical scroll bar appears.
        let n = app.state.project.channels.len();
        for _ in 0..6 {
            let hr = hit_rect(&app, &Action::RackAdd);
            click(&mut app, &mut a, hr);
            assert!(app.state.picker_open);
            let tile = hit_rect(&app, &Action::PickerTile(false, 0));
            click(&mut app, &mut a, tile);
            assert!(!app.state.picker_open);
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
        let Ok(mut a) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut b = Canvas::new(1920, 1080, 1.0).unwrap();
        let mut app = App::new(None);
        app.state.wins.iter_mut().for_each(|w| w.maxed = false);
        app.state.z = vec![Win::Playlist, Win::Mixer];
        frame(&mut app, &mut a, None);
        let _ = app.take_damage();
        let step = |app: &mut App, c: &mut Canvas, f: &dyn Fn(&mut App)| {
            f(app);
            if let Some(d) = app.take_damage() {
                frame(app, c, d);
            }
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
        app.act(Action::CloseWin(Win::Mixer));
        assert!(!app.state.shown(Win::Mixer));

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
            if let Some(d) = app.take_damage() {
                frame(app, c, d);
            }
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
        for key in [71, 70, 69] {
            step(&mut app, &mut a, |ap| { ap.mouse_move(kx, g.y_of(key) + 8.0); });
            assert_eq!(app.state.piano.held, Some(key));
        }
        frame(&mut app, &mut b, None);
        assert_same(&a, &b, "keyboard drag");
        step(&mut app, &mut a, |ap| ap.mouse_up());
        assert_eq!(app.state.piano.held, None);
        frame(&mut app, &mut b, None);
        assert_same(&a, &b, "keyboard released");
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
        app.state.hosted_effects = vec!["Test Delay"];
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
}
