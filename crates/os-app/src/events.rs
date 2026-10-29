// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The event editor (Edit automation on a control's right-click menu): the current pattern's control points of one
//! control, drawn as its level over time, edited with the mouse, and turned into an automation clip. The editing is
//! here, free of any window; the window is `views/events.rs`.
// spec: ui/event-editor.md

use os_model::{AutoMode, AutoPoint};

use crate::app::{App, State, Win};
use crate::piano;
use crate::playlist::{Cmd, PItem, Popup};

/// The editing tools of the toolbar (§3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    /// Left drag draws the level freehand on the snap; right drag draws a straight line.
    #[default]
    Draw,
    /// Left drag deletes the events it passes over.
    Erase,
    /// Left drag selects a stretch of time.
    Select,
}

/// The window's buttons and areas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvAct {
    /// The menu button: Edit's entries (§4).
    Menu,
    Tool(Tool),
    /// The ruler: a drag selects a stretch of time.
    Ruler,
    /// The level graph.
    Graph,
}

/// An entry of the editor's menu (§4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvCmd {
    SelectAll,
    /// The selection's events, or every event of the control without one.
    Delete,
    /// An event with the control's value as it was when the editor opened, at the selection's start (or the
    /// pattern's).
    InsertValue,
    TurnIntoClip,
}

/// The editor's state: which control, the view, the tool and the selection.
#[derive(Debug, Clone, PartialEq)]
pub struct EventsUi {
    pub pos: (f32, f32),
    pub placed: bool,
    /// The control's target word (as automation links and control points name it), its name, and its value
    /// (0..1) when the editor opened.
    pub target: u32,
    pub name: String,
    pub value: f64,
    /// The stored values of levels 0 and 1 (`os_model::target::point_range`, or the plugin parameter's).
    pub range: (f64, f64),
    pub tool: Tool,
    /// The tick at the graph's left edge; pixels per beat.
    pub scroll: f64,
    pub px_per_beat: f32,
    /// A stretch of time, start and end ticks.
    pub sel: Option<(u32, u32)>,
    /// A right-button line being drawn: its start and its end (tick, level).
    pub line: Option<((u32, f64), (u32, f64))>,
}

impl Default for EventsUi {
    fn default() -> Self {
        EventsUi {
            pos: (0.0, 0.0),
            placed: false,
            target: 0,
            name: String::new(),
            value: 0.0,
            range: (0.0, 1.0),
            tool: Tool::Draw,
            scroll: 0.0,
            px_per_beat: 48.0,
            sel: None,
            line: None,
        }
    }
}

/// The editor shows the control `target` named `name` at `value` (0..1); `range`: a plugin parameter's own (min,
/// max), else the target's (`None` when the control takes no control points: the editor does not open).
pub fn open(s: &mut State, target: u32, name: &str, value: f64, range: Option<(f64, f64)>) -> bool {
    use os_model::target;
    let range = match range {
        Some((min, max)) => (f64::from(target::plugin_point_value(min, max, 0.0)), f64::from(target::plugin_point_value(min, max, 1.0))),
        None => match target::point_range(target::decode(target)) {
            Some(r) => r,
            None => return false,
        },
    };
    let e = &mut s.events;
    if e.target != target {
        e.scroll = 0.0;
    }
    e.target = target;
    e.name = name.to_string();
    e.value = value.clamp(0.0, 1.0);
    e.range = range;
    e.sel = None;
    e.line = None;
    true
}

/// Edit automation on a control's menu (`auto`: its target, name and value 0..1; `range`: a plugin parameter's own):
/// it opens the editor on the controls whose events the editor can show, those the engine plays (§1).
pub fn menu_item(auto: Option<&(u32, String, f64)>, range: Option<(f64, f64)>) -> PItem {
    use os_model::target;
    match auto {
        Some((t, name, v)) if range.is_some() || target::point_range(target::decode(*t)).is_some() => {
            PItem::new("Edit automation", Cmd::EditEvents(*t, name.clone(), *v, range))
        }
        _ => PItem::not_yet("Edit automation"),
    }
}

