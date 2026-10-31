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
    /// Draw, ignoring the snap: an event on every tick.
    Paint,
    /// Left drag deletes the events it passes over.
    Erase,
    /// A drag gives the events under it the levels of a straight line (right drag: a smooth one).
    Interpolate,
    /// Left drag selects a stretch of time.
    Select,
    /// A click zooms in about it, a drag zooms on the stretch dragged over, a right click zooms out.
    Zoom,
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
    /// The editor behind the one in front (Edit automation in a window): a click brings it in front.
    Front,
}

/// An entry of the editor's menu (§4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvCmd {
    /// The selection's events to the editor's clipboard; Cut deletes them too.
    Cut,
    Copy,
    /// The clipboard's events at the selection's start (or the pattern's), in place of those they cover.
    Paste,
    SelectAll,
    /// The selection's events, or every event of the control without one.
    Delete,
    /// An event with the control's value as it was when the editor opened, at the selection's start (or the
    /// pattern's).
    InsertValue,
    /// The selection's events copied right after it.
    Duplicate,
    /// Moves the events from the selection's start on later by its length, or pulls those after it back over it.
    InsertSpace,
    DeleteSpace,
    /// The selection moved back or on by its own length.
    SelectPrev,
    SelectNext,
    /// The Relocate events check: Paste inserts, and Cut and Delete close the gap.
    Relocate,
    /// Zoom in or out about the graph's middle, or to the selection.
    ZoomIn,
    ZoomOut,
    ZoomSelection,
    /// Tools ▸ Scale levels and LFO: the piano roll's dialogs on the control's events.
    ScaleLevels,
    Lfo,
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
    /// Cut or copied events: (ticks from the selection's start, level), and the selection's length.
    pub clipboard: Option<(Vec<(u32, f64)>, u32)>,
    /// Relocate events (§4): Paste makes room for what it pastes, Cut and Delete pull the later events back.
    pub relocate: bool,
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
            clipboard: None,
            relocate: false,
        }
    }
}

/// The editor shows the control `target` named `name` at `value` (0..1); `range`: a plugin parameter's own (min,
/// max), else the target's (`None` when the control takes no control points: the editor does not open).
pub fn open(s: &mut State, target: u32, name: &str, value: f64, range: Option<(f64, f64)>) -> bool {
    open_in(&mut s.events, target, name, value, range)
}

