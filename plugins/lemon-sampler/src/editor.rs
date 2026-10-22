// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The plugin editor: a child of the host's window (Win32, X11 on Linux, an NSView on macOS) that paints `view` with
//! the os-ui canvas, and the editor logic (platform independent): knob drags with the cursor held in place, wheel and
//! reset gestures, menus, the keyboard and waveform. Values the editor sets show at once (an overlay over the host's
//! parameter values until the host confirms them), so nothing waits for the host's next flush.
//!
// spec: plugins/lemon-sampler.md (§2 menus, §2.3 waveform, §3.1 keyboard, §4 interaction), plugins/host-link.md,
// spec: plugins/plugin-formats.md (editors)

use std::any::Any;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nih_plug::prelude::*;
use os_ui::kit::menu::{Mark, MenuItem, MenuStyle};
use os_ui::Rect;

use crate::params::{self, SamplerParams};
use crate::view::{self, Hit, Model, SampleView, Target, ViewState};
use crate::Shared;

pub struct SamplerEditor {
    pub params: Arc<SamplerParams>,
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

impl SamplerEditor {
    pub fn new(params: Arc<SamplerParams>, shared: Arc<Shared>) -> Self {
        SamplerEditor {
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

impl Editor for SamplerEditor {
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
        (view::EDITOR_W, view::EDITOR_H)
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
// Parameter access

/// Parameter values by id for the view, with the editor's own not-yet-confirmed edits on top.
pub struct Vals {
    pub ptrs: HashMap<String, ParamPtr>,
    overlay: HashMap<String, (f32, Instant)>,
}

impl Vals {
    pub fn new(params: &SamplerParams) -> Self {
        Vals { ptrs: params.param_map().into_iter().map(|(id, p, _)| (id, p)).collect(), overlay: HashMap::new() }
    }

    /// The host's value.
    pub fn actual(&self, id: &str) -> f32 {
        // SAFETY: the pointers come from the params object, which outlives the editor.
        self.ptrs.get(id).map_or(0.0, |p| params::live_of(*p))
    }

    fn put(&mut self, id: &str, v: f32) {
        self.overlay.insert(id.to_string(), (v, Instant::now()));
    }

    /// Drops overlay entries the host has confirmed (or ignored for a second). True when one went.
    pub fn expire(&mut self) -> bool {
        let before = self.overlay.len();
        let now = Instant::now();
        let ptrs = &self.ptrs;
        self.overlay.retain(|id, (v, t)| {
            let a = ptrs.get(id).map_or(*v, |p| params::live_of(*p));
            (a - *v).abs() > 1e-6 && now.duration_since(*t) < Duration::from_secs(1)
        });
        before != self.overlay.len()
    }

    pub fn default_norm(&self, id: &str) -> f32 {
        self.ptrs.get(id).map_or(0.0, |p| unsafe { p.default_normalized_value() })
    }

    pub fn plain_to_norm(&self, id: &str, plain: f32) -> f32 {
        self.ptrs.get(id).map_or(0.0, |p| unsafe { p.preview_normalized(plain) })
    }

    pub fn text_of(&self, id: &str, norm: f32) -> String {
        self.ptrs.get(id).map_or_else(String::new, |p| unsafe { p.normalized_value_to_string(norm, true) })
    }

    /// A fingerprint of every value (repaint when it changes).
    pub fn signature(&self) -> u64 {
        let mut h: u64 = 1469598103934665603;
        for p in self.ptrs.values() {
            let v = unsafe { p.unmodulated_normalized_value() }.to_bits();
            h = (h ^ u64::from(v)).wrapping_mul(1099511628211);
        }
        for (v, _) in self.overlay.values() {
            h = (h ^ u64::from(v.to_bits())).wrapping_mul(1099511628211);
        }
        h
    }
}

impl Model for Vals {
    fn norm(&self, id: &str) -> f32 {
        match self.overlay.get(id) {
            Some((v, _)) => *v,
            None => self.actual(id),
        }
    }
    fn plain(&self, id: &str) -> f32 {
        let n = self.norm(id);
        self.ptrs.get(id).map_or(0.0, |p| unsafe { p.preview_plain(n) })
    }
    fn text(&self, id: &str) -> String {
        self.text_of(id, self.norm(id))
    }
}

/// Default values of a fresh Lemon Sampler (offscreen renders, tests).
pub struct Defaults(pub Vals);

impl Defaults {
    pub fn new(params: &SamplerParams) -> Self {
        Defaults(Vals::new(params))
    }
}

impl Model for Defaults {
    fn norm(&self, id: &str) -> f32 {
        self.0.norm(id)
    }
    fn plain(&self, id: &str) -> f32 {
        self.0.plain(id)
    }
    fn text(&self, id: &str) -> String {
        self.0.text(id)
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Knob laws (spec §4)

/// Pixels of drag for a knob's whole range.
pub fn knob_px(id: &str) -> f32 {
    match id {
        "start_offset" => 2000.0,
        "loop_start" | "loop_end" => 1600.0,
        "fx_start" | "fx_length" => 1000.0,
        "fx_eq" | "fx_rm_freq" => 128.0,
        "ts_mul" => 400.0,
        "ts_pitch" => 640.0,
        "ts_time" => params::TIME_PX,
        "f_cut" | "f_res" => 500.0,
        _ => 256.0,
    }
}

/// Fine (Ctrl) slow-down of a knob.
pub fn fine_factor(id: &str) -> f32 {
    if id == "ts_mul" {
        0.01
    } else {
        0.25
    }
}

/// Envelope time knobs (the ones with the Set ▸ steps submenu).
fn is_time_knob(id: &str) -> bool {
    id.starts_with("env") && ["_delay", "_att", "_hold", "_dec", "_rel", "_lfo_delay", "_lfo_att"].iter().any(|s| id.ends_with(s))
}

/// Knob position of an envelope time knob for `secs` seconds (inverse of the envelope time law).
pub fn time_knob_norm(secs: f64, falling: bool) -> f32 {
    let s = if falling { (secs - 0.009).max(0.0) } else { secs.max(0.0) };
    let g100 = 2f64.powf(100.0 / 6553.6);
    let g = s / 0.063922 + g100;
    let field = 6553.6 * g.log2();
    (field / 65536.0).clamp(0.0, 1.0) as f32
}

/// Parses a typed value into a knob position (spec §2.2 "Type in value").
pub fn parse_value(vals: &Vals, id: &str, text: &str) -> Option<f32> {
    let t = text.trim();
    let num: String = t.chars().skip_while(|c| !(c.is_ascii_digit() || *c == '-' || *c == '+' || *c == '.')).take_while(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.')).collect();
    let n: f32 = num.parse().ok()?;
    let lower = t.to_ascii_lowercase();
    let suffix = id.split_once('_').map_or(id, |(a, b)| if a.starts_with("env") { b } else { id });
    let v = match suffix {
        "ts_pitch" => (n + 1200.0) / 2400.0,
        "ts_mul" => (((n / 100.0).max(1e-3)).log2() / 4.0) + 0.5,
        "ts_time" => {
            // "bars:steps:ticks" or a number of beats.
            let parts: Vec<&str> = t.split(':').collect();
            let steps = if parts.len() >= 2 {
                let part = |i: usize| parts.get(i).and_then(|p| p.trim().parse::<f64>().ok()).unwrap_or(0.0);
                parts[0].trim().parse::<f64>().ok()? * 16.0 + part(1) + part(2) / 24.0
            } else {
                f64::from(n) * 4.0
            };
            params::time_norm(steps)
        }
        // Loop points are typed in samples (handled by `Logic::typed`, which knows the length).
        "loop_start" | "loop_end" => return None,
        "root" | "zone_lo" | "zone_hi" => return note_from_text(t).map(|k| vals.plain_to_norm(id, k as f32)),
        "fine" => return Some(vals.plain_to_norm(id, n)),
        "fx_sdelay" => {
            let s = if lower.contains("left") { -n } else { n };
            (s / 100.0 + 1.0) / 2.0
        }
        "amt" | "lfo_amt" | "att_t" | "dec_t" | "rel_t" | "fx_eq" | "fx_pogo" => (n / 100.0 + 1.0) / 2.0,
        _ => n / 100.0,
    };
    Some(v.clamp(0.0, 1.0))
}

/// "C5" / "c#4" -> key (C5 = 60).
pub fn note_from_text(t: &str) -> Option<i32> {
    let t = t.trim().to_ascii_uppercase();
    let (name, oct) = t.split_at(t.find(|c: char| c.is_ascii_digit() || c == '-')?);
    let idx = params::NOTE_NAMES.iter().position(|n| *n == name)?;
    Some(oct.parse::<i32>().ok()? * 12 + idx as i32)
}

// ---------------------------------------------------------------------------------------------------------------
// Menus

/// What a menu entry does.
#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    None,
    Reset,
    /// Envelope time knob: the stage lasts `n` steps (16ths of a bar) at the host tempo.
    Steps(i32),
    CopyValue,
    PasteValue,
    TypeIn,
    /// TIME knob value in 16ths of a beat (0 = none); -1 = autodetect / project tempo.
    Time(i32),
    /// Choice list entry.
    Choice(i32),
    Reload,
    SaveAs,
    Locate,
    CopyAudio,
    PasteAudio,
    DetectTempo,
    Properties,
    Spectrum,
    Multichannel,
    Recent(String),
    Browse,
    /// "Create automation clip" (host link).
    Automate,
}

/// A menu to show: rows, what each does, submenus by row.
pub struct MenuDef {
    pub items: Vec<MenuItem>,
    pub cmds: Vec<Cmd>,
    pub subs: Vec<(usize, MenuDef)>,
    pub style: MenuStyle,
    pub current: Option<usize>,
}

impl MenuDef {
    fn new(style: MenuStyle) -> Self {
        MenuDef { items: Vec::new(), cmds: Vec::new(), subs: Vec::new(), style, current: None }
    }
    fn add(&mut self, it: MenuItem, c: Cmd) -> usize {
        self.items.push(it);
        self.cmds.push(c);
        self.items.len() - 1
    }
    fn head(&mut self, s: &str) {
        self.add(MenuItem::head(s), Cmd::None);
    }
    fn sep(&mut self) {
        self.add(MenuItem::Sep, Cmd::None);
    }
    pub fn sub(&self, i: usize) -> Option<&MenuDef> {
        self.subs.iter().find(|(j, _)| *j == i).map(|(_, m)| m)
    }

    /// The same menu as the shared editor kit's (its popups take that one).
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn kit(&self) -> os_plugkit::MenuDef<Cmd> {
        os_plugkit::MenuDef {
            items: self.items.clone(),
            cmds: self.cmds.clone(),
            subs: self.subs.iter().map(|(i, m)| (*i, m.kit())).collect(),
            style: self.style,
            current: self.current,
        }
    }
}

/// Which menu is open.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuReq {
    Knob(String),
    List(String),
    Wave,
    EnvPresets,
    History,
}

// ---------------------------------------------------------------------------------------------------------------
// Editor logic

/// Something the window has to do after an event.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Repaint,
    /// Hide the cursor and hold it in place (knob drag).
    Lock,
    Menu(MenuReq, f32, f32),
    TypeIn(String, f32, f32),
    /// Preview note (key, velocity 1..127, on).
    Note(u8, u8, bool),
    OpenDialog,
    Load(String),
    Clear,
    Reload,
    SaveAs,
    Locate,
    CopyAudio,
    PasteAudio,
    /// Ask the host for an automation clip on this parameter (host link).
    Automate(String),
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
    Knob { id: String },
    Slider { id: String, track: Rect },
    KeyScroll { x0: f32, s0: f32, ratio: f32 },
    Keys,
    /// The sample sounding at its root note while the waveform is held (`held`).
    Wave,
    Ruler { from: i32 },
    Overview { track: Rect, grab: f64 },
    Loop { which: u8, both: bool, x0: f32, s0: f32, e0: f32 },
    EnvPoint { i: u8, x0: f32, y0: f32, v: [f32; 3] },
}

pub struct Logic {
    pub vals: Vals,
    ctx: Option<Arc<dyn GuiContext>>,
    pub params: Arc<SamplerParams>,
    pub shared: Arc<Shared>,
    pub view: ViewState,
    pub hits: Vec<Hit>,
    drag: Option<Drag>,
    /// Hint text (`Label: value`, a tab before a right-aligned part).
    pub hint: String,
    copied: Option<f32>,
    /// Key held in the editor (preview) and its velocity.
    held: Option<(u8, u8)>,
    pub hosted: bool,
    /// TIME knob drags: the position before snapping to whole steps.
    time_drag: f32,
}

impl Logic {
    pub fn new(params: Arc<SamplerParams>, shared: Arc<Shared>, ctx: Option<Arc<dyn GuiContext>>) -> Self {
        Logic {
            vals: Vals::new(&params),
            ctx,
            params,
            shared,
            view: ViewState::default(),
            hits: Vec::new(),
            drag: None,
            hint: String::new(),
            copied: None,
            held: None,
            hosted: false,
            time_drag: 0.0,
        }
    }

    fn at(&self, x: f32, y: f32) -> Option<Target> {
        self.hits.iter().rev().find(|h| h.r.contains(x, y)).map(|h| h.t.clone())
    }

    // Parameter edits: the overlay shows them at once; the host gets them through the GUI context.

    fn begin(&self, id: &str) {
        if let (Some(p), Some(c)) = (self.vals.ptrs.get(id), &self.ctx) {
            unsafe { c.raw_begin_set_parameter(*p) };
        }
    }
    fn set(&mut self, id: &str, norm: f32) {
        let norm = norm.clamp(0.0, 1.0);
        self.vals.put(id, norm);
        if let Some(p) = self.vals.ptrs.get(id) {
            // The sound uses the edit at once, whether or not the host hands it back (params::Live).
            params::live_set(params::param_addr(*p), norm, unsafe { p.unmodulated_normalized_value() });
        }
        if let (Some(p), Some(c)) = (self.vals.ptrs.get(id), &self.ctx) {
            unsafe { c.raw_set_parameter_normalized(*p, norm) };
        }
    }
    fn end(&self, id: &str) {
        if let (Some(p), Some(c)) = (self.vals.ptrs.get(id), &self.ctx) {
            unsafe { c.raw_end_set_parameter(*p) };
        }
    }
    /// One complete edit (begin, set, end).
    pub fn edit(&mut self, id: &str, norm: f32) {
        self.begin(id);
        self.set(id, norm);
        self.end(id);
    }
    fn edit_plain(&mut self, id: &str, plain: f32) {
        let n = self.vals.plain_to_norm(id, plain);
        self.edit(id, n);
    }
    fn reset(&mut self, id: &str) {
        let d = self.vals.default_norm(id);
        self.edit(id, d);
        self.hint_for(id);
    }

    /// Turning PITCH in Resample mode switches the mode to Auto; MUL and TIME leave it alone (spec §5.6).
    fn stretch_touch(&mut self, id: &str) {
        if id == "ts_pitch" && self.vals.plain("ts_mode") as i32 == 0 {
            self.edit_plain("ts_mode", params::STRETCH_AUTO as f32);
        }
    }

    /// Picking Stretch or Stretch Pro while TIME is off sets TIME to the sample's own length at the host tempo, in
    /// whole 1/192 beats (spec §5.6).
    fn realtime_picked(&mut self) {
        if params::time_file(params::time_steps(self.vals.norm("ts_time"))) >= 1.0 {
            return;
        }
        let secs = self.shared.raw.lock().ok().and_then(|r| r.as_ref().map(|s| s.frames() as f64 / f64::from(s.rate.max(1)))).unwrap_or(0.0);
        let units = (secs * self.shared.tempo() / 60.0 * 192.0).round();
        if units >= 1.0 {
            self.edit("ts_time", params::time_norm(units / 48.0));
        }
    }

    fn hint_for(&mut self, id: &str) {
        self.hint = self.param_hint(id);
    }

    /// `Label: value` with the TIME knob's clip tempo.
    fn param_hint(&self, id: &str) -> String {
        let mut h = view::param_hint(&self.vals, id);
        if id == "ts_time" {
            let steps = params::time_steps(self.vals.norm(id));
            let raw = self.shared.raw.lock().ok().and_then(|r| r.as_ref().map(|s| s.frames() as f64 / f64::from(s.rate.max(1))));
            if let (true, Some(secs)) = (params::time_file(steps) >= 1.0, raw) {
                if secs > 0.0 {
                    let bpm = steps / 4.0 * 60.0 / secs;
                    h.push_str(&format!("  Clip tempo is {bpm:.0} BPM"));
                }
            }
        }
        h
    }

    fn with_sample<R>(&self, f: impl FnOnce(&SampleView) -> R) -> R {
        let path = self.params.sample_path.read().map(|p| p.clone()).unwrap_or_default();
        let s = self.shared.processed.lock().ok().and_then(|p| p.as_ref().and_then(|p| p.sample.clone()));
        let s = s.or_else(|| self.shared.raw.lock().ok().and_then(|r| r.clone()));
        let bits = (self.shared.bits.load(Ordering::Relaxed) & 0x1ff) as u16;
        f(&SampleView { sample: s.as_deref(), path: &path, bits, spectrum: None, tones: None })
    }

    fn hover_hint(&self, x: f32, y: f32) -> String {
        match self.at(x, y) {
            Some(Target::Knob(id)) | Some(Target::Slider(id, _)) => self.param_hint(&id),
            Some(Target::Key(k)) if self.held.is_none() => format!("Select range & offset   {}", params::note_name(k)),
            Some(t) => self.with_sample(|sv| view::hint(&t, &self.vals, &self.view, sv, x, y)),
            None => String::new(),
        }
    }

    // Mouse

    pub fn down(&mut self, x: f32, y: f32, btn: Btn, m: Mods, double: bool) -> Vec<Act> {
        if debug() {
            eprintln!("[lemon-sampler] down {x},{y} {btn:?} -> {:?} ({} hits)", self.at(x, y), self.hits.len());
        }
        let Some(t) = self.at(x, y) else { return vec![] };
        if !view::works(&t) {
            // Greyed-out controls only say so.
            self.hint = self.hover_hint(x, y);
            return vec![Act::Repaint];
        }
        if btn == Btn::Middle || (btn == Btn::Left && m.alt && t.param().is_some() && !matches!(t, Target::Key(_))) {
            // Middle click / Alt+click resets (spec §4).
            return match t.param().map(str::to_string) {
                Some(id) => {
                    self.reset(&id);
                    vec![Act::Repaint]
                }
                None => vec![],
            };
        }
        if btn == Btn::Right {
            return self.right(t, x, y);
        }
        match t {
            Target::Knob(id) => {
                self.stretch_touch(&id);
                self.begin(&id);
                self.hint_for(&id);
                self.time_drag = self.vals.norm(&id);
                self.view.pressed = Some(Target::Knob(id.clone()));
                self.drag = Some(Drag::Knob { id });
                return vec![Act::Lock, Act::Repaint];
            }
            Target::Slider(id, track) => {
                if double {
                    self.reset(&id);
                } else {
                    self.begin(&id);
                    self.set(&id, (x - track.x) / track.w);
                    self.hint_for(&id);
                    self.view.pressed = Some(Target::Slider(id.clone(), track));
                    self.drag = Some(Drag::Slider { id, track });
                }
            }
            Target::Toggle(id) => {
                let v = self.vals.norm(&id);
                self.edit(&id, if v > 0.5 { 0.0 } else { 1.0 });
                self.hint = self.hover_hint(x, y);
            }
            Target::Set(id, plain) => self.edit_plain(&id, plain),
            Target::List(id) => {
                let r = self.hits.iter().rev().find(|h| h.t == Target::List(id.clone())).map(|h| h.r).unwrap_or_default();
                return vec![Act::Menu(MenuReq::List(id), r.x, r.bottom())];
            }
            Target::ReverbAb => {
                let v = self.vals.plain("fx_reverb_type");
                self.edit_plain("fx_reverb_type", if v > 0.5 { 0.0 } else { 1.0 });
            }
            Target::Tab(i) => self.view.tab = i,
            Target::SubTab(i) => self.view.sub = i,
            Target::FxPage(i) => self.view.fx_page = i,
            Target::FileOpen | Target::FileName => return vec![Act::OpenDialog],
            Target::FileBrowse => return vec![Act::Locate],
            Target::FileClear => return vec![Act::Clear, Act::Repaint],
            Target::Key(k) => {
                let vel = self.key_velocity(y);
                self.held = Some((k as u8, vel));
                self.drag = Some(Drag::Keys);
                self.hint = format!("{} / velocity: {vel}", params::note_name(k));
                return vec![Act::Note(k as u8, vel, true), Act::Repaint];
            }
            Target::Ruler => {
                let k = view::ruler_key(&self.view, x);
                self.view.zone_drag = Some((k, k));
                self.drag = Some(Drag::Ruler { from: k });
            }
            Target::KeyScroll(track) => {
                let total = view::keyboard_width(false);
                let thumb = (view::KEYS_W / total * track.w).max(24.0);
                let ratio = view::keyboard_max_scroll(false) / (track.w - thumb).max(1.0);
                self.view.pressed = Some(Target::KeyScroll(track));
                self.drag = Some(Drag::KeyScroll { x0: x, s0: self.view.key_scroll, ratio });
            }
            Target::KeyArrow(d) => self.scroll_keys(f32::from(d) * view::OCTAVE_W),
            Target::RootReset => {
                for id in ["root", "fine", "zone_lo", "zone_hi"] {
                    let d = self.vals.default_norm(id);
                    self.edit(id, d);
                }
            }
            Target::Hand => self.view.hand = !self.view.hand,
            Target::Wave(_) => {
                if double {
                    self.view.wave_start = 0.0;
                    self.view.wave_len = 1.0;
                } else {
                    // A press plays the sample at its own pitch (the root note) until the button is let go.
                    let root = (self.vals.plain("root") as i32).clamp(0, 127) as u8;
                    self.held = Some((root, 100));
                    self.drag = Some(Drag::Wave);
                    return vec![Act::Note(root, 100, true), Act::Repaint];
                }
            }
            Target::Overview(track) => {
                let inner = (track.x, track.w);
                let f = f64::from((x - inner.0) / inner.1);
                let in_view = f >= self.view.wave_start && f <= self.view.wave_start + self.view.wave_len;
                let grab = if in_view { f - self.view.wave_start } else { self.view.wave_len / 2.0 };
                self.drag = Some(Drag::Overview { track, grab });
                self.move_overview(track, grab, x);
            }
            Target::WaveArrow(d) => self.scroll_wave(f64::from(d) * self.view.wave_len / 4.0),
            Target::WaveInfo => {
                self.view.multichannel = !self.view.multichannel;
            }
            Target::LoopHandle(i) => {
                let (s0, e0) = (self.vals.norm("loop_start"), self.vals.norm("loop_end"));
                let both = m.shift;
                self.begin("loop_start");
                self.begin("loop_end");
                self.view.pressed = Some(Target::LoopHandle(i));
                self.drag = Some(Drag::Loop { which: i, both, x0: x, s0, e0 });
            }
            Target::EnvMenu => return vec![Act::Menu(MenuReq::EnvPresets, x, y)],
            Target::EnvPoint(i) => {
                let p = view::ENV_PREFIX[self.view.sub.min(4) as usize];
                let ids = env_point_ids(p, i);
                let v = [self.vals.norm(&ids[0]), ids.get(1).map_or(0.0, |s| self.vals.norm(s)), 0.0];
                for id in &ids {
                    self.begin(id);
                }
                self.view.pressed = Some(Target::EnvPoint(i));
                self.drag = Some(Drag::EnvPoint { i, x0: x, y0: y, v });
            }
        }
        vec![Act::Repaint]
    }

    fn right(&mut self, t: Target, x: f32, y: f32) -> Vec<Act> {
        match t {
            Target::Knob(id) => vec![Act::Menu(MenuReq::Knob(id), x, y)],
            Target::Slider(id, _) => vec![Act::Menu(MenuReq::Knob(id), x, y)],
            Target::List(id) => {
                // Steps back through the list (Lemon Studio choice).
                let n = self.vals.plain(&id) as i32;
                let max = list_labels(&id).len() as i32;
                self.edit_plain(&id, ((n - 1).rem_euclid(max)) as f32);
                self.hint_for(&id);
                vec![Act::Repaint]
            }
            Target::Key(k) => {
                self.edit_plain("root", k as f32);
                self.hint = format!("Root note: {}", params::note_name(k));
                vec![Act::Repaint]
            }
            Target::Wave(_) | Target::Overview(_) | Target::WaveInfo => vec![Act::Menu(MenuReq::Wave, x, y)],
            Target::FileOpen | Target::FileName => vec![Act::Menu(MenuReq::History, x, y)],
            Target::EnvMenu => vec![Act::Menu(MenuReq::EnvPresets, x, y)],
            _ => vec![],
        }
    }

    /// Velocity from the click height on a key: 64 at the middle (Lemon Studio choice), louder towards the bottom.
    fn key_velocity(&self, y: f32) -> u8 {
        let (_, kt, _, kb, _, _) = view::key_rows();
        let (kt, kb) = if self.view.hand { (view::y(365.0), view::y(454.0)) } else { (kt, kb) };
        let u = ((y - kt) / (kb - kt)).clamp(0.0, 1.0);
        (1.0 + u * 126.0).round() as u8
    }

    fn scroll_keys(&mut self, d: f32) {
        self.view.key_scroll = (self.view.key_scroll + d).clamp(0.0, view::keyboard_max_scroll(false));
    }

    fn scroll_wave(&mut self, d: f64) {
        self.view.wave_start = (self.view.wave_start + d).clamp(0.0, (1.0 - self.view.wave_len).max(0.0));
    }

    fn move_overview(&mut self, track: Rect, grab: f64, x: f32) {
        let f = f64::from((x - track.x) / track.w);
        self.view.wave_start = (f - grab).clamp(0.0, (1.0 - self.view.wave_len).max(0.0));
    }

    /// Mouse move (not locked). Returns the acts (repaint when something changed).
    pub fn moved(&mut self, x: f32, y: f32, _m: Mods) -> Vec<Act> {
        self.view.mouse = Some((x, y));
        match self.drag.clone() {
            Some(Drag::Slider { id, track }) => {
                self.set(&id, (x - track.x) / track.w);
                self.hint_for(&id);
                vec![Act::Repaint]
            }
            Some(Drag::KeyScroll { x0, s0, ratio }) => {
                self.view.key_scroll = (s0 + (x - x0) * ratio).clamp(0.0, view::keyboard_max_scroll(false));
                vec![Act::Repaint]
            }
            Some(Drag::Keys) => {
                let k = view::key_at(&self.view, x, y);
                match (k, self.held) {
                    (Some(k), Some((h, _))) if k as u8 != h => {
                        let vel = self.key_velocity(y);
                        self.held = Some((k as u8, vel));
                        self.hint = format!("{} / velocity: {vel}", params::note_name(k));
                        vec![Act::Note(h, 0, false), Act::Note(k as u8, vel, true), Act::Repaint]
                    }
                    (Some(k), None) => {
                        let vel = self.key_velocity(y);
                        self.held = Some((k as u8, vel));
                        vec![Act::Note(k as u8, vel, true), Act::Repaint]
                    }
                    _ => vec![],
                }
            }
            Some(Drag::Ruler { from }) => {
                let k = view::ruler_key(&self.view, x);
                self.view.zone_drag = Some((from, k));
                let (a, b) = (from.min(k), from.max(k));
                self.hint = format!("Zone: {} to {}", params::note_name(a), params::note_name(b));
                vec![Act::Repaint]
            }
            Some(Drag::Overview { track, grab }) => {
                self.move_overview(track, grab, x);
                vec![Act::Repaint]
            }
            Some(Drag::Loop { which, both, x0, s0, e0 }) => {
                let area = view::wave_area();
                let d = (x - x0) / area.w * self.view.wave_len as f32;
                if both {
                    let d = d.clamp(-s0, 1.0 - e0);
                    self.set("loop_start", s0 + d);
                    self.set("loop_end", e0 + d);
                } else if which == 0 {
                    self.set("loop_start", (s0 + d).min(e0));
                } else {
                    self.set("loop_end", (e0 + d).max(s0));
                }
                self.hint_for(if which == 0 { "loop_start" } else { "loop_end" });
                vec![Act::Repaint]
            }
            Some(Drag::EnvPoint { i, x0, y0, v }) => {
                let p = view::ENV_PREFIX[self.view.sub.min(4) as usize];
                let ids = env_point_ids(p, i);
                let eg = view::env_geom(&self.vals, p, view::env_rect());
                // The decay node moves (1 - sustain) of the decay knob's run.
                let run = if i == 3 { eg.px * (1.0 - eg.sus).max(0.05) } else { eg.px };
                let dx = (x - x0) / run;
                self.set(&ids[0], v[0] + dx);
                if i == 3 {
                    // Sustain node: vertical = sustain level over the drawn level range.
                    let (top, bot) = view::env_levels(view::env_rect());
                    let h = (bot - top) * if eg.amp > 0.05 { eg.amp } else { 1.0 };
                    self.set(&ids[1], v[1] + (y0 - y) / h);
                }
                self.hint_for(&ids[0]);
                vec![Act::Repaint]
            }
            Some(Drag::Knob { .. } | Drag::Wave) => vec![],
            None => {
                let h = self.hover_hint(x, y);
                let changed = h != self.hint;
                self.hint = h;
                // Hover looks: repaint when the control under the mouse changes.
                let _ = changed;
                vec![Act::Repaint]
            }
        }
    }

    /// Locked knob drag: `dy` logical px (down positive).
    pub fn delta(&mut self, _dx: f32, dy: f32, m: Mods) -> Vec<Act> {
        let Some(Drag::Knob { id }) = self.drag.clone() else { return vec![] };
        let mut rate = 1.0 / knob_px(&id);
        if m.ctrl {
            rate *= fine_factor(&id);
        }
        let v = if id == "ts_time" {
            // TIME lands on whole steps; the drag itself keeps its fractions.
            self.time_drag = (self.time_drag - dy * rate).clamp(0.0, 1.0);
            params::time_snap(self.time_drag)
        } else {
            self.vals.norm(&id) - dy * rate
        };
        self.set(&id, v);
        self.hint_for(&id);
        vec![Act::Repaint]
    }

    pub fn up(&mut self, _btn: Btn) -> Vec<Act> {
        self.view.pressed = None;
        let mut acts = vec![Act::Repaint];
        match self.drag.take() {
            Some(Drag::Knob { id } | Drag::Slider { id, .. }) => self.end(&id),
            Some(Drag::Keys | Drag::Wave) => {
                if let Some((k, _)) = self.held.take() {
                    acts.push(Act::Note(k, 0, false));
                }
            }
            Some(Drag::Ruler { from }) => {
                let to = self.view.zone_drag.map_or(from, |z| z.1);
                self.view.zone_drag = None;
                let (a, b) = (from.min(to), from.max(to));
                if a == b {
                    // A click without a drag clears the zone.
                    self.edit_plain("zone_lo", 0.0);
                    self.edit_plain("zone_hi", params::KEY_MAX as f32);
                } else {
                    self.edit_plain("zone_lo", a as f32);
                    self.edit_plain("zone_hi", b as f32);
                }
            }
            Some(Drag::Loop { .. }) => {
                self.end("loop_start");
                self.end("loop_end");
            }
            Some(Drag::EnvPoint { i, .. }) => {
                let p = view::ENV_PREFIX[self.view.sub.min(4) as usize];
                for id in env_point_ids(p, i) {
                    self.end(&id);
                }
            }
            _ => {}
        }
        acts
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub fn leave(&mut self) -> Vec<Act> {
        self.view.mouse = None;
        if self.drag.is_none() {
            self.hint.clear();
        }
        vec![Act::Repaint]
    }

    /// Wheel (spec §4): one notch = 8 px of drag; Ctrl 4, Shift 16; Alt jumps.
    pub fn wheel(&mut self, x: f32, y: f32, notches: f32, m: Mods) -> Vec<Act> {
        match self.at(x, y) {
            Some(Target::Knob(id)) | Some(Target::Slider(id, _)) => {
                let cur = self.vals.norm(&id);
                let v = if m.alt {
                    if id == "ts_mul" {
                        cur + notches.signum() * 0.25
                    } else if notches > 0.0 {
                        1.0
                    } else {
                        0.0
                    }
                } else {
                    let px = if m.ctrl {
                        4.0
                    } else if m.shift {
                        16.0
                    } else {
                        8.0
                    };
                    let px_range = if id == "fine" { 200.0 } else { knob_px(&id) };
                    cur + notches * px / px_range
                };
                let v = if id == "ts_time" { params::time_snap(v.clamp(0.0, 1.0)) } else { v };
                self.stretch_touch(&id);
                self.edit(&id, v);
                self.hint_for(&id);
                vec![Act::Repaint]
            }
            Some(Target::List(id)) => {
                let n = self.vals.plain(&id) as i32;
                let max = list_labels(&id).len() as i32 - 1;
                self.edit_plain(&id, (n - notches.signum() as i32).clamp(0, max) as f32);
                self.hint_for(&id);
                vec![Act::Repaint]
            }
            Some(Target::Key(_) | Target::Ruler | Target::KeyScroll(_) | Target::KeyArrow(_)) => {
                if self.view.hand {
                    let step = view::OCTAVE_W / (7.0 * view::HAND_WHITE) * 34.0;
                    self.scroll_keys(-notches * step);
                } else {
                    self.scroll_keys(-notches * 34.0);
                }
                vec![Act::Repaint]
            }
            Some(Target::Wave(area)) | Some(Target::Overview(area)) => {
                if m.ctrl || m.shift {
                    self.scroll_wave(-f64::from(notches) * self.view.wave_len / 8.0);
                } else {
                    // Zoom around the mouse, ×1.167 per notch (measured).
                    let f = self.view.wave_start + f64::from((x - area.x) / area.w) * self.view.wave_len;
                    let len = (self.view.wave_len / 1.167f64.powf(f64::from(notches))).clamp(1e-4, 1.0);
                    let u = f64::from((x - area.x) / area.w).clamp(0.0, 1.0);
                    self.view.wave_len = len;
                    self.view.wave_start = (f - u * len).clamp(0.0, (1.0 - len).max(0.0));
                }
                vec![Act::Repaint]
            }
            _ => vec![],
        }
    }

    /// Keys while the editor has the focus.
    pub fn key(&mut self, vk: u32, m: Mods) -> Vec<Act> {
        match (vk, m.ctrl) {
            (0x43, true) if self.view.tab == 0 => vec![Act::CopyAudio],
            (0x56, true) if self.view.tab == 0 => vec![Act::PasteAudio],
            _ => vec![],
        }
    }

    // Menus

    pub fn menu(&self, req: &MenuReq) -> MenuDef {
        match req {
            MenuReq::Knob(id) => self.knob_menu(id),
            MenuReq::List(id) => {
                let mut m = MenuDef::new(MenuStyle::List);
                let cur = self.vals.plain(id) as i32;
                if id == "ts_mode" {
                    // Two columns with headers (spec §2.2).
                    let layout: [(&str, &[i32]); 3] = [("Realtime", &[0, 1, 2]), ("Elastique", &[3, 4]), ("Special", &[5, 6])];
                    for (h, idx) in layout {
                        m.head(h);
                        for &i in idx {
                            let r = m.add(MenuItem::item(params::STRETCH[i as usize]), Cmd::Choice(i));
                            if i == cur {
                                m.current = Some(r);
                            }
                        }
                    }
                    m.add(MenuItem::Column, Cmd::None);
                    let r = m.add(MenuItem::item(params::STRETCH[7]), Cmd::Choice(7));
                    if cur == 7 {
                        m.current = Some(r);
                    }
                    m.head("Legacy elastique");
                    for i in 8..12 {
                        let r = m.add(MenuItem::item(params::STRETCH[i as usize]), Cmd::Choice(i));
                        if i == cur {
                            m.current = Some(r);
                        }
                    }
                } else {
                    for (i, l) in list_labels(id).iter().enumerate() {
                        let r = m.add(MenuItem::item(l), Cmd::Choice(i as i32));
                        if i as i32 == cur {
                            m.current = Some(r);
                        }
                    }
                }
                m
            }
            MenuReq::Wave => {
                let mut m = MenuDef::new(MenuStyle::Context);
                let has = self.shared.raw.lock().map(|r| r.is_some()).unwrap_or(false);
                let has_path = !self.params.sample_path.read().map(|p| p.is_empty()).unwrap_or(true);
                m.add(MenuItem::item("Reload").enabled(has_path), Cmd::Reload);
                m.add(MenuItem::item("Save as...").enabled(has), Cmd::SaveAs);
                m.add(MenuItem::item("Locate file").enabled(has_path), Cmd::Locate);
                // Audio on the clipboard: Windows and macOS so far.
                let clip = cfg!(any(windows, target_os = "macos"));
                m.add(MenuItem::item("Copy audio to clipboard").shortcut("Ctrl+C").enabled(has && clip), Cmd::CopyAudio);
                m.add(MenuItem::item("Paste audio from clipboard").shortcut("Ctrl+V").enabled(clip), Cmd::PasteAudio);
                m.sep();
                m.add(MenuItem::item("Edit in audio editor").shortcut("Ctrl+E").enabled(false), Cmd::None);
                m.add(MenuItem::item("Edit in pitch corrector").enabled(false), Cmd::None);
                m.add(MenuItem::item("Edit in time warper").enabled(false), Cmd::None);
                m.sep();
                m.add(MenuItem::item("Detect tempo").enabled(has), Cmd::DetectTempo);
                m.add(MenuItem::item("Get properties").enabled(has), Cmd::Properties);
                m.sep();
                m.add(MenuItem::item("Spectrum view").mark(Mark::Check(self.view.spectrum)), Cmd::Spectrum);
                m.add(MenuItem::item("Multichannel waveform view").mark(Mark::Check(self.view.multichannel)), Cmd::Multichannel);
                m
            }
            MenuReq::EnvPresets => {
                let mut m = MenuDef::new(MenuStyle::Context);
                m.head("Presets");
                m.add(MenuItem::item("Save as...").enabled(false), Cmd::None);
                m
            }
            MenuReq::History => {
                let mut m = MenuDef::new(MenuStyle::Context);
                m.add(MenuItem::item("Load sample file..."), Cmd::Browse);
                let hist = history();
                if !hist.is_empty() {
                    m.head("Sample history");
                    for p in hist {
                        let name = p.rsplit(['/', '\\']).next().unwrap_or(&p).to_string();
                        m.add(MenuItem::item(&name), Cmd::Recent(p));
                    }
                }
                m
            }
        }
    }

    fn knob_menu(&self, id: &str) -> MenuDef {
        let mut m = MenuDef::new(MenuStyle::Context);
        if id == "ts_time" {
            let cur = params::time_steps(self.vals.norm(id));
            let on = |s: i32| (cur - f64::from(s)).abs() < 1e-3;
            m.add(MenuItem::item("(none)").mark(Mark::Radio(on(0))), Cmd::Time(0));
            let has = self.shared.raw.lock().map(|r| r.is_some()).unwrap_or(false);
            m.add(MenuItem::item("Autodetect...").enabled(has), Cmd::Time(-1));
            m.add(MenuItem::item("Project tempo").enabled(has), Cmd::Time(-1));
            m.sep();
            // In steps (a quarter beat; spec §2.2).
            for (l, s) in [("1 beat", 4), ("2 beats", 8), ("1 bar", 16), ("2 bars", 32), ("3 bars", 48), ("4 bars", 64)] {
                m.add(MenuItem::item(l).mark(Mark::Radio(on(s))), Cmd::Time(s));
            }
            m.add(MenuItem::Column, Cmd::None);
        }
        m.add(MenuItem::item("Reset"), Cmd::Reset);
        if is_time_knob(id) {
            let i = m.add(MenuItem::item("Set").sub(), Cmd::None);
            let mut s = MenuDef::new(MenuStyle::Context);
            for n in [1, 2, 3, 4, 8, 16, 32, 64, 128] {
                s.add(MenuItem::item(&format!("{n} {}", if n == 1 { "step" } else { "steps" })), Cmd::Steps(n));
            }
            m.subs.push((i, s));
        }
        // Automation and remote control belong to the host: shown; "Create automation clip" goes to Lemon Studio (host link).
        m.head("Automation");
        for l in ["Edit events", "Edit events in new window", "Edit events in piano roll", "Init song with this position", "Delete initial value"] {
            m.add(MenuItem::item(l).enabled(false), Cmd::None);
        }
        m.sep();
        m.add(MenuItem::item("Create automation clip").enabled(self.hosted), Cmd::Automate);
        m.add(MenuItem::item("Edit automation clip...").enabled(false), Cmd::None);
        m.head("Remote control");
        m.add(MenuItem::item("Link to controller...").enabled(false), Cmd::None);
        m.add(MenuItem::item("Override global link...").enabled(false), Cmd::None);
        m.head("Value");
        m.add(MenuItem::item("Copy value"), Cmd::CopyValue);
        m.add(MenuItem::item("Paste value").enabled(self.copied.is_some()), Cmd::PasteValue);
        m.add(MenuItem::item("Type in value..."), Cmd::TypeIn);
        m.add(MenuItem::item("Set..."), Cmd::TypeIn);
        m
    }

    /// Runs a picked menu entry.
    pub fn pick(&mut self, req: &MenuReq, cmd: Cmd, x: f32, y: f32) -> Vec<Act> {
        let id = match req {
            MenuReq::Knob(id) | MenuReq::List(id) => id.clone(),
            _ => String::new(),
        };
        match cmd {
            Cmd::None => return vec![],
            Cmd::Automate => return vec![Act::Automate(id)],
            Cmd::Reset => self.reset(&id),
            Cmd::Steps(n) => {
                // A step is a 16th of a bar (4 per beat).
                let secs = f64::from(n) * 60.0 / self.shared.tempo() / 4.0;
                let falling = id.ends_with("_dec") || id.ends_with("_rel");
                self.edit(&id, time_knob_norm(secs, falling));
                self.hint_for(&id);
            }
            Cmd::CopyValue => self.copied = Some(self.vals.norm(&id)),
            Cmd::PasteValue => {
                if let Some(v) = self.copied {
                    self.edit(&id, v);
                    self.hint_for(&id);
                }
            }
            Cmd::TypeIn => return vec![Act::TypeIn(id, x, y)],
            Cmd::Time(s) => {
                // The menu sets the time and leaves the method alone (a Resample channel stays Resample).
                let steps = if s < 0 { self.detect_beats() * 4 } else { s };
                self.edit("ts_time", params::time_norm(f64::from(steps)));
                self.hint_for("ts_time");
            }
            Cmd::Choice(i) => {
                self.edit_plain(&id, i as f32);
                if id == "ts_mode" && params::stretch_realtime(i) {
                    self.realtime_picked();
                }
                self.hint_for(&id);
            }
            Cmd::Reload => return vec![Act::Reload],
            Cmd::SaveAs => return vec![Act::SaveAs],
            Cmd::Locate => return vec![Act::Locate],
            Cmd::CopyAudio => return vec![Act::CopyAudio],
            Cmd::PasteAudio => return vec![Act::PasteAudio],
            Cmd::DetectTempo => {
                let beats = self.detect_beats();
                let secs = self.shared.raw.lock().ok().and_then(|r| r.as_ref().map(|s| s.frames() as f64 / f64::from(s.rate.max(1)))).unwrap_or(0.0);
                if secs > 0.0 {
                    self.hint = format!("Tempo: {:.2} BPM ({beats} beats)", f64::from(beats) * 60.0 / secs);
                }
            }
            Cmd::Properties => {
                self.hint = self.with_sample(|sv| view::hint(&Target::WaveInfo, &self.vals, &self.view, sv, 0.0, 0.0));
            }
            Cmd::Spectrum => self.view.spectrum = !self.view.spectrum,
            Cmd::Multichannel => self.view.multichannel = !self.view.multichannel,
            Cmd::Recent(p) => return vec![Act::Load(p)],
            Cmd::Browse => return vec![Act::OpenDialog],
        }
        vec![Act::Repaint]
    }

    /// Whole beats the loaded sample lasts at the host tempo (at least 1).
    fn detect_beats(&self) -> i32 {
        let secs = self.shared.raw.lock().ok().and_then(|r| r.as_ref().map(|s| s.frames() as f64 / f64::from(s.rate.max(1)))).unwrap_or(0.0);
        ((secs * self.shared.tempo() / 60.0).round() as i32).clamp(1, (params::TIME_BARS * 4.0) as i32)
    }

    /// A typed value for `id`.
    pub fn typed(&mut self, id: &str, text: &str) {
        let frames = self.params.frames.load(Ordering::Relaxed) as f32;
        let typed = if matches!(id, "loop_start" | "loop_end") {
            text.trim().split_whitespace().next().and_then(|n| n.parse::<f32>().ok()).map(|n| (n / frames.max(1.0)).clamp(0.0, 1.0))
        } else {
            parse_value(&self.vals, id, text)
        };
        if let Some(v) = typed {
            self.stretch_touch(id);
            self.edit(id, v);
            self.hint_for(id);
        }
    }

    /// The current value as text for the type-in box.
    pub fn value_text(&self, id: &str) -> String {
        self.vals.text(id)
    }

    /// Updates the lit keys from the engine and the held key. True when they changed.
    pub fn refresh_lit(&mut self) -> bool {
        let mut lit = self.shared.sounding_keys();
        if let Some((k, _)) = self.held {
            if !lit.contains(&i32::from(k)) {
                lit.push(i32::from(k));
            }
        }
        lit.sort_unstable();
        if lit != self.view.lit {
            self.view.lit = lit;
            true
        } else {
            false
        }
    }
}

/// `OS_DEBUG` set: log editor events to stderr.
pub fn debug() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("OS_DEBUG").is_some())
}

/// Labels of a choice list.
pub fn list_labels(id: &str) -> &'static [&'static str] {
    match id {
        "declick" => params::DECLICK,
        "ts_mode" => params::STRETCH,
        "f_type" => params::FILTERS,
        _ => &[],
    }
}

/// The knobs an envelope node edits: horizontal first.
fn env_point_ids(p: &str, i: u8) -> Vec<String> {
    let id = |s: &str| format!("{p}_{s}");
    match i {
        0 => vec![id("delay")],
        1 => vec![id("att")],
        2 => vec![id("hold")],
        3 => vec![id("dec"), id("sus")],
        _ => vec![id("rel")],
    }
}

/// Recently loaded samples (newest first), `sampler-history.txt` in the user folder.
pub fn history() -> Vec<String> {
    history_file().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| t.lines().filter(|l| !l.is_empty()).map(str::to_string).collect()).unwrap_or_default()
}

fn history_file() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    os_paths::user_file("sampler-history.txt")
}

pub fn remember(path: &str) {
    let Some(f) = history_file() else { return };
    let mut h = history();
    h.retain(|p| p != path);
    h.insert(0, path.to_string());
    h.truncate(12);
    if let Some(d) = f.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(f, h.join("\n"));
}

/// A 16-bit PCM (clipboard) or 32-bit float WAV file of a sample.
pub fn wav_bytes(s: &os_engine::Sample, float: bool) -> Vec<u8> {
    let n = s.frames();
    let (bits, fmt) = if float { (32u16, 3u16) } else { (16, 1) };
    let bpf = 2 * u32::from(bits / 8);
    let data = n as u32 * bpf;
    let mut v = Vec::with_capacity(44 + data as usize);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&fmt.to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&s.rate.to_le_bytes());
    v.extend_from_slice(&(s.rate * bpf).to_le_bytes());
    v.extend_from_slice(&(bpf as u16).to_le_bytes());
    v.extend_from_slice(&bits.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data.to_le_bytes());
    for i in 0..n {
        for x in [s.left[i], s.right[i]] {
            if float {
                v.extend_from_slice(&x.to_le_bytes());
            } else {
                v.extend_from_slice(&((x.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
            }
        }
    }
    v
}

/// Where pasted audio is kept so the plugin state can name it.
pub fn paste_path() -> Option<PathBuf> {
    let d = os_paths::user_file("samples")?;
    std::fs::create_dir_all(&d).ok()?;
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    Some(d.join(format!("Pasted audio {t}.wav")))
}

/// Cached spectrum picture of the waveform panel.
#[derive(Default)]
pub struct SpectrumCache {
    key: Option<(u64, u64, u64)>,
    pub px: Vec<u32>,
    pub w: usize,
    pub h: usize,
}

impl SpectrumCache {
    pub fn get(&mut self, version: u64, st: &ViewState, s: &os_engine::Sample) -> (usize, usize, &[u32]) {
        let key = (version, st.wave_start.to_bits(), st.wave_len.to_bits());
        if self.key != Some(key) {
            let area = view::wave_area();
            let (w, h) = ((area.w as usize / 2).max(1), (area.h as usize / 2).max(1));
            let n = s.frames();
            let (v0, vl) = view::wave_range(st, n);
            self.px = view::spectrogram(s, v0, vl, w, h);
            self.w = w;
            self.h = h;
            self.key = Some(key);
        }
        (self.w, self.h, &self.px)
    }
}

/// The shown sample's tones for colourful waveforms, analysed once per sample version (spec: ui/settings-choices.md
/// "Display: Colorful waveforms (2026-10-16)").
#[derive(Default)]
pub struct ToneCache {
    key: Option<u64>,
    tones: os_dsp::tone::Tones,
}

impl ToneCache {
    pub fn get(&mut self, version: u64, s: &os_engine::Sample) -> &os_dsp::tone::Tones {
        if self.key != Some(version) {
            self.tones = os_dsp::tone::Tones::analyse_stereo(&s.left, &s.right, s.rate);
            self.key = Some(version);
        }
        &self.tones
    }
}

#[cfg(windows)]
mod win32 {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicIsize, Ordering};
    use std::sync::Arc;
    use std::time::Instant;

    use nih_plug::prelude::GuiContext;
    use os_theme::Palette;
    use os_ui::kit::menu::{self as kmenu, MenuStyle};
    use os_ui::Canvas;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{
        BeginPaint, ClientToScreen, CreateFontW, DeleteObject, EndPaint, GetMonitorInfoW, InvalidateRect, MonitorFromPoint, SetDIBitsToDevice, UpdateWindow, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, MONITORINFO, MONITOR_DEFAULTTONEAREST, PAINTSTRUCT,
    };
    use windows_sys::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
    use windows_sys::Win32::System::Ole::CF_WAVE;
    use windows_sys::Win32::UI::Controls::{EM_SETSEL, WM_MOUSELEAVE};
    use windows_sys::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, GetSaveFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT, OPENFILENAMEW,
    };
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetCapture, GetKeyState, ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_CONTROL, VK_DOWN, VK_ESCAPE, VK_MENU, VK_RETURN, VK_SHIFT, VK_UP};
    use windows_sys::Win32::UI::Shell::{DragAcceptFiles, DragFinish, DragQueryFileW, ShellExecuteW, HDROP};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    use super::{Act, Btn, Cmd, Logic, MenuDef, Mods, SpectrumCache, ToneCache};
    use crate::params::SamplerParams;
    use crate::view::{self, SampleView, ViewState};
    use crate::Shared;

