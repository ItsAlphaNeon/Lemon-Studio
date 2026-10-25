// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The plugin editor: a child of the host's window (Win32, X11 on Linux, an NSView on macOS) that paints `view` with
//! the os-ui canvas, and the editor logic (platform independent): knob drags with the cursor held in place, stepped
//! boxes, wheel and reset gestures, the page tabs, display switches and speed fader, the display and parameter menus,
//! Store / Flip. Edits show and sound at once (`os_plugkit::vals`).
//!
// spec: plugins/lemon-limiter.md (§2 controls, §3 display, §4 hints, §5 interaction), plugins/host-link.md

use std::any::Any;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::Arc;

use nih_plug::prelude::*;
use os_engine::effects::dynamics::{FLAG_EXACT_CEILING, PARAMS};
use os_plugkit::vals::Vals;
use os_plugkit::MenuDef;
use os_ui::kit::menu::{Mark, MenuItem, MenuStyle};

use crate::params::{self, LimiterParams, DEFS};
use crate::view::{self, Target, ViewState, LAYERS};
use crate::Shared;

pub struct LimiterEditor {
    pub params: Arc<LimiterParams>,
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

impl LimiterEditor {
    pub fn new(params: Arc<LimiterParams>, shared: Arc<Shared>) -> Self {
        LimiterEditor {
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

impl Editor for LimiterEditor {
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
// Laws (spec §5)

/// Pixels of drag for a knob's whole range.
pub const KNOB_PX: f32 = 200.0;
const FINE: f32 = 0.25;
/// Parameters with more steps than this move by 1/`WHEEL_STEPS` of their range per wheel notch.
const WHEEL_STEPS: f32 = 128.0;
/// Pixels of drag per step of a stepped box.
pub const SPIN_PX: f32 = 6.0;
/// Width of a stepped box's arrow column.
const ARROWS_W: f32 = 9.0;

/// Seconds of sound per display column at scrolling speed `s` (0..1): 32 ms slowest, 2 ms fastest (Lemon Studio
/// choice, spec "Open questions").
pub fn column_seconds(s: f32) -> f64 {
    0.002 * 16f64.powf(f64::from(1.0 - s.clamp(0.0, 1.0)))
}

// ---------------------------------------------------------------------------------------------------------------
// Menus

#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    None,
    Reset(String),
    CopyValue(String),
    PasteValue(String),
    TypeIn(String),
    Automate(String),
    /// Display menu: "0.2 dB limiter safety".
    Safety,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuReq {
    Param(String),
    Display,
}

// ---------------------------------------------------------------------------------------------------------------
// Editor logic

/// Something the window has to do after an event.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Repaint,
    /// Hide the cursor and hold it in place (knob, box and fader drags).
    Lock,
    Menu(MenuReq, f32, f32),
    TypeIn(String, f32, f32),
    /// Ask the host for an automation clip of parameter `index` (host link).
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
    /// Knob: `v` = the unrounded normalised value the drag has reached (steps would swallow small moves).
    Knob { id: String, v: f32 },
    /// A stepped box: pixels moved since the last step.
    Spin { t: Target, acc: f32 },
    Speed,
}

pub struct Logic {
    pub vals: Vals,
    pub params: Arc<LimiterParams>,
    pub shared: Arc<Shared>,
    pub view: ViewState,
    pub hits: Vec<view::Hit>,
    drag: Option<Drag>,
    /// Hint text (a tab before the right-aligned part).
    pub hint: String,
    copied: Option<f32>,
    pub hosted: bool,
}

/// Editor settings from the state image's editor bytes (Lemon Studio's use, spec §6).
pub fn view_from(raw: &[u8]) -> ViewState {
    let hidden = params::byte(raw, params::LAYERS_BYTE);
    let speed = match params::byte(raw, params::SPEED_BYTE) {
        0 => 0.5,
        b => f32::from(b - 1) / 254.0,
    };
    ViewState { page: params::byte(raw, params::PAGE_BYTE).min(1), layers: std::array::from_fn(|k| hidden & (1 << k) == 0), speed, ..ViewState::default() }
}

/// Writes the editor settings into the editor bytes.
pub fn store_view(raw: &mut [u8], v: &ViewState) {
    let hidden = (0..LAYERS).filter(|&k| !v.layers[k]).fold(0u8, |m, k| m | (1 << k));
    let speed = if (v.speed - 0.5).abs() < 1e-6 { 0 } else { 1 + (v.speed.clamp(0.0, 1.0) * 254.0).round() as u8 };
    for (at, b) in [(params::PAGE_BYTE, v.page), (params::LAYERS_BYTE, hidden), (params::SPEED_BYTE, speed)] {
        if let Some(x) = raw.get_mut(at) {
            *x = b;
        }
    }
}

impl Logic {
    pub fn new(params: Arc<LimiterParams>, shared: Arc<Shared>, ctx: Option<Arc<dyn GuiContext>>) -> Self {
        let vals = Vals::new(&*params, params.live.clone(), ctx);
        let view = view_from(&params.raw_copy());
        let l = Logic { vals, params, shared, view, hits: Vec::new(), drag: None, hint: String::new(), copied: None, hosted: false };
        l.apply_speed();
        l
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    fn hit(&self, x: f32, y: f32) -> Option<(Target, os_ui::Rect)> {
        self.hits.iter().rev().find(|h| h.r.contains(x, y)).map(|h| (h.t.clone(), h.r))
    }

    fn name(id: &str) -> String {
        params::index(id).map_or_else(|| id.to_string(), |k| DEFS[k].name.to_string())
    }

    fn hint_for(&self, t: Option<&Target>) -> String {
        t.map_or_else(String::new, |t| view::hint(t, &self.vals, &Self::name, self.view.mouse))
    }

    fn range(id: &str) -> (i32, i32) {
        params::index(id).map_or((0, 1), |k| os_engine::effects::dynamics::RANGES[k])
    }

    fn reset(&mut self, id: &str) {
        let d = self.vals.default_norm(id);
        self.vals.edit(id, d);
    }

    /// The display's samples per column follow the scrolling speed.
    fn apply_speed(&self) {
        let rate = f64::from(self.shared.rate.load(Ordering::Relaxed));
        self.shared.column_len.store((rate * column_seconds(self.view.speed)).round().max(1.0) as u32, Ordering::Relaxed);
    }

    /// Editor settings changed: kept in the state image.
    fn save_view(&self) {
        let v = self.view.clone();
        self.params.edit_raw(|b| store_view(b, &v));
        self.apply_speed();
    }

    pub fn sidechain(&self) -> u8 {
        params::byte(&self.params.raw_copy(), params::SIDECHAIN_BYTE)
    }

    fn set_sidechain(&self, v: i32) {
        let v = v.clamp(0, i32::from(params::SIDECHAIN_MAX)) as u8;
        self.params.edit_raw(|b| b[params::SIDECHAIN_BYTE] = v);
    }

    /// One step of a stepped box.
    fn step(&mut self, t: &Target, d: i32) {
        match t {
            Target::Spin(id) => {
                let (lo, hi) = Self::range(id);
                let v = (self.vals.plain(id).round() as i32 + d).clamp(lo, hi);
                self.vals.edit_plain(id, v as f32);
            }
            Target::Sidechain => self.set_sidechain(i32::from(self.sidechain()) + d),
            _ => {}
        }
    }

    fn values(&self) -> [i32; PARAMS] {
        std::array::from_fn(|k| self.vals.plain(DEFS[k].id).round() as i32)
    }

    /// Store: the parameters into the spare bank. Flip: the parameters and the spare bank swap (spec §5).
    fn store(&mut self) {
        let cur = self.values();
        self.params.edit_raw(|b| params::set_spare(b, &cur));
    }

    fn flip(&mut self) {
        let cur = self.values();
        let spare = params::spare(&self.params.raw_copy());
        for (k, d) in DEFS.iter().enumerate() {
            let (lo, hi) = os_engine::effects::dynamics::RANGES[k];
            if cur[k] != spare[k] {
                self.vals.edit_plain(d.id, spare[k].clamp(lo, hi) as f32);
            }
        }
        self.params.edit_raw(|b| params::set_spare(b, &cur));
    }

    pub fn down(&mut self, x: f32, y: f32, btn: Btn, m: Mods, _dbl: bool) -> Vec<Act> {
        self.view.mouse = Some((x, y));
        let Some((t, r)) = self.hit(x, y) else { return vec![] };
        let mut acts = vec![Act::Repaint];
        match (&t, btn) {
            (Target::Knob(id) | Target::Spin(id), Btn::Left) if m.alt => self.reset(id),
            (Target::Knob(id) | Target::Spin(id), Btn::Middle) => self.reset(id),
            (Target::Knob(id), Btn::Left) => {
                self.vals.begin(id);
                if id == "att" {
                    self.shared.attack_held.store(true, Ordering::Release);
                }
                self.drag = Some(Drag::Knob { id: id.clone(), v: self.vals.norm(id) });
                self.view.pressed = Some(t.clone());
                acts.push(Act::Lock);
            }
            (Target::Knob(id) | Target::Spin(id), Btn::Right) => acts.push(Act::Menu(MenuReq::Param(id.clone()), x, y)),
            (Target::Spin(_) | Target::Sidechain, Btn::Left) => {
                if x >= r.right() - ARROWS_W {
                    // The arrows: one step up or down.
                    self.step(&t, if y < r.cy() { 1 } else { -1 });
                } else {
                    if let Target::Spin(id) = &t {
                        self.vals.begin(id);
                    }
                    self.drag = Some(Drag::Spin { t: t.clone(), acc: 0.0 });
                    self.view.pressed = Some(t.clone());
                    acts.push(Act::Lock);
                }
            }
            (Target::Page(i), Btn::Left) => {
                self.view.page = *i;
                self.save_view();
            }
            (Target::Layer(k), Btn::Left) => {
                self.view.layers[*k] = !self.view.layers[*k];
                self.save_view();
            }
            (Target::Speed, Btn::Left) => {
                self.drag = Some(Drag::Speed);
                self.view.pressed = Some(t.clone());
                acts.push(Act::Lock);
            }
            (Target::Menu, Btn::Left | Btn::Right) => acts.push(Act::Menu(MenuReq::Display, x, y)),
            (Target::Store, Btn::Left) => self.store(),
            (Target::Flip, Btn::Left) => self.flip(),
            _ => {}
        }
        self.hint = self.hint_for(Some(&t));
        acts
    }

    /// Mouse movement during a locked drag (logical px since the last event).
    pub fn delta(&mut self, _dx: f32, dy: f32, m: Mods) -> Vec<Act> {
        let f = if m.ctrl { FINE } else { 1.0 };
        match self.drag.clone() {
            Some(Drag::Knob { id, v }) => {
                let v = (v - dy * f / KNOB_PX).clamp(0.0, 1.0);
                self.vals.set(&id, v);
                self.drag = Some(Drag::Knob { id, v });
            }
            Some(Drag::Spin { t, acc }) => {
                let mut acc = acc - dy;
                while acc.abs() >= SPIN_PX {
                    let d = acc.signum();
                    acc -= d * SPIN_PX;
                    match &t {
                        Target::Spin(id) => {
                            let (lo, hi) = Self::range(id);
                            let v = (self.vals.plain(id).round() + d).clamp(lo as f32, hi as f32);
                            let n = self.vals.plain_to_norm(id, v);
                            self.vals.set(id, n);
                        }
                        _ => self.step(&t, d as i32),
                    }
                }
                self.drag = Some(Drag::Spin { t, acc });
            }
            Some(Drag::Speed) => {
                self.view.speed = (self.view.speed - dy * f / (view::SPEED_BOT - view::SPEED_TOP)).clamp(0.0, 1.0);
                self.apply_speed();
            }
            None => return vec![],
        }
        self.hint = self.hint_for(self.view.pressed.as_ref());
        vec![Act::Repaint]
    }

    pub fn up(&mut self, _btn: Btn) -> Vec<Act> {
        let Some(d) = self.drag.take() else { return vec![] };
        match d {
            Drag::Knob { id, .. } => {
                self.vals.end(&id);
                // The look-ahead follows the attack now (spec §4.2).
                if id == "att" {
                    self.shared.attack_held.store(false, Ordering::Release);
                }
            }
            Drag::Spin { t: Target::Spin(id), .. } => self.vals.end(&id),
            Drag::Spin { .. } => {}
            Drag::Speed => self.save_view(),
        }
        self.view.pressed = None;
        vec![Act::Repaint]
    }

    pub fn moved(&mut self, x: f32, y: f32, _m: Mods) -> Vec<Act> {
        self.view.mouse = Some((x, y));
        if self.drag.is_none() {
            let t = self.hit(x, y).map(|h| h.0);
            self.hint = self.hint_for(t.as_ref());
        }
        vec![Act::Repaint]
    }

    pub fn leave(&mut self) -> Vec<Act> {
        self.view.mouse = None;
        self.hint.clear();
        vec![Act::Repaint]
    }

    /// Wheel: one step per notch (1/128 of the range for the knobs with many steps), spec §5.
    pub fn wheel(&mut self, x: f32, y: f32, notches: f32, _m: Mods) -> Vec<Act> {
        let Some((t, _)) = self.hit(x, y) else { return vec![] };
        let d = notches.signum();
        match &t {
            Target::Knob(id) | Target::Spin(id) => {
                let (lo, hi) = Self::range(id);
                let steps = (hi - lo) as f32;
                let step = if steps > WHEEL_STEPS { (steps / WHEEL_STEPS).round() } else { 1.0 };
                let plain = (self.vals.plain(id) + d * step).clamp(lo as f32, hi as f32);
                self.vals.edit_plain(id, plain);
            }
            Target::Sidechain => self.step(&t, d as i32),
            Target::Speed => {
                self.view.speed = (self.view.speed + d / 32.0).clamp(0.0, 1.0);
                self.save_view();
            }
            _ => return vec![],
        }
        self.hint = self.hint_for(Some(&t));
        vec![Act::Repaint]
    }

    /// The "0.2 dB limiter safety" tick: on while flags bit 0 is clear (spec §3).
    pub fn safety(&self) -> bool {
        params::byte(&self.params.raw_copy(), params::FLAGS_BYTE) & FLAG_EXACT_CEILING == 0
    }

    pub fn menu(&self, req: &MenuReq) -> MenuDef<Cmd> {
        match req {
            MenuReq::Param(id) => {
                let mut m = MenuDef::new(MenuStyle::Context);
                m.head(&Self::name(id), Cmd::None);
                m.add(MenuItem::item("Reset"), Cmd::Reset(id.clone()));
                m.sep(Cmd::None);
                m.add(MenuItem::item("Copy value"), Cmd::CopyValue(id.clone()));
                m.add(MenuItem::item("Paste value").enabled(self.copied.is_some()), Cmd::PasteValue(id.clone()));
                m.add(MenuItem::item("Type in value..."), Cmd::TypeIn(id.clone()));
                // Automation belongs to the host: offered when Lemon Studio hosts the editor (host link).
                m.sep(Cmd::None);
                m.add(MenuItem::item("Create automation clip").enabled(self.hosted), Cmd::Automate(id.clone()));
                m
            }
            MenuReq::Display => {
                let mut m = MenuDef::new(MenuStyle::Context);
                m.add(MenuItem::item("0.2 dB limiter safety").mark(Mark::Check(self.safety())), Cmd::Safety);
                let mut about = MenuDef::new(MenuStyle::Context);
                about.head("Lemon Limiter", Cmd::None);
                about.add(MenuItem::item(&format!("Version {}", env!("CARGO_PKG_VERSION"))).enabled(false), Cmd::None);
                about.add(MenuItem::item("Lemon Studio's recreation of Fruity Limiter").enabled(false), Cmd::None);
                m.submenu("About", about, Cmd::None);
                m
            }
        }
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
            Cmd::Automate(id) => return params::index(&id).map(Act::Automate).into_iter().collect(),
            Cmd::Safety => self.params.edit_raw(|b| b[params::FLAGS_BYTE] ^= FLAG_EXACT_CEILING),
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

    /// Paints the editor; `hint` is drawn when the host does not show it.
    pub fn paint(&mut self, c: &mut os_ui::Canvas, palette: &os_theme::Palette) {
        let history = self.shared.history.lock().map(|h| h.latest(view::DISPLAY.w as usize)).unwrap_or_default();
        let look = view::Look { sidechain: self.sidechain(), history: &history };
        let hint = if self.hosted { String::new() } else { self.hint.clone() };
        self.hits = view::paint(c, palette, &self.vals, &self.view, &look, &hint);
    }

    /// Columns written so far (a repaint when it changes).
    pub fn written(&self) -> u64 {
        self.shared.history.lock().map_or(0, |h| h.written)
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
    use crate::params::LimiterParams;
    use crate::view::{self, ViewState};
    use crate::Shared;

    const TIMER: usize = 1;
    const WM_CHANGED: u32 = WM_APP + 1;

    struct Win {
        hwnd: HWND,
        canvas: Canvas,
        buf: Vec<u32>,
        scale: f32,
        /// Painted size in device px.
        size: (u32, u32),
        params: Arc<LimiterParams>,
        logic: Logic,
        palette: Palette,
        last_sig: u64,
        last_raw: u64,
        last_written: u64,
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
            self.2.attack_held.store(false, Ordering::Release);
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

    pub fn open(parent: *mut c_void, scale: f32, params: Arc<LimiterParams>, shared: Arc<Shared>, ctx: Arc<dyn GuiContext>, slot: Arc<AtomicIsize>) -> Handle {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let px = |v: u32| (v as f32 * scale).round() as u32;
        let size = (px(view::W), px(view::H));
        shared.editor_open.store(true, Ordering::Relaxed);
        unsafe {
            let class = pw::register("LemonLimiterView", Some(wndproc));
            let win = Box::new(Win {
                hwnd: std::ptr::null_mut(),
                canvas: Canvas::new(size.0, size.1, scale).expect("canvas"),
                buf: vec![0; (size.0 * size.1) as usize],
                scale,
                size,
                logic: Logic::new(params.clone(), shared.clone(), Some(ctx)),
                params,
                palette: os_plugkit::palette(),
                last_sig: 0,
                last_raw: u64::MAX,
                last_written: u64::MAX,
                last_view: ViewState::default(),
                last_hint: String::new(),
                lock: CursorLock::default(),
                tracking: false,
                slot: slot.clone(),
            });
            let title = pw::wide("Lemon Limiter");
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
                    if w.logic.dragging() && !acts.contains(&Act::Lock) {
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

        /// Timer / parameter change: expire confirmed overlay values, repaint when anything shown changed (the display
        /// scrolls with each new column).
        fn poll(&mut self) {
            let dirty = self.logic.vals.expire();
            let sig = self.logic.vals.signature();
            let rv = self.params.raw_version.load(Ordering::Acquire);
            let written = self.logic.written();
            if dirty || sig != self.last_sig || rv != self.last_raw || written != self.last_written || self.logic.view != self.last_view || self.logic.hint != self.last_hint {
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
            self.last_written = self.logic.written();
            self.logic.paint(&mut self.canvas, &self.palette);
            self.last_sig = self.logic.vals.signature();
            self.last_raw = self.params.raw_version.load(Ordering::Acquire);
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
    use crate::params::LimiterParams;
    use crate::view::{self, ViewState};
    use crate::Shared;

    /// The editor on its GUI thread.
    struct Ed {
        win: Window,
        canvas: Canvas,
        buf: Vec<u32>,
        scale: f32,
        size: (u32, u32),
        params: Arc<LimiterParams>,
        logic: Logic,
        palette: Palette,
        last_sig: u64,
        last_raw: u64,
        last_written: u64,
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
            self.1.attack_held.store(false, Ordering::Release);
        }
    }

    pub fn open(parent: px::Parent, scale: f32, params: Arc<LimiterParams>, shared: Arc<Shared>, ctx: Arc<dyn GuiContext>, slot: &px::Slot) -> Handle {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let size = ((view::W as f32 * scale).round() as u32, (view::H as f32 * scale).round() as u32);
        let sh = shared.clone();
        // One page; joined, the editor keeps its size (it has no strip row of its own).
        let wake = slot.clone();
        let link = Peer::register(parent as u64, vec![PageIcon::Plugin], (view::W, view::H), move || px::notify(&wake));
        let peer = link.clone();
        let thread = px::spawn(parent, size, slot, move |win| {
            let Ok(canvas) = Canvas::new(size.0, size.1, scale) else { return };
            let logic = Logic::new(params.clone(), shared, Some(ctx));
            let ed = Ed {
                win,
                canvas,
                buf: Vec::new(),
                scale,
                size,
                params,
                logic,
                palette: os_plugkit::palette(),
                last_sig: 0,
                last_raw: u64::MAX,
                last_written: u64::MAX,
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

        /// Timer / parameter change: expire confirmed overlay values, repaint when anything shown changed.
        fn poll(&mut self) {
            let dirty = self.logic.vals.expire();
            let sig = self.logic.vals.signature();
            let rv = self.params.raw_version.load(Ordering::Acquire);
            let written = self.logic.written();
            if dirty || sig != self.last_sig || rv != self.last_raw || written != self.last_written || self.logic.view != self.last_view || self.logic.hint != self.last_hint {
                self.dirty = true;
            }
            if self.logic.hint != self.last_hint {
                self.last_hint.clone_from(&self.logic.hint);
                // The host shows the hint in its title bar (hosted mode).
                self.link.set_hint(&self.logic.hint);
                self.link.to_host(Cmd::Changed, 0);
            }
            // Joined the strip: "Create automation clip" works and the host shows the hint.
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
            self.last_written = self.logic.written();
            self.logic.paint(&mut self.canvas, &self.palette);
            self.last_sig = self.logic.vals.signature();
            self.last_raw = self.params.raw_version.load(Ordering::Acquire);
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
    use os_engine::effects::dynamics::{DEFAULTS, LIM_ATT, LIM_ATT_CURVE};

    fn logic() -> Logic {
        let mut l = Logic::new(Arc::new(LimiterParams::default()), Arc::new(Shared::default()), None);
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

    #[test]
    fn knob_drags_wheel_reset_and_the_attack_hold() {
        let mut l = logic();
        let t = Target::Knob("att".into());
        let (x, y) = centre(&l, &t);
        assert_eq!(l.params.v(LIM_ATT), DEFAULTS[LIM_ATT]);
        let acts = l.down(x, y, Btn::Left, Mods::default(), false);
        assert!(acts.contains(&Act::Lock));
        assert!(l.shared.attack_held.load(Ordering::Acquire));
        // 200 px = the whole range: 20 px up = +1000.
        for _ in 0..20 {
            l.delta(0.0, -1.0, Mods::default());
        }
        assert_eq!(l.vals.plain("att"), 2115.0);
        assert_eq!(l.hint, format!("Limiter attack time: {}\tupdated on release", l.vals.text("att")));
        l.up(Btn::Left);
        assert!(!l.shared.attack_held.load(Ordering::Acquire));
        assert_eq!(l.params.v(LIM_ATT), 2115);
        // Wheel: 1/128 of the range; Alt+click resets.
        l.wheel(x, y, 1.0, Mods::default());
        assert_eq!(l.vals.plain("att"), 2193.0);
        l.down(x, y, Btn::Left, Mods { alt: true, ..Mods::default() }, false);
        assert_eq!(l.vals.plain("att"), 1115.0);
        let acts = l.down(x, y, Btn::Right, Mods::default(), false);
        assert!(matches!(acts.last(), Some(Act::Menu(MenuReq::Param(id), _, _)) if id == "att"));
        l.typed("att", "5ms");
        assert!((os_engine::effects::dynamics::time_ms(l.vals.plain("att") as i32) - 5.0).abs() < 0.01);
    }

    #[test]
    fn spinners_step_by_drag_wheel_and_arrows() {
        let mut l = logic();
        let t = Target::Spin("att_curve".into());
        let (x, y) = centre(&l, &t);
        l.down(x, y, Btn::Left, Mods::default(), false);
        l.delta(0.0, -13.0, Mods::default());
        l.up(Btn::Left);
        assert_eq!(l.params.v(LIM_ATT_CURVE), 5);
        l.wheel(x, y, -1.0, Mods::default());
        assert_eq!(l.vals.plain("att_curve"), 4.0);
        let h = l.hits.iter().find(|h| h.t == t).unwrap().r;
        l.down(h.right() - 2.0, h.y + 2.0, Btn::Left, Mods::default(), false);
        assert_eq!(l.vals.plain("att_curve"), 5.0);
        assert_eq!(l.hint, "Limiter attack curve: 5");
        // The sidechain box: "--" = none, stored in the state image.
        l.view.page = 1;
        paint(&mut l);
        let (x, y) = centre(&l, &Target::Sidechain);
        l.wheel(x, y, 1.0, Mods::default());
        assert_eq!(l.sidechain(), 1);
        assert_eq!(params::byte(&l.params.raw_copy(), params::SIDECHAIN_BYTE), 1);
        l.wheel(x, y, -1.0, Mods::default());
        l.wheel(x, y, -1.0, Mods::default());
        assert_eq!(l.sidechain(), 0);
    }

    #[test]
    fn pages_layers_and_speed_are_kept_in_the_editor_bytes() {
        let mut l = logic();
        let (x, y) = centre(&l, &Target::Page(1));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(l.view.page, 1);
        paint(&mut l);
        centre(&l, &Target::Knob("c_knee".into()));
        let (x, y) = centre(&l, &Target::Layer(2));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(l.hint, "Show analysis & gain envelopes");
        let raw = l.params.raw_copy();
        let v = view_from(&raw);
        assert_eq!((v.page, v.layers), (1, [true, true, false, true]));
        // A fresh instance shows LIMIT with every layer, at the middle speed.
        assert_eq!(view_from(&params::default_state()), ViewState::default());
        let mut b = params::default_state();
        store_view(&mut b, &ViewState { speed: 1.0, ..ViewState::default() });
        assert_eq!(view_from(&b).speed, 1.0);
    }

    #[test]
    fn store_and_flip_swap_with_the_spare_bank() {
        let mut l = logic();
        l.vals.edit_plain("gain", 1500.0);
        let (x, y) = centre(&l, &Target::Store);
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(params::spare(&l.params.raw_copy())[0], 1500);
        l.vals.edit_plain("gain", 300.0);
        let (x, y) = centre(&l, &Target::Flip);
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(l.vals.plain("gain"), 1500.0);
        assert_eq!(params::spare(&l.params.raw_copy())[0], 300);
    }

    /// `LIMITER_PNG=<dir> cargo test -p lemon-limiter -- --ignored`: both pages with some display history, as PNGs.
    #[test]
    #[ignore]
    fn render_pages() {
        let Some(dir) = std::env::var_os("LIMITER_PNG") else { return };
        let mut l = logic();
        let (rate, len) = (44100.0, 352.0);
        let input: Vec<f32> = (0..44100 * 5).map(|i| ((i as f64 / rate * 2.0).fract() < 0.3) as u8 as f32 * 1.6 * (i as f64 * 0.05).sin() as f32).collect();
        let mut d = os_engine::effects::dynamics::Dynamics::new(rate, DEFAULTS, 0);
        d.set_meter(len as usize);
        for c in input.chunks(512) {
            let (mut a, mut b) = (c.to_vec(), c.to_vec());
            d.process(&mut a, &mut b, None);
            let mut h = l.shared.history.lock().unwrap();
            for col in d.columns.drain(..) {
                let p = h.pos;
                h.cols[p] = col;
                h.pos = (p + 1) % crate::HISTORY;
                h.written += 1;
            }
        }
        l.vals.edit_plain("c_thres", 700.0);
        for page in 0..2 {
            l.view.page = page;
            l.hint = "Gain: 0.0dB  1.00".into();
            let mut c = os_ui::Canvas::new(view::W, view::H, 1.0).unwrap();
            l.paint(&mut c, &os_plugkit::palette());
            let mut px = vec![0u32; (view::W * view::H) as usize];
            c.copy_to_xrgb(&mut px);
            let mut pm = tiny_skia::Pixmap::new(view::W, view::H).unwrap();
            for (o, p) in pm.data_mut().chunks_mut(4).zip(&px) {
                o.copy_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255]);
            }
            pm.save_png(std::path::Path::new(&dir).join(format!("limiter_{page}.png"))).unwrap();
        }
    }

    #[test]
    fn display_menu_toggles_the_safety_and_hints_read_levels() {
        let mut l = logic();
        assert!(l.safety());
        let m = l.menu(&MenuReq::Display);
        assert_eq!(m.find("0.2 dB limiter safety"), Some(Cmd::Safety));
        l.pick(&MenuReq::Display, Cmd::Safety, 0.0, 0.0);
        assert!(!l.safety());
        assert_eq!(params::byte(&l.params.raw_copy(), params::FLAGS_BYTE), 1);
        // The display reads 0 dB at half height.
        let mid = view::DISPLAY.bottom() - 1.0 - (view::DISPLAY.h - 1.0) / 2.0;
        l.moved(300.0, mid, Mods::default());
        assert_eq!(l.hint, "0.0dB 1.00");
        for (t, want) in [
            (Target::Knob("gain".into()), "Gain: 0.0dB  1.00"),
            (Target::Knob("g_thres".into()), "Noise threshold: -INFdB  0.00"),
            (Target::Knob("rel".into()), "Limiter release time: 85.53ms"),
            (Target::Store, "Store in spare state"),
            (Target::Speed, "Scrolling speed"),
        ] {
            let (x, y) = centre(&l, &t);
            l.moved(x, y, Mods::default());
            assert_eq!(l.hint, want);
        }
    }
}