/// [`open`] on editor `e`.
fn open_in(e: &mut EventsUi, target: u32, name: &str, value: f64, range: Option<(f64, f64)>) -> bool {
    use os_model::target;
    let range = match range {
        Some((min, max)) => (f64::from(target::plugin_point_value(min, max, 0.0)), f64::from(target::plugin_point_value(min, max, 1.0))),
        None => match target::point_range(target::decode(target)) {
            Some(r) => r,
            None => return false,
        },
    };
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

/// Edit automation in a window (§3a), on the menus that have Edit automation: enabled where Edit automation is.
pub fn window_item(auto: Option<&(u32, String, f64)>, range: Option<(f64, f64)>) -> PItem {
    let mut item = menu_item(auto, range);
    item.label = "Edit automation in a window".into();
    if let Cmd::EditEvents(t, name, v, range) = item.cmd {
        item.cmd = Cmd::EditEventsWindow(t, name, v, range);
    }
    item
}

/// Closes the editor in front (§3a): the one behind it, if any, comes in front and keeps the clipboard; else the
/// window closes.
pub fn close(s: &mut State) {
    match s.events_back.take() {
        Some(mut back) => {
            back.clipboard = s.events.clipboard.take();
            s.events = back;
        }
        None => s.hide(Win::Events),
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
    points_of(s, s.events.target)
}

/// Control `target`'s events in the current pattern.
pub fn points_of(s: &State, target: u32) -> Vec<(u32, i32)> {
    let kind = target;
    piano::controls(s).iter().filter(|c| c.kind == kind).map(|c| (c.position, c.value)).collect()
}

fn set(s: &mut State, tick: u32, level: f64) {
    let v = value(s.events.range, level);
    set_value(s, tick, v);
}

fn set_value(s: &mut State, tick: u32, v: i32) {
    let kind = s.events.target;
    piano::set_control(s, (kind >> 16) as u16, kind as u16, tick, v);
}

/// What Copy and Duplicate take without a selection: the pattern's length, or to the last event when that lies later.
fn whole(s: &State) -> (u32, u32) {
    (0, s.project.pattern_length(s.pattern).max(points(s).last().map_or(0, |p| p.0 + 1)).max(1))
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

/// The Interpolate tool (spec §1): the control's events from `a` to `b` (tick, level) keep their times and take the
/// levels of the straight line between them, or with `smooth` of a half cosine from one to the other. Returns how many
/// there were.
pub fn interpolate(s: &mut State, a: (u32, f64), b: (u32, f64), smooth: bool) -> usize {
    let ((t0, l0), (t1, l1)) = if a.0 <= b.0 { (a, b) } else { (b, a) };
    let under: Vec<u32> = points(s).into_iter().map(|p| p.0).filter(|t| (t0..=t1).contains(t)).collect();
    for &t in &under {
        let k = if t1 == t0 { 1.0 } else { f64::from(t - t0) / f64::from(t1 - t0) };
        let k = if smooth { (1.0 - (k * std::f64::consts::PI).cos()) / 2.0 } else { k };
        set(s, t, l0 + (l1 - l0) * k);
    }
    s.piano.dirty = true;
    under.len()
}

/// Edit ▸ Duplicate (Ctrl+B): the selection's events (all of them without one) copied right after it, in place of the
/// control's events there; the copy becomes the selection.
pub fn duplicate(s: &mut State) -> bool {
    let (a, b) = s.events.sel.unwrap_or_else(|| whole(s));
    let len = (b - a).max(1);
    let events: Vec<(u32, i32)> = points(s).into_iter().filter(|p| (a..b).contains(&p.0)).collect();
    if events.is_empty() {
        return false;
    }
    erase(s, a + len, a + 2 * len - 1);
    for (t, v) in events {
        set_value(s, t + len, v);
    }
    s.events.sel = Some((a + len, a + 2 * len));
    true
}

/// Edit ▸ Insert space (Ctrl+Ins): the events from the selection's start on move later by its length. Delete space
/// (Ctrl+Del): the selection's events go and the later ones move back by its length. Both need a selection.
pub fn space(s: &mut State, insert: bool) -> bool {
    let Some((a, b)) = s.events.sel.filter(|(a, b)| b > a) else { return false };
    let len = b - a;
    let from = if insert { a } else { b };
    let moved: Vec<(u32, i32)> = points(s).into_iter().filter(|p| p.0 >= from).collect();
    erase(s, a, u32::MAX);
    for (t, v) in moved {
        set_value(s, if insert { t + len } else { t - len }, v);
    }
    true
}

/// The control's events from `from` up to `to` give way to `points` (tick, stored value).
pub fn replace(s: &mut State, from: u32, to: u32, points: &[(u32, i32)]) {
    erase(s, from, to.saturating_sub(1).max(from));
    for &(t, v) in points {
        set_value(s, t, v);
    }
    s.piano.dirty = true;
}

/// Edit ▸ Select previous (Ctrl+Left) / Select next (Ctrl+Right): the selection moves back or on by its length (the
/// original's manual); back stops at the pattern's start. Returns whether it moved.
pub fn select_step(s: &mut State, next: bool) -> bool {
    let Some((a, b)) = s.events.sel.filter(|(a, b)| b > a) else { return false };
    let len = b - a;
    let at = if next { b } else { a.saturating_sub(len) };
    if at == a {
        return false;
    }
    s.events.sel = Some((at, at + len));
    true
}

/// Zoom in or out (`factor` times the pixels a beat) about the graph's middle.
pub fn zoom(s: &mut State, factor: f32) {
    let g = crate::views::events::graph(s);
    zoom_about(s, factor, g.x + g.w / 2.0);
}

/// Zoom in or out about `x`: the tick there stays there.
pub fn zoom_about(s: &mut State, factor: f32, x: f32) {
    let at = crate::views::events::tick_at(s, x);
    s.events.px_per_beat = (s.events.px_per_beat * factor).clamp(ZOOM.0, ZOOM.1);
    let back = crate::views::events::tick_at(s, x) - s.events.scroll;
    s.events.scroll = (at - back).max(0.0);
}

/// The Zoom tool's and the menu's zoom step.
pub const ZOOM_STEP: f32 = 1.5;

/// Zoom ▸ on selection: the selection fills the graph's width.
pub fn zoom_selection(s: &mut State) -> bool {
    let Some((a, b)) = s.events.sel.filter(|(a, b)| b > a) else { return false };
    let w = crate::views::events::graph(s).w;
    let ppq = f32::from(s.project.ppq.max(4));
    s.events.px_per_beat = (w * ppq / (b - a) as f32).clamp(ZOOM.0, ZOOM.1);
    s.events.scroll = f64::from(a);
    true
}

/// The zoom's limits, pixels a beat.
pub const ZOOM: (f32, f32) = (4.0, 600.0);

/// Edit ▸ Delete: the selection's events, or all of them. With Relocate events on, the later events move back over a
/// selection's place.
pub fn delete(s: &mut State) -> usize {
    if s.events.relocate && s.events.sel.is_some_and(|(a, b)| b > a) {
        let n = points(s).iter().filter(|p| s.events.sel.is_some_and(|(a, b)| (a..b).contains(&p.0))).count();
        space(s, false);
        return n;
    }
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

/// Edit ▸ Copy: the selection's events (all of them without one) as levels, from the selection's start.
pub fn copy(s: &mut State) -> bool {
    let range = s.events.range;
    let (a, b) = s.events.sel.unwrap_or_else(|| whole(s));
    let events: Vec<(u32, f64)> = points(s).into_iter().filter(|p| (a..b).contains(&p.0)).map(|(t, v)| (t - a, level(range, v))).collect();
    if events.is_empty() {
        return false;
    }
    s.events.clipboard = Some((events, b - a));
    true
}

/// Edit ▸ Paste: the clipboard at the selection's start (the pattern's without one), the control's events in the
/// stretch it covers replaced; the stretch becomes the selection. With Relocate events on, the events from there on
/// move later by its length first, so none is replaced.
pub fn paste(s: &mut State) -> bool {
    let Some((events, len)) = s.events.clipboard.clone() else { return false };
    let at = s.events.sel.map_or(0, |(a, _)| a);
    if s.events.relocate {
        s.events.sel = Some((at, at + len.max(1)));
        space(s, true);
    }
    erase(s, at, at + len.max(1) - 1);
    for (t, l) in events {
        set(s, at + t, l);
    }
    s.events.sel = Some((at, at + len.max(1)));
    true
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

/// The automation clip points that follow `points` (tick, value) in `range`, `ppq` ticks a beat: each event's level
/// holds until the next one, as the pattern plays it; the steps a ramp of events makes become straight lines, and
/// points the line through their neighbours already passes within `tol` of are left out. Before the first event the
/// clip holds `start` (the control's level), stepping to the event over its last tick; it ends on the last event,
/// as the original's does (spec §6).
pub fn clip_points(points: &[(u32, i32)], range: (f64, f64), ppq: u32, start: f64, tol: f64) -> Vec<AutoPoint> {
    let Some(first) = points.first() else { return Vec::new() };
    let beat = |t: u32| f64::from(t) / f64::from(ppq.max(1));
    // The held levels as a line: a vertex where each event takes over.
    let mut line: Vec<(f64, f64)> = Vec::new();
    if first.0 > 0 {
        line.push((0.0, start));
        line.push((beat(first.0 - 1), start));
    }
    for &(t, v) in points {
        line.push((beat(t), level(range, v)));
    }
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
    /// The Interpolate tool's line (`EventsUi::line`), smooth with the right button: made on release.
    Interpolate(bool),
    /// The Zoom tool's drag from the anchor tick; the selection it hides while the stretch shows.
    Zoom(u32, Option<(u32, u32)>),
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
    let on_sel = |p: PItem| if s.events.sel.is_some() { p } else { p.off() };
    let items = vec![
        PItem::head("Edit"),
        PItem::new("Cut", Cmd::Events(EvCmd::Cut)).key("Ctrl+X"),
        PItem::new("Copy", Cmd::Events(EvCmd::Copy)).key("Ctrl+C"),
        {
            let p = PItem::new("Paste", Cmd::Events(EvCmd::Paste)).key("Ctrl+V");
            if s.events.clipboard.is_some() { p } else { p.off() }
        },
        PItem::new("Select all", Cmd::Events(EvCmd::SelectAll)).key("Ctrl+A"),
        on_sel(PItem::new("Select previous", Cmd::Events(EvCmd::SelectPrev)).key("Ctrl+Left")),
        on_sel(PItem::new("Select next", Cmd::Events(EvCmd::SelectNext)).key("Ctrl+Right")),
        PItem::new("Delete", Cmd::Events(EvCmd::Delete)).key("Del"),
        PItem::new("Duplicate", Cmd::Events(EvCmd::Duplicate)).key("Ctrl+B"),
        PItem::sep(),
        on_sel(PItem::new("Insert space", Cmd::Events(EvCmd::InsertSpace)).key("Ctrl+Ins")),
        on_sel(PItem::new("Delete space", Cmd::Events(EvCmd::DeleteSpace)).key("Ctrl+Del")),
        PItem::sep(),
        PItem::new("Insert current controller value", Cmd::Events(EvCmd::InsertValue)).key("Ctrl+I"),
        PItem::sep(),
        if any { turn } else { turn.off() },
        PItem::head("Tools"),
        {
            let p = PItem::new("Scale levels...", Cmd::Events(EvCmd::ScaleLevels));
            if any { p } else { p.off() }
        },
        PItem::new("LFO...", Cmd::Events(EvCmd::Lfo)).key("Ctrl+L"),
        PItem::head("Zoom"),
        PItem::new("Zoom in", Cmd::Events(EvCmd::ZoomIn)).key("PgUp"),
        PItem::new("Zoom out", Cmd::Events(EvCmd::ZoomOut)).key("PgDn"),
        on_sel(PItem::new("Zoom on selection", Cmd::Events(EvCmd::ZoomSelection)).key("Shift+5")),
        PItem::sep(),
        PItem::new("Relocate events", Cmd::Events(EvCmd::Relocate)).mark(crate::playlist::Mark::Check(s.events.relocate)),
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
        // A second window left from before the editor closed is not brought back.
        if !s.shown(Win::Events) {
            s.events_back = None;
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

    /// Edit automation in a window (§3a; Lemon Studio's design after the original's "Edit events in new window"): a
    /// second editor window on the control, in front, 30 px right of and below the editor showing, which keeps its
    /// control and goes behind. With no editor showing, this is Edit automation. A second window already open gives
    /// way: there are two at most.
    pub(crate) fn edit_events_window(&mut self, target: u32, name: &str, value: f64, range: Option<(f64, f64)>) -> Option<String> {
        if !self.state.shown(Win::Events) {
            return self.edit_events(target, name, value, range);
        }
        let s = &mut self.state;
        let first = &s.events;
        let mut e = EventsUi { pos: (first.pos.0 + 30.0, first.pos.1 + 30.0), placed: true, tool: first.tool, px_per_beat: first.px_per_beat, relocate: first.relocate, ..EventsUi::default() };
        if !open_in(&mut e, target, name, value, range) {
            return None;
        }
        e.clipboard = s.events.clipboard.take();
        s.events_back = Some(std::mem::replace(&mut s.events, e));
        s.raise(Win::Events);
        self.invalidate();
        None
    }

    /// A click on the editor behind (§3a): the two editors change places, the clipboard staying with the one in front.
    fn events_front(&mut self) {
        let s = &mut self.state;
        let Some(back) = s.events_back.as_mut() else { return };
        std::mem::swap(&mut s.events, back);
        std::mem::swap(&mut s.events.clipboard, &mut back.clipboard);
        s.raise(Win::Events);
        self.damage_win(Win::Events);
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
            EvAct::Front => return self.events_front(),
        }
        self.damage_win(Win::Events);
    }

    /// A press on the graph (`right`: the right button) or the ruler (`ruler`): a stroke starts (§3).
    pub(crate) fn events_press(&mut self, right: bool, ruler: bool) {
        let (x, y) = self.mouse;
        let ctrl = self.ctrl;
        let s = &mut self.state;
        let alt = self.alt || s.events.tool == Tool::Paint;
        let tick = snapped(s, crate::views::events::tick_at(s, x), alt);
        let level = crate::views::events::level_at(s, y);
        let stroke = if ruler || ctrl || (!right && s.events.tool == Tool::Select) {
            s.events.sel = None;
            Stroke::Select(tick)
        } else if s.events.tool == Tool::Zoom {
            if right {
                zoom_about(s, 1.0 / ZOOM_STEP, x);
                self.damage_win(Win::Events);
                return;
            }
            let anchor = crate::views::events::tick_at(s, x).max(0.0) as u32;
            Stroke::Zoom(anchor, s.events.sel.take())
        } else if s.events.tool == Tool::Interpolate {
            s.events.line = Some(((tick, level), (tick, level)));
            Stroke::Interpolate(right)
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
        let s = &mut self.state;
        let alt = self.alt || s.events.tool == Tool::Paint;
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
            // The stretch shows as the selection until the button goes up.
            Stroke::Zoom(anchor, _) => {
                let t = crate::views::events::tick_at(s, x).max(0.0) as u32;
                s.events.sel = Some((anchor.min(t), anchor.max(t)));
                stroke
            }
            Stroke::Line | Stroke::Interpolate(_) => {
                if let Some(l) = &mut s.events.line {
                    l.1 = (tick, level);
                }
                stroke
            }
        };
        self.drag = Some(crate::app::Drag::Events(next));
        self.damage_win(Win::Events);
    }

    /// The button went up: a line is drawn, or the events under it interpolated (one undo step).
    pub(crate) fn events_release(&mut self, stroke: Stroke) {
        let s = &mut self.state;
        let alt = self.alt || s.events.tool == Tool::Paint;
        // Zoom: a drag of 4 px or more zooms on its stretch, a click zooms in about it; the selection comes back.
        if let Stroke::Zoom(anchor, kept) = stroke {
            let dragged = s.events.sel.take().filter(|(a, b)| {
                let px = f64::from(s.events.px_per_beat) / f64::from(s.project.ppq.max(4));
                f64::from(b - a) * px >= 4.0
            });
            match dragged {
                Some(r) => {
                    s.events.sel = Some(r);
                    zoom_selection(s);
                }
                None => {
                    let x = crate::views::events::x_of(s, f64::from(anchor));
                    zoom_about(s, ZOOM_STEP, x);
                }
            }
            s.events.sel = kept;
            self.damage_win(Win::Events);
            return;
        }
        let line = match stroke {
            Stroke::Line | Stroke::Interpolate(_) => s.events.line.take(),
            _ => return,
        };
        if let Some((a, b)) = line {
            piano::snapshot_as(s, EDIT);
            match stroke {
                Stroke::Interpolate(smooth) => {
                    interpolate(s, a, b, smooth);
                }
                _ => {
                    let u = unit(s, alt);
                    draw(s, a, b, u);
                }
            }
        }
        self.damage_win(Win::Events);
    }

    /// The menu's entries (§4).
    pub(crate) fn events_cmd(&mut self, c: EvCmd) -> Option<String> {
        let s = &mut self.state;
        let m = match c {
            EvCmd::Copy => {
                copy(s);
                None
            }
            EvCmd::Cut => {
                if copy(s) {
                    piano::snapshot_as(s, EDIT);
                    delete(s);
                }
                None
            }
            EvCmd::Paste => {
                if s.events.clipboard.is_some() {
                    piano::snapshot_as(s, EDIT);
                    paste(s);
                }
                None
            }
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
            EvCmd::Duplicate => {
                piano::snapshot_as(s, EDIT);
                duplicate(s);
                None
            }
            EvCmd::InsertSpace | EvCmd::DeleteSpace => {
                if s.events.sel.is_some() {
                    piano::snapshot_as(s, EDIT);
                    space(s, c == EvCmd::InsertSpace);
                }
                None
            }
            EvCmd::SelectPrev | EvCmd::SelectNext => {
                select_step(s, c == EvCmd::SelectNext);
                None
            }
            EvCmd::Relocate => {
                s.events.relocate = !s.events.relocate;
                Some(format!("Relocate events {}", if s.events.relocate { "on" } else { "off" }))
            }
            EvCmd::ZoomIn | EvCmd::ZoomOut => {
                zoom(s, if c == EvCmd::ZoomIn { ZOOM_STEP } else { 1.0 / ZOOM_STEP });
                None
            }
            EvCmd::ZoomSelection => {
                zoom_selection(s);
                None
            }
            EvCmd::ScaleLevels | EvCmd::Lfo => {
                let tool = if c == EvCmd::Lfo { crate::piano_dlg::Tool::Lfo } else { crate::piano_dlg::Tool::Levels };
                self.open_events_tool(tool);
                None
            }
            EvCmd::TurnIntoClip => turn_into_clip(s),
        };
        self.invalidate();
        m
    }

    /// Tools ▸ Scale levels / LFO: the piano roll's dialog, where it was last left (centred the first time), on the
    /// control's events (§4).
    fn open_events_tool(&mut self, tool: crate::piano_dlg::Tool) {
        let s = &mut self.state;
        let i = tool.index();
        if !s.piano_dlg.placed[i] {
            let (w, h) = crate::views::piano_dlg::size(&s.piano_dlg, tool);
            s.piano_dlg.pos[i] = (((s.screen.0 - w) / 2.0).max(0.0).round(), ((s.screen.1 - h) / 2.0).max(crate::views::work_y(s)).round());
            s.piano_dlg.placed[i] = true;
        }
        crate::piano_dlg::open_events(s, tool);
        s.raise(Win::PianoDlg);
    }

    /// The editor's keys while it is in front (§4): Del, Ctrl+X / C / V, Ctrl+A, Ctrl+I, Ctrl+B, Ctrl+Ins / Del,
    /// Ctrl+Left / Right, PgUp / PgDn, Shift+5.
    pub(crate) fn events_key(&mut self, key: &winit::keyboard::Key, ctrl: bool, alt: bool) -> bool {
        use winit::keyboard::{Key, NamedKey};
        let shift = self.shift;
        let c = match key {
            Key::Named(NamedKey::ArrowLeft) if ctrl && !alt => EvCmd::SelectPrev,
            Key::Named(NamedKey::ArrowRight) if ctrl && !alt => EvCmd::SelectNext,
            Key::Named(NamedKey::PageUp) if !ctrl && !alt => EvCmd::ZoomIn,
            Key::Named(NamedKey::PageDown) if !ctrl && !alt => EvCmd::ZoomOut,
            Key::Character(c) if shift && !ctrl && !alt && (c == "5" || c == "%") => EvCmd::ZoomSelection,
            Key::Named(NamedKey::Delete) if !ctrl && !alt => EvCmd::Delete,
            Key::Named(NamedKey::Delete) if ctrl && !alt => EvCmd::DeleteSpace,
            Key::Named(NamedKey::Insert) if ctrl && !alt => EvCmd::InsertSpace,
            Key::Character(c) if ctrl && !alt && c.eq_ignore_ascii_case("b") => EvCmd::Duplicate,
            Key::Character(c) if ctrl && !alt && c.eq_ignore_ascii_case("x") => EvCmd::Cut,
            Key::Character(c) if ctrl && !alt && c.eq_ignore_ascii_case("c") => EvCmd::Copy,
            Key::Character(c) if ctrl && !alt && c.eq_ignore_ascii_case("v") => EvCmd::Paste,
            Key::Character(c) if ctrl && !alt && c.eq_ignore_ascii_case("a") => EvCmd::SelectAll,
            Key::Character(c) if ctrl && !alt && c.eq_ignore_ascii_case("i") => EvCmd::InsertValue,
            Key::Character(c) if ctrl && !alt && c.eq_ignore_ascii_case("l") => EvCmd::Lfo,
            // Z: the Zoom tool (the original's key).
            Key::Character(c) if !ctrl && !alt && !shift && c.eq_ignore_ascii_case("z") => {
                self.state.events.tool = Tool::Zoom;
                return true;
            }
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
            s.events.px_per_beat = (s.events.px_per_beat * 1.25f32.powf(d)).clamp(ZOOM.0, ZOOM.1);
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
/// points follow the events (`clip_points`); the events leave the pattern, the clip now drives the control, and the
/// editor closes, as in the original (§6).
pub fn turn_into_clip(s: &mut State) -> Option<String> {
    let points = points(s);
    if points.is_empty() {
        return None;
    }
    let ppq = u32::from(s.project.ppq.max(4));
    let clip = clip_points(&points, s.events.range, ppq, s.events.value, CLIP_TOLERANCE);
    let (target, name) = (s.events.target, s.events.name.clone());
    let row = crate::playlist::automation_channel(s, target, &name, clip);
    erase(s, 0, u32::MAX);
    close(s);
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
        // Copy and paste: the selection's events from its start, pasted over the stretch they cover.
        draw(s, (24, 0.25), (48, 0.5), 24);
        s.events.sel = Some((24, 72));
        assert!(copy(s));
        s.events.sel = Some((192, 200));
        piano::set_control(s, 0, 0, 200, 1);
        assert!(paste(s));
        assert_eq!(points(s).iter().filter(|p| p.0 >= 192).copied().collect::<Vec<_>>(), vec![(192, 3200), (216, 6400)]);
        assert_eq!(s.events.sel, Some((192, 240)));
        s.events.sel = None;
        assert_eq!(delete(s), 6, "every event without a selection");
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

    /// Interpolate, Duplicate, Insert and Delete space (spec §1, §4).
    #[test]
    fn interpolate_duplicate_and_space() {
        let mut app = App::new(None);
        app.state.project = test_project();
        let s = &mut app.state;
        assert!(open(s, os_model::target::channel(0, 0), "Kick - Volume", 0.5, None));
        for t in [0, 24, 48, 72, 96] {
            set(s, t, 0.25);
        }
        assert_eq!(interpolate(s, (0, 0.0), (96, 1.0), false), 5);
        assert_eq!(points(s), vec![(0, 0), (24, 3200), (48, 6400), (72, 9600), (96, 12800)]);
        assert_eq!(interpolate(s, (0, 0.0), (96, 1.0), true), 5);
        assert_eq!(point(s, 48), Some(6400), "smooth: the middle is the same");
        assert_eq!(point(s, 24), Some(1875), "smooth: a slower start");
        assert_eq!(interpolate(s, (200, 0.0), (300, 1.0), false), 0, "no events under it");

        s.events.sel = Some((0, 48));
        assert!(duplicate(s));
        assert_eq!(points(s), vec![(0, 0), (24, 1875), (48, 0), (72, 1875), (96, 12800)]);
        assert_eq!(s.events.sel, Some((48, 96)));
        assert!(space(s, true), "insert space");
        assert_eq!(points(s), vec![(0, 0), (24, 1875), (96, 0), (120, 1875), (144, 12800)]);
        s.events.sel = Some((0, 96));
        assert!(space(s, false), "delete space");
        assert_eq!(points(s), vec![(0, 0), (24, 1875), (48, 12800)]);
        s.events.sel = None;
        assert!(!space(s, true), "needs a selection");
    }

    /// Tools ▸ Scale levels and LFO: the piano roll's dialogs preview on the control's events, Accept is one undo step
    /// and × puts them back (§4).
    #[test]
    fn scale_levels_and_lfo() {
        use crate::piano_dlg::{self as dlg, Tool as DTool};
        let mut app = App::new(None);
        app.state.project = test_project();
        let s = &mut app.state;
        assert!(open(s, os_model::target::channel(0, 0), "Kick - Volume", 0.5, None));
        for t in [0, 24, 48] {
            set(s, t, 0.5);
        }
        set(s, 400, 0.25);
        let before = points(s);
        app.events_cmd(EvCmd::ScaleLevels);
        let s = &mut app.state;
        assert!(s.shown(Win::PianoDlg) && s.piano_dlg.on_events);
        assert_eq!(s.piano_dlg.tool.events_title(), "Events - level scaling");
        assert_eq!(points(s), before, "the settings kept: nothing changes yet");
        s.piano_dlg.levels.mult = 0.5;
        dlg::preview(s);
        assert_eq!(point(s, 24), Some(3200), "half of 50 %");
        assert_eq!(point(s, 400), Some(1600));
        assert_eq!(dlg::accept(s), "Event editor: levels scaled");
        assert!(piano::undo(s));
        assert_eq!(points(s), before, "one undo step");

        // LFO over the selection; × puts the events back.
        s.events.sel = Some((0, 96));
        app.events_cmd(EvCmd::Lfo);
        let s = &mut app.state;
        assert_eq!(s.piano_dlg.tool, DTool::Lfo);
        let lfo: Vec<(u32, i32)> = points(s).into_iter().filter(|p| p.0 < 96).collect();
        assert!(lfo.len() > 10 && lfo.windows(2).all(|w| w[0].1 != w[1].1), "an event wherever the value changes");
        assert_eq!(point(s, 400), Some(3200), "outside the selection: kept");
        app.act(crate::app::Action::CloseWin(Win::PianoDlg));
        assert_eq!(points(&app.state), before);
        // Without a selection: the first bar.
        app.state.events.sel = None;
        app.events_cmd(EvCmd::Lfo);
        let s = &mut app.state;
        let bar = 4 * u32::from(s.project.ppq);
        assert!(points(s).iter().filter(|p| p.0 < bar).count() > 10);
        assert_eq!(point(s, 400.max(bar)), if bar > 400 { None } else { Some(3200) });
        dlg::accept(s);
        assert!(!s.piano_dlg.session_open());
    }

    /// The Zoom tool (§3): a click zooms in about it, a drag on the stretch, a right click out; Z picks it.
    #[test]
    fn zoom_tool() {
        use crate::app::Drag;
        let mut app = App::new(None);
        app.state.project = test_project();
        assert!(open(&mut app.state, os_model::target::channel(0, 0), "Kick - Volume", 0.5, None));
        app.state.z.push(Win::Events);
        assert!(app.events_key(&winit::keyboard::Key::Character("z".into()), false, false));
        assert_eq!(app.state.events.tool, Tool::Zoom);
        let g = crate::views::events::graph(&app.state);
        let ppq = f64::from(app.state.project.ppq);
        app.state.events.sel = Some((0, 10));
        // A click: in, the tick under it stays.
        app.mouse = (g.x + 200.0, g.y + 20.0);
        let (px, at) = (app.state.events.px_per_beat, crate::views::events::tick_at(&app.state, g.x + 200.0));
        app.events_press(false, false);
        let Some(Drag::Events(st)) = app.drag.take() else { panic!("a zoom stroke") };
        app.events_release(st);
        assert_eq!(app.state.events.px_per_beat, px * ZOOM_STEP);
        assert!((crate::views::events::tick_at(&app.state, g.x + 200.0) - at).abs() < 1e-6);
        assert_eq!(app.state.events.sel, Some((0, 10)), "the selection stays");
        // A drag: on its stretch.
        app.state.events.scroll = 0.0;
        app.mouse = (g.x + 10.0, g.y + 20.0);
        app.events_press(false, false);
        let Some(Drag::Events(st)) = app.drag.take() else { panic!() };
        let to = crate::views::events::x_of(&app.state, 8.0 * ppq);
        let from = crate::views::events::tick_at(&app.state, g.x + 10.0).floor();
        app.events_drag(st, to, g.y + 20.0);
        let Some(Drag::Events(st)) = app.drag.take() else { panic!() };
        app.events_release(st);
        assert!((app.state.events.scroll - from).abs() < 1.0);
        assert!((crate::views::events::x_of(&app.state, 8.0 * ppq) - (g.x + g.w)).abs() < 1.0, "the stretch fills the graph");
        assert_eq!(app.state.events.sel, Some((0, 10)));
        // A right click: out.
        let px = app.state.events.px_per_beat;
        app.events_press(true, false);
        assert!(app.drag.is_none());
        assert!((app.state.events.px_per_beat - px / ZOOM_STEP).abs() < 1e-3);
    }

    /// Select previous / next, Relocate events and the zoom (§4).
    #[test]
    fn select_step_relocate_and_zoom() {
        let mut app = App::new(None);
        app.state.project = test_project();
        let s = &mut app.state;
        assert!(open(s, os_model::target::channel(0, 0), "Kick - Volume", 0.5, None));
        assert!(!select_step(s, true), "needs a selection");
        s.events.sel = Some((96, 144));
        assert!(select_step(s, true));
        assert_eq!(s.events.sel, Some((144, 192)));
        assert!(select_step(s, false) && select_step(s, false));
        assert_eq!(s.events.sel, Some((48, 96)));
        assert!(select_step(s, false));
        assert_eq!(s.events.sel, Some((0, 48)), "stops at the start");
        assert!(!select_step(s, false));

        for t in [0, 24, 48, 72] {
            set(s, t, 0.25);
        }
        s.events.relocate = true;
        s.events.sel = Some((0, 48));
        assert!(copy(s));
        assert_eq!(delete(s), 2);
        assert_eq!(points(s), vec![(0, 3200), (24, 3200)], "the later events move back");
        s.events.sel = Some((24, 30));
        assert!(paste(s));
        assert_eq!(points(s), vec![(0, 3200), (24, 3200), (48, 3200), (72, 3200)], "pasted in, the event at 24 moved on");
        assert_eq!(s.events.sel, Some((24, 72)));

        let px = s.events.px_per_beat;
        zoom(s, 2.0);
        assert_eq!(s.events.px_per_beat, px * 2.0);
        let end = 96 + 8 * u32::from(s.project.ppq);
        s.events.sel = Some((96, end));
        assert!(zoom_selection(s));
        assert_eq!(s.events.scroll, 96.0);
        let g = crate::views::events::graph(s);
        assert!((crate::views::events::x_of(s, f64::from(end)) - (g.x + g.w)).abs() < 0.01, "the selection fills the graph");

        // The keys.
        s.events.sel = Some((0, 48));
        app.state.z.push(Win::Events);
        assert!(app.events_key(&winit::keyboard::Key::Named(winit::keyboard::NamedKey::ArrowRight), true, false));
        assert_eq!(app.state.events.sel, Some((48, 96)));
        assert_eq!(app.events_cmd(EvCmd::Relocate).as_deref(), Some("Relocate events off"));
    }

    /// The clip follows the events: a ramp recorded every 4 ticks becomes one straight line ending on the last event
    /// (§5, §6).
    #[test]
    fn clip_points_follow_the_events() {
        let pts: Vec<(u32, i32)> = (0..=24).map(|k| (k * 4, k as i32 * 400)).collect();
        let c = clip_points(&pts, (0.0, 9600.0), 96, 0.5, CLIP_TOLERANCE);
        let at: Vec<(f64, f64)> = c.iter().map(|p| (p.beat, p.value)).collect();
        assert_eq!(at, vec![(0.0, 0.0), (1.0, 1.0)]);
        // A late first event: the control's level until a tick before it.
        let c = clip_points(&[(96, 0), (192, 9600)], (0.0, 9600.0), 96, 0.5, CLIP_TOLERANCE);
        let at: Vec<(f64, f64)> = c.iter().map(|p| (p.beat, p.value)).collect();
        assert_eq!(at, vec![(0.0, 0.5), (95.0 / 96.0, 0.5), (1.0, 0.0), (2.0, 1.0)]);
        assert!(clip_points(&[], (0.0, 1.0), 96, 0.5, CLIP_TOLERANCE).is_empty());
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
        assert!(points(&app.state).is_empty(), "the events leave the pattern");
        assert!(!app.state.shown(Win::Events));
        crate::history::undo(&mut app.state);
        assert_eq!(app.state.project.channels.len(), rows);
        assert_eq!(points(&app.state), pts, "undo brings the events back");
    }

    /// Edit automation in a window (§3a): with no editor showing it is Edit automation; with one, a second editor on the
    /// panning knob comes in front, 30 px off the first, which keeps the volume knob behind it; both are drawn; a click
    /// on the one behind brings it in front, the clipboard staying in front; closing the front one brings the other
    /// back, and closing that one closes the window.
    #[test]
    fn edit_automation_in_a_window() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut d = Canvas::new(1920, 1080, 1.0).unwrap();
        let mut app = App::new(None);
        app.state.project = test_project();
        let item = |app: &App, k: KnobId| {
            let auto = app.automation_target(&Action::Knob(k));
            crate::rack::knob_menu(&app.state, k, auto, 0.0, 0.0).items.into_iter().find(|i| i.label == "Edit automation in a window").expect("the entry")
        };
        let target = |app: &App, k: KnobId| app.automation_target(&Action::Knob(k)).expect("automated").0;
        let (vol, pan) = (target(&app, KnobId::ChanVol(0)), target(&app, KnobId::ChanPan(0)));

        let w = item(&app, KnobId::ChanVol(0));
        assert!(w.enabled);
        crate::playlist::run(&mut app, w.cmd);
        assert!(app.state.shown(Win::Events) && app.state.in_front(Win::Events) && app.state.events_back.is_none());
        assert_eq!(app.state.events.target, vol);
        let first = crate::views::events::rect(&app.state);
        // The editor is drawn (its paint area is the window's).
        app.paint(&mut c, None);
        app.state.z.retain(|w| *w != Win::Events);
        app.paint(&mut d, None);
        app.state.z.push(Win::Events);
        let px = |c: &Canvas, x: f32, y: f32| c.pixmap.pixel(x as u32, y as u32).map(|p| (p.red(), p.green(), p.blue()));
        assert_ne!(px(&c, first.cx(), first.y + 4.0), px(&d, first.cx(), first.y + 4.0), "the editor is drawn");

        set(&mut app.state, 24, 0.75);
        app.state.events.sel = Some((0, 48));
        app.events_cmd(EvCmd::Copy);
        assert!(app.state.events.clipboard.is_some());
        let cmd = item(&app, KnobId::ChanPan(0)).cmd;
        crate::playlist::run(&mut app, cmd);
        assert_eq!(app.state.events.target, pan);
        assert_eq!(app.state.events_back.as_ref().map(|b| b.target), Some(vol));
        let second = crate::views::events::rect(&app.state);
        assert_eq!((second.x - first.x, second.y - first.y), (30.0, 30.0));
        assert!(app.state.events.clipboard.is_some(), "the clipboard is in front");

        // Both are drawn: the first's title, left of the second, is the first's title.
        app.paint(&mut c, None);
        let back = app.state.events_back.take();
        app.paint(&mut d, None);
        app.state.events_back = back;
        let (x, y) = (first.x + 12.0, first.y + 14.0);
        assert!(!second.contains(x, y));
        assert_ne!(px(&c, x, y), px(&d, x, y), "the editor behind is drawn");

        // A click on the one behind brings it in front.
        app.paint(&mut c, None);
        app.mouse_move(first.x + 200.0, first.y + 10.0);
        app.mouse_down();
        app.mouse_up();
        assert_eq!(app.state.events.target, vol);
        assert_eq!(app.state.events_back.as_ref().map(|b| b.target), Some(pan));
        assert_eq!(crate::views::events::rect(&app.state), first);
        assert!(app.state.events.clipboard.is_some() && app.state.events_back.as_ref().unwrap().clipboard.is_none());

        app.act(Action::CloseWin(Win::Events));
        assert!(app.state.shown(Win::Events) && app.state.events_back.is_none());
        assert_eq!(app.state.events.target, pan);
        assert!(app.state.events.clipboard.is_some());
        app.act(Action::CloseWin(Win::Events));
        assert!(!app.state.shown(Win::Events));
        // Opened again later: one window.
        let cmd = item(&app, KnobId::ChanVol(0)).cmd;
        crate::playlist::run(&mut app, cmd);
        assert!(app.state.shown(Win::Events) && app.state.events_back.is_none());
    }
}
