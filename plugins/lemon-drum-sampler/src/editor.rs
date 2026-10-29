// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The plugin editor: a child of the host's window (Win32, X11 on Linux, an NSView on macOS) that paints `view` with
//! the os-ui canvas, and the editor logic (platform independent): choosing and playing pads, the banks, the chosen
//! pad's and layer's values (vertical drags with the cursor held in place, the wheel, typed values), loading a
//! sample onto a pad (the open dialog, or a file dropped on the pad). Every edit changes the saved state the voices
//! play.
//!
// spec: plugins/lemon-drum-sampler.md §4, plugins/host-link.md

use std::any::Any;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::Arc;

use nih_plug::prelude::*;
use os_plugkit::MenuDef;
use os_ui::kit::menu::{MenuItem, MenuStyle};

use crate::params::DrumParams;
use crate::state::{self, State};
use crate::view::{self, Field, Target, ViewState};
use crate::Shared;

pub struct DrumEditor {
    pub params: Arc<DrumParams>,
    pub shared: Arc<Shared>,
    /// DPI scale as f32 bits.
    pub scale: AtomicU32,
    /// The open editor window (0 = closed).
    #[cfg_attr(not(windows), allow(dead_code))]
    hwnd: Arc<AtomicIsize>,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    slot: os_plugkit::gui::Slot,
}

impl DrumEditor {
    pub fn new(params: Arc<DrumParams>, shared: Arc<Shared>) -> Self {
        DrumEditor {
            params,
            shared,
            scale: AtomicU32::new(1f32.to_bits()),
            hwnd: Arc::new(AtomicIsize::new(0)),
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            slot: Default::default(),
        }
    }
}

