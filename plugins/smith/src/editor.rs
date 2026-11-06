// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The plugin editor: a child of the host's window (Win32, X11 on Linux, an NSView on macOS) that paints `view` with
//! the os-ui canvas, and the editor logic (platform independent). The editor keeps the state it shows; every edit
//! writes it back as the plugin's saved state and hands its patch to the audio thread. A state replaced from outside
//! (a preset, the host) is read again on the next tick.
//!
// spec: plugins/smith.md, plugins/ogun.md ("Controls and ranges"), plugins/host-link.md

use std::any::Any;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use citrus::edit;
use nih_plug::prelude::*;
use os_dsp::curve::{Curve, MAP, SYNCED};
use os_plugkit::MenuDef;
use os_ui::kit::menu::{Mark, MenuItem, MenuStyle};

use crate::ctl::{self, Field, Flag};
use crate::state::*;
use crate::synth::Patch;
use crate::view::{self, CurveId, Target, ViewState};
use crate::SmithParams;

pub struct SmithEditor {
    pub params: Arc<SmithParams>,
    /// DPI scale as f32 bits.
    pub scale: AtomicU32,
    /// The keyboard is hidden: it sets the editor's size, also across a close and reopen.
    pub no_keys: Arc<AtomicBool>,
    /// The open editor window (0 = closed).
    #[cfg_attr(not(windows), allow(dead_code))]
    hwnd: Arc<AtomicIsize>,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    slot: os_plugkit::gui::Slot,
}

impl SmithEditor {
    pub fn new(params: Arc<SmithParams>) -> Self {
        SmithEditor {
            params,
            scale: AtomicU32::new(1f32.to_bits()),
            no_keys: Arc::new(AtomicBool::new(false)),
            hwnd: Arc::new(AtomicIsize::new(0)),
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            slot: Default::default(),
        }
    }
}

impl Editor for SmithEditor {
    fn spawn(&self, parent: ParentWindowHandle, context: Arc<dyn GuiContext>) -> Box<dyn Any + Send> {
        let scale = f32::from_bits(self.scale.load(Ordering::Relaxed));
        let (params, keys) = (self.params.clone(), self.no_keys.clone());
        match parent {
            #[cfg(windows)]
            ParentWindowHandle::Win32Hwnd(hwnd) => Box::new(win32::open(hwnd, scale, params, keys, context, self.hwnd.clone())),
            #[cfg(target_os = "linux")]
            ParentWindowHandle::X11Window(window) => Box::new(gui::open(window, scale, params, keys, context, &self.slot)),
            #[cfg(target_os = "macos")]
            ParentWindowHandle::AppKitNsView(v) => {
                let scale = os_plugkit::gui::backing_scale(v as usize);
                Box::new(gui::open(v as usize, scale, params, keys, context, &self.slot))
            }
            _ => {
                let _ = (context, scale, params, keys);
                Box::new(())
            }
        }
    }

    fn size(&self) -> (u32, u32) {
        (view::W, view::height(self.no_keys.load(Ordering::Relaxed)))
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
// Laws (the same as Citrus's)

/// Pixels of drag for a knob's whole range.
pub const KNOB_PX: f32 = 250.0;
/// Ctrl: this much finer.
const FINE_DRAG: f32 = 1.0 / 20.0;
/// The wheel moves this part of the range per notch (at least one step).
const WHEEL_PART: f32 = 1.0 / 32.0;
/// Pixels of drag per step of a spinner.
pub const BOX_PX: f32 = 8.0;
/// Pixels of drag per unit of a segment's tension (-1 .. 1).
pub const TENSION_PX: f32 = 100.0;
/// The snap tool's grid: time and level steps.
const SNAP_T: f64 = 32.0;
const SNAP_L: f32 = 16.0;

/// A curve point's modes, in the order the menu lists them.
const POINT_MODES: [(&str, u8); 13] = [
    ("Hold", 2),
    ("Smooth", 10),
    ("Single curve", 0),
    ("Single curve 2", 7),
    ("Single curve 3", 11),
    ("Double curve", 1),
    ("Double curve 2", 8),
    ("Double curve 3", 12),
    ("Half sine", 9),
    ("Stairs", 3),
    ("Smooth stairs", 4),
    ("Pulse", 5),
    ("Wave", 6),
];

/// The preset the Options menu copied, for every open editor.
static PRESET_CLIP: Mutex<Option<Vec<u8>>> = Mutex::new(None);

// ---------------------------------------------------------------------------------------------------------------
// Menus

#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    None,
    Reset(Field),
    CopyValue(Field),
    PasteValue(Field),
    TypeIn(Field),
    Set(Field, i32),
    Toggle(Field),
    Target(usize),
    Part(usize),
    CopyPreset,
    PastePreset,
    Random,
    RandomNumber,
    Effect(Fx),
    About,
    Curve(CurveId, CurveCmd),
}

/// The effect presets of the Options menu whose values were measured: the default state's and off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fx {
    Unison(bool),
    Chorus(bool),
    Reverb(bool),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CurveCmd {
    Delete(usize),
    Mode(usize, u8),
    Break(usize, u8),
    Marker(usize, usize),
    CopyLevel(usize),
    PasteLevel(usize),
    TypeIn(usize),
    CopyState,
    PasteState,
    AllLoop,
    Flip,
    Normalize,
    AllSmooth,
    SmoothAbrupt,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuReq {
    Param(Field),
    Options,
    Target,
    Part,
    Comb,
    EqType(usize),
    Point(CurveId, usize),
    Curve(CurveId),
}

// ---------------------------------------------------------------------------------------------------------------
// Editor logic

/// Something the window has to do after an event.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Repaint,
    /// Hide the cursor and hold it in place (knob and spinner drags).
    Lock,
    /// Keep the mouse while the button is down (points, the pad, the keyboard).
    Capture,
    Menu(MenuReq, f32, f32),
    TypeIn(TypeTarget, f32, f32),
    /// A key of the testing keyboard, down or up.
    Note(u8, bool),
    /// The keyboard was shown or hidden: the editor changes size.
    Resize,
}

/// What a type-in box sets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TypeTarget {
    Param(Field),
    Point(CurveId, usize),
    Random,
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
    Value(Field, f32),
    /// A spinner, in steps.
    Steps(Field, f32),
    Pad,
    /// A point of the shown curve; the time span the drag began with.
    Point(usize, f64),
    /// A segment's tension handle: which way up bends it (1 or -1).
    Tension(usize, f32),
    Key,
}

pub struct Logic {
    pub params: Arc<SmithParams>,
    /// The state shown and edited.
    pub st: State,
    /// The state version `st` was read at.
    version: u64,
    pub view: ViewState,
    pub hits: Vec<view::Hit>,
    drag: Option<Drag>,
    /// An edit not yet handed to the plugin: during a drag once a tick (a patch takes its time).
    pending: bool,
    /// Hint text (a tab before the value).
    pub hint: String,
    copied: Option<i32>,
    level_clip: Option<f32>,
    curve_clip: Option<Curve>,
    /// The random preset last made.
    random: u32,
    pub hosted: bool,
    /// The keyboard hidden, as the host asks for the editor's size.
    no_keys: Arc<AtomicBool>,
}