/// The level (0..1) of stored `value` in `range`.
pub fn level(range: (f64, f64), value: i32) -> f64 {
    let (lo, hi) = range;
    if hi == lo { 0.0 } else { ((f64::from(value) - lo) / (hi - lo)).clamp(0.0, 1.0) }
}

/// The stored value of `level` in `range`.
pub fn value(range: (f64, f64), level: f64) -> i32 {
    let (lo, hi) = range;
    (lo + level.clamp(0.0, 1.0) * (hi - lo)).round() as i32
}

/// The control's events in the current pattern: (tick, stored value), in time order.
pub fn points(s: &State) -> Vec<(u32, i32)> {
    let kind = s.events.target;
    piano::controls(s).iter().filter(|c| c.kind == kind).map(|c| (c.position, c.value)).collect()
}

fn set(s: &mut State, tick: u32, level: f64) {
    let (kind, v) = (s.events.target, value(s.events.range, level));
    piano::set_control(s, (kind >> 16) as u16, kind as u16, tick, v);
}

/// Deletes the control's events from `from` to `to` (both included); returns how many went.
pub fn erase(s: &mut State, from: u32, to: u32) -> usize {
    let kind = s.events.target;
    piano::erase_controls(s, (kind >> 16) as u16, kind as u16, from.min(to), from.max(to))
}

/// Draws from `a` to `b` (tick, level): an event on every multiple of `unit` between them (both ends included) at
/// the level of the straight line through them, the control's other events between them gone. A freehand stroke
/// is a run of these from one mouse position to the next.
pub fn draw(s: &mut State, a: (u32, f64), b: (u32, f64), unit: u32) {
    let ((t0, l0), (t1, l1)) = if a.0 <= b.0 { (a, b) } else { (b, a) };
    let unit = unit.max(1);
    let (first, last) = (t0 / unit * unit, t1 / unit * unit);
    let kind = s.events.target;
    let p = piano::pattern_mut(s);
    p.controls.retain(|c| !(c.kind == kind && (first..=last).contains(&c.position)));
    let mut t = first;
    while t <= last {
        let level = if t1 == t0 { l1 } else { l0 + (l1 - l0) * (f64::from(t.clamp(t0, t1) - t0) / f64::from(t1 - t0)) };
        set(s, t, level);
        t += unit;
    }
    s.piano.dirty = true;
}

/// Edit ▸ Delete: the selection's events, or all of them.
pub fn delete(s: &mut State) -> usize {
    match s.events.sel {
        Some((a, b)) => erase(s, a, b.saturating_sub(1).max(a)),
        None => erase(s, 0, u32::MAX),
    }
}

/// Edit ▸ Select all: the pattern's whole length, or to the last event when that lies later.
pub fn select_all(s: &mut State) {
    let end = s.project.pattern_length(s.pattern).max(points(s).last().map_or(0, |p| p.0 + 1));
    s.events.sel = Some((0, end.max(1)));
}

/// Edit ▸ Insert current controller value.
pub fn insert_value(s: &mut State) {
    let at = s.events.sel.map_or(0, |(a, _)| a);
    let level = s.events.value;
    set(s, at, level);
}

/// How far (in level) a clip made of the events may stray from them: Turn into automation clip keeps the fewest
/// points that stay this close (a Lemon Studio choice; spec §5).
pub const CLIP_TOLERANCE: f64 = 0.01;

/// The automation clip points that follow `points` (tick, value) in `range`, `ppq` ticks a beat, to `end` ticks:
/// each event's level holds until the next one, as the pattern plays it; the steps a ramp of events makes become
/// straight lines, and points the line through their neighbours already passes within `tol` of are left out. The
/// clip starts at the pattern's start (the first event's level from there) and ends at `end`.
pub fn clip_points(points: &[(u32, i32)], range: (f64, f64), ppq: u32, end: u32, tol: f64) -> Vec<AutoPoint> {
    let Some(first) = points.first() else { return Vec::new() };
    let beat = |t: u32| f64::from(t) / f64::from(ppq.max(1));
    // The held levels as a line: a vertex where each event takes over.
    let mut line: Vec<(f64, f64)> = vec![(0.0, level(range, first.1))];
    for &(t, v) in points {
        line.push((beat(t), level(range, v)));
    }
    let end = beat(end.max(points.last().map_or(0, |p| p.0)));
    let last = line.last().map_or(0.0, |p| p.1);
    line.push((end, last));
    line.dedup_by(|b, a| a.0 == b.0 && { a.1 = b.1; true });
    let keep = simplify(&line, tol);
    let mut out: Vec<AutoPoint> = keep.iter().map(|&i| AutoPoint::new(line[i].0, line[i].1)).collect();
    if let Some(p) = out.first_mut() {
        p.flag = 0;
        p.mode = AutoMode::Single;
    }
    out
}

