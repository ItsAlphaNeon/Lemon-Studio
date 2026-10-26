// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The plugin editor: a child of the host's window (Win32, X11 on Linux, an NSView on macOS) that paints `view` with
//! the os-ui canvas, and the editor logic (platform independent). The editor keeps the state it shows; every edit
//! writes it back as the plugin's saved state and hands it to the audio thread (`PressParams::edited`). A state
//! replaced from outside (a preset, the host) is read again on the next tick.
//!
// spec: plugins/lemon-press.md §2 .. §4, plugins/host-link.md

use std::any::Any;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::Arc;

use nih_plug::prelude::*;
use os_engine::effects::maximus::state::*;
use os_plugkit::MenuDef;
use os_ui::kit::menu::{Mark, MenuItem, MenuStyle};

use crate::ctl;
use crate::edit::{self, POINT_MODES};
use crate::view::{self, Target, ViewState};
use crate::PressParams;

pub struct PressEditor {
    pub params: Arc<PressParams>,
    /// DPI scale as f32 bits.
    pub scale: AtomicU32,
    /// The open editor window (0 = closed).
    #[cfg_attr(not(windows), allow(dead_code))]
    hwnd: Arc<AtomicIsize>,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    slot: os_plugkit::gui::Slot,
}

impl PressEditor {
    pub fn new(params: Arc<PressParams>) -> Self {
        PressEditor {
            params,
            scale: AtomicU32::new(1f32.to_bits()),
            hwnd: Arc::new(AtomicIsize::new(0)),
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            slot: Default::default(),
        }
    }
}

