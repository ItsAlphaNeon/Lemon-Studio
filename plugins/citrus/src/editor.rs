// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The plugin editor: a child of the host's window (Win32, X11 on Linux, an NSView on macOS) that paints `view` with
//! the os-ui canvas, and the editor logic (platform independent). The editor keeps the patch it shows as a state of
//! its own; every edit writes it back as the plugin's saved state and hands the audio thread the rebuilt patch
//! (`CitrusParams::edited`). A state replaced from outside (a preset, the host) is read again on the next tick.
//!
// spec: plugins/citrus.md §6, plugins/sytrus-editor.md (§1 .. §2, §6 .. §8), plugins/host-link.md

use std::any::Any;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::Arc;

use nih_plug::prelude::*;
use os_plugkit::MenuDef;
use os_ui::kit::menu::{Mark, MenuItem, MenuStyle};

use crate::ctl::{self, Field};
use crate::curve::{Curve, Point, MAP};
use crate::edit;
use crate::patch::{self, Patch};
use crate::state::*;
use crate::view::{self, Cover, Target, ViewState};
use crate::CitrusParams;

pub struct CitrusEditor {
    pub params: Arc<CitrusParams>,
    /// DPI scale as f32 bits.
    pub scale: AtomicU32,
    /// The testing keyboard is hidden (kept while the editor is closed).
    pub no_keys: Arc<AtomicBool>,
    /// The open editor window (0 = closed).
    #[cfg_attr(not(windows), allow(dead_code))]
    hwnd: Arc<AtomicIsize>,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    slot: os_plugkit::gui::Slot,
}

impl CitrusEditor {
    pub fn new(params: Arc<CitrusParams>) -> Self {
        CitrusEditor {
            params,
            scale: AtomicU32::new(1f32.to_bits()),
            no_keys: Arc::new(AtomicBool::new(false)),
            hwnd: Arc::new(AtomicIsize::new(0)),
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            slot: Default::default(),
        }
    }
}

impl Editor for CitrusEditor {
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

    // Mod X / Mod Y have no control of their own; the state's settings are not parameters.
    fn param_value_changed(&self, _id: &str, _normalized_value: f32) {}
    fn param_modulation_changed(&self, _id: &str, _modulation_offset: f32) {}
    fn param_values_changed(&self) {}
}

// ---------------------------------------------------------------------------------------------------------------
// Laws (plugins/sytrus-editor.md §8, plugins/citrus.md §6)

/// Pixels of drag for a knob's, slider's or cell's whole range.
pub const KNOB_PX: f32 = 250.0;
/// Ctrl: this much finer.
const FINE: f32 = 1.0 / 20.0;
/// The wheel moves this part of the range per notch (at least one step).
const WHEEL_PART: f32 = 1.0 / 32.0;
/// Pixels of drag per step of a box.
pub const BOX_PX: f32 = 8.0;
/// Pixels of drag per step of a numeric display's digit.
pub const DIGIT_PX: f32 = 10.0;
/// Points of the operator page's drawn wave.
const WAVE_POINTS: usize = 160;
/// Pixels of drag per unit of a segment's tension (-1 .. 1).
pub const TENSION_PX: f32 = 100.0;
/// The snap tool's grid: the view's span in 32 steps, levels in 16.
const SNAP_T: f64 = 32.0;
const SNAP_L: f32 = 16.0;
/// Harmonics the wheel scrolls per notch.
const HARM_WHEEL: i32 = 8;

/// The point menu's modes in FL's order, with their codes (plugins/image-line-curves.md).
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

// ---------------------------------------------------------------------------------------------------------------
// Menus

#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    None,
    Reset(Field),
    CopyValue(Field),
    PasteValue(Field),
    TypeIn(Field),
    /// Sets a setting (the Options menu's items).
    Set(Field, i32),
    /// A curve edit (the point menu, the curve view's Options menu).
    Curve(CurveId, CurveCmd),
    /// A harmonics tool on operator `m`.
    Harm(usize, HarmCmd),
    /// The Options menu's oscillator items.
    Osc(OscCmd),
    /// Unison › Default.
    UnisonDefault,
}

/// The Options menu's oscillator items, on operator `m` (the page's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OscCmd {
    Copy(usize),
    Paste(usize),
    Reset(usize),
    Randomize(usize),
    Morph,
}

