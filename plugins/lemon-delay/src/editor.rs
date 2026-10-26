// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The plugin editor: a child of the host's window (Win32, X11 on Linux, an NSView on macOS) that paints `view` with
//! the os-ui canvas, and the editor logic (platform independent): knob, fader and slider drags with the cursor held in
//! place, the switches and selectors, wheel and reset gestures, the parameter menus. Edits show and sound at once
//! (`os_plugkit::vals`).
//!
// spec: plugins/lemon-delay.md (§2 controls, §3 hints, §4 interaction), plugins/host-link.md

use std::any::Any;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::Arc;

use nih_plug::prelude::*;
use os_engine::effects::delay as dsp;
use os_plugkit::vals::Vals;
use os_plugkit::MenuDef;
use os_ui::kit::menu::{MenuItem, MenuStyle};

use crate::params::{self, DelayParams, DEFS};
use crate::view::{self, Target, ViewState};
use crate::Shared;

pub struct DelayEditor {
    pub params: Arc<DelayParams>,
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

impl DelayEditor {
    pub fn new(params: Arc<DelayParams>, shared: Arc<Shared>) -> Self {
        DelayEditor {
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

impl Editor for DelayEditor {
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
// Laws (spec §4)

/// Pixels of drag for a knob's whole range.
pub const KNOB_PX: f32 = 200.0;
const FINE: f32 = 0.25;
/// Parameters with more steps than this move by 1/`WHEEL_STEPS` of their range per wheel notch.
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
    Automate(String),
    About,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuReq {
    Param(String),
    About,
}

// ---------------------------------------------------------------------------------------------------------------
// Editor logic

/// Something the window has to do after an event.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Repaint,
    /// Hide the cursor and hold it in place (knob, fader and slider drags).
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

/// A drag: `v` = the unrounded normalised value it has reached (steps would swallow small moves), `px` = pixels for
/// the whole range, `across` = a horizontal drag.
#[derive(Clone, Debug)]
struct Drag {
    id: String,
    v: f32,
    px: f32,
    across: bool,
}

/// Every parameter id in the host's order.
pub fn ids() -> impl Iterator<Item = &'static str> {
    DEFS.iter().map(|d| d.id)
}

pub struct Logic {
    pub vals: Vals,
    pub params: Arc<DelayParams>,
    pub shared: Arc<Shared>,
    pub view: ViewState,
    pub hits: Vec<view::Hit>,
    drag: Option<Drag>,
    /// Hint text.
    pub hint: String,
    copied: Option<f32>,
    pub hosted: bool,
}

impl Logic {
    pub fn new(params: Arc<DelayParams>, shared: Arc<Shared>, ctx: Option<Arc<dyn GuiContext>>) -> Self {
        let vals = Vals::new(&*params, params.live.clone(), ctx);
        let mut l = Logic { vals, params, shared, view: ViewState::default(), hits: Vec::new(), drag: None, hint: String::new(), copied: None, hosted: false };
        l.sync_shown();
        l
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    fn hit(&self, x: f32, y: f32) -> Option<(Target, os_ui::Rect)> {
        self.hits.iter().rev().find(|h| h.r.contains(x, y)).map(|h| (h.t.clone(), h.r))
    }

    /// The time's text follows the shown sync and the cutoff's the shown filter mode (also before the host confirms
    /// them).
    fn sync_shown(&mut self) {
        self.params.shown.sync.store(self.vals.plain("sync") > 0.5, Ordering::Relaxed);
        self.params.shown.filter.store(self.vals.plain("filter").round() as i32, Ordering::Relaxed);
    }

    fn hint_for(&mut self, t: Option<&Target>) -> String {
        self.sync_shown();
        t.map(|t| view::hint(t, &self.vals)).unwrap_or_default()
    }

    fn range(id: &str) -> (i32, i32) {
        params::index(id).map_or((0, 1), |k| dsp::RANGES[k])
    }

    fn edit(&mut self, id: &str, norm: f32) {
        self.vals.edit(id, norm);
        self.sync_shown();
    }

    /// A drag position `v` (normalised) moved onto its snap, if it has one (spec §4).
    fn snapped(&self, id: &str, v: f32) -> f32 {
        let Some(k) = params::index(id) else { return v };
        let (lo, hi) = Self::range(id);
        let plain = lo as f32 + v * (hi - lo) as f32;
        match params::snap(k, plain, self.vals.plain("sync") > 0.5) {
            Some(p) => self.vals.plain_to_norm(id, p as f32),
            None => v,
        }
    }

    fn reset(&mut self, id: &str) {
        let d = self.vals.default_norm(id);
        self.edit(id, d);
    }

    fn begin_drag(&mut self, id: &str, px: f32, across: bool, t: Target) -> Vec<Act> {
        self.vals.begin(id);
        self.drag = Some(Drag { id: id.into(), v: self.vals.norm(id), px, across });
        self.view.pressed = Some(t);
        vec![Act::Repaint, Act::Lock]
    }

    pub fn down(&mut self, x: f32, y: f32, btn: Btn, m: Mods, _dbl: bool) -> Vec<Act> {
        self.view.mouse = Some((x, y));
        let Some((t, _)) = self.hit(x, y) else { return vec![] };
        let id = t.id().to_string();
        let mut acts = vec![Act::Repaint];
        match (&t, btn) {
            (Target::Logo, Btn::Left | Btn::Right) => acts.push(Act::Menu(MenuReq::About, x, y)),
            (Target::Logo, _) => {}
            (_, Btn::Left) if m.alt => self.reset(&id),
            (_, Btn::Middle) => self.reset(&id),
            (_, Btn::Right) => acts.push(Act::Menu(MenuReq::Param(id), x, y)),
            (Target::Knob(_), Btn::Left) => acts = self.begin_drag(&id, KNOB_PX, false, t.clone()),
            (Target::Fader(_), Btn::Left) => acts = self.begin_drag(&id, view::FADER_BOT - view::FADER_TOP, false, t.clone()),
            (Target::Slider(_), Btn::Left) => acts = self.begin_drag(&id, view::SLIDER_X1 - view::SLIDER_X0, true, t.clone()),
            (Target::Switch(_), Btn::Left) => {
                let on = self.vals.plain(&id) > 0.5;
                self.edit(&id, if on { 0.0 } else { 1.0 });
            }
            (Target::Choice(_, v), Btn::Left) => {
                let n = self.vals.plain_to_norm(&id, *v as f32);
                self.edit(&id, n);
            }
        }
        self.hint = self.hint_for(Some(&t));
        acts
    }

    /// Mouse movement during a locked drag (logical px since the last event).
    pub fn delta(&mut self, dx: f32, dy: f32, m: Mods) -> Vec<Act> {
        let Some(Drag { id, v, px, across }) = self.drag.clone() else { return vec![] };
        let f = if m.ctrl { FINE } else { 1.0 };
        let moved = if across { dx } else { -dy };
        let v = (v + moved * f / px).clamp(0.0, 1.0);
        // The drag keeps its own position; the setting lands on a snap (Shift drags freely).
        let shown = if m.shift { v } else { self.snapped(&id, v) };
        self.vals.set(&id, shown);
        self.drag = Some(Drag { id, v, px, across });
        let t = self.view.pressed.clone();
        self.hint = self.hint_for(t.as_ref());
        vec![Act::Repaint]
    }

    pub fn up(&mut self, _btn: Btn) -> Vec<Act> {
        let Some(d) = self.drag.take() else { return vec![] };
        self.vals.end(&d.id);
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

    /// Wheel: one step per notch (1/128 of the range for the settings with many steps).
    pub fn wheel(&mut self, x: f32, y: f32, notches: f32, m: Mods) -> Vec<Act> {
        let Some((t, _)) = self.hit(x, y).filter(|(t, _)| *t != Target::Logo) else { return vec![] };
        let id = t.id().to_string();
        let (lo, hi) = Self::range(&id);
        let steps = (hi - lo) as f32;
        let step = if steps > WHEEL_STEPS { (steps / WHEEL_STEPS).round() } else { 1.0 };
        let cur = self.vals.plain(&id);
        let mut plain = (cur + notches.signum() * step).clamp(lo as f32, hi as f32);
        if let (Some(k), false) = (params::index(&id), m.shift) {
            let sync = self.vals.plain("sync") > 0.5;
            plain = params::snap_step(k, cur.round() as i32, plain.round() as i32, notches > 0.0, sync) as f32;
        }
        self.vals.edit_plain(&id, plain);
        self.sync_shown();
        self.hint = self.hint_for(Some(&t));
        vec![Act::Repaint]
    }

    pub fn menu(&self, req: &MenuReq) -> MenuDef<Cmd> {
        match req {
            MenuReq::Param(id) => {
                let mut m = MenuDef::new(MenuStyle::Context);
                m.head(&view::name(id, &self.vals), Cmd::None);
                m.add(MenuItem::item("Reset"), Cmd::Reset(id.clone()));
                m.sep(Cmd::None);
                m.add(MenuItem::item("Copy value"), Cmd::CopyValue(id.clone()));
                m.add(MenuItem::item("Paste value").enabled(self.copied.is_some()), Cmd::PasteValue(id.clone()));
                m.add(MenuItem::item("Type in value..."), Cmd::TypeIn(id.clone()));
                // Automation belongs to the host: offered when Lemon Studio hosts the editor (host link).
                m.sep(Cmd::None);
                m.add(MenuItem::item("Create automation clip").enabled(self.hosted), Cmd::Automate(id.clone()));
                m.sep(Cmd::None);
                m.add(MenuItem::item("About Lemon Delay..."), Cmd::About);
                m
            }
            MenuReq::About => {
                let mut m = MenuDef::new(MenuStyle::Context);
                m.head("Lemon Delay", Cmd::None);
                m.add(MenuItem::item(&format!("Version {}", env!("CARGO_PKG_VERSION"))).enabled(false), Cmd::None);
                m.add(MenuItem::item("Lemon Studio's recreation of Fruity Delay 3").enabled(false), Cmd::None);
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
                    self.edit(&id, v);
                }
            }
            Cmd::TypeIn(id) => return vec![Act::TypeIn(id, x, y)],
            Cmd::Automate(id) => return ids().position(|i| i == id).map(Act::Automate).into_iter().collect(),
            Cmd::About => return vec![Act::Menu(MenuReq::About, x, y)],
        }
        vec![Act::Repaint]
    }

    /// The value text a type-in box starts with.
    pub fn value_text(&self, id: &str) -> String {
        self.vals.text(id)
    }

    /// Applies a typed value.
    pub fn typed(&mut self, id: &str, text: &str) {
        self.sync_shown();
        if let Some(n) = self.vals.parse(id, text) {
            self.edit(id, n);
        }
    }

    /// Paints the editor; `hint` is drawn when the host does not show it.
    pub fn paint(&mut self, c: &mut os_ui::Canvas, palette: &os_theme::Palette) {
        self.sync_shown();
        let hint = if self.hosted { String::new() } else { self.hint.clone() };
        self.hits = view::paint(c, palette, &self.vals, &self.view, &hint);
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
    use crate::params::DelayParams;
    use crate::view;
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
        logic: Logic,
        palette: Palette,
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

    pub fn open(parent: *mut c_void, scale: f32, params: Arc<DelayParams>, shared: Arc<Shared>, ctx: Arc<dyn GuiContext>, slot: Arc<AtomicIsize>) -> Handle {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let px = |v: u32| (v as f32 * scale).round() as u32;
        let size = (px(view::W), px(view::H));
        shared.editor_open.store(true, Ordering::Relaxed);
        unsafe {
            let class = pw::register("LemonDelayView", Some(wndproc));
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
            });
            let title = pw::wide("Lemon Delay");
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
            // About 33 checks a second (edits the host confirms, the hint).
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

        /// Timer / parameter change: expire confirmed overlay values and repaint.
        fn poll(&mut self) {
            self.logic.vals.expire();
            self.invalidate();
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
            self.logic.paint(&mut self.canvas, &self.palette);
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
    use crate::params::DelayParams;
    use crate::view;
    use crate::Shared;

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

    pub fn open(parent: px::Parent, scale: f32, params: Arc<DelayParams>, shared: Arc<Shared>, ctx: Arc<dyn GuiContext>, slot: &px::Slot) -> Handle {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let size = ((view::W as f32 * scale).round() as u32, (view::H as f32 * scale).round() as u32);
        let sh = shared.clone();
        // One page; joined, the editor keeps its size (it has no strip row of its own).
        let wake = slot.clone();
        let link = Peer::register(parent as u64, vec![PageIcon::Plugin], (view::W, view::H), move || px::notify(&wake));
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

        /// Timer / parameter change: expire confirmed overlay values and repaint.
        fn poll(&mut self) {
            self.logic.vals.expire();
            self.dirty = true;
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
            self.logic.paint(&mut self.canvas, &self.palette);
            self.buf.resize((self.size.0 * self.size.1) as usize, 0);
            self.canvas.copy_to_xrgb(&mut self.buf);
            self.win.blit(&self.buf, self.size.0, self.size.1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use os_engine::effects::delay::{DEFAULTS, FEEDBACK, FILTER, INPUT, PARAMS, SMOOTH, SYNC, TYPE};

    fn logic() -> Logic {
        let mut l = Logic::new(Arc::new(DelayParams::default()), Arc::new(Shared::default()), None);
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
    fn every_parameter_has_a_control_and_controls_do_not_overlap() {
        let l = logic();
        for d in DEFS.iter() {
            assert!(l.hits.iter().any(|h| h.t.id() == d.id), "{}", d.id);
        }
        assert_eq!(ids().count(), PARAMS);
        for (i, a) in l.hits.iter().enumerate() {
            assert!(a.r.x >= 0.0 && a.r.y >= 0.0 && a.r.x + a.r.w <= view::W as f32 && a.r.y + a.r.h <= view::H as f32, "{:?}", a.t);
            for b in &l.hits[i + 1..] {
                let apart = a.r.x + a.r.w <= b.r.x || b.r.x + b.r.w <= a.r.x || a.r.y + a.r.h <= b.r.y || b.r.y + b.r.h <= a.r.y;
                assert!(apart, "{:?} overlaps {:?}", a.t, b.t);
            }
        }
    }

    #[test]
    fn knob_drags_wheel_and_reset() {
        let mut l = logic();
        let t = Target::Knob("fb".into());
        let (x, y) = centre(&l, &t);
        let acts = l.down(x, y, Btn::Left, Mods::default(), false);
        assert!(acts.contains(&Act::Lock));
        // 200 px = the whole range (0..6000): 40 px up = +1200.
        for _ in 0..40 {
            l.delta(0.0, -1.0, Mods::default());
        }
        l.up(Btn::Left);
        assert_eq!(l.params.v(FEEDBACK), 4200);
        assert_eq!(l.hint, "Feedback level: 87.5%");
        l.wheel(x, y, 1.0, Mods::default());
        assert_eq!(l.vals.plain("fb"), 4247.0);
        l.down(x, y, Btn::Left, Mods { alt: true, ..Mods::default() }, false);
        assert_eq!(l.vals.plain("fb"), DEFAULTS[FEEDBACK] as f32);
        let acts = l.down(x, y, Btn::Right, Mods::default(), false);
        assert!(matches!(acts.last(), Some(Act::Menu(MenuReq::Param(id), _, _)) if id == "fb"));
        // From 50 % (2400), 78 px up is 4740: within the magnet of 100 % (4800); Shift lets the drag sit there.
        l.vals.edit_plain("fb", 2400.0);
        l.down(x, y, Btn::Left, Mods::default(), false);
        for _ in 0..78 {
            l.delta(0.0, -1.0, Mods::default());
        }
        assert_eq!(l.vals.plain("fb"), 4800.0);
        l.delta(0.0, 0.0, Mods { shift: true, ..Mods::default() });
        assert_eq!(l.vals.plain("fb"), 4740.0);
        l.up(Btn::Left);
    }

    #[test]
    fn fader_and_slider_move_over_their_travel() {
        let mut l = logic();
        let (x, y) = centre(&l, &Target::Fader("in".into()));
        l.down(x, y, Btn::Left, Mods::default(), false);
        l.delta(0.0, view::FADER_BOT - view::FADER_TOP, Mods::default());
        l.up(Btn::Left);
        assert_eq!(l.params.v(INPUT), 0);
        assert_eq!(l.hint, "Input wet: 0.0%");
        let (x, y) = centre(&l, &Target::Slider("smooth".into()));
        l.down(x, y, Btn::Left, Mods::default(), false);
        l.delta(view::SLIDER_X1 - view::SLIDER_X0, 0.0, Mods::default());
        l.up(Btn::Left);
        assert_eq!(l.params.v(SMOOTH), 1000);
        assert_eq!(l.hint, "Time smooth: 100.0%");
    }

    #[test]
    fn selectors_and_switches_and_the_texts_that_follow_them() {
        let mut l = logic();
        let (x, y) = centre(&l, &Target::Choice("type".into(), 2));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(l.params.v(TYPE), 2);
        assert_eq!(l.hint, "Delay type: Ping Pong");
        paint(&mut l);
        let (ox, oy) = centre(&l, &Target::Knob("offset".into()));
        l.moved(ox, oy, Mods::default());
        assert_eq!(l.hint, "Delay time R/L pan: 0.0%");
        // FL's Default preset starts synced at 4:0; unsynced, the same setting reads in ms.
        let (tx, ty) = centre(&l, &Target::Knob("time".into()));
        l.moved(tx, ty, Mods::default());
        assert_eq!(l.hint, "Time: 4:0");
        let (x, y) = centre(&l, &Target::Switch("sync".into()));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(l.params.v(SYNC), 0);
        assert_eq!(l.hint, "Tempo sync: Off");
        l.moved(tx, ty, Mods::default());
        assert_eq!(l.hint, "Time: 26.2ms");
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(l.params.v(SYNC), 1);
        l.typed("time", "1:32");
        assert_eq!(l.vals.plain("time"), 80.0);
        // The cutoff's text follows the filter mode.
        l.vals.edit_plain("cutoff", 3000.0);
        let (x, y) = centre(&l, &Target::Choice("filter".into(), 1));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(l.params.v(FILTER), 1);
        let (cx, cy) = centre(&l, &Target::Knob("cutoff".into()));
        l.moved(cx, cy, Mods::default());
        assert_eq!(l.hint, "Feedback cutoff: 399.8Hz");
    }

    #[test]
    fn menus_offer_reset_type_in_and_about() {
        let mut l = logic();
        let m = l.menu(&MenuReq::Param("bits".into()));
        assert_eq!(m.find("Reset"), Some(Cmd::Reset("bits".into())));
        assert_eq!(m.find("Type in value..."), Some(Cmd::TypeIn("bits".into())));
        l.vals.edit_plain("bits", 100.0);
        l.pick(&MenuReq::Param("bits".into()), Cmd::Reset("bits".into()), 0.0, 0.0);
        assert_eq!(l.vals.plain("bits"), 6000.0);
        assert_eq!(l.pick(&MenuReq::Param("dry".into()), Cmd::Automate("dry".into()), 0.0, 0.0), vec![Act::Automate(25)]);
        assert!(matches!(l.pick(&MenuReq::Param("dry".into()), Cmd::About, 1.0, 2.0).last(), Some(Act::Menu(MenuReq::About, _, _))));
        let (x, y) = centre(&l, &Target::Logo);
        assert!(matches!(l.down(x, y, Btn::Left, Mods::default(), false).last(), Some(Act::Menu(MenuReq::About, _, _))));
        assert_eq!(l.hint, "Lemon Delay");
        assert!(l.wheel(x, y, 1.0, Mods::default()).is_empty());
    }

    /// `DELAY_PNG=<dir> cargo test -p lemon-delay -- --ignored`: the editor at a few settings, as PNGs.
    #[test]
    #[ignore]
    fn render_editor() {
        let Some(dir) = std::env::var_os("DELAY_PNG") else { return };
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
        l.hint = "Time: 4:0".into();
        save(&mut l, "delay_default", 1.0);
        save(&mut l, "delay_default_x2", 2.0);
        for (id, v) in [("type", 2), ("sync", 1), ("time", 191), ("mtime", 2000), ("mcut", 1500), ("dtype", 1), ("drive", 4000), ("knee", 3000), ("sym", 4500), ("filter", 2)] {
            l.vals.edit_plain(id, v as f32);
        }
        save(&mut l, "delay_ping_pong", 1.0);
        l.vals.edit_plain("type", 3.0);
        l.vals.edit_plain("filter", 3.0);
        save(&mut l, "delay_off", 1.0);
    }
}
