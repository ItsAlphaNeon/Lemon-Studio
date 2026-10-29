// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The plugin editor: a child of the host's window (Win32, X11 on Linux, an NSView on macOS) that paints `view` with
//! the os-ui canvas, and the editor logic (platform independent): knob drags with the cursor held in place, the
//! switches, the loop's tempo and length, loading a loop, the slicing and dump menus, choosing and playing a slice,
//! its key and reverse switch. Parameter edits show and sound at once (`os_plugkit::vals`); the loop's edits change
//! the saved state the voices play.
//!
// spec: plugins/lemon-slicer.md §3–§5, plugins/host-link.md

use std::any::Any;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::Arc;

use nih_plug::prelude::*;
use os_hostlink::NoteOut;
use os_plugkit::vals::Vals;
use os_plugkit::MenuDef;
use os_ui::kit::menu::{Mark, MenuItem, MenuStyle};

use crate::params::{self, SlicerParams, NAMES};
use crate::state::{State, H_BEATS, H_BPM};
use crate::view::{self, Target, ViewState};
use crate::{dump, slicing, Shared};

pub struct SlicerEditor {
    pub params: Arc<SlicerParams>,
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

impl SlicerEditor {
    pub fn new(params: Arc<SlicerParams>, shared: Arc<Shared>) -> Self {
        SlicerEditor {
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

impl Editor for SlicerEditor {
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
// Laws

/// Pixels of drag for a knob's whole range.
pub const KNOB_PX: f32 = 200.0;
const FINE: f32 = 0.25;
/// The velocity a click on the well plays its slice at.
const PREVIEW_VELOCITY: f32 = 100.0 / 127.0;

// ---------------------------------------------------------------------------------------------------------------
// Menus

/// The slicing menu's entries (spec: plugins/fruity-slicer.md §3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slicing {
    BuiltIn,
    /// Dull, Medium, Sharp.
    Auto(usize),
    /// Slices to a beat: 6, 4, 3, 2, 1.
    Beats(u32),
    None,
    ZeroCross,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    None,
    Reset(String),
    CopyValue(String),
    PasteValue(String),
    TypeIn(String),
    Automate(String),
    Slice(Slicing),
    /// Dump: Original length (true) or Normal.
    Dump(bool),
    /// The chosen slice's root key (−1 = its own).
    Root(i32),
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuReq {
    Param(String),
    Slicing,
    Dump,
    Key,
    About,
}

// ---------------------------------------------------------------------------------------------------------------
// Editor logic

/// The type-in ids of the loop's tempo and length (not parameters).
pub const BPM_ID: &str = "bpm";
pub const BEATS_ID: &str = "beats";

/// Something the window has to do after an event.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Repaint,
    /// Hide the cursor and hold it in place (knob drags).
    Lock,
    Menu(MenuReq, f32, f32),
    TypeIn(String, f32, f32),
    /// Ask the host for an automation clip of parameter `index` (host link).
    Automate(usize),
    OpenDialog,
    /// Send these notes to the host for the channel's current pattern (host link `Notes`).
    Notes(Vec<NoteOut>),
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
struct Drag {
    id: String,
    v: f32,
}

pub struct Logic {
    pub vals: Vals,
    pub params: Arc<SlicerParams>,
    pub shared: Arc<Shared>,
    pub view: ViewState,
    pub hits: Vec<view::Hit>,
    drag: Option<Drag>,
    /// Hint text (a tab before the right-aligned part).
    pub hint: String,
    copied: Option<f32>,
    pub hosted: bool,
    /// The key a click on the well is playing.
    playing: Option<u8>,
    /// The waveform's columns and the kit version they were made for.
    peaks: (u64, Vec<(f32, f32)>),
}

impl Logic {
    pub fn new(params: Arc<SlicerParams>, shared: Arc<Shared>, ctx: Option<Arc<dyn GuiContext>>) -> Self {
        let vals = Vals::new(&*params, params.live.clone(), ctx);
        Logic { vals, params, shared, view: ViewState::default(), hits: Vec::new(), drag: None, hint: String::new(), copied: None, hosted: false, playing: None, peaks: (u64::MAX, Vec::new()) }
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    fn hit(&self, x: f32, y: f32) -> Option<Target> {
        self.hits.iter().rev().find(|h| h.r.contains(x, y)).map(|h| h.t.clone())
    }

    fn name(id: &str) -> String {
        NAMES.iter().find(|(i, _)| *i == id).map_or_else(|| id.to_string(), |(_, n)| n.to_string())
    }

    fn state(&self) -> State {
        self.params.state()
    }

    fn frames(&self) -> usize {
        self.shared.sample().map_or(0, |s| s.frames())
    }

    fn chosen(&self, st: &State) -> usize {
        self.view.chosen.min(st.slices.len().saturating_sub(1))
    }

    /// The loop changed: store it and play it.
    fn commit(&mut self, st: &State) {
        self.shared.set_state(&self.params, st);
    }

    fn hint_for(&self, t: Option<&Target>) -> String {
        let st = self.state();
        match t {
            Some(Target::Knob(id)) => format!("{}: {}", Self::name(id), self.vals.text(id)),
            Some(Target::Switch(id)) => format!("{}\t{}", Self::name(id), if self.vals.norm(id) > 0.5 { "on" } else { "off" }),
            Some(Target::Bpm) => format!("Loop tempo: {:.3} BPM", st.bpm()),
            Some(Target::Beats) => format!("Loop length: {:.2} beats", st.beats()),
            Some(Target::Load) => "Load a loop".into(),
            Some(Target::Slicing) => format!("Slicing\t{} slices", st.slices.len()),
            Some(Target::Dump) => {
                if self.hosted { "Dump to piano roll".into() } else { "Dump to piano roll\tin Lemon Studio".into() }
            }
            Some(Target::Well) => {
                let i = self.view.mouse.map_or(0, |(x, _)| slice_at(&st, view::frame_at(x, self.frames())));
                match st.slices.get(i) {
                    Some(s) => format!("Slice {}{}\t{}", i + 1, if s.name.is_empty() { String::new() } else { format!(": {}", s.name()) }, view::key_name(st.key_of(i))),
                    None => "No slices".into(),
                }
            }
            Some(Target::Key) => {
                let i = self.chosen(&st);
                match st.slices.get(i) {
                    Some(s) => format!("Slice key: {}{}", view::key_name(st.key_of(i)), if s.root < 0 { " (default)" } else { "" }),
                    None => String::new(),
                }
            }
            Some(Target::Rev) => {
                let i = self.chosen(&st);
                format!("Reverse slice\t{}", if st.slices.get(i).is_some_and(|s| s.reversed()) { "on" } else { "off" })
            }
            Some(Target::Logo) => "Lemon Slicer".into(),
            None => String::new(),
        }
    }

    fn reset(&mut self, id: &str) {
        let d = self.vals.default_norm(id);
        self.vals.edit(id, d);
    }

    fn play(&mut self, key: Option<u8>) {
        if let Ok(mut q) = self.shared.previews.lock() {
            if let Some(k) = self.playing.take() {
                q.push((k, 0.0, false));
            }
            if let Some(k) = key {
                q.push((k, PREVIEW_VELOCITY, true));
                self.playing = Some(k);
            }
        }
    }

    pub fn down(&mut self, x: f32, y: f32, btn: Btn, m: Mods, _dbl: bool) -> Vec<Act> {
        self.view.mouse = Some((x, y));
        let Some(t) = self.hit(x, y) else { return vec![] };
        let mut acts = vec![Act::Repaint];
        let has = self.frames() > 0;
        match (&t, btn) {
            (Target::Knob(id) | Target::Switch(id), Btn::Left) if m.alt => self.reset(id),
            (Target::Knob(id) | Target::Switch(id), Btn::Middle) => self.reset(id),
            (Target::Knob(id), Btn::Left) => {
                self.vals.begin(id);
                self.drag = Some(Drag { id: id.clone(), v: self.vals.norm(id) });
                self.view.pressed = Some(t.clone());
                acts.push(Act::Lock);
            }
            (Target::Switch(id), Btn::Left) => {
                let on = self.vals.norm(id) > 0.5;
                self.vals.edit(id, if on { 0.0 } else { 1.0 });
            }
            (Target::Knob(id) | Target::Switch(id), Btn::Right) => acts.push(Act::Menu(MenuReq::Param(id.clone()), x, y)),
            (Target::Bpm, Btn::Left | Btn::Right) if has => acts.push(Act::TypeIn(BPM_ID.into(), x, y)),
            (Target::Beats, Btn::Left | Btn::Right) if has => acts.push(Act::TypeIn(BEATS_ID.into(), x, y)),
            (Target::Load, Btn::Left) => acts.push(Act::OpenDialog),
            (Target::Slicing, Btn::Left | Btn::Right) if has => acts.push(Act::Menu(MenuReq::Slicing, view::SLICE.x, view::SLICE.bottom())),
            (Target::Dump, Btn::Left | Btn::Right) => acts.push(Act::Menu(MenuReq::Dump, view::DUMP.x, view::DUMP.bottom())),
            (Target::Well, Btn::Left) if has => {
                let st = self.state();
                let i = slice_at(&st, view::frame_at(x, self.frames()));
                if i < st.slices.len() {
                    self.view.chosen = i;
                    self.view.pressed = Some(Target::Well);
                    let key = u8::try_from(st.key_of(i)).ok().filter(|&k| k < 128);
                    self.play(key);
                }
            }
            (Target::Key, Btn::Left | Btn::Right) if !self.state().slices.is_empty() => acts.push(Act::Menu(MenuReq::Key, view::KEY_BOX.x, view::KEY_BOX.bottom())),
            (Target::Rev, Btn::Left) => {
                let mut st = self.state();
                let i = self.chosen(&st);
                if let Some(s) = st.slices.get_mut(i) {
                    s.flags ^= 1;
                    self.commit(&st);
                }
            }
            (Target::Logo, Btn::Left | Btn::Right) => acts.push(Act::Menu(MenuReq::About, x, y)),
            _ => {}
        }
        self.hint = self.hint_for(Some(&t));
        acts
    }

    /// Mouse movement during a locked drag (logical px since the last event).
    pub fn delta(&mut self, _dx: f32, dy: f32, m: Mods) -> Vec<Act> {
        let Some(Drag { id, v }) = self.drag.clone() else { return vec![] };
        let f = if m.ctrl { FINE } else { 1.0 };
        let v = (v - dy * f / KNOB_PX).clamp(0.0, 1.0);
        self.vals.set(&id, v);
        self.drag = Some(Drag { id, v });
        let t = self.view.pressed.clone();
        self.hint = self.hint_for(t.as_ref());
        vec![Act::Repaint]
    }

    pub fn up(&mut self, _btn: Btn) -> Vec<Act> {
        self.play(None);
        if let Some(d) = self.drag.take() {
            self.vals.end(&d.id);
        }
        self.view.pressed = None;
        vec![Act::Repaint]
    }

    pub fn moved(&mut self, x: f32, y: f32, _m: Mods) -> Vec<Act> {
        self.view.mouse = Some((x, y));
        if self.drag.is_none() {
            let t = self.hit(x, y);
            self.hint = self.hint_for(t.as_ref());
        }
        vec![Act::Repaint]
    }

    pub fn leave(&mut self) -> Vec<Act> {
        self.view.mouse = None;
        self.hint.clear();
        vec![Act::Repaint]
    }

    /// Wheel: a knob moves a step (10 cents / 1 ms; ctrl: 1), a switch flips, the tempo and length move by one.
    pub fn wheel(&mut self, x: f32, y: f32, notches: f32, m: Mods) -> Vec<Act> {
        let Some(t) = self.hit(x, y) else { return vec![] };
        let d = notches.signum();
        match &t {
            Target::Knob(id) => {
                let step = if m.ctrl || id != params::PITCH_ID { 1.0 } else { 10.0 };
                let plain = self.vals.plain(id) + d * step;
                self.vals.edit_plain(id, plain);
            }
            Target::Switch(id) => self.vals.edit(id, if d > 0.0 { 1.0 } else { 0.0 }),
            Target::Bpm if self.frames() > 0 => {
                let st = self.state();
                self.set_tempo(Some(st.bpm() + f64::from(d)), None);
            }
            Target::Beats if self.frames() > 0 => {
                let st = self.state();
                self.set_tempo(None, Some(st.beats() + f64::from(d)));
            }
            _ => return vec![],
        }
        self.hint = self.hint_for(Some(&t));
        vec![Act::Repaint]
    }

    /// Sets the loop's tempo or length; the other follows, so the loop keeps its length in seconds (spec:
    /// plugins/fruity-slicer.md §1.1).
    pub fn set_tempo(&mut self, bpm: Option<f64>, beats: Option<f64>) {
        let mut st = self.state();
        let secs = self.shared.sample().map(|s| s.frames() as f64 / f64::from(s.rate.max(1))).filter(|s| *s > 0.0);
        let Some(secs) = secs else { return };
        let (bpm, beats) = match (bpm, beats) {
            (Some(t), _) => {
                let t = t.clamp(10.0, 522.0);
                (t, t * secs / 60.0)
            }
            (None, Some(b)) => {
                let b = b.clamp(0.25, 512.0);
                (b * 60.0 / secs, b)
            }
            (None, None) => return,
        };
        st.set_f32(H_BPM, bpm as f32);
        st.set_f32(H_BEATS, beats as f32);
        self.commit(&st);
    }

    pub fn menu(&self, req: &MenuReq) -> MenuDef<Cmd> {
        let mut m = MenuDef::new(MenuStyle::Context);
        match req {
            MenuReq::Param(id) => {
                m.head(&Self::name(id), Cmd::None);
                m.add(MenuItem::item("Reset"), Cmd::Reset(id.clone()));
                m.sep(Cmd::None);
                m.add(MenuItem::item("Copy value"), Cmd::CopyValue(id.clone()));
                m.add(MenuItem::item("Paste value").enabled(self.copied.is_some()), Cmd::PasteValue(id.clone()));
                if [params::PITCH_ID, params::ATTACK_ID, params::DECAY_ID].contains(&id.as_str()) {
                    m.add(MenuItem::item("Type in value..."), Cmd::TypeIn(id.clone()));
                }
                m.sep(Cmd::None);
                m.add(MenuItem::item("Create automation clip").enabled(self.hosted), Cmd::Automate(id.clone()));
            }
            MenuReq::Slicing => {
                let has_cues = self.state().path().to_lowercase().ends_with(".wav");
                m.head("Slicing", Cmd::None);
                m.add(MenuItem::item("Use sample built-in slicing").enabled(has_cues), Cmd::Slice(Slicing::BuiltIn));
                m.sep(Cmd::None);
                for (i, name) in ["Dull auto-slicing", "Medium auto-slicing", "Sharp auto-slicing"].into_iter().enumerate() {
                    m.add(MenuItem::item(name), Cmd::Slice(Slicing::Auto(i)));
                }
                m.sep(Cmd::None);
                for (n, name) in [(6, "1/6 beat"), (4, "1/4 beat"), (3, "1/3 beat"), (2, "1/2 beat"), (1, "Beat")] {
                    m.add(MenuItem::item(name), Cmd::Slice(Slicing::Beats(n)));
                }
                m.sep(Cmd::None);
                m.add(MenuItem::item("No slicing"), Cmd::Slice(Slicing::None));
                m.sep(Cmd::None);
                m.add(MenuItem::item("Zero-cross check slices"), Cmd::Slice(Slicing::ZeroCross));
            }
            MenuReq::Dump => {
                let ok = self.hosted && self.frames() > 0 && !self.state().slices.is_empty();
                m.head("Dump to piano roll", Cmd::None);
                m.add(MenuItem::item("Normal").enabled(ok), Cmd::Dump(false));
                m.add(MenuItem::item("Original length").enabled(ok), Cmd::Dump(true));
            }
            MenuReq::Key => {
                let st = self.state();
                let i = self.chosen(&st);
                let root = st.slices.get(i).map_or(-1, |s| s.root);
                m.head("Slice key", Cmd::None);
                m.add(MenuItem::item(&format!("Default ({})", view::key_name(60 + i as i32))).mark(Mark::Radio(root < 0)), Cmd::Root(-1));
                m.sep(Cmd::None);
                for oct in 0..=10 {
                    let mut sub = MenuDef::new(MenuStyle::Context);
                    for k in oct * 12..(oct * 12 + 12).min(128) {
                        sub.add(MenuItem::item(&view::key_name(k)).mark(Mark::Radio(k == root)), Cmd::Root(k));
                    }
                    m.submenu(&format!("Octave {oct}"), sub, Cmd::None);
                }
            }
            MenuReq::About => {
                m.head("Lemon Slicer", Cmd::None);
                m.add(MenuItem::item(&format!("Version {}", env!("CARGO_PKG_VERSION"))).enabled(false), Cmd::None);
                m.add(MenuItem::item("Lemon Studio's recreation of Fruity Slicer").enabled(false), Cmd::None);
            }
        }
        m
    }

    pub fn pick(&mut self, _req: &MenuReq, cmd: Cmd, x: f32, y: f32) -> Vec<Act> {
        match cmd {
            Cmd::None => {}
            Cmd::Reset(id) => self.reset(&id),
            Cmd::CopyValue(id) => self.copied = Some(self.vals.norm(&id)),
            Cmd::PasteValue(id) => {
                if let Some(v) = self.copied {
                    self.vals.edit(&id, v);
                }
            }
            Cmd::TypeIn(id) => return vec![Act::TypeIn(id, x, y)],
            Cmd::Automate(id) => return NAMES.iter().position(|(i, _)| *i == id).map(Act::Automate).into_iter().collect(),
            Cmd::Slice(s) => self.slice(s),
            Cmd::Dump(original) => return self.dump(original).map(Act::Notes).into_iter().collect(),
            Cmd::Root(k) => {
                let mut st = self.state();
                let i = self.chosen(&st);
                if let Some(s) = st.slices.get_mut(i) {
                    s.root = k;
                    self.commit(&st);
                }
            }
        }
        vec![Act::Repaint]
    }

    /// Cuts the loop again (the slicing menu).
    pub fn slice(&mut self, how: Slicing) {
        let Some(s) = self.shared.sample() else { return };
        let mut st = self.state();
        let frames = s.frames();
        st.slices = match how {
            Slicing::BuiltIn => {
                let info = std::fs::read(crate::sample_file(&self.params)).map(|b| slicing::wav_info(&b)).unwrap_or_default();
                let b = slicing::builtin(&info, frames);
                if b.is_empty() {
                    return;
                }
                b
            }
            Slicing::Auto(i) => slicing::auto(&s.left, &s.right, s.rate, slicing::AUTO_DB[i.min(2)]),
            Slicing::Beats(n) => {
                let beats = if st.beats() > 0.0 { st.beats() } else { f64::from(slicing::guess_tempo(frames, s.rate).0) };
                slicing::by_beats(frames, beats, n)
            }
            Slicing::None => {
                let path = st.path().replace('\\', "/");
                let name = path.rsplit('/').next().unwrap_or("").to_string();
                let stem = name.rsplit_once('.').map_or(name.as_str(), |(a, _)| a).to_string();
                slicing::whole(&stem)
            }
            Slicing::ZeroCross => {
                let mut sl = st.slices.clone();
                slicing::zero_cross(&mut sl, &s.left, &s.right, s.rate);
                sl
            }
        };
        self.view.chosen = self.view.chosen.min(st.slices.len().saturating_sub(1));
        self.commit(&st);
    }

    /// The notes of a dump: Normal, or Original length at the host's tempo (the loop's own when the host gave none).
    pub fn dump(&self, original: bool) -> Option<Vec<NoteOut>> {
        let s = self.shared.sample()?;
        let st = self.state();
        let notes = if original {
            let tempo = self.shared.tempo().unwrap_or_else(|| if st.bpm() > 0.0 { st.bpm() } else { 120.0 });
            dump::original(&st, s.frames(), s.rate, tempo)
        } else {
            dump::normal(&st, s.frames())
        };
        (!notes.is_empty()).then_some(notes)
    }

    /// The value text a type-in box starts with.
    pub fn value_text(&self, id: &str) -> String {
        let st = self.state();
        match id {
            BPM_ID => format!("{:.3}", st.bpm()),
            BEATS_ID => format!("{:.2}", st.beats()),
            _ => self.vals.text(id),
        }
    }

    /// Applies a typed value.
    pub fn typed(&mut self, id: &str, text: &str) {
        let num = || text.trim().trim_end_matches(|c: char| c.is_alphabetic() || c.is_whitespace()).parse::<f64>().ok();
        match id {
            BPM_ID => self.set_tempo(num(), None),
            BEATS_ID => self.set_tempo(None, num()),
            _ => {
                if let Some(n) = self.vals.parse(id, text) {
                    self.vals.edit(id, n);
                }
            }
        }
    }

    /// Loads a loop (the open dialog's file).
    pub fn load(&mut self, path: &str) {
        self.view.chosen = 0;
        if let Err(e) = self.shared.load(path, &self.params) {
            self.hint = e;
        }
    }

    /// The waveform's columns for the well (made again when the sample changes).
    fn peaks(&mut self) -> &[(f32, f32)] {
        let v = self.shared.kit_version.load(Ordering::Acquire);
        if self.peaks.0 != v {
            let cols = view::WAVE.w as usize;
            self.peaks = (v, self.shared.sample().map_or_else(Vec::new, |s| peaks(&s.left, &s.right, cols)));
        }
        &self.peaks.1
    }

    /// Paints the editor; `hint` is drawn when the host does not show it.
    pub fn paint(&mut self, c: &mut os_ui::Canvas, palette: &os_theme::Palette) {
        let st = self.state();
        let frames = self.frames();
        let can_dump = self.hosted && frames > 0 && !st.slices.is_empty();
        self.view.chosen = self.chosen(&st);
        let hint = if self.hosted { String::new() } else { self.hint.clone() };
        self.peaks();
        let look = view::Look { state: &st, frames, peaks: &self.peaks.1, can_dump };
        self.hits = view::paint(c, palette, &self.vals, &self.view, &look, &hint);
    }
}

/// The slice the frame is in (the last one starting at or before it).
pub fn slice_at(st: &State, frame: usize) -> usize {
    st.slices.iter().rposition(|s| s.start as usize <= frame).unwrap_or(0)
}

/// The low and high of the summed channels in `cols` columns.
pub fn peaks(left: &[f32], right: &[f32], cols: usize) -> Vec<(f32, f32)> {
    let n = left.len().min(right.len());
    if n == 0 || cols == 0 {
        return Vec::new();
    }
    (0..cols)
        .map(|c| {
            let (a, b) = (c * n / cols, ((c + 1) * n / cols).max(c * n / cols + 1).min(n));
            (a..b).fold((0.0f32, 0.0f32), |(lo, hi), i| {
                let v = (left[i] + right[i]) * 0.5;
                (lo.min(v), hi.max(v))
            })
        })
        .collect()
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
    use windows_sys::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OPENFILENAMEW};
    use windows_sys::Win32::UI::Controls::WM_MOUSELEAVE;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    use super::{Act, Btn, Logic, Mods};
    use crate::params::SlicerParams;
    use crate::view;
    use crate::Shared;

    const TIMER: usize = 1;
    const WM_CHANGED: u32 = WM_APP + 1;

    struct Win {
        hwnd: HWND,
        canvas: Canvas,
        buf: Vec<u32>,
        scale: f32,
        size: (u32, u32),
        logic: Logic,
        palette: Palette,
        last_hint: String,
        lock: CursorLock,
        tracking: bool,
        slot: Arc<AtomicIsize>,
        /// What was painted last (repaint when it changes): the values, the saved state.
        last_sig: u64,
        last_raw: u64,
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

    pub fn open(parent: *mut c_void, scale: f32, params: Arc<SlicerParams>, shared: Arc<Shared>, ctx: Arc<dyn GuiContext>, slot: Arc<AtomicIsize>) -> Handle {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let px = |v: u32| (v as f32 * scale).round() as u32;
        let size = (px(view::W), px(view::H));
        shared.editor_open.store(true, Ordering::Relaxed);
        unsafe {
            let class = pw::register("LemonSlicerView", Some(wndproc));
            let win = Box::new(Win {
                hwnd: std::ptr::null_mut(),
                canvas: Canvas::new(size.0, size.1, scale).expect("canvas"),
                buf: vec![0; (size.0 * size.1) as usize],
                scale,
                size,
                logic: Logic::new(params, shared.clone(), Some(ctx)),
                palette: os_plugkit::palette(),
                last_hint: String::new(),
                lock: CursorLock::default(),
                tracking: false,
                slot: slot.clone(),
                last_sig: 0,
                last_raw: u64::MAX,
            });
            let title = pw::wide("Lemon Slicer");
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
            SetTimer(hwnd, TIMER, 50, None);
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
                    if !acts.contains(&Act::Lock) {
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
                    if w.logic.dragging() && lp as HWND != hwnd {
                        w.lock.unlock();
                        let acts = w.logic.up(Btn::Left);
                        w.run(acts);
                    }
                    0
                }
                WM_MOUSEWHEEL => {
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

        /// Timer / parameter change: expire confirmed overlay values; repaint when something changed.
        fn poll(&mut self) {
            let gone = self.logic.vals.expire();
            let sig = self.logic.vals.signature();
            let raw = self.logic.params.raw_version.load(Ordering::Acquire);
            if gone || sig != self.last_sig || raw != self.last_raw {
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
                    Act::OpenDialog => {
                        if let Some(path) = file_dialog(self.hwnd) {
                            self.logic.load(&path);
                        }
                        repaint = true;
                    }
                    Act::Notes(notes) => {
                        let token = os_hostlink::post_notes(notes);
                        if os_hostlink::to_host(self.hwnd as isize, os_hostlink::Cmd::Notes, token) == 0 {
                            os_hostlink::take_notes(token);
                        }
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
                Cmd::PageIcon => PageIcon::Sample as LRESULT,
                Cmd::GetPage | Cmd::SetPage => 0,
                Cmd::Metrics => os_hostlink::pack_size(view::W, view::H),
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
            self.logic.paint(&mut self.canvas, &self.palette);
            self.last_sig = self.logic.vals.signature();
            self.last_raw = self.logic.params.raw_version.load(Ordering::Acquire);
            self.buf.resize((self.size.0 * self.size.1) as usize, 0);
            self.canvas.copy_to_xrgb(&mut self.buf);
            pw::blit(hdc, &self.buf, self.size.0, self.size.1);
            pw::end_paint(self.hwnd, &ps);
        }
    }

    /// The open sample dialog; `None` when cancelled or in tests that allow no modal loops.
    fn file_dialog(owner: HWND) -> Option<String> {
        if os_plugkit::no_modal() {
            return None;
        }
        let mut file = vec![0u16; 1024];
        let filter: Vec<u16> = "Audio files\0*.wav;*.flac;*.ogg;*.wv;*.aif;*.aiff;*.mp3\0All files\0*.*\0\0".encode_utf16().collect();
        let title = pw::wide("Loop to slice");
        unsafe {
            let mut ofn: OPENFILENAMEW = std::mem::zeroed();
            ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
            ofn.hwndOwner = owner;
            ofn.lpstrFilter = filter.as_ptr();
            ofn.lpstrFile = file.as_mut_ptr();
            ofn.nMaxFile = file.len() as u32;
            ofn.lpstrTitle = title.as_ptr();
            ofn.Flags = OFN_NOCHANGEDIR | OFN_FILEMUSTEXIST;
            if GetOpenFileNameW(&mut ofn) == 0 {
                return None;
            }
        }
        let end = file.iter().position(|&c| c == 0).unwrap_or(file.len());
        Some(String::from_utf16_lossy(&file[..end]))
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
    use crate::params::SlicerParams;
    use crate::view;
    use crate::Shared;

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
        link: Arc<Peer>,
    }

    impl Drop for Ed {
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

    pub fn open(parent: px::Parent, scale: f32, params: Arc<SlicerParams>, shared: Arc<Shared>, ctx: Arc<dyn GuiContext>, slot: &px::Slot) -> Handle {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let size = ((view::W as f32 * scale).round() as u32, (view::H as f32 * scale).round() as u32);
        let sh = shared.clone();
        let wake = slot.clone();
        let link = Peer::register(parent as u64, vec![PageIcon::Sample], (view::W, view::H), move || px::notify(&wake));
        let peer = link.clone();
        let thread = px::spawn(parent, size, slot, move |win| {
            let Ok(canvas) = Canvas::new(size.0, size.1, scale) else { return };
            let logic = Logic::new(params, shared, Some(ctx));
            let ed = Ed { win, canvas, buf: Vec::new(), scale, size, logic, palette: os_plugkit::palette(), last_hint: String::new(), dirty: true, link: peer };
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

        fn poll(&mut self) {
            self.logic.vals.expire();
            self.dirty = true;
            if self.logic.hint != self.last_hint {
                self.last_hint.clone_from(&self.logic.hint);
                self.link.set_hint(&self.logic.hint);
                self.link.to_host(Cmd::Changed, 0);
            }
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
                    Act::Automate(i) => {
                        self.link.to_host(Cmd::Automate, i as isize);
                    }
                    Act::OpenDialog => {
                        if let Some(path) = file_dialog(&self.win) {
                            self.logic.load(&path);
                        }
                        self.dirty = true;
                    }
                    Act::Notes(notes) => {
                        let token = os_hostlink::post_notes(notes);
                        if self.link.to_host(Cmd::Notes, token) == 0 {
                            os_hostlink::take_notes(token);
                        }
                    }
                }
            }
        }

        fn paint(&mut self) {
            self.dirty = false;
            self.logic.paint(&mut self.canvas, &self.palette);
            self.buf.resize((self.size.0 * self.size.1) as usize, 0);
            self.canvas.copy_to_xrgb(&mut self.buf);
            self.win.blit(&self.buf, self.size.0, self.size.1);
        }
    }

    /// The open sample dialog: the desktop's own (through its portal on Linux; on macOS AppKit's).
    fn file_dialog(win: &Window) -> Option<String> {
        if os_plugkit::no_modal() {
            return None;
        }
        let path = win.dialog(move || {
            rfd::FileDialog::new()
                .set_title("Loop to slice")
                .add_filter("Audio files", &["wav", "flac", "ogg", "wv", "aif", "aiff", "mp3"])
                .pick_file()
        });
        path.flatten().map(|p| p.to_string_lossy().into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::loop_file;

    fn logic() -> Logic {
        let mut l = Logic::new(Arc::new(SlicerParams::default()), Arc::new(Shared::default()), None);
        paint(&mut l);
        l
    }

    fn paint(l: &mut Logic) {
        let mut c = os_ui::Canvas::new(view::W, view::H, 1.0).unwrap();
        l.paint(&mut c, &os_plugkit::palette());
    }

    fn centre(l: &Logic, t: &Target) -> (f32, f32) {
        let h = l.hits.iter().rev().find(|h| &h.t == t).unwrap_or_else(|| panic!("no hit {t:?}"));
        (h.r.cx(), h.r.cy())
    }

    /// A loop loaded into the editor, from a file of its own (`name`), as the tests run side by side.
    fn loaded(name: &str) -> (Logic, std::path::PathBuf) {
        let p = loop_file(name, &[]);
        let mut l = logic();
        l.load(&p.to_string_lossy());
        paint(&mut l);
        (l, p)
    }

    #[test]
    fn every_parameter_has_a_control() {
        let l = logic();
        for (id, _) in NAMES {
            assert!(l.hits.iter().any(|h| matches!(&h.t, Target::Knob(i) | Target::Switch(i) if i == id)), "{id}");
        }
        for t in [Target::Load, Target::Slicing, Target::Dump, Target::Well, Target::Key, Target::Rev, Target::Bpm, Target::Beats, Target::Logo] {
            centre(&l, &t);
        }
    }

    #[test]
    fn knobs_switches_and_hints() {
        let mut l = logic();
        let (x, y) = centre(&l, &Target::Knob(params::DECAY_ID.into()));
        assert!(l.down(x, y, Btn::Left, Mods::default(), false).contains(&Act::Lock));
        // 200 px = 0..1000 ms: 4 px up = +20.
        for _ in 0..4 {
            l.delta(0.0, -1.0, Mods::default());
        }
        l.up(Btn::Left);
        assert_eq!(l.params.values().decay, 24);
        assert_eq!(l.hint, "Decay: 24ms");
        l.wheel(x, y, 1.0, Mods::default());
        assert_eq!(l.params.values().decay, 25);
        let (x, y) = centre(&l, &Target::Knob(params::PITCH_ID.into()));
        l.wheel(x, y, 1.0, Mods::default());
        assert_eq!(l.params.values().pitch, 10);
        assert_eq!(l.hint, "Pitch shift: +10 cents");
        let (x, y) = centre(&l, &Target::Switch(params::TO_END_ID.into()));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert!(l.params.values().to_end);
        assert_eq!(l.hint, "Play to end\ton");
        assert_eq!(l.pick(&MenuReq::Param("toend".into()), Cmd::Automate("toend".into()), 0.0, 0.0), vec![Act::Automate(4)]);
    }

    #[test]
    fn slicing_choosing_and_the_slices_settings() {
        let (mut l, p) = loaded("slicing");
        assert_eq!(l.state().slices.len(), 4);
        l.pick(&MenuReq::Slicing, Cmd::Slice(Slicing::Beats(2)), 0.0, 0.0);
        let st = l.state();
        assert_eq!(st.slices.len(), 8);
        assert_eq!(st.slices[2].name(), "3 - Beat 2");
        l.pick(&MenuReq::Slicing, Cmd::Slice(Slicing::None), 0.0, 0.0);
        assert_eq!(l.state().slices[0].name(), p.file_stem().unwrap().to_string_lossy());
        l.pick(&MenuReq::Slicing, Cmd::Slice(Slicing::Auto(1)), 0.0, 0.0);
        paint(&mut l);
        // A click on the well's third quarter chooses slice 3 and plays its key; letting go ends the note.
        let x = view::WAVE.x + view::WAVE.w * 0.6;
        l.down(x, view::WAVE.cy(), Btn::Left, Mods::default(), false);
        assert_eq!(l.view.chosen, 2);
        assert_eq!(*l.shared.previews.lock().unwrap(), vec![(62, PREVIEW_VELOCITY, true)]);
        l.up(Btn::Left);
        assert_eq!(l.shared.previews.lock().unwrap().last(), Some(&(62, 0.0, false)));
        assert_eq!(l.hint, "Slice 3\tD5");
        // Its key and its reverse switch.
        l.pick(&MenuReq::Key, Cmd::Root(36), 0.0, 0.0);
        let (x, y) = centre(&l, &Target::Rev);
        l.down(x, y, Btn::Left, Mods::default(), false);
        let st = l.state();
        assert_eq!((st.slices[2].root, st.slices[2].reversed()), (36, true));
        assert_eq!(l.shared.kit.lock().unwrap().as_ref().unwrap().keys[36], Some(2));
        assert!(l.menu(&MenuReq::Key).find("C3").is_some());
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn tempo_and_length_move_together() {
        let (mut l, p) = loaded("tempo");
        // 2 s at 4 beats, 120 BPM; 9 beats make 270 BPM; 60 BPM make 2 beats.
        l.typed(BEATS_ID, "9");
        assert_eq!((l.state().beats(), l.state().bpm()), (9.0, 270.0));
        l.typed(BPM_ID, "60 bpm");
        assert_eq!((l.state().beats(), l.state().bpm()), (2.0, 60.0));
        assert_eq!(l.value_text(BPM_ID), "60.000");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn the_dump_is_offered_when_hosted() {
        let (mut l, p) = loaded("dump");
        assert!(!l.menu(&MenuReq::Dump).items.iter().any(|i| matches!(i, MenuItem::Item { enabled: true, .. })));
        l.hosted = true;
        assert_eq!(l.menu(&MenuReq::Dump).find("Normal"), Some(Cmd::Dump(false)));
        let acts = l.pick(&MenuReq::Dump, Cmd::Dump(false), 0.0, 0.0);
        let Some(Act::Notes(n)) = acts.first() else { panic!("{acts:?}") };
        let got: Vec<(f64, f64, u8)> = n.iter().map(|n| (n.start, n.end, n.key)).collect();
        assert_eq!(got, [(0.0, 1.0, 60), (1.0, 2.0, 61), (2.0, 3.0, 62), (3.0, 4.0, 63)]);
        // Original length at the host's 60 BPM: the 2 s loop takes 2 beats.
        l.shared.tempo.store(60f64.to_bits(), Ordering::Relaxed);
        let Some(Act::Notes(n)) = l.pick(&MenuReq::Dump, Cmd::Dump(true), 0.0, 0.0).first().cloned() else { panic!() };
        assert_eq!(n.last().map(|n| n.end), Some(2.0));
        let _ = std::fs::remove_file(p);
    }

    /// `SLICER_PNG=<dir> cargo test -p lemon-slicer -- --ignored`: the editor, empty and with a loop, as PNGs.
    #[test]
    #[ignore]
    fn render_editor() {
        let Some(dir) = std::env::var_os("SLICER_PNG") else { return };
        let save = |l: &mut Logic, name: &str, scale: f32| {
            let (w, h) = ((view::W as f32 * scale) as u32, (view::H as f32 * scale) as u32);
            let mut c = os_ui::Canvas::new(w, h, scale).unwrap();
            l.paint(&mut c, &os_plugkit::palette());
            let mut px = vec![0u32; (w * h) as usize];
            c.copy_to_xrgb(&mut px);
            let mut pm = tiny_skia::Pixmap::new(w, h).unwrap();
            for (o, p) in pm.data_mut().chunks_mut(4).zip(&px) {
                o.copy_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255]);
            }
            pm.save_png(std::path::Path::new(&dir).join(format!("{name}.png"))).unwrap();
        };
        let mut l = logic();
        save(&mut l, "slicer_empty", 1.0);
        let path = std::env::var("SLICER_LOOP").ok();
        let p = loop_file("render", &[]);
        l.load(path.as_deref().unwrap_or(&p.to_string_lossy()));
        l.view.chosen = 1;
        l.hint = "Slice 2\tC#5".into();
        save(&mut l, "slicer_loop", 1.0);
        save(&mut l, "slicer_loop_x2", 2.0);
        let _ = std::fs::remove_file(p);
    }
}