/// Indices of the vertices of `line` to keep so that the rest lie within `tol` of the segments between them
/// (Ramer–Douglas–Peucker), the ends always kept.
fn simplify(line: &[(f64, f64)], tol: f64) -> Vec<usize> {
    let n = line.len();
    if n <= 2 {
        return (0..n).collect();
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0, n - 1)];
    while let Some((a, b)) = stack.pop() {
        let ((x0, y0), (x1, y1)) = (line[a], line[b]);
        let mut far = (0.0, 0);
        for (i, &(x, y)) in line.iter().enumerate().take(b).skip(a + 1) {
            let on = if x1 > x0 { y0 + (y1 - y0) * (x - x0) / (x1 - x0) } else { y0 };
            let d = (y - on).abs();
            if d > far.0 {
                far = (d, i);
            }
        }
        if far.0 > tol {
            keep[far.1] = true;
            stack.push((a, far.1));
            stack.push((far.1, b));
        }
    }
    (0..n).filter(|&i| keep[i]).collect()
}

/// What a press on the graph or the ruler started.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Stroke {
    /// Drawing freehand; the previous mouse position (tick, level).
    Draw((u32, f64)),
    /// Erasing; the previous mouse tick.
    Erase(u32),
    /// Selecting time from the anchor tick.
    Select(u32),
    /// A straight line from where the right button went down (`EventsUi::line`).
    Line,
}

/// The snap's unit in ticks at the editor's zoom: the global snap (none with Alt held).
pub fn unit(s: &State, alt: bool) -> u32 {
    if alt {
        return 1;
    }
    let ppq = u32::from(s.project.ppq.max(4));
    crate::piano_snap::unit_of(crate::snap::EditorSnap::Main, s.global_snap.now, ppq, s.events.px_per_beat).unwrap_or(1).max(1)
}

/// `tick` on the snap.
fn snapped(s: &State, tick: f64, alt: bool) -> u32 {
    let u = f64::from(unit(s, alt));
    ((tick.max(0.0) / u).floor() * u) as u32
}

/// The editor's menu (§4).
pub fn menu(s: &State, x: f32, y: f32) -> Popup {
    let any = !points(s).is_empty();
    let turn = PItem::new("Turn into automation clip", Cmd::Events(EvCmd::TurnIntoClip));
    let items = vec![
        PItem::head("Edit"),
        PItem::new("Select all", Cmd::Events(EvCmd::SelectAll)).key("Ctrl+A"),
        PItem::new("Delete", Cmd::Events(EvCmd::Delete)).key("Del"),
        PItem::new("Insert current controller value", Cmd::Events(EvCmd::InsertValue)).key("Ctrl+I"),
        PItem::sep(),
        if any { turn } else { turn.off() },
    ];
    Popup { x, y, items, sub: None, scroll: [0; 2] }
}

impl App {
    /// Edit automation on a control's menu: the editor shows that control's events in the current pattern, centred over
    /// the main window the first time (then where it was left), in front (§1).
    pub(crate) fn edit_events(&mut self, target: u32, name: &str, value: f64, range: Option<(f64, f64)>) -> Option<String> {
        let s = &mut self.state;
        if !open(s, target, name, value, range) {
            return None;
        }
        if !s.events.placed {
            let (w, h) = crate::views::events::SIZE;
            s.events.pos = (((s.screen.0 - w) / 2.0).max(0.0).round(), ((s.screen.1 - h) / 2.0).max(crate::views::work_y(s)).round());
            s.events.placed = true;
        }
        s.raise(Win::Events);
        self.invalidate();
        None
    }