impl Editor for PressEditor {
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
// Laws (the same as Citrus's: plugins/lemon-press.md §4)

/// Pixels of drag for a knob's whole range.
pub const KNOB_PX: f32 = 250.0;
/// Ctrl: this much finer.
const FINE: f32 = 1.0 / 20.0;
/// The wheel moves this part of the range per notch (at least one step).
const WHEEL_PART: f32 = 1.0 / 32.0;
/// Pixels of drag per step of a spinner.
pub const BOX_PX: f32 = 8.0;
/// Pixels of drag per unit of a segment's tension (-1 .. 1).
pub const TENSION_PX: f32 = 100.0;
/// The snap tool's grid on both axes: 32 steps.
const SNAP: f64 = 32.0;
/// How many meter readings the monitor keeps (one per tick).
const HISTORY: usize = 400;

// ---------------------------------------------------------------------------------------------------------------
// Menus

#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    None,
    Reset(usize),
    CopyValue(usize),
    PasteValue(usize),
    TypeIn(usize),
    /// The Options menu's switches.
    Option(Opt),
    ResetState,
    Curve(CurveCmd),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opt {
    /// 2x oversampling, or none (a state may hold more stages; unchecking turns them off).
    Oversample,
    MidTrigger,
    Fir,
    /// The 0.2 dB safety: the maps' 0 dB at 0.975 (on) or at 1.0.
    Safety,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CurveCmd {
    Delete(usize),
    Mode(usize, u8),
    TypeIn(usize),
    CopyMap,
    PasteMap,
    Reset,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuReq {
    Param(usize),
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
    /// Hide the cursor and hold it in place (knob and spinner drags).
    Lock,
    /// Keep the mouse while the button is down (points, edges).
    Capture,
    Menu(MenuReq, f32, f32),
    TypeIn(TypeTarget, f32, f32),
}

/// What a type-in box sets: a setting, or a point's output level in dB.
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
    /// A knob: the unrounded value it has reached.
    Value(usize, f32),
    /// A spinner, in steps.
    Steps(usize, f32),
    /// A point of the map.
    Point(usize),
    /// A segment's tension handle: which way up bends it (1 or -1).
    Tension(usize, f32),
    /// A crossover edge of the bands display: the frequency parameter it moves.
    Edge(usize),
}

pub struct Logic {
    pub params: Arc<PressParams>,
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
    map_clip: Option<Map>,
    /// The meters, one reading per tick, oldest first.
    pub history: Vec<[[f32; 2]; BANDS]>,
    pub hosted: bool,
}

/// The crossover knob's value for a frequency (the inverse of its law).
pub fn crossover_value(hz: f32) -> i32 {
    let v = (f64::from(hz) / 30.0).ln() / 533.333_333_333_333_4f64.ln() * 65536.0;
    (v.round() as i32).clamp(0, 65536)
}

impl Logic {
    pub fn new(params: Arc<PressParams>) -> Self {
        let mut l = Logic {
            params,
            st: State::new_default(),
            version: 0,
            view: ViewState::default(),
            hits: Vec::new(),
            drag: None,
            hint: String::new(),
            copied: None,
            map_clip: None,
            history: Vec::with_capacity(HISTORY),
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

    /// Timer: a state replaced from outside is read again; the meters move on. True when the editor has to repaint
    /// (always: the monitor scrolls).
    pub fn poll(&mut self) -> bool {
        if self.params.state_version.load(Ordering::Acquire) != self.version {
            self.reload();
        }
        if self.history.len() >= HISTORY {
            self.history.remove(0);
        }
        self.history.push(self.params.meters());
        true
    }

    /// An edit: the edited state becomes the plugin's at once and the audio thread plays it.
    fn commit(&mut self) {
        if !self.params.edited(self.version, self.st.clone()) {
            self.reload();
        }
    }

    fn b(&self) -> usize {
        self.st.shown().min(BANDS - 1)
    }

    fn set(&mut self, k: usize, v: i32) {
        let (lo, hi) = range(k);
        let v = v.clamp(lo, hi);
        if self.st.params[k] != v {
            self.st.set_param(k, v);
            self.commit();
        }
    }

    fn reset(&mut self, k: usize) {
        self.set(k, ctl::default(k));
    }

    /// Edits the shown band's map; a change is committed.
    fn edit_map(&mut self, f: impl FnOnce(&mut Map)) {
        let b = self.b();
        let m = self.st.map_mut(b);
        let before = m.clone();
        f(m);
        if *m != before {
            self.commit();
        }
    }

    fn snapped(&self, x: f64, l: f64) -> (f64, f64) {
        if self.view.tools[1] { ((x * SNAP).round() / SNAP, (l * SNAP).round() / SNAP) } else { (x, l) }
    }

    /// `Point 2/3: 0.0dB 1.00`, then the output level the same way.
    fn point_hint(&self, i: usize) -> String {
        let m = self.st.map(self.b());
        let Some(&(x, y)) = m.places().get(i) else { return String::new() };
        let g = |p: f64| ctl::gain_text(edit::scale_to_level(p));
        format!("Point {}/{}: {}\t{}", i + 1, m.points.len(), g(x), g(y))
    }

    fn hit(&self, x: f32, y: f32) -> Option<Target> {
        self.hits.iter().rev().find(|h| h.r.contains(x, y)).map(|h| h.t.clone())
    }

    fn param(t: &Target) -> Option<usize> {
        match t {
            Target::Knob(k) | Target::Steps(k) => Some(*k),
            _ => None,
        }
    }

    fn param_hint(&self, k: usize) -> String {
        format!("{}: {}", ctl::name(k), ctl::text(k, ctl::shown(&self.st, k)))
    }

    /// The hint of a target: FL's measured hints (`Name: value`), the Lemon Studio choices where FL's were not read
    /// (plugins/lemon-press.md §3).
    fn hint_for(&self, t: Option<&Target>) -> String {
        let b = self.b();
        let target = |b: usize| format!("Target: {}", ["low band", "mid band", "high band", "master"][b]);
        match t {
            None | Some(Target::CurveArea) | Some(Target::DisplayArea) | Some(Target::Cover) => String::new(),
            Some(Target::Knob(k)) | Some(Target::Steps(k)) | Some(Target::Slope(k)) => self.param_hint(*k),
            Some(Target::Mode(_)) => self.param_hint(band(b, B_MODE)),
            Some(Target::Rms(_)) => self.param_hint(band(b, B_RMS)),
            Some(Target::Lin) => "Linear phase (not done yet)".into(),
            Some(Target::Solo) => "Solo mode".into(),
            Some(Target::BandTab(b)) | Some(Target::Region(b)) => target(*b),
            Some(Target::Point(i)) => self.point_hint(*i),
            Some(Target::Tension(i)) => {
                let t = self.st.map(b).points.get(*i).map_or(0.0, |p| p.tension);
                format!("Point {} to {} tension\t{}%", i, i + 1, (t * 100.0).round())
            }
            Some(Target::CurveOptions) => "Compression mapping options".into(),
            Some(Target::Tool(k)) => view::TOOL_NAMES[*k].into(),
            Some(Target::Edge(i)) => self.param_hint(self.edge_param(*i)),
            Some(Target::DisplayTab(i)) => ["Display", "Display: bands"][*i].into(),
            Some(Target::Show(k)) => view::SHOW_NAMES[*k].into(),
            Some(Target::Header(i)) => ["Store in spare state", "Flip with spare state (A/B compare)", "Options", "Show info"][(*i).min(3)].into(),
            Some(Target::Logo) => "Show info".into(),
        }
    }

    /// The frequency parameter a drawn crossover edge stands for (the edges are drawn swapped when the low one is
    /// above the high one).
    fn edge_param(&self, i: usize) -> usize {
        let p = &self.st.params;
        let swapped = p[band(LOW, B_MODE)] != MODE_OFF && p[band(HIGH, B_MODE)] != MODE_OFF && p[P_LOW_FREQ] > p[P_HIGH_FREQ];
        match (i, swapped) {
            (0, false) | (1, true) => if p[band(LOW, B_MODE)] != MODE_OFF { P_LOW_FREQ } else { P_HIGH_FREQ },
            _ => if p[band(HIGH, B_MODE)] != MODE_OFF { P_HIGH_FREQ } else { P_LOW_FREQ },
        }
    }

    fn show_band(&mut self, b: usize) {
        if self.st.shown() != b {
            self.st.set_shown(b);
            self.commit();
        }
    }

    pub fn down(&mut self, x: f32, y: f32, btn: Btn, m: Mods, _dbl: bool) -> Vec<Act> {
        self.view.mouse = Some((x, y));
        let Some(t) = self.hit(x, y) else { return vec![] };
        let mut acts = vec![Act::Repaint];
        let resettable = Self::param(&t);
        let b = self.b();
        match (&t, btn) {
            (Target::Cover, _) => self.view.info = false,
            (Target::BandTab(b) | Target::Region(b), Btn::Left | Btn::Right) => self.show_band(*b),
            (Target::Mode(md), Btn::Left) => self.set(band(b, B_MODE), *md),
            (Target::Rms(on), Btn::Left) => self.set(band(b, B_RMS), i32::from(*on)),
            (Target::Slope(k), Btn::Left) => self.set(*k, 1 - ctl::shown(&self.st, *k)),
            (Target::Solo, Btn::Left) => {
                let on = !self.st.solo();
                self.st.set_solo(on);
                self.commit();
            }
            (Target::Header(0), Btn::Left) => {
                self.st.store_spare();
                self.commit();
            }
            (Target::Header(1), Btn::Left) => {
                self.st.flip_spare();
                self.commit();
            }
            (Target::Header(2), Btn::Left) => acts.push(Act::Menu(MenuReq::Options, x, y)),
            (Target::Header(_), Btn::Left) | (Target::Logo, Btn::Left) => self.view.info = true,
            (Target::CurveOptions, Btn::Left) => acts.push(Act::Menu(MenuReq::Curve, x, y)),
            (Target::Tool(k), Btn::Left) => self.view.tools[*k] = !self.view.tools[*k],
            (Target::Show(k), Btn::Left) => self.view.show[*k] = !self.view.show[*k],
            (Target::DisplayTab(i), Btn::Left | Btn::Right) => self.view.display = *i,
            (Target::Point(i), Btn::Left) => {
                self.drag = Some(Drag::Point(*i));
                self.view.pressed = Some(t.clone());
                acts.push(Act::Capture);
            }
            (Target::Point(i), Btn::Right) => acts.push(Act::Menu(MenuReq::Point(*i), x, y)),
            (Target::Tension(i), Btn::Left) => {
                // Up raises the segment's middle, whichever way a tension bends it.
                let mut mp = self.st.map(b).clone();
                let mut mid = |t: f32| {
                    edit::set_tension(&mut mp, *i, t);
                    view::map_points(&mp).1[*i].1
                };
                let sign = if mid(0.5) >= mid(-0.5) { 1.0 } else { -1.0 };
                self.drag = Some(Drag::Tension(*i, sign));
                self.view.pressed = Some(t.clone());
                acts.push(Act::Lock);
            }
            (Target::Tension(i), Btn::Middle) => self.edit_map(|m| edit::set_tension(m, *i, 0.0)),
            // A right click on empty space adds a point there.
            (Target::CurveArea, Btn::Right) if !self.view.tools[0] => {
                let (px, l) = self.snapped(view::x_to_place(x), view::y_to_level(y));
                self.edit_map(|m| {
                    edit::insert_point(m, px.clamp(1e-3, 1.0 - 1e-3), l);
                });
            }
            (Target::Edge(i), Btn::Left) => {
                self.drag = Some(Drag::Edge(self.edge_param(*i)));
                self.view.pressed = Some(t.clone());
                acts.push(Act::Capture);
            }
            (_, Btn::Left) if m.alt && resettable.is_some() => self.reset(resettable.unwrap()),
            (_, Btn::Middle) if resettable.is_some() => self.reset(resettable.unwrap()),
            (Target::Knob(k), Btn::Left) => {
                self.drag = Some(Drag::Value(*k, ctl::shown(&self.st, *k) as f32));
                self.view.pressed = Some(t.clone());
                acts.push(Act::Lock);
            }
            (Target::Steps(k), Btn::Left) => {
                self.drag = Some(Drag::Steps(*k, ctl::shown(&self.st, *k) as f32));
                self.view.pressed = Some(t.clone());
                acts.push(Act::Lock);
            }
            (Target::Knob(k) | Target::Steps(k), Btn::Right) => acts.push(Act::Menu(MenuReq::Param(*k), x, y)),
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
                let t = self.st.map(self.b()).points.get(i).map_or(0.0, |p| p.tension) - sign * dy * f / TENSION_PX;
                self.edit_map(|m| edit::set_tension(m, i, t));
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
        let t = self.view.mouse.and_then(|(x, y)| self.hit(x, y));
        self.hint = self.hint_for(t.as_ref());
        vec![Act::Repaint]
    }

    pub fn moved(&mut self, x: f32, y: f32, _m: Mods) -> Vec<Act> {
        self.view.mouse = Some((x, y));
        match self.drag {
            Some(Drag::Point(i)) => {
                let (px, l) = self.snapped(view::x_to_place(x), view::y_to_level(y));
                self.edit_map(|m| edit::move_point(m, i, px, l));
                self.hint = self.point_hint(i);
            }
            Some(Drag::Edge(k)) => {
                self.set(k, crossover_value(view::x_to_hz(x)));
                self.hint = self.param_hint(k);
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

    /// Wheel: 1/32 of the range per notch (at least one step); spinners one step; switches and radios step.
    pub fn wheel(&mut self, x: f32, y: f32, notches: f32, _m: Mods) -> Vec<Act> {
        let Some(t) = self.hit(x, y) else { return vec![] };
        let d = notches.signum() as i32;
        let b = self.b();
        match &t {
            Target::Knob(k) => {
                let (lo, hi) = range(*k);
                let step = (((hi - lo) as f32 * WHEEL_PART).round() as i32).max(1);
                self.set(*k, ctl::shown(&self.st, *k) + d * step);
            }
            Target::Steps(k) => self.set(*k, ctl::shown(&self.st, *k) + d),
            Target::Slope(k) => self.set(*k, i32::from(d > 0)),
            Target::Mode(_) => self.set(band(b, B_MODE), ctl::shown(&self.st, band(b, B_MODE)) - d),
            Target::Rms(_) => self.set(band(b, B_RMS), i32::from(d < 0)),
            Target::BandTab(_) => self.show_band((b as i32 - d).clamp(0, BANDS as i32 - 1) as usize),
            _ => return vec![],
        }
        self.hint = self.hint_for(Some(&t));
        vec![Act::Repaint]
    }

    pub fn menu(&self, req: &MenuReq) -> MenuDef<Cmd> {
        match req {
            MenuReq::Param(k) => {
                let mut m = MenuDef::new(MenuStyle::Context);
                m.head(ctl::name(*k), Cmd::None);
                m.add(MenuItem::item("Reset"), Cmd::Reset(*k));
                m.sep(Cmd::None);
                m.add(MenuItem::item("Copy value"), Cmd::CopyValue(*k));
                m.add(MenuItem::item("Paste value").enabled(self.copied.is_some()), Cmd::PasteValue(*k));
                m.add(MenuItem::item("Type in value..."), Cmd::TypeIn(*k));
                m
            }
            MenuReq::Options => {
                let mut m = MenuDef::new(MenuStyle::Context);
                for (opt, l, on) in [
                    (Opt::Oversample, "Oversample", self.st.oversampling() > 0),
                    (Opt::MidTrigger, "Master mid mode", self.st.mid_trigger()),
                    (Opt::Fir, "Linear-phase filters (legacy mode)", self.st.fir()),
                    (Opt::Safety, "0.2 dB compressor safety", !self.st.full_ceiling()),
                ] {
                    let item = MenuItem::item(l).mark(Mark::Check(on));
                    // A state older than version 14 has no place for the full ceiling.
                    let item = if opt == Opt::Safety { item.enabled(self.st.ext.is_some()) } else { item };
                    m.add(item, Cmd::Option(opt));
                }
                m.sep(Cmd::None);
                m.add(MenuItem::item("Reset all settings"), Cmd::ResetState);
                m
            }
            MenuReq::Point(i) => self.point_menu(*i),
            MenuReq::Curve => {
                let mut m = MenuDef::new(MenuStyle::Context);
                m.head("Map", Cmd::None);
                m.add(MenuItem::item("Copy map"), Cmd::Curve(CurveCmd::CopyMap));
                m.add(MenuItem::item("Paste map").enabled(self.map_clip.is_some()), Cmd::Curve(CurveCmd::PasteMap));
                m.add(MenuItem::item("Reset"), Cmd::Curve(CurveCmd::Reset));
                m
            }
        }
    }

    fn point_menu(&self, i: usize) -> MenuDef<Cmd> {
        let mut m = MenuDef::new(MenuStyle::Context);
        let mp = self.st.map(self.b());
        let Some(p) = mp.points.get(i) else { return m };
        let cmd = Cmd::Curve;
        m.head(&format!("Point {}/{}", i + 1, mp.points.len()), Cmd::None);
        let fixed = i == 0 || i + 1 == mp.points.len();
        m.add(MenuItem::item("Delete").enabled(!fixed), cmd(CurveCmd::Delete(i)));
        if i > 0 {
            m.head("Mode", Cmd::None);
            for (l, v) in POINT_MODES {
                m.add(MenuItem::item(l).mark(Mark::Radio(p.mode == v)), cmd(CurveCmd::Mode(i, v)));
            }
        }
        m.sep(Cmd::None);
        m.add(MenuItem::item("Type in value..."), cmd(CurveCmd::TypeIn(i)));
        m
    }

    fn curve_cmd(&mut self, k: CurveCmd) {
        let n = self.st.map(self.b()).points.len();
        match k {
            CurveCmd::Delete(i) | CurveCmd::Mode(i, _) if i >= n => {}
            CurveCmd::CopyMap => self.map_clip = Some(self.st.map(self.b()).clone()),
            CurveCmd::TypeIn(_) => {}
            k => {
                let clip = self.map_clip.clone();
                self.edit_map(|m| match k {
                    CurveCmd::Delete(i) => {
                        edit::delete_point(m, i);
                    }
                    CurveCmd::Mode(i, v) => edit::set_mode(m, i, v),
                    CurveCmd::PasteMap => {
                        if let Some(c) = clip {
                            *m = c;
                        }
                    }
                    CurveCmd::Reset => *m = Map::default_map(),
                    _ => {}
                });
            }
        }
    }

    pub fn pick(&mut self, _req: &MenuReq, cmd: Cmd, x: f32, y: f32) -> Vec<Act> {
        match cmd {
            Cmd::None => {}
            Cmd::Reset(k) => self.reset(k),
            Cmd::CopyValue(k) => self.copied = Some(self.st.params[k]),
            Cmd::PasteValue(k) => {
                if let Some(v) = self.copied {
                    self.set(k, v);
                }
            }
            Cmd::TypeIn(k) => return vec![Act::TypeIn(TypeTarget::Param(k), x, y)],
            Cmd::Option(o) => {
                match o {
                    Opt::Oversample => self.st.set_oversampling(u32::from(self.st.oversampling() == 0)),
                    Opt::MidTrigger => self.st.set_mid_trigger(!self.st.mid_trigger()),
                    Opt::Fir => self.st.set_fir(!self.st.fir()),
                    Opt::Safety => self.st.set_full_ceiling(!self.st.full_ceiling()),
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
            TypeTarget::Param(k) => ctl::text(k, ctl::shown(&self.st, k)),
            TypeTarget::Point(i) => self
                .st
                .map(self.b())
                .places()
                .get(i)
                .map(|&(_, y)| format!("{:.1}", 20.0 * edit::scale_to_level(y).max(1e-9).log10()))
                .unwrap_or_default(),
        }
    }

    /// Applies a typed value; text that does not read leaves it as it was. A point's output level is typed in dB.
    pub fn typed(&mut self, t: &TypeTarget, text: &str) {
        match *t {
            TypeTarget::Param(k) => {
                if let Some(v) = ctl::parse(k, text) {
                    self.set(k, v);
                }
            }
            TypeTarget::Point(i) => {
                let num: String = text.trim().chars().take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+')).collect();
                let Some(db) = num.parse::<f64>().ok().filter(|d| d.is_finite()) else { return };
                let Some(&(x, _)) = self.st.map(self.b()).places().get(i) else { return };
                let l = edit::level_to_scale(10f64.powf(db / 20.0));
                self.edit_map(|m| edit::move_point(m, i, x, l));
            }
        }
    }

    /// Paints the editor; `hint` is drawn when the host does not show it.
    pub fn paint(&mut self, c: &mut os_ui::Canvas) {
        let hint = if self.hosted { String::new() } else { self.hint.clone() };
        let model = view::Model { st: &self.st, history: &self.history, version: env!("CARGO_PKG_VERSION") };
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
    use crate::PressParams;

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

    pub fn open(parent: *mut c_void, scale: f32, params: Arc<PressParams>, _ctx: Arc<dyn GuiContext>, slot: Arc<AtomicIsize>) -> Handle {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let logic = Logic::new(params);
        let px = |v: u32| (v as f32 * scale).round() as u32;
        let size = (px(view::W), px(view::H));
        unsafe {
            let class = pw::register("PressView", Some(wndproc));
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
            let title = pw::wide("Press");
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
    use crate::PressParams;

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

    pub fn open(parent: px::Parent, scale: f32, params: Arc<PressParams>, _ctx: Arc<dyn GuiContext>, slot: &px::Slot) -> Handle {
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
        let mut l = Logic::new(Arc::new(PressParams::default()));
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

    /// The knobs sit on the 6 px grid's rows and columns (plugins/lemon-press.md §2).
    #[test]
    fn controls_sit_on_the_grid() {
        let l = logic();
        let m = |k| Target::Knob(band(MASTER, k));
        for (t, x, y) in [
            (m(B_PRE), 180.0, 312.0),
            (m(B_POST), 228.0, 312.0),
            (m(B_SEP), 118.0, 416.0),
            (m(B_THRES), 180.0, 416.0),
            (m(B_CEIL), 228.0, 416.0),
            (m(B_ATT), 292.0, 312.0),
            (m(B_REL), 348.0, 312.0),
            (m(B_WINDOW), 404.0, 312.0),
            (m(B_REL2), 292.0, 416.0),
            (Target::Knob(P_LMH_DELAY), 468.0, 312.0),
            (Target::Knob(P_LMH_MIX), 468.0, 416.0),
            (Target::Knob(P_LOW_FREQ), 540.0, 312.0),
            (Target::Knob(P_HIGH_FREQ), 598.0, 312.0),
            (Target::Knob(P_LOW_CUT), 569.0, 416.0),
        ] {
            let (cx, cy) = centre(&l, &t);
            assert!((cx - x).abs() < 1.0 && (cy - y).abs() < 1.0, "{t:?} at {cx},{cy}");
        }
        for b in 0..BANDS {
            assert_eq!(centre(&l, &Target::BandTab(b)), (view::band_tab(b).cx(), view::band_tab(b).cy()));
        }
    }

    #[test]
    fn drags_wheel_and_reset_edit_the_saved_state() {
        let mut l = logic();
        let k = band(MASTER, B_POST);
        let (x, y) = centre(&l, &Target::Knob(k));
        assert!(l.down(x, y, Btn::Left, Mods::default(), false).contains(&Act::Lock));
        l.delta(0.0, -125.0, Mods::default());
        l.up(Btn::Left);
        assert_eq!(saved(&l).params[k], 2000);
        assert_eq!(l.hint, "Post gain: 18.1dB 8.00");
        l.down(x, y, Btn::Middle, Mods::default(), false);
        assert_eq!(saved(&l).params[k], 1000);
        l.wheel(x, y, -1.0, Mods::default());
        assert_eq!(saved(&l).params[k], 937);
    }

    #[test]
    fn tabs_switches_and_options() {
        let mut l = logic();
        let (x, y) = centre(&l, &Target::BandTab(LOW));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(saved(&l).shown(), LOW);
        paint(&mut l);
        // The cards now show LOW's settings.
        assert!(l.hits.iter().any(|h| h.t == Target::Knob(band(LOW, B_PRE))));
        let (x, y) = centre(&l, &Target::Mode(MODE_MUTED));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(saved(&l).params[band(LOW, B_MODE)], MODE_MUTED);
        let (x, y) = centre(&l, &Target::Rms(true));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(saved(&l).params[band(LOW, B_RMS)], 1);
        let (x, y) = centre(&l, &Target::Slope(P_LOW_SLOPE));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(saved(&l).params[P_LOW_SLOPE], 0);
        let (x, y) = centre(&l, &Target::Steps(band(LOW, B_REL_STAGES)));
        l.wheel(x, y, 1.0, Mods::default());
        assert_eq!(saved(&l).params[band(LOW, B_REL_STAGES)], 4);
        l.pick(&MenuReq::Options, Cmd::Option(Opt::Fir), 0.0, 0.0);
        assert!(saved(&l).fir());
        let (x, y) = centre(&l, &Target::Solo);
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert!(saved(&l).solo());
        l.pick(&MenuReq::Options, Cmd::Option(Opt::Oversample), 0.0, 0.0);
        assert_eq!(saved(&l).oversampling(), 1);
        l.pick(&MenuReq::Options, Cmd::Option(Opt::Safety), 0.0, 0.0);
        assert!(saved(&l).full_ceiling());
        // A / B: store, change, flip back.
        let (x, y) = centre(&l, &Target::Header(0));
        l.down(x, y, Btn::Left, Mods::default(), false);
        l.pick(&MenuReq::Param(P_LMH_MIX), Cmd::Reset(P_LMH_MIX), 0.0, 0.0);
        let k = band(LOW, B_PRE);
        let st = saved(&l);
        l.st.params[k] = 0;
        l.commit();
        let (x, y) = centre(&l, &Target::Header(1));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(saved(&l).params, st.params);
        assert_eq!(saved(&l).twin[k], 0);
    }

    #[test]
    fn the_map_view_edits_the_shown_bands_map() {
        let mut l = logic();
        let (x, y) = centre(&l, &Target::CurveArea);
        l.down(x - 40.0, y + 60.0, Btn::Right, Mods::default(), false);
        assert_eq!(saved(&l).map(MASTER).points.len(), 4);
        paint(&mut l);
        let (x, y) = centre(&l, &Target::Point(1));
        l.down(x, y, Btn::Left, Mods::default(), false);
        l.moved(x, y - 30.0, Mods::default());
        l.up(Btn::Left);
        let m = saved(&l).map(MASTER).clone();
        assert!(m.points[1].level > 0.2, "{m:?}");
        assert!(l.point_hint(1).starts_with("Point 2/4: "));
        l.pick(&MenuReq::Point(1), Cmd::Curve(CurveCmd::Delete(1)), 0.0, 0.0);
        assert_eq!(saved(&l).map(MASTER).points.len(), 3);
        l.pick(&MenuReq::Curve, Cmd::Curve(CurveCmd::Reset), 0.0, 0.0);
        assert_eq!(saved(&l).map(MASTER), &Map::default_map());
        l.typed(&TypeTarget::Point(1), "-6");
        let y = saved(&l).map(MASTER).places()[1].1;
        assert!((20.0 * edit::scale_to_level(y).log10() + 6.0).abs() < 0.01);
    }

    /// The bands display's edges move the crossover.
    #[test]
    fn crossover_edges_drag() {
        let mut l = logic();
        l.view.display = 1;
        paint(&mut l);
        let (x, y) = centre(&l, &Target::Edge(0));
        l.down(x, y, Btn::Left, Mods::default(), false);
        l.moved(view::hz_to_x(100.0), y, Mods::default());
        l.up(Btn::Left);
        let v = saved(&l).params[P_LOW_FREQ];
        assert_eq!(ctl::text(P_LOW_FREQ, v), "100Hz");
        assert_eq!(crossover_value(os_engine::effects::maximus::crossover_hz(19800)), 19800);
    }

    #[test]
    fn states_from_outside_are_read_again() {
        let mut l = logic();
        let mut st = State::new_default();
        st.params[P_LMH_MIX] = 500;
        l.params.set_state(st.to_bytes());
        assert!(l.poll());
        assert_eq!(l.st.params[P_LMH_MIX], 500);
    }

    /// `PRESS_PNG=<dir> cargo test -p press -- --ignored render_editor`: the editor as PNGs.
    #[test]
    #[ignore]
    fn render_editor() {
        let Some(dir) = std::env::var_os("PRESS_PNG") else { return };
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
        // A made-up meter history: a band level swinging around 0 dB and its gain under it.
        for i in 0..400 {
            let t = i as f32 / 20.0;
            let v = 0.6 + 0.5 * t.sin().abs();
            let g = (0.975 / v).min(1.0);
            l.history.push([[v * 0.8, g.powf(0.5)], [v * 0.6, 1.0], [v * 0.4, g.powf(0.3)], [v, g]]);
        }
        save(&mut l, "press_default", 1.0);
        save(&mut l, "press_x2", 2.0);
        l.view.display = 1;
        l.st.set_shown(LOW);
        save(&mut l, "press_bands_low", 1.0);
        l.view.display = 0;
        l.view.show = [true, true, true];
        l.hint = "Post gain\t0.0dB 1.00".into();
        save(&mut l, "press_hint", 1.0);
        l.hint.clear();
        let presets = r"C:\Program Files\Image-Line\FL Studio 2026\Data\Patches\Plugin presets\Effects\Maximus";
        for name in ["Mastering/Max loudness", "Fancy/Gate", "Default"] {
            let path = std::path::Path::new(presets).join(format!("{name}.fst"));
            if let Some(st) = std::fs::read(path).ok().and_then(|b| fst_state(&b)) {
                l.params.set_state(st);
                l.poll();
                save(&mut l, &format!("press_fl_{}", name.to_lowercase().replace([' ', '/'], "_")), 1.0);
            }
        }
        l.view.info = true;
        save(&mut l, "press_info", 1.0);
    }

    /// The state in a preset file: the data of its event 213 (a test helper; presets are FLP-like event files).
    fn fst_state(b: &[u8]) -> Option<Vec<u8>> {
        let mut i = 22;
        while i < b.len() {
            let id = b[i];
            i += 1;
            let n = match id {
                0..=63 => 1,
                64..=127 => 2,
                128..=191 => 4,
                _ => {
                    let mut n = 0usize;
                    let mut s = 0;
                    loop {
                        let c = *b.get(i)?;
                        i += 1;
                        n |= ((c & 0x7f) as usize) << s;
                        s += 7;
                        if c & 0x80 == 0 {
                            break n;
                        }
                    }
                }
            };
            if id == 213 {
                return b.get(i..i + n).map(<[u8]>::to_vec);
            }
            i += n;
        }
        None
    }
}
