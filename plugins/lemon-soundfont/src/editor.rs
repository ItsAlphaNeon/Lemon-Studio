// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The plugin editor: a child of the host's window (Win32, X11 on Linux, an NSView on macOS) that paints `view` with
//! the os-ui canvas, and the editor logic (platform independent): opening a font (the open dialog, or a file dropped
//! on the editor), choosing its preset (the patch box, the arrows, the name's menu), the player's controls (vertical
//! drags with the cursor held in place, the wheel, typed values; dragged to the bottom they are off and the font's
//! own value plays), the effect sends and switches, and the playback mode. Every edit changes the saved state.
//!
// spec: plugins/lemon-soundfont.md §4, plugins/host-link.md

use std::any::Any;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::Arc;

use nih_plug::prelude::*;
use os_plugkit::MenuDef;
use os_ui::kit::menu::{MenuItem, MenuStyle};

use crate::params::FontParams;
use crate::profile::Mode;
use crate::state::{Control, State};
use crate::view::{self, Target, ViewState};
use crate::Shared;

pub struct FontEditor {
    pub params: Arc<FontParams>,
    pub shared: Arc<Shared>,
    pub scale: AtomicU32,
    #[cfg_attr(not(windows), allow(dead_code))]
    hwnd: Arc<AtomicIsize>,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    slot: os_plugkit::gui::Slot,
}

impl FontEditor {
    pub fn new(params: Arc<FontParams>, shared: Arc<Shared>) -> Self {
        FontEditor {
            params,
            shared,
            scale: AtomicU32::new(1f32.to_bits()),
            hwnd: Arc::new(AtomicIsize::new(0)),
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            slot: Default::default(),
        }
    }
}