    /// A control of the window, pressed.
    pub(crate) fn events_act(&mut self, a: EvAct) {
        match a {
            EvAct::Menu => {
                let r = crate::views::events::rect(&self.state);
                self.state.playlist.popup = Some(menu(&self.state, r.x + 8.0, r.y + 58.0));
                return self.invalidate();
            }
            EvAct::Tool(t) => self.state.events.tool = t,
            EvAct::Ruler => self.events_press(false, true),
            EvAct::Graph => self.events_press(false, false),
        }
        self.damage_win(Win::Events);
    }

    /// A press on the graph (`right`: the right button) or the ruler (`ruler`): a stroke starts (§3).
    pub(crate) fn events_press(&mut self, right: bool, ruler: bool) {
        let (x, y) = self.mouse;
        let (alt, ctrl) = (self.alt, self.ctrl);
        let s = &mut self.state;
        let tick = snapped(s, crate::views::events::tick_at(s, x), alt);
        let level = crate::views::events::level_at(s, y);
        let stroke = if ruler || ctrl || (!right && s.events.tool == Tool::Select) {
            s.events.sel = None;
            Stroke::Select(tick)
        } else if right {
            s.events.line = Some(((tick, level), (tick, level)));
            Stroke::Line
        } else if s.events.tool == Tool::Erase {
            piano::snapshot_as(s, EDIT);
            let u = unit(s, alt);
            erase(s, tick, tick + u - 1);
            Stroke::Erase(tick)
        } else {
            piano::snapshot_as(s, EDIT);
            let u = unit(s, alt);
            draw(s, (tick, level), (tick, level), u);
            Stroke::Draw((tick, level))
        };
        self.drag = Some(crate::app::Drag::Events(stroke));
        self.damage_win(Win::Events);
    }

    /// The mouse moved during a stroke.
    pub(crate) fn events_drag(&mut self, stroke: Stroke, x: f32, y: f32) {
        let alt = self.alt;
        let s = &mut self.state;
        let tick = snapped(s, crate::views::events::tick_at(s, x), alt);
        let level = crate::views::events::level_at(s, y);
        let u = unit(s, alt);
        let next = match stroke {
            Stroke::Draw(last) => {
                draw(s, last, (tick, level), u);
                Stroke::Draw((tick, level))
            }
            Stroke::Erase(last) => {
                erase(s, last.min(tick), last.max(tick) + u - 1);
                Stroke::Erase(tick)
            }
            Stroke::Select(anchor) => {
                s.events.sel = Some((anchor.min(tick), anchor.max(tick) + u));
                stroke
            }
            Stroke::Line => {
                if let Some(l) = &mut s.events.line {
                    l.1 = (tick, level);
                }
                stroke
            }
        };
        self.drag = Some(crate::app::Drag::Events(next));
        self.damage_win(Win::Events);
    }

    /// The button went up: a line is drawn (one undo step).
    pub(crate) fn events_release(&mut self, stroke: Stroke) {
        if stroke != Stroke::Line {
            return;
        }
        let alt = self.alt;
        let s = &mut self.state;
        if let Some((a, b)) = s.events.line.take() {
            piano::snapshot_as(s, EDIT);
            let u = unit(s, alt);
            draw(s, a, b, u);
        }
        self.damage_win(Win::Events);
    }

    /// The menu's entries (§4).
    pub(crate) fn events_cmd(&mut self, c: EvCmd) -> Option<String> {
        let s = &mut self.state;
        let m = match c {
            EvCmd::SelectAll => {
                select_all(s);
                None
            }
            EvCmd::Delete => {
                piano::snapshot_as(s, EDIT);
                delete(s);
                None
            }
            EvCmd::InsertValue => {
                piano::snapshot_as(s, EDIT);
                insert_value(s);
                None
            }
            EvCmd::TurnIntoClip => turn_into_clip(s),
        };
        self.invalidate();
        m
    }

