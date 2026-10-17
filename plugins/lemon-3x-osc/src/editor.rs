// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The plugin editor: a child of the host's window (Win32, X11 on Linux, an NSView on macOS) that paints `view` with
//! the os-ui canvas, and the editor logic (platform independent): knob / fader drags with the cursor held in place,
//! envelope node drags, wheel and reset gestures, shape buttons and switches, the parameter menus. Edits show and sound
//! at once (`os_plugkit::vals`).
//!
// spec: plugins/lemon-3x-osc.md (§4 hints, §5 interaction), plugins/host-link.md

use std::any::Any;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::Arc;

use nih_plug::prelude::*;
use os_plugkit::vals::Vals;
use os_plugkit::MenuDef;
use os_ui::kit::menu::{MenuItem, MenuStyle};

use crate::params::{Osc3Params, FILTERS};
use crate::view::{self, Hit, Target, ViewState};
use crate::Shared;

pub struct Osc3Editor {
    pub params: Arc<Osc3Params>,
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

impl Osc3Editor {
    pub fn new(params: Arc<Osc3Params>, shared: Arc<Shared>) -> Self {
        Osc3Editor {
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

impl Editor for Osc3Editor {
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
// Laws (spec §5)

/// Pixels of drag for a knob's whole range.
pub const KNOB_PX: f32 = 200.0;
const FINE: f32 = 0.25;
/// Parameters with more steps than this move by 1/`WHEEL_STEPS` of their range per wheel notch (times, speeds).
const WHEEL_STEPS: f32 = 128.0;

// ---------------------------------------------------------------------------------------------------------------
// Menus

#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    None,
    Reset(String),
    CopyValue(String),
    PasteValue(String),
    TypeIn(String),
    Choice(String, i32),
    Automate(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuReq {
    Param(String),
    List(String),
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
    /// Knob or fader: `v` = the unrounded normalised value the drag has reached (steps would swallow small moves).
    Knob { id: String, v: f32 },
    /// Envelope node: start point and the start values of the knobs it moves.
    EnvPoint { i: u8, x0: f32, y0: f32, v: [f32; 2] },
}

pub struct Logic {
    pub vals: Vals,
    pub params: Arc<Osc3Params>,
    pub shared: Arc<Shared>,
    pub view: ViewState,
    pub hits: Vec<Hit>,
    drag: Option<Drag>,
    /// Hint text (a tab before the right-aligned part).
    pub hint: String,
    copied: Option<f32>,
    pub hosted: bool,
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

impl Logic {
    pub fn new(params: Arc<Osc3Params>, shared: Arc<Shared>, ctx: Option<Arc<dyn GuiContext>>) -> Self {
        let vals = Vals::new(&*params, params.live.clone(), ctx);
        Logic { vals, params, shared, view: ViewState::default(), hits: Vec::new(), drag: None, hint: String::new(), copied: None, hosted: false }
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    fn hit(&self, x: f32, y: f32) -> Option<Target> {
        self.hits.iter().rev().find(|h| h.r.contains(x, y)).map(|h| h.t.clone())
    }

    fn name(&self, id: &str) -> String {
        self.params.defs.iter().find(|d| d.id == id).map_or_else(|| id.to_string(), |d| d.name.clone())
    }

    fn hint_for(&self, t: Option<&Target>) -> String {
        match t {
            Some(t) => view::hint(t, &self.vals, &|id| self.name(id)),
            None => String::new(),
        }
    }

    fn range(&self, id: &str) -> (i32, i32) {
        self.params.defs.iter().find(|d| d.id == id).map_or((0, 1), |d| (d.min, d.max))
    }

    fn reset(&mut self, id: &str) {
        let d = self.vals.default_norm(id);
        self.vals.edit(id, d);
    }

    fn toggle_flag(&mut self, b: usize) {
        let cur = self.vals.plain("flags").round() as i32;
        self.vals.edit_plain("flags", (cur ^ (1 << b)) as f32);
    }

    pub fn down(&mut self, x: f32, y: f32, btn: Btn, m: Mods, _dbl: bool) -> Vec<Act> {
        self.view.mouse = Some((x, y));
        let Some(t) = self.hit(x, y) else { return vec![] };
        if !view::works(&t) {
            // Greyed-out controls only say so.
            self.hint = self.hint_for(Some(&t));
            return vec![Act::Repaint];
        }
        let mut acts = vec![Act::Repaint];
        match (&t, btn) {
            (Target::Knob(id), Btn::Left) if m.alt => self.reset(id),
            (Target::Knob(id), Btn::Middle) => self.reset(id),
            (Target::Knob(id), Btn::Left) => {
                self.vals.begin(id);
                self.drag = Some(Drag::Knob { id: id.clone(), v: self.vals.norm(id) });
                self.view.pressed = Some(t.clone());
                acts.push(Act::Lock);
            }
            (Target::Knob(id) | Target::Toggle(id) | Target::Set(id, _), Btn::Right) => acts.push(Act::Menu(MenuReq::Param(id.clone()), x, y)),
            (Target::List(id), Btn::Left | Btn::Right) => acts.push(Act::Menu(MenuReq::List(id.clone()), x, y)),
            (Target::Toggle(id), Btn::Left) => {
                let on = self.vals.plain(id) > 0.5;
                self.vals.edit_plain(id, if on { 0.0 } else { 1.0 });
            }
            (Target::Set(id, v), Btn::Left) => self.vals.edit_plain(id, *v),
            (Target::Flag(b), Btn::Left) => self.toggle_flag(*b),
            (Target::Tab(i), Btn::Left) => self.view.tab = *i,
            (Target::SubTab(i), Btn::Left) => self.view.sub = *i,
            (Target::EnvPoint(i), Btn::Left) => {
                let p = view::ENV_PREFIX[usize::from(self.view.sub.min(4))];
                let ids = env_point_ids(p, *i);
                let v = [self.vals.norm(&ids[0]), ids.get(1).map_or(0.0, |s| self.vals.norm(s))];
                for id in &ids {
                    self.vals.begin(id);
                }
                self.view.pressed = Some(t.clone());
                self.drag = Some(Drag::EnvPoint { i: *i, x0: x, y0: y, v });
            }
            _ => {}
        }
        self.hint = self.hint_for(Some(&t));
        acts
    }

    /// Mouse movement during a locked drag (logical px since the last event).
    pub fn delta(&mut self, _dx: f32, dy: f32, m: Mods) -> Vec<Act> {
        let Some(Drag::Knob { id, v }) = self.drag.clone() else { return vec![] };
        let f = if m.ctrl { FINE } else { 1.0 };
        let v = (v - dy * f / KNOB_PX).clamp(0.0, 1.0);
        self.vals.set(&id, v);
        self.drag = Some(Drag::Knob { id, v });
        self.hint = self.hint_for(self.view.pressed.as_ref());
        vec![Act::Repaint]
    }

    pub fn up(&mut self, _btn: Btn) -> Vec<Act> {
        let Some(d) = self.drag.take() else { return vec![] };
        match d {
            Drag::Knob { id, .. } => self.vals.end(&id),
            Drag::EnvPoint { i, .. } => {
                let p = view::ENV_PREFIX[usize::from(self.view.sub.min(4))];
                for id in env_point_ids(p, i) {
                    self.vals.end(&id);
                }
            }
        }
        self.view.pressed = None;
        vec![Act::Repaint]
    }

    pub fn moved(&mut self, x: f32, y: f32, _m: Mods) -> Vec<Act> {
        self.view.mouse = Some((x, y));
        match self.drag.clone() {
            Some(Drag::EnvPoint { i, x0, y0, v }) => {
                let p = view::ENV_PREFIX[usize::from(self.view.sub.min(4))];
                let ids = env_point_ids(p, i);
                let eg = view::env_geom(&self.vals, p, view::env_rect());
                // The decay node moves (1 - sustain) of the decay knob's run.
                let run = if i == 3 { eg.px * (1.0 - eg.sus).max(0.05) } else { eg.px };
                self.vals.set(&ids[0], v[0] + (x - x0) / run);
                if i == 3 {
                    // Sustain node: vertical = sustain level over the drawn level range.
                    let (top, bot) = view::env_levels(view::env_rect());
                    let h = (bot - top) * if eg.amp > 0.05 { eg.amp } else { 1.0 };
                    self.vals.set(&ids[1], v[1] + (y0 - y) / h);
                }
                self.hint = format!("{}: {}", self.name(&ids[0]), self.vals.text(&ids[0]));
                vec![Act::Repaint]
            }
            Some(Drag::Knob { .. }) => vec![Act::Repaint],
            None => {
                let t = self.hit(x, y);
                self.hint = self.hint_for(t.as_ref());
                vec![Act::Repaint]
            }
        }
    }

    pub fn leave(&mut self) -> Vec<Act> {
        self.view.mouse = None;
        self.hint.clear();
        vec![Act::Repaint]
    }

    /// Wheel: one step per notch (1/128 of the range for times and speeds), spec §5.
    pub fn wheel(&mut self, x: f32, y: f32, notches: f32, _m: Mods) -> Vec<Act> {
        let Some(t) = self.hit(x, y) else { return vec![] };
        if !view::works(&t) {
            self.hint = self.hint_for(Some(&t));
            return vec![Act::Repaint];
        }
        match &t {
            Target::Knob(id) | Target::List(id) | Target::Set(id, _) => {
                let (lo, hi) = self.range(id);
                // The wheel stops short of the Custom shape (greyed out).
                let hi = if view::is_osc_shape(id) { hi.min(i32::from(crate::dsp::NOISE)) } else { hi };
                let steps = (hi - lo) as f32;
                let step = if steps > WHEEL_STEPS { (steps / WHEEL_STEPS).round() } else { 1.0 };
                let plain = (self.vals.plain(id) + notches.signum() * step).clamp(lo as f32, hi as f32);
                self.vals.edit_plain(id, plain);
            }
            _ => return vec![],
        }
        self.hint = self.hint_for(Some(&t));
        vec![Act::Repaint]
    }

    pub fn menu(&self, req: &MenuReq) -> MenuDef<Cmd> {
        match req {
            MenuReq::Param(id) => {
                let mut m = MenuDef::new(MenuStyle::Context);
                m.head(&self.name(id), Cmd::None);
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
            MenuReq::List(id) => {
                let mut m = MenuDef::new(MenuStyle::List);
                let cur = self.vals.plain(id).round() as i32;
                for (i, n) in FILTERS.iter().enumerate() {
                    let r = m.add(MenuItem::item(n), Cmd::Choice(id.clone(), i as i32));
                    if i as i32 == cur {
                        m.current = Some(r);
                    }
                }
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
            Cmd::Choice(id, v) => self.vals.edit_plain(&id, v as f32),
            Cmd::Automate(id) => return self.param_index(&id).map(Act::Automate).into_iter().collect(),
        }
        vec![Act::Repaint]
    }

    /// Position of `id` in the parameter list (= the original's parameter index).
    pub fn param_index(&self, id: &str) -> Option<usize> {
        self.params.defs.iter().position(|d| d.id == id)
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
    use crate::params::Osc3Params;
    use crate::view::{self, ViewState};
    use crate::Shared;

    const TIMER: usize = 1;
    const WM_CHANGED: u32 = WM_APP + 1;

    struct Win {
        hwnd: HWND,
        canvas: Canvas,
        buf: Vec<u32>,
        crop: Vec<u32>,
        scale: f32,
        /// Painted frame size in device px.
        size: (u32, u32),
        params: Arc<Osc3Params>,
        logic: Logic,
        palette: Palette,
        last_sig: u64,
        last_raw: u64,
        last_view: ViewState,
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

    pub fn post_changed(hwnd: isize) {
        if hwnd != 0 {
            unsafe { PostMessageW(hwnd as HWND, WM_CHANGED, 0, 0) };
        }
    }

    fn mods() -> Mods {
        let (ctrl, shift, alt) = pw::mods();
        Mods { ctrl, shift, alt }
    }

    pub fn open(parent: *mut c_void, scale: f32, params: Arc<Osc3Params>, shared: Arc<Shared>, ctx: Arc<dyn GuiContext>, slot: Arc<AtomicIsize>) -> Handle {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let px = |v: u32| (v as f32 * scale).round() as u32;
        let full = (px(view::W), px(view::H));
        unsafe {
            let class = pw::register("Lemon3xOscView", Some(wndproc));
            let win = Box::new(Win {
                hwnd: std::ptr::null_mut(),
                canvas: Canvas::new(full.0, full.1, scale).expect("canvas"),
                buf: vec![0; (full.0 * full.1) as usize],
                crop: Vec::new(),
                scale,
                size: full,
                logic: Logic::new(params.clone(), shared, Some(ctx)),
                params,
                palette: os_plugkit::palette(),
                last_sig: 0,
                last_raw: u64::MAX,
                last_view: ViewState::default(),
                last_hint: String::new(),
                lock: CursorLock::default(),
                tracking: false,
                slot: slot.clone(),
            });
            let title = pw::wide("Lemon Trio");
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
                0,
                0,
                px(view::EDITOR_W) as i32,
                px(view::EDITOR_H) as i32,
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
                    let (x, y) = w.to_view((sx - r.left) as f32, (sy - r.top) as f32);
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
        /// Top of the shown crop in the painted frame: the strip row is left out when the host draws it.
        fn crop_y(&self) -> u32 {
            if self.logic.hosted { view::HOSTED_CROP_Y } else { view::CROP_Y }
        }

        /// Editor content size in logical px.
        fn content(&self) -> (u32, u32) {
            (view::EDITOR_W, if self.logic.hosted { view::HOSTED_EDITOR_H } else { view::EDITOR_H })
        }

        /// Window-client device px to painted-frame coordinates (the editor is a crop of the painted frame).
        fn to_view(&self, x: f32, y: f32) -> (f32, f32) {
            (x / self.scale + view::CROP_X as f32, y / self.scale + self.crop_y() as f32)
        }

        fn mouse(&self, l: LPARAM) -> (f32, f32) {
            let (x, y) = pw::xy(l);
            self.to_view(x as f32, y as f32)
        }

        /// Screen position of a painted-frame point.
        fn screen(&self, x: f32, y: f32) -> (i32, i32) {
            let cx = ((x - view::CROP_X as f32) * self.scale) as i32;
            let cy = ((y - self.crop_y() as f32) * self.scale) as i32;
            pw::to_screen(self.hwnd, cx, cy)
        }

        fn invalidate(&self) {
            unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) };
        }

        /// Timer / parameter change: expire confirmed overlay values, repaint when anything shown changed.
        fn poll(&mut self) {
            let dirty = self.logic.vals.expire();
            let sig = self.logic.vals.signature();
            let rv = self.params.raw_version.load(Ordering::Acquire);
            if dirty || sig != self.last_sig || rv != self.last_raw || self.logic.view != self.last_view || self.logic.hint != self.last_hint {
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

        /// Answers the host (spec `plugins/host-link.md`): two pages (oscillators, envelopes), the hint line.
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
                    0 => PageIcon::Plugin as LRESULT,
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
            let (hdc, ps) = pw::begin_paint(self.hwnd);
            let hint = if self.logic.hosted { String::new() } else { self.logic.hint.clone() };
            self.logic.hits = view::paint(&mut self.canvas, &self.palette, &self.logic.vals, &self.logic.view, &hint, self.logic.hosted);
            self.last_sig = self.logic.vals.signature();
            self.last_raw = self.params.raw_version.load(Ordering::Acquire);
            self.last_view = self.logic.view.clone();
            // Show the editor area: the painted frame minus the original's border (and the strip row when hosted).
            let (ox, oy) = ((view::CROP_X as f32 * self.scale).round() as u32, (self.crop_y() as f32 * self.scale).round() as u32);
            let (cw, ch) = self.content();
            let w = ((cw as f32 * self.scale).round() as u32).min(self.size.0 - ox);
            let h = ((ch as f32 * self.scale).round() as u32).min(self.size.1 - oy);
            let full = self.size.0 as usize;
            self.buf.resize(full * self.size.1 as usize, 0);
            self.canvas.copy_to_xrgb(&mut self.buf);
            self.crop.clear();
            for row in oy as usize..(oy + h) as usize {
                let at = row * full + ox as usize;
                self.crop.extend_from_slice(&self.buf[at..at + w as usize]);
            }
            pw::blit(hdc, &self.crop, w, h);
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
    use crate::params::Osc3Params;
    use crate::view::{self, ViewState};
    use crate::Shared;

    /// The editor on its GUI thread.
    struct Ed {
        win: Window,
        canvas: Canvas,
        buf: Vec<u32>,
        crop: Vec<u32>,
        scale: f32,
        /// Painted frame size in device px.
        size: (u32, u32),
        params: Arc<Osc3Params>,
        logic: Logic,
        palette: Palette,
        last_sig: u64,
        last_raw: u64,
        last_view: ViewState,
        last_hint: String,
        dirty: bool,
        /// The host link: the host finds the editor by the window it gave it (spec: plugins/host-link.md).
        link: Arc<Peer>,
        /// The page as last agreed with the host.
        last_page: usize,
    }

    pub fn open(parent: px::Parent, scale: f32, params: Arc<Osc3Params>, shared: Arc<Shared>, ctx: Arc<dyn GuiContext>, slot: &px::Slot) -> Option<px::EditorThread> {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let px = move |v: u32| (v as f32 * scale).round() as u32;
        // Two pages (oscillators, envelopes); joined, the editor is what lies below the strip row.
        let wake = slot.clone();
        let link = Peer::register(parent as u64, vec![PageIcon::Plugin, PageIcon::Envelope], (view::EDITOR_W, view::HOSTED_EDITOR_H), move || px::notify(&wake));
        let peer = link.clone();
        let thread = px::spawn(parent, (px(view::EDITOR_W), px(view::EDITOR_H)), slot, move |win| {
            let full = (px(view::W), px(view::H));
            let Ok(canvas) = Canvas::new(full.0, full.1, scale) else { return };
            let logic = Logic::new(params.clone(), shared, Some(ctx));
            let ed = Ed {
                win,
                canvas,
                buf: Vec::new(),
                crop: Vec::new(),
                scale,
                size: full,
                params,
                logic,
                palette: os_plugkit::palette(),
                last_sig: 0,
                last_raw: u64::MAX,
                last_view: ViewState::default(),
                last_hint: String::new(),
                dirty: true,
                link: peer,
                last_page: 0,
            };
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
                        let (x, y) = self.to_view(x, y);
                        let acts = self.logic.wheel(x, y, notches, self.mods());
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
            let cx = ((x - view::CROP_X as f32) * self.scale) as i32;
            let cy = ((y - self.crop_y() as f32) * self.scale) as i32;
            self.win.to_root(cx, cy)
        }

        /// Timer / parameter change: expire confirmed overlay values, repaint when anything shown changed.
        fn poll(&mut self) {
            let dirty = self.logic.vals.expire();
            let sig = self.logic.vals.signature();
            let rv = self.params.raw_version.load(Ordering::Acquire);
            if dirty || sig != self.last_sig || rv != self.last_raw || self.logic.view != self.last_view || self.logic.hint != self.last_hint {
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
            let hint = if self.logic.hosted { String::new() } else { self.logic.hint.clone() };
            self.logic.hits = view::paint(&mut self.canvas, &self.palette, &self.logic.vals, &self.logic.view, &hint, self.logic.hosted);
            self.last_sig = self.logic.vals.signature();
            self.last_raw = self.params.raw_version.load(Ordering::Acquire);
            self.last_view = self.logic.view.clone();
            // Show the editor area: the painted frame minus the original's border (and the strip row when hosted).
            let (ox, oy) = ((view::CROP_X as f32 * self.scale).round() as u32, (self.crop_y() as f32 * self.scale).round() as u32);
            let (cw, ch) = self.content();
            let w = ((cw as f32 * self.scale).round() as u32).min(self.size.0 - ox);
            let h = ((ch as f32 * self.scale).round() as u32).min(self.size.1 - oy);
            let full = self.size.0 as usize;
            self.buf.resize(full * self.size.1 as usize, 0);
            self.canvas.copy_to_xrgb(&mut self.buf);
            self.crop.clear();
            for row in oy as usize..(oy + h) as usize {
                let at = row * full + ox as usize;
                self.crop.extend_from_slice(&self.buf[at..at + w as usize]);
            }
            self.win.blit(&self.crop, w, h);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{osc, rec, COARSE, R_ATT, R_SUS, SHAPE, T_VOL};

    fn logic() -> Logic {
        let mut l = Logic::new(Arc::new(Osc3Params::default()), Arc::new(Shared::default()), None);
        paint(&mut l);
        l
    }

    fn paint(l: &mut Logic) {
        let mut c = os_ui::Canvas::new(view::W, view::H, 1.0).unwrap();
        l.hits = view::paint(&mut c, &os_plugkit::palette(), &l.vals, &l.view, "", false);
    }

    fn centre(l: &Logic, t: &Target) -> (f32, f32) {
        let h = l.hits.iter().rev().find(|h| &h.t == t).unwrap_or_else(|| panic!("no hit {t:?}"));
        (h.r.cx(), h.r.cy())
    }

    #[test]
    fn knob_drags_wheel_and_reset() {
        let mut l = logic();
        let t = Target::Knob("o2_coarse".into());
        let (x, y) = centre(&l, &t);
        assert_eq!(l.params.v(osc(1) + COARSE), -12);
        let acts = l.down(x, y, Btn::Left, Mods::default(), false);
        assert!(acts.contains(&Act::Lock));
        // 200 px = the whole range (48 semitones): 25 px up = +6, in small moves (steps must not swallow them).
        for _ in 0..25 {
            l.delta(0.0, -1.0, Mods::default());
        }
        assert_eq!(l.vals.plain("o2_coarse"), -6.0);
        // Ctrl: a quarter of the speed.
        l.delta(0.0, -100.0, Mods { ctrl: true, ..Mods::default() });
        assert_eq!(l.vals.plain("o2_coarse"), 0.0);
        l.up(Btn::Left);
        assert!(!l.dragging());
        // The sound follows the editor at once.
        assert_eq!(l.params.v(osc(1) + COARSE), 0);
        // Wheel: one semitone per notch; Alt+click resets.
        l.wheel(x, y, 1.0, Mods::default());
        assert_eq!(l.vals.plain("o2_coarse"), 1.0);
        l.down(x, y, Btn::Left, Mods { alt: true, ..Mods::default() }, false);
        assert_eq!(l.vals.plain("o2_coarse"), -12.0);
        // Middle click resets too; right click opens the parameter menu.
        let acts = l.down(x, y, Btn::Right, Mods::default(), false);
        assert!(matches!(acts.last(), Some(Act::Menu(MenuReq::Param(id), _, _)) if id == "o2_coarse"));
        let m = l.menu(&MenuReq::Param("o2_coarse".into()));
        assert_eq!(m.find("Type in value..."), Some(Cmd::TypeIn("o2_coarse".into())));
        l.typed("o2_coarse", "+7");
        assert_eq!(l.vals.plain("o2_coarse"), 7.0);
        l.pick(&MenuReq::Param("o2_coarse".into()), Cmd::CopyValue("o2_coarse".into()), 0.0, 0.0);
        l.pick(&MenuReq::Param("o3_coarse".into()), Cmd::PasteValue("o3_coarse".into()), 0.0, 0.0);
        assert_eq!(l.vals.plain("o3_coarse"), 7.0);
        // "Create automation clip" asks the host (only Lemon Studio, via the host link) for parameter k's clip.
        let req = MenuReq::Param("o2_coarse".into());
        let entry = |l: &Logic| {
            let m = l.menu(&req);
            let i = m.items.iter().position(|it| matches!(it, MenuItem::Item { label, .. } if label == "Create automation clip")).unwrap();
            (matches!(&m.items[i], MenuItem::Item { enabled: true, .. }), m.cmds[i].clone())
        };
        assert!(!entry(&l).0);
        l.hosted = true;
        let (enabled, cmd) = entry(&l);
        assert!(enabled);
        assert_eq!(l.pick(&req, cmd, 0.0, 0.0), vec![Act::Automate(osc(1) + COARSE)]);
    }

    #[test]
    fn shapes_switches_and_lists() {
        let mut l = logic();
        let (x, y) = centre(&l, &Target::Set("o1_shape".into(), 3.0));
        l.down(x, y, Btn::Left, Mods::default(), false);
        l.up(Btn::Left);
        assert_eq!(l.params.v(osc(0) + SHAPE), 3);
        assert_eq!(l.hint, "Osc 1 shape");
        // Invert osc 2 and AM are bits of the switches parameter.
        let (x, y) = centre(&l, &Target::Flag(1));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert!(l.params.flag(1) && !l.params.flag(0));
        assert_eq!(l.hint, "Invert osc 2");
        let (x, y) = centre(&l, &Target::Flag(crate::params::AM_BYTE));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert!(l.params.flag(crate::params::AM_BYTE));
        assert_eq!(l.hint, "Oscillator 3 = amplitude modulation");
        // The saved state holds the switches as bytes.
        assert_eq!(crate::params::encode(crate::params::FLAGS, l.vals.plain("flags") as i32), 0x0100_0100);
        // HQ radio.
        let (x, y) = centre(&l, &Target::Toggle("hq".into()));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(l.vals.plain("hq"), 1.0);
        assert_eq!(l.hint, "High quality (anti aliased oscillators)");
        // Envelope page: the filter type list.
        l.view.tab = 1;
        paint(&mut l);
        let (x, y) = centre(&l, &Target::List("filter".into()));
        let acts = l.down(x, y, Btn::Left, Mods::default(), false);
        assert!(matches!(acts.last(), Some(Act::Menu(MenuReq::List(_), _, _))));
        let m = l.menu(&MenuReq::List("filter".into()));
        assert_eq!(m.current, Some(0));
        assert_eq!(m.find("SVF LP"), Some(Cmd::Choice("filter".into(), 6)));
        l.pick(&MenuReq::List("filter".into()), Cmd::Choice("filter".into(), 6), 0.0, 0.0);
        assert_eq!(l.params.v(crate::params::FILTER), 6);
    }

    #[test]
    fn hints_use_the_original_wording() {
        let mut l = logic();
        for (t, want) in [
            (Target::Knob("o1_phase".into()), "Osc 1 stereo phase offset: 0%"),
            (Target::Knob("o1_detune".into()), "Osc 1 stereo detune: +0 cent"),
            (Target::Knob("o1_coarse".into()), "Osc 1 coarse pitch: +0 semitone"),
            (Target::Knob("o1_fine".into()), "Osc 1 fine pitch: +0 cent"),
            (Target::Knob("o1_pan".into()), "Osc 1 panning: Centered"),
            (Target::Knob("o2_mix".into()), "Osc 2 mix level: 50%"),
            (Target::Knob("rand".into()), "Stereo phase randomness: 0%"),
            (Target::Flag(0), "Invert osc 1"),
        ] {
            let (x, y) = centre(&l, &t);
            l.moved(x, y, Mods::default());
            assert_eq!(l.hint, want);
        }
    }

    #[test]
    fn envelope_page_edits_the_selected_target() {
        let mut l = logic();
        l.view.tab = 1;
        paint(&mut l);
        // Volume is selected first; its envelope switch turns the envelope on.
        let (x, y) = centre(&l, &Target::Toggle("vol_env".into()));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(l.params.v(rec(T_VOL) + crate::params::R_ON), 1);
        // Dragging the attack node right lengthens the attack.
        let a0 = l.vals.norm("vol_att");
        let (x, y) = centre(&l, &Target::EnvPoint(1));
        l.down(x, y, Btn::Left, Mods::default(), false);
        l.moved(x + 9.0, y, Mods::default());
        l.up(Btn::Left);
        assert!((l.vals.norm("vol_att") - a0 - 0.1).abs() < 0.01, "{}", l.vals.norm("vol_att"));
        assert!(l.params.v(rec(T_VOL) + R_ATT) > 20000);
        // The sustain node moves the sustain level vertically.
        let s0 = l.vals.norm("vol_sus");
        let (x, y) = centre(&l, &Target::EnvPoint(3));
        l.down(x, y, Btn::Left, Mods::default(), false);
        l.moved(x, y - 20.0, Mods::default());
        l.up(Btn::Left);
        assert!(l.vals.norm("vol_sus") > s0);
        assert!(l.params.v(rec(T_VOL) + R_SUS) > 50);
        // Pitch sub-tab: its knobs and amount are hit regions.
        let (x, y) = centre(&l, &Target::SubTab(4));
        l.down(x, y, Btn::Left, Mods::default(), false);
        paint(&mut l);
        centre(&l, &Target::Knob("pitch_amt".into()));
        centre(&l, &Target::Set("pitch_lfo_shape".into(), 2.0));
        // Panning has no envelope switch (only its LFO).
        let (x, y) = centre(&l, &Target::SubTab(0));
        l.down(x, y, Btn::Left, Mods::default(), false);
        paint(&mut l);
        assert!(!l.hits.iter().any(|h| h.t == Target::Toggle("pan_env".into())));
        centre(&l, &Target::Knob("pan_lfo_amt".into()));
    }

    #[test]
    fn greyed_out_controls_only_say_so() {
        let mut l = logic();
        // The Custom shape plays silence: its button does not pick it.
        let s0 = l.vals.plain("o1_shape");
        let (x, y) = centre(&l, &Target::Set("o1_shape".into(), 6.0));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(l.vals.plain("o1_shape"), s0);
        assert_eq!(l.hint, "Osc 1 custom shape: not available yet");
        // The wheel over a shape stops short of it.
        let (x, y) = centre(&l, &Target::Set("o1_shape".into(), 0.0));
        for _ in 0..8 {
            l.wheel(x, y, 1.0, Mods::default());
        }
        assert_eq!(l.vals.plain("o1_shape") as u8, crate::dsp::NOISE);
        // The Panning LFO is not played: no drag, wheel, reset or menu.
        l.view.tab = 1;
        l.view.sub = 0;
        paint(&mut l);
        let (x, y) = centre(&l, &Target::Knob("pan_lfo_amt".into()));
        let v0 = l.vals.norm("pan_lfo_amt");
        for btn in [Btn::Left, Btn::Middle, Btn::Right] {
            assert_eq!(l.down(x, y, btn, Mods::default(), false), vec![Act::Repaint], "{btn:?}");
        }
        l.wheel(x, y, 1.0, Mods::default());
        assert_eq!(l.vals.norm("pan_lfo_amt"), v0);
        assert!(!l.dragging());
        assert!(l.hint.ends_with(": not available yet"), "{}", l.hint);
        // The other targets' LFOs play.
        assert!(view::works(&Target::Knob("vol_lfo_amt".into())));
    }

    #[test]
    fn hosted_editor_leaves_the_tabs_to_the_host() {
        let mut l = logic();
        assert!(l.hits.iter().any(|h| h.t == Target::Tab(1)));
        let mut c = os_ui::Canvas::new(view::W, view::H, 1.0).unwrap();
        l.hits = view::paint(&mut c, &os_plugkit::palette(), &l.vals, &l.view, "", true);
        assert!(!l.hits.iter().any(|h| matches!(h.t, Target::Tab(_))));
        // Every control lies inside the area shown when hosted.
        let top = view::HOSTED_CROP_Y as f32;
        assert!(l.hits.iter().all(|h| h.r.y >= top), "{:?}", l.hits.iter().find(|h| h.r.y < top));
    }
}