/// The curve the curve view edits: an articulator's part (module, articulator, part) or a filter's shaper.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CurveId {
    Part(usize, usize, usize),
    Shaper(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CurveCmd {
    Delete(usize),
    Mode(usize, u8),
    Break(usize, u8),
    /// Marker 0 decay, 1 loop start, 2 sustain / loop end at the point (or off).
    Marker(usize, usize),
    CopyLevel(usize),
    PasteLevel(usize),
    CopyState,
    PasteState,
    AllLoop,
    Flip,
    Normalize,
    AllSmooth,
    /// Type in value for point `i`.
    TypeIn(usize),
    /// Points selection.
    SelectAll,
    SelectNone,
    Invert,
    Cut,
    Copy,
    PasteInsert,
    PasteReplace,
    Duplicate,
    DeleteSel,
    /// Scale levels... (opens the dialog), Smooth up abrupt changes.
    Scale,
    SmoothAbrupt,
}

impl CurveCmd {
    /// The point the command is about.
    fn point(self) -> Option<usize> {
        match self {
            CurveCmd::Delete(i) | CurveCmd::Mode(i, _) | CurveCmd::Break(i, _) | CurveCmd::Marker(i, _) | CurveCmd::CopyLevel(i) | CurveCmd::PasteLevel(i) | CurveCmd::TypeIn(i) => {
                Some(i)
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HarmCmd {
    Convert,
    Transpose,
    /// Shift right (true) or left.
    Shift(bool),
    Gibbs,
    ResetPhases,
    RandomPhases,
    Copy,
    Paste,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuReq {
    Param(Field),
    Options,
    /// Operator `m`'s waveform.
    Wave(usize),
    /// A point of the curve view.
    Point(CurveId, usize),
    /// The curve view's Options arrow, the harmonics editor's (operator `m`).
    Curve(CurveId),
    Harm(usize),
}

// ---------------------------------------------------------------------------------------------------------------
// Editor logic

/// Something the window has to do after an event.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Repaint,
    /// Hide the cursor and hold it in place (knob, slider, cell and box drags).
    Lock,
    /// Keep the mouse while the button is down (the X / Y pad, the keyboard).
    Capture,
    Menu(MenuReq, f32, f32),
    TypeIn(TypeTarget, f32, f32),
    /// A testing keyboard note (key, on).
    Note(u8, bool),
    /// The surface's height changed (the keyboard was shown or hidden).
    Resize,
}

/// What a type-in box sets: a setting, or a point's level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TypeTarget {
    Field(Field),
    Point(CurveId, usize),
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
    /// A knob, slider or cell: `v` = the unrounded value it has reached (steps would swallow small moves).
    Value(Field, f32),
    /// A box: `v` likewise, in steps.
    Steps(Field, f32),
    /// A digit of a numeric display: its step and the pixels moved since the last step.
    Digit(Field, i32, f32),
    /// A point of the curve view (absolute), with the view's time span when the drag began.
    Point(usize, f64),
    /// The selected points, by the one grabbed (`orig` holds the curve as it was): where the drag began, the span.
    Group(usize, f32, f32, f64),
    /// A rubber band from (x, y).
    Band(f32, f32),
    /// A Level scaling control.
    Scale(usize),
    /// A segment's tension handle: which way up bends it (1 or -1).
    Tension(usize, f32),
    /// Painting harmonics' levels: the last harmonic painted and its level.
    Harm(usize, f32),
    HarmScroll,
    Pad,
    Key,
}

pub struct Logic {
    pub params: Arc<CitrusParams>,
    /// The patch shown and edited.
    pub st: State,
    /// The unpacked stream `st` was read from (what an edited state carries over).
    body: Option<Vec<u8>>,
    /// The state version `st` was read at.
    version: u64,
    used: [bool; 9],
    pub view: ViewState,
    pub hits: Vec<view::Hit>,
    drag: Option<Drag>,
    /// Hint text (a tab before the right-aligned part).
    pub hint: String,
    copied: Option<i32>,
    pub hosted: bool,
    no_keys: Arc<AtomicBool>,
    /// The operator page's wave and what it was drawn from (the block, band-limit smoothing).
    wave: Option<(OpBlock, bool, Vec<f32>)>,
    /// The operators' tables of the last edit, reused by the next one and by the waveform preview.
    tables: patch::Tables,
    /// An edit whose patch the audio thread has not got yet (see [`Logic::flush`]).
    pending: bool,
    /// The editor's clipboards: a point's level, a curve, an operator's harmonics.
    level_clip: Option<f32>,
    curve_clip: Option<Curve>,
    harm_clip: Option<(Vec<f32>, Vec<f32>)>,
    /// Randomize phases' seed (moved on by each use).
    seed: u32,
    /// The curve view's selection and the curve it was made on.
    sel: Vec<bool>,
    sel_of: Option<CurveId>,
    /// The points Copy and Cut took.
    points_clip: Option<Vec<Point>>,
    /// A group drag's curve as it was when the drag began.
    orig: Option<Curve>,
    /// The Level scaling dialog's curve and that curve as it was when the dialog opened.
    scaling: Option<(CurveId, Curve)>,
    /// Copy oscillator settings' operator block.
    op_clip: Option<OpBlock>,
}

impl Logic {
    pub fn new(params: Arc<CitrusParams>, no_keys: Arc<AtomicBool>) -> Self {
        // FL opens each page on its VOL target.
        let view = ViewState { no_keys: no_keys.load(Ordering::Relaxed), target: [1; view::PAGES], ..ViewState::default() };
        let mut l = Logic {
            params,
            st: crate::preset::default_patch(),
            body: None,
            version: 0,
            used: [false; 9],
            view,
            hits: Vec::new(),
            drag: None,
            hint: String::new(),
            copied: None,
            hosted: false,
            no_keys,
            wave: None,
            tables: patch::Tables::default(),
            pending: false,
            level_clip: None,
            curve_clip: None,
            harm_clip: None,
            seed: 1,
            sel: Vec::new(),
            sel_of: None,
            points_clip: None,
            orig: None,
            scaling: None,
            op_clip: None,
        };
        l.reload();
        l
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Reads the plugin's state again: the default patch for none, or for one that does not read.
    fn reload(&mut self) {
        self.version = self.params.state_version.load(Ordering::Acquire);
        let bytes = self.params.state.read().map(|s| s.0.clone()).unwrap_or_default();
        let read = (!bytes.is_empty()).then(|| State::from_bytes(&bytes).zip(State::unpack(&bytes))).flatten();
        (self.st, self.body) = match read {
            Some((st, (_, body))) => (st, Some(body)),
            None => (crate::preset::default_patch(), None),
        };
        self.used = patch::usage(&self.st).0;
    }

    /// Timer: a drag's edit reaches the audio thread, a state replaced from outside is read again, the keyboard follows
    /// the notes playing. True when the editor has to repaint.
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

    /// An edit: the edited state becomes the plugin's at once and the audio thread plays it. During a drag its patch
    /// waits for the next repaint or timer tick ([`Logic::flush`]): a waveform setting builds a wave table (up to some
    /// milliseconds), and mouse moves come faster than the editor repaints.
    fn commit(&mut self) {
        self.used = patch::usage(&self.st).0;
        let bytes = self.st.to_bytes(self.body.as_deref());
        if !self.params.stored(self.version, bytes) {
            // The state was replaced meanwhile: show that one.
            self.pending = false;
            self.reload();
            return;
        }
        self.pending = true;
        if self.drag.is_none() {
            self.flush();
        }
    }

    /// The audio thread gets the patch of the last edit, if it has not yet.
    pub fn flush(&mut self) {
        if !std::mem::take(&mut self.pending) {
            return;
        }
        let p = Patch::with_tables(self.st.clone(), &mut self.tables);
        if !self.params.play(self.version, p) {
            self.reload();
        }
    }

    /// A setting as its control shows it: within the control's range (a state may hold, say, unison order 0).
    fn shown(&self, f: Field) -> i32 {
        let (lo, hi) = ctl::range(f);
        ctl::get(&self.st, f).clamp(lo, hi)
    }

    fn set(&mut self, f: Field, v: i32) {
        if ctl::get(&self.st, f) != v.clamp(ctl::range(f).0, ctl::range(f).1) {
            ctl::set(&mut self.st, f, v);
            self.commit();
        }
    }

    fn reset(&mut self, f: Field) {
        self.set(f, ctl::default(f));
    }

    /// The curve the curve view shows (none on the main page and the harmonics editor).
    fn curve_id(&self) -> Option<CurveId> {
        let page = self.view.page;
        if page == 0 {
            return None;
        }
        let m = view::module_of(page);
        match view::shown(&self.view) {
            view::Shown::Art(a) => Some(CurveId::Part(m, a, self.view.part[page].min(7))),
            view::Shown::Shaper => Some(CurveId::Shaper(m)),
            view::Shown::Harmonics => None,
        }
    }

    fn curve(&self, id: CurveId) -> &Curve {
        match id {
            CurveId::Part(m, a, p) => ctl::part(&self.st, m, a, p),
            CurveId::Shaper(m) => &self.st.modules[m].filter.shaper,
        }
    }

    fn curve_mut(&mut self, id: CurveId) -> &mut Curve {
        match id {
            CurveId::Part(m, a, p) => ctl::part_mut(&mut self.st, m, a, p),
            CurveId::Shaper(m) => &mut self.st.modules[m].filter.shaper,
        }
    }

    /// Edits a curve; a change is committed.
    fn edit_curve(&mut self, id: CurveId, f: impl FnOnce(&mut Curve)) {
        let c = self.curve_mut(id);
        let before = c.clone();
        f(c);
        if *c != before {
            self.commit();
        }
    }

    /// A time and level on the snap tool's grid (when it is on).
    fn snapped(&self, t: f64, l: f32, span: f64) -> (f64, f32) {
        if self.view.tools[2] {
            ((t / span * SNAP_T).round() * span / SNAP_T, (l * SNAP_L).round() / SNAP_L)
        } else {
            (t, l)
        }
    }

    /// `Point 5/7:2:517`, the level in its target's form as the right part (plugins/sytrus-editor.md §3.1). Times
    /// are half seconds (`s:ms`, the milliseconds truncated), `bar:beat:tick` when tempo-based; a map's positions
    /// are shown as percent (KEY M too: FL's note names were not measured).
    fn point_hint(&self, i: usize) -> String {
        let Some(id) = self.curve_id() else { return String::new() };
        let c = self.curve(id);
        let Some(p) = c.points.get(i) else { return String::new() };
        let t = edit::times(c)[i];
        let time = if c.kind == MAP {
            format!("{}%", (t * 100.0).round())
        } else if c.flags & crate::curve::SYNCED != 0 {
            let ticks = (t * 96.0).round() as i64;
            format!("{:03}:{:02}:{:03}", ticks / 384 + 1, ticks / 96 % 4 + 1, ticks % 96)
        } else {
            // FL's sum is single precision.
            let ms = (t as f32 * 500.0).floor() as i64;
            format!("{}:{:03}", ms / 1000, ms % 1000)
        };
        format!("Point {}/{}:{time}\t{}", i + 1, c.points.len(), Self::level_hint(id, p.level))
    }

    /// A point's level as its target shows it: a pan as left / right, a bipolar target (and every LFO) signed.
    fn level_hint(id: CurveId, l: f32) -> String {
        let (pan, signed) = match id {
            CurveId::Part(m, a, p) => {
                let bipolar = if m < OPERATORS {
                    a == OP_PITCH || a == OP_PHASE
                } else {
                    m < OPERATORS + FILTERS && (a == F_CUTOFF || a == F_RES)
                };
                (a == 0, bipolar || p == 1)
            }
            CurveId::Shaper(_) => (false, false),
        };
        if pan {
            let v = ((l - 0.5) * 200.0).round() as i32;
            match v {
                0 => "Centered".into(),
                v if v < 0 => format!("{} left", -v),
                v => format!("{v} right"),
            }
        } else if signed {
            format!("{}%", ((2.0 * l - 1.0) * 100.0).round() as i32)
        } else {
            format!("{}%", (l * 100.0).round() as i32)
        }
    }

    /// The selection on curve `id`: none when it was made on another curve or the points changed since.
    fn selection(&self, id: CurveId) -> Vec<bool> {
        let n = self.curve(id).points.len();
        if self.sel_of == Some(id) && self.sel.len() == n { self.sel.clone() } else { vec![false; n] }
    }

    fn select(&mut self, id: CurveId, sel: Vec<bool>) {
        (self.sel, self.sel_of) = (sel, Some(id));
    }

    /// Whether a selection command applies (FL: Copy and Delete need a selected point, Cut, Paste (replace) and
    /// Duplicate two).
    fn sel_enabled(&self, id: CurveId, k: CurveCmd) -> bool {
        let n = edit::count(&self.selection(id));
        match k {
            CurveCmd::Copy | CurveCmd::DeleteSel => n >= 1,
            CurveCmd::Cut | CurveCmd::PasteReplace | CurveCmd::Duplicate => n >= 2,
            _ => true,
        }
    }

    /// The Points selection commands.
    fn sel_cmd(&mut self, id: CurveId, k: CurveCmd) {
        if !self.sel_enabled(id, k) {
            return;
        }
        let sel = self.selection(id);
        let n = sel.len();
        if matches!(k, CurveCmd::Copy | CurveCmd::Cut | CurveCmd::Duplicate) {
            self.points_clip = Some(edit::copy_points(self.curve(id), &sel));
        }
        let clip = self.points_clip.clone().unwrap_or_default();
        let last = sel.iter().rposition(|&s| s);
        let mut pasted = None;
        match k {
            CurveCmd::SelectAll => self.select(id, vec![true; n]),
            CurveCmd::SelectNone => self.select(id, vec![false; n]),
            CurveCmd::Invert => self.select(id, sel.iter().map(|s| !s).collect()),
            CurveCmd::Cut => self.edit_curve(id, |c| {
                edit::cut_points(c, &sel);
            }),
            CurveCmd::DeleteSel => self.edit_curve(id, |c| {
                edit::delete_points(c, &sel);
            }),
            // Paste (insert) does nothing without a selection.
            CurveCmd::PasteInsert | CurveCmd::Duplicate if last.is_some() && !clip.is_empty() => {
                self.edit_curve(id, |c| pasted = Some(edit::paste_insert(c, last.unwrap(), &clip)));
            }
            CurveCmd::PasteReplace if !clip.is_empty() => self.edit_curve(id, |c| pasted = edit::paste_replace(c, &sel, &clip)),
            _ => {}
        }
        if let Some(r) = pasted {
            let n = self.curve(id).points.len();
            self.select(id, (0..n).map(|i| r.contains(&i)).collect());
        }
    }

    /// Scale levels...: the dialog opens reset, over the curve as it is.
    fn open_scaling(&mut self, id: CurveId) {
        self.scaling = Some((id, self.curve(id).clone()));
        self.view.scale = Some(view::SCALE_DEFAULT);
    }

    /// Sets a Level scaling control; the curve follows.
    fn scale_set(&mut self, k: usize, v: f32) {
        let Some(mut vals) = self.view.scale else { return };
        let (lo, hi) = view::SCALE_RANGE[k];
        vals[k] = v.clamp(lo, hi);
        self.view.scale = Some(vals);
        let Some((id, base)) = self.scaling.clone() else { return };
        let mut c = base;
        edit::scale_levels(&mut c, vals[0], vals[1], vals[2], vals[3]);
        self.edit_curve(id, |cur| *cur = c);
    }

    /// Reset (the controls to their defaults), Accept (the curve stays), close (the curve as it was).
    fn scale_button(&mut self, k: usize) {
        if k != 1 {
            for (j, d) in view::SCALE_DEFAULT.into_iter().enumerate() {
                self.scale_set(j, d);
            }
        }
        if k != 0 {
            self.view.scale = None;
            self.scaling = None;
        }
    }

    /// A key (Windows key codes: letters as their upper case, 0x2E Delete): the curve view's selection shortcuts.
    pub fn key(&mut self, vk: u32, m: Mods) -> Vec<Act> {
        let Some(id) = self.curve_id().filter(|_| self.view.scale.is_none() && self.view.cover == Cover::None) else { return vec![] };
        let k = match (vk, m.ctrl) {
            (0x41, true) => CurveCmd::SelectAll,
            (0x44, true) => CurveCmd::SelectNone,
            (0x49, true) => CurveCmd::Invert,
            (0x58, true) => CurveCmd::Cut,
            (0x43, true) => CurveCmd::Copy,
            (0x56, true) => CurveCmd::PasteInsert,
            (0x52, true) => CurveCmd::PasteReplace,
            (0x42, true) => CurveCmd::Duplicate,
            (0x2E, false) => CurveCmd::DeleteSel,
            _ => return vec![],
        };
        self.curve_cmd(id, k);
        vec![Act::Repaint]
    }

    fn tension_hint(&self, i: usize) -> String {
        let Some(c) = self.curve_id().map(|id| self.curve(id)) else { return String::new() };
        let t = c.points.get(i).map_or(0.0, |p| p.tension);
        format!("Point {} to {} tension\t{:.2}%", i, i + 1, t * 100.0)
    }

    fn harm_hint(&self, h: usize) -> String {
        let m = view::module_of(self.view.page);
        let l = self.st.modules[m].op.levels.get(h).copied().unwrap_or(0.0);
        format!("Harmonic {} magnitude: {}%\t+{}oct", h + 1, (l * 100.0).round(), (h + 1).ilog2())
    }

    /// Sets harmonics `a` ..= `b` of operator `m`, their levels on a line from `la` to `lb`.
    fn paint_harmonics(&mut self, m: usize, (a, la): (usize, f32), (b, lb): (usize, f32)) {
        let levels = &mut self.st.modules[m].op.levels;
        let before = levels.clone();
        for h in a.min(b)..=a.max(b).min(levels.len() - 1) {
            let x = if a == b { 1.0 } else { (h as f32 - a as f32) / (b as f32 - a as f32) };
            levels[h] = la + (lb - la) * x;
        }
        if *levels != before {
            self.commit();
        }
    }

    fn hit(&self, x: f32, y: f32) -> Option<Target> {
        self.hits.iter().rev().find(|h| h.r.contains(x, y)).map(|h| h.t.clone())
    }

    /// The setting a target edits with drags, the wheel and its menu.
    fn field(t: &Target) -> Option<Field> {
        match t {
            Target::Slider(f) | Target::Knob(f) | Target::Box(f) | Target::Cell(f) | Target::Cycle(f) | Target::Switch(f) | Target::Digit(f, _) => {
                Some(*f)
            }
            _ => None,
        }
    }

    fn hint_for(&self, t: Option<&Target>) -> String {
        let dragging = self.drag.is_some();
        match t {
            None => String::new(),
            Some(Target::Tab(i)) => match i {
                0 => "Main".into(),
                1..=6 => format!("Operator {i}"),
                7..=9 => format!("Filter {}", i - 6),
                _ => "Effects".into(),
            },
            Some(Target::Header(i)) => ["Options", "Show info", "Show block diagram", "Show / hide testing keyboard"][*i].into(),
            Some(Target::Choice(f, _)) => ctl::hint(&self.st, *f, false),
            Some(Target::Pad) => "X / Y modulation".into(),
            Some(Target::Row(r)) if *r < OPERATORS => format!("Select operator {}", r + 1),
            Some(Target::Row(r)) => format!("Select filter {}", r - OPERATORS + 1),
            Some(Target::Col(c)) if *c < OPERATORS => format!("Select operator {}", c + 1),
            Some(Target::Col(_)) => "Select effects".into(),
            Some(Target::MatrixTab(_)) => "Switch frequency / ring modulation levels".into(),
            Some(Target::Key(k)) => view::key_name(*k),
            Some(Target::PatchText) => "Info / comments".into(),
            Some(Target::Logo) => "Citrus".into(),
            Some(Target::Inert(s)) => (*s).into(),
            Some(Target::ArtTab(i)) => {
                let t = view::targets(self.view.page);
                format!("Editor target: {}", t[(*i).min(t.len() - 1)].1)
            }
            Some(Target::PartTab(i)) => format!("Articulator part: {}", ctl::PART_NAMES[*i]),
            Some(Target::FxUnit(k)) if *k < 3 => format!("Effect: delay unit {}", k + 1),
            Some(Target::FxUnit(_)) => "Effect: reverb unit".into(),
            Some(Target::Wave(_)) => "Waveform".into(),
            Some(Target::Point(i)) => self.point_hint(*i),
            Some(Target::Tension(i)) => self.tension_hint(*i),
            Some(Target::CurveArea) => String::new(),
            Some(Target::Tool(k)) => view::TOOL_NAMES[*k].into(),
            Some(Target::CurveOptions | Target::HarmOptions) => "Options".into(),
            Some(Target::Harm(h)) => self.harm_hint(*h),
            Some(Target::HarmScroll) => "Scroll harmonics".into(),
            Some(Target::Cover | Target::Dialog) => String::new(),
            Some(Target::ScaleCtl(k)) => {
                let v = self.view.scale.map_or(0.0, |v| v[*k]);
                format!("{}: {}%", view::SCALE_NAMES[*k], (v * 100.0).round() as i32)
            }
            Some(Target::ScaleBtn(k)) => ["Reset", "Accept", "Close"][*k].into(),
            Some(t) => Self::field(t).map(|f| ctl::hint(&self.st, f, dragging)).unwrap_or_default(),
        }
    }

    fn pad_to(&mut self, x: f32, y: f32) {
        let (vx, vy) = view::pad_values(x, y);
        if (ctl::get(&self.st, Field::S(25)), ctl::get(&self.st, Field::S(26))) != (vx, vy) {
            self.st.s[25] = vx;
            self.st.s[26] = vy;
            self.commit();
        }
    }

    pub fn down(&mut self, x: f32, y: f32, btn: Btn, m: Mods, _dbl: bool) -> Vec<Act> {
        self.view.mouse = Some((x, y));
        let Some(t) = self.hit(x, y) else { return vec![] };
        let mut acts = vec![Act::Repaint];
        let resettable = Self::field(&t);
        match (&t, btn) {
            (Target::Cover, _) => self.view.cover = Cover::None,
            (Target::Dialog, _) => {}
            (Target::ScaleCtl(k), Btn::Left) if m.alt => self.scale_set(*k, view::SCALE_DEFAULT[*k]),
            (Target::ScaleCtl(k), Btn::Middle) => self.scale_set(*k, view::SCALE_DEFAULT[*k]),
            (Target::ScaleCtl(k), Btn::Left) => {
                self.drag = Some(Drag::Scale(*k));
                self.view.pressed = Some(t.clone());
                acts.push(Act::Lock);
            }
            (Target::ScaleBtn(k), Btn::Left) => self.scale_button(*k),
            (Target::Tab(i), Btn::Left | Btn::Right) => self.view.page = *i,
            (Target::ArtTab(i), Btn::Left | Btn::Right) => self.view.target[self.view.page] = *i,
            (Target::PartTab(i), Btn::Left | Btn::Right) => self.view.part[self.view.page] = *i,
            (Target::FxUnit(k), Btn::Left | Btn::Right) => self.view.fx_unit = *k,
            (Target::Wave(m), Btn::Right) => acts.push(Act::Menu(MenuReq::Wave(*m), x, y)),
            // A click on a point selects it alone; on one of a selection it drags them all.
            (Target::Point(i), Btn::Left) => {
                if let Some(id) = self.curve_id() {
                    let span = view::curve_span(self.curve(id));
                    let sel = self.selection(id);
                    if sel.get(*i) == Some(&true) && edit::count(&sel) > 1 {
                        self.orig = Some(self.curve(id).clone());
                        self.drag = Some(Drag::Group(*i, x, y, span));
                    } else {
                        self.select(id, (0..sel.len()).map(|k| k == *i).collect());
                        self.drag = Some(Drag::Point(*i, span));
                    }
                    self.view.span = Some(span);
                    self.view.pressed = Some(t.clone());
                    acts.push(Act::Capture);
                }
            }
            (Target::Point(i), Btn::Right) => {
                if let Some(id) = self.curve_id() {
                    acts.push(Act::Menu(MenuReq::Point(id, *i), x, y));
                }
            }
            (Target::Tension(i), Btn::Left) => {
                if let Some(id) = self.curve_id() {
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
            }
            (Target::Tension(i), Btn::Middle) => {
                if let Some(id) = self.curve_id() {
                    self.edit_curve(id, |c| edit::set_tension(c, *i, 0.0));
                }
            }
            // Ctrl + drag on empty space: a rubber band.
            (Target::CurveArea, Btn::Left) if m.ctrl => {
                if self.curve_id().is_some() {
                    self.drag = Some(Drag::Band(x, y));
                    self.view.band = Some(os_ui::Rect::new(x, y, 0.0, 0.0));
                    acts.push(Act::Capture);
                }
            }
            // A right click on empty space adds a point there.
            (Target::CurveArea, Btn::Right) if !self.view.tools[0] => {
                if let Some(id) = self.curve_id() {
                    let span = view::curve_span(self.curve(id));
                    let (t, l) = self.snapped(view::x_to_t(x, span).min(span), view::y_to_level(y), span);
                    self.edit_curve(id, |c| {
                        edit::insert_point(c, if c.kind == MAP { t.min(1.0 - 1e-3) } else { t }, l);
                    });
                }
            }
            // Step editing: not done.
            (Target::Tool(1), _) => {}
            (Target::Tool(k), Btn::Left) => self.view.tools[*k] = !self.view.tools[*k],
            (Target::CurveOptions, Btn::Left) => {
                if let Some(id) = self.curve_id() {
                    acts.push(Act::Menu(MenuReq::Curve(id), x, y));
                }
            }
            (Target::HarmOptions, Btn::Left) => acts.push(Act::Menu(MenuReq::Harm(view::module_of(self.view.page)), x, y)),
            (Target::Harm(h), Btn::Left) => {
                let (m, l) = (view::module_of(self.view.page), view::harm_level(y));
                self.paint_harmonics(m, (*h, l), (*h, l));
                self.drag = Some(Drag::Harm(*h, l));
                acts.push(Act::Capture);
            }
            (Target::HarmScroll, Btn::Left) => {
                self.view.harm_scroll = view::harm_scroll_at(x);
                self.drag = Some(Drag::HarmScroll);
                acts.push(Act::Capture);
            }
            (Target::Header(0), Btn::Left) => acts.push(Act::Menu(MenuReq::Options, x, y)),
            (Target::Header(1), Btn::Left) | (Target::Logo, Btn::Left) => self.view.cover = Cover::Info,
            (Target::Header(2), Btn::Left) => self.view.cover = Cover::Diagram,
            (Target::Header(_), Btn::Left) => {
                self.view.no_keys = !self.view.no_keys;
                self.no_keys.store(self.view.no_keys, Ordering::Relaxed);
                acts.push(Act::Resize);
            }
            (_, Btn::Left) if m.alt && resettable.is_some() => self.reset(resettable.unwrap()),
            (_, Btn::Middle) if resettable.is_some() => self.reset(resettable.unwrap()),
            (Target::Slider(f) | Target::Knob(f) | Target::Cell(f), Btn::Left) => {
                self.drag = Some(Drag::Value(*f, self.shown(*f) as f32));
                self.view.pressed = Some(t.clone());
                acts.push(Act::Lock);
            }
            (Target::Box(f), Btn::Left) => {
                self.drag = Some(Drag::Steps(*f, self.shown(*f) as f32));
                self.view.pressed = Some(t.clone());
                acts.push(Act::Lock);
            }
            (Target::Digit(f, step), Btn::Left) => {
                self.drag = Some(Drag::Digit(*f, *step, 0.0));
                self.view.pressed = Some(t.clone());
                acts.push(Act::Lock);
            }
            (Target::Switch(f), Btn::Left) => {
                let on = ctl::get(&self.st, *f) != 0;
                self.set(*f, i32::from(!on));
            }
            (Target::Choice(f, v), Btn::Left) => self.set(*f, *v),
            (Target::Cycle(f), Btn::Left) => {
                let (lo, hi) = ctl::range(*f);
                let v = ctl::get(&self.st, *f);
                self.set(*f, if v >= hi { lo } else { v + 1 });
            }
            (Target::Pad, Btn::Left) => {
                self.drag = Some(Drag::Pad);
                self.view.pressed = Some(t.clone());
                self.pad_to(x, y);
                acts.push(Act::Capture);
            }
            // A right click turns a cell off and on (its "cell off" byte).
            (Target::Cell(f), Btn::Right) => {
                let k = match *f {
                    Field::Ring(r, c) => 81 + r * 9 + c,
                    Field::Cell(r, c) => r * 9 + c,
                    _ => unreachable!(),
                };
                self.st.cell_off[k] = u8::from(self.st.cell_off[k] == 0);
                self.commit();
            }
            (Target::Slider(f) | Target::Knob(f) | Target::Box(f) | Target::Cycle(f) | Target::Digit(f, _), Btn::Right) => {
                acts.push(Act::Menu(MenuReq::Param(*f), x, y))
            }
            (Target::Row(i), Btn::Left) => self.view.page = view::label_page(true, *i).unwrap_or(self.view.page),
            (Target::Col(i), Btn::Left) => self.view.page = view::label_page(false, *i).unwrap_or(self.view.page),
            (Target::MatrixTab(ring), Btn::Left) => self.view.ring = *ring,
            (Target::Key(k), Btn::Left) => {
                self.drag = Some(Drag::Key);
                self.view.key = Some(*k);
                acts.push(Act::Capture);
                acts.push(Act::Note(*k, true));
            }
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
                let k = if m.ctrl { FINE } else { 1.0 };
                let v = (v - dy * k * (hi - lo) as f32 / KNOB_PX).clamp(lo as f32, hi as f32);
                self.drag = Some(Drag::Value(f, v));
                self.set(f, v.round() as i32);
            }
            Some(Drag::Steps(f, v)) => {
                let (lo, hi) = ctl::range(f);
                let v = (v - dy / BOX_PX).clamp(lo as f32, hi as f32);
                self.drag = Some(Drag::Steps(f, v));
                self.set(f, v.round() as i32);
            }
            // One step of the digit per 10 px, up raising it.
            Some(Drag::Digit(f, step, moved)) => {
                let mut moved = moved - dy;
                let mut v = self.shown(f);
                while moved >= DIGIT_PX {
                    moved -= DIGIT_PX;
                    v += step;
                }
                while moved <= -DIGIT_PX {
                    moved += DIGIT_PX;
                    v -= step;
                }
                self.drag = Some(Drag::Digit(f, step, moved));
                self.set(f, v);
            }
            Some(Drag::Scale(k)) => {
                let Some(v) = self.view.scale else { return vec![] };
                let per = if k == 1 { 1.0 / 128.0 } else { 1.0 / 256.0 };
                let f = if m.ctrl { FINE } else { 1.0 };
                self.scale_set(k, v[k] - dy * per * f);
            }
            Some(Drag::Tension(i, sign)) => {
                let Some(id) = self.curve_id() else { return vec![] };
                let k = if m.ctrl { FINE } else { 1.0 };
                let t = self.curve(id).points.get(i).map_or(0.0, |p| p.tension) - sign * dy * k / TENSION_PX;
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
        self.flush();
        self.view.pressed = None;
        self.view.span = None;
        let mut acts = vec![Act::Repaint];
        if let (Drag::Key, Some(k)) = (d, self.view.key.take()) {
            acts.push(Act::Note(k, false));
        }
        self.orig = None;
        // The rubber band selects the points inside it.
        if let (Drag::Band(..), Some(b), Some(id)) = (d, self.view.band.take(), self.curve_id()) {
            let c = self.curve(id);
            let span = view::curve_span(c);
            let sel = (0..c.points.len())
                .map(|i| {
                    let (px, py) = view::point_xy(c, i, span);
                    b.contains(px, py)
                })
                .collect();
            self.select(id, sel);
        }
        let t = self.view.mouse.and_then(|(x, y)| self.hit(x, y));
        self.hint = self.hint_for(t.as_ref());
        acts
    }

    pub fn moved(&mut self, x: f32, y: f32, _m: Mods) -> Vec<Act> {
        self.view.mouse = Some((x, y));
        match self.drag {
            Some(Drag::Pad) => self.pad_to(x, y),
            Some(Drag::Point(i, span)) => {
                if let Some(id) = self.curve_id() {
                    let (t, l) = self.snapped(view::x_to_t(x, span), view::y_to_level(y), span);
                    let slide = self.view.tools[3];
                    self.edit_curve(id, |c| edit::move_point(c, i, t, l, slide));
                    self.hint = self.point_hint(i);
                }
            }
            Some(Drag::Group(i, x0, y0, span)) => {
                if let (Some(id), Some(orig)) = (self.curve_id(), self.orig.clone()) {
                    let inner = view::curve_inner();
                    let dt = f64::from((x - x0) / inner.w) * span;
                    let dl = (y0 - y) / inner.h;
                    let sel = self.selection(id);
                    self.edit_curve(id, |c| edit::move_points(c, &orig, &sel, dt, dl));
                    self.hint = self.point_hint(i);
                }
            }
            Some(Drag::Band(x0, y0)) => {
                self.view.band = Some(os_ui::Rect::new(x0.min(x), y0.min(y), (x - x0).abs(), (y - y0).abs()));
            }
            // Bars skipped between two moves are painted on the line between them.
            Some(Drag::Harm(h0, l0)) => {
                let m = view::module_of(self.view.page);
                let (h, l) = (view::harm_at(x, self.view.harm_scroll.min(256 - view::HARM_VISIBLE)), view::harm_level(y));
                self.paint_harmonics(m, (h0, l0), (h, l));
                self.drag = Some(Drag::Harm(h, l));
                self.hint = self.harm_hint(h);
            }
            Some(Drag::HarmScroll) => self.view.harm_scroll = view::harm_scroll_at(x),
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

    /// Wheel: 1/32 of the range per notch (at least one step); a box one step; a switch flips.
    pub fn wheel(&mut self, x: f32, y: f32, notches: f32, _m: Mods) -> Vec<Act> {
        let Some(t) = self.hit(x, y) else { return vec![] };
        let d = notches.signum() as i32;
        match &t {
            Target::Slider(f) | Target::Knob(f) | Target::Cell(f) => {
                let (lo, hi) = ctl::range(*f);
                let step = (((hi - lo) as f32 * WHEEL_PART).round() as i32).max(1);
                self.set(*f, self.shown(*f) + d * step);
            }
            Target::Box(f) | Target::Cycle(f) => self.set(*f, self.shown(*f) + d),
            Target::Digit(f, step) => self.set(*f, self.shown(*f) + d * step),
            Target::Switch(f) => self.set(*f, i32::from(d > 0)),
            Target::Harm(_) | Target::HarmScroll => {
                let s = self.view.harm_scroll.min(256 - view::HARM_VISIBLE) as i32 - d * HARM_WHEEL;
                self.view.harm_scroll = s.clamp(0, (256 - view::HARM_VISIBLE) as i32) as usize;
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
                m.add(MenuItem::item("Type in value..."), Cmd::TypeIn(*f));
                m
            }
            MenuReq::Options => self.options(),
            // The shape's corners (plugins/sytrus.md, "Shape"); converting to harmonics comes with the harmonics editor.
            MenuReq::Wave(m) => {
                let mut w = MenuDef::new(MenuStyle::Context);
                for (l, v) in [("Square", 384), ("Saw", 256), ("Triangle", 128), ("Sine", 0)] {
                    w.add(MenuItem::item(l), Cmd::Set(Field::Op(*m, 3), v));
                }
                w.sep(Cmd::None);
                w.add(MenuItem::item("Convert shape to sine harmonics"), Cmd::Harm(*m, HarmCmd::Convert));
                w
            }
            MenuReq::Point(id, i) => self.point_menu(*id, *i),
            MenuReq::Curve(id) => self.curve_menu(*id),
            MenuReq::Harm(m) => self.harm_menu(*m),
        }
    }

    /// A point's menu (plugins/sytrus-editor.md §3).
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
        m.add(MenuItem::item("Type in value..."), cmd(CurveCmd::TypeIn(i)));
        m
    }

    /// The curve view's Options menu (plugins/sytrus-editor.md §3); the items not done are disabled.
    fn curve_menu(&self, id: CurveId) -> MenuDef<Cmd> {
        let c = self.curve(id);
        let cmd = |k| Cmd::Curve(id, k);
        let off = |m: &mut MenuDef<Cmd>, labels: &[&str]| {
            for l in labels {
                m.add(MenuItem::item(l).enabled(false), Cmd::None);
            }
        };
        let mut m = MenuDef::new(MenuStyle::Context);
        m.head("State", Cmd::None);
        off(&mut m, &["Presets", "Open state file...", "Save state file..."]);
        m.add(MenuItem::item("Copy state"), cmd(CurveCmd::CopyState));
        m.add(MenuItem::item("Paste state").enabled(self.curve_clip.as_ref().is_some_and(|s| s.kind == c.kind)), cmd(CurveCmd::PasteState));
        m.head("Points selection", Cmd::None);
        for (l, key, k) in [
            ("Select all", "Ctrl+A", CurveCmd::SelectAll),
            ("Deselect all", "Ctrl+D", CurveCmd::SelectNone),
            ("Invert", "Ctrl+I", CurveCmd::Invert),
            ("Cut", "Ctrl+X", CurveCmd::Cut),
            ("Copy", "Ctrl+C", CurveCmd::Copy),
            ("Paste (insert)", "Ctrl+V", CurveCmd::PasteInsert),
            ("Paste (replace)", "Ctrl+R", CurveCmd::PasteReplace),
            ("Duplicate", "Ctrl+B", CurveCmd::Duplicate),
            ("Delete", "Del", CurveCmd::DeleteSel),
        ] {
            m.add(MenuItem::item(l).shortcut(key).enabled(self.sel_enabled(id, k)), cmd(k));
        }
        m.head("Tools", Cmd::None);
        m.add(MenuItem::item("Make all loop").enabled(edit::keeps_marker(c, 1)), cmd(CurveCmd::AllLoop));
        off(&mut m, &["Insert point at locator"]);
        m.add(MenuItem::item("Flip vertically"), cmd(CurveCmd::Flip));
        m.add(MenuItem::item("Scale levels..."), cmd(CurveCmd::Scale));
        m.add(MenuItem::item("Normalize levels"), cmd(CurveCmd::Normalize));
        off(&mut m, &["Decimate points...", "Filter...", "Smooth up..."]);
        m.add(MenuItem::item("Smooth up abrupt changes"), cmd(CurveCmd::SmoothAbrupt));
        m.add(MenuItem::item("Turn all points smooth"), cmd(CurveCmd::AllSmooth));
        off(&mut m, &["Create sequence...", "Analyze audio file..."]);
        m.head("Options", Cmd::None);
        off(&mut m, &["Focus on click"]);
        m.head("View", Cmd::None);
        off(&mut m, &["Background gradient", "Invert grid"]);
        m
    }

    /// The harmonics editor's Options menu (plugins/sytrus-editor.md §4.2); the items not done are disabled.
    fn harm_menu(&self, op: usize) -> MenuDef<Cmd> {
        let cmd = |k| Cmd::Harm(op, k);
        let mut m = MenuDef::new(MenuStyle::Context);
        let add = |m: &mut MenuDef<Cmd>, l: &str, k: Option<HarmCmd>| {
            m.add(MenuItem::item(l).enabled(k.is_some()), k.map_or(Cmd::None, cmd));
        };
        add(&mut m, "Convert shape to sine harmonics", Some(HarmCmd::Convert));
        add(&mut m, "Analyze single-cycle waveform...", None);
        add(&mut m, "Export single-cycle waveform...", None);
        add(&mut m, "Lock harmonics", None);
        m.sep(Cmd::None);
        add(&mut m, "Transpose one octave up", Some(HarmCmd::Transpose));
        add(&mut m, "Shift left", Some(HarmCmd::Shift(false)));
        add(&mut m, "Shift right", Some(HarmCmd::Shift(true)));
        add(&mut m, "Smooth up (soft)", None);
        add(&mut m, "Smooth up (hard)", None);
        add(&mut m, "Attenuate Gibbs phenomenon", Some(HarmCmd::Gibbs));
        add(&mut m, "Remove Gibbs phenomenon", None);
        m.sep(Cmd::None);
        add(&mut m, "Reset phases", Some(HarmCmd::ResetPhases));
        add(&mut m, "Randomize phases", Some(HarmCmd::RandomPhases));
        m.sep(Cmd::None);
        add(&mut m, "Copy", Some(HarmCmd::Copy));
        m.add(MenuItem::item("Paste").enabled(self.harm_clip.is_some()), cmd(HarmCmd::Paste));
        m
    }

    fn curve_cmd(&mut self, id: CurveId, k: CurveCmd) {
        if k.point().is_some_and(|i| i >= self.curve(id).points.len()) {
            return;
        }
        match k {
            CurveCmd::SelectAll
            | CurveCmd::SelectNone
            | CurveCmd::Invert
            | CurveCmd::Cut
            | CurveCmd::Copy
            | CurveCmd::PasteInsert
            | CurveCmd::PasteReplace
            | CurveCmd::Duplicate
            | CurveCmd::DeleteSel => self.sel_cmd(id, k),
            CurveCmd::Scale => self.open_scaling(id),
            CurveCmd::TypeIn(_) => {}
            CurveCmd::CopyLevel(i) => self.level_clip = Some(self.curve(id).points[i].level),
            CurveCmd::CopyState => self.curve_clip = Some(self.curve(id).clone()),
            k => {
                let (level, clip) = (self.level_clip, self.curve_clip.clone());
                self.edit_curve(id, |c| match k {
                    CurveCmd::Delete(i) => {
                        edit::delete_point(c, i);
                    }
                    CurveCmd::Mode(i, v) => edit::set_mode(c, i, v),
                    CurveCmd::Break(i, v) => edit::set_break(c, i, v),
                    CurveCmd::Marker(i, m) => edit::toggle_marker(c, i, m),
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
                    CurveCmd::Normalize => edit::normalize(c),
                    CurveCmd::AllSmooth => edit::all_smooth(c),
                    CurveCmd::SmoothAbrupt => edit::smooth_abrupt(c),
                    _ => {}
                });
            }
        }
    }

    fn harm_cmd(&mut self, m: usize, k: HarmCmd) {
        if k == HarmCmd::Copy {
            let op = &self.st.modules[m].op;
            self.harm_clip = Some((op.levels.clone(), op.phases.clone()));
            return;
        }
        let smooth = self.st.switch(2) != 0;
        self.seed = self.seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        let (seed, clip) = (self.seed, self.harm_clip.clone());
        let op = &mut self.st.modules[m].op;
        let before = op.clone();
        match k {
            HarmCmd::Convert => edit::convert_to_harmonics(op, smooth),
            HarmCmd::Transpose => edit::transpose_up(op),
            HarmCmd::Shift(right) => edit::shift(op, right),
            HarmCmd::Gibbs => edit::attenuate_gibbs(op),
            HarmCmd::ResetPhases => edit::reset_phases(op),
            HarmCmd::RandomPhases => edit::randomize_phases(op, seed),
            HarmCmd::Paste => {
                if let Some((l, p)) = clip {
                    (op.levels, op.phases) = (l, p);
                }
            }
            HarmCmd::Copy => {}
        }
        if *op != before {
            self.commit();
        }
    }

    /// The Options menu's switch items (plugins/sytrus-editor.md §7).
    fn options(&self) -> MenuDef<Cmd> {
        let sw = |n: usize| i32::from(self.st.switches[n]);
        let check = |m: &mut MenuDef<Cmd>, label: &str, f: Field, on: bool, value: i32| {
            m.add(MenuItem::item(label).mark(Mark::Check(on)), Cmd::Set(f, value));
        };
        let radios = |m: &mut MenuDef<Cmd>, labels: &[&str], n: usize| {
            for (k, l) in labels.iter().enumerate() {
                m.add(MenuItem::item(l).mark(Mark::Radio(sw(n) == k as i32)), Cmd::Set(Field::Switch(n), k as i32));
            }
        };
        let mut dist = MenuDef::new(MenuStyle::Context);
        radios(&mut dist, &["Original", "Improved", "Uniform", "Blurred"], 11);
        // Presets: only Unison's Default was measured; the others are shown disabled.
        let presets = |labels: &[&str], first: Cmd| {
            let mut p = MenuDef::new(MenuStyle::Context);
            for (k, l) in labels.iter().enumerate() {
                let cmd = if k == 0 { first.clone() } else { Cmd::None };
                p.add(MenuItem::item(l).enabled(cmd != Cmd::None), cmd);
            }
            p
        };
        let mut unison = MenuDef::new(MenuStyle::Context);
        unison.submenu("Distribution", dist, Cmd::None);
        unison.sep(Cmd::None);
        for (k, l) in ["Default", "Detuned", "Flanged", "Sub", "Stereoized", "Fat", "None"].into_iter().enumerate() {
            let cmd = if k == 0 { Cmd::UnisonDefault } else { Cmd::None };
            unison.add(MenuItem::item(l).enabled(k == 0), cmd);
        }
        let chorus = presets(
            &["Default", "Deep chorus", "Fast flanger", "Flanger", "Alt flanger", "Modulation", "Stereoized", "Surround flanger", "Vibration", "None"],
            Cmd::None,
        );
        let reverb = presets(&["Default", "Drum room", "The venue", "Cathedral", "Large hall", "Small studio", "None"], Cmd::None);
        let mut size = MenuDef::new(MenuStyle::Context);
        for (k, l) in ["Small", "Medium", "Large", "XL"].into_iter().enumerate() {
            size.add(MenuItem::item(l).mark(Mark::Radio(k == 0)).enabled(k == 0), Cmd::None);
        }
        let mut arp = MenuDef::new(MenuStyle::Context);
        check(&mut arp, "Invert", Field::Switch(3), sw(3) != 0, i32::from(sw(3) == 0));
        let mut porta = MenuDef::new(MenuStyle::Context);
        for (n, l) in [(5, "Variable time"), (6, "Track origin")] {
            check(&mut porta, l, Field::Switch(n), sw(n) != 0, i32::from(sw(n) == 0));
        }
        porta.sep(Cmd::None);
        radios(&mut porta, &["No limit", "Octave wrap", "4 semitones limit", "3 semitones limit", "2 semitones limit", "1 semitone limit"], 7);
        let mut vel = MenuDef::new(MenuStyle::Context);
        check(&mut vel, "Link velocity to volume", Field::Switch(8), sw(8) != 0, i32::from(sw(8) == 0));
        for (bit, l) in [(1, "Link velocity to attack time"), (2, "Link velocity to attack length")] {
            check(&mut vel, l, Field::Switch(9), sw(9) & bit != 0, sw(9) ^ bit);
        }
        check(&mut vel, "Link release to release length", Field::Switch(10), sw(10) != 0, i32::from(sw(10) == 0));
        let mut m = MenuDef::new(MenuStyle::Context);
        // The oscillator items work on the operator page shown.
        let op = (1..=OPERATORS).contains(&self.view.page).then(|| self.view.page - 1);
        m.head("Oscillators", Cmd::None);
        for (l, k, on) in [
            ("Copy oscillator settings", op.map(OscCmd::Copy), true),
            ("Paste oscillator settings", op.map(OscCmd::Paste), self.op_clip.is_some()),
            ("Reset waveform settings", op.map(OscCmd::Reset), true),
            ("Randomize waveform settings", op.map(OscCmd::Randomize), true),
            ("Morph oscillators 1 to 5", Some(OscCmd::Morph), true),
        ] {
            m.add(MenuItem::item(l).enabled(on && k.is_some()), k.map_or(Cmd::None, Cmd::Osc));
        }
        m.head("Effects", Cmd::None);
        m.submenu("Unison", unison, Cmd::None);
        m.submenu("Chorus", chorus, Cmd::None);
        m.submenu("Reverb", reverb, Cmd::None);
        m.head("Tone", Cmd::None);
        m.submenu("Arpeggiator", arp, Cmd::None);
        m.submenu("Portamento", porta, Cmd::None);
        m.submenu("Velocity", vel, Cmd::None);
        m.head("Options", Cmd::None);
        m.submenu("Size", size, Cmd::None);
        m.add(MenuItem::item("HQ WaveShaper").mark(Mark::Check(true)).enabled(false), Cmd::None);
        m
    }

    /// The Options menu's oscillator items (plugins/sytrus-editor.md §7).
    fn osc_cmd(&mut self, k: OscCmd) {
        if let OscCmd::Copy(m) = k {
            self.op_clip = Some(self.st.modules[m].op.clone());
            return;
        }
        let before: Vec<OpBlock> = self.st.modules[..OPERATORS].iter().map(|m| m.op.clone()).collect();
        let smooth = self.st.switch(2) != 0;
        self.seed = self.seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        match k {
            OscCmd::Paste(m) => {
                if let Some(op) = self.op_clip.clone() {
                    self.st.modules[m].op = op;
                }
            }
            OscCmd::Reset(m) => edit::reset_waveform(&mut self.st.modules[m].op),
            OscCmd::Randomize(m) => edit::randomize_waveform(&mut self.st.modules[m].op, self.seed),
            OscCmd::Morph => {
                let [a, b, c, d, e, ..] = &mut self.st.modules[..] else { return };
                edit::morph_operators(&mut [&mut a.op, &mut b.op, &mut c.op, &mut d.op, &mut e.op], smooth);
            }
            OscCmd::Copy(_) => {}
        }
        if before.iter().zip(&self.st.modules).any(|(b, m)| *b != m.op) {
            self.commit();
        }
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
            Cmd::TypeIn(f) => return vec![Act::TypeIn(TypeTarget::Field(f), x, y)],
            Cmd::Curve(id, CurveCmd::TypeIn(i)) => return vec![Act::TypeIn(TypeTarget::Point(id, i), x, y)],
            Cmd::Set(f, v) => self.set(f, v),
            Cmd::Curve(id, k) => self.curve_cmd(id, k),
            Cmd::Harm(m, k) => self.harm_cmd(m, k),
            Cmd::Osc(k) => self.osc_cmd(k),
            // FL's measured Default: unison order 4, the other unison settings as they were in its test.
            Cmd::UnisonDefault => {
                let before = self.st.s;
                self.st.s[8..=14].copy_from_slice(&[4, 128, 100, 128, 0, 16, 0]);
                if self.st.s != before {
                    self.commit();
                }
            }
        }
        vec![Act::Repaint]
    }

    /// The value text a type-in box starts with: a point's level with 15 significant digits.
    pub fn value_text(&self, t: &TypeTarget) -> String {
        match *t {
            TypeTarget::Field(f) => ctl::text(f, ctl::get(&self.st, f)),
            TypeTarget::Point(id, i) => self.curve(id).points.get(i).map(|p| edit::level_text(p.level)).unwrap_or_default(),
        }
    }

    /// Applies a typed value; text that does not read leaves it as it was.
    pub fn typed(&mut self, t: &TypeTarget, text: &str) {
        match *t {
            TypeTarget::Field(f) => {
                if let Some(v) = ctl::parse(f, text) {
                    self.set(f, v);
                }
            }
            TypeTarget::Point(id, i) => {
                if let Some(l) = edit::parse_level(text).filter(|_| i < self.curve(id).points.len()) {
                    self.edit_curve(id, |c| {
                        c.points[i].level = l;
                        c.prepare();
                    });
                }
            }
        }
    }

    /// The surface's height now.
    pub fn height(&self) -> u32 {
        view::height(self.view.no_keys)
    }

    /// Paints the editor; `hint` is drawn when the host does not show it.
    pub fn paint(&mut self, c: &mut os_ui::Canvas) {
        self.flush();
        let hint = if self.hosted { String::new() } else { self.hint.clone() };
        if (1..=OPERATORS).contains(&self.view.page) {
            let o = self.view.page - 1;
            let op = &self.st.modules[o].op;
            let smooth = self.st.switch(2) != 0;
            if !self.wave.as_ref().is_some_and(|(k, s, _)| k == op && *s == smooth) {
                // The patch's own table (flushed just above), not a second build.
                let table = self.tables.get(o, op, smooth);
                let t = &table.t;
                let n = crate::wave::N;
                let w = (0..WAVE_POINTS).map(|i| t[i * n / (WAVE_POINTS - 1)]).collect();
                self.wave = Some((op.clone(), smooth, w));
            }
        }
        self.view.selected = self.curve_id().map(|id| self.selection(id)).unwrap_or_default();
        let wave = self.wave.as_ref().filter(|_| (1..=OPERATORS).contains(&self.view.page)).map(|w| w.2.as_slice());
        let model = view::Model { st: &self.st, used: self.used, version: env!("CARGO_PKG_VERSION"), wave };
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
    use crate::CitrusParams;

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

    pub fn open(parent: *mut c_void, scale: f32, params: Arc<CitrusParams>, no_keys: Arc<AtomicBool>, ctx: Arc<dyn GuiContext>, slot: Arc<AtomicIsize>) -> Handle {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let logic = Logic::new(params, no_keys);
        let px = |v: u32| (v as f32 * scale).round() as u32;
        let size = (px(view::W), px(logic.height()));
        unsafe {
            let class = pw::register("CitrusView", Some(wndproc));
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
            let title = pw::wide("Citrus");
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
                    // Another window took the mouse (not this one capturing again): end the drag.
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
                WM_KEYDOWN => {
                    let acts = w.logic.key(wp as u32, mods());
                    if acts.is_empty() {
                        return DefWindowProcW(hwnd, msg, wp, lp);
                    }
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
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    use nih_plug::prelude::GuiContext;
    use os_hostlink::{Cmd, PageIcon, Peer};
    use os_plugkit::gui::{self as px, Button, Input, Window};
    use os_theme::Palette;
    use os_ui::Canvas;

    use super::{Act, Btn, Logic, Mods};
    use crate::view;
    use crate::CitrusParams;

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
        /// The editor closes: the host no longer finds it.
        fn drop(&mut self) {
            self.link.unregister();
        }
    }

    /// Owns the editor thread; dropping it closes the editor.
    pub struct Handle(#[allow(dead_code)] Option<px::EditorThread>);

    pub fn open(parent: px::Parent, scale: f32, params: Arc<CitrusParams>, no_keys: Arc<AtomicBool>, ctx: Arc<dyn GuiContext>, slot: &px::Slot) -> Handle {
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
                    // The logic takes Windows key codes: letters as their upper case, Delete 0x2E.
                    Input::Key { sym, .. } => {
                        let vk = match sym {
                            0x61..=0x7a => sym - 0x20,
                            0x41..=0x5a => sym,
                            0xffff => 0x2E,
                            _ => 0,
                        };
                        if vk != 0 {
                            let acts = self.logic.key(vk, self.mods());
                            self.act(acts);
                        }
                    }
                    Input::Drop { .. } => {}
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

        /// Timer: a state replaced from outside, the hint for the host.
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
                    // The window keeps the pointer while a button is held.
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
        let mut l = Logic::new(Arc::new(CitrusParams::default()), Arc::new(AtomicBool::new(false)));
        paint(&mut l);
        l
    }

    fn paint(l: &mut Logic) {
        let mut c = os_ui::Canvas::new(view::W, l.height(), 1.0).unwrap();
        l.paint(&mut c);
    }

    fn centre(l: &Logic, t: &Target) -> (f32, f32) {
        let h = l.hits.iter().rev().find(|h| &h.t == t).unwrap_or_else(|| panic!("no hit {t:?}"));
        (h.r.cx(), h.r.cy())
    }

    /// The state the plugin holds now, as read.
    fn saved(l: &Logic) -> State {
        State::from_bytes(&l.params.state.read().unwrap().0).expect("an edited state")
    }

    /// The controls sit where FL has them (plugins/sytrus-editor.md).
    #[test]
    fn controls_sit_where_fl_has_them() {
        let l = logic();
        for (t, x, y) in [
            (Target::Slider(Field::Main(0)), 33.0, 88.0),
            (Target::Knob(Field::S(25)), 162.0, 192.0),
            (Target::Knob(Field::Soften), 524.0, 261.0),
            (Target::Switch(Field::Switch(4)), 448.0, 283.0),
            (Target::Cell(Field::Cell(0, 1)), 613.0, 63.0),
            (Target::Cell(Field::Cell(8, 8)), 830.0, 311.0),
            (Target::Tab(10), 6.0 + 680.0 + 33.0, 15.0),
        ] {
            let h = l.hits.iter().find(|h| h.t == t).unwrap_or_else(|| panic!("{t:?}"));
            assert!(h.r.contains(x, y), "{t:?} at {:?}", h.r);
        }
        assert!(l.hits.iter().any(|h| h.t == Target::Key(60)));
    }

    /// During a drag each step's state is saved at once and its patch reaches the audio thread once a repaint (or
    /// timer tick) and when the drag ends, not once a mouse move.
    #[test]
    fn drag_patches_wait_for_a_repaint() {
        let mut l = logic();
        let edit = |l: &Logic| l.params.edit.lock().unwrap().take();
        let (x, y) = centre(&l, &Target::Knob(Field::Soften));
        l.down(x, y, Btn::Left, Mods::default(), false);
        edit(&l);
        for _ in 0..40 {
            l.delta(0.0, -1.0, Mods::default());
        }
        assert_eq!(saved(&l).soften, 20);
        assert!(edit(&l).is_none(), "no patch per mouse move");
        paint(&mut l);
        assert_eq!(edit(&l).map(|(_, p)| p.st.soften), Some(20));
        l.delta(0.0, -2.0, Mods::default());
        assert!(edit(&l).is_none());
        l.up(Btn::Left);
        assert_eq!(edit(&l).map(|(_, p)| p.st.soften), Some(saved(&l).soften));
        // Outside a drag an edit plays at once.
        l.wheel(x, y, 1.0, Mods::default());
        assert!(edit(&l).is_some());
    }

    /// A knob drag: 250 px for the range, Ctrl 20 times finer; the wheel 1/32; Alt resets; each edit is saved.
    #[test]
    fn drags_wheel_and_reset_edit_the_saved_state() {
        let mut l = logic();
        let t = Target::Knob(Field::Soften);
        let (x, y) = centre(&l, &t);
        assert!(l.down(x, y, Btn::Left, Mods::default(), false).contains(&Act::Lock));
        // 125 px up = half of 0..128.
        for _ in 0..125 {
            l.delta(0.0, -1.0, Mods::default());
        }
        l.up(Btn::Left);
        assert_eq!(saved(&l).soften, 64);
        assert_eq!(l.hint, "Soften attack of lower-velocity voices: 50%");
        l.down(x, y, Btn::Left, Mods::default(), false);
        for _ in 0..100 {
            l.delta(0.0, -1.0, Mods { ctrl: true, ..Mods::default() });
        }
        l.up(Btn::Left);
        assert_eq!(saved(&l).soften, 64 + 3);
        l.wheel(x, y, 1.0, Mods::default());
        assert_eq!(saved(&l).soften, 67 + 4);
        l.down(x, y, Btn::Left, Mods { alt: true, ..Mods::default() }, false);
        assert_eq!(saved(&l).soften, 0);
        // The edit reached the audio thread's slot, for the state version it edits.
        let e = l.params.edit.lock().unwrap().take().expect("an edit");
        assert_eq!(e.0, l.params.state_version.load(Ordering::Acquire));
        assert_eq!(e.1.st.soften, 0);
    }

    /// The matrix: a cell drag, a right click turns it off, RM shows the ring cells.
    #[test]
    fn matrix_cells_edit_and_turn_off() {
        let mut l = logic();
        let t = Target::Cell(Field::Cell(0, 1));
        let (x, y) = centre(&l, &t);
        l.moved(x, y, Mods::default());
        assert_eq!(l.hint, "operator 2 to 1 frequency modulation\tFM");
        l.down(x, y, Btn::Left, Mods::default(), false);
        // 250 px = the whole -256..256: 25 px = 51.2.
        for _ in 0..25 {
            l.delta(0.0, -1.0, Mods::default());
        }
        assert_eq!(l.hint, "20%");
        l.up(Btn::Left);
        assert_eq!(saved(&l).matrix[1], 51);
        l.down(x, y, Btn::Right, Mods::default(), false);
        assert_eq!((saved(&l).cell_off[1], saved(&l).cell(0, 1)), (1, 0));
        let (rx, ry) = centre(&l, &Target::MatrixTab(true));
        l.down(rx, ry, Btn::Left, Mods::default(), false);
        paint(&mut l);
        let (x, y) = centre(&l, &Target::Cell(Field::Ring(0, 1)));
        l.wheel(x, y, 1.0, Mods::default());
        assert_eq!(saved(&l).ring[1], 16);
        // The output column stays the modulation matrix's.
        centre(&l, &Target::Cell(Field::Cell(0, COL_OUT)));
        // A row label selects the module's page.
        let (x, y) = centre(&l, &Target::Row(6));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(l.view.page, 7);
    }

    /// Boxes step by 8 px; the unison order box goes 1..9; switches toggle; the EQ target picks.
    #[test]
    fn boxes_switches_and_choices() {
        let mut l = logic();
        let (x, y) = centre(&l, &Target::Box(Field::S(8)));
        l.down(x, y, Btn::Left, Mods::default(), false);
        l.delta(0.0, -20.0, Mods::default());
        l.up(Btn::Left);
        assert_eq!(saved(&l).s[8], 1 + 3);
        let (x, y) = centre(&l, &Target::Switch(Field::S32(1)));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert!(saved(&l).s32(1));
        let (x, y) = centre(&l, &Target::Choice(Field::EqMode, 2));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(saved(&l).eq.mode, 2);
        assert_eq!(l.hint, "Equalizer target: out only");
        let (x, y) = centre(&l, &Target::Cycle(Field::Eq(1, 3)));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(saved(&l).eq.bands[1][3], 2);
        let (x, y) = centre(&l, &Target::Pad);
        l.down(x, y, Btn::Left, Mods::default(), false);
        l.moved(view::PAD.right(), view::PAD.y, Mods::default());
        l.up(Btn::Left);
        assert_eq!((saved(&l).s[25], saved(&l).s[26]), (512, 512));
    }

    /// The Options menu's switches, a parameter menu's Reset and Type in.
    #[test]
    fn menus() {
        let mut l = logic();
        let m = l.menu(&MenuReq::Options);
        assert_eq!(m.find("Blurred"), Some(Cmd::Set(Field::Switch(11), 3)));
        assert_eq!(m.find("2 semitones limit"), Some(Cmd::Set(Field::Switch(7), 4)));
        assert_eq!(m.find("Link velocity to attack length"), Some(Cmd::Set(Field::Switch(9), 2)));
        l.pick(&MenuReq::Options, Cmd::Set(Field::Switch(3), 1), 0.0, 0.0);
        assert_eq!(saved(&l).switch(3), 1);
        let m = l.menu(&MenuReq::Param(Field::Main(0)));
        assert_eq!(m.find("Type in value..."), Some(Cmd::TypeIn(Field::Main(0))));
        l.typed(&TypeTarget::Field(Field::Main(0)), "50%");
        assert_eq!(saved(&l).main_volume, 64);
        l.pick(&MenuReq::Param(Field::Main(0)), Cmd::Reset(Field::Main(0)), 0.0, 0.0);
        assert_eq!(saved(&l).main_volume, 128);
    }

    /// A state loaded from outside shows; an edit made against an older version is dropped.
    #[test]
    fn states_from_outside_are_read_again() {
        let mut l = logic();
        let mut st = crate::preset::default_patch();
        st.name = "Loaded".into();
        st.main_volume = 100;
        l.params.set_state(st.to_bytes(None));
        assert!(l.poll());
        assert_eq!((l.st.name.as_str(), l.st.main_volume), ("Loaded", 100));
        let v = l.params.state_version.load(Ordering::Acquire);
        assert!(!l.params.edited(v - 1, Vec::new(), Patch::new(st)));
        assert_eq!(saved(&l).name, "Loaded");
    }

    /// The keyboard shows the notes the host plays.
    #[test]
    fn keyboard_shows_host_notes() {
        let mut l = logic();
        assert!(!l.poll());
        l.params.playing[0].store(1 << 60, Ordering::Relaxed);
        assert!(l.poll(), "a note on repaints");
        assert!(!l.poll());
        assert_eq!(l.view.playing, [1 << 60, 0]);
        l.params.playing[0].store(0, Ordering::Relaxed);
        assert!(l.poll(), "a note off repaints");
        assert_eq!(l.view.playing, [0, 0]);
    }

    /// The keyboard plays notes and hides; the header opens the info page.
    #[test]
    fn keyboard_and_header() {
        let mut l = logic();
        let (x, y) = centre(&l, &Target::Key(60));
        assert!(l.down(x, y, Btn::Left, Mods::default(), false).contains(&Act::Note(60, true)));
        assert!(l.up(Btn::Left).contains(&Act::Note(60, false)));
        let (x, y) = centre(&l, &Target::Header(3));
        assert!(l.down(x, y, Btn::Left, Mods::default(), false).contains(&Act::Resize));
        assert_eq!(l.height(), view::H_NO_KEYS);
        paint(&mut l);
        assert!(!l.hits.iter().any(|h| matches!(h.t, Target::Key(_))));
        let (x, y) = centre(&l, &Target::Header(1));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(l.view.cover, Cover::Info);
        paint(&mut l);
        l.down(10.0, 10.0, Btn::Left, Mods::default(), false);
        assert_eq!(l.view.cover, Cover::None);
    }

    /// `CITRUS_PNG=<dir> cargo test -p citrus -- --ignored render_editor`: the editor as PNGs.
    #[test]
    #[ignore]
    fn render_editor() {
        let Some(dir) = std::env::var_os("CITRUS_PNG") else { return };
        let save = |l: &mut Logic, name: &str, scale: f32| {
            let (w, h) = ((view::W as f32 * scale) as u32, (l.height() as f32 * scale) as u32);
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
        save(&mut l, "citrus_default", 1.0);
        let deep = r"C:\Program Files\Image-Line\FL Studio 2026\Data\Patches\Plugin presets\Generators\Sytrus\Bass\Deep.fst";
        if let Some(fl) = std::fs::read(deep).ok().and_then(|b| crate::preset::state_from_fst(&b)) {
            l.params.set_state(fl);
            l.poll();
            l.hint = "Master level: 100%".into();
            save(&mut l, "citrus_deep", 1.0);
            l.hint.clear();
            l.view.ring = true;
            save(&mut l, "citrus_deep_rm", 1.0);
            l.view.ring = false;
            l.view.page = 1;
            save(&mut l, "citrus_deep_op1", 1.0);
            // A point hovered, the snap and slide tools on.
            paint(&mut l);
            if let Some(h) = l.hits.iter().find(|h| h.t == Target::Point(1)) {
                l.view.mouse = Some((h.r.cx(), h.r.cy()));
            }
            l.view.tools = [false, false, true, true];
            save(&mut l, "citrus_deep_op1_point", 1.0);
            l.view.tools = [false; 4];
            l.view.mouse = None;
            // Every point selected, then the Level scaling dialog.
            l.key(0x41, Mods { ctrl: true, ..Mods::default() });
            save(&mut l, "citrus_deep_op1_sel", 1.0);
            if let Some(id) = l.curve_id() {
                l.open_scaling(id);
                l.scale_set(1, 0.5);
                save(&mut l, "citrus_deep_op1_scale", 1.0);
                l.scale_button(2);
            }
            l.view.target[1] = 6;
            save(&mut l, "citrus_deep_op1_osc", 1.0);
            l.view.harm_scroll = 40;
            save(&mut l, "citrus_deep_op1_osc_scrolled", 1.0);
            l.view.harm_scroll = 0;
            l.view.target[1] = 1;
            l.view.part[1] = 1;
            save(&mut l, "citrus_deep_op1_lfo", 1.0);
            l.view.page = 7;
            l.view.target[7] = 2;
            save(&mut l, "citrus_deep_filter1", 1.0);
            l.view.target[7] = 7;
            save(&mut l, "citrus_deep_filter1_ws", 1.0);
            l.view.page = view::PAGE_FX;
            save(&mut l, "citrus_deep_fx", 1.0);
            l.view.fx_unit = 3;
            save(&mut l, "citrus_deep_fx_reverb", 1.0);
            l.view.page = 0;
            save(&mut l, "citrus_deep_x2", 2.0);
        }
        l.view.cover = Cover::Diagram;
        save(&mut l, "citrus_diagram", 1.0);
        l.view.cover = Cover::Info;
        save(&mut l, "citrus_info", 1.0);
    }

    /// The module pages' controls sit where FL has them (plugins/sytrus-editor.md §3 .. §5).
    #[test]
    fn module_pages_sit_where_fl_has_them() {
        let mut l = logic();
        let at = |l: &Logic, t: Target, x: f32, y: f32| {
            let h = l.hits.iter().find(|h| h.t == t).unwrap_or_else(|| panic!("{t:?}"));
            assert!(h.r.contains(x, y), "{t:?} at {:?}", h.r);
        };
        l.view.page = 2;
        paint(&mut l);
        for (t, x, y) in [
            (Target::Slider(Field::Op(1, 3)), 106.0, 84.0),
            (Target::Slider(Field::Op(1, 2)), 239.0, 84.0),
            (Target::Switch(Field::Op(1, 8)), 20.0, 113.0),
            (Target::Switch(Field::Op(1, 14)), 267.0, 114.0),
            (Target::Knob(Field::Head(1, 0)), 355.0, 92.0),
            (Target::Knob(Field::Head(1, 2)), 513.0, 92.0),
            (Target::Wave(1), 40.0, 60.0),
            (Target::ArtTab(6), 520.0, 138.0),
            (Target::PartTab(0), 30.0, 157.0),
            (Target::Switch(Field::Part(1, OP_VOLUME, 0, 0)), 15.0, 320.0),
            (Target::Knob(Field::Part(1, OP_VOLUME, 0, 4)), 104.0, 323.0),
            (Target::Switch(Field::Part(1, OP_VOLUME, 0, 6)), 123.0, 335.0),
        ] {
            at(&l, t, x, y);
        }
        assert!(l.hits.iter().any(|h| matches!(h.t, Target::Digit(Field::Op(1, 0), 1)) && h.r.x > 440.0));
        l.view.page = 8;
        paint(&mut l);
        for (t, x, y) in [
            (Target::Box(Field::Filt(7, 3)), 39.0, 48.0),
            (Target::Choice(Field::Head(7, 0), 2), 86.0, 71.0),
            (Target::Knob(Field::Head(7, 2)), 198.0, 56.0),
            (Target::Knob(Field::Head(7, 9)), 516.0, 56.0),
            (Target::Switch(Field::Filt(7, 2)), 481.0, 112.0),
            (Target::ArtTab(8), 520.0, 138.0),
        ] {
            at(&l, t, x, y);
        }
        l.view.page = view::PAGE_FX;
        paint(&mut l);
        for (t, x, y) in [
            (Target::Knob(Field::S(15)), 21.0, 56.0),
            (Target::Slider(Field::S(22)), 206.0, 84.0),
            (Target::Box(Field::S(24)), 240.0, 52.0),
            (Target::FxUnit(3), 309.0, 101.0),
            (Target::Slider(Field::Delay(0, 1)), 412.0, 84.0),
            (Target::Choice(Field::Delay(0, 4), 2), 498.0, 79.0),
            (Target::Switch(Field::Delay(0, 7)), 345.0, 110.0),
            (Target::ArtTab(1), 400.0, 138.0),
        ] {
            at(&l, t, x, y);
        }
        let (x, y) = centre(&l, &Target::FxUnit(3));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(l.hint, "Effect: reverb unit");
        paint(&mut l);
        at(&l, Target::Slider(Field::Reverb(8)), 520.0, 84.0);
        at(&l, Target::Box(Field::Reverb(7)), 341.0, 65.0);
    }

    /// Operator controls: the digits of the ratio, the shape slider, the waveform menu, the target and part tabs
    /// and the envelope bar edit the saved state.
    #[test]
    fn operator_controls_edit_the_saved_state() {
        let mut l = logic();
        l.view.page = 1;
        paint(&mut l);
        // The ratio's first decimal (1000 = 0.1 x): 30 px up = three steps.
        let h = l.hits.iter().find(|h| h.t == Target::Digit(Field::Op(0, 0), 1000)).expect("the ratio's first decimal").r;
        l.down(h.cx(), h.cy(), Btn::Left, Mods::default(), false);
        for _ in 0..30 {
            l.delta(0.0, -1.0, Mods::default());
        }
        l.up(Btn::Left);
        assert_eq!(saved(&l).modules[0].op.ratio, 13000);
        l.moved(h.cx(), h.cy(), Mods::default());
        assert_eq!(l.hint, "Freq ratio: 1.3000 x");
        let (x, y) = centre(&l, &Target::Wave(0));
        let acts = l.down(x, y, Btn::Right, Mods::default(), false);
        assert!(acts.contains(&Act::Menu(MenuReq::Wave(0), x, y)));
        l.pick(&MenuReq::Wave(0), Cmd::Set(Field::Op(0, 3), 256), x, y);
        assert_eq!(saved(&l).modules[0].op.shape[0], 256);
        let (x, y) = centre(&l, &Target::Switch(Field::Op(0, 14)));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(saved(&l).modules[0].op.flags[PLUCKED], 1);
        // VOL, then its envelope: on, its attack scale, tempo.
        let (x, y) = centre(&l, &Target::ArtTab(1));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(l.hint, "Editor target: volume");
        paint(&mut l);
        let (x, y) = centre(&l, &Target::Switch(Field::Part(0, OP_VOLUME, 0, 0)));
        l.down(x, y, Btn::Left, Mods::default(), false);
        let (x, y) = centre(&l, &Target::Knob(Field::Part(0, OP_VOLUME, 0, 1)));
        l.wheel(x, y, -1.0, Mods::default());
        assert_eq!(l.hint, "Attack time scale: 94%");
        let (x, y) = centre(&l, &Target::Switch(Field::Part(0, OP_VOLUME, 0, 5)));
        l.down(x, y, Btn::Left, Mods::default(), false);
        let env = &saved(&l).modules[0].arts[OP_VOLUME].env;
        assert!(env.enabled);
        assert_eq!((env.scales[0], env.flags & crate::curve::SYNCED), (120, crate::curve::SYNCED));
        // The LFO's part tab and its speed.
        let (x, y) = centre(&l, &Target::PartTab(1));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(l.hint, "Articulator part: LFO");
        paint(&mut l);
        let (x, y) = centre(&l, &Target::Knob(Field::Part(0, OP_VOLUME, 1, 1)));
        l.wheel(x, y, 1.0, Mods::default());
        assert_eq!(saved(&l).modules[0].arts[OP_VOLUME].lfo.lfo[0], 512 + 2032);
    }

    /// Filter and FX controls: the type box, the stages, a knob, the delay and reverb units.
    #[test]
    fn filter_and_fx_controls_edit_the_saved_state() {
        let mut l = logic();
        l.view.page = 7;
        paint(&mut l);
        let (x, y) = centre(&l, &Target::Box(Field::Filt(6, 3)));
        l.wheel(x, y, 1.0, Mods::default());
        assert_eq!(l.hint, "Filter type: Lime low pass");
        let (x, y) = centre(&l, &Target::Choice(Field::Head(6, 0), 2));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(l.hint, "Filter mode: dual");
        let (x, y) = centre(&l, &Target::Knob(Field::Head(6, 2)));
        l.down(x, y, Btn::Left, Mods::default(), false);
        for _ in 0..125 {
            l.delta(0.0, -1.0, Mods::default());
        }
        l.up(Btn::Left);
        let st = saved(&l);
        assert_eq!((st.modules[6].filter.kind, st.modules[6].header[0], st.modules[6].header[2]), (1, 2, 128));
        assert_eq!(l.hint, "Cutoff frequency: 50%");
        l.view.page = view::PAGE_FX;
        paint(&mut l);
        let (x, y) = centre(&l, &Target::FxUnit(1));
        l.down(x, y, Btn::Left, Mods::default(), false);
        paint(&mut l);
        let (x, y) = centre(&l, &Target::Switch(Field::Delay(1, 7)));
        l.down(x, y, Btn::Left, Mods::default(), false);
        let (x, y) = centre(&l, &Target::Choice(Field::Delay(1, 4), 2));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(l.hint, "Delay feedback mode: ping-pong");
        let (x, y) = centre(&l, &Target::Switch(Field::Delay(1, 5)));
        l.down(x, y, Btn::Left, Mods::default(), false);
        let st = saved(&l);
        assert!(st.delays[1].on && st.delays[1].synced && !st.delays[0].on);
        assert_eq!(st.delays[1].mode, 2);
        ctl::set(&mut l.st, Field::Delay(1, 1), 144);
        assert_eq!(ctl::hint(&l.st, Field::Delay(1, 1), false), "Delay time: 3:00");
        let (x, y) = centre(&l, &Target::FxUnit(3));
        l.down(x, y, Btn::Left, Mods::default(), false);
        paint(&mut l);
        let (x, y) = centre(&l, &Target::Slider(Field::Reverb(5)));
        l.moved(x, y, Mods::default());
        assert_eq!(l.hint, "Reverb decay");
        l.wheel(x, y, 1.0, Mods::default());
        assert!(l.hint.starts_with("Reverb decay"), "{}", l.hint);
        let (x, y) = centre(&l, &Target::Box(Field::S(24)));
        l.wheel(x, y, 1.0, Mods::default());
        assert_eq!(saved(&l).s[24], ctl::get(&crate::preset::default_patch(), Field::S(24)) + 1);
    }

    /// The curve view: a right click on empty space adds a point, a drag moves it, its menu sets its mode and a
    /// marker, a tension handle bends its segment, snap lands it on the grid, freeze hides the points, and the
    /// Options menu's state copy and tools work.
    #[test]
    fn curve_view_edits_points() {
        let mut l = logic();
        l.view.page = 1;
        paint(&mut l);
        let env = |l: &Logic| saved(l).modules[0].arts[OP_VOLUME].env.clone();
        let id = CurveId::Part(0, OP_VOLUME, 0);
        let n = l.st.modules[0].arts[OP_VOLUME].env.points.len();
        let inner = view::curve_inner();
        // Right of the last point (the view spans the curve and a tenth more).
        l.down(inner.right() - 4.0, inner.y + inner.h * 0.25, Btn::Right, Mods::default(), false);
        let c = env(&l);
        assert_eq!(c.points.len(), n + 1);
        assert!((c.points[n].level - 0.75).abs() < 0.01, "{}", c.points[n].level);
        // Drag it to the top.
        paint(&mut l);
        let (px, py) = centre(&l, &Target::Point(n));
        let acts = l.down(px, py, Btn::Left, Mods::default(), false);
        assert!(acts.contains(&Act::Capture));
        assert!(l.hint.starts_with(&format!("Point {}/{}:", n + 1, n + 1)), "{}", l.hint);
        l.moved(px, inner.y - 10.0, Mods::default());
        assert!(l.hint.ends_with("\t100%"), "{}", l.hint);
        l.up(Btn::Left);
        let c = env(&l);
        assert_eq!(c.points[n].level, 1.0);
        assert!((edit::times(&c)[n] - edit::times(&l.st.modules[0].arts[OP_VOLUME].env)[n]).abs() < 1e-9);
        // Its menu.
        paint(&mut l);
        let (px, py) = centre(&l, &Target::Point(n));
        let acts = l.down(px, py, Btn::Right, Mods::default(), false);
        let req = MenuReq::Point(id, n);
        assert!(acts.contains(&Act::Menu(req.clone(), px, py)));
        let _ = l.menu(&req);
        l.pick(&req, Cmd::Curve(id, CurveCmd::Mode(n, 10)), px, py);
        l.pick(&req, Cmd::Curve(id, CurveCmd::Marker(n, 2)), px, py);
        let c = env(&l);
        assert_eq!((c.points[n].mode, c.sustain), (10, n as i32));
        // A tension handle: up raises the segment's middle.
        l.pick(&req, Cmd::Curve(id, CurveCmd::Mode(n, 0)), px, py);
        paint(&mut l);
        let before = edit::segment_level(&l.st.modules[0].arts[OP_VOLUME].env, n, 0.5);
        let (tx, ty) = centre(&l, &Target::Tension(n));
        let acts = l.down(tx, ty, Btn::Left, Mods::default(), false);
        assert!(acts.contains(&Act::Lock));
        l.delta(0.0, -30.0, Mods::default());
        assert!(l.hint.starts_with(&format!("Point {} to {} tension\t", n, n + 1)), "{}", l.hint);
        l.up(Btn::Left);
        let c = env(&l);
        assert!(edit::segment_level(&c, n, 0.5) > before);
        assert!(c.points[n].tension != 0.0);
        // Snap: the level lands on a sixteenth.
        let (sx, sy) = centre(&l, &Target::Tool(2));
        l.down(sx, sy, Btn::Left, Mods::default(), false);
        assert_eq!(l.hint, "Snap to grid");
        paint(&mut l);
        let (px, py) = centre(&l, &Target::Point(n));
        l.down(px, py, Btn::Left, Mods::default(), false);
        l.moved(px, inner.bottom() - inner.h * 0.53, Mods::default());
        l.up(Btn::Left);
        assert_eq!(env(&l).points[n].level, 0.5);
        // The Options menu: copy the state, paste it into the LFO, flip it.
        let (ox, oy) = centre(&l, &Target::CurveOptions);
        let acts = l.down(ox, oy, Btn::Left, Mods::default(), false);
        assert!(acts.contains(&Act::Menu(MenuReq::Curve(id), ox, oy)));
        let _ = l.menu(&MenuReq::Curve(id));
        l.pick(&MenuReq::Curve(id), Cmd::Curve(id, CurveCmd::CopyState), ox, oy);
        l.pick(&MenuReq::Curve(id), Cmd::Curve(id, CurveCmd::Flip), ox, oy);
        assert_eq!(env(&l).points[n].level, 0.5);
        assert_eq!(env(&l).points[0].level, 1.0 - c.points[0].level);
        let lfo = CurveId::Part(0, OP_VOLUME, 1);
        let lfo_before = l.st.modules[0].arts[OP_VOLUME].lfo.clone();
        l.pick(&MenuReq::Curve(lfo), Cmd::Curve(lfo, CurveCmd::PasteState), ox, oy);
        assert_eq!(l.st.modules[0].arts[OP_VOLUME].lfo, lfo_before, "a state pastes into a curve of its kind only");
        // Freeze hides the points.
        let (fx, fy) = centre(&l, &Target::Tool(0));
        l.down(fx, fy, Btn::Left, Mods::default(), false);
        paint(&mut l);
        assert!(!l.hits.iter().any(|h| matches!(h.t, Target::Point(_) | Target::Tension(_))));
        // A filter's wave shaper: a map, its ends kept.
        l.view.tools = [false; 4];
        l.view.page = 7;
        l.view.target[7] = 7;
        paint(&mut l);
        let ws = l.st.modules[6].filter.shaper.points.len();
        l.down(inner.x + inner.w * 0.5, inner.y + inner.h * 0.5, Btn::Right, Mods::default(), false);
        let s = saved(&l).modules[6].filter.shaper.clone();
        assert_eq!(s.points.len(), ws + 1);
        assert!((edit::times(&s).last().unwrap() - 1.0).abs() < 1e-6);
    }

    /// The curve view's points selection: keys, a click, the rubber band, a group drag, the menu's enabling, Cut and
    /// Paste; a point's Type in value; the hint's forms.
    #[test]
    fn curve_points_selection() {
        let mut l = logic();
        l.view.page = 1;
        paint(&mut l);
        let id = CurveId::Part(0, OP_VOLUME, 0);
        let env = |l: &Logic| l.st.modules[0].arts[OP_VOLUME].env.clone();
        // Four points to work on.
        let inner = view::curve_inner();
        for k in 1..=3 {
            let x = inner.right() - 40.0 + 10.0 * k as f32;
            l.down(x, inner.y + inner.h * 0.5, Btn::Right, Mods::default(), false);
        }
        let n = env(&l).points.len();
        assert!(n >= 4);
        let ctrl = Mods { ctrl: true, ..Mods::default() };
        let enabled = |l: &Logic, label: &str| {
            let m = l.menu(&MenuReq::Curve(id));
            m.items.iter().any(|it| it.clickable() && matches!(it, MenuItem::Item { label: s, .. } if s == label))
        };
        assert!(!enabled(&l, "Copy") && enabled(&l, "Paste (insert)"), "nothing selected");
        assert_eq!(l.key(0x41, ctrl), vec![Act::Repaint]);
        paint(&mut l);
        assert!(l.view.selected.iter().all(|&s| s) && l.view.selected.len() == n);
        assert!(enabled(&l, "Duplicate") && enabled(&l, "Cut"));
        l.key(0x49, ctrl);
        paint(&mut l);
        assert!(l.view.selected.iter().all(|&s| !s), "inverted");
        // A click selects one point; Copy is on, Duplicate needs two.
        let (px, py) = centre(&l, &Target::Point(n - 1));
        l.down(px, py, Btn::Left, Mods::default(), false);
        l.up(Btn::Left);
        assert_eq!(edit::count(&l.selection(id)), 1);
        assert!(enabled(&l, "Copy") && !enabled(&l, "Duplicate"));
        // The rubber band over the last three points, then a group drag down.
        paint(&mut l);
        let (ax, _) = centre(&l, &Target::Point(n - 3));
        l.down(ax - 3.0, inner.y + 1.0, Btn::Left, ctrl, false);
        l.moved(inner.right() - 1.0, inner.bottom() - 1.0, ctrl);
        paint(&mut l);
        assert!(l.view.band.is_some());
        l.up(Btn::Left);
        let sel = l.selection(id);
        assert_eq!((edit::count(&sel), sel[n - 3], sel[n - 4]), (3, true, false));
        paint(&mut l);
        let before = env(&l);
        let (px, py) = centre(&l, &Target::Point(n - 2));
        l.down(px, py, Btn::Left, Mods::default(), false);
        l.moved(px, py + inner.h * 0.25, Mods::default());
        l.up(Btn::Left);
        let after = env(&l);
        for i in n - 3..n {
            assert!((after.points[i].level - (before.points[i].level - 0.25).max(0.0)).abs() < 1e-4, "point {i}");
        }
        assert_eq!(after.points[n - 4].level, before.points[n - 4].level);
        // Duplicate: three more points, selected; Delete removes them.
        l.key(0x42, ctrl);
        assert_eq!(env(&l).points.len(), n + 3);
        assert_eq!(edit::count(&l.selection(id)), 3);
        assert_eq!(l.selection(id)[n], true);
        l.key(0x2E, Mods::default());
        assert_eq!(env(&l).points.len(), n);
        assert_eq!(saved(&l).modules[0].arts[OP_VOLUME].env.points.len(), n);
        // Type in value: the level's text, a percentage typed.
        let acts = l.pick(&MenuReq::Point(id, 1), Cmd::Curve(id, CurveCmd::TypeIn(1)), 5.0, 6.0);
        let t = TypeTarget::Point(id, 1);
        assert_eq!(acts, vec![Act::TypeIn(t, 5.0, 6.0)]);
        l.typed(&t, "50");
        assert_eq!(env(&l).points[1].level, 0.5);
        assert_eq!(l.value_text(&t), "0.5");
        l.typed(&t, "1,5");
        assert_eq!(env(&l).points[1].level, 0.5);
        // The hint's right part: VOL unsigned, an LFO signed, PAN as left / right.
        assert!(l.point_hint(1).ends_with("\t50%"), "{}", l.point_hint(1));
        assert_eq!(Logic::level_hint(CurveId::Part(0, OP_VOLUME, 1), 0.5), "0%");
        assert_eq!(Logic::level_hint(CurveId::Part(0, OP_PAN, 0), 0.0), "100 left");
        assert_eq!(Logic::level_hint(CurveId::Part(6, F_CUTOFF, 0), 1.0), "100%");
        assert_eq!(Logic::level_hint(CurveId::Part(0, OP_PITCH, 2), 0.25), "-50%");
    }

    /// Scale levels...: the dialog blocks the editor, its controls move the curve, close reverts, Accept keeps.
    #[test]
    fn scale_levels_dialog() {
        let mut l = logic();
        l.view.page = 1;
        paint(&mut l);
        let id = CurveId::Part(0, OP_VOLUME, 0);
        let env = |l: &Logic| l.st.modules[0].arts[OP_VOLUME].env.clone();
        let orig = env(&l);
        let (ox, oy) = centre(&l, &Target::CurveOptions);
        let m = l.menu(&MenuReq::Curve(id));
        assert_eq!(m.find("Scale levels..."), Some(Cmd::Curve(id, CurveCmd::Scale)));
        l.pick(&MenuReq::Curve(id), Cmd::Curve(id, CurveCmd::Scale), ox, oy);
        paint(&mut l);
        assert_eq!(l.hit(10.0, 10.0), Some(Target::Dialog), "the dialog blocks the rest");
        // The offset fader: 64 px up = +0.25.
        let (x, y) = centre(&l, &Target::ScaleCtl(2));
        assert!(l.down(x, y, Btn::Left, Mods::default(), false).contains(&Act::Lock));
        for _ in 0..64 {
            l.delta(0.0, -1.0, Mods::default());
        }
        assert_eq!(l.hint, "Offset: 25%");
        l.up(Btn::Left);
        let raised = env(&l);
        for (a, b) in raised.points.iter().zip(&orig.points) {
            assert!((a.level - (b.level + 0.25).min(1.0)).abs() < 1e-5);
        }
        let (x, y) = centre(&l, &Target::ScaleBtn(2));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!((env(&l), l.view.scale), (orig.clone(), None), "closing reverts");
        l.pick(&MenuReq::Curve(id), Cmd::Curve(id, CurveCmd::Scale), ox, oy);
        paint(&mut l);
        let (x, y) = centre(&l, &Target::ScaleCtl(1));
        l.down(x, y, Btn::Left, Mods::default(), false);
        l.delta(0.0, 64.0, Mods::default());
        l.up(Btn::Left);
        let (x, y) = centre(&l, &Target::ScaleBtn(1));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(l.view.scale, None);
        let kept = saved(&l).modules[0].arts[OP_VOLUME].env.clone();
        for (a, b) in kept.points.iter().zip(&orig.points) {
            assert!((a.level - b.level * 0.5).abs() < 1e-5, "multiplier 0.5 kept");
        }
    }

    /// The Options menu's oscillator items and Unison › Default.
    #[test]
    fn options_oscillator_items() {
        let mut l = logic();
        let m = l.menu(&MenuReq::Options);
        assert_eq!(m.find("Copy oscillator settings"), Some(Cmd::None), "not on the main page");
        assert_eq!(m.find("Morph oscillators 1 to 5"), Some(Cmd::Osc(OscCmd::Morph)));
        l.view.page = 2;
        let m = l.menu(&MenuReq::Options);
        assert_eq!(m.find("Randomize waveform settings"), Some(Cmd::Osc(OscCmd::Randomize(1))));
        l.pick(&MenuReq::Options, Cmd::Osc(OscCmd::Randomize(1)), 0.0, 0.0);
        let op = saved(&l).modules[1].op.clone();
        assert!(op.levels.iter().skip(1).any(|&v| v > 0.0));
        l.pick(&MenuReq::Options, Cmd::Osc(OscCmd::Copy(1)), 0.0, 0.0);
        l.pick(&MenuReq::Options, Cmd::Osc(OscCmd::Paste(2)), 0.0, 0.0);
        assert_eq!(saved(&l).modules[2].op, op);
        l.pick(&MenuReq::Options, Cmd::Osc(OscCmd::Reset(1)), 0.0, 0.0);
        let r = saved(&l).modules[1].op.clone();
        assert_eq!((r.levels[0], r.levels.iter().skip(1).any(|&v| v != 0.0), r.shape), (1.0, false, [0, 0, 0, 0, 128]));
        assert_eq!(l.menu(&MenuReq::Options).find("Default"), Some(Cmd::UnisonDefault));
        l.pick(&MenuReq::Options, Cmd::UnisonDefault, 0.0, 0.0);
        assert_eq!(saved(&l).s[8..=14], [4, 128, 100, 128, 0, 16, 0]);
    }

    /// The harmonics editor: a drag paints the bars it passes, the wheel scrolls, the Options menu's tools and the
    /// waveform menu's conversion edit the operator.
    #[test]
    fn harmonics_editor_paints_levels() {
        let mut l = logic();
        l.view.page = 1;
        l.view.target[1] = 6;
        paint(&mut l);
        let b = view::harm_bars();
        let (x0, _) = centre(&l, &Target::Harm(0));
        let (x4, _) = centre(&l, &Target::Harm(4));
        let acts = l.down(x0, b.bottom() - b.h * 0.5, Btn::Left, Mods::default(), false);
        assert!(acts.contains(&Act::Capture));
        l.moved(x4, b.y - 5.0, Mods::default());
        assert_eq!(l.hint, "Harmonic 5 magnitude: 100%\t+2oct");
        l.up(Btn::Left);
        let levels = saved(&l).modules[0].op.levels.clone();
        for (h, want) in [0.5, 0.625, 0.75, 0.875, 1.0].into_iter().enumerate() {
            assert!((levels[h] - want).abs() < 0.01, "harmonic {h}: {}", levels[h]);
        }
        l.wheel(x0, b.cy(), -1.0, Mods::default());
        paint(&mut l);
        let first = l.hits.iter().find(|h| matches!(h.t, Target::Harm(_))).unwrap();
        assert_eq!(first.t, Target::Harm(8));
        let (ox, oy) = centre(&l, &Target::HarmOptions);
        let acts = l.down(ox, oy, Btn::Left, Mods::default(), false);
        assert!(acts.contains(&Act::Menu(MenuReq::Harm(0), ox, oy)));
        let _ = l.menu(&MenuReq::Harm(0));
        l.pick(&MenuReq::Harm(0), Cmd::Harm(0, HarmCmd::Shift(true)), ox, oy);
        assert!((saved(&l).modules[0].op.levels[1] - 0.5).abs() < 0.01);
        l.pick(&MenuReq::Harm(0), Cmd::Harm(0, HarmCmd::RandomPhases), ox, oy);
        assert!(saved(&l).modules[0].op.phases.iter().any(|&p| p != 0.0));
        // The waveform menu: a saw over the painted harmonics, converted, plays the same cycle (normalised again: the
        // pre-filter had lowered its peak).
        l.pick(&MenuReq::Wave(0), Cmd::Set(Field::Op(0, 3), 256), ox, oy);
        let smooth = l.st.switch(2) != 0;
        let before = crate::wave::build(&l.st.modules[0].op, smooth).t;
        l.pick(&MenuReq::Wave(0), Cmd::Harm(0, HarmCmd::Convert), ox, oy);
        let op = saved(&l).modules[0].op.clone();
        assert_eq!(op.shape[0], 0);
        let after = crate::wave::build(&op, smooth).t;
        let n = crate::wave::N;
        let peak = |t: &[f32]| t[..n].iter().fold(0f32, |m, x| m.max(x.abs()));
        let (pb, pa) = (peak(&before), peak(&after));
        let rms = (before[..n].iter().zip(&after[..n]).map(|(a, b)| f64::from(a / pb - b / pa).powi(2)).sum::<f64>() / n as f64).sqrt();
        assert!(rms < 0.02, "rms difference {rms}");
    }
}
