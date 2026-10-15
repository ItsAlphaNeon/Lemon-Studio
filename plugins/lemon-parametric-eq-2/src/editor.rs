// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! The plugin editor: a child of the host's window (Win32, X11 on Linux, an NSView on macOS) that paints `view` with
//! the os-ui canvas, and the editor logic (platform independent): token / fader / knob drags with the cursor held in
//! place, wheel and reset gestures, mute / solo, A/B compare, the options and menus. Edits show and sound at once
//! (`os_plugkit::vals`).
//!
// spec: plugins/lemon-parametric-eq-2.md (§5 hints, §6 menus, §7 interaction), plugins/host-link.md

use std::any::Any;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::Arc;

use nih_plug::prelude::*;
use os_engine::effects::eq::{self, has_level, Band, Settings, BANDS, MASK_BYTE};
use os_plugkit::vals::Vals;
use os_plugkit::MenuDef;
use os_ui::kit::menu::{Mark, MenuItem, MenuStyle};

use crate::params::{self, byte, flag, freq_id, level_id, order_id, set_byte, set_flag, type_id, width_id, EqParams, Opt, MAIN_ID, ORDER_NAMES, TYPE_NAMES};
use crate::view::{self, Hit, Target, ViewState, MAIN_FADER};
use crate::Shared;

/// "Lock spare state" (Options menu) is not saved; it lasts while the plugin instance lives.
static LOCK_SPARE: AtomicBool = AtomicBool::new(false);

pub struct EqEditor {
    pub params: Arc<EqParams>,
    pub shared: Arc<Shared>,
    /// DPI scale as f32 bits.
    pub scale: AtomicU32,
    /// The open editor window (0 = closed): parameter changes post a repaint to it.
    #[cfg_attr(not(windows), allow(dead_code))]
    hwnd: Arc<AtomicIsize>,
    /// Linux and macOS: the open editor's thread waker, for the same.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    slot: os_plugkit::gui::Slot,
}

impl EqEditor {
    pub fn new(params: Arc<EqParams>, shared: Arc<Shared>) -> Self {
        EqEditor {
            params,
            shared,
            scale: AtomicU32::new(1f32.to_bits()),
            hwnd: Arc::new(AtomicIsize::new(0)),
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            slot: Default::default(),
        }
    }

    fn notify(&self) {
        #[cfg(windows)]
        win32::post_changed(self.hwnd.load(Ordering::Acquire));
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        os_plugkit::gui::notify(&self.slot);
    }
}

