// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The plugin editor: a child of the host's window (Win32, X11 on Linux, an NSView on macOS) that paints `view` with
//! the os-ui canvas, and the editor logic (platform independent). The editor keeps the state it shows; every edit
//! writes it back as the plugin's saved state and hands it to the audio thread (`PulpParams::edited`). A state
//! replaced from outside (a preset, the host) is read again on the next tick.
//!
// spec: plugins/lemon-pulp-filter.md §2 .. §4, plugins/host-link.md

use std::any::Any;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::Arc;

use nih_plug::prelude::*;
use os_dsp::curve::{Curve, MAP, SYNCED};
use os_engine::effects::love_philter::state::*;
use os_plugkit::MenuDef;
use os_ui::kit::menu::{Mark, MenuItem, MenuStyle};

use crate::ctl;
use crate::edit::{self, POINT_MODES};
use crate::view::{self, Shown, Sw, Target, ViewState};
use crate::PulpParams;

pub struct PulpEditor {
    pub params: Arc<PulpParams>,
    /// DPI scale as f32 bits.
    pub scale: AtomicU32,
    /// The open editor window (0 = closed).
    #[cfg_attr(not(windows), allow(dead_code))]
    hwnd: Arc<AtomicIsize>,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    slot: os_plugkit::gui::Slot,
}

impl PulpEditor {
    pub fn new(params: Arc<PulpParams>) -> Self {
        PulpEditor {
            params,
            scale: AtomicU32::new(1f32.to_bits()),
            hwnd: Arc::new(AtomicIsize::new(0)),
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            slot: Default::default(),
        }
    }
}

impl Editor for PulpEditor {
    fn spawn(&self, parent: ParentWindowHandle, context: Arc<dyn GuiContext>) -> Box<dyn Any + Send> {
        let scale = f32::from_bits(self.scale.load(Ordering::Relaxed));
        let params = self.params.clone();
        match parent {
            #[cfg(windows)]
            ParentWindowHandle::Win32Hwnd(hwnd) => Box::new(win32::open(hwnd, scale, params, context, self.hwnd.clone())),
            #[cfg(target_os = "linux")]
            ParentWindowHandle::X11Window(window) => Box::new(gui::open(window, scale, params, context, &self.slot)),
            #[cfg(target_os = "macos")]
            ParentWindowHandle::AppKitNsView(v) => {
                let scale = os_plugkit::gui::backing_scale(v as usize);
                Box::new(gui::open(v as usize, scale, params, context, &self.slot))
            }
            _ => {
                let _ = (context, scale, params);
                Box::new(())
            }
        }
    }

    fn size(&self) -> (u32, u32) {
        (view::W, view::H)
    }

    fn set_scale_factor(&self, factor: f32) -> bool {
        self.scale.store(factor.to_bits(), Ordering::Relaxed);
        true
    }

    // The state's settings are not parameters.
    fn param_value_changed(&self, _id: &str, _normalized_value: f32) {}
    fn param_modulation_changed(&self, _id: &str, _modulation_offset: f32) {}
    fn param_values_changed(&self) {}
}

// ---------------------------------------------------------------------------------------------------------------
// Laws (the same as Citrus's: plugins/lemon-pulp-filter.md §4)

/// Pixels of drag for a knob's or slider's whole range.
pub const KNOB_PX: f32 = 250.0;
/// Ctrl: this much finer.
const FINE: f32 = 1.0 / 20.0;
/// The wheel moves this part of the range per notch (at least one step).
const WHEEL_PART: f32 = 1.0 / 32.0;
/// Pixels of drag per step of a box.
pub const BOX_PX: f32 = 8.0;
/// Pixels of drag per unit of a segment's tension (-1 .. 1).
pub const TENSION_PX: f32 = 100.0;
/// The snap tool's grid: the view's span in 16 steps, levels in 16.
const SNAP_T: f64 = 16.0;
const SNAP_L: f32 = 16.0;

// ---------------------------------------------------------------------------------------------------------------
// Menus

#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    None,
    Reset(usize),
    CopyValue(usize),
    PasteValue(usize),
    TypeIn(usize),
    Kind(i32),
    Oversampling(i32),
    /// The Options menu: sync pattern changes, note input, HQ waveshaper.
    Option(usize),
    ResetState,
    Curve(CurveCmd),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CurveCmd {
    Delete(usize),
    Mode(usize, u8),
    /// The loop start (false) or the loop end (true) at the point, or off.
    Marker(usize, bool),
    CopyLevel(usize),
    PasteLevel(usize),
    TypeIn(usize),
    CopyState,
    PasteState,
    AllLoop,
    Flip,
    Reset,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuReq {
    Param(usize),
    Type,
    Oversampling,
    Options,
    Point(usize),
    Curve,
}

// ---------------------------------------------------------------------------------------------------------------
// Editor logic

/// Something the window has to do after an event.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Repaint,
    /// Hide the cursor and hold it in place (knob, slider and box drags).
    Lock,
    /// Keep the mouse while the button is down (the X / Y pad, points).
    Capture,
    Menu(MenuReq, f32, f32),
    TypeIn(TypeTarget, f32, f32),
}

/// What a type-in box sets: a setting, or a point's level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TypeTarget {
    Param(usize),
    Point(usize),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Btn {
    Left,
    Right,
    Middle,
}

#[derive(Clone, Copy, Debug)]
enum Drag {
    /// A knob or slider: the unrounded value it has reached.
    Value(usize, f32),
    /// The pattern box, in steps.
    Steps(usize, f32),
    /// A point of the curve view, with the view's time span when the drag began.
    Point(usize, f64),
    /// A segment's tension handle: which way up bends it (1 or -1).
    Tension(usize, f32),
    Pad,
}

pub struct Logic {
    pub params: Arc<PulpParams>,
    /// The state shown and edited.
    pub st: State,
    /// The state version `st` was read at.
    version: u64,
    pub view: ViewState,
    pub hits: Vec<view::Hit>,
    drag: Option<Drag>,
    /// Hint text (a tab before the value).
    pub hint: String,
    copied: Option<i32>,
    level_clip: Option<f32>,
    curve_clip: Option<Curve>,
    pub hosted: bool,
}