    const TIMER: usize = 1;
    const WM_CHANGED: u32 = WM_APP + 1;

    struct Win {
        hwnd: HWND,
        canvas: Canvas,
        buf: Vec<u32>,
        crop: Vec<u32>,
        scale: f32,
        size: (u32, u32),
        params: Arc<SamplerParams>,
        shared: Arc<Shared>,
        logic: Logic,
        palette: Palette,
        spectrum: SpectrumCache,
        tones: ToneCache,
        /// `os_theme::waves::generation()` at the last paint: the host's colourful waveforms setting changed since.
        last_waves: u64,
        last_sig: u64,
        last_view: ViewState,
        last_ver: u64,
        last_hint: String,
        /// Cursor held here (screen px) during a knob drag.
        lock: Option<POINT>,
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

    pub fn post_changed(hwnd: isize) {
        if hwnd != 0 {
            unsafe { PostMessageW(hwnd as HWND, WM_CHANGED, 0, 0) };
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    /// One class per DLL instance: the name carries this module's address so two Lemon Studio plugins never share it.
    fn class_name(kind: &str) -> Vec<u16> {
        wide(&format!("LemonSampler{kind}{:x}", wndproc as *const () as usize))
    }

    fn register(kind: &str, proc_: WNDPROC) -> Vec<u16> {
        let class = class_name(kind);
        unsafe {
            let wc = WNDCLASSW {
                style: CS_DBLCLKS,
                lpfnWndProc: proc_,
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: GetModuleHandleW(std::ptr::null()),
                hIcon: std::ptr::null_mut(),
                hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
                hbrBackground: std::ptr::null_mut(),
                lpszMenuName: std::ptr::null(),
                lpszClassName: class.as_ptr(),
            };
            RegisterClassW(&wc);
        }
        class
    }

    pub fn open(parent: *mut c_void, scale: f32, params: Arc<SamplerParams>, shared: Arc<Shared>, ctx: Arc<dyn GuiContext>, slot: Arc<AtomicIsize>) -> Handle {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let size = ((view::EDITOR_W as f32 * scale).round() as u32, (view::EDITOR_H as f32 * scale).round() as u32);
        let full = ((view::W as f32 * scale).round() as u32, (view::H as f32 * scale).round() as u32);
        unsafe {
            let class = register("View", Some(wndproc));
            let canvas = Canvas::new(full.0, full.1, scale).expect("canvas");
            let win = Box::new(Win {
                hwnd: std::ptr::null_mut(),
                canvas,
                buf: vec![0; (full.0 * full.1) as usize],
                crop: Vec::new(),
                scale,
                size: full,
                logic: Logic::new(params.clone(), shared.clone(), Some(ctx)),
                params,
                shared,
                palette: os_plugkit::palette(),
                spectrum: SpectrumCache::default(),
                tones: ToneCache::default(),
                last_waves: 0,
                last_sig: 0,
                last_view: ViewState::default(),
                last_ver: 0,
                last_hint: String::new(),
                lock: None,
                tracking: false,
                slot: slot.clone(),
            });
            let title = wide("Lemon Sampler");
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
            DragAcceptFiles(hwnd, 1);
            SetTimer(hwnd, TIMER, 30, None);
            Handle(hwnd as isize, slot)
        }
    }

    unsafe fn state(hwnd: HWND) -> Option<&'static mut Win> {
        let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Win;
        if p.is_null() { None } else { Some(unsafe { &mut *p }) }
    }

    fn mods() -> Mods {
        let down = |vk: u16| unsafe { GetKeyState(i32::from(vk)) } < 0;
        Mods { ctrl: down(VK_CONTROL), shift: down(VK_SHIFT), alt: down(VK_MENU) }
    }

    fn xy(l: LPARAM) -> (i32, i32) {
        ((l & 0xffff) as u16 as i16 as i32, ((l >> 16) & 0xffff) as u16 as i16 as i32)
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
                    if w.logic.dragging() && w.lock.is_none() {
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
                    if let Some(anchor) = w.lock {
                        // Held cursor: the movement since the last event, then back to the anchor.
                        let mut p = POINT { x: 0, y: 0 };
                        GetCursorPos(&mut p);
                        let (dx, dy) = (p.x - anchor.x, p.y - anchor.y);
                        if dx != 0 || dy != 0 {
                            SetCursorPos(anchor.x, anchor.y);
                            let acts = w.logic.delta(dx as f32 / w.scale, dy as f32 / w.scale, mods());
                            w.run(acts);
                        }
                        return 0;
                    }
                    let (x, y) = w.mouse(lp);
                    let acts = w.logic.moved(x, y, mods());
                    w.run_hover(acts);
                    0
                }
                WM_MOUSELEAVE => {
                    w.tracking = false;
                    if w.lock.is_none() && !w.logic.dragging() {
                        let acts = w.logic.leave();
                        w.run(acts);
                    }
                    0
                }
                WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP => {
                    w.unlock();
                    ReleaseCapture();
                    let acts = w.logic.up(Btn::Left);
                    w.run(acts);
                    0
                }
                WM_CAPTURECHANGED => {
                    // Another window took the mouse (not this one capturing again): end the drag.
                    if w.logic.dragging() && lp as HWND != hwnd {
                        w.unlock();
                        let acts = w.logic.up(Btn::Left);
                        w.run(acts);
                    }
                    0
                }
                WM_KILLFOCUS => 0,
                WM_MOUSEWHEEL => {
                    // Wheel positions are screen coordinates.
                    let mut r: RECT = std::mem::zeroed();
                    GetWindowRect(hwnd, &mut r);
                    let (sx, sy) = xy(lp);
                    let (x, y) = w.to_view((sx - r.left) as f32, (sy - r.top) as f32);
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
                WM_DROPFILES => {
                    let drop = wp as HDROP;
                    let mut buf = vec![0u16; 1024];
                    let n = DragQueryFileW(drop, 0, buf.as_mut_ptr(), buf.len() as u32) as usize;
                    DragFinish(drop);
                    if n > 0 {
                        let path = String::from_utf16_lossy(&buf[..n]);
                        w.run(vec![Act::Load(path)]);
                    }
                    0
                }
                WM_DESTROY => {
                    KillTimer(hwnd, TIMER);
                    w.unlock();
                    0
                }
                _ => DefWindowProcW(hwnd, msg, wp, lp),
            }
        }
    }