impl Logic {
    pub fn new(params: Arc<SmithParams>, no_keys: Arc<AtomicBool>) -> Self {
        // As Ogun opens: on the impulse EQ, with Slide on.
        let view = ViewState { no_keys: no_keys.load(Ordering::Relaxed), target: ARTS + M_IMPULSE, tools: [false, false, false, true], ..ViewState::default() };
        let mut l = Logic {
            params,
            st: State::default(),
            version: 0,
            view,
            hits: Vec::new(),
            drag: None,
            pending: false,
            hint: String::new(),
            copied: None,
            level_clip: None,
            curve_clip: None,
            random: 1,
            hosted: false,
            no_keys,
        };
        l.reload();
        l
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// The painted height, in logical px.
    pub fn height(&self) -> u32 {
        view::height(self.view.no_keys)
    }

    fn reload(&mut self) {
        self.version = self.params.state_version.load(Ordering::Acquire);
        let bytes = self.params.state.read().map(|s| s.0.clone()).unwrap_or_default();
        self.st = crate::state_of(&bytes);
        self.pending = false;
    }

    /// Timer: the edit a drag holds is handed over, a state replaced from outside is read again. True when the editor
    /// has to repaint.
    pub fn poll(&mut self) -> bool {
        self.flush();
        let playing = self.params.playing.each_ref().map(|a| a.load(Ordering::Relaxed));
        let keys = std::mem::replace(&mut self.view.playing, playing) != playing && !self.view.no_keys;
        if self.params.state_version.load(Ordering::Acquire) != self.version {
            self.reload();
            return true;
        }
        keys
    }

    /// An edit: the edited state becomes the plugin's and the audio thread plays it; during a drag on the next tick.
    fn commit(&mut self) {
        self.pending = true;
        if self.drag.is_none() {
            self.flush();
        }
    }

    pub fn flush(&mut self) {
        if !std::mem::take(&mut self.pending) {
            return;
        }
        // The state was replaced meanwhile: show that one.
        if !self.params.stored(self.version, self.st.to_bytes()) || !self.params.play(self.version, Patch::new(self.st.clone())) {
            self.reload();
        }
    }

    fn set(&mut self, f: Field, v: i32) {
        let (lo, hi) = ctl::range(f);
        let v = v.clamp(lo, hi);
        if ctl::get(&self.st, f) != v {
            ctl::set(&mut self.st, f, v);
            self.commit();
        }
    }

    fn reset(&mut self, f: Field) {
        self.set(f, ctl::default(f));
    }

    fn curve_id(&self) -> CurveId {
        view::shown_curve(&self.view)
    }

    fn curve(&self, id: CurveId) -> &Curve {
        view::curve_of(&self.st, id)
    }

    /// Freeze editing, and a curve too long to edit by hand.
    fn frozen(&self, id: CurveId) -> bool {
        self.view.tools[0] || self.curve(id).points.len() > view::MANY_POINTS
    }

    /// Edits a curve; a change is committed.
    fn edit_curve(&mut self, id: CurveId, f: impl FnOnce(&mut Curve)) {
        let c = view::curve_mut(&mut self.st, id);
        let before = c.clone();
        f(c);
        if *c != before {
            self.commit();
        }
    }

    /// A time and level on the snap tool's grid (when it is on).
    fn snapped(&self, t: f64, l: f32, span: f64) -> (f64, f32) {
        if self.view.tools[2] { ((t / span * SNAP_T).round() * span / SNAP_T, (l * SNAP_L).round() / SNAP_L) } else { (t, l) }
    }

    /// `Point 5/7:2:517`, the level as the right part: times are half seconds (`s:ms`), `bar:beat:tick` when
    /// tempo-based; a map's positions are percent.
    fn point_hint(&self, i: usize) -> String {
        let id = self.curve_id();
        let c = self.curve(id);
        let Some(p) = c.points.get(i) else { return String::new() };
        let t = edit::times(c)[i];
        let time = if c.kind == MAP {
            format!("{}%", (t * 100.0).round())
        } else if c.flags & SYNCED != 0 {
            let ticks = (t * 96.0).round() as i64;
            format!("{:03}:{:02}:{:03}", ticks / 384 + 1, ticks / 96 % 4 + 1, ticks % 96)
        } else {
            let ms = (t * 500.0).floor() as i64;
            format!("{}:{:03}", ms / 1000, ms % 1000)
        };
        format!("Point {}/{}:{time}\t{}", i + 1, c.points.len(), Self::level_hint(id, p.level))
    }

    /// A point's level as its target shows it: a bipolar target (and every LFO) signed.
    fn level_hint(id: CurveId, l: f32) -> String {
        let signed = match id {
            CurveId::Part(_, 1) => true,
            CurveId::Part(a, _) => bipolar(a),
            CurveId::Final(_) => false,
        };
        if signed {
            format!("{:+}%", ((l * 2.0 - 1.0) * 100.0).round() as i32)
        } else {
            format!("{}%", (l * 100.0).round() as i32)
        }
    }

    pub fn hit(&self, x: f32, y: f32) -> Option<Target> {
        self.hits.iter().rev().find(|h| h.r.contains(x, y)).map(|h| h.t.clone())
    }

    /// The hint of a target.
    fn hint_for(&self, t: Option<&Target>) -> String {
        match t {
            None | Some(Target::CurveArea) | Some(Target::Cover) => String::new(),
            Some(Target::Knob(f)) | Some(Target::Steps(f)) | Some(Target::Switch(f)) => ctl::hint(&self.st, *f),
            Some(Target::Choice(f, v)) => format!("{}: {}", ctl::name(*f), ctl::text(*f, *v)),
            Some(Target::Pad) => format!("{} / {}", ctl::hint(&self.st, Field::Int(MOD_X)), ctl::text(Field::Int(MOD_Y), ctl::shown(&self.st, Field::Int(MOD_Y)))),
            Some(Target::TargetBox) => format!("Editor target: {}", view::TARGET_NAMES[self.view.target.min(view::TARGETS - 1)]),
            Some(Target::PartBox) => {
                let at = self.view.target.min(view::TARGETS - 1);
                let p = if let CurveId::Part(_, p) = self.curve_id() { p } else { 0 };
                format!("Editor part: {}", view::part_name(at, p))
            }
            Some(Target::Comb) => ctl::hint(&self.st, self.comb_field()),
            Some(Target::EqType(b)) => ctl::hint(&self.st, Field::EqType(*b)),
            Some(Target::Point(i)) => self.point_hint(*i),
            Some(Target::Tension(i)) => {
                let t = self.curve(self.curve_id()).points.get(*i).map_or(0.0, |p| p.tension);
                format!("Point {} to {} tension\t{}%", i, i + 1, (t * 100.0).round())
            }
            Some(Target::CurveOptions) => "Curve options".into(),
            Some(Target::Tool(k)) => view::TOOL_NAMES[*k].into(),
            Some(Target::Key(k)) => view::key_name(*k),
            Some(Target::Header(i)) => view::HEADER_NAMES[(*i).min(2)].into(),
            Some(Target::Logo) => "About".into(),
        }
    }

    fn pad_to(&mut self, x: f32, y: f32) {
        let (vx, vy) = view::pad_values(x, y);
        if (self.st.ints[MOD_X], self.st.ints[MOD_Y]) != (vx, vy) {
            self.st.ints[MOD_X] = vx;
            self.st.ints[MOD_Y] = vy;
            self.commit();
        }
    }

    fn random_preset(&mut self, n: u32) {
        self.random = n;
        self.st = ctl::random_preset(n);
        self.commit();
    }

    pub fn down(&mut self, x: f32, y: f32, btn: Btn, m: Mods, dbl: bool) -> Vec<Act> {
        self.view.mouse = Some((x, y));
        let Some(t) = self.hit(x, y) else { return vec![] };
        let mut acts = vec![Act::Repaint];
        let resettable = match &t {
            Target::Knob(f) | Target::Steps(f) => Some(*f),
            _ => None,
        };
        let id = self.curve_id();
        match (&t, btn) {
            (Target::Cover, _) => self.view.info = false,
            (Target::Logo, Btn::Left) => self.view.info = true,
            (Target::Header(2), Btn::Left) => {
                self.view.no_keys = !self.view.no_keys;
                self.no_keys.store(self.view.no_keys, Ordering::Relaxed);
                acts.push(Act::Resize);
            }
            (Target::Header(0), Btn::Left) => self.random_preset(self.random.wrapping_add(1)),
            (Target::Header(1), Btn::Left) => acts.push(Act::Menu(MenuReq::Options, x, y)),
            (Target::Switch(f), Btn::Left) => self.set(*f, ctl::toggled(*f, ctl::get(&self.st, *f))),
            (Target::Choice(f, v), Btn::Left) => self.set(*f, *v),
            (Target::Pad, Btn::Left) => {
                self.drag = Some(Drag::Pad);
                self.view.pressed = Some(t.clone());
                acts.push(Act::Capture);
                self.pad_to(x, y);
            }
            (Target::TargetBox, Btn::Left | Btn::Right) => acts.push(Act::Menu(MenuReq::Target, x, y)),
            (Target::PartBox, Btn::Left | Btn::Right) => acts.push(Act::Menu(MenuReq::Part, x, y)),
            (Target::Comb, Btn::Left | Btn::Right) => acts.push(Act::Menu(MenuReq::Comb, x, y)),
            (Target::EqType(b), Btn::Left) => self.set(Field::EqType(*b), (ctl::shown(&self.st, Field::EqType(*b)) + 1) % 3),
            (Target::EqType(b), Btn::Right) => acts.push(Act::Menu(MenuReq::EqType(*b), x, y)),
            (Target::Point(i), Btn::Left) => {
                let span = view::curve_span(self.curve(id));
                self.drag = Some(Drag::Point(*i, span));
                self.view.span = Some(span);
                self.view.pressed = Some(t.clone());
                acts.push(Act::Capture);
            }
            (Target::Point(i), Btn::Right) => acts.push(Act::Menu(MenuReq::Point(id, *i), x, y)),
            (Target::Tension(i), Btn::Left) => {
                // Up raises the segment's middle, whichever way a tension bends it.
                let mut c = self.curve(id).clone();
                let mut mid = |t: f32| {
                    edit::set_tension(&mut c, *i, t);
                    edit::segment_level(&c, *i, 0.5)
                };
                let sign = if mid(0.5) >= mid(-0.5) { 1.0 } else { -1.0 };
                self.drag = Some(Drag::Tension(*i, sign));
                self.view.pressed = Some(t.clone());
                acts.push(Act::Lock);
            }
            (Target::Tension(i), Btn::Middle) => self.edit_curve(id, |c| edit::set_tension(c, *i, 0.0)),
            // A right click on empty space adds a point there.
            (Target::CurveArea, Btn::Right) if !self.frozen(id) => {
                let span = view::curve_span(self.curve(id));
                let (t, l) = self.snapped(view::x_to_t(x, span).min(span), view::y_to_level(y), span);
                self.edit_curve(id, |c| {
                    edit::insert_point(c, if c.kind == MAP { t.min(1.0 - 1e-3) } else { t }, l);
                });
            }
            // Step editing: not done.
            (Target::Tool(1), _) => {}
            (Target::Tool(k), Btn::Left) => self.view.tools[*k] = !self.view.tools[*k],
            (Target::CurveOptions, Btn::Left) => acts.push(Act::Menu(MenuReq::Curve(id), x, y)),
            (Target::Key(k), Btn::Left) => {
                self.drag = Some(Drag::Key);
                self.view.key = Some(*k);
                acts.push(Act::Capture);
                acts.push(Act::Note(*k, true));
            }
            (_, Btn::Left) if m.alt && resettable.is_some() => self.reset(resettable.unwrap()),
            (_, Btn::Middle) if resettable.is_some() => self.reset(resettable.unwrap()),
            (Target::Steps(f), Btn::Left) if dbl => acts.push(Act::TypeIn(TypeTarget::Param(*f), x, y)),
            (Target::Knob(f), Btn::Left) => {
                self.drag = Some(Drag::Value(*f, ctl::shown(&self.st, *f) as f32));
                self.view.pressed = Some(t.clone());
                acts.push(Act::Lock);
            }
            (Target::Steps(f), Btn::Left) => {
                self.drag = Some(Drag::Steps(*f, ctl::shown(&self.st, *f) as f32));
                self.view.pressed = Some(t.clone());
                acts.push(Act::Lock);
            }
            (Target::Knob(f) | Target::Steps(f), Btn::Right) => acts.push(Act::Menu(MenuReq::Param(*f), x, y)),
            _ => {}
        }
        self.hint = self.hint_for(Some(&t));
        acts
    }

    /// Mouse movement during a locked drag (logical px since the last event).
    pub fn delta(&mut self, _dx: f32, dy: f32, m: Mods) -> Vec<Act> {
        match self.drag {
            Some(Drag::Value(f, v)) => {
                let (lo, hi) = ctl::range(f);
                let g = if m.ctrl { FINE_DRAG } else { 1.0 };
                let v = (v - dy * g * (hi - lo) as f32 / KNOB_PX).clamp(lo as f32, hi as f32);
                self.drag = Some(Drag::Value(f, v));
                self.set(f, v.round() as i32);
            }
            Some(Drag::Steps(f, v)) => {
                let (lo, hi) = ctl::range(f);
                let v = (v - dy / BOX_PX).clamp(lo as f32, hi as f32);
                self.drag = Some(Drag::Steps(f, v));
                self.set(f, v.round() as i32);
            }
            Some(Drag::Tension(i, sign)) => {
                let id = self.curve_id();
                let g = if m.ctrl { FINE_DRAG } else { 1.0 };
                let t = self.curve(id).points.get(i).map_or(0.0, |p| p.tension) - sign * dy * g / TENSION_PX;
                self.edit_curve(id, |c| edit::set_tension(c, i, t));
            }
            _ => return vec![],
        }
        let t = self.view.pressed.clone();
        self.hint = self.hint_for(t.as_ref());
        vec![Act::Repaint]
    }

    pub fn up(&mut self, _btn: Btn) -> Vec<Act> {
        let Some(d) = self.drag.take() else { return vec![] };
        self.view.pressed = None;
        self.view.span = None;
        let mut acts = vec![Act::Repaint];
        if let (Drag::Key, Some(k)) = (d, self.view.key.take()) {
            acts.push(Act::Note(k, false));
        }
        self.flush();
        let t = self.view.mouse.and_then(|(x, y)| self.hit(x, y));
        self.hint = self.hint_for(t.as_ref());
        acts
    }

    pub fn moved(&mut self, x: f32, y: f32, _m: Mods) -> Vec<Act> {
        self.view.mouse = Some((x, y));
        match self.drag {
            Some(Drag::Pad) => {
                self.pad_to(x, y);
                self.hint = self.hint_for(Some(&Target::Pad));
            }
            Some(Drag::Point(i, span)) => {
                let id = self.curve_id();
                let (t, l) = self.snapped(view::x_to_t(x, span), view::y_to_level(y), span);
                let slide = self.view.tools[3];
                self.edit_curve(id, |c| edit::move_point(c, i, t, l, slide));
                self.hint = self.point_hint(i);
            }
            // The held key follows the cursor along the keyboard.
            Some(Drag::Key) => {
                if let Some(Target::Key(k)) = self.hit(x, y) {
                    if self.view.key != Some(k) {
                        let old = self.view.key.replace(k);
                        self.hint = view::key_name(k);
                        return old.map(|o| Act::Note(o, false)).into_iter().chain([Act::Note(k, true), Act::Repaint]).collect();
                    }
                }
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

    /// Wheel: 1/32 of the range per notch (at least one step); spinners one step; switches and choices step.
    pub fn wheel(&mut self, x: f32, y: f32, notches: f32, _m: Mods) -> Vec<Act> {
        let Some(t) = self.hit(x, y) else { return vec![] };
        let d = notches.signum() as i32;
        match &t {
            Target::Knob(f) => {
                let (lo, hi) = ctl::range(*f);
                let step = (((hi - lo) as f32 * WHEEL_PART).round() as i32).max(1);
                self.set(*f, ctl::shown(&self.st, *f) + d * step);
            }
            Target::Steps(f) => self.set(*f, ctl::shown(&self.st, *f) + d),
            Target::Choice(f, _) => self.set(*f, ctl::shown(&self.st, *f) - d),
            Target::Comb => {
                let f = self.comb_field();
                self.set(f, ctl::shown(&self.st, f) - d);
            }
            Target::EqType(b) => self.set(Field::EqType(*b), ctl::shown(&self.st, Field::EqType(*b)) - d),
            // Both boxes wrap, the wheel going down steps to the next.
            Target::TargetBox => self.view.target = (self.view.target as i32 - d).rem_euclid(view::TARGETS as i32) as usize,
            Target::PartBox => {
                let at = self.view.target.min(view::TARGETS - 1);
                let parts = view::parts_of(at);
                if let CurveId::Part(_, p) = self.curve_id() {
                    let i = parts.iter().position(|&q| q == p).unwrap_or(0) as i32;
                    self.view.part[at] = parts[(i - d).rem_euclid(parts.len() as i32) as usize];
                }
            }
            _ => return vec![],
        }
        self.hint = self.hint_for(Some(&t));
        vec![Act::Repaint]
    }

    pub fn menu(&self, req: &MenuReq) -> MenuDef<Cmd> {
        match req {
            MenuReq::Param(f) => {
                let mut m = MenuDef::new(MenuStyle::Context);
                m.head(&ctl::name(*f), Cmd::None);
                m.add(MenuItem::item("Reset"), Cmd::Reset(*f));
                m.sep(Cmd::None);
                m.add(MenuItem::item("Copy value"), Cmd::CopyValue(*f));
                m.add(MenuItem::item("Paste value").enabled(self.copied.is_some()), Cmd::PasteValue(*f));
                m.add(MenuItem::item("Enter value..."), Cmd::TypeIn(*f));
                m
            }
            MenuReq::Options => self.options_menu(),
            MenuReq::Target => {
                let mut m = MenuDef::new(MenuStyle::Context);
                for (i, name) in view::TARGET_NAMES.into_iter().enumerate() {
                    match i {
                        A_VOLUME => m.head("Articulation", Cmd::None),
                        A_U_PHASE => m.head("Mapping", Cmd::None),
                        _ if i == ARTS => m.head("Timbre equalization", Cmd::None),
                        _ => {}
                    }
                    m.add(MenuItem::item(name).mark(Mark::Radio(self.view.target == i)), Cmd::Target(i));
                }
                m
            }
            MenuReq::Part => {
                let at = self.view.target.min(view::TARGETS - 1);
                let mut m = MenuDef::new(MenuStyle::Context);
                let shown = if let CurveId::Part(_, p) = self.curve_id() { p } else { 0 };
                let parts = view::parts_of(at);
                if parts.is_empty() {
                    m.add(MenuItem::item(view::part_name(at, 0)).mark(Mark::Radio(true)), Cmd::None);
                }
                for &p in parts {
                    m.add(MenuItem::item(view::part_name(at, p)).mark(Mark::Radio(shown == p)), Cmd::Part(p));
                }
                m
            }
            MenuReq::Comb => {
                let f = self.comb_field();
                Self::radios(f, &ctl::COMB_NAMES, ctl::get(&self.st, f))
            }
            MenuReq::EqType(b) => Self::radios(Field::EqType(*b), &ctl::EQ_TYPES, self.st.eq_types[*b]),
            MenuReq::Point(id, i) => self.point_menu(*id, *i),
            MenuReq::Curve(id) => self.curve_menu(*id),
        }
    }

    /// The comb of the final map the articulator editor shows.
    fn comb_field(&self) -> Field {
        Field::Comb(self.view.target.min(view::TARGETS - 1).saturating_sub(ARTS).min(FINALS - 1))
    }

    fn radios(f: Field, names: &[&str], on: i32) -> MenuDef<Cmd> {
        let mut m = MenuDef::new(MenuStyle::Context);
        m.head(&ctl::name(f), Cmd::None);
        for (v, l) in names.iter().enumerate() {
            m.add(MenuItem::item(l).mark(Mark::Radio(on == v as i32)), Cmd::Set(f, v as i32));
        }
        m
    }

    /// The Options menu (`plugins/ogun.md`, "Controls and ranges"); what is not done is disabled.
    fn options_menu(&self) -> MenuDef<Cmd> {
        let check = |m: &mut MenuDef<Cmd>, f: Flag, l: &str| {
            let on = ctl::get(&self.st, Field::Flag(f)) != 0;
            m.add(MenuItem::item(l).mark(Mark::Check(on)), Cmd::Toggle(Field::Flag(f)));
        };
        let off = |m: &mut MenuDef<Cmd>, l: &str| {
            m.add(MenuItem::item(l).enabled(false), Cmd::None);
        };
        let mut m = MenuDef::new(MenuStyle::Context);
        m.add(MenuItem::item("Copy preset"), Cmd::CopyPreset);
        let held = PRESET_CLIP.lock().map(|c| c.is_some()).unwrap_or(false);
        m.add(MenuItem::item("Paste preset").enabled(held), Cmd::PastePreset);
        m.sep(Cmd::None);
        m.add(MenuItem::item("Generate random preset"), Cmd::Random);
        m.add(MenuItem::item("Enter random preset number..."), Cmd::RandomNumber);
        m.sep(Cmd::None);
        let mut fx = MenuDef::new(MenuStyle::Context);
        for (l, on, none) in [("Unison", Fx::Unison(true), Fx::Unison(false)), ("Chorus", Fx::Chorus(true), Fx::Chorus(false)), ("Reverb", Fx::Reverb(true), Fx::Reverb(false))] {
            let mut sub = MenuDef::new(MenuStyle::Context);
            sub.add(MenuItem::item("Default"), Cmd::Effect(on));
            sub.add(MenuItem::item("None"), Cmd::Effect(none));
            fx.submenu(l, sub, Cmd::None);
        }
        m.submenu("Effects", fx, Cmd::None);
        let mut tone = MenuDef::new(MenuStyle::Context);
        tone.head("Portamento", Cmd::None);
        check(&mut tone, Flag::Porta, "Portamento");
        check(&mut tone, Flag::PortaVariable, "Variable time");
        check(&mut tone, Flag::OneVoice, "Limit to 1 voice per key");
        let mut limit = Self::radios(Field::PortaLimit, &ctl::PORTA_LIMITS, i32::from(self.st.porta_limit));
        limit.sep(Cmd::None);
        tone.submenu("Limit", limit, Cmd::None);
        tone.head("Processing quality", Cmd::None);
        for (v, l) in ctl::QUALITY_NAMES.iter().enumerate() {
            tone.add(MenuItem::item(l).mark(Mark::Radio(self.st.quality == v as i32)), Cmd::Set(Field::Quality, v as i32));
        }
        tone.sep(Cmd::None);
        off(&mut tone, "Key");
        off(&mut tone, "Velocity");
        m.submenu("Tone", tone, Cmd::None);
        m.sep(Cmd::None);
        check(&mut m, Flag::LinkRelease, "Link timbre release to volume decay");
        check(&mut m, Flag::NaturalScale, "Natural EQ mapping scale");
        check(&mut m, Flag::NaturalWeight, "Natural EQ mapping weighting");
        check(&mut m, Flag::AutoLoudness, "Auto loudness");
        m.sep(Cmd::None);
        off(&mut m, "Resynthesis");
        m.add(MenuItem::item("About"), Cmd::About);
        m
    }

    /// A point's menu.
    fn point_menu(&self, id: CurveId, i: usize) -> MenuDef<Cmd> {
        let c = self.curve(id);
        let p = c.points.get(i).copied().unwrap_or_default();
        let cmd = |k| Cmd::Curve(id, k);
        let mut m = MenuDef::new(MenuStyle::Context);
        m.head(&format!("Point {}/{}", i + 1, c.points.len()), Cmd::None);
        m.add(MenuItem::item("Delete").enabled(i > 0 && c.points.len() > 2), cmd(CurveCmd::Delete(i)));
        m.head("Mode", Cmd::None);
        for (l, v) in POINT_MODES {
            m.add(MenuItem::item(l).mark(Mark::Radio(p.mode == v)), cmd(CurveCmd::Mode(i, v)));
        }
        if c.kind != MAP {
            m.head("Arpeggiator break", Cmd::None);
            for (v, l) in ["None", "Previous", "Same", "Next"].into_iter().enumerate() {
                m.add(MenuItem::item(l).mark(Mark::Radio(p.step == v as u8)), cmd(CurveCmd::Break(i, v as u8)));
            }
        }
        let marks = [("Decay", c.decay), ("Loop start", c.loop_start), ("Sustain / loop end", c.sustain)];
        if (0..3).any(|k| edit::keeps_marker(c, k)) {
            m.head("ADSR section", Cmd::None);
            for (k, (l, at)) in marks.into_iter().enumerate().filter(|(k, _)| edit::keeps_marker(c, *k)) {
                m.add(MenuItem::item(l).mark(Mark::Check(at == i as i32)), cmd(CurveCmd::Marker(i, k)));
            }
        }
        m.sep(Cmd::None);
        m.add(MenuItem::item("Copy value"), cmd(CurveCmd::CopyLevel(i)));
        m.add(MenuItem::item("Paste value").enabled(self.level_clip.is_some()), cmd(CurveCmd::PasteLevel(i)));
        m.add(MenuItem::item("Enter value..."), cmd(CurveCmd::TypeIn(i)));
        m
    }

    /// The curve view's Options menu.
    fn curve_menu(&self, id: CurveId) -> MenuDef<Cmd> {
        let c = self.curve(id);
        let cmd = |k| Cmd::Curve(id, k);
        let mut m = MenuDef::new(MenuStyle::Context);
        m.head("State", Cmd::None);
        m.add(MenuItem::item("Copy state"), cmd(CurveCmd::CopyState));
        m.add(MenuItem::item("Paste state").enabled(self.curve_clip.as_ref().is_some_and(|s| s.kind == c.kind)), cmd(CurveCmd::PasteState));
        m.head("Tools", Cmd::None);
        m.add(MenuItem::item("Make all loop").enabled(edit::keeps_marker(c, 1)), cmd(CurveCmd::AllLoop));
        m.add(MenuItem::item("Flip vertically"), cmd(CurveCmd::Flip));
        m.add(MenuItem::item("Normalize levels"), cmd(CurveCmd::Normalize));
        m.add(MenuItem::item("Smooth up abrupt changes"), cmd(CurveCmd::SmoothAbrupt));
        m.add(MenuItem::item("Turn all points smooth"), cmd(CurveCmd::AllSmooth));
        m
    }

    fn curve_cmd(&mut self, id: CurveId, k: CurveCmd) {
        match k {
            CurveCmd::CopyLevel(i) => self.level_clip = self.curve(id).points.get(i).map(|p| p.level),
            CurveCmd::CopyState => self.curve_clip = Some(self.curve(id).clone()),
            CurveCmd::TypeIn(_) => {}
            k => {
                let (level, clip) = (self.level_clip, self.curve_clip.clone());
                self.edit_curve(id, |c| match k {
                    CurveCmd::Delete(i) if i < c.points.len() => {
                        edit::delete_point(c, i);
                    }
                    CurveCmd::Mode(i, v) if i < c.points.len() => edit::set_mode(c, i, v),
                    CurveCmd::Break(i, v) if i < c.points.len() => edit::set_break(c, i, v),
                    CurveCmd::Marker(i, m) if i < c.points.len() => edit::toggle_marker(c, i, m),
                    CurveCmd::PasteLevel(i) if i < c.points.len() => {
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
                    CurveCmd::Normalize => edit::normalize(c),
                    CurveCmd::AllSmooth => edit::all_smooth(c),
                    CurveCmd::SmoothAbrupt => edit::smooth_abrupt(c),
                    _ => {}
                });
            }
        }
    }

    /// An effect preset of the Options menu: the default state's settings of that effect, or the effect off.
    fn effect(&mut self, fx: Fx) {
        let d = State::default();
        match fx {
            Fx::Unison(true) => self.st.ints[U_ORD..=U_PHS].copy_from_slice(&d.ints[U_ORD..=U_PHS]),
            Fx::Unison(false) => self.st.ints[U_ORD] = 1,
            Fx::Chorus(true) => self.st.ints[crate::state::CHORUS..crate::state::CHORUS + 7].copy_from_slice(&d.ints[crate::state::CHORUS..crate::state::CHORUS + 7]),
            Fx::Chorus(false) => self.st.ints[crate::state::CHORUS] = 0,
            Fx::Reverb(on) => {
                if on {
                    self.st.ints[crate::state::REVERB..crate::state::REVERB + 9].copy_from_slice(&d.ints[crate::state::REVERB..crate::state::REVERB + 9]);
                }
                self.st.reverb_on = on;
            }
        }
        self.commit();
    }

    pub fn pick(&mut self, _req: &MenuReq, cmd: Cmd, x: f32, y: f32) -> Vec<Act> {
        match cmd {
            Cmd::None => {}
            Cmd::Reset(f) => self.reset(f),
            Cmd::CopyValue(f) => self.copied = Some(ctl::get(&self.st, f)),
            Cmd::PasteValue(f) => {
                if let Some(v) = self.copied {
                    self.set(f, v);
                }
            }
            Cmd::TypeIn(f) => return vec![Act::TypeIn(TypeTarget::Param(f), x, y)],
            Cmd::Set(f, v) => self.set(f, v),
            Cmd::Toggle(f) => self.set(f, ctl::toggled(f, ctl::get(&self.st, f))),
            Cmd::Target(t) => self.view.target = t.min(view::TARGETS - 1),
            Cmd::Part(p) => self.view.part[self.view.target.min(view::TARGETS - 1)] = p,
            Cmd::CopyPreset => {
                if let Ok(mut c) = PRESET_CLIP.lock() {
                    *c = Some(self.st.to_bytes());
                }
            }
            Cmd::PastePreset => {
                let held = PRESET_CLIP.lock().ok().and_then(|c| c.clone());
                if let Some(st) = held.and_then(|b| State::from_bytes(&b)) {
                    self.st = st;
                    self.commit();
                }
            }
            Cmd::Random => self.random_preset(self.random.wrapping_add(1)),
            Cmd::RandomNumber => return vec![Act::TypeIn(TypeTarget::Random, x, y)],
            Cmd::Effect(fx) => self.effect(fx),
            Cmd::About => self.view.info = true,
            Cmd::Curve(id, CurveCmd::TypeIn(i)) => return vec![Act::TypeIn(TypeTarget::Point(id, i), x, y)],
            Cmd::Curve(id, k) => self.curve_cmd(id, k),
        }
        vec![Act::Repaint]
    }

    pub fn value_text(&self, t: &TypeTarget) -> String {
        match *t {
            TypeTarget::Param(f) => ctl::text(f, ctl::shown(&self.st, f)),
            TypeTarget::Point(id, i) => self.curve(id).points.get(i).map(|p| edit::level_text(p.level)).unwrap_or_default(),
            TypeTarget::Random => self.random.to_string(),
        }
    }

    /// Applies a typed value; text that does not read leaves it as it was, and so does the text the box opened on.
    pub fn typed(&mut self, t: &TypeTarget, text: &str) {
        if text.trim() == self.value_text(t) {
            return;
        }
        match *t {
            TypeTarget::Param(f) => {
                if let Some(v) = ctl::parse(f, text, ctl::shown(&self.st, f)) {
                    self.set(f, v);
                }
            }
            TypeTarget::Point(id, i) => {
                if let Some(l) = edit::parse_level(text) {
                    self.edit_curve(id, |c| {
                        if i < c.points.len() {
                            let t = edit::times(c)[i];
                            edit::move_point(c, i, t, l, false);
                        }
                    });
                }
            }
            TypeTarget::Random => {
                if let Ok(n) = text.trim().parse() {
                    self.random_preset(n);
                }
            }
        }
    }

    /// Paints the editor; `hint` is drawn when the host does not show it.
    pub fn paint(&mut self, c: &mut os_ui::Canvas) {
        let hint = if self.hosted { String::new() } else { self.hint.clone() };
        let model = view::Model { st: &self.st, version: env!("CARGO_PKG_VERSION") };
        self.hits = view::paint(c, &model, &self.view, &hint);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The window

#[cfg(windows)]
mod win32 {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
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
    use crate::SmithParams;

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
        ctx: Arc<dyn GuiContext>,
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

    pub fn open(parent: *mut c_void, scale: f32, params: Arc<SmithParams>, no_keys: Arc<AtomicBool>, ctx: Arc<dyn GuiContext>, slot: Arc<AtomicIsize>) -> Handle {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let logic = Logic::new(params, no_keys);
        let px = |v: u32| (v as f32 * scale).round() as u32;
        let size = (px(view::W), px(logic.height()));
        unsafe {
            let class = pw::register("SmithView", Some(wndproc));
            let win = Box::new(Win {
                hwnd: std::ptr::null_mut(),
                canvas: Canvas::new(size.0, size.1, scale).expect("canvas"),
                buf: vec![0; (size.0 * size.1) as usize],
                scale,
                size,
                logic,
                palette: os_plugkit::palette(),
                ctx,
                last_hint: String::new(),
                lock: CursorLock::default(),
                tracking: false,
                slot: slot.clone(),
            });
            let title = pw::wide("Smith");
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
                    Act::Note(k, on) => {
                        let ans = os_hostlink::to_host(self.hwnd as isize, os_hostlink::Cmd::Note, os_hostlink::pack_note(k, 100, on));
                        if ans != 1 {
                            if let Ok(mut q) = self.logic.params.previews.lock() {
                                q.push((k, on));
                            }
                        }
                    }
                    Act::Resize => {
                        self.resize();
                        repaint = true;
                    }
                }
            }
            if repaint {
                self.invalidate();
            }
        }

        /// The keyboard was shown or hidden: the window, the canvas and the host follow.
        fn resize(&mut self) {
            let h = (self.logic.height() as f32 * self.scale).round() as u32;
            self.size.1 = h;
            self.canvas.resize(self.size.0, h, self.scale);
            unsafe { SetWindowPos(self.hwnd, std::ptr::null_mut(), 0, 0, self.size.0 as i32, h as i32, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE) };
            if self.logic.hosted {
                os_hostlink::to_host(self.hwnd as isize, os_hostlink::Cmd::Changed, 0);
            } else {
                self.ctx.request_resize();
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
                Cmd::Metrics => os_hostlink::pack_size(view::W, self.logic.height()),
                Cmd::Hint => {
                    if lp != 0 {
                        // SAFETY: the host passes a `HintBuf` of its own process (spec: host-link.md).
                        unsafe { &mut *(lp as *mut os_hostlink::HintBuf) }.set(&self.logic.hint);
                    }
                    1
                }
                Cmd::Changed | Cmd::Note | Cmd::Automate | Cmd::Audio | Cmd::Resizable | Cmd::SetSize | Cmd::Notes => 0,
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
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    use nih_plug::prelude::GuiContext;
    use os_hostlink::{Cmd, PageIcon, Peer};
    use os_plugkit::gui::{self as px, Button, Input, Window};
    use os_theme::Palette;
    use os_ui::Canvas;

    use super::{Act, Btn, Logic, Mods};
    use crate::view;
    use crate::SmithParams;

    /// The editor on its GUI thread.
    struct Ed {
        win: Window,
        canvas: Canvas,
        buf: Vec<u32>,
        scale: f32,
        size: (u32, u32),
        logic: Logic,
        palette: Palette,
        ctx: Arc<dyn GuiContext>,
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

    pub fn open(parent: px::Parent, scale: f32, params: Arc<SmithParams>, no_keys: Arc<AtomicBool>, ctx: Arc<dyn GuiContext>, slot: &px::Slot) -> Handle {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let logic = Logic::new(params, no_keys);
        let h = logic.height();
        let size = ((view::W as f32 * scale).round() as u32, (h as f32 * scale).round() as u32);
        let wake = slot.clone();
        let link = Peer::register(parent as u64, vec![PageIcon::Plugin], (view::W, h), move || px::notify(&wake));
        let peer = link.clone();
        let thread = px::spawn(parent, size, slot, move |win| {
            let Ok(canvas) = Canvas::new(size.0, size.1, scale) else { return };
            let ed = Ed { win, canvas, buf: Vec::new(), scale, size, logic, palette: os_plugkit::palette(), ctx, last_hint: String::new(), dirty: true, link: peer };
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
                    Act::Note(k, on) => {
                        if self.link.to_host(Cmd::Note, os_hostlink::pack_note(k, 100, on)) != 1 {
                            if let Ok(mut q) = self.logic.params.previews.lock() {
                                q.push((k, on));
                            }
                        }
                    }
                    Act::Resize => {
                        let h = (self.logic.height() as f32 * self.scale).round() as u32;
                        self.size.1 = h;
                        self.canvas.resize(self.size.0, h, self.scale);
                        self.win.resize(self.size.0, h);
                        self.ctx.request_resize();
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
        let mut l = Logic::new(Arc::new(SmithParams::default()), Arc::new(AtomicBool::new(false)));
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

    fn click(l: &mut Logic, t: &Target) -> Vec<Act> {
        let (x, y) = centre(l, t);
        let acts = l.down(x, y, Btn::Left, Mods::default(), false);
        l.up(Btn::Left);
        paint(l);
        acts
    }

    /// Everything the editor shows lies inside it.
    #[test]
    fn controls_lie_inside_the_editor() {
        let l = logic();
        for h in &l.hits {
            assert!(h.r.x >= 0.0 && h.r.y >= 0.0 && h.r.right() <= view::W as f32 && h.r.bottom() <= view::H as f32, "{:?}", h);
        }
    }

    /// No two controls share a place (the keys and the curve's points lie over their grounds on purpose).
    #[test]
    fn controls_do_not_overlap() {
        let mut l = logic();
        for target in 0..view::TARGETS {
            l.view.target = target;
            paint(&mut l);
            let small: Vec<_> = l.hits.iter().filter(|h| !matches!(h.t, Target::Key(_) | Target::CurveArea | Target::Point(_) | Target::Tension(_))).collect();
            for (i, a) in small.iter().enumerate() {
                for b in &small[..i] {
                    let apart = a.r.right() <= b.r.x || b.r.right() <= a.r.x || a.r.bottom() <= b.r.y || b.r.bottom() <= a.r.y;
                    assert!(apart, "{:?} {:?} at target {target}", a, b);
                }
            }
        }
    }

    /// A knob's drag moves its setting, the edit is saved and goes to the audio thread once the drag ends.
    #[test]
    fn a_knob_drag_edits_the_state() {
        let mut l = logic();
        let f = Field::Int(CUT);
        let t = Target::Knob(f);
        let (x, y) = centre(&l, &t);
        assert!(l.down(x, y, Btn::Left, Mods::default(), false).contains(&Act::Lock));
        let before = l.st.ints[CUT];
        l.delta(0.0, 25.0, Mods::default());
        assert_eq!(l.st.ints[CUT], before - 26);
        assert!(l.params.edit.lock().unwrap().is_none(), "a drag's patch waits for the tick");
        l.poll();
        assert!(l.params.edit.lock().unwrap().is_some());
        l.delta(0.0, 25.0, Mods::default());
        l.up(Btn::Left);
        assert_eq!(saved(&l).ints[CUT], l.st.ints[CUT]);
        assert_eq!(l.params.edit.lock().unwrap().as_ref().unwrap().1.st.ints[CUT], l.st.ints[CUT]);
    }

    #[test]
    fn alt_click_and_middle_click_reset() {
        let mut l = logic();
        let t = Target::Knob(Field::Int(LEVEL));
        l.st.ints[LEVEL] = 7;
        let (x, y) = centre(&l, &t);
        l.down(x, y, Btn::Left, Mods { alt: true, ..Default::default() }, false);
        assert_eq!(l.st.ints[LEVEL], State::default().ints[LEVEL]);
        l.st.ints[LEVEL] = 7;
        l.down(x, y, Btn::Middle, Mods::default(), false);
        assert_eq!(l.st.ints[LEVEL], State::default().ints[LEVEL]);
    }

    #[test]
    fn switches_and_choices_set_their_values() {
        let mut l = logic();
        click(&mut l, &Target::Switch(Field::Int(FLAT)));
        assert_eq!(saved(&l).ints[FLAT], 0);
        click(&mut l, &Target::Switch(Field::Int(FLAT)));
        assert_eq!(saved(&l).ints[FLAT], -1);
        click(&mut l, &Target::Choice(Field::Int(F_TYPE), 3));
        assert_eq!(saved(&l).ints[F_TYPE], 3);
        let on = l.st.delay_on;
        click(&mut l, &Target::Switch(Field::Flag(Flag::DelayOn)));
        assert_eq!(saved(&l).delay_on, !on);
        let ty = l.st.eq_types[1];
        click(&mut l, &Target::EqType(1));
        assert_eq!(saved(&l).eq_types[1], (ty + 1) % 3);
    }

    #[test]
    fn the_pad_sets_both_mods() {
        let mut l = logic();
        let (x, y) = view::pad_point(384, 384);
        l.down(x, y, Btn::Left, Mods::default(), false);
        l.moved(x, y, Mods::default());
        l.up(Btn::Left);
        assert_eq!((saved(&l).ints[MOD_X], saved(&l).ints[MOD_Y]), (384, 384));
    }

    /// The parts follow the target: an articulation target has all six, a mapping target the maps, a final map one
    /// curve. The part box opens a menu of them and the wheel steps them, wrapping.
    #[test]
    fn parts_follow_the_target() {
        let mut l = logic();
        assert_eq!(l.curve_id(), CurveId::Final(M_IMPULSE), "as Ogun opens");
        for (t, n) in [(A_VOLUME, 6), (A_CUT, 6), (A_U_PITCH, 6), (A_U_PHASE, 4), (A_SHIFT, 4), (ARTS + M_DECAY, 0)] {
            l.pick(&MenuReq::Target, Cmd::Target(t), 0.0, 0.0);
            assert_eq!(view::parts_of(t).len(), n, "{t}");
            assert_eq!(l.menu(&MenuReq::Part).items.len(), n.max(1), "{t}");
        }
        assert_eq!(l.curve_id(), CurveId::Final(M_DECAY));
        l.pick(&MenuReq::Target, Cmd::Target(A_VOLUME), 0.0, 0.0);
        assert_eq!(l.curve_id(), CurveId::Part(A_VOLUME, 0));
        l.pick(&MenuReq::Target, Cmd::Target(A_PRE), 0.0, 0.0);
        assert_eq!(l.curve_id(), CurveId::Part(A_PRE, 2));
        paint(&mut l);
        assert!(click(&mut l, &Target::PartBox).contains(&Act::Menu(MenuReq::Part, view::PART_BOX.cx(), view::PART_BOX.cy())));
        l.pick(&MenuReq::Part, Cmd::Part(5), 0.0, 0.0);
        assert_eq!(l.curve_id(), CurveId::Part(A_PRE, 5));
        let (x, y) = (view::PART_BOX.cx(), view::PART_BOX.cy());
        l.wheel(x, y, -1.0, Mods::default());
        assert_eq!(l.curve_id(), CurveId::Part(A_PRE, 2), "the wheel wraps round to the first map");
        l.wheel(x, y, 1.0, Mods::default());
        assert_eq!(l.curve_id(), CurveId::Part(A_PRE, 5));
        let (x, y) = (view::TARGET_BOX.cx(), view::TARGET_BOX.cy());
        l.view.target = view::TARGETS - 1;
        l.wheel(x, y, -1.0, Mods::default());
        assert_eq!(l.view.target, 0, "the target box wraps too");
    }

    /// The third round button hides the keyboard, and the editor's size follows.
    #[test]
    fn the_keyboard_hides() {
        let mut l = logic();
        assert_eq!(l.height(), view::H);
        assert!(click(&mut l, &Target::Header(2)).contains(&Act::Resize));
        assert_eq!(l.height(), view::H_NO_KEYS);
        assert!(l.no_keys.load(Ordering::Relaxed));
        assert!(!l.hits.iter().any(|h| matches!(h.t, Target::Key(_))));
        for h in &l.hits {
            assert!(h.r.bottom() <= view::H_NO_KEYS as f32, "{:?}", h);
        }
        assert!(click(&mut l, &Target::Header(2)).contains(&Act::Resize));
        assert_eq!(l.height(), view::H);
    }

    /// A curve's point moves by its drag, a right click adds one, the menu deletes it again.
    #[test]
    fn the_curve_view_edits_points() {
        let mut l = logic();
        l.pick(&MenuReq::Target, Cmd::Target(ARTS + M_RELEASE), 0.0, 0.0);
        paint(&mut l);
        // The point is dragged alone.
        l.view.tools[3] = false;
        let id = l.curve_id();
        let n = l.curve(id).points.len();
        let r = view::curve_inner();
        let (x, y) = (r.cx(), r.y + r.h * 0.25);
        l.down(x, y, Btn::Right, Mods::default(), false);
        paint(&mut l);
        assert_eq!(saved(&l).finals[M_RELEASE].points.len(), n + 1);
        let i = l.hits.iter().find_map(|h| if let Target::Point(i) = h.t { (h.r.contains(x, y)).then_some(i) } else { None }).expect("a point there");
        l.down(x, y, Btn::Left, Mods::default(), false);
        l.moved(x, r.y + r.h * 0.5, Mods::default());
        l.up(Btn::Left);
        assert!((saved(&l).finals[M_RELEASE].points[i].level - 0.5).abs() < 0.01);
        l.pick(&MenuReq::Point(id, i), Cmd::Curve(id, CurveCmd::Delete(i)), 0.0, 0.0);
        assert_eq!(saved(&l).finals[M_RELEASE].points.len(), n);
    }

    /// A map of many points is drawn without handles and does not edit.
    #[test]
    fn a_resynthesized_map_is_not_edited_by_hand() {
        let mut l = logic();
        let n = view::MANY_POINTS + 10;
        let mut c = flat_map(0.5);
        c.points = (0..n).map(|k| os_dsp::curve::Point { dist: if k == 0 { 0.0 } else { 1.0 / (n - 1) as f32 }, level: 0.25, ..Default::default() }).collect();
        c.prepare();
        l.st.finals[M_DECAY] = c;
        l.view.target = ARTS + M_DECAY;
        paint(&mut l);
        assert!(!l.hits.iter().any(|h| matches!(h.t, Target::Point(_) | Target::Tension(_))));
        let r = view::curve_inner();
        l.down(r.cx(), r.cy(), Btn::Right, Mods::default(), false);
        assert_eq!(l.st.finals[M_DECAY].points.len(), n);
    }

    #[test]
    fn the_keyboard_plays_and_follows_the_cursor() {
        let mut l = logic();
        let t = Target::Key(60);
        let (x, y) = centre(&l, &t);
        assert!(l.down(x, y, Btn::Left, Mods::default(), false).contains(&Act::Note(60, true)));
        let (x2, y2) = centre(&l, &Target::Key(62));
        let acts = l.moved(x2, y2, Mods::default());
        assert!(acts.contains(&Act::Note(60, false)) && acts.contains(&Act::Note(62, true)));
        assert!(l.up(Btn::Left).contains(&Act::Note(62, false)));
    }

    /// A state replaced from outside shows on the next tick; an edit of the old one does not land.
    #[test]
    fn a_replaced_state_is_read_again() {
        let mut l = logic();
        let mut st = State::default();
        st.ints[CUT] = 77;
        l.params.set_state(st.to_bytes());
        l.st.ints[CUT] = 5;
        l.pending = true;
        l.flush();
        assert_eq!(l.st.ints[CUT], 77);
        assert_eq!(saved(&l).ints[CUT], 77);
    }

    /// Each final map has its own comb, set from the box the editor shows on it.
    #[test]
    fn each_final_map_has_its_own_comb() {
        let mut l = logic();
        for m in 0..FINALS {
            l.view.target = ARTS + m;
            let f = l.comb_field();
            assert_eq!(f, Field::Comb(m));
            l.set(f, m as i32 + 1);
        }
        assert_eq!(l.st.combs, [1, 2, 3, 4]);
    }

    /// Every menu builds; the preset clipboard carries a preset; a typed value lands.
    #[test]
    fn menus_and_type_in() {
        let mut l = logic();
        for req in [MenuReq::Options, MenuReq::Target, MenuReq::Part, MenuReq::Comb, MenuReq::EqType(0), MenuReq::Param(Field::Int(LEVEL)), MenuReq::Curve(CurveId::Final(0)), MenuReq::Point(CurveId::Final(0), 0)] {
            l.menu(&req);
        }
        l.st.ints[RICH] = 13;
        l.pick(&MenuReq::Options, Cmd::CopyPreset, 0.0, 0.0);
        l.st.ints[RICH] = 9;
        l.pick(&MenuReq::Options, Cmd::PastePreset, 0.0, 0.0);
        assert_eq!(saved(&l).ints[RICH], 13);
        l.typed(&TypeTarget::Param(Field::Int(LEVEL)), "50%");
        assert_eq!(saved(&l).ints[LEVEL], 64);
        l.typed(&TypeTarget::Random, "1234");
        assert_eq!(saved(&l).comment, "Random preset 1234");
        let a = saved(&l);
        l.pick(&MenuReq::Options, Cmd::Random, 0.0, 0.0);
        l.typed(&TypeTarget::Random, "1234");
        assert_eq!(saved(&l).to_bytes(), a.to_bytes());
        l.pick(&MenuReq::Options, Cmd::Effect(Fx::Chorus(false)), 0.0, 0.0);
        assert_eq!(saved(&l).ints[crate::state::CHORUS], 0);
        l.pick(&MenuReq::Options, Cmd::Effect(Fx::Reverb(false)), 0.0, 0.0);
        assert!(!saved(&l).reverb_on);
    }

    /// The hint names what the mouse is over; About shows and goes.
    #[test]
    fn hints_and_about() {
        let mut l = logic();
        let (x, y) = centre(&l, &Target::Knob(Field::Int(LEVEL)));
        l.moved(x, y, Mods::default());
        assert_eq!(l.hint, "Level: 125%");
        click(&mut l, &Target::Logo);
        assert!(l.view.info);
        click(&mut l, &Target::Cover);
        assert!(!l.view.info);
    }

    /// Look check: `OS_SHOT_DIR=dir cargo test -p smith editor_shots -- --ignored`.
    #[test]
    #[ignore]
    fn editor_shots() {
        let Some(dir) = std::env::var_os("OS_SHOT_DIR") else { return };
        let dir = std::path::PathBuf::from(dir);
        let mut l = logic();
        // 2x to look at, 1x to lay over the spec's form px.
        let shot = |l: &mut Logic, name: &str| {
            for (k, s) in [(2, "2x"), (1, "1x")] {
                let mut c = os_ui::Canvas::new(view::W * k, l.height() * k, k as f32).unwrap();
                l.paint(&mut c);
                c.pixmap.save_png(dir.join(format!("{name}-{s}.png"))).unwrap();
            }
        };
        shot(&mut l, "smith-default");
        l.view.target = A_VOLUME;
        l.view.part[A_VOLUME] = 0;
        shot(&mut l, "smith-envelope");
        l.view.part[A_VOLUME] = 1;
        shot(&mut l, "smith-lfo");
        for (p, name) in [(2, "keyboard"), (3, "velocity"), (4, "mod-x")] {
            l.view.part[A_VOLUME] = p;
            shot(&mut l, &format!("smith-{name}"));
        }
        l.view.target = ARTS + M_DECAY;
        shot(&mut l, "smith-map");
        l.view.no_keys = true;
        shot(&mut l, "smith-no-keys");
        l.view.no_keys = false;
        l.view.info = true;
        shot(&mut l, "smith-about");
    }
}