impl Logic {
    pub fn new(params: Arc<PulpParams>) -> Self {
        let mut l = Logic {
            params,
            st: State::new_default(),
            version: 0,
            view: ViewState::default(),
            hits: Vec::new(),
            drag: None,
            hint: String::new(),
            copied: None,
            level_clip: None,
            curve_clip: None,
            hosted: false,
        };
        l.reload();
        l
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    fn reload(&mut self) {
        self.version = self.params.state_version.load(Ordering::Acquire);
        let bytes = self.params.state.read().map(|s| s.0.clone()).unwrap_or_default();
        self.st = crate::state_of(&bytes);
    }

    /// Timer: a state replaced from outside is read again; the pad's and maps' positions move. True when the editor
    /// has to repaint.
    pub fn poll(&mut self) -> bool {
        if self.params.state_version.load(Ordering::Acquire) != self.version {
            self.reload();
            return true;
        }
        // The map positions are drawn on the map shown.
        matches!(view::shown(&self.view), Shown::Part(_, 2..=4))
    }

    /// An edit: the edited state becomes the plugin's at once and the audio thread plays it.
    fn commit(&mut self) {
        if !self.params.edited(self.version, self.st.clone()) {
            self.reload();
        }
    }

    fn f(&self) -> usize {
        self.view.filter.min(FILTERS - 1)
    }

    fn set(&mut self, k: usize, v: i32) {
        let (lo, hi) = range(k);
        let v = v.clamp(lo, hi);
        if self.st.param(k) != Some(v) {
            self.st.set_param(k, v);
            self.commit();
        }
    }

    fn reset(&mut self, k: usize) {
        self.set(k, ctl::default(k));
    }

    fn switch(&self, s: Sw) -> bool {
        let fl = &self.st.filters[self.f()];
        match s {
            Sw::Alt => fl.alt,
            Sw::Ws => fl.ws,
            Sw::WsBipolar => fl.ws_bipolar != 0,
            Sw::Smooth => self.st.smooth(),
            Sw::Center => self.st.dc(),
            Sw::HqEnvelopes => self.st.hq_envelopes(),
            Sw::Use1 => self.st.follow_filter1,
            Sw::PartOn => self.curve().is_some_and(|c| c.enabled),
            Sw::Tempo => self.curve().is_some_and(|c| c.flags & SYNCED != 0),
        }
    }

    fn set_switch(&mut self, s: Sw, on: bool) {
        let f = self.f();
        let fl = &mut self.st.filters[f];
        match s {
            Sw::Alt => fl.alt = on,
            Sw::Ws => fl.ws = on,
            Sw::WsBipolar => fl.ws_bipolar = u8::from(on),
            Sw::Smooth => self.st.switches[SW_SMOOTH] = u8::from(on),
            Sw::Center => self.st.switches[SW_DC] = u8::from(on),
            Sw::HqEnvelopes => self.st.set_switch_int(SW_HQ_ENV, i32::from(on)),
            Sw::Use1 => self.st.follow_filter1 = on,
            Sw::PartOn | Sw::Tempo => {
                if self.curve().is_none() {
                    return;
                }
                self.edit_curve(|c| {
                    if s == Sw::PartOn {
                        c.enabled = on;
                    } else if on {
                        c.flags |= SYNCED;
                    } else {
                        c.flags &= !SYNCED;
                    }
                });
                return;
            }
        }
        self.commit();
    }

    /// The curve the curve view shows.
    fn curve(&self) -> Option<&Curve> {
        let f = self.f();
        Some(match view::shown(&self.view) {
            Shown::Part(t, p) => view::part(&self.st, f, t, p),
            Shown::Ws => &self.st.filters[f].ws_curve,
        })
    }

    fn curve_mut(&mut self) -> &mut Curve {
        let f = self.f();
        let pat = view::pattern_index(&self.st);
        match view::shown(&self.view) {
            Shown::Part(t, p) => {
                let a = &mut self.st.filters[f].arts[t];
                match p {
                    0 => &mut a.patterns[pat],
                    1 => &mut a.lfo,
                    p => &mut a.maps[(p - 2).min(2)],
                }
            }
            Shown::Ws => &mut self.st.filters[f].ws_curve,
        }
    }

    /// The rect the curve view is drawn in.
    fn curve_rect(&self) -> os_ui::Rect {
        if view::shown(&self.view) == Shown::Ws { view::CURVE_WS } else { view::CURVE }
    }

    /// Edits the curve shown; a change is committed.
    fn edit_curve(&mut self, f: impl FnOnce(&mut Curve)) {
        let c = self.curve_mut();
        let before = c.clone();
        f(c);
        if *c != before {
            self.commit();
        }
    }

    fn snapped(&self, t: f64, l: f32, span: f64) -> (f64, f32) {
        if self.view.tools[2] { ((t / span * SNAP_T).round() * span / SNAP_T, (l * SNAP_L).round() / SNAP_L) } else { (t, l) }
    }

    /// A level as the target shows it: a pan as left / right, an additive target signed, others as percent.
    fn level_text(&self, l: f32) -> String {
        match view::shown(&self.view) {
            Shown::Part(T_PAN, _) => {
                let v = ((l - 0.5) * 200.0).round() as i32;
                match v {
                    0 => "Centered".into(),
                    v if v < 0 => format!("{}% left", -v),
                    v => format!("{v}% right"),
                }
            }
            Shown::Part(t, _) if additive(t) => format!("{}%", ((2.0 * l - 1.0) * 100.0).round() as i32),
            Shown::Ws if self.st.filters[self.f()].ws_bipolar != 0 => format!("{}%", ((2.0 * l - 1.0) * 100.0).round() as i32),
            _ => format!("{}%", (l * 100.0).round() as i32),
        }
    }

    /// `Point 2/5`, the time (a pattern's in bar:step:tick when synced, else s:ms; a map's position as percent) and the
    /// level.
    fn point_hint(&self, i: usize) -> String {
        let Some(c) = self.curve() else { return String::new() };
        let Some(p) = c.points.get(i) else { return String::new() };
        let t = edit::times(c)[i];
        let time = if c.kind == MAP {
            format!("{}%", (t * 100.0).round())
        } else if c.flags & SYNCED != 0 {
            let ticks = (t * 96.0).round() as i64;
            format!("{:03}:{:02}:{:03}", ticks / 384 + 1, ticks / 96 % 4 + 1, ticks % 96)
        } else {
            let ms = (t as f32 * 500.0).floor() as i64;
            format!("{}:{:03}", ms / 1000, ms % 1000)
        };
        format!("Point {}/{} at {time}\t{}", i + 1, c.points.len(), self.level_text(p.level))
    }

    fn hit(&self, x: f32, y: f32) -> Option<Target> {
        self.hits.iter().rev().find(|h| h.r.contains(x, y)).map(|h| h.t.clone())
    }

    fn param(t: &Target) -> Option<usize> {
        match t {
            Target::Knob(k) | Target::Slider(k) => Some(*k),
            Target::Pattern => Some(PATTERN),
            _ => None,
        }
    }

    /// The hint of a target: FL's measured hints (`Name: value`), the Lemon Studio choices where FL's were not read
    /// (plugins/lemon-pulp-filter.md §3).
    fn hint_for(&self, t: Option<&Target>) -> String {
        let f = self.f();
        match t {
            None => String::new(),
            Some(Target::Enable(_)) => "Enable unit (Right-click to solo)".into(),
            Some(Target::FilterTab(_)) => "Filter unit".into(),
            Some(Target::Switch(s)) => match s {
                Sw::Alt => "Alternate resonance / bandwidth".into(),
                Sw::Ws => "Enable waveshaper".into(),
                Sw::WsBipolar => format!("Waveshaper mode: {}", if self.switch(*s) { "bipolar" } else { "unipolar" }),
                Sw::Smooth => "Smooth up modulation".into(),
                Sw::Center => "Global DC offset removal".into(),
                Sw::HqEnvelopes => "Extra high envelope accuracy".into(),
                Sw::Use1 => "Use filter unit 1 as input for envelope follower".into(),
                Sw::PartOn => "Enable envelope".into(),
                Sw::Tempo => "Tempo-based time".into(),
            },
            Some(Target::Order(o)) => {
                let m = ["off", "single", "dual", "triple", "alternate dual", "alternate triple"][(*o).clamp(0, 5) as usize];
                format!("Filter mode: {m}")
            }
            Some(Target::Type) => format!("Filter type: {}", view::TYPE_NAMES[self.st.filters[f].kind.clamp(0, TYPES - 1) as usize]),
            Some(Target::TargetTab(i)) => {
                let n = [
                    "panning",
                    "volume",
                    "filter cutoff",
                    "filter resonance / bandwidth",
                    "filter low / gain / drive",
                    "filter band",
                    "filter high",
                    "waveshaper curve",
                    "waveshaper mix",
                ][*i];
                format!("Editor target: {n}")
            }
            Some(Target::PartTab(i)) => {
                let n = ["pattern", "LFO", "modulation X mapping", "modulation Y mapping", "input envelope follower mapping"][*i];
                format!("Articulator part: {n}")
            }
            Some(Target::Pad) => format!("X / Y modulation: {} / {}", ctl::text(&self.st, 6, self.st.mod_x()), ctl::text(&self.st, 7, self.st.mod_y())),
            Some(Target::Point(i)) => self.point_hint(*i),
            Some(Target::Tension(i)) => {
                let t = self.curve().and_then(|c| c.points.get(*i)).map_or(0.0, |p| p.tension);
                format!("Point {} to {} tension	{}%", i, i + 1, (t * 100.0).round())
            }
            Some(Target::CurveArea) => String::new(),
            Some(Target::Tool(k)) => view::TOOL_NAMES[*k].into(),
            Some(Target::CurveOptions) | Some(Target::Header(1)) => "Options".into(),
            Some(Target::Header(_)) | Some(Target::Logo) => "Show info".into(),
            Some(Target::Oversampling) => format!("Oversampling factor: {}", view::OVERSAMPLING_LABELS[self.st.oversampling() as usize]),
            Some(Target::Desc) => "Info / comments".into(),
            Some(Target::Inert(s)) => (*s).into(),
            Some(Target::Cover) => String::new(),
            Some(t) => Self::param(t).map(|k| format!("{}: {}", ctl::name(&self.st, k), ctl::text(&self.st, k, ctl::shown(&self.st, k)))).unwrap_or_default(),
        }
    }

    fn pad_to(&mut self, x: f32, y: f32) {
        let (vx, vy) = view::pad_values(x, y);
        if (self.st.mod_x(), self.st.mod_y()) != (vx, vy) {
            self.st.set_switch_int(SW_MODX, vx);
            self.st.set_switch_int(SW_MODY, vy);
            self.commit();
        }
    }

    /// Turns filter `f` on or off; a filter turned on with order 0 plays a single stage.
    fn toggle_filter(&mut self, f: usize) {
        let fl = &mut self.st.filters[f];
        fl.enabled = !fl.enabled;
        self.commit();
    }

    /// Solos filter `f`, or ends its solo.
    fn solo(&mut self, f: usize) {
        let v = if self.st.solo() == Some(f) { -1 } else { f as i32 };
        self.st.set_switch_int(SW_SOLO, v);
        self.commit();
    }

    pub fn down(&mut self, x: f32, y: f32, btn: Btn, m: Mods, _dbl: bool) -> Vec<Act> {
        self.view.mouse = Some((x, y));
        let Some(t) = self.hit(x, y) else { return vec![] };
        let mut acts = vec![Act::Repaint];
        let resettable = Self::param(&t);
        match (&t, btn) {
            (Target::Cover, _) => self.view.info = false,
            (Target::Enable(f), Btn::Left) => self.toggle_filter(*f),
            (Target::Enable(f), Btn::Right) => self.solo(*f),
            (Target::FilterTab(f), Btn::Left | Btn::Right) => self.view.filter = *f,
            (Target::TargetTab(i), Btn::Left | Btn::Right) => self.view.target = *i,
            (Target::PartTab(i), Btn::Left | Btn::Right) => self.view.part = *i,
            (Target::Switch(s), Btn::Left) => {
                let on = self.switch(*s);
                self.set_switch(*s, !on);
            }
            (Target::Order(o), Btn::Left) => {
                let f = self.f();
                if self.st.filters[f].order != *o {
                    self.st.filters[f].order = *o;
                    self.commit();
                }
            }
            (Target::Type, Btn::Left | Btn::Right) => acts.push(Act::Menu(MenuReq::Type, view::TYPE_BOX.x, view::TYPE_BOX.bottom())),
            (Target::Oversampling, Btn::Left | Btn::Right) => acts.push(Act::Menu(MenuReq::Oversampling, view::OVERSAMPLING.x, view::OVERSAMPLING.bottom())),
            (Target::Header(1), Btn::Left) => acts.push(Act::Menu(MenuReq::Options, x, y)),
            (Target::Header(2), Btn::Left) | (Target::Logo, Btn::Left) => self.view.info = true,
            (Target::CurveOptions, Btn::Left) => acts.push(Act::Menu(MenuReq::Curve, x, y)),
            (Target::Tool(1), _) => {}
            (Target::Tool(k), Btn::Left) => self.view.tools[*k] = !self.view.tools[*k],
            (Target::Point(i), Btn::Left) => {
                if let Some(c) = self.curve() {
                    let span = view::curve_span(c);
                    self.drag = Some(Drag::Point(*i, span));
                    self.view.span = Some(span);
                    self.view.pressed = Some(t.clone());
                    acts.push(Act::Capture);
                }
            }
            (Target::Point(i), Btn::Right) => acts.push(Act::Menu(MenuReq::Point(*i), x, y)),
            (Target::Tension(i), Btn::Left) => {
                if let Some(c) = self.curve() {
                    // Up raises the segment's middle, whichever way a tension bends it.
                    let mut c = c.clone();
                    let mut mid = |t: f32| {
                        edit::set_tension(&mut c, *i, t);
                        edit::segment_level(&c, *i, 0.5)
                    };
                    let sign = if mid(0.5) >= mid(-0.5) { 1.0 } else { -1.0 };
                    self.drag = Some(Drag::Tension(*i, sign));
                    self.view.pressed = Some(t.clone());
                    acts.push(Act::Lock);
                }
            }
            (Target::Tension(i), Btn::Middle) => self.edit_curve(|c| edit::set_tension(c, *i, 0.0)),
            // A right click on empty space adds a point there.
            (Target::CurveArea, Btn::Right) if !self.view.tools[0] => {
                if let Some(c) = self.curve() {
                    let r = self.curve_rect();
                    let span = view::curve_span(c);
                    let (t, l) = self.snapped(view::x_to_t(r, x, span).min(span), view::y_to_level(r, y), span);
                    self.edit_curve(|c| {
                        edit::insert_point(c, if c.kind == MAP { t.min(1.0 - 1e-3) } else { t }, l);
                    });
                }
            }
            (Target::Pad, Btn::Left) => {
                self.drag = Some(Drag::Pad);
                self.view.pressed = Some(t.clone());
                self.pad_to(x, y);
                acts.push(Act::Capture);
            }
            (_, Btn::Left) if m.alt && resettable.is_some() => self.reset(resettable.unwrap()),
            (_, Btn::Middle) if resettable.is_some() => self.reset(resettable.unwrap()),
            (Target::Knob(k) | Target::Slider(k), Btn::Left) => {
                self.drag = Some(Drag::Value(*k, ctl::shown(&self.st, *k) as f32));
                self.view.pressed = Some(t.clone());
                acts.push(Act::Lock);
            }
            (Target::Pattern, Btn::Left) => {
                self.drag = Some(Drag::Steps(PATTERN, ctl::shown(&self.st, PATTERN) as f32));
                self.view.pressed = Some(t.clone());
                acts.push(Act::Lock);
            }
            (Target::Knob(k) | Target::Slider(k), Btn::Right) => acts.push(Act::Menu(MenuReq::Param(*k), x, y)),
            (Target::Pattern, Btn::Right) => acts.push(Act::Menu(MenuReq::Param(PATTERN), x, y)),
            _ => {}
        }
        self.hint = self.hint_for(Some(&t));
        acts
    }

    /// Mouse movement during a locked drag (logical px since the last event).
    pub fn delta(&mut self, _dx: f32, dy: f32, m: Mods) -> Vec<Act> {
        match self.drag {
            Some(Drag::Value(k, v)) => {
                let (lo, hi) = range(k);
                let f = if m.ctrl { FINE } else { 1.0 };
                let v = (v - dy * f * (hi - lo) as f32 / KNOB_PX).clamp(lo as f32, hi as f32);
                self.drag = Some(Drag::Value(k, v));
                self.set(k, v.round() as i32);
            }
            Some(Drag::Steps(k, v)) => {
                let (lo, hi) = range(k);
                let v = (v - dy / BOX_PX).clamp(lo as f32, hi as f32);
                self.drag = Some(Drag::Steps(k, v));
                self.set(k, v.round() as i32);
            }
            Some(Drag::Tension(i, sign)) => {
                let f = if m.ctrl { FINE } else { 1.0 };
                let t = self.curve().and_then(|c| c.points.get(i)).map_or(0.0, |p| p.tension) - sign * dy * f / TENSION_PX;
                self.edit_curve(|c| edit::set_tension(c, i, t));
            }
            _ => return vec![],
        }
        let t = self.view.pressed.clone();
        self.hint = self.hint_for(t.as_ref());
        vec![Act::Repaint]
    }

    pub fn up(&mut self, _btn: Btn) -> Vec<Act> {
        if self.drag.take().is_none() {
            return vec![];
        }
        self.view.pressed = None;
        self.view.span = None;
        let t = self.view.mouse.and_then(|(x, y)| self.hit(x, y));
        self.hint = self.hint_for(t.as_ref());
        vec![Act::Repaint]
    }

    pub fn moved(&mut self, x: f32, y: f32, _m: Mods) -> Vec<Act> {
        self.view.mouse = Some((x, y));
        match self.drag {
            Some(Drag::Pad) => {
                self.pad_to(x, y);
                self.hint = self.hint_for(Some(&Target::Pad));
            }
            Some(Drag::Point(i, span)) => {
                let r = self.curve_rect();
                let (t, l) = self.snapped(view::x_to_t(r, x, span), view::y_to_level(r, y), span);
                let slide = self.view.tools[3];
                self.edit_curve(|c| edit::move_point(c, i, t, l, slide));
                self.hint = self.point_hint(i);
            }
            Some(_) => {}
            None => {
                let t = self.hit(x, y);
                self.hint = self.hint_for(t.as_ref());
            }
        }
        vec![Act::Repaint]
    }

    pub fn leave(&mut self) -> Vec<Act> {
        self.view.mouse = None;
        self.hint.clear();
        vec![Act::Repaint]
    }

    /// Wheel: 1/32 of the range per notch (at least one step); the pattern box and the type one step; a switch flips.
    pub fn wheel(&mut self, x: f32, y: f32, notches: f32, _m: Mods) -> Vec<Act> {
        let Some(t) = self.hit(x, y) else { return vec![] };
        let d = notches.signum() as i32;
        match &t {
            Target::Knob(k) | Target::Slider(k) => {
                let (lo, hi) = range(*k);
                let step = (((hi - lo) as f32 * WHEEL_PART).round() as i32).max(1);
                self.set(*k, ctl::shown(&self.st, *k) + d * step);
            }
            Target::Pattern => self.set(PATTERN, ctl::shown(&self.st, PATTERN) + d),
            Target::Type => {
                let f = self.f();
                let k = (self.st.filters[f].kind - d).clamp(0, TYPES - 1);
                self.set_kind(k);
            }
            Target::Switch(s) => self.set_switch(*s, d > 0),
            _ => return vec![],
        }
        self.hint = self.hint_for(Some(&t));
        vec![Act::Repaint]
    }

    fn set_kind(&mut self, k: i32) {
        let f = self.f();
        if self.st.filters[f].kind != k {
            self.st.filters[f].kind = k;
            self.commit();
        }
    }

    pub fn menu(&self, req: &MenuReq) -> MenuDef<Cmd> {
        match req {
            MenuReq::Param(k) => {
                let mut m = MenuDef::new(MenuStyle::Context);
                m.head(&ctl::name(&self.st, *k), Cmd::None);
                m.add(MenuItem::item("Reset"), Cmd::Reset(*k));
                m.sep(Cmd::None);
                m.add(MenuItem::item("Copy value"), Cmd::CopyValue(*k));
                m.add(MenuItem::item("Paste value").enabled(self.copied.is_some()), Cmd::PasteValue(*k));
                m.add(MenuItem::item("Type in value..."), Cmd::TypeIn(*k));
                m
            }
            MenuReq::Type => {
                let kind = self.st.filters[self.f()].kind;
                let mut m = MenuDef::new(MenuStyle::List);
                for (k, name) in view::TYPE_NAMES.into_iter().enumerate() {
                    m.add(MenuItem::item(name).mark(Mark::Radio(kind == k as i32)), Cmd::Kind(k as i32));
                }
                m
            }
            MenuReq::Oversampling => {
                let o = self.st.oversampling() as i32;
                let mut m = MenuDef::new(MenuStyle::List);
                for (k, l) in view::OVERSAMPLING_LABELS.into_iter().enumerate() {
                    m.add(MenuItem::item(l).mark(Mark::Radio(o == k as i32)), Cmd::Oversampling(k as i32));
                }
                m
            }
            MenuReq::Options => {
                let mut m = MenuDef::new(MenuStyle::Context);
                for (n, l, on) in [
                    (0, "Sync pattern changes", self.st.switches[SW_SYNC] != 0),
                    (1, "Note input", self.st.switches[SW_MIDI] != 0),
                    (2, "HQ WaveShaper", self.st.hq_ws),
                ] {
                    m.add(MenuItem::item(l).mark(Mark::Check(on)), Cmd::Option(n));
                }
                m.sep(Cmd::None);
                m.add(MenuItem::item("Reset all settings"), Cmd::ResetState);
                m
            }
            MenuReq::Point(i) => self.point_menu(*i),
            MenuReq::Curve => self.curve_menu(),
        }
    }

    fn point_menu(&self, i: usize) -> MenuDef<Cmd> {
        let mut m = MenuDef::new(MenuStyle::Context);
        let Some(c) = self.curve() else { return m };
        let p = c.points.get(i).copied().unwrap_or_default();
        let cmd = Cmd::Curve;
        m.head(&format!("Point {}/{}", i + 1, c.points.len()), Cmd::None);
        let fixed = i == 0 || (c.kind == MAP && i + 1 == c.points.len());
        m.add(MenuItem::item("Delete").enabled(!fixed), cmd(CurveCmd::Delete(i)));
        if i > 0 {
            m.head("Mode", Cmd::None);
            for (l, v) in POINT_MODES {
                m.add(MenuItem::item(l).mark(Mark::Radio(p.mode == v)), cmd(CurveCmd::Mode(i, v)));
            }
        }
        if c.kind != MAP {
            m.head("Loop", Cmd::None);
            m.add(MenuItem::item("Loop start").mark(Mark::Check(c.loop_start == i as i32)), cmd(CurveCmd::Marker(i, false)));
            m.add(MenuItem::item("Loop end").mark(Mark::Check(c.sustain == i as i32)), cmd(CurveCmd::Marker(i, true)));
        }
        m.sep(Cmd::None);
        m.add(MenuItem::item("Copy value"), cmd(CurveCmd::CopyLevel(i)));
        m.add(MenuItem::item("Paste value").enabled(self.level_clip.is_some()), cmd(CurveCmd::PasteLevel(i)));
        m.add(MenuItem::item("Type in value..."), cmd(CurveCmd::TypeIn(i)));
        m
    }

    fn curve_menu(&self) -> MenuDef<Cmd> {
        let mut m = MenuDef::new(MenuStyle::Context);
        let Some(c) = self.curve() else { return m };
        let cmd = Cmd::Curve;
        m.head("State", Cmd::None);
        m.add(MenuItem::item("Copy state"), cmd(CurveCmd::CopyState));
        m.add(MenuItem::item("Paste state").enabled(self.curve_clip.as_ref().is_some_and(|s| s.kind == c.kind)), cmd(CurveCmd::PasteState));
        m.add(MenuItem::item("Reset"), cmd(CurveCmd::Reset));
        m.head("Tools", Cmd::None);
        m.add(MenuItem::item("Make all loop").enabled(c.kind != MAP), cmd(CurveCmd::AllLoop));
        m.add(MenuItem::item("Flip vertically"), cmd(CurveCmd::Flip));
        m
    }

    /// The level a curve resets to: a map's and a pattern's the target's neutral level, the waveshaper's a line.
    fn reset_curve(&mut self) {
        match view::shown(&self.view) {
            Shown::Ws => self.edit_curve(|c| {
                edit::reset(c, 0.0);
                c.points[1].level = 1.0;
                c.keep_love_philter();
            }),
            Shown::Part(t, p) => {
                let n = if p == 1 { 1.0 } else if additive(t) { 0.5 } else { 1.0 };
                self.edit_curve(|c| edit::reset(c, n));
            }
        }
    }

    fn curve_cmd(&mut self, k: CurveCmd) {
        let Some(c) = self.curve() else { return };
        let n = c.points.len();
        match k {
            CurveCmd::Delete(i) | CurveCmd::Mode(i, _) | CurveCmd::Marker(i, _) | CurveCmd::CopyLevel(i) | CurveCmd::PasteLevel(i) if i >= n => {}
            CurveCmd::CopyLevel(i) => self.level_clip = Some(c.points[i].level),
            CurveCmd::CopyState => self.curve_clip = Some(c.clone()),
            CurveCmd::TypeIn(_) => {}
            CurveCmd::Reset => self.reset_curve(),
            k => {
                let (level, clip) = (self.level_clip, self.curve_clip.clone());
                self.edit_curve(|c| match k {
                    CurveCmd::Delete(i) => {
                        edit::delete_point(c, i);
                    }
                    CurveCmd::Mode(i, v) => edit::set_mode(c, i, v),
                    CurveCmd::Marker(i, end) => edit::toggle_marker(c, i, end),
                    CurveCmd::PasteLevel(i) => {
                        if let Some(l) = level {
                            let t = edit::times(c)[i];
                            edit::move_point(c, i, t, l, false);
                        }
                    }
                    CurveCmd::PasteState => {
                        if let Some(s) = clip.filter(|s| s.kind == c.kind) {
                            *c = s;
                        }
                    }
                    CurveCmd::AllLoop => edit::make_all_loop(c),
                    CurveCmd::Flip => edit::flip(c),
                    _ => {}
                });
            }
        }
    }

    pub fn pick(&mut self, _req: &MenuReq, cmd: Cmd, x: f32, y: f32) -> Vec<Act> {
        match cmd {
            Cmd::None => {}
            Cmd::Reset(k) => self.reset(k),
            Cmd::CopyValue(k) => self.copied = self.st.param(k),
            Cmd::PasteValue(k) => {
                if let Some(v) = self.copied {
                    self.set(k, v);
                }
            }
            Cmd::TypeIn(k) => return vec![Act::TypeIn(TypeTarget::Param(k), x, y)],
            Cmd::Kind(k) => self.set_kind(k),
            Cmd::Oversampling(o) => {
                if self.st.switch_int(SW_OVERSAMPLING) != o {
                    self.st.set_switch_int(SW_OVERSAMPLING, o);
                    self.commit();
                }
            }
            Cmd::Option(n) => {
                match n {
                    0 => self.st.switches[SW_SYNC] ^= 1,
                    1 => self.st.switches[SW_MIDI] ^= 1,
                    _ => self.st.hq_ws = !self.st.hq_ws,
                }
                self.commit();
            }
            Cmd::ResetState => {
                self.st = State::new_default();
                self.commit();
            }
            Cmd::Curve(CurveCmd::TypeIn(i)) => return vec![Act::TypeIn(TypeTarget::Point(i), x, y)],
            Cmd::Curve(k) => self.curve_cmd(k),
        }
        vec![Act::Repaint]
    }

    pub fn value_text(&self, t: &TypeTarget) -> String {
        match *t {
            TypeTarget::Param(k) => ctl::text(&self.st, k, ctl::shown(&self.st, k)),
            TypeTarget::Point(i) => self.curve().and_then(|c| c.points.get(i)).map(|p| format!("{}", p.level)).unwrap_or_default(),
        }
    }

    /// Applies a typed value; text that does not read leaves it as it was. A point's level is typed 0 .. 1.
    pub fn typed(&mut self, t: &TypeTarget, text: &str) {
        match *t {
            TypeTarget::Param(k) => {
                if let Some(v) = ctl::parse(&self.st, k, text) {
                    self.set(k, v);
                }
            }
            TypeTarget::Point(i) => {
                let n = self.curve().map_or(0, |c| c.points.len());
                if let Some(l) = text.trim().parse::<f32>().ok().filter(|l| l.is_finite() && i < n) {
                    self.edit_curve(|c| {
                        let t = edit::times(c)[i];
                        edit::move_point(c, i, t, l, false);
                    });
                }
            }
        }
    }

    /// Paints the editor; `hint` is drawn when the host does not show it.
    pub fn paint(&mut self, c: &mut os_ui::Canvas) {
        let hint = if self.hosted { String::new() } else { self.hint.clone() };
        let positions = self.params.positions.each_ref().map(|a| a.load(Ordering::Relaxed));
        let model = view::Model { st: &self.st, positions, version: env!("CARGO_PKG_VERSION") };
        self.hits = view::paint(c, &model, &self.view, &hint);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The window

#[cfg(windows)]
mod win32 {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicIsize, Ordering};
    use std::sync::Arc;

    use nih_plug::prelude::GuiContext;
    use os_plugkit::win::{self as pw, CursorLock};
    use os_theme::Palette;
    use os_ui::Canvas;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Controls::WM_MOUSELEAVE;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    use super::{Act, Btn, Logic, Mods};
    use crate::view;
    use crate::PulpParams;

    const TIMER: usize = 1;

    struct Win {
        hwnd: HWND,
        canvas: Canvas,
        buf: Vec<u32>,
        scale: f32,
        /// Painted size in device px.
        size: (u32, u32),
        logic: Logic,
        palette: Palette,
        last_hint: String,
        lock: CursorLock,
        tracking: bool,
        slot: Arc<AtomicIsize>,
    }

    /// Owns the window; dropping it closes the editor.
    pub struct Handle(isize, Arc<AtomicIsize>);

    // SAFETY: the handle is only an HWND value; the wrapper drops it on the GUI thread.
    unsafe impl Send for Handle {}

    impl Drop for Handle {
        fn drop(&mut self) {
            self.1.store(0, Ordering::Release);
            unsafe { DestroyWindow(self.0 as HWND) };
        }
    }

    fn mods() -> Mods {
        let (ctrl, shift, alt) = pw::mods();
        Mods { ctrl, shift, alt }
    }

    pub fn open(parent: *mut c_void, scale: f32, params: Arc<PulpParams>, _ctx: Arc<dyn GuiContext>, slot: Arc<AtomicIsize>) -> Handle {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let logic = Logic::new(params);
        let px = |v: u32| (v as f32 * scale).round() as u32;
        let size = (px(view::W), px(view::H));
        unsafe {
            let class = pw::register("PulpFilterView", Some(wndproc));
            let win = Box::new(Win {
                hwnd: std::ptr::null_mut(),
                canvas: Canvas::new(size.0, size.1, scale).expect("canvas"),
                buf: vec![0; (size.0 * size.1) as usize],
                scale,
                size,
                logic,
                palette: os_plugkit::palette(),
                last_hint: String::new(),
                lock: CursorLock::default(),
                tracking: false,
                slot: slot.clone(),
            });
            let title = pw::wide("Pulp Filter");
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
                0,
                0,
                size.0 as i32,
                size.1 as i32,
                parent as HWND,
                std::ptr::null_mut(),
                GetModuleHandleW(std::ptr::null()),
                Box::into_raw(win) as *const c_void,
            );
            slot.store(hwnd as isize, Ordering::Release);
            SetTimer(hwnd, TIMER, 30, None);
            Handle(hwnd as isize, slot)
        }
    }

    unsafe fn state(hwnd: HWND) -> Option<&'static mut Win> {
        let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Win;
        if p.is_null() { None } else { Some(unsafe { &mut *p }) }
    }

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        unsafe {
            if msg == WM_NCCREATE {
                let cs = lp as *const CREATESTRUCTW;
                let w = (*cs).lpCreateParams as *mut Win;
                (*w).hwnd = hwnd;
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, w as isize);
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            if msg == WM_NCDESTROY {
                let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Win;
                if !p.is_null() {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    (*p).slot.store(0, Ordering::Release);
                    drop(Box::from_raw(p));
                }
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            let Some(w) = state(hwnd) else { return DefWindowProcW(hwnd, msg, wp, lp) };
            if msg == os_hostlink::message() {
                return w.host_link(wp, lp);
            }
            match msg {
                WM_PAINT => {
                    w.paint();
                    0
                }
                WM_ERASEBKGND => 1,
                WM_TIMER => {
                    w.poll();
                    0
                }
                WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_RBUTTONDOWN | WM_RBUTTONDBLCLK | WM_MBUTTONDOWN => {
                    SetFocus(hwnd);
                    let (x, y) = w.mouse(lp);
                    let btn = match msg {
                        WM_RBUTTONDOWN | WM_RBUTTONDBLCLK => Btn::Right,
                        WM_MBUTTONDOWN => Btn::Middle,
                        _ => Btn::Left,
                    };
                    let acts = w.logic.down(x, y, btn, mods(), msg == WM_LBUTTONDBLCLK);
                    w.run(acts);
                    0
                }
                WM_MOUSEMOVE => {
                    if !w.tracking {
                        let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                        TrackMouseEvent(&mut tme);
                        w.tracking = true;
                    }
                    if let Some((dx, dy)) = w.lock.delta() {
                        if dx != 0 || dy != 0 {
                            let acts = w.logic.delta(dx as f32 / w.scale, dy as f32 / w.scale, mods());
                            w.run(acts);
                        }
                        return 0;
                    }
                    let (x, y) = w.mouse(lp);
                    let acts = w.logic.moved(x, y, mods());
                    w.run(acts);
                    0
                }
                WM_MOUSELEAVE => {
                    w.tracking = false;
                    if !w.lock.locked() && !w.logic.dragging() {
                        let acts = w.logic.leave();
                        w.run(acts);
                    }
                    0
                }
                WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP => {
                    w.lock.unlock();
                    ReleaseCapture();
                    let acts = w.logic.up(Btn::Left);
                    w.run(acts);
                    0
                }
                WM_CAPTURECHANGED => {
                    if w.logic.dragging() && lp as HWND != hwnd {
                        w.lock.unlock();
                        let acts = w.logic.up(Btn::Left);
                        w.run(acts);
                    }
                    0
                }
                WM_MOUSEWHEEL => {
                    // Wheel positions are screen coordinates.
                    let mut r: RECT = std::mem::zeroed();
                    GetWindowRect(hwnd, &mut r);
                    let (sx, sy) = pw::xy(lp);
                    let (x, y) = ((sx - r.left) as f32 / w.scale, (sy - r.top) as f32 / w.scale);
                    let notches = ((wp >> 16) & 0xffff) as u16 as i16 as f32 / 120.0;
                    let acts = w.logic.wheel(x, y, notches, mods());
                    w.run(acts);
                    0
                }
                WM_DESTROY => {
                    KillTimer(hwnd, TIMER);
                    w.lock.unlock();
                    0
                }
                _ => DefWindowProcW(hwnd, msg, wp, lp),
            }
        }
    }