    impl Win {
        /// Top of the shown crop in the painted frame: the strip row is left out when the host draws it.
        fn crop_y(&self) -> u32 {
            if self.logic.hosted { view::HOSTED_CROP_Y } else { view::CROP_Y }
        }

        /// Editor content size in logical px.
        fn content(&self) -> (u32, u32) {
            (view::EDITOR_W, if self.logic.hosted { view::HOSTED_EDITOR_H } else { view::EDITOR_H })
        }

        fn to_view(&self, x: f32, y: f32) -> (f32, f32) {
            (x / self.scale + view::CROP_X as f32, y / self.scale + self.crop_y() as f32)
        }

        /// Window-client pixels to painted-frame coordinates (the editor is a crop of the painted frame).
        fn mouse(&self, l: LPARAM) -> (f32, f32) {
            let (x, y) = xy(l);
            self.to_view(x as f32, y as f32)
        }

        /// Screen position of a painted-frame point.
        fn screen(&self, x: f32, y: f32) -> (i32, i32) {
            let mut p = POINT { x: ((x - view::CROP_X as f32) * self.scale) as i32, y: ((y - self.crop_y() as f32) * self.scale) as i32 };
            unsafe { ClientToScreen(self.hwnd, &mut p) };
            (p.x, p.y)
        }

