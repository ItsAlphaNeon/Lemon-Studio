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

/// Docked windows shown in the main area (one in front at a time in this draft).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Win {
    Playlist,
    Mixer,
    ChannelRack,
    PianoRoll,
    Browser,
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
    RackDrag,
    CloseRack,
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
}

/// What views paint with.
pub struct Ui<'a> {
    pub c: &'a mut Canvas,
    pub t: &'a Palette,
    hits: &'a mut Vec<Hit>,
    hover: &'a Option<Action>,
    pub mouse: (f32, f32),
}

impl Ui<'_> {
    /// Register a hit region; returns whether it is hovered (topmost last frame).
    pub fn hit(&mut self, rect: Rect, action: Action, hint: impl Into<Cow<'static, str>>) -> bool {
        let hovered = self.hover.as_ref() == Some(&action) && rect.contains(self.mouse.0, self.mouse.1);
        self.hits.push(Hit { rect, action, hint: hint.into() });
        hovered
    }
}

#[derive(Debug, Clone)]
pub enum Drag {
    Rack { dx: f32, dy: f32 },
    Fader { track: u16, y0: f32, v0: f64 },
    Tempo { y0: f32, t0: f64 },
    /// Piano roll: move notes (originals by index), resize their ends, erase, draw velocities.
    NoteMove { x0: f32, y0: f32, orig: Vec<(usize, Note)> },
    NoteResize { x0: f32, orig: Vec<(usize, Note)> },
    NoteErase,
    Velocity { last_x: f32 },
}

pub struct State {
    pub project: Project,
    pub file: Option<os_flp::FlpFile>,
    pub path: Option<PathBuf>,
    pub front: Option<Win>,
    pub show_rack: bool,
    pub show_browser: bool,
    pub rack_pos: (f32, f32),
    pub selected_channel: usize,
    pub selected_mixer: u16,
    pub pattern: u16,
    pub song_mode: bool,
    pub menu_open: Option<usize>,
    pub playlist_scroll: (f32, f32),
    pub mixer_scroll: f32,
    pub rack_scroll: f32,
    pub browser_selected: usize,
    pub tempo: f64,
    pub message: Option<String>,
    pub toggles: Vec<&'static str>,
    pub maximized: bool,
    pub piano: PianoState,
}