impl Editor for DrumEditor {
    fn spawn(&self, parent: ParentWindowHandle, context: Arc<dyn GuiContext>) -> Box<dyn Any + Send> {
        let scale = f32::from_bits(self.scale.load(Ordering::Relaxed));
        let _ = &context;
        match parent {
            #[cfg(windows)]
            ParentWindowHandle::Win32Hwnd(hwnd) => Box::new(win32::open(hwnd, scale, self.params.clone(), self.shared.clone(), self.hwnd.clone())),
            #[cfg(target_os = "linux")]
            ParentWindowHandle::X11Window(window) => Box::new(gui::open(window, scale, self.params.clone(), self.shared.clone(), &self.slot)),
            // macOS hosts set no scale (sizes are points): the editor paints at its window's backing scale.
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

// ---------------------------------------------------------------------------------------------------------------
// Laws

/// Pixels of drag for a value's whole range (at least 2 px a step).
const DRAG_PX: f32 = 200.0;
const FINE: f32 = 0.25;
/// How long a pad stays lit after a click (editor ticks of 50 ms).
const LIT_TICKS: u8 = 4;

#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    None,
    TypeIn(Field),
    Reset(Field),
    /// Clears the chosen pad's sample.
    Clear,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuReq {
    Value(Field),
    Pad,
    About,
}

/// Something the window has to do after an event.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Repaint,
    /// Hide the cursor and hold it in place (value drags).
    Lock,
    Menu(MenuReq, f32, f32),
    TypeIn(Field, f32, f32),
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
    pub params: Arc<DrumParams>,
    pub shared: Arc<Shared>,
    pub view: ViewState,
    pub hits: Vec<view::Hit>,
    /// The value being dragged and where it stands (in steps, fractional).
    drag: Option<(Field, f32)>,
    /// Hint text (a tab before the right-aligned part).
    pub hint: String,
    pub hosted: bool,
    lit: Vec<u8>,
}

impl Logic {
    pub fn new(params: Arc<DrumParams>, shared: Arc<Shared>) -> Self {
        Logic { params, shared, view: ViewState::default(), hits: Vec::new(), drag: None, hint: String::new(), hosted: false, lit: vec![0; state::PADS] }
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    fn hit(&self, x: f32, y: f32) -> Option<Target> {
        self.hits.iter().rev().find(|h| h.r.contains(x, y)).map(|h| h.t.clone())
    }

    fn state(&self) -> State {
        self.params.state()
    }

    /// The chosen pad's value of a field.
    pub fn value(&self, f: Field) -> Option<i32> {
        self.state().pad(self.view.pad).and_then(|p| view::value(&p, self.view.layer, f))
    }

    /// Sets a field of the chosen pad (or its chosen layer), clamped to its range; the voices play it at once.
    pub fn set(&mut self, f: Field, v: i32) {
        let (lo, hi) = f.range();
        let v = v.clamp(lo, hi);
        if self.value(f) == Some(v) {
            return;
        }
        let mut st = self.state();
        let (p, l) = (self.view.pad, self.view.layer);
        match f {
            Field::Key => st.set_pad(p, state::PAD_KEY, v),
            Field::Cut => st.set_pad(p, state::PAD_CUT, v),
            Field::CutBy => st.set_pad(p, state::PAD_CUT_BY, v),
            Field::PadVolume => st.set_pad(p, state::PAD_VOLUME, v),
            Field::PadPan => st.set_pad(p, state::PAD_PAN, v),
            Field::PadTune => st.set_pad(p, state::PAD_TUNE, v),
            Field::VelLo => st.set_layer(p, l, state::L_VEL_LO, v),
            Field::VelHi => st.set_layer(p, l, state::L_VEL_HI, v),
            Field::LayerVolume => st.set_layer(p, l, state::L_VOLUME, v),
            Field::LayerPan => st.set_layer(p, l, state::L_PAN, v),
            Field::LayerTune => st.set_layer(p, l, state::L_TUNE, v),
        }
        self.shared.set_state(&self.params, &st);
    }

    /// A field's default (a new pad's).
    fn default_of(f: Field) -> i32 {
        match f {
            Field::Key => 60,
            Field::Cut | Field::CutBy => -1,
            Field::PadVolume => state::NEW_PAD_VOLUME,
            Field::LayerVolume => state::NEW_LAYER_VOLUME,
            Field::VelHi => 127,
            _ => 0,
        }
    }

    fn hint_for(&self, t: Option<&Target>) -> String {
        let st = self.state();
        match t {
            Some(Target::Pad(n)) => match st.pad(*n) {
                Some(p) => {
                    let name = if p.name.is_empty() { String::new() } else { format!(": {}", p.name) };
                    let layers = p.sounding().count();
                    format!("Pad {}{}{name}\t{}, {} layer{}", if *n >= 16 { "B" } else { "A" }, n % 16 + 1, view::key_name(p.key), layers, if layers == 1 { "" } else { "s" })
                }
                None => String::new(),
            },
            Some(Target::Bank(b)) => format!("Bank {}", if *b == 0 { "A" } else { "B" }),
            Some(Target::Value(f)) => match self.value(*f) {
                Some(v) => format!("{}: {}", f.name(), f.text(v)),
                None => f.name().into(),
            },
            Some(Target::Prev) => "Previous layer".into(),
            Some(Target::Next) => "Next layer".into(),
            Some(Target::Load) => "Load a sample onto the pad".into(),
            Some(Target::Logo) => "Lemon Drum Sampler".into(),
            None => String::new(),
        }
    }

    /// Plays pad `n` at the velocity of the click's height (top soft, bottom hard) and lights it.
    fn play(&mut self, n: usize, y: f32) {
        let Some(p) = self.state().pad(n) else { return };
        let r = view::pad_rect(n);
        let vel = (((y - r.y) / r.h).clamp(0.0, 1.0) * 126.0 + 1.0).round() / 127.0;
        if let (Ok(k), Ok(mut q)) = (u8::try_from(p.key), self.shared.previews.lock())
            && k < 128
        {
            q.push((k, vel));
        }
        if let Some(l) = self.lit.get_mut(n) {
            *l = LIT_TICKS;
        }
    }

    pub fn down(&mut self, x: f32, y: f32, btn: Btn, m: Mods, _dbl: bool) -> Vec<Act> {
        self.view.mouse = Some((x, y));
        let Some(t) = self.hit(x, y) else { return vec![] };
        let mut acts = vec![Act::Repaint];
        match (&t, btn) {
            (Target::Pad(n), Btn::Left) => {
                if self.view.pad != *n {
                    self.view.layer = 0;
                }
                self.view.pad = *n;
                self.view.pressed = Some(t.clone());
                self.play(*n, y);
            }
            (Target::Pad(n), Btn::Right) => {
                self.view.pad = *n;
                self.view.layer = 0;
                acts.push(Act::Menu(MenuReq::Pad, x, y));
            }
            (Target::Bank(b), Btn::Left) => {
                self.view.bank = *b;
                self.view.pad = b * 16 + self.view.pad % 16;
                self.view.layer = 0;
            }
            (Target::Value(f), Btn::Left) if m.alt => self.set(*f, Self::default_of(*f)),
            (Target::Value(f), Btn::Middle) => self.set(*f, Self::default_of(*f)),
            (Target::Value(f), Btn::Left) => {
                if let Some(v) = self.value(*f) {
                    self.drag = Some((*f, v as f32));
                    self.view.pressed = Some(t.clone());
                    acts.push(Act::Lock);
                }
            }
            (Target::Value(f), Btn::Right) => acts.push(Act::Menu(MenuReq::Value(*f), x, y)),
            (Target::Prev, Btn::Left) => self.view.layer = self.view.layer.saturating_sub(1),
            (Target::Next, Btn::Left) => self.view.layer += 1,
            (Target::Load, Btn::Left) => acts.push(Act::OpenDialog),
            (Target::Logo, Btn::Left | Btn::Right) => acts.push(Act::Menu(MenuReq::About, x, y)),
            _ => {}
        }
        self.clamp_layer();
        self.hint = self.hint_for(Some(&t));
        acts
    }

    fn clamp_layer(&mut self) {
        let n = self.state().pad(self.view.pad).map_or(0, |p| p.layers.len());
        self.view.layer = self.view.layer.min(n.saturating_sub(1));
    }

    /// Mouse movement during a locked drag (logical px since the last event).
    pub fn delta(&mut self, _dx: f32, dy: f32, m: Mods) -> Vec<Act> {
        let Some((f, v)) = self.drag else { return vec![] };
        let (lo, hi) = f.range();
        let per_px = ((hi - lo) as f32 / DRAG_PX).min(0.5) * if m.ctrl { FINE } else { 1.0 };
        let v = (v - dy * per_px).clamp(lo as f32, hi as f32);
        self.drag = Some((f, v));
        self.set(f, v.round() as i32);
        let t = self.view.pressed.clone();
        self.hint = self.hint_for(t.as_ref());
        vec![Act::Repaint]
    }

    pub fn up(&mut self, _btn: Btn) -> Vec<Act> {
        self.drag = None;
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

    /// Wheel: a value moves a step (5 with shift), the bank buttons and pads choose.
    pub fn wheel(&mut self, x: f32, y: f32, notches: f32, m: Mods) -> Vec<Act> {
        let Some(t) = self.hit(x, y) else { return vec![] };
        let d = notches.signum() as i32 * if m.shift { 5 } else { 1 };
        match &t {
            Target::Value(f) => {
                if let Some(v) = self.value(*f) {
                    self.set(*f, v + d);
                }
            }
            _ => return vec![],
        }
        self.hint = self.hint_for(Some(&t));
        vec![Act::Repaint]
    }

    pub fn menu(&self, req: &MenuReq) -> MenuDef<Cmd> {
        let mut m = MenuDef::new(MenuStyle::Context);
        match req {
            MenuReq::Value(f) => {
                m.head(f.name(), Cmd::None);
                m.add(MenuItem::item("Reset"), Cmd::Reset(*f));
                m.add(MenuItem::item("Type in value..."), Cmd::TypeIn(*f));
            }
            MenuReq::Pad => {
                let n = self.view.pad;
                m.head(&format!("Pad {}{}", if n >= 16 { "B" } else { "A" }, n % 16 + 1), Cmd::None);
                let has = self.state().pad(n).is_some_and(|p| p.sounding().next().is_some());
                m.add(MenuItem::item("Clear pad").enabled(has), Cmd::Clear);
            }
            MenuReq::About => {
                m.head("Lemon Drum Sampler", Cmd::None);
                m.add(MenuItem::item(&format!("Version {}", env!("CARGO_PKG_VERSION"))).enabled(false), Cmd::None);
                m.add(MenuItem::item("Lemon Studio's recreation of FPC").enabled(false), Cmd::None);
            }
        }
        m
    }

    pub fn pick(&mut self, _req: &MenuReq, cmd: Cmd, x: f32, y: f32) -> Vec<Act> {
        match cmd {
            Cmd::None => {}
            Cmd::TypeIn(f) => return vec![Act::TypeIn(f, x, y)],
            Cmd::Reset(f) => self.set(f, Self::default_of(f)),
            Cmd::Clear => {
                let mut st = self.state();
                st.set_sample(self.view.pad, "");
                st.set_name(self.view.pad, "");
                self.shared.set_state(&self.params, &st);
            }
        }
        vec![Act::Repaint]
    }

    /// The value text a type-in box starts with.
    pub fn value_text(&self, f: Field) -> String {
        self.value(f).map_or_else(String::new, |v| if f == Field::Key { view::key_name(v) } else { v.to_string() })
    }

    /// Applies a typed value: a number, or a key name for the key ("C3", "F#5").
    pub fn typed(&mut self, f: Field, text: &str) {
        let t = text.trim();
        let v = if f == Field::Key { parse_key(t).or_else(|| t.parse().ok()) } else { t.trim_end_matches(|c: char| !c.is_ascii_digit()).parse().ok() };
        if let Some(v) = v {
            self.set(f, v);
        }
    }

    /// Loads sample file `path` onto pad `n` (the chosen one when `None`), which becomes the chosen pad.
    pub fn load(&mut self, n: Option<usize>, path: &str) {
        let n = n.unwrap_or(self.view.pad);
        self.view.pad = n;
        self.view.layer = 0;
        if let Err(e) = self.shared.load(&self.params, n, path) {
            self.hint = e;
        }
    }

    /// The pad under a point, if any.
    pub fn pad_at(&self, x: f32, y: f32) -> Option<usize> {
        match self.hit(x, y) {
            Some(Target::Pad(n)) => Some(n),
            _ => None,
        }
    }

    /// Timer: the lit pads fade; true when something changed.
    pub fn tick(&mut self) -> bool {
        let mut any = false;
        for l in self.lit.iter_mut().filter(|l| **l > 0) {
            *l -= 1;
            any = true;
        }
        any
    }

    /// Paints the editor; `hint` is drawn when the host does not show it.
    pub fn paint(&mut self, c: &mut os_ui::Canvas, palette: &os_theme::Palette) {
        let st = self.state();
        self.view.bank = self.view.bank.min(1);
        self.clamp_layer();
        let loaded: Vec<bool> = st.pads().iter().enumerate().map(|(i, p)| p.sounding().any(|(j, _)| self.shared.loaded(i, j))).collect();
        let lit: Vec<bool> = self.lit.iter().map(|&l| l > 0).collect();
        let hint = if self.hosted { String::new() } else { self.hint.clone() };
        let look = view::Look { state: &st, loaded: &loaded, lit: &lit };
        self.hits = view::paint(c, palette, &self.view, &look, &hint);
    }
}

/// A key name ("C5" = 60, "F#3", "Db4") as a key number.
pub fn parse_key(s: &str) -> Option<i32> {
    let s = s.trim();
    let mut ch = s.chars();
    let base = match ch.next()?.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let rest: String = ch.collect();
    let (acc, oct) = match rest.chars().next() {
        Some('#') => (1, &rest[1..]),
        Some('b') => (-1, &rest[1..]),
        _ => (0, rest.as_str()),
    };
    let o: i32 = oct.parse().ok()?;
    Some((o * 12 + base + acc).clamp(0, 127))
}

// ---------------------------------------------------------------------------------------------------------------
// The window

#[cfg(windows)]
mod win32 {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicIsize, Ordering};
    use std::sync::Arc;

    use os_plugkit::win::{self as pw, CursorLock};
    use os_theme::Palette;
    use os_ui::Canvas;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OPENFILENAMEW};
    use windows_sys::Win32::UI::Controls::WM_MOUSELEAVE;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
    use windows_sys::Win32::UI::Shell::{DragAcceptFiles, DragFinish, DragQueryFileW, DragQueryPoint, HDROP};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    use super::{Act, Btn, Logic, Mods};
    use crate::params::DrumParams;
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
        last_kit: u64,
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

    fn mods() -> Mods {
        let (ctrl, shift, alt) = pw::mods();
        Mods { ctrl, shift, alt }
    }

    pub fn open(parent: *mut c_void, scale: f32, params: Arc<DrumParams>, shared: Arc<Shared>, slot: Arc<AtomicIsize>) -> Handle {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let px = |v: u32| (v as f32 * scale).round() as u32;
        let size = (px(view::W), px(view::H));
        shared.editor_open.store(true, Ordering::Relaxed);
        unsafe {
            let class = pw::register("LemonDrumSamplerView", Some(wndproc));
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
                last_kit: u64::MAX,
            });
            let title = pw::wide("Lemon Drum Sampler");
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
                // A file dropped on a pad loads onto that pad (elsewhere: onto the chosen pad).
                WM_DROPFILES => {
                    let drop = wp as HDROP;
                    let mut buf = vec![0u16; 1024];
                    let n = DragQueryFileW(drop, 0, buf.as_mut_ptr(), buf.len() as u32) as usize;
                    let mut pt = POINT { x: 0, y: 0 };
                    DragQueryPoint(drop, &mut pt);
                    DragFinish(drop);
                    if n > 0 {
                        let path = String::from_utf16_lossy(&buf[..n]);
                        let pad = w.logic.pad_at(pt.x as f32 / w.scale, pt.y as f32 / w.scale);
                        w.logic.load(pad, &path);
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

        /// Timer: repaint when the state, the kit or a lit pad changed; the hint goes to the host.
        fn poll(&mut self) {
            let lit = self.logic.tick();
            let raw = self.logic.params.raw_version.load(Ordering::Acquire);
            let kit = self.logic.shared.kit_version.load(Ordering::Acquire);
            if lit || raw != self.last_raw || kit != self.last_kit {
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
                    Act::TypeIn(f, x, y) => {
                        let (sx, sy) = self.screen(x, y);
                        let cur = self.logic.value_text(f);
                        if let Some(t) = pw::type_in(self.hwnd, sx, sy, &cur, self.scale) {
                            self.logic.typed(f, &t);
                        }
                        repaint = true;
                    }
                    Act::OpenDialog => {
                        if let Some(path) = file_dialog(self.hwnd) {
                            self.logic.load(None, &path);
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
                Cmd::PageIcon => PageIcon::Sample as LRESULT,
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
            self.last_kit = self.logic.shared.kit_version.load(Ordering::Acquire);
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
        let title = pw::wide("Sample for the pad");
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
    use crate::params::DrumParams;
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

    pub fn open(parent: px::Parent, scale: f32, params: Arc<DrumParams>, shared: Arc<Shared>, slot: &px::Slot) -> Handle {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let size = ((view::W as f32 * scale).round() as u32, (view::H as f32 * scale).round() as u32);
        let sh = shared.clone();
        let wake = slot.clone();
        let link = Peer::register(parent as u64, vec![PageIcon::Sample], (view::W, view::H), move || px::notify(&wake));
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
                    // A dropped file loads onto the chosen pad.
                    Input::Drop { path } => {
                        self.logic.load(None, &path);
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
                    Act::TypeIn(f, x, y) => {
                        let (sx, sy) = self.screen(x, y);
                        let cur = self.logic.value_text(f);
                        if let Some(t) = self.win.type_in(sx, sy, &cur, self.scale) {
                            self.logic.typed(f, &t);
                        }
                        self.dirty = true;
                    }
                    Act::OpenDialog => {
                        if let Some(path) = file_dialog(&self.win) {
                            self.logic.load(None, &path);
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

    /// The open sample dialog: the desktop's own (through its portal on Linux; on macOS AppKit's).
    fn file_dialog(win: &Window) -> Option<String> {
        if os_plugkit::no_modal() {
            return None;
        }
        let path = win.dialog(move || {
            rfd::FileDialog::new()
                .set_title("Sample for the pad")
                .add_filter("Audio files", &["wav", "flac", "ogg", "wv", "aif", "aiff", "mp3"])
                .pick_file()
        });
        path.flatten().map(|p| p.to_string_lossy().into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn logic() -> Logic {
        let mut l = Logic::new(Arc::new(DrumParams::default()), Arc::new(Shared::default()));
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
    fn pads_choose_play_and_take_dropped_samples() {
        let mut l = logic();
        let (x, y) = centre(&l, &Target::Pad(1));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(l.view.pad, 1);
        assert_eq!(l.shared.previews.lock().unwrap().as_slice().first().map(|p| p.0), Some(36));
        assert!(l.hint.starts_with("Pad A2"), "{}", l.hint);
        l.up(Btn::Left);
        // A file dropped on pad 3 goes onto pad 3.
        let wav = crate::tests::wav("drop", 0.5);
        let (x, y) = centre(&l, &Target::Pad(2));
        let at = l.pad_at(x, y);
        l.load(at, &wav.to_string_lossy());
        assert_eq!(l.view.pad, 2);
        assert_eq!(l.state().pad(2).unwrap().layers[0].path, wav.to_string_lossy());
        assert!(l.shared.loaded(2, 0));
        paint(&mut l);
        // Bank B shows pads 17..32.
        let (x, y) = centre(&l, &Target::Bank(1));
        l.down(x, y, Btn::Left, Mods::default(), false);
        paint(&mut l);
        assert!(l.hits.iter().any(|h| h.t == Target::Pad(31)) && !l.hits.iter().any(|h| h.t == Target::Pad(0)));
        let _ = std::fs::remove_file(wav);
    }

    #[test]
    fn values_drag_wheel_and_type_in() {
        let mut l = logic();
        let (x, y) = centre(&l, &Target::Value(Field::PadVolume));
        let acts = l.down(x, y, Btn::Left, Mods::default(), false);
        assert!(acts.contains(&Act::Lock));
        l.delta(0.0, -40.0, Mods::default());
        l.up(Btn::Left);
        assert!(l.value(Field::PadVolume).unwrap() > state::NEW_PAD_VOLUME);
        let (x, y) = centre(&l, &Target::Value(Field::Key));
        l.wheel(x, y, 1.0, Mods::default());
        assert_eq!(l.value(Field::Key), Some(38));
        l.typed(Field::Key, "C5");
        assert_eq!(l.value(Field::Key), Some(60));
        l.typed(Field::Cut, "3");
        assert_eq!(l.value(Field::Cut), Some(3));
        l.typed(Field::PadPan, "-200");
        assert_eq!(l.value(Field::PadPan), Some(-128));
        assert_eq!((parse_key("F#3"), parse_key("Db4"), parse_key("x")), (Some(42), Some(49), None));
        // The state keeps everything else as it was.
        let st = l.state();
        assert_eq!(st.pad(0).unwrap().pan, -128);
        assert_eq!(st.pad(1).unwrap(), State::default().pad(1).unwrap());
    }

    /// Renders the editor with a few pads loaded to `OS_SHOT_DIR` (for a look by eye).
    #[test]
    fn render_editor() {
        let Ok(dir) = std::env::var("OS_SHOT_DIR") else { return };
        let mut l = logic();
        let wav = crate::tests::wav("shot", 0.5);
        for n in [0, 1, 2, 5] {
            l.load(Some(n), &wav.to_string_lossy());
        }
        let mut st = l.state();
        st.set_name(1, "Kick");
        st.set_pad(1, state::PAD_COLOR, 0x00a19145);
        l.shared.set_state(&l.params, &st);
        l.view.pad = 1;
        l.view.mouse = Some(centre(&l, &Target::Value(Field::PadPan)));
        let mut c = os_ui::Canvas::new(view::W * 2, view::H * 2, 2.0).unwrap();
        l.paint(&mut c, &os_plugkit::palette());
        c.pixmap.save_png(std::path::Path::new(&dir).join("lemon_drum_sampler.png")).unwrap();
        let _ = std::fs::remove_file(wav);
    }
}