        fn invalidate(&self) {
            unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) };
        }

        fn unlock(&mut self) {
            if let Some(p) = self.lock.take() {
                unsafe {
                    SetCursorPos(p.x, p.y);
                    while ShowCursor(1) < 0 {}
                }
            }
        }

        /// Timer / parameter change: expire confirmed overlay values, refresh the lit keys, keep the processed sample
        /// current, repaint when anything shown changed.
        fn poll(&mut self) {
            let mut dirty = self.logic.vals.expire();
            dirty |= self.logic.refresh_lit();
            if self.shared.stale(&self.params) {
                self.shared.precompute_async(&self.params);
            }
            let sig = self.logic.vals.signature();
            let ver = self.shared.version();
            let waves = os_theme::waves::generation() != self.last_waves;
            if dirty || waves || sig != self.last_sig || ver != self.last_ver || self.logic.view != self.last_view || self.logic.hint != self.last_hint {
                self.invalidate();
            }
            if self.logic.hint != self.last_hint {
                self.last_hint = self.logic.hint.clone();
                // Tell the host its hint line changed (hosted mode).
                if self.logic.hosted {
                    os_hostlink::to_host(self.hwnd as isize, os_hostlink::Cmd::Changed, 0);
                }
            }
        }

        /// Hover moves repaint only when the look under the mouse could change.
        fn run_hover(&mut self, acts: Vec<super::Act>) {
            let only_repaint = acts.iter().all(|a| *a == Act::Repaint);
            if only_repaint && !self.logic.dragging() {
                let hover_changed = self.hover_target() != self.last_hover();
                if !hover_changed && self.logic.hint == self.last_hint {
                    return;
                }
            }
            self.run(acts);
        }