    impl Win {
        fn mouse(&self, l: LPARAM) -> (f32, f32) {
            let (x, y) = pw::xy(l);
            (x as f32 / self.scale, y as f32 / self.scale)
        }

        fn screen(&self, x: f32, y: f32) -> (i32, i32) {
            pw::to_screen(self.hwnd, (x * self.scale) as i32, (y * self.scale) as i32)
        }

        fn invalidate(&self) {
            unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) };
        }

        /// Timer: a state replaced from outside, the hint for the host.
        fn poll(&mut self) {
            if self.logic.poll() {
                self.invalidate();
            }
            if self.logic.hint != self.last_hint {
                self.last_hint = self.logic.hint.clone();
                if self.logic.hosted {
                    os_hostlink::to_host(self.hwnd as isize, os_hostlink::Cmd::Changed, 0);
                }
            }
        }

        fn run(&mut self, acts: Vec<Act>) {
            let mut repaint = false;
            for a in acts {
                match a {
                    Act::Repaint => repaint = true,
                    Act::Lock => self.lock.lock(self.hwnd),
                    Act::Capture => unsafe {
                        SetCapture(self.hwnd);
                    },
                    Act::Menu(req, x, y) => {
                        let def = self.logic.menu(&req);
                        let (sx, sy) = self.screen(x, y);
                        if let Some(cmd) = pw::popup(self.hwnd, sx, sy, &def, &self.palette, self.scale) {
                            let acts = self.logic.pick(&req, cmd, x, y);
                            self.run(acts);
                        }
                        repaint = true;
                    }
                    Act::TypeIn(f, x, y) => {
                        let (sx, sy) = self.screen(x, y);
                        let cur = self.logic.value_text(&f);
                        if let Some(t) = pw::type_in(self.hwnd, sx, sy, &cur, self.scale) {
                            self.logic.typed(&f, &t);
                        }
                        repaint = true;
                    }
                }
            }
            if repaint {
                self.invalidate();
            }
        }

        /// Answers the host (spec `plugins/host-link.md`): one page, the hint line. Unknown commands return 0.
        unsafe fn host_link(&mut self, wp: WPARAM, lp: LPARAM) -> LRESULT {
            use os_hostlink::{Cmd, PageIcon};
            let Some(cmd) = Cmd::from_usize(wp) else { return 0 };
            match cmd {
                Cmd::Hello => {
                    self.logic.hosted = true;
                    self.invalidate();
                    os_hostlink::VERSION as LRESULT
                }
                Cmd::PageCount => 1,
                Cmd::PageIcon => PageIcon::Plugin as LRESULT,
                Cmd::GetPage | Cmd::SetPage => 0,
                Cmd::Metrics => os_hostlink::pack_size(view::W, view::H),
                Cmd::Hint => {
                    if lp != 0 {
                        // SAFETY: the host passes a `HintBuf` of its own process (spec: host-link.md).
                        unsafe { &mut *(lp as *mut os_hostlink::HintBuf) }.set(&self.logic.hint);
                    }
                    1
                }
                Cmd::Changed | Cmd::Note | Cmd::Automate | Cmd::Audio | Cmd::Resizable | Cmd::SetSize => 0,
            }
        }

        fn paint(&mut self) {
            let (hdc, ps) = pw::begin_paint(self.hwnd);
            self.logic.paint(&mut self.canvas);
            self.buf.resize((self.size.0 * self.size.1) as usize, 0);
            self.canvas.copy_to_xrgb(&mut self.buf);
            pw::blit(hdc, &self.buf, self.size.0, self.size.1);
            pw::end_paint(self.hwnd, &ps);
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod gui {
    use std::sync::Arc;

    use nih_plug::prelude::GuiContext;
    use os_hostlink::{Cmd, PageIcon, Peer};
    use os_plugkit::gui::{self as px, Button, Input, Window};
    use os_theme::Palette;
    use os_ui::Canvas;

    use super::{Act, Btn, Logic, Mods};
    use crate::view;
    use crate::PulpParams;

    /// The editor on its GUI thread.
    struct Ed {
        win: Window,
        canvas: Canvas,
        buf: Vec<u32>,
        scale: f32,
        size: (u32, u32),
        logic: Logic,
        palette: Palette,
        last_hint: String,
        dirty: bool,
        /// The host link: the host finds the editor by the window it gave it (spec: plugins/host-link.md).
        link: Arc<Peer>,
    }

    impl Drop for Ed {
        fn drop(&mut self) {
            self.link.unregister();
        }
    }

    /// Owns the editor thread; dropping it closes the editor.
    pub struct Handle(#[allow(dead_code)] Option<px::EditorThread>);

    pub fn open(parent: px::Parent, scale: f32, params: Arc<PulpParams>, _ctx: Arc<dyn GuiContext>, slot: &px::Slot) -> Handle {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let logic = Logic::new(params);
        let size = ((view::W as f32 * scale).round() as u32, (view::H as f32 * scale).round() as u32);
        let wake = slot.clone();
        let link = Peer::register(parent as u64, vec![PageIcon::Plugin], (view::W, view::H), move || px::notify(&wake));
        let peer = link.clone();
        let thread = px::spawn(parent, size, slot, move |win| {
            let Ok(canvas) = Canvas::new(size.0, size.1, scale) else { return };
            let ed = Ed { win, canvas, buf: Vec::new(), scale, size, logic, palette: os_plugkit::palette(), last_hint: String::new(), dirty: true, link: peer };
            ed.run();
        });
        if thread.is_none() {
            link.unregister();
        }
        Handle(thread)
    }

    fn btn(b: Button) -> Btn {
        match b {
            Button::Left => Btn::Left,
            Button::Right => Btn::Right,
            Button::Middle => Btn::Middle,
        }
    }

    impl Ed {
        fn run(mut self) {
            while let Some(input) = self.win.next() {
                match input {
                    Input::Expose => self.dirty = true,
                    Input::Tick => self.poll(),
                    Input::Down { x, y, btn: b, dbl } => {
                        let (x, y) = (x / self.scale, y / self.scale);
                        let acts = self.logic.down(x, y, btn(b), self.mods(), dbl && b == Button::Left);
                        self.act(acts);
                    }
                    Input::Move { x, y } => {
                        let acts = self.logic.moved(x / self.scale, y / self.scale, self.mods());
                        self.act(acts);
                    }
                    Input::Delta { dx, dy } => {
                        let acts = self.logic.delta(dx / self.scale, dy / self.scale, self.mods());
                        self.act(acts);
                    }
                    Input::Up { .. } => {
                        self.win.unlock();
                        let acts = self.logic.up(Btn::Left);
                        self.act(acts);
                    }
                    Input::Leave => {
                        if !self.win.locked() && !self.logic.dragging() {
                            let acts = self.logic.leave();
                            self.act(acts);
                        }
                    }
                    Input::Wheel { x, y, notches } => {
                        let acts = self.logic.wheel(x / self.scale, y / self.scale, notches, self.mods());
                        self.act(acts);
                    }
                    Input::Key { .. } | Input::Drop { .. } => {}
                }
                if self.dirty && self.win.idle() {
                    self.paint();
                }
            }
        }

        fn mods(&self) -> Mods {
            let (ctrl, shift, alt) = self.win.mods();
            Mods { ctrl, shift, alt }
        }

        fn screen(&self, x: f32, y: f32) -> (i32, i32) {
            self.win.to_root((x * self.scale) as i32, (y * self.scale) as i32)
        }

        fn poll(&mut self) {
            if self.logic.poll() {
                self.dirty = true;
            }
            if self.logic.hint != self.last_hint {
                self.last_hint.clone_from(&self.logic.hint);
                self.link.set_hint(&self.logic.hint);
                self.link.to_host(Cmd::Changed, 0);
                self.dirty = true;
            }
            self.logic.hosted = self.link.hosted();
        }

        fn act(&mut self, acts: Vec<Act>) {
            for a in acts {
                match a {
                    Act::Repaint => self.dirty = true,
                    Act::Lock => self.win.lock(),
                    Act::Capture => {}
                    Act::Menu(req, x, y) => {
                        let def = self.logic.menu(&req);
                        let (sx, sy) = self.screen(x, y);
                        if let Some(cmd) = self.win.popup(sx, sy, &def, &self.palette, self.scale) {
                            let acts = self.logic.pick(&req, cmd, x, y);
                            self.act(acts);
                        }
                        self.dirty = true;
                    }
                    Act::TypeIn(f, x, y) => {
                        let (sx, sy) = self.screen(x, y);
                        let cur = self.logic.value_text(&f);
                        if let Some(t) = self.win.type_in(sx, sy, &cur, self.scale) {
                            self.logic.typed(&f, &t);
                        }
                        self.dirty = true;
                    }
                }
            }
        }

        fn paint(&mut self) {
            self.dirty = false;
            self.logic.paint(&mut self.canvas);
            self.buf.resize((self.size.0 * self.size.1) as usize, 0);
            self.canvas.copy_to_xrgb(&mut self.buf);
            self.win.blit(&self.buf, self.size.0, self.size.1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn logic() -> Logic {
        let mut l = Logic::new(Arc::new(PulpParams::default()));
        paint(&mut l);
        l
    }

    fn paint(l: &mut Logic) {
        let mut c = os_ui::Canvas::new(view::W, view::H, 1.0).unwrap();
        l.paint(&mut c);
    }

    fn centre(l: &Logic, t: &Target) -> (f32, f32) {
        let h = l.hits.iter().rev().find(|h| &h.t == t).unwrap_or_else(|| panic!("no hit {t:?}"));
        (h.r.cx(), h.r.cy())
    }

    fn saved(l: &Logic) -> State {
        crate::state_of(&l.params.state.read().unwrap().0)
    }

    /// The knobs, sliders and switches sit in FL's arrangement on the 6 px grid (plugins/lemon-pulp-filter.md §2).
    #[test]
    fn controls_sit_on_the_grid() {
        let l = logic();
        let c = |n| Target::Knob(control_index(0, n).unwrap());
        for (t, x, y) in [
            (c(C_IN), 29.0, 157.0),
            (c(C_ENV), 101.0, 157.0),
            (c(C_CUT), 144.0, 157.0),
            (c(C_RES), 187.0, 157.0),
            (c(C_PRE), 350.0, 96.0),
            (c(C_WSMIX), 350.0, 157.0),
            (c(C_PAN), 400.0, 96.0),
            (c(C_VOL), 400.0, 157.0),
            (Target::Knob(6), 503.0, 257.0),
            (Target::Knob(7), 555.0, 257.0),
            (Target::Knob(FOLLOW_ATTACK), 645.0, 215.0),
            (Target::Knob(FOLLOW_RELEASE), 645.0, 261.0),
        ] {
            let (cx, cy) = centre(&l, &t);
            assert!((cx - x).abs() < 1.0 && (cy - y).abs() < 1.0, "{t:?} at {cx},{cy}");
        }
        for (k, x) in [(MASTER, 623.0), (LFO_MASTER, 645.0), (FOLLOW_LEVEL, 667.0)] {
            assert!((centre(&l, &Target::Slider(k)).0 - x).abs() < 1.0);
        }
        assert!((centre(&l, &Target::Slider(control_index(0, C_NEXT).unwrap())).0 - 433.0).abs() < 1.0);
        for f in 0..FILTERS {
            assert!((centre(&l, &Target::Enable(f)).0 - view::enable_x(f)).abs() < 1.0);
        }
    }

    /// The SVF shows its L / B / H sliders; the shelves and peaking a GAIN knob; Mango a DRIVE knob.
    #[test]
    fn type_controls() {
        let mut l = logic();
        let low = control_index(0, C_LOW).unwrap();
        assert!(l.hits.iter().any(|h| h.t == Target::Slider(low)));
        l.pick(&MenuReq::Type, Cmd::Kind(PEAKING), 0.0, 0.0);
        paint(&mut l);
        assert!(l.hits.iter().any(|h| h.t == Target::Knob(low)));
        assert_eq!(saved(&l).filters[0].kind, PEAKING);
        l.wheel(90.0, 107.0, 1.0, Mods::default());
        assert_eq!(saved(&l).filters[0].kind, HIGH_SHELF - 2);
    }

    #[test]
    fn drags_wheel_and_reset_edit_the_saved_state() {
        let mut l = logic();
        let k = control_index(0, C_CUT).unwrap();
        let (x, y) = centre(&l, &Target::Knob(k));
        assert!(l.down(x, y, Btn::Left, Mods::default(), false).contains(&Act::Lock));
        l.delta(0.0, -125.0, Mods::default());
        l.up(Btn::Left);
        assert_eq!(saved(&l).filters[0].controls[C_CUT], 256);
        l.down(x, y, Btn::Middle, Mods::default(), false);
        assert_eq!(saved(&l).filters[0].controls[C_CUT], 128);
        l.wheel(x, y, -1.0, Mods::default());
        assert_eq!(saved(&l).filters[0].controls[C_CUT], 120);
        assert_eq!(l.hint, "Cutoff frequency: 47%");
    }

    #[test]
    fn filters_switches_and_orders() {
        let mut l = logic();
        let (x, y) = centre(&l, &Target::Enable(2));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert!(saved(&l).filters[2].enabled);
        l.down(x, y, Btn::Right, Mods::default(), false);
        assert_eq!(saved(&l).solo(), Some(2));
        let (x, y) = centre(&l, &Target::FilterTab(2));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(l.view.filter, 2);
        paint(&mut l);
        let (x, y) = centre(&l, &Target::Order(3));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(saved(&l).filters[2].order, 3);
        let (x, y) = centre(&l, &Target::Switch(Sw::Ws));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert!(saved(&l).filters[2].ws);
        let (x, y) = centre(&l, &Target::Switch(Sw::Center));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert!(saved(&l).dc());
        l.pick(&MenuReq::Oversampling, Cmd::Oversampling(3), 0.0, 0.0);
        assert_eq!(saved(&l).oversampling(), 3);
        let (x, y) = centre(&l, &Target::Pad);
        l.down(x, y, Btn::Left, Mods::default(), false);
        l.up(Btn::Left);
        let s = saved(&l);
        assert!((s.mod_x() - 512).abs() <= 8 && (s.mod_y() - 512).abs() <= 8);
    }

    #[test]
    fn curve_view_edits_the_pattern() {
        let mut l = logic();
        l.view.target = 2;
        paint(&mut l);
        let (x, y) = centre(&l, &Target::CurveArea);
        l.down(x, y - 20.0, Btn::Right, Mods::default(), false);
        let s = saved(&l);
        assert_eq!(s.filters[0].arts[T_CUT].patterns[0].points.len(), 2);
        // The pattern's switch.
        paint(&mut l);
        let (x, y) = centre(&l, &Target::Switch(Sw::PartOn));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert!(saved(&l).filters[0].arts[T_CUT].patterns[0].enabled);
        // Pattern 2 is another curve.
        l.wheel(centre(&l, &Target::Pattern).0, centre(&l, &Target::Pattern).1, 1.0, Mods::default());
        assert_eq!(saved(&l).globals[PATTERN], 2);
        paint(&mut l);
        assert!(!l.hits.iter().any(|h| h.t == Target::Point(1)));
        // The LFO's knobs.
        l.view.part = 1;
        paint(&mut l);
        assert!(l.hits.iter().any(|h| h.t == Target::Knob(lfo_index(0, T_CUT, 0))));
        // The waveshaper's curve: no part row.
        l.view.target = view::TAB_WS;
        paint(&mut l);
        assert!(!l.hits.iter().any(|h| matches!(h.t, Target::PartTab(_))));
        let (x, y) = centre(&l, &Target::Point(1));
        l.down(x, y, Btn::Left, Mods::default(), false);
        l.moved(x, y + 60.0, Mods::default());
        l.up(Btn::Left);
        assert!(saved(&l).filters[0].ws_curve.points[1].level < 0.8);
    }

    #[test]
    fn states_from_outside_are_read_again() {
        let mut l = logic();
        let mut st = State::new_default();
        st.globals[MASTER] = 64;
        l.params.set_state(st.to_bytes());
        assert!(l.poll());
        assert_eq!(l.st.globals[MASTER], 64);
    }

    /// `PULP_PNG=<dir> cargo test -p pulp-filter -- --ignored render_editor`: the editor as PNGs.
    #[test]
    #[ignore]
    fn render_editor() {
        let Some(dir) = std::env::var_os("PULP_PNG") else { return };
        let save = |l: &mut Logic, name: &str, scale: f32| {
            let (w, h) = ((view::W as f32 * scale) as u32, (view::H as f32 * scale) as u32);
            let mut c = os_ui::Canvas::new(w, h, scale).unwrap();
            l.paint(&mut c);
            let mut px = vec![0u32; (w * h) as usize];
            c.copy_to_xrgb(&mut px);
            let mut pm = tiny_skia::Pixmap::new(w, h).unwrap();
            for (o, p) in pm.data_mut().chunks_mut(4).zip(&px) {
                o.copy_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255]);
            }
            pm.save_png(std::path::Path::new(&dir).join(format!("{name}.png"))).unwrap();
        };
        let mut l = logic();
        save(&mut l, "pulp_default", 1.0);
        let def = r"C:\Program Files\Image-Line\FL Studio 2026\Plugins\Fruity\Effects\Fruity Love Philter\Data\Default.dat";
        if let Some(b) = std::fs::read(def).ok().filter(|b| State::from_bytes(b).is_some()) {
            l.params.set_state(b);
            l.poll();
            save(&mut l, "pulp_fl_default", 1.0);
            l.view.target = 2;
            save(&mut l, "pulp_fl_default_cut", 1.0);
            l.view.part = 1;
            save(&mut l, "pulp_fl_default_cut_lfo", 1.0);
            l.view.part = 2;
            save(&mut l, "pulp_fl_default_cut_modx", 1.0);
            l.view.filter = 1;
            l.view.part = 0;
            save(&mut l, "pulp_fl_default_f2", 1.0);
            l.view.target = view::TAB_WS;
            save(&mut l, "pulp_fl_default_ws", 1.0);
            l.hint = "Cutoff frequency: 50%".into();
            save(&mut l, "pulp_hint", 1.0);
            l.hint.clear();
            save(&mut l, "pulp_x2", 2.0);
        }
        l.view.info = true;
        save(&mut l, "pulp_info", 1.0);
    }
}