impl Editor for EqEditor {
    fn spawn(&self, parent: ParentWindowHandle, context: Arc<dyn GuiContext>) -> Box<dyn Any + Send> {
        let scale = f32::from_bits(self.scale.load(Ordering::Relaxed));
        match parent {
            #[cfg(windows)]
            ParentWindowHandle::Win32Hwnd(hwnd) => {
                Box::new(win32::open(hwnd, scale, self.params.clone(), self.shared.clone(), context, self.hwnd.clone()))
            }
            #[cfg(target_os = "linux")]
            ParentWindowHandle::X11Window(window) => Box::new(gui::open(window, scale, self.params.clone(), self.shared.clone(), context, &self.slot)),
            // macOS hosts set no scale (sizes are points): the editor paints at its window's backing scale.
            #[cfg(target_os = "macos")]
            ParentWindowHandle::AppKitNsView(view) => {
                let scale = os_plugkit::gui::backing_scale(view as usize);
                Box::new(gui::open(view as usize, scale, self.params.clone(), self.shared.clone(), context, &self.slot))
            }
            // Elsewhere no editor opens yet.
            _ => {
                let _ = (context, scale);
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

    fn param_value_changed(&self, _id: &str, _normalized_value: f32) {
        self.notify();
    }
    fn param_modulation_changed(&self, _id: &str, _modulation_offset: f32) {
        self.notify();
    }
    fn param_values_changed(&self) {
        self.notify();
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Laws (spec §7)

/// Pixels of drag for the whole range.
const FREQ_PX: f32 = 1024.0;
const LEVEL_PX: f32 = 640.0;
const WIDTH_PX: f32 = 256.0;
/// Pixels per step of a type / order drag.
const STEP_PX: f32 = 16.0;
const FINE: f32 = 0.25;

/// Note names in the Key menu's order and their semitone above C.
const KEY_NAMES: [(&str, i32); 12] = [("A", 9), ("A#", 10), ("B", 11), ("C", 0), ("C#", 1), ("D", 2), ("D#", 3), ("E", 4), ("F", 5), ("F#", 6), ("G", 7), ("G#", 8)];

/// Frequency of a note (`semitone` above C, scientific octave; A4 = 440 Hz).
pub fn note_hz(semitone: i32, octave: i32) -> f64 {
    440.0 * 2f64.powf(f64::from(12 * (octave + 1) + semitone - 69) / 12.0)
}

// ---------------------------------------------------------------------------------------------------------------
// Menus

/// What a menu entry does.
#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    None,
    ResetBand(usize),
    SetType(usize, i32),
    SetOrder(usize, i32),
    SetHz(usize, f64),
    Reset(String),
    CopyValue(String),
    PasteValue(String),
    TypeIn(String),
    /// "Create automation clip" (host link).
    Automate(String),
    /// Toggle a flag of the flags byte.
    Flag(u8),
    /// Set an index option.
    Opt(Opt, i32),
    /// Set a state byte.
    Byte(usize, u8),
    /// HQ mode: 3 HQ+, 1 Legacy, 2 Enhanced (stored as 1 + the Enhanced flag).
    Hq(u8),
    /// Toggle "Enhanced frequency" (stored inverted).
    Enhanced,
    LockSpare,
}

/// Which menu is open.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuReq {
    Token(usize),
    Param(String),
    Options,
    Lin,
    Hq,
    Phones,
}

fn radio(m: &mut MenuDef<Cmd>, label: &str, on: bool, c: Cmd) {
    m.add(MenuItem::item(label).mark(Mark::Radio(on)), c);
}

fn toggle(b: &mut [u8], f: u8) {
    let on = flag(b, f);
    set_flag(b, f, !on);
}

fn check(m: &mut MenuDef<Cmd>, label: &str, on: bool, c: Cmd) {
    m.add(MenuItem::item(label).mark(Mark::Check(on)), c);
}

/// A check entry for a setting nothing reads yet: greyed out.
fn check_off(m: &mut MenuDef<Cmd>, label: &str, on: bool) {
    m.add(MenuItem::item(label).mark(Mark::Check(on)).enabled(false), Cmd::None);
}

/// `m` with every entry greyed out (settings nothing reads yet).
fn greyed(mut m: MenuDef<Cmd>) -> MenuDef<Cmd> {
    m.items = m.items.into_iter().map(|it| it.enabled(false)).collect();
    m
}

fn opt_menu(extra: &[u8], o: Opt, names: &[&str], first: i32) -> MenuDef<Cmd> {
    let mut m = MenuDef::new(MenuStyle::Context);
    let cur = params::opt(extra, o);
    for (i, n) in names.iter().enumerate() {
        radio(&mut m, n, cur == i as i32 + first, Cmd::Opt(o, i as i32 + first));
    }
    m
}

fn key_menu(k: usize) -> MenuDef<Cmd> {
    let mut m = MenuDef::new(MenuStyle::Context);
    for (name, semi) in KEY_NAMES {
        let mut sub = MenuDef::new(MenuStyle::Context);
        for oct in 0..=10 {
            // Octaves count from C (scientific pitch): A0 = 27.5 Hz.
            let hz = note_hz(semi, oct);
            if (20.0..=20000.0).contains(&hz) {
                sub.add(MenuItem::item(&format!("{name}{oct} - {hz:.1}Hz")), Cmd::SetHz(k, hz));
            }
        }
        m.submenu(name, sub, Cmd::None);
    }
    m
}

// ---------------------------------------------------------------------------------------------------------------
// Editor logic

/// Something the window has to do after an event.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Repaint,
    /// Hide the cursor and hold it in place (drag).
    Lock,
    Menu(MenuReq, f32, f32),
    TypeIn(String, f32, f32),
    /// Ask the host for an automation clip on the parameter at this list position (host link).
    Automate(usize),
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

#[derive(Clone, Debug)]
enum Drag {
    /// Token: frequency across and level up / down, or width.
    Token { k: usize, width: bool },
    /// A parameter along `px` pixels for its whole (normalised) range, up = more.
    Linear { id: String, px: f32 },
    /// Stepped integer parameter: `acc` collects the pixels toward the next step.
    Steps { id: String, acc: f32 },
    /// Shift+click solo held: the mask to put back.
    Solo { restore: u8 },
}

pub struct Logic {
    pub vals: Vals,
    pub params: Arc<EqParams>,
    pub shared: Arc<Shared>,
    pub view: ViewState,
    pub hits: Vec<Hit>,
    drag: Option<Drag>,
    /// Hint text (a tab before the right-aligned part).
    pub hint: String,
    copied: Option<f32>,
    pub hosted: bool,
    /// HQ mode to turn back on (the HQ button toggles off and back).
    last_hq: u8,
}

impl Logic {
    pub fn new(params: Arc<EqParams>, shared: Arc<Shared>, ctx: Option<Arc<dyn GuiContext>>) -> Self {
        let vals = Vals::new(&*params, params.live.clone(), ctx);
        Logic { vals, params, shared, view: ViewState::default(), hits: Vec::new(), drag: None, hint: String::new(), copied: None, hosted: false, last_hq: 1 }
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// The values shown (pending edits included) with the enable mask.
    pub fn shown(&self) -> Settings {
        let v = &self.vals;
        Settings {
            bands: std::array::from_fn(|k| Band {
                db: f64::from(v.plain(&level_id(k))),
                freq: f64::from(v.plain(&freq_id(k))),
                width: f64::from(v.plain(&width_id(k))),
                kind: v.plain(&type_id(k)).round() as i32,
                order: v.plain(&order_id(k)).round() as i32,
            }),
            main_db: f64::from(v.plain(MAIN_ID)),
            mask: self.params.extra.read().map_or(0x7f, |b| params::mask(&b)),
        }
    }

    fn hit(&self, x: f32, y: f32) -> Option<Target> {
        self.hits.iter().rev().find(|h| h.r.contains(x, y)).map(|h| h.t)
    }

    fn band_hint(&self, k: usize) -> String {
        let b = self.shown().bands[k];
        format!(
            "{} {} {:.0}%\tBand {} ({})",
            params::db_text(b.db),
            params::hz_text(b.hz()),
            b.width * 100.0,
            k + 1,
            TYPE_NAMES[b.kind.clamp(0, 7) as usize].to_lowercase()
        )
    }

    fn hint_for(&self, t: Option<Target>, x: f32, y: f32) -> String {
        let extra = self.params.extra_copy();
        match t {
            Some(Target::Token(k) | Target::Fader(k) | Target::Freq(k) | Target::Bw(k) | Target::Switch(k)) if k < BANDS => {
                let h = self.band_hint(k);
                if t == Some(Target::Switch(k)) { format!("Mute / solo\t{}", h.split('\t').nth(1).unwrap_or("")) } else { h }
            }
            Some(Target::Fader(_)) => format!("Main level: {}", params::db_text(f64::from(self.vals.plain(MAIN_ID)))),
            Some(Target::TypeTab(k) | Target::OrderMark(k)) => {
                format!("Band {} order: {}", k + 1, ORDER_NAMES[(self.vals.plain(&order_id(k)).round() as i32 + 3).clamp(0, 6) as usize])
            }
            Some(Target::Graph) => format!("{}Hz {:.1}dB", eq::freq_hz(view::n_of(x)).round(), view::db_of(y)),
            Some(Target::Options) => "Options".into(),
            // Greyed out (`view::works`).
            Some(Target::Lin) => "Linear phase: not available yet".into(),
            Some(Target::Hq) => "High quality (oversampling): not available yet".into(),
            Some(Target::Tokens) => "View band tokens".into(),
            Some(Target::Phones) => "Band solo on shift".into(),
            Some(Target::Monitor(m)) => format!("Monitor: {}", ["off", "input", "output"][usize::from(m.min(2))]),
            Some(Target::Store) => if LOCK_SPARE.load(Ordering::Relaxed) { "Store in spare state (locked)".into() } else { "Store in spare state".into() },
            Some(Target::Flip) => "Flip with spare state (A/B compare)".into(),
            _ => {
                let _ = extra;
                String::new()
            }
        }
    }

    /// One complete edit of a band to `b` (only the parameters that differ).
    fn set_band(&mut self, k: usize, b: Band) {
        let cur = self.shown().bands[k];
        if cur.db != b.db {
            self.vals.edit_plain(&level_id(k), b.db as f32);
        }
        if cur.freq != b.freq {
            self.vals.edit_plain(&freq_id(k), b.freq as f32);
        }
        if cur.width != b.width {
            self.vals.edit_plain(&width_id(k), b.width as f32);
        }
        if cur.kind != b.kind {
            self.vals.edit_plain(&type_id(k), b.kind as f32);
        }
        if cur.order != b.order {
            self.vals.edit_plain(&order_id(k), b.order as f32);
        }
    }

    /// Reset (spec: level 0, default frequency, the type's default width; type and order stay).
    pub fn reset_band(&mut self, k: usize) {
        let mut b = self.shown().bands[k];
        b.db = 0.0;
        b.freq = eq::default_freq(k);
        b.width = eq::default_width(b.kind);
        self.set_band(k, b);
    }

    fn reset_param(&mut self, id: &str) {
        let d = self.vals.default_norm(id);
        self.vals.edit(id, d);
    }

    fn set_mask(&mut self, m: u8) {
        self.params.edit_extra(|b| set_byte(b, MASK_BYTE, m & 0x7f));
    }

    fn mask(&self) -> u8 {
        self.params.extra.read().map_or(0x7f, |b| params::mask(&b))
    }

    /// Spare bank: store the current settings in it, or swap them with it.
    pub fn compare(&mut self, flip: bool) {
        let cur = self.shown();
        let fields = params::fields_of(&cur);
        if !flip {
            if LOCK_SPARE.load(Ordering::Relaxed) {
                return;
            }
            self.params.edit_extra(|b| fields.iter().enumerate().for_each(|(i, &v)| params::set_field(b, 37 + i, v)));
            return;
        }
        let spare = params::settings_at(&self.params.extra_copy(), 37);
        self.params.edit_extra(|b| fields.iter().enumerate().for_each(|(i, &v)| params::set_field(b, 37 + i, v)));
        for k in 0..BANDS {
            self.set_band(k, spare.bands[k]);
        }
        if cur.main_db != spare.main_db {
            self.vals.edit_plain(MAIN_ID, spare.main_db as f32);
        }
    }

    pub fn down(&mut self, x: f32, y: f32, btn: Btn, m: Mods, _dbl: bool) -> Vec<Act> {
        self.view.mouse = Some((x, y));
        let Some(t) = self.hit(x, y) else { return vec![] };
        if !view::works(t) {
            // Greyed-out controls only say so.
            self.hint = self.hint_for(Some(t), x, y);
            return vec![Act::Repaint];
        }
        let extra = self.params.extra_copy();
        let mut acts = vec![Act::Repaint];
        let lock = |l: &mut Logic, d: Drag, ids: &[String]| {
            for id in ids {
                l.vals.begin(id);
            }
            l.drag = Some(d);
            l.view.pressed = Some(t);
        };
        match (t, btn) {
            (Target::Token(k), Btn::Left) if m.alt => self.reset_band(k),
            (Target::Token(k), Btn::Left) if m.shift && flag(&extra, params::F_SOLO_SHIFT) => {
                let restore = params::mask(&extra);
                self.set_mask(1 << k);
                self.drag = Some(Drag::Solo { restore });
                self.view.pressed = Some(t);
            }
            (Target::Token(k), Btn::Left | Btn::Middle) => {
                let width = btn == Btn::Middle || m.shift;
                let ids = if width { vec![width_id(k)] } else { vec![freq_id(k), level_id(k)] };
                lock(self, Drag::Token { k, width }, &ids);
                self.view.active = Some(k);
                acts.push(Act::Lock);
            }
            (Target::Token(k), Btn::Right) => acts.push(Act::Menu(MenuReq::Token(k), x, y)),
            (Target::Fader(k), Btn::Left) => {
                let id = if k == MAIN_FADER { MAIN_ID.to_string() } else { level_id(k) };
                lock(self, Drag::Linear { id: id.clone(), px: LEVEL_PX }, &[id]);
                acts.push(Act::Lock);
            }
            (Target::Graph, Btn::Left) => {
                lock(self, Drag::Linear { id: MAIN_ID.into(), px: LEVEL_PX }, &[MAIN_ID.into()]);
                acts.push(Act::Lock);
            }
            (Target::Freq(k), Btn::Left) => {
                lock(self, Drag::Linear { id: freq_id(k), px: FREQ_PX }, &[freq_id(k)]);
                acts.push(Act::Lock);
            }
            (Target::Bw(k), Btn::Left) => {
                lock(self, Drag::Linear { id: width_id(k), px: WIDTH_PX }, &[width_id(k)]);
                acts.push(Act::Lock);
            }
            (Target::TypeTab(k), Btn::Left) => {
                lock(self, Drag::Steps { id: type_id(k), acc: 0.0 }, &[type_id(k)]);
                acts.push(Act::Lock);
            }
            (Target::OrderMark(k), Btn::Left) => {
                lock(self, Drag::Steps { id: order_id(k), acc: 0.0 }, &[order_id(k)]);
                acts.push(Act::Lock);
            }
            (Target::Fader(k), Btn::Middle) => self.reset_param(&if k == MAIN_FADER { MAIN_ID.to_string() } else { level_id(k) }),
            (Target::Freq(k), Btn::Middle) => self.reset_param(&freq_id(k)),
            (Target::Bw(k), Btn::Middle) => self.reset_param(&width_id(k)),
            (Target::Fader(k), Btn::Right) => acts.push(Act::Menu(MenuReq::Param(if k == MAIN_FADER { MAIN_ID.into() } else { level_id(k) }), x, y)),
            (Target::Freq(k), Btn::Right) => acts.push(Act::Menu(MenuReq::Param(freq_id(k)), x, y)),
            (Target::Bw(k), Btn::Right) => acts.push(Act::Menu(MenuReq::Param(width_id(k)), x, y)),
            (Target::TypeTab(k), Btn::Right) => acts.push(Act::Menu(MenuReq::Param(type_id(k)), x, y)),
            (Target::OrderMark(k), Btn::Right) => acts.push(Act::Menu(MenuReq::Param(order_id(k)), x, y)),
            (Target::Switch(k), Btn::Left) => {
                let m = self.mask() ^ (1 << k);
                self.set_mask(m);
            }
            (Target::Switch(k), Btn::Right) => {
                let m = if self.mask() == 1 << k { 0x7f } else { 1 << k };
                self.set_mask(m);
            }
            (Target::Options, Btn::Left) => acts.push(Act::Menu(MenuReq::Options, x, y)),
            (Target::Lin, Btn::Left) => self.params.edit_extra(|b| toggle(b, params::F_LIN)),
            (Target::Lin, Btn::Right) => acts.push(Act::Menu(MenuReq::Lin, x, y)),
            (Target::Hq, Btn::Left) => {
                let cur = byte(&extra, params::HQ_BYTE);
                if cur != 0 {
                    self.last_hq = cur;
                }
                let next = if cur != 0 { 0 } else { self.last_hq.max(1) };
                self.params.edit_extra(|b| set_byte(b, params::HQ_BYTE, next));
            }
            (Target::Hq, Btn::Right) => acts.push(Act::Menu(MenuReq::Hq, x, y)),
            (Target::Tokens, Btn::Left) => self.params.edit_extra(|b| {
                let v = byte(b, params::TOKENS_BYTE);
                set_byte(b, params::TOKENS_BYTE, u8::from(v == 0));
            }),
            (Target::Phones, Btn::Left) => self.params.edit_extra(|b| toggle(b, params::F_SOLO_SHIFT)),
            (Target::Phones, Btn::Right) => acts.push(Act::Menu(MenuReq::Phones, x, y)),
            (Target::Monitor(i), Btn::Left) => self.params.edit_extra(|b| set_byte(b, params::MONITOR_BYTE, i)),
            (Target::Store, Btn::Left) => self.compare(false),
            (Target::Flip, Btn::Left) => self.compare(true),
            _ => {}
        }
        self.hint = self.hint_for(Some(t), x, y);
        acts
    }

    /// Mouse movement during a locked drag (logical px since the last event).
    pub fn delta(&mut self, dx: f32, dy: f32, m: Mods) -> Vec<Act> {
        let f = if m.ctrl { FINE } else { 1.0 };
        let Some(d) = self.drag.clone() else { return vec![] };
        match d {
            Drag::Token { k, width: true } => {
                let id = width_id(k);
                let v = self.vals.norm(&id) + (dx - dy) * f / WIDTH_PX;
                self.vals.set(&id, v);
            }
            Drag::Token { k, width: false } => {
                let fid = freq_id(k);
                let v = self.vals.norm(&fid) + dx * f / FREQ_PX;
                self.vals.set(&fid, v);
                if has_level(self.shown().bands[k].kind) {
                    let lid = level_id(k);
                    let v = self.vals.norm(&lid) - dy * f / LEVEL_PX;
                    self.vals.set(&lid, v);
                }
            }
            Drag::Linear { id, px } => {
                let v = self.vals.norm(&id) - dy * f / px;
                self.vals.set(&id, v);
            }
            Drag::Steps { id, acc } => {
                let acc = acc - dy;
                let steps = (acc / STEP_PX).trunc();
                if steps != 0.0 {
                    let plain = self.vals.plain(&id) + steps;
                    let n = self.vals.plain_to_norm(&id, plain);
                    self.vals.set(&id, n);
                }
                self.drag = Some(Drag::Steps { id, acc: acc - steps * STEP_PX });
            }
            Drag::Solo { .. } => return vec![],
        }
        let (x, y) = self.view.mouse.unwrap_or_default();
        self.hint = self.hint_for(self.view.pressed, x, y);
        vec![Act::Repaint]
    }

    pub fn up(&mut self, _btn: Btn) -> Vec<Act> {
        let Some(d) = self.drag.take() else { return vec![] };
        match d {
            Drag::Token { k, width: true } => self.vals.end(&width_id(k)),
            Drag::Token { k, width: false } => {
                self.vals.end(&freq_id(k));
                self.vals.end(&level_id(k));
            }
            Drag::Linear { id, .. } | Drag::Steps { id, .. } => self.vals.end(&id),
            Drag::Solo { restore } => self.set_mask(restore),
        }
        self.view.pressed = None;
        vec![Act::Repaint]
    }

    pub fn moved(&mut self, x: f32, y: f32, _m: Mods) -> Vec<Act> {
        self.view.mouse = Some((x, y));
        if self.drag.is_some() {
            return vec![Act::Repaint];
        }
        let t = self.hit(x, y);
        self.view.active = match t {
            Some(Target::Token(k)) => Some(k),
            _ => None,
        };
        self.hint = self.hint_for(t, x, y);
        vec![Act::Repaint]
    }

    pub fn leave(&mut self) -> Vec<Act> {
        self.view.mouse = None;
        self.view.active = None;
        self.hint.clear();
        vec![Act::Repaint]
    }

    pub fn wheel(&mut self, x: f32, y: f32, notches: f32, _m: Mods) -> Vec<Act> {
        let Some(t) = self.hit(x, y) else { return vec![] };
        let step = |l: &mut Logic, id: &str, d: f32| {
            let v = l.vals.norm(id) + d * notches;
            l.vals.edit(id, v);
        };
        match t {
            Target::Token(k) | Target::Bw(k) if k < BANDS => step(self, &width_id(k), 0.05),
            Target::Fader(k) => step(self, &if k == MAIN_FADER { MAIN_ID.to_string() } else { level_id(k) }, 0.5 / 36.0),
            Target::Freq(k) => step(self, &freq_id(k), 1.0 / 128.0),
            Target::TypeTab(k) | Target::OrderMark(k) => {
                let id = if matches!(t, Target::TypeTab(_)) { type_id(k) } else { order_id(k) };
                let plain = self.vals.plain(&id) + notches.signum();
                let n = self.vals.plain_to_norm(&id, plain);
                self.vals.edit(&id, n);
            }
            _ => return vec![],
        }
        self.hint = self.hint_for(Some(t), x, y);
        vec![Act::Repaint]
    }

    /// The parameter's position in the plugin's parameter list (its CLAP / VST3 index).
    pub fn param_index(&self, id: &str) -> Option<usize> {
        nih_plug::params::Params::param_map(&*self.params).iter().position(|(p, _, _)| p == id)
    }

    pub fn menu(&self, req: &MenuReq) -> MenuDef<Cmd> {
        let extra = self.params.extra_copy();
        let s = self.shown();
        let mut m = MenuDef::new(MenuStyle::Context);
        match req {
            MenuReq::Token(k) => {
                let k = *k;
                m.head(&format!("Band {}", k + 1), Cmd::None);
                m.add(MenuItem::item("Reset"), Cmd::ResetBand(k));
                let mut ty = MenuDef::new(MenuStyle::Context);
                for (i, n) in TYPE_NAMES.iter().enumerate() {
                    radio(&mut ty, n, s.bands[k].kind == i as i32, Cmd::SetType(k, i as i32));
                }
                m.submenu("Type", ty, Cmd::None);
                let mut or = MenuDef::new(MenuStyle::Context);
                for (i, n) in ORDER_NAMES.iter().enumerate() {
                    radio(&mut or, n, s.bands[k].order == i as i32 - 3, Cmd::SetOrder(k, i as i32 - 3));
                }
                m.submenu("Order", or, Cmd::None);
                m.submenu("Key", key_menu(k), Cmd::None);
            }
            MenuReq::Param(id) => {
                m.add(MenuItem::item("Reset"), Cmd::Reset(id.clone()));
                if let Some(k) = id.strip_prefix("freq").and_then(|n| n.parse::<usize>().ok()) {
                    m.submenu("Key", key_menu(k - 1), Cmd::None);
                }
                m.sep(Cmd::None);
                m.add(MenuItem::item("Copy value"), Cmd::CopyValue(id.clone()));
                m.add(MenuItem::item("Paste value").enabled(self.copied.is_some()), Cmd::PasteValue(id.clone()));
                m.add(MenuItem::item("Type in value..."), Cmd::TypeIn(id.clone()));
                // Automation belongs to the host: offered when Lemon Studio hosts the editor (host link).
                m.sep(Cmd::None);
                m.add(MenuItem::item("Create automation clip").enabled(self.hosted), Cmd::Automate(id.clone()));
            }
            MenuReq::Options => {
                m.head("Options", Cmd::None);
                check(&mut m, "Lock spare state", LOCK_SPARE.load(Ordering::Relaxed), Cmd::LockSpare);
                m.submenu("Band solo on Shift", self.solo_menu(&extra), Cmd::None);
                // Greyed out: nothing reads the mod speeds, phase rotation or the heatmap's enhanced frequency and high
                // precision yet.
                m.submenu("Minimum phase mod speed", greyed(opt_menu(&extra, Opt::MinPhaseSpeed, &["Off", "Medium", "Smooth"], 0)), Cmd::None);
                let mut lin = MenuDef::new(MenuStyle::Context);
                for (i, n) in ["Normal", "Fast", "Faster", "Ultra"].iter().enumerate() {
                    radio(&mut lin, n, byte(&extra, params::LIN_SPEED_BYTE) == i as u8, Cmd::Byte(params::LIN_SPEED_BYTE, i as u8));
                }
                m.submenu("Linear phase mod speed", greyed(lin), Cmd::None);
                m.head("Visualization", Cmd::None);
                check_off(&mut m, "Phase rotation", flag(&extra, params::F_PHASE_ROTATION));
                m.submenu("Monitor input", opt_menu(&extra, Opt::MonitorInput, &["Mid", "Side", "Left", "Right"], 1), Cmd::None);
                m.head("Histogram", Cmd::None);
                check(&mut m, "Enabled", flag(&extra, params::F_HISTOGRAM), Cmd::Flag(params::F_HISTOGRAM));
                m.submenu("Range", opt_menu(&extra, Opt::Range, &["-60dB", "-90dB", "-120dB"], 1), Cmd::None);
                m.submenu("Pivot slope", opt_menu(&extra, Opt::PivotSlope, &["0 dB/oct", "1.5 dB/oct", "3 dB/oct", "4.5 dB/oct", "6 dB/oct"], 0), Cmd::None);
                m.submenu("Frequency precision", opt_menu(&extra, Opt::FreqPrecision, &["Low", "Medium", "High", "Highest"], 0), Cmd::None);
                m.submenu("Time smoothing", opt_menu(&extra, Opt::TimeSmoothing, &["Fastest", "Fast", "Medium", "Slow", "Slowest"], 0), Cmd::None);
                check(&mut m, "Average mode", flag(&extra, params::F_AVERAGE), Cmd::Flag(params::F_AVERAGE));
                m.head("Heatmap", Cmd::None);
                check(&mut m, "Enabled", flag(&extra, params::F_HEATMAP), Cmd::Flag(params::F_HEATMAP));
                check_off(&mut m, "Enhanced frequency", byte(&extra, params::ENHANCED_OFF_BYTE) == 0);
                check_off(&mut m, "High precision", byte(&extra, params::HEAT_PRECISION_BYTE) != 0);
                m.submenu("Heatmap position", opt_menu(&extra, Opt::HeatmapPosition, &["Top", "Bottom", "Full"], 0), Cmd::None);
                m.sep(Cmd::None);
                let mut about = MenuDef::new(MenuStyle::Context);
                about.add(MenuItem::item("Lemon Parametric EQ").enabled(false), Cmd::None);
                about.add(MenuItem::item(&format!("Lemon Studio {} - GPLv3", env!("CARGO_PKG_VERSION"))).enabled(false), Cmd::None);
                about.add(MenuItem::item("Filters after the RBJ Audio EQ Cookbook").enabled(false), Cmd::None);
                m.submenu("About", about, Cmd::None);
            }
            MenuReq::Lin => {
                let cur = byte(&extra, params::LIN_PRECISION_BYTE);
                for (i, n) in ["Lowest", "Low", "Medium", "High", "Higher", "Highest"].iter().enumerate() {
                    radio(&mut m, n, cur == i as u8, Cmd::Byte(params::LIN_PRECISION_BYTE, i as u8));
                }
            }
            MenuReq::Hq => {
                let hq = byte(&extra, params::HQ_BYTE);
                let enh = flag(&extra, params::F_HQ_ENHANCED);
                radio(&mut m, "HQ+", hq & 2 != 0, Cmd::Hq(3));
                radio(&mut m, "HQ Legacy", hq & 2 == 0 && !enh && hq != 0, Cmd::Hq(1));
                radio(&mut m, "HQ Enhanced", hq & 2 == 0 && enh && hq != 0, Cmd::Hq(2));
            }
            MenuReq::Phones => m = self.solo_menu(&extra),
        }
        m
    }

    /// Solo mode and "Inherit bandwidth": greyed out, as no solo reads them yet (Shift+click solos the band as it is).
    fn solo_menu(&self, extra: &[u8]) -> MenuDef<Cmd> {
        let mut m = opt_menu(extra, Opt::SoloMode, &["Adaptive", "Bandpass"], 0);
        check(&mut m, "Inherit bandwidth", flag(extra, params::F_INHERIT_BW), Cmd::Flag(params::F_INHERIT_BW));
        greyed(m)
    }

    pub fn pick(&mut self, _req: &MenuReq, cmd: Cmd, x: f32, y: f32) -> Vec<Act> {
        match cmd {
            Cmd::None => {}
            Cmd::ResetBand(k) => self.reset_band(k),
            Cmd::SetType(k, t) => self.vals.edit_plain(&type_id(k), t as f32),
            Cmd::SetOrder(k, o) => self.vals.edit_plain(&order_id(k), o as f32),
            Cmd::SetHz(k, hz) => self.vals.edit_plain(&freq_id(k), eq::freq_pos(hz) as f32),
            Cmd::Reset(id) => self.reset_param(&id),
            Cmd::CopyValue(id) => self.copied = Some(self.vals.norm(&id)),
            Cmd::PasteValue(id) => {
                if let Some(v) = self.copied {
                    self.vals.edit(&id, v);
                }
            }
            Cmd::TypeIn(id) => return vec![Act::TypeIn(id, x, y)],
            Cmd::Automate(id) => return self.param_index(&id).map(Act::Automate).into_iter().collect(),
            Cmd::Flag(f) => self.params.edit_extra(|b| toggle(b, f)),
            Cmd::Opt(o, v) => self.params.edit_extra(|b| params::set_opt(b, o, v)),
            Cmd::Byte(i, v) => self.params.edit_extra(|b| set_byte(b, i, v)),
            Cmd::Hq(mode) => self.params.edit_extra(|b| {
                set_byte(b, params::HQ_BYTE, if mode == 3 { 3 } else { 1 });
                set_flag(b, params::F_HQ_ENHANCED, mode == 2);
            }),
            Cmd::Enhanced => self.params.edit_extra(|b| {
                let off = byte(b, params::ENHANCED_OFF_BYTE);
                set_byte(b, params::ENHANCED_OFF_BYTE, u8::from(off == 0));
            }),
            Cmd::LockSpare => {
                LOCK_SPARE.fetch_xor(true, Ordering::Relaxed);
            }
        }
        vec![Act::Repaint]
    }

    /// The value text a type-in box starts with.
    pub fn value_text(&self, id: &str) -> String {
        self.vals.text(id)
    }

    /// Applies a typed value.
    pub fn typed(&mut self, id: &str, text: &str) {
        if let Some(n) = self.vals.parse(id, text) {
            self.vals.edit(id, n);
        }
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
    use crate::analyzer::{Analyzer, Look as ALook};
    use crate::params::{self, EqParams, Opt};
    use crate::view::{self, Look, ViewState};
    use crate::Shared;

    const TIMER: usize = 1;
    const WM_CHANGED: u32 = WM_APP + 1;

    struct Win {
        hwnd: HWND,
        canvas: Canvas,
        buf: Vec<u32>,
        scale: f32,
        size: (u32, u32),
        params: Arc<EqParams>,
        shared: Arc<Shared>,
        logic: Logic,
        palette: Palette,
        analyzer: Analyzer,
        scope_seen: u64,
        last_sig: u64,
        last_extra: u64,
        last_view: ViewState,
        last_hint: String,
        lock: CursorLock,
        tracking: bool,
        slot: Arc<AtomicIsize>,
    }

    /// Owns the window; dropping it closes the editor.
    pub struct Handle(isize, Arc<AtomicIsize>, Arc<Shared>);

    // SAFETY: the handle is only an HWND value; the wrapper drops it on the GUI thread.
    unsafe impl Send for Handle {}

    impl Drop for Handle {
        fn drop(&mut self) {
            self.1.store(0, Ordering::Release);
            self.2.editor_open.store(false, Ordering::Relaxed);
            unsafe { DestroyWindow(self.0 as HWND) };
        }
    }

    pub fn post_changed(hwnd: isize) {
        if hwnd != 0 {
            unsafe { PostMessageW(hwnd as HWND, WM_CHANGED, 0, 0) };
        }
    }

    fn mods() -> Mods {
        let (ctrl, shift, alt) = pw::mods();
        Mods { ctrl, shift, alt }
    }

    pub fn open(parent: *mut c_void, scale: f32, params: Arc<EqParams>, shared: Arc<Shared>, ctx: Arc<dyn GuiContext>, slot: Arc<AtomicIsize>) -> Handle {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let size = ((view::W as f32 * scale).round() as u32, (view::H as f32 * scale).round() as u32);
        unsafe {
            let class = pw::register("LemonParametricEq2View", Some(wndproc));
            let win = Box::new(Win {
                hwnd: std::ptr::null_mut(),
                canvas: Canvas::new(size.0, size.1, scale).expect("canvas"),
                buf: vec![0; (size.0 * size.1) as usize],
                scale,
                size,
                logic: Logic::new(params.clone(), shared.clone(), Some(ctx)),
                params,
                shared: shared.clone(),
                palette: os_plugkit::palette(),
                analyzer: Analyzer::default(),
                scope_seen: 0,
                last_sig: 0,
                last_extra: u64::MAX,
                last_view: ViewState::default(),
                last_hint: String::new(),
                lock: CursorLock::default(),
                tracking: false,
                slot: slot.clone(),
            });
            let title = pw::wide("Lemon Parametric EQ");
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
            shared.editor_open.store(true, Ordering::Relaxed);
            SetTimer(hwnd, TIMER, 30, None);
            Handle(hwnd as isize, slot, shared)
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
                WM_TIMER | WM_CHANGED => {
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
                    if w.logic.dragging() && !w.lock.locked() {
                        SetCapture(hwnd);
                    }
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

        /// Analyzer settings, or `None` when nothing is monitored.
        fn analyzer_look(&self) -> Option<ALook> {
            let b = self.params.extra_copy();
            let on = params::byte(&b, params::MONITOR_BYTE) != 0 && (params::flag(&b, params::F_HISTOGRAM) || params::flag(&b, params::F_HEATMAP));
            on.then(|| ALook {
                precision: params::opt(&b, Opt::FreqPrecision).clamp(0, 3) as usize,
                slope: params::opt(&b, Opt::PivotSlope).clamp(0, 4) as usize,
                smoothing: params::opt(&b, Opt::TimeSmoothing).clamp(0, 4) as usize,
                average: params::flag(&b, params::F_AVERAGE),
            })
        }

        /// Timer / parameter change: expire confirmed overlay values, analyse new audio, repaint when anything shown
        /// changed.
        fn poll(&mut self) {
            let mut dirty = self.logic.vals.expire();
            if let Some(look) = self.analyzer_look() {
                let n = Analyzer::size_for(&look);
                let fresh = self.shared.scope.lock().ok().filter(|s| s.written != self.scope_seen).map(|s| (s.written, s.latest(n)));
                if let Some((written, samples)) = fresh {
                    self.scope_seen = written;
                    let rate = self.shared.rate.load(Ordering::Relaxed) as f32;
                    self.analyzer.update(&samples, rate, view::COLS, &look);
                    dirty = true;
                }
            } else if !self.analyzer.cols.is_empty() {
                self.analyzer.cols.clear();
                dirty = true;
            }
            let sig = self.logic.vals.signature();
            let ev = self.params.extra_version.load(Ordering::Acquire);
            if dirty || sig != self.last_sig || ev != self.last_extra || self.logic.view != self.last_view || self.logic.hint != self.last_hint {
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
                    Act::Menu(req, x, y) => {
                        let def = self.logic.menu(&req);
                        let (sx, sy) = self.screen(x, y);
                        if let Some(cmd) = pw::popup(self.hwnd, sx, sy, &def, &self.palette, self.scale) {
                            let acts = self.logic.pick(&req, cmd, x, y);
                            self.run(acts);
                        }
                        repaint = true;
                    }
                    Act::TypeIn(id, x, y) => {
                        let (sx, sy) = self.screen(x, y);
                        let cur = self.logic.value_text(&id);
                        if let Some(t) = pw::type_in(self.hwnd, sx, sy, &cur, self.scale) {
                            self.logic.typed(&id, &t);
                        }
                        repaint = true;
                    }
                    Act::Automate(i) => {
                        os_hostlink::to_host(self.hwnd as isize, os_hostlink::Cmd::Automate, i as isize);
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
                Cmd::Changed | Cmd::Note | Cmd::Automate => 0,
            }
        }

        fn paint(&mut self) {
            let (hdc, ps) = pw::begin_paint(self.hwnd);
            let extra = self.params.extra_copy();
            let histogram = params::flag(&extra, params::F_HISTOGRAM) || params::flag(&extra, params::F_HEATMAP);
            let spectrum = (histogram && params::byte(&extra, params::MONITOR_BYTE) != 0 && !self.analyzer.cols.is_empty()).then_some(self.analyzer.cols.as_slice());
            let look = Look { s: self.logic.shown(), extra: &extra, spectrum, rate: f64::from(self.shared.rate.load(Ordering::Relaxed)) };
            self.logic.hits = view::paint(&mut self.canvas, &look, &self.logic.view);
            self.last_sig = self.logic.vals.signature();
            self.last_extra = self.params.extra_version.load(Ordering::Acquire);
            self.last_view = self.logic.view.clone();
            self.buf.resize((self.size.0 * self.size.1) as usize, 0);
            self.canvas.copy_to_xrgb(&mut self.buf);
            pw::blit(hdc, &self.buf, self.size.0, self.size.1);
            pw::end_paint(self.hwnd, &ps);
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod gui {
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    use nih_plug::prelude::GuiContext;
    use os_hostlink::{Cmd, PageIcon, Peer};
    use os_plugkit::gui::{self as px, Button, Input, Window};
    use os_theme::Palette;
    use os_ui::Canvas;

    use super::{Act, Btn, Logic, Mods};
    use crate::analyzer::{Analyzer, Look as ALook};
    use crate::params::{self, EqParams, Opt};
    use crate::view::{self, Look, ViewState};
    use crate::Shared;

    /// The editor on its GUI thread.
    struct Ed {
        win: Window,
        canvas: Canvas,
        buf: Vec<u32>,
        scale: f32,
        size: (u32, u32),
        params: Arc<EqParams>,
        shared: Arc<Shared>,
        logic: Logic,
        palette: Palette,
        analyzer: Analyzer,
        scope_seen: u64,
        last_sig: u64,
        last_extra: u64,
        last_view: ViewState,
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
    pub struct Handle(Option<px::EditorThread>, Arc<Shared>);

    impl Drop for Handle {
        fn drop(&mut self) {
            self.0.take();
            self.1.editor_open.store(false, Ordering::Relaxed);
        }
    }

    pub fn open(parent: px::Parent, scale: f32, params: Arc<EqParams>, shared: Arc<Shared>, ctx: Arc<dyn GuiContext>, slot: &px::Slot) -> Handle {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let size = ((view::W as f32 * scale).round() as u32, (view::H as f32 * scale).round() as u32);
        let sh = shared.clone();
        // One page; joined, the editor keeps its size (it has no strip row of its own).
        let wake = slot.clone();
        let link = Peer::register(parent as u64, vec![PageIcon::Plugin], (view::W, view::H), move || px::notify(&wake));
        let peer = link.clone();
        let thread = px::spawn(parent, size, slot, move |win| {
            let Ok(canvas) = Canvas::new(size.0, size.1, scale) else { return };
            let logic = Logic::new(params.clone(), shared.clone(), Some(ctx));
            let ed = Ed {
                win,
                canvas,
                buf: Vec::new(),
                scale,
                size,
                params,
                shared,
                logic,
                palette: os_plugkit::palette(),
                analyzer: Analyzer::default(),
                scope_seen: 0,
                last_sig: 0,
                last_extra: u64::MAX,
                last_view: ViewState::default(),
                last_hint: String::new(),
                dirty: true,
                link: peer,
            };
            ed.run();
        });
        if thread.is_none() {
            link.unregister();
        }
        sh.editor_open.store(thread.is_some(), Ordering::Relaxed);
        Handle(thread, sh)
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

        /// Analyzer settings, or `None` when nothing is monitored.
        fn analyzer_look(&self) -> Option<ALook> {
            let b = self.params.extra_copy();
            let on = params::byte(&b, params::MONITOR_BYTE) != 0 && (params::flag(&b, params::F_HISTOGRAM) || params::flag(&b, params::F_HEATMAP));
            on.then(|| ALook {
                precision: params::opt(&b, Opt::FreqPrecision).clamp(0, 3) as usize,
                slope: params::opt(&b, Opt::PivotSlope).clamp(0, 4) as usize,
                smoothing: params::opt(&b, Opt::TimeSmoothing).clamp(0, 4) as usize,
                average: params::flag(&b, params::F_AVERAGE),
            })
        }

        /// Timer / parameter change: expire confirmed overlay values, analyse new audio, repaint when anything shown
        /// changed.
        fn poll(&mut self) {
            let mut dirty = self.logic.vals.expire();
            if let Some(look) = self.analyzer_look() {
                let n = Analyzer::size_for(&look);
                let fresh = self.shared.scope.lock().ok().filter(|s| s.written != self.scope_seen).map(|s| (s.written, s.latest(n)));
                if let Some((written, samples)) = fresh {
                    self.scope_seen = written;
                    let rate = self.shared.rate.load(Ordering::Relaxed) as f32;
                    self.analyzer.update(&samples, rate, view::COLS, &look);
                    dirty = true;
                }
            } else if !self.analyzer.cols.is_empty() {
                self.analyzer.cols.clear();
                dirty = true;
            }
            let sig = self.logic.vals.signature();
            let ev = self.params.extra_version.load(Ordering::Acquire);
            if dirty || sig != self.last_sig || ev != self.last_extra || self.logic.view != self.last_view || self.logic.hint != self.last_hint {
                self.dirty = true;
            }
            if self.logic.hint != self.last_hint {
                self.last_hint.clone_from(&self.logic.hint);
                // The host shows the hint in its title bar (hosted mode).
                self.link.set_hint(&self.logic.hint);
                self.link.to_host(Cmd::Changed, 0);
            }
            // Joined the strip: "Create automation clip" works.
            self.logic.hosted = self.link.hosted();
        }

        fn act(&mut self, acts: Vec<Act>) {
            for a in acts {
                match a {
                    Act::Repaint => self.dirty = true,
                    Act::Lock => self.win.lock(),
                    Act::Menu(req, x, y) => {
                        let def = self.logic.menu(&req);
                        let (sx, sy) = self.screen(x, y);
                        if let Some(cmd) = self.win.popup(sx, sy, &def, &self.palette, self.scale) {
                            let acts = self.logic.pick(&req, cmd, x, y);
                            self.act(acts);
                        }
                        self.dirty = true;
                    }
                    Act::TypeIn(id, x, y) => {
                        let (sx, sy) = self.screen(x, y);
                        let cur = self.logic.value_text(&id);
                        if let Some(t) = self.win.type_in(sx, sy, &cur, self.scale) {
                            self.logic.typed(&id, &t);
                        }
                        self.dirty = true;
                    }
                    // The host makes the clip; the index is the parameter's list position.
                    Act::Automate(i) => {
                        self.link.to_host(Cmd::Automate, i as isize);
                    }
                }
            }
        }

        fn paint(&mut self) {
            self.dirty = false;
            let extra = self.params.extra_copy();
            let histogram = params::flag(&extra, params::F_HISTOGRAM) || params::flag(&extra, params::F_HEATMAP);
            let spectrum = (histogram && params::byte(&extra, params::MONITOR_BYTE) != 0 && !self.analyzer.cols.is_empty()).then_some(self.analyzer.cols.as_slice());
            let look = Look { s: self.logic.shown(), extra: &extra, spectrum, rate: f64::from(self.shared.rate.load(Ordering::Relaxed)) };
            self.logic.hits = view::paint(&mut self.canvas, &look, &self.logic.view);
            self.last_sig = self.logic.vals.signature();
            self.last_extra = self.params.extra_version.load(Ordering::Acquire);
            self.last_view = self.logic.view.clone();
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
        let mut l = Logic::new(Arc::new(EqParams::default()), Arc::new(Shared::default()), None);
        paint(&mut l);
        l
    }

    fn paint(l: &mut Logic) {
        let mut c = os_ui::Canvas::new(view::W, view::H, 1.0).unwrap();
        let extra = l.params.extra_copy();
        let look = view::Look { s: l.shown(), extra: &extra, spectrum: None, rate: 48000.0 };
        l.hits = view::paint(&mut c, &look, &l.view);
    }

    fn centre(l: &Logic, t: Target) -> (f32, f32) {
        let h = l.hits.iter().rev().find(|h| h.t == t).unwrap_or_else(|| panic!("no hit {t:?}"));
        (h.r.cx(), h.r.cy())
    }

    /// A knob's "Create automation clip" works only inside Lemon Studio (host link) and asks for the parameter's list
    /// position: band 1 frequency is parameter 7 (spec: plugins/plugin-parameters.md §3, plugins/host-link.md).
    #[test]
    fn create_automation_clip_asks_the_host_for_the_parameter_index() {
        let mut l = logic();
        let req = MenuReq::Param(freq_id(0));
        let entry = |l: &Logic| {
            let m = l.menu(&req);
            let i = m.items.iter().position(|it| matches!(it, MenuItem::Item { label, .. } if label == "Create automation clip")).unwrap();
            (matches!(&m.items[i], MenuItem::Item { enabled: true, .. }), m.cmds[i].clone())
        };
        assert!(!entry(&l).0, "another host: disabled");
        l.hosted = true;
        let (enabled, cmd) = entry(&l);
        assert!(enabled);
        assert_eq!(l.pick(&req, cmd, 0.0, 0.0), vec![Act::Automate(7)]);
    }

    #[test]
    fn token_drag_moves_frequency_and_level_at_the_measured_speeds() {
        let mut l = logic();
        let (x, y) = centre(&l, Target::Token(3));
        let f0 = l.vals.norm(&freq_id(3));
        let acts = l.down(x, y, Btn::Left, Mods::default(), false);
        assert!(acts.contains(&Act::Lock));
        l.delta(102.4, -64.0, Mods::default());
        assert!((l.vals.norm(&freq_id(3)) - f0 - 0.1).abs() < 1e-4);
        // 64 px of 640 = a tenth of 36 dB.
        assert!((l.vals.plain(&level_id(3)) - 3.6).abs() < 1e-3);
        // Ctrl: a quarter of the speed.
        l.delta(0.0, -64.0, Mods { ctrl: true, ..Mods::default() });
        assert!((l.vals.plain(&level_id(3)) - 4.5).abs() < 1e-3);
        l.up(Btn::Left);
        assert!(!l.dragging());
        // Alt+click resets the band.
        l.down(x, y, Btn::Left, Mods { alt: true, ..Mods::default() }, false);
        assert_eq!(l.vals.plain(&level_id(3)), 0.0);
        assert!((l.vals.norm(&freq_id(3)) - f0).abs() < 1e-6);
        // Wheel: width ±5 % per notch.
        let w0 = l.vals.norm(&width_id(3));
        l.wheel(x, y, 1.0, Mods::default());
        assert!((l.vals.norm(&width_id(3)) - w0 - 0.05).abs() < 1e-5);
    }

    #[test]
    fn type_tab_steps_and_menus() {
        let mut l = logic();
        let (x, y) = centre(&l, Target::TypeTab(1));
        l.down(x, y, Btn::Left, Mods::default(), false);
        l.delta(0.0, -17.0, Mods::default());
        assert_eq!(l.vals.plain(&type_id(1)) as i32, eq::HIGH_SHELF);
        l.up(Btn::Left);
        let m = l.menu(&MenuReq::Token(0));
        assert_eq!(m.find("Steep 6"), Some(Cmd::SetOrder(0, -2)));
        assert_eq!(m.find("A4 - 440.0Hz"), Some(Cmd::SetHz(0, 440.0)));
        l.pick(&MenuReq::Token(0), Cmd::SetHz(0, 440.0), 0.0, 0.0);
        assert!((eq::freq_hz(f64::from(l.vals.plain(&freq_id(0)))) - 440.0).abs() < 0.05);
        let o = l.menu(&MenuReq::Options);
        assert_eq!(o.find("-120dB"), Some(Cmd::Opt(Opt::Range, 3)));
    }

    /// Whether the first entry labelled `label` in `m` or its submenus can be picked.
    fn clickable(m: &MenuDef<Cmd>, label: &str) -> bool {
        let it = m.items.iter().chain(m.subs.iter().flat_map(|(_, s)| s.items.iter()));
        it.clone().find(|it| matches!(it, MenuItem::Item { label: l, .. } if l == label)).unwrap_or_else(|| panic!("no {label}")).clickable()
    }

    #[test]
    fn greyed_out_controls_only_say_so() {
        let mut l = logic();
        let extra = l.params.extra_copy();
        for t in [Target::Lin, Target::Hq] {
            let (x, y) = centre(&l, t);
            for btn in [Btn::Left, Btn::Right] {
                assert_eq!(l.down(x, y, btn, Mods::default(), false), vec![Act::Repaint], "{t:?}");
            }
            assert!(l.hint.ends_with(": not available yet"), "{}", l.hint);
        }
        assert_eq!(l.params.extra_copy(), extra, "LIN and HQ left alone");
        // Options nothing reads yet are greyed out; the analyzer's are not.
        let o = l.menu(&MenuReq::Options);
        for label in ["Phase rotation", "Enhanced frequency", "High precision", "Smooth", "Ultra", "Adaptive", "Inherit bandwidth"] {
            assert!(!clickable(&o, label), "{label}");
        }
        for label in ["Lock spare state", "-120dB", "Average mode", "Highest"] {
            assert!(clickable(&o, label), "{label}");
        }
        assert!(l.menu(&MenuReq::Phones).items.iter().all(|it| !it.clickable()));
    }

    #[test]
    fn mute_solo_and_compare() {
        let mut l = logic();
        let (x, y) = centre(&l, Target::Switch(2));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(l.mask(), 0x7f & !(1 << 2));
        l.down(x, y, Btn::Right, Mods::default(), false);
        assert_eq!(l.mask(), 1 << 2);
        l.down(x, y, Btn::Right, Mods::default(), false);
        assert_eq!(l.mask(), 0x7f);
        // A/B: store, change, flip back.
        l.compare(false);
        l.vals.edit_plain(&level_id(4), 6.0);
        l.compare(true);
        assert_eq!(l.vals.plain(&level_id(4)), 0.0);
        l.compare(true);
        assert_eq!(l.vals.plain(&level_id(4)), 6.0);
    }

    #[test]
    fn hints() {
        let mut l = logic();
        let (x, y) = centre(&l, Target::Token(0));
        l.moved(x, y, Mods::default());
        assert_eq!(l.hint, "0.0dB 63Hz 61%\tBand 1 (low shelf)");
        let (x, y) = centre(&l, Target::Fader(MAIN_FADER));
        l.moved(x, y, Mods::default());
        assert_eq!(l.hint, "Main level: 0.0dB");
        let (x, y) = centre(&l, Target::TypeTab(0));
        l.moved(x, y, Mods::default());
        assert_eq!(l.hint, "Band 1 order: 2");
    }
}