        fn hover_target(&self) -> Option<view::Target> {
            let (x, y) = self.logic.view.mouse?;
            self.logic.hits.iter().rev().find(|h| h.r.contains(x, y)).map(|h| h.t.clone())
        }

        fn last_hover(&self) -> Option<view::Target> {
            let (x, y) = self.last_view.mouse?;
            self.logic.hits.iter().rev().find(|h| h.r.contains(x, y)).map(|h| h.t.clone())
        }

        fn run(&mut self, acts: Vec<Act>) {
            let mut repaint = false;
            for a in acts {
                match a {
                    Act::Repaint => repaint = true,
                    Act::Lock => unsafe {
                        let mut p = POINT { x: 0, y: 0 };
                        GetCursorPos(&mut p);
                        self.lock = Some(p);
                        let n = ShowCursor(0);
                        if super::debug() {
                            eprintln!("[lemon-sampler] lock at {},{} (cursor count {n})", p.x, p.y);
                        }
                        if n >= 0 {
                            while ShowCursor(0) >= 0 {}
                        }
                        if GetCapture() != self.hwnd {
                            SetCapture(self.hwnd);
                        }
                    },
                    Act::Menu(req, x, y) => {
                        let def = self.logic.menu(&req);
                        let (sx, sy) = self.screen(x, y);
                        if let Some(cmd) = popup(self.hwnd, sx, sy, &def, &self.palette, self.scale) {
                            let acts = self.logic.pick(&req, cmd, x, y);
                            self.run(acts);
                        }
                        repaint = true;
                    }
                    Act::TypeIn(id, x, y) => {
                        let (sx, sy) = self.screen(x, y);
                        let cur = self.logic.value_text(&id);
                        if let Some(t) = type_in(self.hwnd, sx, sy, &cur, self.scale) {
                            self.logic.typed(&id, &t);
                        }
                        repaint = true;
                    }
                    Act::Automate(id) => {
                        // The host makes the clip (spec: plugins/host-link.md); the index is the parameter's list position.
                        let i = nih_plug::params::Params::param_map(&*self.params).iter().position(|(p, _, _)| *p == id);
                        if let Some(i) = i {
                            os_hostlink::to_host(self.hwnd as isize, os_hostlink::Cmd::Automate, i as isize);
                        }
                    }
                    Act::Note(k, v, on) => {
                        let ans = os_hostlink::to_host(self.hwnd as isize, os_hostlink::Cmd::Note, os_hostlink::pack_note(k, v, on));
                        if ans != 1 {
                            if let Ok(mut q) = self.shared.previews.lock() {
                                q.push((k, f32::from(v) / 127.0, on));
                            }
                        }
                    }
                    Act::OpenDialog => {
                        if let Some(path) = file_dialog(self.hwnd, false) {
                            self.load(&path);
                        }
                        repaint = true;
                    }
                    Act::Load(p) => {
                        self.load(&p);
                        repaint = true;
                    }
                    Act::Clear => {
                        self.shared.clear(&self.params);
                        repaint = true;
                    }
                    Act::Reload => {
                        let p = self.params.sample_path.read().map(|p| p.clone()).unwrap_or_default();
                        if !p.is_empty() {
                            self.load(&p);
                        }
                        repaint = true;
                    }
                    Act::SaveAs => {
                        let s = self.shared.processed.lock().ok().and_then(|p| p.as_ref().and_then(|p| p.sample.clone()));
                        if let (Some(s), Some(path)) = (s, file_dialog(self.hwnd, true)) {
                            let path = if path.to_ascii_lowercase().ends_with(".wav") { path } else { format!("{path}.wav") };
                            self.logic.hint = match std::fs::write(&path, super::wav_bytes(&s, true)) {
                                Ok(()) => format!("Saved {path}"),
                                Err(e) => format!("Could not save: {e}"),
                            };
                        }
                        repaint = true;
                    }
                    Act::Locate => {
                        let p = self.params.sample_path.read().map(|p| p.clone()).unwrap_or_default();
                        if !p.is_empty() {
                            let args = wide(&format!("/select,\"{p}\""));
                            let exe = wide("explorer.exe");
                            let op = wide("open");
                            unsafe { ShellExecuteW(self.hwnd, op.as_ptr(), exe.as_ptr(), args.as_ptr(), std::ptr::null(), SW_SHOWNORMAL) };
                        }
                    }
                    Act::CopyAudio => {
                        let s = self.shared.processed.lock().ok().and_then(|p| p.as_ref().and_then(|p| p.sample.clone()));
                        if let Some(s) = s {
                            set_clipboard_wave(self.hwnd, &super::wav_bytes(&s, false));
                            self.logic.hint = "Audio copied to the clipboard".into();
                            repaint = true;
                        }
                    }
                    Act::PasteAudio => {
                        if let (Some(bytes), Some(path)) = (clipboard_wave(self.hwnd), super::paste_path()) {
                            if std::fs::write(&path, bytes).is_ok() {
                                self.load(&path.to_string_lossy());
                            }
                        }
                        repaint = true;
                    }
                }
            }
            if repaint {
                self.invalidate();
            }
        }

        fn load(&mut self, path: &str) {
            match self.shared.load(path, &self.params) {
                Ok(()) => {
                    super::remember(path);
                    self.logic.view.wave_start = 0.0;
                    self.logic.view.wave_len = 1.0;
                    self.logic.hint = format!("Loaded {}", path.rsplit(['/', '\\']).next().unwrap_or(path));
                }
                Err(e) => self.logic.hint = e,
            }
        }

