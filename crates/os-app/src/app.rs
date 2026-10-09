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

use crate::piano::{self, Geom, PianoState};
use crate::transport::Transport;
use crate::views;

/// Child windows. Playlist, Mixer and Piano roll are shown maximized (filling the workspace); the
/// Channel rack and the channel plugin window float. All share one stacking order (`State::z`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Win {
    Playlist,
    Mixer,
    ChannelRack,
    PianoRoll,
    Browser,
    Plugin,
}

impl Win {
    /// Shown maximized in the workspace (this draft has no floating Playlist / Mixer / Piano roll).
    pub fn maximized(self) -> bool {
        matches!(self, Win::Playlist | Win::Mixer | Win::PianoRoll)
    }
}

/// Mouse cursor shapes the shell should show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    Default,
    EwResize,
    NsResize,
    NwseResize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// A control that is designed but not functional yet.
    Placeholder(&'static str),
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
    MixerSelect(u16),
    Fader(u16),
    ChannelSelect(usize),
    ChannelEnable(usize),
    Step(usize, u32),
    /// Channel rack: activity / select slot, mixer track box (drag), add channel, resize edges
    /// (`views::rack::EDGE_*` bits).
    RackSelect(usize),
    RackMixer(usize),
    RackAdd,
    RackResize(u8),
    /// Step / piano-roll view toggle, and a row's mini piano roll (spec: ui/channel-rack.md §6).
    RackRollView,
    RackRoll(usize),
    /// Floating window title bar (drag to move) and close button.
    WinDrag(Win),
    CloseWin(Win),
    BrowserRow(usize),
    PlaylistArea,
    MixerArea,
    RackArea,
    Swallow,
    /// Piano roll: empty grid, a note (index into the pattern's notes), keyboard, velocity panel.
    PianoGrid,
    PianoNote(usize),
    PianoKeys,
    PianoVelocity,
    PianoUndo,
    /// A scroll bar (left-drag scrolls; right-drag scrolls horizontally and zooms vertically).
    Scrollbar(Scroll),
}

/// Scrollable view axes. Scroll values are in content pixels (see `App::scroll_get`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scroll {
    PianoX,
    PianoY,
    PlaylistX,
    PlaylistY,
    MixerX,
    RackX,
    RackY,
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

/// Actions whose hover state has no visual feedback (large background regions).
fn hover_is_visible(a: &Action) -> bool {
    !matches!(
        a,
        Action::DragWindow
            | Action::PlaylistArea
            | Action::MixerArea
            | Action::RackArea
            | Action::Swallow
            | Action::PianoGrid
            | Action::PianoNote(_)
            | Action::PianoKeys
            | Action::PianoVelocity
            | Action::Scrollbar(_)
            | Action::RackResize(_)
            | Action::WinDrag(_)
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
    pub mouse: (f32, f32),
    /// The window being painted (tags hits).
    pub win: Option<Win>,
}

impl Ui<'_> {
    /// Register a hit region; returns whether it is hovered (topmost last frame).
    pub fn hit(&mut self, rect: Rect, action: Action, hint: impl Into<Cow<'static, str>>) -> bool {
        let hovered = self.hover.as_ref() == Some(&action) && rect.contains(self.mouse.0, self.mouse.1);
        self.hits.push(Hit { rect, action, hint: hint.into(), win: self.win });
        hovered
    }
}

#[derive(Debug, Clone)]
pub enum Drag {
    /// Move a floating window (offset of the mouse from its top-left).
    Window { win: Win, dx: f32, dy: f32 },
    /// Resize the channel rack from its right / bottom edges.
    RackResize { edges: u8, m0: (f32, f32), s0: (f32, f32) },
    /// Change a channel's mixer track (spec: ui/channel-rack.md §5).
    RackMixer { ch: usize, y0: f32, v0: u16 },
    Fader { track: u16, y0: f32, v0: f64 },
    Tempo { y0: f32, t0: f64 },
    /// Piano roll: move notes (originals by index), resize their ends, erase, draw velocities.
    NoteMove { x0: f32, y0: f32, orig: Vec<(usize, Note)> },
    NoteResize { x0: f32, orig: Vec<(usize, Note)> },
    NoteErase,
    Velocity { last_x: f32 },
    /// Left button held on the piano keyboard: each key entered is auditioned (§8).
    PianoKeys { key: i32 },
    /// Scroll axes from a start mouse position: value = start + (mouse - m0) × ratio per axis
    /// (ratio < 0 grabs the content, as with the middle button).
    Scroll { m0: (f32, f32), axes: Vec<(Scroll, bool, f32, f32)> },
    /// Right-drag on a scroll bar: horizontal mouse movement scrolls the bar, vertical movement
    /// zooms its axis around the view centre (up = zoom in).
    BarZoom { t: Scroll, m0: (f32, f32), p0: f32, ratio: f32, z0: f32, center: f32, view: f32 },
}