    /// The editor's keys while it is in front (§4): Del, Ctrl+A, Ctrl+I.
    pub(crate) fn events_key(&mut self, key: &winit::keyboard::Key, ctrl: bool, alt: bool) -> bool {
        use winit::keyboard::{Key, NamedKey};
        let c = match key {
            Key::Named(NamedKey::Delete) if !ctrl && !alt => EvCmd::Delete,
            Key::Character(c) if ctrl && !alt && c.eq_ignore_ascii_case("a") => EvCmd::SelectAll,
            Key::Character(c) if ctrl && !alt && c.eq_ignore_ascii_case("i") => EvCmd::InsertValue,
            _ => return false,
        };
        if let Some(m) = self.events_cmd(c) {
            self.say(m);
        }
        true
    }

    /// The wheel over the graph or the ruler: it scrolls time, a beat a notch; with Ctrl it zooms about the mouse.
    pub(crate) fn events_wheel(&mut self, d: f32, ctrl: bool) {
        let x = self.mouse.0;
        let s = &mut self.state;
        let ppq = f64::from(s.project.ppq.max(4));
        if ctrl {
            let at = crate::views::events::tick_at(s, x);
            s.events.px_per_beat = (s.events.px_per_beat * 1.25f32.powf(d)).clamp(4.0, 600.0);
            let back = crate::views::events::tick_at(s, x) - s.events.scroll;
            s.events.scroll = (at - back).max(0.0);
        } else {
            s.events.scroll = (s.events.scroll - f64::from(d) * ppq).max(0.0);
        }
        self.damage_win(Win::Events);
    }
}

/// The undo steps of an edit in the editor and of Turn into automation clip (as the browser's History lists them).
pub const EDIT: &str = "Event editor edit";
pub const TURN_INTO_CLIP: &str = "Turn into automation clip";

/// Edit ▸ Turn into automation clip (§5): an automation clip channel named after the control, linked to it, whose
/// points follow the events (`clip_points`).
pub fn turn_into_clip(s: &mut State) -> Option<String> {
    let points = points(s);
    if points.is_empty() {
        return None;
    }
    let ppq = u32::from(s.project.ppq.max(4));
    let end = s.project.pattern_length(s.pattern);
    let clip = clip_points(&points, s.events.range, ppq, end, CLIP_TOLERANCE);
    let (target, name) = (s.events.target, s.events.name.clone());
    let row = crate::playlist::automation_channel(s, target, &name, clip);
    let name = s.project.channels.get(row).and_then(|c| c.name.clone()).unwrap_or(name);
    Some(format!("Created automation clip \"{name}\""))
}