        /// Answers the host (spec `plugins/host-link.md`). Unknown commands return 0.
        unsafe fn host_link(&mut self, wp: WPARAM, lp: LPARAM) -> LRESULT {
            use os_hostlink::{Cmd, PageIcon};
            let Some(cmd) = Cmd::from_usize(wp) else { return 0 };
            match cmd {
                Cmd::Hello => {
                    if !self.logic.hosted {
                        self.logic.hosted = true;
                        // Shrink to the content below the strip row; the host asks for the size next.
                        let (cw, ch) = self.content();
                        let px = |v: u32| (v as f32 * self.scale).round() as i32;
                        unsafe {
                            SetWindowPos(self.hwnd, std::ptr::null_mut(), 0, 0, px(cw), px(ch), SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
                        }
                        self.invalidate();
                    }
                    os_hostlink::VERSION as LRESULT
                }
                Cmd::PageCount => 2,
                Cmd::PageIcon => match lp {
                    0 => PageIcon::Sample as LRESULT,
                    _ => PageIcon::Envelope as LRESULT,
                },
                Cmd::GetPage => LRESULT::from(self.logic.view.tab),
                Cmd::SetPage => {
                    self.logic.view.tab = if lp <= 0 { 0 } else { 1 };
                    self.invalidate();
                    LRESULT::from(self.logic.view.tab)
                }
                Cmd::Metrics => {
                    let (cw, ch) = self.content();
                    os_hostlink::pack_size(cw, ch)
                }
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
            unsafe {
                let mut ps: PAINTSTRUCT = std::mem::zeroed();
                let hdc = BeginPaint(self.hwnd, &mut ps);
                let path = self.params.sample_path.read().map(|p| p.clone()).unwrap_or_default();
                let s = self.shared.processed.lock().ok().and_then(|p| p.as_ref().and_then(|p| p.sample.clone()));
                let s = s.or_else(|| self.shared.raw.lock().ok().and_then(|r| r.clone()));
                let bits = (self.shared.bits.load(Ordering::Relaxed) & 0x1ff) as u16;
                let ver = self.shared.version();
                let spectrum = match (&s, self.logic.view.spectrum) {
                    (Some(s), true) => Some(self.spectrum.get(ver, &self.logic.view, s)),
                    _ => None,
                };
                // Colourful waveforms, as the host has them.
                self.last_waves = os_theme::waves::generation();
                let waves = os_theme::waves::current();
                let tones = match &s {
                    Some(s) if waves.on() => Some(self.tones.get(ver, s)),
                    _ => None,
                };
                let sv = SampleView { sample: s.as_deref(), path: &path, bits, spectrum, tones: tones.map(|t| (t, &*waves)) };
                let hint = if self.logic.hosted { String::new() } else { self.logic.hint.clone() };
                let hits = view::paint(&mut self.canvas, &self.palette, &self.logic.vals, &self.logic.view, &sv, &hint, self.logic.hosted);
                self.logic.hits = hits;
                self.last_sig = self.logic.vals.signature();
                self.last_view = self.logic.view.clone();
                self.last_ver = ver;
                // Show the editor area: the painted frame minus FL's border.
                let (ox, oy) = ((view::CROP_X as f32 * self.scale).round() as usize, (self.crop_y() as f32 * self.scale).round() as usize);
                let (cw, ch) = self.content();
                let (w, h) = ((cw as f32 * self.scale).round() as u32, (ch as f32 * self.scale).round() as u32);
                let (w, h) = (w.min(self.size.0 - ox as u32), h.min(self.size.1 - oy as u32));
                let full = self.size.0 as usize;
                self.buf.resize(full * self.size.1 as usize, 0);
                self.canvas.copy_to_xrgb(&mut self.buf);
                self.crop.clear();
                for row in oy..oy + h as usize {
                    self.crop.extend_from_slice(&self.buf[row * full + ox..row * full + ox + w as usize]);
                }
                let mut bmi: BITMAPINFO = std::mem::zeroed();
                bmi.bmiHeader = BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w as i32,
                    biHeight: -(h as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    ..std::mem::zeroed()
                };
                SetDIBitsToDevice(hdc, 0, 0, w, h, 0, 0, 0, h, self.crop.as_ptr() as *const c_void, &bmi, DIB_RGB_COLORS);
                EndPaint(self.hwnd, &ps);
            }
        }
    }

    // -----------------------------------------------------------------------------------------------------------
    // Popup menus: an owned popup window painted with `kit::menu`, run modally until a pick or a cancel.

    struct Pop {
        canvas: Canvas,
        items: Vec<os_ui::kit::menu::MenuItem>,
        layout: kmenu::MenuLayout,
        style: MenuStyle,
        hover: Option<usize>,
        current: Option<usize>,
        palette: Palette,
        scale: f32,
        buf: Vec<u32>,
    }

    enum PopRes {
        Pick(Cmd),
        Cancel,
        /// The mouse went back to the parent menu.
        Back,
    }

    unsafe extern "system" fn pop_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        unsafe {
            if msg == WM_PAINT {
                let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Pop;
                let mut ps: PAINTSTRUCT = std::mem::zeroed();
                let hdc = BeginPaint(hwnd, &mut ps);
                if !p.is_null() {
                    let pop = &mut *p;
                    pop.canvas.begin_frame(None);
                    kmenu::paint(&mut pop.canvas, &pop.palette, &pop.items, &pop.layout, pop.style, 0.0, 0.0, pop.hover, pop.current);
                    let (w, h) = ((pop.layout.w * pop.scale).ceil() as u32, (pop.layout.h * pop.scale).ceil() as u32);
                    pop.buf.resize((w * h) as usize, 0);
                    pop.canvas.copy_to_xrgb(&mut pop.buf);
                    let mut bmi: BITMAPINFO = std::mem::zeroed();
                    bmi.bmiHeader = BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: w as i32,
                        biHeight: -(h as i32),
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB,
                        ..std::mem::zeroed()
                    };
                    SetDIBitsToDevice(hdc, 0, 0, w, h, 0, 0, 0, h, pop.buf.as_ptr() as *const c_void, &bmi, DIB_RGB_COLORS);
                }
                EndPaint(hwnd, &ps);
                return 0;
            }
            if msg == WM_ERASEBKGND {
                return 1;
            }
            if msg == WM_MOUSEACTIVATE {
                return MA_NOACTIVATE as LRESULT;
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
    }

    /// Work area of the monitor at a screen point.
    fn work_area(x: i32, y: i32) -> RECT {
        unsafe {
            let mon = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
            let mut mi: MONITORINFO = std::mem::zeroed();
            mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            GetMonitorInfoW(mon, &mut mi);
            mi.rcWork
        }
    }

    /// Shows a menu at screen (`x`, `y`); the picked entry's command.
    fn popup(owner: HWND, x: i32, y: i32, def: &MenuDef, pal: &Palette, scale: f32) -> Option<Cmd> {
        match popup_at(owner, x, y, def, pal, scale, None) {
            PopRes::Pick(c) => Some(c),
            _ => None,
        }
    }

    /// Tests that click at random set `LEMONSAMPLER_NO_MODAL`: menus, type-in boxes and file dialogs then return nothing
    /// instead of running a modal loop.
    fn no_modal() -> bool {
        std::env::var_os("LEMONSAMPLER_NO_MODAL").is_some()
    }

    fn popup_at(owner: HWND, x: i32, y: i32, def: &MenuDef, pal: &Palette, scale: f32, parent: Option<RECT>) -> PopRes {
        if no_modal() {
            return PopRes::Cancel;
        }
        unsafe {
            let class = register("Popup", Some(pop_proc));
            let mut measure = Canvas::new(8, 8, scale).expect("canvas");
            let layout = kmenu::layout(&mut measure, &def.items, def.style);
            let (w, h) = ((layout.w * scale).ceil() as i32, (layout.h * scale).ceil() as i32);
            let wa = work_area(x, y);
            let mut px = x;
            let mut py = y;
            if px + w > wa.right {
                px = match parent {
                    Some(p) => p.left - w,
                    None => wa.right - w,
                };
            }
            if py + h > wa.bottom {
                py = (wa.bottom - h).max(wa.top);
            }
            let pop = Box::new(Pop {
                canvas: Canvas::new(w.max(1) as u32, h.max(1) as u32, scale).expect("canvas"),
                items: def.items.clone(),
                layout,
                style: def.style,
                hover: None,
                current: def.current,
                palette: pal.clone(),
                scale,
                buf: Vec::new(),
            });
            let root = GetAncestor(owner, GA_ROOT);
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                class.as_ptr(),
                std::ptr::null(),
                WS_POPUP,
                px,
                py,
                w,
                h,
                root,
                std::ptr::null_mut(),
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null(),
            );
            let pop = Box::into_raw(pop);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, pop as isize);
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            SetCapture(hwnd);
            let rect = RECT { left: px, top: py, right: px + w, bottom: py + h };
            let opened = Instant::now();
            let mut start = POINT { x: 0, y: 0 };
            GetCursorPos(&mut start);
            let mut moved = false;
            let mut sub_open: Option<usize> = None;
            let item_at = |pop: &Pop, sx: i32, sy: i32| -> Option<usize> {
                let (lx, ly) = ((sx - px) as f32 / scale, (sy - py) as f32 / scale);
                pop.layout.item_at(&pop.items, lx, ly)
            };
            let inside = |r: &RECT, sx: i32, sy: i32| sx >= r.left && sx < r.right && sy >= r.top && sy < r.bottom;
            let result = 'run: loop {
                let mut msg: MSG = std::mem::zeroed();
                if GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) <= 0 {
                    PostQuitMessage(msg.wParam as i32);
                    break PopRes::Cancel;
                }
                if GetCapture() != hwnd && msg.message != WM_PAINT && msg.message != WM_TIMER {
                    SetCapture(hwnd);
                }
                let mut cur = POINT { x: 0, y: 0 };
                GetCursorPos(&mut cur);
                match msg.message {
                    WM_MOUSEMOVE => {
                        if (cur.x - start.x).abs() + (cur.y - start.y).abs() > 2 {
                            moved = true;
                        }
                        if !inside(&rect, cur.x, cur.y) {
                            if let Some(pr) = parent {
                                if inside(&pr, cur.x, cur.y) {
                                    break 'run PopRes::Back;
                                }
                            }
                        }
                        let hov = if inside(&rect, cur.x, cur.y) { item_at(&*pop, cur.x, cur.y) } else { None };
                        let hov = hov.filter(|&i| def.items[i].clickable());
                        if hov != (*pop).hover {
                            (*pop).hover = hov;
                            InvalidateRect(hwnd, std::ptr::null(), 0);
                            UpdateWindow(hwnd);
                        }
                        // Submenu: opens while its row is hovered.
                        if let Some(i) = hov {
                            if sub_open != Some(i) {
                                if let Some(sub) = def.sub(i) {
                                    sub_open = Some(i);
                                    let row = (&(*pop).layout.rows)[i];
                                    let sx = px + ((row.x + row.w) * scale) as i32 - 2;
                                    let sy = py + (row.y * scale) as i32 - 2;
                                    match popup_at(owner, sx, sy, sub, pal, scale, Some(rect)) {
                                        PopRes::Pick(c) => break 'run PopRes::Pick(c),
                                        PopRes::Cancel => break 'run PopRes::Cancel,
                                        PopRes::Back => {
                                            SetCapture(hwnd);
                                        }
                                    }
                                }
                            }
                        } else {
                            sub_open = None;
                        }
                    }
                    WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_NCLBUTTONDOWN | WM_NCRBUTTONDOWN => {
                        if !inside(&rect, cur.x, cur.y) {
                            break 'run PopRes::Cancel;
                        }
                        moved = true;
                    }
                    WM_LBUTTONUP | WM_RBUTTONUP => {
                        // The release of the click that opened the menu does not pick.
                        if !moved && opened.elapsed().as_millis() < 400 {
                            continue;
                        }
                        if inside(&rect, cur.x, cur.y) {
                            if let Some(i) = item_at(&*pop, cur.x, cur.y) {
                                if def.items[i].clickable() && def.sub(i).is_none() {
                                    break 'run PopRes::Pick(def.cmds[i].clone());
                                }
                            }
                        }
                    }
                    WM_KEYDOWN | WM_SYSKEYDOWN => {
                        let vk = msg.wParam as u16;
                        if vk == VK_ESCAPE {
                            break 'run PopRes::Cancel;
                        }
                        let n = def.items.len();
                        if vk == VK_DOWN || vk == VK_UP {
                            let mut i = (*pop).hover.or((*pop).current).map_or(if vk == VK_DOWN { n - 1 } else { 0 }, |i| i);
                            for _ in 0..n {
                                i = if vk == VK_DOWN { (i + 1) % n } else { (i + n - 1) % n };
                                if def.items[i].clickable() {
                                    break;
                                }
                            }
                            (*pop).hover = Some(i);
                            InvalidateRect(hwnd, std::ptr::null(), 0);
                        } else if vk == VK_RETURN {
                            if let Some(i) = (*pop).hover {
                                if def.items[i].clickable() && def.sub(i).is_none() {
                                    break 'run PopRes::Pick(def.cmds[i].clone());
                                }
                            }
                        }
                    }
                    WM_CANCELMODE | WM_ACTIVATEAPP => break 'run PopRes::Cancel,
                    _ => {
                        TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
            };
            ReleaseCapture();
            DestroyWindow(hwnd);
            drop(Box::from_raw(pop));
            result
        }
    }

    /// A small edit box at screen (`x`, `y`) for "Type in value"; Enter accepts, Esc or a click elsewhere cancels.
    fn type_in(owner: HWND, x: i32, y: i32, text: &str, scale: f32) -> Option<String> {
        if no_modal() {
            return None;
        }
        unsafe {
            let class = wide("EDIT");
            let init = wide(text);
            let (w, h) = ((140.0 * scale) as i32, (22.0 * scale) as i32);
            let root = GetAncestor(owner, GA_ROOT);
            let edit = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                class.as_ptr(),
                init.as_ptr(),
                WS_POPUP | WS_BORDER | ES_AUTOHSCROLL as u32,
                x,
                y,
                w,
                h,
                root,
                std::ptr::null_mut(),
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null(),
            );
            if edit.is_null() {
                return None;
            }
            let face = wide("Segoe UI");
            let font = CreateFontW(-(14.0 * scale) as i32, 0, 0, 0, 400, 0, 0, 0, 0, 0, 0, 5, 0, face.as_ptr());
            SendMessageW(edit, WM_SETFONT, font as WPARAM, 1);
            ShowWindow(edit, SW_SHOW);
            SetForegroundWindow(edit);
            SetFocus(edit);
            SendMessageW(edit, EM_SETSEL, 0, -1);
            let result = loop {
                let mut msg: MSG = std::mem::zeroed();
                if GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) <= 0 {
                    PostQuitMessage(msg.wParam as i32);
                    break None;
                }
                if msg.message == WM_KEYDOWN && msg.wParam as u16 == VK_RETURN {
                    let mut buf = vec![0u16; 256];
                    let n = GetWindowTextW(edit, buf.as_mut_ptr(), buf.len() as i32) as usize;
                    break Some(String::from_utf16_lossy(&buf[..n]));
                }
                if msg.message == WM_KEYDOWN && msg.wParam as u16 == VK_ESCAPE {
                    break None;
                }
                if matches!(msg.message, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_NCLBUTTONDOWN) && msg.hwnd != edit {
                    break None;
                }
                if GetFocus() != edit && msg.message == WM_KEYDOWN {
                    break None;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            };
            DestroyWindow(edit);
            DeleteObject(font);
            SetFocus(owner);
            result
        }
    }

    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus;

    fn file_dialog(owner: HWND, save: bool) -> Option<String> {
        if no_modal() {
            return None;
        }
        let mut file = vec![0u16; 1024];
        let filter: Vec<u16> = if save { "WAV files\0*.wav\0\0" } else { "Audio files\0*.wav;*.flac;*.ogg;*.wv;*.aif;*.aiff;*.mp3\0All files\0*.*\0\0" }
            .encode_utf16()
            .collect();
        let title = wide(if save { "Save sample as" } else { "Open sample" });
        unsafe {
            let mut ofn: OPENFILENAMEW = std::mem::zeroed();
            ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
            ofn.hwndOwner = owner;
            ofn.lpstrFilter = filter.as_ptr();
            ofn.lpstrFile = file.as_mut_ptr();
            ofn.nMaxFile = file.len() as u32;
            ofn.lpstrTitle = title.as_ptr();
            ofn.Flags = OFN_NOCHANGEDIR | if save { OFN_OVERWRITEPROMPT } else { OFN_FILEMUSTEXIST };
            let ok = if save { GetSaveFileNameW(&mut ofn) } else { GetOpenFileNameW(&mut ofn) };
            if ok == 0 {
                return None;
            }
        }
        let end = file.iter().position(|&c| c == 0).unwrap_or(file.len());
        Some(String::from_utf16_lossy(&file[..end]))
    }

    fn set_clipboard_wave(owner: HWND, bytes: &[u8]) {
        unsafe {
            if OpenClipboard(owner) == 0 {
                return;
            }
            EmptyClipboard();
            let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
            if !h.is_null() {
                let p = GlobalLock(h) as *mut u8;
                if !p.is_null() {
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
                    GlobalUnlock(h);
                    SetClipboardData(u32::from(CF_WAVE), h);
                }
            }
            CloseClipboard();
        }
    }

    fn clipboard_wave(owner: HWND) -> Option<Vec<u8>> {
        unsafe {
            if OpenClipboard(owner) == 0 {
                return None;
            }
            let h = GetClipboardData(u32::from(CF_WAVE));
            let out = if h.is_null() {
                None
            } else {
                let size = GlobalSize(h);
                let p = GlobalLock(h) as *const u8;
                let v = (!p.is_null()).then(|| std::slice::from_raw_parts(p, size).to_vec());
                GlobalUnlock(h);
                v
            };
            CloseClipboard();
            out
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

    use super::{Act, Btn, Logic, Mods, SpectrumCache, ToneCache};
    use crate::params::SamplerParams;
    use crate::view::{self, SampleView, ViewState};
    use crate::Shared;

    /// What the copy / paste audio shortcuts and menu entries say on Linux: no audio clipboard there yet.
    #[cfg(not(target_os = "macos"))]
    const NO_AUDIO_CLIPBOARD: &str = "Audio on the clipboard: not available yet on Linux";

    /// The editor on its GUI thread.
    struct Ed {
        win: Window,
        canvas: Canvas,
        buf: Vec<u32>,
        crop: Vec<u32>,
        scale: f32,
        size: (u32, u32),
        params: Arc<SamplerParams>,
        shared: Arc<Shared>,
        logic: Logic,
        palette: Palette,
        spectrum: SpectrumCache,
        tones: ToneCache,
        /// `os_theme::waves::generation()` at the last paint: the host's colourful waveforms setting changed since.
        last_waves: u64,
        last_sig: u64,
        last_view: ViewState,
        last_ver: u64,
        last_hint: String,
        dirty: bool,
        /// The host link: the host finds the editor by the window it gave it (spec: plugins/host-link.md).
        link: Arc<Peer>,
        /// The page as last agreed with the host.
        last_page: usize,
    }

    pub fn open(parent: px::Parent, scale: f32, params: Arc<SamplerParams>, shared: Arc<Shared>, ctx: Arc<dyn GuiContext>, slot: &px::Slot) -> Option<px::EditorThread> {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let px = move |v: u32| (v as f32 * scale).round() as u32;
        // Two pages (sample, envelopes); joined, the editor is what lies below the strip row.
        let wake = slot.clone();
        let link = Peer::register(parent as u64, vec![PageIcon::Sample, PageIcon::Envelope], (view::EDITOR_W, view::HOSTED_EDITOR_H), move || px::notify(&wake));
        let peer = link.clone();
        let thread = px::spawn(parent, (px(view::EDITOR_W), px(view::EDITOR_H)), slot, move |win| {
            let full = (px(view::W), px(view::H));
            let Ok(canvas) = Canvas::new(full.0, full.1, scale) else { return };
            let logic = Logic::new(params.clone(), shared.clone(), Some(ctx));
            let ed = Ed {
                win,
                canvas,
                buf: Vec::new(),
                crop: Vec::new(),
                scale,
                size: full,
                params,
                shared,
                logic,
                palette: os_plugkit::palette(),
                spectrum: SpectrumCache::default(),
                tones: ToneCache::default(),
                last_waves: 0,
                last_sig: 0,
                last_view: ViewState::default(),
                last_ver: 0,
                last_hint: String::new(),
                dirty: true,
                link: peer,
                last_page: 0,
            };
            // Sample files dropped on the editor load (as on Windows).
            #[cfg(target_os = "macos")]
            ed.win.accept_files();
            ed.run();
        });
        if thread.is_none() {
            link.unregister();
        }
        thread
    }

    impl Drop for Ed {
        /// The editor closes: the host no longer finds it.
        fn drop(&mut self) {
            self.link.unregister();
        }
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
                        let (x, y) = self.to_view(x, y);
                        let acts = self.logic.down(x, y, btn(b), self.mods(), dbl && b == Button::Left);
                        self.act(acts);
                    }
                    Input::Move { x, y } => {
                        let (x, y) = self.to_view(x, y);
                        let acts = self.logic.moved(x, y, self.mods());
                        self.act_hover(acts);
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
                        let (x, y) = self.to_view(x, y);
                        let acts = self.logic.wheel(x, y, notches, self.mods());
                        self.act(acts);
                    }
                    // The logic takes Windows key codes: letters are their upper case.
                    Input::Key { text: Some(c), .. } if c.is_ascii_alphabetic() => {
                        let acts = self.logic.key(u32::from(c.to_ascii_uppercase()), self.mods());
                        self.act(acts);
                    }
                    Input::Key { .. } => {}
                    // A sample file dropped on the editor (macOS so far).
                    Input::Drop { path } => self.act(vec![Act::Load(path)]),
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

        /// Top of the shown crop in the painted frame: the strip row is left out when the host draws it.
        fn crop_y(&self) -> u32 {
            if self.logic.hosted { view::HOSTED_CROP_Y } else { view::CROP_Y }
        }

        /// Editor content size in logical px.
        fn content(&self) -> (u32, u32) {
            (view::EDITOR_W, if self.logic.hosted { view::HOSTED_EDITOR_H } else { view::EDITOR_H })
        }

        /// Window device px to painted-frame coordinates (the editor is a crop of the painted frame).
        fn to_view(&self, x: f32, y: f32) -> (f32, f32) {
            (x / self.scale + view::CROP_X as f32, y / self.scale + self.crop_y() as f32)
        }

        /// Screen position of a painted-frame point.
        fn screen(&self, x: f32, y: f32) -> (i32, i32) {
            self.win.to_root(((x - view::CROP_X as f32) * self.scale) as i32, ((y - self.crop_y() as f32) * self.scale) as i32)
        }

        /// Timer / parameter change: expire confirmed overlay values, refresh the lit keys, keep the processed sample
        /// current, repaint when anything shown changed.
        fn poll(&mut self) {
            let mut dirty = self.logic.vals.expire();
            dirty |= self.logic.refresh_lit();
            if self.shared.stale(&self.params) {
                self.shared.precompute_async(&self.params);
            }
            let sig = self.logic.vals.signature();
            let ver = self.shared.version();
            let waves = os_theme::waves::generation() != self.last_waves;
            if dirty || waves || sig != self.last_sig || ver != self.last_ver || self.logic.view != self.last_view || self.logic.hint != self.last_hint {
                self.dirty = true;
            }
            if self.logic.hint != self.last_hint {
                self.last_hint.clone_from(&self.logic.hint);
                // The host shows the hint in its title bar (hosted mode).
                self.link.set_hint(&self.logic.hint);
                self.link.to_host(Cmd::Changed, 0);
            }
            self.sync_link();
        }

        /// Follows the host link: joining the strip (the editor shrinks to the content below the strip row, which the
        /// host now draws) and the host's page; a page the editor switched to itself goes to the host.
        fn sync_link(&mut self) {
            if self.link.hosted() && !self.logic.hosted {
                self.logic.hosted = true;
                let (cw, ch) = self.content();
                let px = |v: u32| (v as f32 * self.scale).round() as u32;
                self.win.resize(px(cw), px(ch));
                self.dirty = true;
            }
            let page = self.link.page();
            if page != self.last_page {
                self.logic.view.tab = u8::from(page > 0);
                self.last_page = page;
                self.dirty = true;
            } else if usize::from(self.logic.view.tab) != page {
                self.last_page = usize::from(self.logic.view.tab);
                self.link.set_page(self.last_page);
                self.link.to_host(Cmd::Changed, 0);
            }
        }

        /// Hover moves repaint only when the look under the mouse could change.
        fn act_hover(&mut self, acts: Vec<Act>) {
            let only_repaint = acts.iter().all(|a| *a == Act::Repaint);
            if only_repaint && !self.logic.dragging() && self.hover_target() == self.last_hover() && self.logic.hint == self.last_hint {
                return;
            }
            self.act(acts);
        }

        fn hover_target(&self) -> Option<view::Target> {
            let (x, y) = self.logic.view.mouse?;
            self.logic.hits.iter().rev().find(|h| h.r.contains(x, y)).map(|h| h.t.clone())
        }

        fn last_hover(&self) -> Option<view::Target> {
            let (x, y) = self.last_view.mouse?;
            self.logic.hits.iter().rev().find(|h| h.r.contains(x, y)).map(|h| h.t.clone())
        }

        fn act(&mut self, acts: Vec<Act>) {
            for a in acts {
                match a {
                    Act::Repaint => self.dirty = true,
                    Act::Lock => self.win.lock(),
                    Act::Menu(req, x, y) => {
                        let def = self.logic.menu(&req);
                        let (sx, sy) = self.screen(x, y);
                        if let Some(cmd) = self.win.popup(sx, sy, &def.kit(), &self.palette, self.scale) {
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
                    Act::Automate(id) => {
                        // The host makes the clip (spec: plugins/host-link.md); the index is the parameter's list position.
                        let i = nih_plug::params::Params::param_map(&*self.params).iter().position(|(p, _, _)| *p == id);
                        if let Some(i) = i {
                            self.link.to_host(Cmd::Automate, i as isize);
                        }
                    }
                    // Hosted, the host plays the preview notes on its live stream; otherwise the plugin does.
                    Act::Note(k, v, on) => {
                        if self.link.to_host(Cmd::Note, os_hostlink::pack_note(k, v, on)) != 1 {
                            if let Ok(mut q) = self.shared.previews.lock() {
                                q.push((k, f32::from(v) / 127.0, on));
                            }
                        }
                    }
                    Act::OpenDialog => {
                        if let Some(path) = file_dialog(&self.win, false) {
                            self.load(&path);
                        }
                        self.dirty = true;
                    }
                    Act::Load(p) => {
                        self.load(&p);
                        self.dirty = true;
                    }
                    Act::Clear => {
                        self.shared.clear(&self.params);
                        self.dirty = true;
                    }
                    Act::Reload => {
                        let p = self.params.sample_path.read().map(|p| p.clone()).unwrap_or_default();
                        if !p.is_empty() {
                            self.load(&p);
                        }
                        self.dirty = true;
                    }
                    Act::SaveAs => {
                        let s = self.shared.processed.lock().ok().and_then(|p| p.as_ref().and_then(|p| p.sample.clone()));
                        if let (Some(s), Some(path)) = (s, file_dialog(&self.win, true)) {
                            let path = if path.to_ascii_lowercase().ends_with(".wav") { path } else { format!("{path}.wav") };
                            self.logic.hint = match std::fs::write(&path, super::wav_bytes(&s, true)) {
                                Ok(()) => format!("Saved {path}"),
                                Err(e) => format!("Could not save: {e}"),
                            };
                        }
                        self.dirty = true;
                    }
                    Act::Locate => {
                        // The sample's folder in the file manager (on macOS a Finder window with the sample picked).
                        let p = self.params.sample_path.read().map(|p| p.clone()).unwrap_or_default();
                        if let Some(dir) = std::path::Path::new(&p).parent().filter(|d| !p.is_empty() && d.is_dir()) {
                            #[cfg(target_os = "macos")]
                            let _ = (dir, std::process::Command::new("open").arg("-R").arg(&p).spawn());
                            #[cfg(not(target_os = "macos"))]
                            let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
                        }
                    }
                    #[cfg(target_os = "macos")]
                    Act::CopyAudio => {
                        let s = self.shared.processed.lock().ok().and_then(|p| p.as_ref().and_then(|p| p.sample.clone()));
                        if let Some(s) = s {
                            if self.win.copy_audio(super::wav_bytes(&s, false)) {
                                self.logic.hint = "Audio copied to the clipboard".into();
                            }
                        }
                        self.dirty = true;
                    }
                    #[cfg(target_os = "macos")]
                    Act::PasteAudio => {
                        if let (Some(bytes), Some(path)) = (self.win.paste_audio(), super::paste_path()) {
                            if std::fs::write(&path, bytes).is_ok() {
                                self.load(&path.to_string_lossy());
                            }
                        }
                        self.dirty = true;
                    }
                    #[cfg(not(target_os = "macos"))]
                    Act::CopyAudio | Act::PasteAudio => {
                        self.logic.hint = NO_AUDIO_CLIPBOARD.into();
                        self.dirty = true;
                    }
                }
            }
        }

        fn load(&mut self, path: &str) {
            match self.shared.load(path, &self.params) {
                Ok(()) => {
                    super::remember(path);
                    self.logic.view.wave_start = 0.0;
                    self.logic.view.wave_len = 1.0;
                    self.logic.hint = format!("Loaded {}", path.rsplit(['/', '\\']).next().unwrap_or(path));
                }
                Err(e) => self.logic.hint = e,
            }
        }

        fn paint(&mut self) {
            self.dirty = false;
            let path = self.params.sample_path.read().map(|p| p.clone()).unwrap_or_default();
            let s = self.shared.processed.lock().ok().and_then(|p| p.as_ref().and_then(|p| p.sample.clone()));
            let s = s.or_else(|| self.shared.raw.lock().ok().and_then(|r| r.clone()));
            let bits = (self.shared.bits.load(Ordering::Relaxed) & 0x1ff) as u16;
            let ver = self.shared.version();
            let spectrum = match (&s, self.logic.view.spectrum) {
                (Some(s), true) => Some(self.spectrum.get(ver, &self.logic.view, s)),
                _ => None,
            };
            // Colourful waveforms, as the host has them.
            self.last_waves = os_theme::waves::generation();
            let waves = os_theme::waves::current();
            let tones = match &s {
                Some(s) if waves.on() => Some(self.tones.get(ver, s)),
                _ => None,
            };
            let sv = SampleView { sample: s.as_deref(), path: &path, bits, spectrum, tones: tones.map(|t| (t, &*waves)) };
            let hint = if self.logic.hosted { String::new() } else { self.logic.hint.clone() };
            self.logic.hits = view::paint(&mut self.canvas, &self.palette, &self.logic.vals, &self.logic.view, &sv, &hint, self.logic.hosted);
            self.last_sig = self.logic.vals.signature();
            self.last_view = self.logic.view.clone();
            self.last_ver = ver;
            // Show the editor area: the painted frame minus FL's border.
            let (ox, oy) = ((view::CROP_X as f32 * self.scale).round() as usize, (self.crop_y() as f32 * self.scale).round() as usize);
            let (cw, ch) = self.content();
            let (w, h) = ((cw as f32 * self.scale).round() as u32, (ch as f32 * self.scale).round() as u32);
            let (w, h) = (w.min(self.size.0 - ox as u32), h.min(self.size.1 - oy as u32));
            let full = self.size.0 as usize;
            self.buf.resize(full * self.size.1 as usize, 0);
            self.canvas.copy_to_xrgb(&mut self.buf);
            self.crop.clear();
            for row in oy..oy + h as usize {
                self.crop.extend_from_slice(&self.buf[row * full + ox..row * full + ox + w as usize]);
            }
            self.win.blit(&self.crop, w, h);
        }
    }

    /// The open / save sample dialog: the desktop's own (through its portal on Linux; on macOS AppKit's, run on the main
    /// thread).
    fn file_dialog(win: &Window, save: bool) -> Option<String> {
        if os_plugkit::no_modal() {
            return None;
        }
        let path = win.dialog(move || {
            let d = rfd::FileDialog::new();
            if save {
                d.set_title("Save sample as").add_filter("WAV files", &["wav"]).save_file()
            } else {
                d.set_title("Open sample").add_filter("Audio files", &["wav", "flac", "ogg", "wv", "aif", "aiff", "mp3"]).pick_file()
            }
        });
        path.flatten().map(|p| p.to_string_lossy().into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn logic(tab: u8) -> Logic {
        let params = Arc::new(SamplerParams::default());
        let mut l = Logic::new(params, Arc::new(Shared::default()), None);
        l.view.tab = tab;
        l.view.sub = 4;
        paint(&mut l);
        l
    }

    fn paint(l: &mut Logic) {
        let mut c = os_ui::Canvas::new(view::W, view::H, 1.0).unwrap();
        let sv = SampleView { sample: None, path: "", bits: 16, spectrum: None, tones: None };
        l.hits = view::paint(&mut c, &os_plugkit::palette(), &l.vals, &l.view, &sv, "", false);
    }

    // spec: ui/settings-choices.md "Display: Colorful waveforms (2026-10-16)"
    /// With the host's colourful waveforms on, the waveform and its overview strip take the colours of the sample's
    /// tones (a 100 Hz sine in Multiband: orange-red); off, they keep their greys (only the panel's own marks are red).
    #[test]
    fn colourful_waveforms_colour_the_wave_and_the_overview() {
        use os_theme::waves::{Mode, Waves};
        let l = logic(0);
        let rate = 44100u32;
        let sine: Vec<f32> = (0..rate).map(|i| (std::f32::consts::TAU * 100.0 * i as f32 / rate as f32).sin() * 0.8).collect();
        let s = os_engine::Sample { rate, left: sine.clone(), right: sine };
        let tones = os_dsp::tone::Tones::analyse_stereo(&s.left, &s.right, s.rate);
        let waves = Waves::new(Mode::Multiband, &[]);
        let draw = |tones: Option<(&os_dsp::tone::Tones, &Waves)>| {
            let mut c = os_ui::Canvas::new(view::W, view::H, 1.0).unwrap();
            let sv = SampleView { sample: Some(&s), path: "sine.wav", bits: 16, spectrum: None, tones };
            view::paint(&mut c, &os_plugkit::palette(), &l.vals, &l.view, &sv, "", false);
            let mut px = vec![0u32; (view::W * view::H) as usize];
            c.copy_to_xrgb(&mut px);
            px
        };
        let red = |px: &[u32], r: os_ui::Rect| {
            let (x0, y0) = (r.x.ceil() as u32, r.y.ceil() as u32);
            let (x1, y1) = (r.right().floor() as u32, r.bottom().floor() as u32);
            (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|&(x, y)| {
                let v = px[(y * view::W + x) as usize];
                let (r, g, b) = ((v >> 16) & 0xff, (v >> 8) & 0xff, v & 0xff);
                r > g + 40 && r > b + 60
            }).count()
        };
        let (plain, colour) = (draw(None), draw(Some((&tones, &waves))));
        for (what, area) in [("waveform", view::wave_area()), ("overview", view::overview_track())] {
            let (before, after) = (red(&plain, area), red(&colour, area));
            assert!(after > before + 100, "{what}: {before} orange-red pixels grey, {after} coloured");
        }
    }

    fn centre(l: &Logic, t: &Target) -> (f32, f32) {
        let h = l.hits.iter().rev().find(|h| &h.t == t).unwrap_or_else(|| panic!("no hit {t:?}"));
        (h.r.cx(), h.r.cy())
    }

    #[test]
    fn knob_drags_lock_the_cursor_and_follow_the_measured_speeds() {
        let mut l = logic(0);
        let k = Target::Knob("start_offset".into());
        let (x, y) = centre(&l, &k);
        let acts = l.down(x, y, Btn::Left, Mods::default(), false);
        assert!(acts.contains(&Act::Lock));
        l.delta(0.0, -20.0, Mods::default());
        assert!((l.vals.norm("start_offset") - 0.01).abs() < 1e-5);
        // Ctrl: a quarter of the speed.
        l.delta(0.0, -20.0, Mods { ctrl: true, ..Mods::default() });
        assert!((l.vals.norm("start_offset") - 0.0125).abs() < 1e-5);
        l.up(Btn::Left);
        assert!(!l.dragging());
        // Middle click resets.
        l.down(x, y, Btn::Middle, Mods::default(), false);
        assert_eq!(l.vals.norm("start_offset"), 0.0);
        // Wheel: 8 px per notch over the knob's 2000 px.
        l.wheel(x, y, 1.0, Mods::default());
        assert!((l.vals.norm("start_offset") - 0.004).abs() < 1e-5);
        // Alt+wheel jumps to the end.
        l.wheel(x, y, 1.0, Mods { alt: true, ..Mods::default() });
        assert_eq!(l.vals.norm("start_offset"), 1.0);
    }

    #[test]
    fn greyed_out_controls_only_say_so() {
        let mut l = logic(0);
        for id in ["keep_disk", "resample", "load_regions", "load_slices"] {
            let t = Target::Toggle(id.into());
            let v0 = l.vals.plain(id);
            let (x, y) = centre(&l, &t);
            for btn in [Btn::Left, Btn::Middle, Btn::Right] {
                l.down(x, y, btn, Mods::default(), false);
            }
            assert_eq!(l.vals.plain(id), v0, "{id}");
            assert!(l.hint.ends_with(": not available yet"), "{}", l.hint);
        }
        assert!(view::works(&Target::Toggle("loop_use".into())));
        // The envelope presets menu has no presets yet.
        let mut l = logic(1);
        let (x, y) = centre(&l, &Target::EnvMenu);
        assert!(!l.down(x, y, Btn::Left, Mods::default(), false).iter().any(|a| matches!(a, Act::Menu(..))));
        assert_eq!(l.hint, "Channel envelope presets: not available yet");
    }

    #[test]
    fn pitch_switches_resample_to_auto() {
        let mut l = logic(0);
        assert_eq!(l.vals.plain("ts_mode") as i32, 0);
        for id in ["ts_mul", "ts_time"] {
            let (x, y) = centre(&l, &Target::Knob(id.into()));
            l.wheel(x, y, 1.0, Mods::default());
            assert_eq!(l.vals.plain("ts_mode") as i32, 0, "{id} leaves Resample alone");
        }
        let (x, y) = centre(&l, &Target::Knob("ts_pitch".into()));
        l.wheel(x, y, 1.0, Mods::default());
        assert_eq!(l.vals.plain("ts_mode") as i32, params::STRETCH_AUTO);
    }

    // spec §5.6: Stretch and Stretch Pro picked with TIME off fit the sample to its own length at the host tempo.
    #[test]
    fn picking_a_realtime_method_sets_time_to_the_sample() {
        let shared = Arc::new(Shared::default());
        // 6.4 s at 130 BPM: 2662.4 / 192 beats, stored as 2662.
        *shared.raw.lock().unwrap() = Some(Arc::new(os_engine::Sample { rate: 1000, left: vec![0.0; 6400], right: vec![0.0; 6400] }));
        shared.set_tempo(130.0);
        let mut l = Logic::new(Arc::new(SamplerParams::default()), shared, None);
        paint(&mut l);
        let req = MenuReq::List("ts_mode".into());
        let time = |l: &Logic| params::time_file(params::time_steps(l.vals.norm("ts_time")));
        l.pick(&req, Cmd::Choice(3), 0.0, 0.0);
        assert_eq!(time(&l), 0.0, "e3 generic leaves TIME off");
        l.pick(&req, Cmd::Choice(1), 0.0, 0.0);
        assert!((time(&l) - 2662.0).abs() < 0.05, "{}", time(&l));
        // A TIME already set stays.
        l.pick(&MenuReq::Knob("ts_time".into()), Cmd::Time(16), 0.0, 0.0);
        l.pick(&req, Cmd::Choice(2), 0.0, 0.0);
        assert!((time(&l) - 768.0).abs() < 0.05, "{}", time(&l));
    }

    // spec §2.1, §2.2, §4: TIME counts bars:steps:ticks, its menu leaves the method alone, drags land on whole steps.
    #[test]
    fn time_knob_counts_bars_steps_and_ticks() {
        let mut l = logic(0);
        let req = MenuReq::Knob("ts_time".into());
        let steps = |l: &Logic| params::time_steps(l.vals.norm("ts_time"));
        l.pick(&req, Cmd::Time(4), 0.0, 0.0);
        assert_eq!(params::time_text(steps(&l)), "0:04:00", "1 beat");
        assert_eq!(l.vals.plain("ts_mode") as i32, 0, "a Resample channel stays Resample");
        assert_eq!(params::time_text(4.488 * 4.0), "1:01:23");
        l.typed("ts_time", "1:01:23");
        assert!((steps(&l) - (17.0 + 23.0 / 24.0)).abs() < 1e-3, "{}", steps(&l));
        // 16 px of single steps, then a bar per 36 px.
        l.pick(&req, Cmd::Time(0), 0.0, 0.0);
        let (x, y) = centre(&l, &Target::Knob("ts_time".into()));
        l.down(x, y, Btn::Left, Mods::default(), false);
        l.delta(0.0, -10.0, Mods::default());
        assert!((steps(&l) - 10.0).abs() < 1e-3);
        l.delta(0.0, -42.0, Mods::default());
        assert!((steps(&l) - 32.0).abs() < 1e-3, "two bars: {}", steps(&l));
        l.delta(0.0, -1.0, Mods::default());
        assert!((steps(&l) - 32.0).abs() < 1e-3, "on whole steps: {}", steps(&l));
        l.delta(0.0, -2.0, Mods::default());
        assert!((steps(&l) - 33.0).abs() < 1e-3, "{}", steps(&l));
        l.up(Btn::Left);
        // A stored time keeps its fractions.
        assert_eq!(params::time_from_file(params::time_file(17.5)), 17.5);
        assert!((params::time_steps(params::time_norm(1234.25)) - 1234.25).abs() < 0.01);
    }

    #[test]
    fn menus_and_typed_values() {
        let mut l = logic(1);
        let m = l.menu(&MenuReq::Knob("envpitch_att".into()));
        let set = m.items.iter().position(|i| matches!(i, MenuItem::Item { label, .. } if label == "Set")).expect("Set row");
        assert_eq!(m.sub(set).map(|s| s.items.len()), Some(9));
        let before = l.vals.norm("envpitch_att");
        l.pick(&MenuReq::Knob("envpitch_att".into()), Cmd::Steps(4), 0.0, 0.0);
        assert_ne!(l.vals.norm("envpitch_att"), before);
        // Copy / paste value.
        l.pick(&MenuReq::Knob("envpitch_att".into()), Cmd::CopyValue, 0.0, 0.0);
        l.pick(&MenuReq::Knob("envpitch_dec".into()), Cmd::PasteValue, 0.0, 0.0);
        assert_eq!(l.vals.norm("envpitch_dec"), l.vals.norm("envpitch_att"));
        // A time-stretch mode list has every mode.
        let modes = l.menu(&MenuReq::List("ts_mode".into()));
        assert_eq!(modes.cmds.iter().filter(|c| matches!(c, Cmd::Choice(_))).count(), params::STRETCH.len());
        l.typed("root", "C4");
        assert_eq!(l.vals.plain("root") as i32, 48);
        l.typed("ts_pitch", "-1200");
        assert!(l.vals.norm("ts_pitch").abs() < 1e-5);
    }

    #[test]
    fn keyboard_previews_and_sets_the_root() {
        let mut l = logic(1);
        let key = l.hits.iter().find_map(|h| match h.t {
            Target::Key(k) => Some((k, h.r)),
            _ => None,
        });
        let Some((k, r)) = key else { panic!("no keys") };
        let acts = l.down(r.cx(), r.bottom() - 2.0, Btn::Left, Mods::default(), false);
        assert!(acts.iter().any(|a| matches!(a, Act::Note(n, _, true) if i32::from(*n) == k)));
        let acts = l.up(Btn::Left);
        assert!(acts.iter().any(|a| matches!(a, Act::Note(_, _, false))));
        l.down(r.cx(), r.bottom() - 2.0, Btn::Right, Mods::default(), false);
        assert_eq!(l.vals.plain("root") as i32, k);
    }

    /// A press on the waveform plays the sample at its root note until the button is let go (issue #93); a double
    /// click still zooms out.
    #[test]
    fn waveform_press_previews_the_sample() {
        let mut l = logic(0);
        l.typed("root", "C4");
        let (x, y) = centre(&l, &Target::Wave(view::wave_area()));
        let acts = l.down(x, y, Btn::Left, Mods::default(), false);
        assert!(acts.contains(&Act::Note(48, 100, true)), "{acts:?}");
        l.moved(x + 30.0, y, Mods::default());
        assert!(l.up(Btn::Left).contains(&Act::Note(48, 0, false)));
        l.view.wave_len = 0.5;
        let acts = l.down(x, y, Btn::Left, Mods::default(), true);
        assert!(!acts.iter().any(|a| matches!(a, Act::Note(..))));
        assert_eq!(l.view.wave_len, 1.0);
    }

    #[test]
    fn envelope_graph_follows_the_measured_law() {
        let l = logic(1);
        // Defaults: nodes at x 19, 47, 75, 100, 127 (measured, spec §3).
        let eg = view::env_geom(&l.vals, "envvol", view::env_rect());
        let want = [46.5, 74.0, 99.0, 126.5];
        for (got, w) in eg.x[2..].iter().zip(want) {
            assert!((got - w).abs() < 1.2, "{:?}", eg.x);
        }
    }
}