pub struct State {
    pub project: Project,
    pub file: Option<os_flp::FlpFile>,
    pub path: Option<PathBuf>,
    /// Open child windows, bottom to top.
    pub z: Vec<Win>,
    pub show_browser: bool,
    pub rack_pos: (f32, f32),
    pub rack_size: (f32, f32),
    /// Horizontal step scroll of the rack, pixels.
    pub rack_scroll_x: f32,
    /// Channel whose select slot is lit (spec: ui/channel-rack.md §5).
    pub rack_selected: Option<usize>,
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
    pub playlist_scroll: (f32, f32),
    /// Playlist horizontal zoom, pixels per bar.
    pub playlist_bar_w: f32,
    pub mixer_scroll: f32,
    pub rack_scroll: f32,
    pub browser_selected: usize,
    pub tempo: f64,
    pub message: Option<String>,
    pub toggles: Vec<&'static str>,
    pub maximized: bool,
    pub piano: PianoState,
}

impl State {
    pub fn shown(&self, w: Win) -> bool {
        self.z.contains(&w)
    }

    /// The maximized window in front (the one filling the workspace), if any.
    pub fn docked(&self) -> Option<Win> {
        self.z.iter().rev().copied().find(|w| w.maximized())
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
        if w.maximized() { self.docked() == Some(w) } else { self.z.last() == Some(&w) }
    }
}

pub struct App {
    pub state: State,
    pub theme: Theme,
    pub transport: Transport,
    pub hits: Vec<Hit>,
    pub bars: Vec<Bar>,
    pub hover: Option<Action>,
    hover_rect: Option<Rect>,
    damage: Damage,
    last_playing: bool,
    pub hint: Cow<'static, str>,
    pub mouse: (f32, f32),
    pub drag: Option<Drag>,
    pub want_minimize: bool,
    pub want_maximize: bool,
    pub want_close: bool,
    pub want_drag_window: bool,
    /// Show the horizontal-resize cursor (hovering or dragging a note end).
    pub resize_cursor: bool,
    /// Piano roll regions from the last paint (window, grid, velocity panel).
    piano_win: Option<Rect>,
    piano_grid: Option<Rect>,
    piano_panel: Option<Rect>,
}