/// A control point of the control at `tick`, for tests.
#[cfg(test)]
pub fn point(s: &State, tick: u32) -> Option<i32> {
    points(s).into_iter().find(|p| p.0 == tick).map(|p| p.1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{test_project, Action};
    use crate::knobs::KnobId;
    use os_ui::{Canvas, Rect};

    fn hit_rect(app: &App, a: &Action) -> Rect {
        app.hits.iter().rev().find(|h| &h.action == a).map(|h| h.rect).unwrap_or_else(|| panic!("no hit for {a:?}"))
    }

    /// A stroke puts an event on every snap unit between the mouse positions, on the straight line through them, in
    /// place of the control's events there; Erase and Delete take them away (§3, §4).
    #[test]
    fn draw_erase_and_delete() {
        let mut app = App::new(None);
        app.state.project = test_project();
        let s = &mut app.state;
        assert!(open(s, os_model::target::channel(0, 0), "Kick - Volume", 0.5, None));
        draw(s, (0, 0.0), (96, 1.0), 24);
        assert_eq!(points(s), vec![(0, 0), (24, 3200), (48, 6400), (72, 9600), (96, 12800)]);
        draw(s, (30, 0.5), (30, 0.5), 24);
        assert_eq!(point(s, 24), Some(6400), "on the snap");
        assert_eq!(erase(s, 40, 80), 2);
        s.events.sel = Some((0, 30));
        assert_eq!(delete(s), 2);
        assert_eq!(points(s), vec![(96, 12800)]);
        s.events.sel = None;
        insert_value(s);
        assert_eq!(point(s, 0), Some(6400), "the value it had when the editor opened");
        assert_eq!(delete(s), 2, "every event without a selection");
        // Another control's events stay.
        piano::set_control(s, 1, 0, 0, 100);
        assert_eq!(delete(s), 0);
        assert_eq!(piano::controls(s).len(), 1);
        // A mixer track's pan keeps its own unit.
        assert!(open(s, os_model::target::mixer(1, os_model::target::MIXER_PAN), "Insert 1 - Pan", 0.5, None));
        draw(s, (0, 0.0), (0, 0.0), 24);
        assert_eq!(points(s), vec![(0, -6400)]);
        assert!(!open(s, 0x4000_0000, "Main volume", 0.5, None), "no events for it");
    }

    /// The clip follows the events: a ramp recorded every 4 ticks becomes one straight line, a held level one more
    /// point to the pattern's end (§5).
    #[test]
    fn clip_points_follow_the_events() {
        let pts: Vec<(u32, i32)> = (0..=24).map(|k| (k * 4, k as i32 * 400)).collect();
        let c = clip_points(&pts, (0.0, 9600.0), 96, 384, CLIP_TOLERANCE);
        let at: Vec<(f64, f64)> = c.iter().map(|p| (p.beat, p.value)).collect();
        assert_eq!(at, vec![(0.0, 0.0), (1.0, 1.0), (4.0, 1.0)]);
        // A late first event: its level from the start.
        let c = clip_points(&[(96, 0), (192, 9600)], (0.0, 9600.0), 96, 384, CLIP_TOLERANCE);
        let at: Vec<(f64, f64)> = c.iter().map(|p| (p.beat, p.value)).collect();
        assert_eq!(at, vec![(0.0, 0.0), (1.0, 0.0), (2.0, 1.0), (4.0, 1.0)]);
        assert!(clip_points(&[], (0.0, 1.0), 96, 384, CLIP_TOLERANCE).is_empty());
    }

    /// Edit automation on a channel knob's menu opens the editor on that control; a drag draws; Turn into automation clip
    /// makes an automation clip channel linked to the control, one undo step (§1, §3, §5).
    #[test]
    fn edit_events_draw_and_turn_into_clip() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        let knob = Action::Knob(KnobId::ChanVol(0));
        let auto = app.automation_target(&knob);
        let menu = crate::rack::knob_menu(&app.state, KnobId::ChanVol(0), auto, 0.0, 0.0);
        let item = menu.items.iter().find(|i| i.label == "Edit automation").expect("Edit automation");
        assert!(item.enabled && item.cmd != Cmd::Nothing);
        crate::playlist::run(&mut app, item.cmd.clone());
        assert!(app.state.shown(Win::Events) && app.state.in_front(Win::Events));
        assert_eq!(app.state.events.target, os_model::target::channel(0, 0));
        app.paint(&mut c, None);

        let g = hit_rect(&app, &Action::Events(EvAct::Graph));
        app.mouse_move(g.x + 2.0, g.bottom() - 2.0);
        app.mouse_down();
        app.mouse_move(g.x + 2.0 + app.state.events.px_per_beat * 2.0, g.y + 2.0);
        app.mouse_up();
        let pts = points(&app.state);
        assert!(pts.len() >= 3, "{pts:?}");
        assert!(pts[0].1 < 200 && pts.last().unwrap().1 > 12600, "{pts:?}");

        let rows = app.state.project.channels.len();
        let m = app.events_cmd(EvCmd::TurnIntoClip);
        assert_eq!(m.as_deref(), Some("Created automation clip \"Kick - Channel volume\""), "{m:?}");
        let ch = app.state.project.channels.last().expect("clip channel");
        assert_eq!((app.state.project.channels.len(), ch.kind), (rows + 1, os_model::ChannelKind::Automation));
        let a = app.state.project.automation(ch.index).expect("clip");
        assert_eq!(a.targets, vec![os_model::target::channel(0, 0)]);
        assert!(a.points.len() >= 2 && a.points[0].value < 0.02);
        assert_eq!(points(&app.state), pts, "the events stay");
        crate::history::undo(&mut app.state);
        assert_eq!(app.state.project.channels.len(), rows);
    }
}