impl Editor for FontEditor {
    fn spawn(&self, parent: ParentWindowHandle, context: Arc<dyn GuiContext>) -> Box<dyn Any + Send> {
        let scale = f32::from_bits(self.scale.load(Ordering::Relaxed));
        let _ = &context;
        match parent {
            #[cfg(windows)]
            ParentWindowHandle::Win32Hwnd(hwnd) => Box::new(win32::open(hwnd, scale, self.params.clone(), self.shared.clone(), self.hwnd.clone())),
            #[cfg(target_os = "linux")]
            ParentWindowHandle::X11Window(window) => Box::new(gui::open(window, scale, self.params.clone(), self.shared.clone(), &self.slot)),
            #[cfg(target_os = "macos")]
            ParentWindowHandle::AppKitNsView(view) => {
                let scale = os_plugkit::gui::backing_scale(view as usize);
                Box::new(gui::open(view as usize, scale, self.params.clone(), self.shared.clone(), &self.slot))
            }
            _ => {
                let _ = scale;
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

    fn param_value_changed(&self, _id: &str, _normalized_value: f32) {}
    fn param_modulation_changed(&self, _id: &str, _modulation_offset: f32) {}
    fn param_values_changed(&self) {}
}

/// Pixels of drag for a control's whole range.
const DRAG_PX: f32 = 200.0;
const FINE: f32 = 0.25;

/// A control's wheel step (as the original's: a notch from off lands one step above off).
fn step(c: Control) -> i32 {
    match c {
        Control::Attack | Control::Decay | Control::Release | Control::LfoDelay => 128,
        Control::LfoAmount => 8,
        Control::ReverbLevel | Control::ChorusLevel => 2,
        Control::Mod | Control::Sustain | Control::LfoSpeed | Control::Cutoff => 4,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    None,
    TypeIn(Control),
    Off(Control),
    Preset(usize),
    Send(usize, i32),
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuReq {
    Value(Control),
    Presets,
    Send(usize),
    About,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Repaint,
    Lock,
    Menu(MenuReq, f32, f32),
    TypeIn(Control, f32, f32),
    OpenDialog,
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

pub struct Logic {
    pub params: Arc<FontParams>,
    pub shared: Arc<Shared>,
    pub view: ViewState,
    pub hits: Vec<view::Hit>,
    /// The control being dragged and where it stands (fractional).
    drag: Option<(Control, f32)>,
    /// Vertical drag on the patch box (fractional preset index).
    patch_drag: Option<f32>,
    pub hint: String,
    pub hosted: bool,
    /// Writes to the state from the editor, and how many of them the host was told of (`take_edit`).
    made: std::cell::Cell<u64>,
    told: u64,
}

impl Logic {
    /// The parameters, for a write to the state from the editor: counted for the host (`take_edit`).
    pub fn raw(&self) -> &Arc<FontParams> {
        self.made.set(self.made.get() + 1);
        &self.params
    }

    /// Whether the host is to be told of edits made since the last call (host link `Edited`): once a drag is let go,
    /// so a drag is one edit.
    pub fn take_edit(&mut self) -> bool {
        if self.made.get() == self.told || self.dragging() {
            return false;
        }
        self.told = self.made.get();
        true
    }

    pub fn new(params: Arc<FontParams>, shared: Arc<Shared>) -> Self {
        Logic { params, shared, view: ViewState::default(), hits: Vec::new(), drag: None, patch_drag: None, hint: String::new(), hosted: false, made: std::cell::Cell::new(0), told: 0 }
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some() || self.patch_drag.is_some()
    }

    fn hit(&self, x: f32, y: f32) -> Option<Target> {
        self.hits.iter().rev().find(|h| h.r.contains(x, y)).map(|h| h.t.clone())
    }

    fn state(&self) -> State {
        self.params.state()
    }

    fn commit(&self, st: &State) {
        self.raw().set_raw(st.to_bytes());
        self.shared.rebuild(&self.params);
    }

    /// Sets a control from one number running from off upward; below its range it is off.
    pub fn set_raw(&mut self, c: Control, v: i32) {
        let (lo, hi) = c.range();
        let mut st = self.state();
        let v = v.clamp(if c.can_be_off() { c.off() } else { lo }, hi);
        let new = if v < lo { None } else { Some(v) };
        if st.control(c) == new {
            return;
        }
        st.set_control(c, new);
        self.commit(&st);
    }

    fn preset_count(&self) -> usize {
        self.shared.current_font().map_or(0, |f| f.presets.len())
    }

    fn choose(&mut self, i: usize) {
        let n = self.preset_count();
        if n > 0 {
            self.shared.choose(self.raw(), i.min(n - 1));
        }
    }

    fn step_preset(&mut self, d: i32) {
        let n = self.preset_count() as i32;
        if n == 0 {
            return;
        }
        let i = self.shared.current_preset().unwrap_or(0) as i32;
        self.choose((i + d).clamp(0, n - 1) as usize);
    }

    /// Moves to the first preset of the next (d > 0) or previous bank.
    fn step_bank(&mut self, d: i32) {
        let Some(font) = self.shared.current_font() else { return };
        let i = self.shared.current_preset().unwrap_or(0);
        let Some(bank) = font.presets.get(i).map(|p| p.bank) else { return };
        let target = if d > 0 {
            font.presets.iter().position(|p| p.bank > bank)
        } else {
            let prev = font.presets.iter().rev().find(|p| p.bank < bank).map(|p| p.bank);
            prev.and_then(|b| font.presets.iter().position(|p| p.bank == b))
        };
        if let Some(t) = target {
            self.choose(t);
        }
    }

    fn mode_hint(m: Mode) -> &'static str {
        match m {
            Mode::Classic => "Classic: the 32-bit player's sound (linear interpolation, its level laws)",
            Mode::Modern => "Modern: the 64-bit player's sound (its sinc interpolation, level laws and quirks)",
        }
    }

    fn hint_for(&self, t: Option<&Target>) -> String {
        let st = self.state();
        let mode = self.params.mode();
        match t {
            Some(Target::Open) => {
                let p = st.path();
                if p.is_empty() { "Open a SoundFont (.sf2)".into() } else { format!("{p}\tclick: open another") }
            }
            Some(Target::Patch) => "Patch\tdrag or wheel: the font's presets in order".into(),
            Some(Target::Bank) => "Bank\twheel: the next bank's first preset".into(),
            Some(Target::Name) => "Preset\tclick: the font's presets".into(),
            Some(Target::Prev) => "Previous preset".into(),
            Some(Target::Next) => "Next preset".into(),
            Some(Target::Value(c @ (Control::ReverbLevel | Control::ChorusLevel))) if mode == Mode::Classic => {
                format!("{}: {}	Classic plays no built-in effects", view::name(*c), view::text(*c, st.control(*c), mode))
            }
            Some(Target::Value(c)) => format!("{}: {}", view::name(*c), view::text(*c, st.control(*c), mode)),
            Some(Target::Send(fx)) => {
                let tr = st.send(*fx);
                let what = if *fx == 0 { "Reverb" } else { "Chorus" };
                if tr < 0 { format!("{what} send: none") } else { format!("{what} send: mixer track {tr}	kept in the project, not played") }
            }
            Some(Target::BuiltIn(fx)) => {
                let s = format!("{} built-in: {}", if *fx == 0 { "Reverb" } else { "Chorus" }, if st.built_in(*fx) { "on" } else { "off" });
                if mode == Mode::Classic { format!("{s}	Classic plays no built-in effects") } else { s }
            }
            Some(Target::Mode(m)) => Self::mode_hint(*m).into(),
            Some(Target::Logo) => "Lemon SoundFont Player".into(),
            None => String::new(),
        }
    }

    pub fn down(&mut self, x: f32, y: f32, btn: Btn, m: Mods, _dbl: bool) -> Vec<Act> {
        self.view.mouse = Some((x, y));
        let Some(t) = self.hit(x, y) else { return vec![] };
        let mut acts = vec![Act::Repaint];
        match (&t, btn) {
            (Target::Open, Btn::Left) => acts.push(Act::OpenDialog),
            (Target::Name, Btn::Left | Btn::Right) if self.preset_count() > 0 => acts.push(Act::Menu(MenuReq::Presets, x, y)),
            (Target::Prev, Btn::Left) => self.step_preset(-1),
            (Target::Next, Btn::Left) => self.step_preset(1),
            (Target::Patch, Btn::Left) => {
                if let Some(i) = self.shared.current_preset() {
                    self.patch_drag = Some(i as f32);
                    self.view.pressed = Some(t.clone());
                    acts.push(Act::Lock);
                }
            }
            (Target::Value(c), Btn::Left) if m.alt && c.can_be_off() => self.set_raw(*c, c.off()),
            (Target::Value(c), Btn::Middle) if c.can_be_off() => self.set_raw(*c, c.off()),
            (Target::Value(c), Btn::Left) => {
                self.drag = Some((*c, view::raw(&self.state(), *c) as f32));
                self.view.pressed = Some(t.clone());
                acts.push(Act::Lock);
            }
            (Target::Value(c), Btn::Right) => acts.push(Act::Menu(MenuReq::Value(*c), x, y)),
            (Target::Send(fx), Btn::Left | Btn::Right) => acts.push(Act::Menu(MenuReq::Send(*fx), x, y)),
            (Target::BuiltIn(fx), Btn::Left) => {
                let mut st = self.state();
                let on = st.built_in(*fx);
                st.set_built_in(*fx, !on);
                self.commit(&st);
            }
            (Target::Mode(md), Btn::Left) => self.shared.set_mode(self.raw(), *md),
            (Target::Logo, Btn::Left | Btn::Right) => acts.push(Act::Menu(MenuReq::About, x, y)),
            _ => {}
        }
        self.hint = self.hint_for(Some(&t));
        acts
    }

    pub fn delta(&mut self, _dx: f32, dy: f32, m: Mods) -> Vec<Act> {
        if let Some(p) = self.patch_drag {
            let n = self.preset_count().max(1) as f32;
            let p = (p - dy / 8.0 * if m.ctrl { FINE } else { 1.0 }).clamp(0.0, n - 1.0);
            self.patch_drag = Some(p);
            if Some(p.round() as usize) != self.shared.current_preset() {
                self.choose(p.round() as usize);
            }
            self.hint = self.hint_for(Some(&Target::Patch));
            return vec![Act::Repaint];
        }
        let Some((c, v)) = self.drag else { return vec![] };
        let (lo, hi) = c.range();
        let lo = if c.can_be_off() { c.off() } else { lo };
        let per_px = ((hi - lo) as f32 / DRAG_PX) * if m.ctrl { FINE } else { 1.0 };
        let v = (v - dy * per_px).clamp(lo as f32, hi as f32);
        self.drag = Some((c, v));
        self.set_raw(c, v.round() as i32);
        let t = self.view.pressed.clone();
        self.hint = self.hint_for(t.as_ref());
        vec![Act::Repaint]
    }

    pub fn up(&mut self, _btn: Btn) -> Vec<Act> {
        self.drag = None;
        self.patch_drag = None;
        self.view.pressed = None;
        vec![Act::Repaint]
    }

    pub fn moved(&mut self, x: f32, y: f32, _m: Mods) -> Vec<Act> {
        self.view.mouse = Some((x, y));
        if !self.dragging() {
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

    pub fn wheel(&mut self, x: f32, y: f32, notches: f32, m: Mods) -> Vec<Act> {
        let Some(t) = self.hit(x, y) else { return vec![] };
        let d = notches.signum() as i32;
        match &t {
            Target::Value(c) => {
                let s = if m.shift { 1 } else { step(*c) };
                let v = view::raw(&self.state(), *c);
                self.set_raw(*c, v + d * s);
            }
            Target::Patch | Target::Name => self.step_preset(-d),
            Target::Bank => self.step_bank(d),
            Target::Send(fx) => {
                let mut st = self.state();
                let tr = (st.send(*fx) + d).clamp(-1, 125);
                st.set_send(*fx, tr);
                self.commit(&st);
            }
            _ => return vec![],
        }
        self.hint = self.hint_for(Some(&t));
        vec![Act::Repaint]
    }

    pub fn menu(&self, req: &MenuReq) -> MenuDef<Cmd> {
        let mut m = MenuDef::new(MenuStyle::Context);
        match req {
            MenuReq::Value(c) => {
                m.head(view::name(*c), Cmd::None);
                if c.can_be_off() {
                    m.add(MenuItem::item("Off (use the font's value)"), Cmd::Off(*c));
                }
                m.add(MenuItem::item("Enter value..."), Cmd::TypeIn(*c));
            }
            MenuReq::Presets => {
                if let Some(font) = self.shared.current_font() {
                    let cur = self.shared.current_preset();
                    m.head(&font.name, Cmd::None);
                    for (i, p) in font.presets.iter().enumerate() {
                        let label = format!("{:03}:{:03}  {}", p.bank, p.program + 1, p.name);
                        m.add(MenuItem::item(&label).mark(os_ui::kit::menu::Mark::Radio(cur == Some(i))), Cmd::Preset(i));
                    }
                }
            }
            MenuReq::Send(fx) => {
                let cur = self.state().send(*fx);
                m.head(if *fx == 0 { "Reverb send" } else { "Chorus send" }, Cmd::None);
                m.add(MenuItem::item("None").mark(os_ui::kit::menu::Mark::Radio(cur < 0)), Cmd::Send(*fx, -1));
                for t in 0..=125 {
                    let label = if t == 0 { "Master".to_string() } else { format!("Bus {t}") };
                    m.add(MenuItem::item(&label).mark(os_ui::kit::menu::Mark::Radio(cur == t)), Cmd::Send(*fx, t));
                }
            }
            MenuReq::About => {
                m.head("Lemon SoundFont Player", Cmd::None);
                m.add(MenuItem::item(&format!("Version {}", env!("CARGO_PKG_VERSION"))).enabled(false), Cmd::None);
            }
        }
        m
    }

    pub fn pick(&mut self, _req: &MenuReq, cmd: Cmd, x: f32, y: f32) -> Vec<Act> {
        match cmd {
            Cmd::None => {}
            Cmd::TypeIn(c) => return vec![Act::TypeIn(c, x, y)],
            Cmd::Off(c) => self.set_raw(c, c.off()),
            Cmd::Preset(i) => self.choose(i),
            Cmd::Send(fx, t) => {
                let mut st = self.state();
                st.set_send(fx, t);
                self.commit(&st);
            }
        }
        vec![Act::Repaint]
    }

    /// The text a type-in box starts with: the control's number, or "off".
    pub fn value_text(&self, c: Control) -> String {
        self.state().control(c).map_or_else(|| "off".into(), |v| v.to_string())
    }

    /// Applies a typed value: a number in the control's own units, or "off".
    pub fn typed(&mut self, c: Control, text: &str) {
        let t = text.trim().to_ascii_lowercase();
        if t.starts_with("off") && c.can_be_off() {
            self.set_raw(c, c.off());
            return;
        }
        let num: String = t.chars().skip_while(|ch| !(ch.is_ascii_digit() || *ch == '-')).take_while(|ch| ch.is_ascii_digit() || *ch == '-' || *ch == '.').collect();
        if let Ok(v) = num.parse::<f64>() {
            let (lo, hi) = c.range();
            self.set_raw(c, (v.round() as i32).clamp(lo, hi));
        }
    }

    /// Opens font file `path`.
    pub fn load(&mut self, path: &str) {
        if let Err(e) = self.shared.load(self.raw(), path) {
            self.hint = e;
        }
    }

    pub fn tick(&mut self) -> bool {
        false
    }

    pub fn paint(&mut self, c: &mut os_ui::Canvas, palette: &os_theme::Palette) {
        let st = self.state();
        let font = self.shared.current_font();
        let preset = self.shared.current_preset().and_then(|i| font.as_ref()?.presets.get(i)).map(|p| (p.bank, p.program, p.name.clone()));
        let error = self.shared.error.lock().map(|e| e.clone()).unwrap_or_default();
        let hint = if self.hosted { String::new() } else { self.hint.clone() };
        let look = view::Look { state: &st, mode: self.params.mode(), font: font.map(|f| f.name.clone()), preset, error: &error };
        self.hits = view::paint(c, palette, &self.view, &look, &hint);
    }
}

#[cfg(windows)]
mod win32 {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicIsize, Ordering};
    use std::sync::Arc;

    use os_plugkit::win::{self as pw, CursorLock};
    use os_theme::Palette;
    use os_ui::Canvas;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OPENFILENAMEW};
    use windows_sys::Win32::UI::Controls::WM_MOUSELEAVE;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
    use windows_sys::Win32::UI::Shell::{DragAcceptFiles, DragFinish, DragQueryFileW, HDROP};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    use super::{Act, Btn, Logic, Mods};
    use crate::params::FontParams;
    use crate::view;
    use crate::Shared;

    const TIMER: usize = 1;

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
        last_raw: u64,
        last_patch: u64,
    }

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

    fn mods() -> Mods {
        let (ctrl, shift, alt) = pw::mods();
        Mods { ctrl, shift, alt }
    }

    pub fn open(parent: *mut c_void, scale: f32, params: Arc<FontParams>, shared: Arc<Shared>, slot: Arc<AtomicIsize>) -> Handle {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let px = |v: u32| (v as f32 * scale).round() as u32;
        let size = (px(view::W), px(view::H));
        shared.editor_open.store(true, Ordering::Relaxed);
        unsafe {
            let class = pw::register("LemonSoundfontView", Some(wndproc));
            let win = Box::new(Win {
                hwnd: std::ptr::null_mut(),
                canvas: Canvas::new(size.0, size.1, scale).expect("canvas"),
                buf: vec![0; (size.0 * size.1) as usize],
                scale,
                size,
                logic: Logic::new(params, shared.clone()),
                palette: os_plugkit::palette(),
                last_hint: String::new(),
                lock: CursorLock::default(),
                tracking: false,
                slot: slot.clone(),
                last_raw: u64::MAX,
                last_patch: u64::MAX,
            });
            let title = pw::wide("Lemon SoundFont Player");
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
                // A font dropped on the editor opens.
                WM_DROPFILES => {
                    let drop = wp as HDROP;
                    let mut buf = vec![0u16; 1024];
                    let n = DragQueryFileW(drop, 0, buf.as_mut_ptr(), buf.len() as u32) as usize;
                    DragFinish(drop);
                    if n > 0 {
                        let path = String::from_utf16_lossy(&buf[..n]);
                        w.logic.load(&path);
                        w.invalidate();
                    }
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

        fn poll(&mut self) {
            let raw = self.logic.params.raw_version.load(Ordering::Acquire);
            let patch = self.logic.shared.patch_version.load(Ordering::Acquire);
            if self.logic.tick() || raw != self.last_raw || patch != self.last_patch {
                self.invalidate();
            }
            if self.logic.hint != self.last_hint {
                self.last_hint = self.logic.hint.clone();
                if self.logic.hosted {
                    os_hostlink::to_host(self.hwnd as isize, os_hostlink::Cmd::Changed, 0);
                }
            }
            if self.logic.take_edit() && self.logic.hosted {
                os_hostlink::to_host(self.hwnd as isize, os_hostlink::Cmd::Edited, 0);
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
                    Act::TypeIn(c, x, y) => {
                        let (sx, sy) = self.screen(x, y);
                        let cur = self.logic.value_text(c);
                        if let Some(t) = pw::type_in(self.hwnd, sx, sy, &cur, self.scale) {
                            self.logic.typed(c, &t);
                        }
                        repaint = true;
                    }
                    Act::OpenDialog => {
                        if let Some(path) = file_dialog(self.hwnd) {
                            self.logic.load(&path);
                        }
                        repaint = true;
                    }
                }
            }
            if repaint {
                self.invalidate();
            }
        }

        /// Answers the host (spec `plugins/host-link.md`): one page, the hint line.
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
                Cmd::Metrics => os_hostlink::pack_size(view::W, view::H),
                Cmd::Hint => {
                    if lp != 0 {
                        // SAFETY: the host passes a `HintBuf` of its own process (spec: host-link.md).
                        unsafe { &mut *(lp as *mut os_hostlink::HintBuf) }.set(&self.logic.hint);
                    }
                    1
                }
                _ => 0,
            }
        }

        fn paint(&mut self) {
            let (hdc, ps) = pw::begin_paint(self.hwnd);
            self.logic.paint(&mut self.canvas, &self.palette);
            self.last_raw = self.logic.params.raw_version.load(Ordering::Acquire);
            self.last_patch = self.logic.shared.patch_version.load(Ordering::Acquire);
            self.buf.resize((self.size.0 * self.size.1) as usize, 0);
            self.canvas.copy_to_xrgb(&mut self.buf);
            pw::blit(hdc, &self.buf, self.size.0, self.size.1);
            pw::end_paint(self.hwnd, &ps);
        }
    }

    fn file_dialog(owner: HWND) -> Option<String> {
        if os_plugkit::no_modal() {
            return None;
        }
        let mut file = vec![0u16; 1024];
        let filter: Vec<u16> = "SoundFonts (*.sf2)\0*.sf2\0All files\0*.*\0\0".encode_utf16().collect();
        let title = pw::wide("Open a SoundFont");
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

    use os_hostlink::{Cmd, PageIcon, Peer};
    use os_plugkit::gui::{self as px, Button, Input, Window};
    use os_theme::Palette;
    use os_ui::Canvas;

    use super::{Act, Btn, Logic, Mods};
    use crate::params::FontParams;
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

    pub struct Handle(Option<px::EditorThread>, Arc<Shared>);

    impl Drop for Handle {
        fn drop(&mut self) {
            self.0.take();
            self.1.editor_open.store(false, Ordering::Relaxed);
        }
    }

    pub fn open(parent: px::Parent, scale: f32, params: Arc<FontParams>, shared: Arc<Shared>, slot: &px::Slot) -> Handle {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let size = ((view::W as f32 * scale).round() as u32, (view::H as f32 * scale).round() as u32);
        let sh = shared.clone();
        let wake = slot.clone();
        let link = Peer::register(parent as u64, vec![PageIcon::Plugin], (view::W, view::H), move || px::notify(&wake));
        let peer = link.clone();
        let thread = px::spawn(parent, size, slot, move |win| {
            let Ok(canvas) = Canvas::new(size.0, size.1, scale) else { return };
            let logic = Logic::new(params, shared);
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
                    Input::Drop { path } => {
                        self.logic.load(&path);
                        self.dirty = true;
                    }
                    Input::Key { .. } => {}
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
            self.logic.tick();
            self.dirty = true;
            if self.logic.hint != self.last_hint {
                self.last_hint.clone_from(&self.logic.hint);
                self.link.set_hint(&self.logic.hint);
                self.link.to_host(Cmd::Changed, 0);
            }
            if self.logic.take_edit() {
                self.link.to_host(Cmd::Edited, 0);
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
                    Act::TypeIn(c, x, y) => {
                        let (sx, sy) = self.screen(x, y);
                        let cur = self.logic.value_text(c);
                        if let Some(t) = self.win.type_in(sx, sy, &cur, self.scale) {
                            self.logic.typed(c, &t);
                        }
                        self.dirty = true;
                    }
                    Act::OpenDialog => {
                        if let Some(path) = file_dialog(&self.win) {
                            self.logic.load(&path);
                        }
                        self.dirty = true;
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

    fn file_dialog(win: &Window) -> Option<String> {
        if os_plugkit::no_modal() {
            return None;
        }
        let path = win.dialog(move || rfd::FileDialog::new().set_title("Open a SoundFont").add_filter("SoundFonts", &["sf2"]).pick_file());
        path.flatten().map(|p| p.to_string_lossy().into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font_file(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("lemon-sf-ed-{}-{tag}.sf2", std::process::id()));
        std::fs::write(&p, crate::sf2::tests::font_bytes(&[8000; 400], 44100, (0, 0), &[], 55)).unwrap();
        p
    }

    fn logic() -> Logic {
        let mut l = Logic::new(Arc::new(FontParams::default()), Arc::new(Shared::default()));
        paint(&mut l);
        l
    }

    fn paint(l: &mut Logic) {
        let mut c = os_ui::Canvas::new(view::W, view::H, 1.0).unwrap();
        l.paint(&mut c, &os_plugkit::palette());
    }

    fn centre(l: &Logic, t: &Target) -> (f32, f32) {
        let h = l.hits.iter().find(|h| &h.t == t).unwrap_or_else(|| panic!("{t:?}"));
        (h.r.cx(), h.r.cy())
    }

    #[test]
    fn controls_drag_wheel_type_and_turn_off() {
        let mut l = logic();
        let a = Target::Value(Control::Attack);
        let (x, y) = centre(&l, &a);
        // A notch from off lands one step above off, as the original's.
        l.wheel(x, y, 1.0, Mods::default());
        assert_eq!(l.state().control(Control::Attack), Some(127));
        l.wheel(x, y, 1.0, Mods::default());
        assert_eq!(l.state().control(Control::Attack), Some(255));
        let acts = l.down(x, y, Btn::Left, Mods::default(), false);
        assert!(acts.contains(&Act::Lock));
        l.delta(0.0, 400.0, Mods::default());
        l.up(Btn::Left);
        assert_eq!(l.state().control(Control::Attack), None);
        l.typed(Control::Sustain, "64");
        assert_eq!(l.state().control(Control::Sustain), Some(64));
        l.typed(Control::Sustain, "off");
        assert_eq!(l.state().control(Control::Sustain), None);
        let (x, y) = centre(&l, &Target::Value(Control::LfoAmount));
        l.wheel(x, y, 1.0, Mods::default());
        assert_eq!(l.state().control(Control::LfoAmount), Some(-120));
        // The mode buttons choose the mode.
        paint(&mut l);
        let (x, y) = centre(&l, &Target::Mode(Mode::Modern));
        let _ = l.take_edit();
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert!(l.take_edit(), "the host hears of it");
        assert_eq!(l.params.mode(), Mode::Modern);
    }

    #[test]
    fn a_font_opens_and_its_presets_choose() {
        let mut l = logic();
        let f = font_file("open");
        l.load(&f.to_string_lossy());
        paint(&mut l);
        assert_eq!(l.shared.current_preset(), Some(0));
        let m = l.menu(&MenuReq::Presets);
        assert!(m.find("000:056  p").is_some());
        assert_eq!(l.state().path(), f.to_string_lossy());
        let _ = std::fs::remove_file(f);
    }

    /// Renders the editor to `OS_SHOT_DIR` (for a look by eye).
    #[test]
    fn render_editor() {
        let Ok(dir) = std::env::var("OS_SHOT_DIR") else { return };
        let mut l = logic();
        let font = std::env::var("OS_SF2").map(std::path::PathBuf::from).unwrap_or_else(|_| {
            let p = std::env::temp_dir().join(format!("lemon-sf-ed-{}-shot.sf2", std::process::id()));
            std::fs::write(&p, crate::sf2::tests::named_font_bytes(&[8000; 400], 44100, (0, 0), &[], 4, "Lemon Demo Kit", "Soft Strings")).unwrap();
            p
        });
        l.load(&font.to_string_lossy());
        let mut st = l.state();
        st.set_control(Control::Attack, Some(600));
        st.set_control(Control::Sustain, Some(90));
        st.set_control(Control::LfoSpeed, Some(64));
        st.set_control(Control::Cutoff, Some(100));
        l.commit(&st);
        paint(&mut l);
        l.view.mouse = Some(centre(&l, &Target::Value(Control::Cutoff)));
        // `OS_SHOT_SCALE` sets the display scale (2 when unset).
        let k: f32 = std::env::var("OS_SHOT_SCALE").ok().and_then(|v| v.parse().ok()).filter(|k: &f32| *k > 0.0).unwrap_or(2.0);
        let mut c = os_ui::Canvas::new((view::W as f32 * k) as u32, (view::H as f32 * k) as u32, k).unwrap();
        l.paint(&mut c, &os_plugkit::palette());
        c.pixmap.save_png(std::path::Path::new(&dir).join("lemon_soundfont.png")).unwrap();
    }
}