/// Window-level requests the shell must carry out after an event.
impl App {
    pub fn new(path: Option<PathBuf>) -> App {
        let transport = Transport::new(os_engine::SampleLibrary::default_factory_dir());
        let mut app = App {
            state: State {
                project: default_project(),
                file: None,
                path: None,
                z: vec![Win::Playlist, Win::ChannelRack],
                show_browser: true,
                rack_pos: (317.0, 195.0),
                rack_size: views::rack::DEFAULT_SIZE,
                rack_scroll_x: 0.0,
                rack_selected: Some(0),
                rack_roll_view: false,
                plugin_channel: 0,
                plugin_pos: (560.0, 260.0),
                selected_channel: 0,
                selected_mixer: 1,
                pattern: 1,
                song_mode: false,
                menu_open: None,
                playlist_scroll: (0.0, 0.0),
                playlist_bar_w: views::playlist::BAR_W,
                mixer_scroll: 0.0,
                rack_scroll: 0.0,
                browser_selected: 6,
                tempo: 130.0,
                message: None,
                toggles: vec!["typing", "multilink"],
                maximized: true,
                piano: PianoState::default(),
            },
            theme: load_theme(),
            transport,
            hits: Vec::new(),
            bars: Vec::new(),
            hover: None,
            hover_rect: None,
            damage: Damage::Full,
            last_playing: false,
            hint: Cow::Borrowed(""),
            mouse: (-1.0, -1.0),
            drag: None,
            want_minimize: false,
            want_maximize: false,
            want_close: false,
            want_drag_window: false,
            resize_cursor: false,
            piano_win: None,
            piano_grid: None,
            piano_panel: None,
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
                let s = &mut self.state;
                s.tempo = project.tempo.unwrap_or(130.0);
                s.pattern = project.current_pattern.unwrap_or(1).max(1);
                s.selected_channel = 0;
                s.selected_mixer = 1;
                s.playlist_scroll = (0.0, 0.0);
                s.rack_scroll = 0.0;
                s.rack_scroll_x = 0.0;
                s.rack_selected = Some(0);
                s.hide(Win::Plugin);
                s.mixer_scroll = 0.0;
                s.song_mode = project.arrangements.first().is_some_and(|a| !a.clips.is_empty());
                s.project = project;
                s.file = Some(file);
                s.message = Some(format!("Opened {}", path.display()));
                s.path = Some(path);
                s.piano.reset();
                piano::center_keys(s, self.piano_grid.map_or(700.0, |g| g.h));
            }
            Err(e) => self.state.message = Some(format!("Could not open {}: {e}", path.display())),
        }
    }

    fn save(&mut self, choose: bool) {
        let s = &mut self.state;
        let Some(file) = s.file.as_mut() else {
            s.message = Some("Saving new projects is not supported yet (open an FLP first)".into());
            return;
        };
        let target = if choose || s.path.is_none() {
            rfd::FileDialog::new().add_filter("FLP project", &["flp"]).save_file()
        } else {
            s.path.clone()
        };
        let Some(target) = target else { return };
        s.project.tempo = Some(s.tempo);
        let result = os_flp::project::apply(&s.project, file)
            .map_err(|e| e.to_string())
            .and_then(|()| file.to_bytes().map_err(|e| e.to_string()))
            .and_then(|b| std::fs::write(&target, b).map_err(|e| e.to_string()));
        s.message = Some(match result {
            Ok(()) => format!("Saved {}", target.display()),
            Err(e) => format!("Save failed: {e}"),
        });
        if s.message.as_deref().is_some_and(|m| m.starts_with("Saved")) {
            s.path = Some(target);
        }
    }

    /// Paint the whole frame.
    /// Mark everything dirty (state changed in a way that can affect any view).
    pub fn invalidate(&mut self) {
        self.damage = Damage::Full;
    }

    fn damage_rect(&mut self, r: Rect) {
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

    /// Periodic housekeeping; returns whether a repaint is needed.
    pub fn tick(&mut self) -> bool {
        let playing = self.transport.playing();
        if let Some(e) = self.transport.take_error() {
            self.state.message = Some(format!("Audio: {e}"));
            self.invalidate();
        }
        // Note edits reach playback from the next pattern loop (after a drag has finished).
        if self.state.piano.dirty && self.drag.is_none() {
            self.state.piano.dirty = false;
            if playing {
                let mut p = self.state.project.clone();
                p.tempo = Some(self.state.tempo);
                self.transport.update(&p);
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
        self.hits.clear();
        self.bars.clear();
        let title = self.title();
        let mut ui = Ui { c: canvas, t: &self.theme.colors, hits: &mut self.hits, bars: &mut self.bars, hover: &self.hover, mouse: self.mouse, win: None };
        let info = views::FrameInfo {
            title,
            hint: self.hint.clone(),
            playing: self.transport.playing(),
            seconds: self.transport.seconds(),
            peaks: self.transport.peaks(),
            sounding: if self.transport.playing() { self.transport.sounding() } else { Vec::new() },
        };
        views::paint(&mut ui, &self.state, &info);
        // Remember where the piano roll's regions are, for mouse mapping and damage.
        let find = |a: Action| self.hits.iter().find(|h| h.action == a).map(|h| h.rect);
        self.piano_grid = find(Action::PianoGrid);
        self.piano_panel = find(Action::PianoVelocity);
        self.piano_win = match (self.piano_grid, self.piano_panel) {
            // The window below its title bar: overview, ruler, keyboard, grid, velocity panel.
            (Some(g), Some(p)) => Some(Rect::new(g.x - 73.0, g.y - 41.0, g.w + 96.0, p.bottom() - g.y + 41.0)),
            _ => None,
        };
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

    fn top_hit(&self, x: f32, y: f32) -> Option<&Hit> {
        self.hits.iter().rev().find(|h| h.rect.contains(x, y))
    }

    /// Mouse moved; returns whether a repaint is needed.
    pub fn mouse_move(&mut self, x: f32, y: f32) -> bool {
        self.mouse = (x, y);
        if let Some(d) = self.drag.clone() {
            match d {
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
                Drag::NoteMove { x0, y0, orig } => self.drag_notes(x0, y0, &orig, false),
                Drag::NoteResize { x0, orig } => self.drag_notes(x0, 0.0, &orig, true),
                Drag::NoteErase => self.erase_at(x, y),
                Drag::Velocity { last_x } => {
                    self.paint_velocity(last_x, x, y);
                    self.drag = Some(Drag::Velocity { last_x: x });
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
                    match win {
                        Win::ChannelRack => self.state.rack_pos = pos,
                        Win::Plugin => self.state.plugin_pos = pos,
                        _ => {}
                    }
                    let new = self.win_bounds(win);
                    self.damage_rect(old.union(&new));
                }
                Drag::RackResize { edges, m0, s0 } => {
                    let old = views::rack::bounds(&self.state);
                    let s = &mut self.state;
                    let w = if edges & views::rack::EDGE_RIGHT != 0 { s0.0 + x - m0.0 } else { s0.0 };
                    let h = if edges & views::rack::EDGE_BOTTOM != 0 { s0.1 + y - m0.1 } else { s0.1 };
                    s.rack_size = views::rack::snap_size(w, h);
                    let new = views::rack::bounds(&self.state);
                    self.damage_rect(old.union(&new));
                }
                Drag::RackMixer { ch, y0, v0 } => {
                    // One insert per 4 px, up = higher (spec: ui/channel-rack.md §5).
                    let v = (i32::from(v0) + ((y0 - y) / 4.0).round() as i32).clamp(0, 125) as u16;
                    let s = &mut self.state;
                    if let Some(c) = s.project.channels.get_mut(ch) {
                        if c.mixer_track != v {
                            c.mixer_track = v;
                            s.piano.dirty = true;
                        }
                    }
                    self.hint = Cow::Owned(format!("Mixer track: {}", views::rack::insert_name(v)));
                    let r = self.hits.iter().rev().find(|h| h.action == Action::RackMixer(ch)).map(|h| h.rect);
                    match r {
                        Some(r) => self.damage_rect(r.outset(2.0)),
                        None => self.invalidate(),
                    }
                    self.damage_rect(HINT_RECT);
                }
                Drag::Fader { track, y0, v0 } => {
                    let v = (v0 - f64::from(y - y0) * views::mixer::FADER_PER_PX).clamp(0.0, 1.0);
                    if let Some(t) = self.state.project.mixer.iter_mut().find(|t| t.index == track) {
                        t.volume = Some(v);
                    }
                    self.hint = Cow::Owned(format!("Volume {}", views::mixer::fader_db_text(v)));
                    let col = self.hits.iter().rev().find(|h| h.action == Action::Fader(track)).map(|h| h.rect);
                    match col {
                        Some(r) => self.damage_rect(r.outset(10.0)),
                        None => self.invalidate(),
                    }
                    self.damage_rect(HINT_RECT);
                }
                Drag::Tempo { y0, t0 } => {
                    let t = (t0 + f64::from(y0 - y).round()).clamp(10.0, 522.0);
                    self.state.tempo = t;
                    self.hint = Cow::Owned(format!("Tempo {t:.3} BPM"));
                    self.damage_rect(Rect::new(380.0, 0.0, 200.0, 83.0));
                    self.damage_rect(HINT_RECT);
                }
            }
            return true;
        }
        let (hover, hint, rect) = match self.top_hit(x, y) {
            Some(h) => (Some(h.action.clone()), h.hint.clone(), Some(h.rect)),
            None => (None, Cow::Borrowed(""), None),
        };
        let resize = matches!(hover, Some(Action::PianoNote(_))) && rect.is_some_and(|r| x >= r.right() - resize_zone(r.w));
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

    pub fn mouse_down(&mut self) {
        self.invalidate();
        let (x, y) = self.mouse;
        let action = self.top_hit(x, y).map(|h| h.action.clone());
        // Any click outside an open menu closes it.
        if self.state.menu_open.is_some() && !matches!(action, Some(Action::MenuItem(..)) | Some(Action::Menu(_))) {
            self.state.menu_open = None;
            return;
        }
        self.raise_under_mouse();
        if let Some(a) = action {
            self.act(a);
        }
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
            _ => views::rack::bounds(&self.state),
        }
    }

    /// Cursor for the current hover / drag.
    pub fn cursor(&self) -> Cursor {
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
        matches!(self.drag, Some(Drag::BarZoom { .. }))
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
            Scroll::PlaylistX => s.playlist_scroll.0,
            Scroll::PlaylistY => s.playlist_scroll.1,
            Scroll::MixerX => s.mixer_scroll,
            Scroll::RackX => s.rack_scroll_x,
            Scroll::RackY => s.rack_scroll,
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
            Scroll::PlaylistX => s.playlist_scroll.0 = v,
            Scroll::PlaylistY => s.playlist_scroll.1 = v,
            Scroll::MixerX => s.mixer_scroll = v,
            Scroll::RackX => s.rack_scroll_x = v,
            Scroll::RackY => s.rack_scroll = v,
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
        self.raise_under_mouse();
        if let Some(Action::Scrollbar(t)) = action {
            return self.scrollbar_press(t, true);
        }
        if matches!(action, Some(Action::PianoNote(_) | Action::PianoGrid)) {
            piano::snapshot(&mut self.state);
            self.drag = Some(Drag::NoteErase);
            self.erase_at(x, y);
        }
    }

    pub fn mouse_up(&mut self) {
        self.hold_key(None, false);
        if matches!(self.drag, Some(Drag::Velocity { .. })) {
            self.state.piano.dirty = true;
        }
        if matches!(self.drag, Some(Drag::NoteMove { .. } | Drag::NoteResize { .. } | Drag::NoteErase)) {
            piano::finish_edit(&mut self.state);
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
            self.damage_piano();
        }
    }

    /// Set the velocity of this channel's notes whose start lies between the last and current x.
    fn paint_velocity(&mut self, last_x: f32, x: f32, y: f32) {
        let (Some(g), Some(panel)) = (self.piano_geom(), self.piano_panel) else { return };
        let v = piano::velocity_at(panel, y);
        let (a, b) = (last_x.min(x) - 3.0, last_x.max(x) + 3.0);
        let ch = piano::channel(&self.state);
        let p = piano::pattern_mut(&mut self.state);
        let mut span: Option<(f32, f32)> = None;
        for n in p.notes.iter_mut().filter(|n| Some(n.channel) == ch) {
            let nx = g.x_of(n.position as f32);
            if nx >= a && nx <= b {
                n.velocity = v;
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
            Some(Action::Fader(_) | Action::MixerSelect(_) | Action::MixerArea) => {
                s.mixer_scroll = (s.mixer_scroll - (dy + dx) * 44.0).max(0.0);
            }
            Some(Action::PlaylistArea) if ctrl => {
                let bar = (s.playlist_scroll.0 + x - 481.0) / s.playlist_bar_w;
                s.playlist_bar_w = (s.playlist_bar_w * 1.25f32.powf(dy + dx)).clamp(4.0, 600.0);
                s.playlist_scroll.0 = (bar * s.playlist_bar_w - (x - 481.0)).max(0.0);
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
        match key {
            Key::Named(NamedKey::Space) => self.act(if self.transport.playing() { Action::Stop } else { Action::Play }),
            Key::Named(NamedKey::F5) => self.act(Action::ToggleWin(Win::Playlist)),
            Key::Named(NamedKey::F6) => self.act(Action::ToggleWin(Win::ChannelRack)),
            Key::Named(NamedKey::F7) => self.act(Action::ToggleWin(Win::PianoRoll)),
            Key::Named(NamedKey::F8) if alt => self.act(Action::ToggleWin(Win::Browser)),
            Key::Named(NamedKey::F9) => self.act(Action::ToggleWin(Win::Mixer)),
            Key::Named(NamedKey::Escape) => {
                self.state.menu_open = None;
                self.state.piano.clear_selection();
            }
            // Piano roll editing keys (spec: ui/piano-roll.md §8).
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("a") && self.state.docked() == Some(Win::PianoRoll) => {
                let s = &mut self.state;
                if let Some(ch) = piano::channel(s) {
                    let all = piano::notes(s).iter().enumerate().filter(|(_, n)| n.channel == ch).map(|(i, _)| i).collect();
                    s.piano.select(s.pattern, ch, all);
                }
            }
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("z") => self.act(Action::PianoUndo),
            Key::Named(NamedKey::Delete) if self.state.docked() == Some(Win::PianoRoll) => {
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

    fn play_mode(&self) -> PlayMode {
        if self.state.song_mode { PlayMode::Song } else { PlayMode::Pattern(self.state.pattern) }
    }

    /// Play one note of channel `channel` (Note::channel) once, unless the song is playing.
    fn preview(&self, channel: u16, key: i32) {
        if self.transport.playing() {
            return;
        }
        let mut p = self.state.project.clone();
        p.tempo = Some(self.state.tempo);
        let note = Note { key: key.clamp(0, views::piano::TOP_KEY) as u16, length: u32::from(p.ppq.max(4)), ..Note::step(channel, 0) };
        p.patterns = vec![Pattern { number: 1, notes: vec![note], ..Pattern::default() }];
        p.arrangements.clear();
        self.transport.preview(p);
    }

    /// Left click in the piano roll (spec: ui/piano-roll.md §8, Draw tool).
    fn piano_click(&mut self, a: &Action) {
        let (x, y) = self.mouse;
        let Some(g) = self.piano_geom() else { return };
        let Some(ch) = piano::channel(&self.state) else {
            self.state.message = Some("Piano roll: no channel selected".into());
            return;
        };
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
                let idx = if sel.contains(&i) {
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
                self.drag = Some(Drag::Velocity { last_x: x });
                self.paint_velocity(x, x, y);
            }
            Action::PianoKeys => {
                let k = g.key_at(y).clamp(0, views::piano::TOP_KEY);
                self.drag = Some(Drag::PianoKeys { key: k });
                self.hold_key(Some(k), true);
            }
            _ => {}
        }
    }

    pub fn act(&mut self, a: Action) {
        if let Action::Scrollbar(t) = a {
            return self.scrollbar_press(t, false);
        }
        if matches!(a, Action::PianoGrid | Action::PianoNote(_) | Action::PianoVelocity | Action::PianoKeys) {
            self.piano_click(&a);
            return;
        }
        let s = &mut self.state;
        match a {
            Action::PianoGrid | Action::PianoNote(_) | Action::PianoVelocity | Action::PianoKeys | Action::Scrollbar(_) => {}
            Action::PianoUndo => {
                if !piano::undo(s) {
                    s.message = Some("Nothing to undo".into());
                }
            }
            Action::Placeholder(name) => {
                if let Some(i) = s.toggles.iter().position(|t| *t == name) {
                    s.toggles.remove(i);
                } else if views::toolbar::is_toggle(name) {
                    s.toggles.push(name);
                }
                s.message = Some(format!("{name}: not available yet"));
            }
            Action::Minimize => self.want_minimize = true,
            Action::Maximize => self.want_maximize = true,
            Action::Close => self.want_close = true,
            Action::DragWindow => self.want_drag_window = true,
            Action::Play => {
                if self.transport.playing() {
                    // Play while playing restarts, as Pause is not modeled yet.
                    self.transport.stop();
                }
                let mut p = s.project.clone();
                p.tempo = Some(s.tempo);
                let mode = self.play_mode();
                self.transport.play(&p, mode);
            }
            Action::Stop => self.transport.stop(),
            Action::Record => s.message = Some("Recording: not available yet".into()),
            Action::PatSong(song) => s.song_mode = song,
            Action::Tempo => self.drag = Some(Drag::Tempo { y0: self.mouse.1, t0: s.tempo }),
            Action::Menu(i) => s.menu_open = if s.menu_open == Some(i) { None } else { Some(i) },
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
                if w == Win::PianoRoll && s.docked() == Some(Win::PianoRoll) {
                    piano::center_keys(s, self.piano_grid.map_or(700.0, |g| g.h));
                }
            }
            Action::PatternPrev => s.pattern = s.pattern.saturating_sub(1).max(1),
            Action::PatternNext => s.pattern = (s.pattern + 1).min(999),
            Action::PatternAdd => {
                let next = s.project.patterns.iter().map(|p| p.number).max().unwrap_or(0) + 1;
                s.pattern = next;
            }
            Action::MixerSelect(t) => s.selected_mixer = t,
            Action::Fader(t) => {
                s.selected_mixer = t;
                let v0 = s.project.mixer.iter().find(|m| m.index == t).and_then(|m| m.volume).unwrap_or(0.8);
                self.drag = Some(Drag::Fader { track: t, y0: self.mouse.1, v0 });
            }
            Action::ChannelSelect(i) => {
                if s.selected_channel != i && s.docked() == Some(Win::PianoRoll) {
                    s.selected_channel = i;
                    s.piano.clear_selection();
                    piano::center_keys(s, self.piano_grid.map_or(700.0, |g| g.h));
                }
                s.selected_channel = i;
                // The name button opens the channel's plugin window (spec: ui/channel-rack.md §5).
                if !s.shown(Win::Plugin) {
                    let r = views::rack::rect(s);
                    s.plugin_pos = ((r.right() + 8.0).min(1920.0 - 440.0), r.y.min(1080.0 - 210.0));
                }
                s.plugin_channel = i;
                s.raise(Win::Plugin);
            }
            Action::RackSelect(i) => s.rack_selected = Some(i),
            Action::RackRollView => s.rack_roll_view = !s.rack_roll_view,
            Action::RackRoll(i) => {
                s.selected_channel = i;
                s.piano.clear_selection();
                s.raise(Win::PianoRoll);
                piano::center_keys(s, self.piano_grid.map_or(700.0, |g| g.h));
            }
            Action::RackMixer(i) => {
                let v0 = s.project.channels.get(i).map_or(0, |c| c.mixer_track);
                self.drag = Some(Drag::RackMixer { ch: i, y0: self.mouse.1, v0 });
            }
            Action::RackAdd => {
                let ch = add_channel(&mut s.project);
                s.selected_channel = ch;
                s.rack_selected = Some(ch);
                s.message = Some("Added a sampler channel (plugin picker not available yet)".into());
                // Keep the new row in view.
                let r = views::rack::rect(s);
                let view_h = views::rack::panel(r).h;
                s.rack_scroll = (views::rack::content_h(s) - view_h).max(0.0);
            }
            Action::RackResize(edges) => {
                self.drag = Some(Drag::RackResize { edges, m0: self.mouse, s0: s.rack_size });
            }
            Action::ChannelEnable(i) => {
                if let Some(c) = s.project.channels.get_mut(i) {
                    c.enabled = !c.enabled;
                }
            }
            Action::Step(ch, step) => {
                s.piano.clear_selection();
                toggle_step(s, ch, step);
            }
            Action::WinDrag(win) => {
                let pos = if win == Win::Plugin { s.plugin_pos } else { s.rack_pos };
                self.drag = Some(Drag::Window { win, dx: self.mouse.0 - pos.0, dy: self.mouse.1 - pos.1 });
            }
            Action::CloseWin(w) => s.hide(w),
            Action::BrowserRow(i) => s.browser_selected = i,
            Action::PlaylistArea | Action::MixerArea | Action::RackArea | Action::Swallow => {}
        }
    }

    fn menu_item(&mut self, menu: usize, item: usize) {
        use views::menus::*;
        match (menu, item) {
            (0, FILE_NEW) => {
                self.transport.stop();
                let s = &mut self.state;
                s.project = default_project();
                s.file = None;
                s.path = None;
                s.tempo = 130.0;
                s.pattern = 1;
                s.piano.reset();
                s.message = Some("New project".into());
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
            (m, i) => {
                let label = MENUS.get(m).and_then(|(_, items)| items.get(i)).map_or("item", |it| it.label);
                self.state.message = Some(format!("{label}: not available yet"));
            }
        }
    }
}

/// Toggle a one-step note in the current pattern (spec: flp/events/patterns.md — a step note is
/// one step (PPQ/4 ticks) long at the default key).
fn toggle_step(s: &mut State, ch: usize, step: u32) {
    let ppq = u32::from(s.project.ppq.max(1));
    let pos = step * ppq / 4;
    let Some(channel) = s.project.channels.get(ch).map(|c| c.index) else { return };
    let number = s.pattern;
    let pat = match s.project.patterns.iter_mut().position(|p| p.number == number) {
        Some(i) => &mut s.project.patterns[i],
        None => {
            s.project.patterns.push(Pattern { number, ..Pattern::default() });
            s.project.patterns.last_mut().expect("pushed")
        }
    };
    let before = pat.notes.len();
    pat.notes.retain(|n| !(n.channel == channel && n.position == pos));
    if pat.notes.len() == before {
        pat.notes.push(Note::step(channel, pos));
        pat.notes.sort_by_key(|n| (n.position, n.channel));
    }
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
    }
}

/// Add-channel button (spec: ui/channel-rack.md §5): an empty sampler routed to the next insert
/// after the highest one in use. Returns its rack row.
fn add_channel(p: &mut Project) -> usize {
    let index = p.channels.iter().map(|c| c.index + 1).max().unwrap_or(0);
    let track = (p.channels.iter().map(|c| c.mixer_track).max().unwrap_or(0) + 1).min(125);
    p.channels.push(sampler_channel(index, "Sampler", track));
    p.channels.len() - 1
}

/// A project with the default template's shape: five sampler channels and pattern 1.
pub fn default_project() -> Project {
    let channel = |i: u16, name: &str| sampler_channel(i, name, i + 1);
    let mixer = (0..=126u16)
        .map(|i| MixerTrack {
            index: i,
            name: None,
            color: None,
            enabled: true,
            slots: Default::default(),
            routes: if i == 0 { Vec::new() } else { vec![0] },
            send_levels: Vec::new(),
            volume: Some(0.8),
            pan: Some(0.0),
            stereo_separation: Some(0.0),
        })
        .collect();
    Project {
        ppq: 96,
        tempo: Some(130.0),
        current_pattern: Some(1),
        channels: ["Kick", "Clap", "Hat", "Snare", "Pluck"].iter().enumerate().map(|(i, n)| channel(i as u16, n)).collect(),
        patterns: vec![Pattern { number: 1, ..Pattern::default() }],
        mixer,
        ..Project::default()
    }
}

/// The theme: `%APPDATA%/OpenStudio/theme.txt` if present, else the default.
fn load_theme() -> Theme {
    let path = std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("OpenStudio").join("theme.txt"));
    if let Some(text) = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()) {
        match Theme::parse("User", &text) {
            Ok(t) => return t,
            Err(e) => eprintln!("theme: {e}; using the default"),
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
        assert!(!app.hits.iter().any(|h| h.win == Some(Win::ChannelRack)), "covered rack gets no clicks");
        app.act(Action::ToggleWin(Win::ChannelRack));
        frame(&mut app, &mut a, None);
        assert_eq!(app.state.z.last(), Some(&Win::ChannelRack));
        app.act(Action::ToggleWin(Win::ChannelRack));
        assert!(!app.state.shown(Win::ChannelRack), "F6 on the front rack hides it");
        app.act(Action::ToggleWin(Win::ChannelRack));
        frame(&mut app, &mut a, None);

        // Name button opens the plugin window for that channel; the slot selects.
        let hr = hit_rect(&app, &Action::ChannelSelect(2));
        click(&mut app, &mut a, hr);
        assert_eq!((app.state.z.last(), app.state.plugin_channel, app.state.selected_channel), (Some(&Win::Plugin), 2, 2));
        let hr = hit_rect(&app, &Action::RackSelect(3));
        click(&mut app, &mut a, hr);
        assert_eq!(app.state.rack_selected, Some(3));
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
        assert_eq!(app.state.project.channels[0].mixer_track, t0 + 5);
        step(&mut app, &mut a, &|ap| ap.mouse_up());

        // Resize from the corner: wider shows more steps, taller more room.
        let r = views::rack::rect(&app.state);
        step(&mut app, &mut a, &|ap| { ap.mouse_move(r.right() - 2.0, r.bottom() - 2.0); });
        assert_eq!(app.cursor(), Cursor::NwseResize);
        step(&mut app, &mut a, &|ap| ap.mouse_down());
        step(&mut app, &mut a, &|ap| { ap.mouse_move(r.right() + 318.0, r.bottom() + 58.0); });
        step(&mut app, &mut a, &|ap| ap.mouse_up());
        assert_eq!(app.state.rack_size, (819.0, 306.0));
        assert_eq!(views::rack::visible_steps(views::rack::rect(&app.state)), 36);
        frame(&mut app, &mut b, None);
        assert_same(&a, &b, "after mixer drag and resize");

        // A dimmed step (past the one-bar pattern) extends the pattern to two bars.
        assert_eq!(views::rack::pattern_steps(&app.state), 16);
        let hr = hit_rect(&app, &Action::Step(0, 20));
        click(&mut app, &mut a, hr);
        assert_eq!(views::rack::pattern_steps(&app.state), 32);

        // Sizes snap to whole steps / rows, down to 4 steps and one row.
        assert_eq!(views::rack::snap_size(822.0, 300.0), (819.0, 306.0));
        assert_eq!(views::rack::snap_size(10.0, 10.0), (307.0, 126.0));
        assert_eq!(views::rack::visible_steps(Rect::new(0.0, 0.0, 307.0, 126.0)), 4);

        // Piano-roll view toggle: rows become mini rolls; clicking one opens the Piano roll.
        assert!(!app.hits.iter().any(|h| matches!(h.action, Action::RackRoll(_))));
        let hr = hit_rect(&app, &Action::RackRollView);
        click(&mut app, &mut a, hr);
        assert!(app.state.rack_roll_view);
        assert!(!app.hits.iter().any(|h| matches!(h.action, Action::Step(..))), "no steps in roll view");
        let hr = hit_rect(&app, &Action::RackRoll(1));
        click(&mut app, &mut a, hr);
        assert_eq!((app.state.docked(), app.state.selected_channel), (Some(Win::PianoRoll), 1));
        app.state.rack_roll_view = false;
        app.act(Action::ToggleWin(Win::ChannelRack));
        frame(&mut app, &mut a, None);

        // Add channels until the rows overflow: a vertical scroll bar appears.
        let n = app.state.project.channels.len();
        for _ in 0..6 {
            let hr = hit_rect(&app, &Action::RackAdd);
            click(&mut app, &mut a, hr);
        }
        assert_eq!(app.state.project.channels.len(), n + 6);
        assert_eq!(app.state.project.channels.last().unwrap().mixer_track, 12, "after the highest insert in use (6 after the drag)");
        assert!(app.bar(Scroll::RackY).is_some_and(|b| b.max > 0.0));
    }

    /// Piano roll edits repaint only damaged regions; the result must match a full repaint.
    #[test]
    fn piano_damage_matches_full() {
        let Ok(mut a) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut b = Canvas::new(1920, 1080, 1.0).unwrap();
        let mut app = App::new(None);
        app.state.z = vec![Win::PianoRoll, Win::ChannelRack];
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
}