pub struct App {
    pub state: State,
    pub theme: Theme,
    pub transport: Transport,
    pub hits: Vec<Hit>,
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
                front: Some(Win::Playlist),
                show_rack: true,
                show_browser: true,
                rack_pos: (317.0, 195.0),
                selected_channel: 0,
                selected_mixer: 1,
                pattern: 1,
                song_mode: false,
                menu_open: None,
                playlist_scroll: (0.0, 0.0),
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
        if playing || playing != self.last_playing {
            self.last_playing = playing;
            self.invalidate();
        }
        self.has_damage()
    }

    pub fn paint(&mut self, canvas: &mut Canvas, damage: Option<Rect>) {
        canvas.begin_frame(damage);
        self.hits.clear();
        let title = self.title();
        let mut ui = Ui { c: canvas, t: &self.theme.colors, hits: &mut self.hits, hover: &self.hover, mouse: self.mouse };
        let info = views::FrameInfo {
            title,
            hint: self.hint.clone(),
            playing: self.transport.playing(),
            seconds: self.transport.seconds(),
            peaks: self.transport.peaks(),
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
                Drag::NoteMove { x0, y0, orig } => self.drag_notes(x0, y0, &orig, false),
                Drag::NoteResize { x0, orig } => self.drag_notes(x0, 0.0, &orig, true),
                Drag::NoteErase => self.erase_at(x, y),
                Drag::Velocity { last_x } => {
                    self.paint_velocity(last_x, x, y);
                    self.drag = Some(Drag::Velocity { last_x: x });
                }
                Drag::Rack { dx, dy } => {
                    let old = views::rack::bounds(&self.state);
                    self.state.rack_pos = ((x - dx).max(0.0), (y - dy).max(83.0));
                    let new = views::rack::bounds(&self.state);
                    self.damage_rect(old.union(&new));
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
        if let Some(a) = action {
            self.act(a);
        }
    }

    /// Right button: delete notes in the piano roll (spec: ui/piano-roll.md §8).
    pub fn mouse_down_right(&mut self) {
        let (x, y) = self.mouse;
        let action = self.top_hit(x, y).map(|h| h.action.clone());
        if self.state.menu_open.is_some() {
            self.state.menu_open = None;
            self.invalidate();
            return;
        }
        if matches!(action, Some(Action::PianoNote(_) | Action::PianoGrid)) {
            piano::snapshot(&mut self.state);
            self.drag = Some(Drag::NoteErase);
            self.erase_at(x, y);
        }
    }

    pub fn mouse_up(&mut self) {
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
            for &(i, o) in orig {
                if let Some(n) = p.notes.get_mut(i) {
                    n.position = (i64::from(o.position) + dt) as u32;
                    n.key = (i32::from(o.key) + dk) as u16;
                }
            }
        }
        dirty.extend(rects(&self.state));
        self.damage_notes(&dirty);
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
        let s = &mut self.state;
        let (dx, dy) = if shift { (dy, dx) } else { (dx, dy) };
        match area {
            Some(Action::Fader(_) | Action::MixerSelect(_) | Action::MixerArea) => {
                s.mixer_scroll = (s.mixer_scroll - (dy + dx) * 44.0).max(0.0);
            }
            Some(Action::Step(..) | Action::ChannelSelect(_) | Action::ChannelEnable(_) | Action::RackArea) => {
                s.rack_scroll = (s.rack_scroll - dy * 30.0).max(0.0);
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
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("a") && self.state.front == Some(Win::PianoRoll) => {
                let s = &mut self.state;
                if let Some(ch) = piano::channel(s) {
                    let all = piano::notes(s).iter().enumerate().filter(|(_, n)| n.channel == ch).map(|(i, _)| i).collect();
                    s.piano.select(s.pattern, ch, all);
                }
            }
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("z") => self.act(Action::PianoUndo),
            Key::Named(NamedKey::Delete) if self.state.front == Some(Win::PianoRoll) => {
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
                let orig: Vec<(usize, Note)> = idx.iter().filter_map(|&j| notes.get(j).map(|&m| (j, m))).collect();
                s.piano.last_len = n.length.max(g.step);
                let nr = g.note_rect(&n).unwrap_or_default();
                self.drag = Some(if x >= nr.right() - resize_zone(nr.w) {
                    self.resize_cursor = true;
                    Drag::NoteResize { x0: x, orig }
                } else {
                    Drag::NoteMove { x0: x, y0: y, orig }
                });
            }
            Action::PianoVelocity => {
                piano::snapshot(s);
                self.drag = Some(Drag::Velocity { last_x: x });
                self.paint_velocity(x, x, y);
            }
            Action::PianoKeys => s.message = Some(format!("{}: key preview is not available yet", views::piano::key_name(g.key_at(y)))),
            _ => {}
        }
    }

    pub fn act(&mut self, a: Action) {
        if matches!(a, Action::PianoGrid | Action::PianoNote(_) | Action::PianoVelocity | Action::PianoKeys) {
            self.piano_click(&a);
            return;
        }
        let s = &mut self.state;
        match a {
            Action::PianoGrid | Action::PianoNote(_) | Action::PianoVelocity | Action::PianoKeys => {}
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
            Action::ToggleWin(w) => match w {
                Win::ChannelRack => s.show_rack = !s.show_rack,
                Win::Browser => s.show_browser = !s.show_browser,
                Win::Playlist | Win::Mixer | Win::PianoRoll => {
                    s.front = if s.front == Some(w) { None } else { Some(w) };
                    if s.front == Some(Win::PianoRoll) {
                        piano::center_keys(s, self.piano_grid.map_or(700.0, |g| g.h));
                    }
                }
            },
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
                if s.selected_channel != i && s.front == Some(Win::PianoRoll) {
                    s.selected_channel = i;
                    s.piano.clear_selection();
                    piano::center_keys(s, self.piano_grid.map_or(700.0, |g| g.h));
                }
                s.selected_channel = i;
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
            Action::RackDrag => {
                self.drag = Some(Drag::Rack { dx: self.mouse.0 - s.rack_pos.0, dy: self.mouse.1 - s.rack_pos.1 });
            }
            Action::CloseRack => s.show_rack = false,
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

/// A project with the default template's shape: five sampler channels and pattern 1.
pub fn default_project() -> Project {
    let channel = |i: u16, name: &str| Channel {
        index: i,
        kind: ChannelKind::Sampler,
        name: Some(name.into()),
        plugin: None,
        color: None,
        enabled: true,
        volume: 10000.0 / 12800.0,
        pan: 0.0,
        pitch_cents: 0,
        mixer_track: i + 1,
        sample_path: None,
        layer_children: Vec::new(),
    };
    let mixer = (0..=126u16)
        .map(|i| MixerTrack {
            index: i,
            name: None,
            color: None,
            enabled: true,
            slots: Default::default(),
            routes: Vec::new(),
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
        app.state.front = Some(Win::Mixer);
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

    /// Piano roll edits repaint only damaged regions; the result must match a full repaint.
    #[test]
    fn piano_damage_matches_full() {
        let Ok(mut a) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut b = Canvas::new(1920, 1080, 1.0).unwrap();
        let mut app = App::new(None);
        app.state.front = Some(Win::PianoRoll);
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
        step(&mut app, &mut a, |ap| ap.mouse_up());
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
    }
}
