// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! A small effect's editor: a child of the host's window (Win32, X11 on Linux, an NSView on macOS) that paints `view`
//! with the os-ui canvas, and the editor logic (platform independent): knob drags with the cursor held in place, the
//! selectors, wheel and reset gestures, the parameter menus. Edits show and sound at once (`os_plugkit::vals`). Lemon
//! Delay's editor, with the effect's description (`Spec`) for its controls.
//!
// spec: plugins/lemon-small-effects.md (§2 the frame, §4 interaction), the plugin pages
// (plugins/lemon-soft-clipper.md, lemon-fast-distortion.md, lemon-balance.md, lemon-stereo-enhancer.md,
// lemon-transient-processor.md; each §2 controls, §4 magnets), plugins/host-link.md

use std::any::Any;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::Arc;

use nih_plug::prelude::*;
use os_plugkit::vals::Vals;
use os_plugkit::MenuDef;
use os_ui::kit::menu::{MenuItem, MenuStyle};

use crate::params::{self, FxParams};
use crate::view::{self, Target, ViewState};
use crate::{Shared, Spec};

pub struct FxEditor {
    pub spec: &'static Spec,
    pub params: Arc<FxParams>,
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

impl FxEditor {
    pub fn new(spec: &'static Spec, params: Arc<FxParams>, shared: Arc<Shared>) -> Self {
        FxEditor {
            spec,
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

impl Editor for FxEditor {
    fn spawn(&self, parent: ParentWindowHandle, context: Arc<dyn GuiContext>) -> Box<dyn Any + Send> {
        let scale = f32::from_bits(self.scale.load(Ordering::Relaxed));
        match parent {
            #[cfg(windows)]
            ParentWindowHandle::Win32Hwnd(hwnd) => {
                Box::new(win32::open(hwnd, scale, self.spec, self.params.clone(), self.shared.clone(), context, self.hwnd.clone()))
            }
            #[cfg(target_os = "linux")]
            ParentWindowHandle::X11Window(window) => {
                Box::new(gui::open(window, scale, self.spec, self.params.clone(), self.shared.clone(), context, &self.slot))
            }
            // macOS hosts set no scale (sizes are points): the editor paints at its window's backing scale.
            #[cfg(target_os = "macos")]
            ParentWindowHandle::AppKitNsView(view) => {
                let scale = os_plugkit::gui::backing_scale(view as usize);
                Box::new(gui::open(view as usize, scale, self.spec, self.params.clone(), self.shared.clone(), context, &self.slot))
            }
            // Elsewhere no editor opens yet.
            _ => {
                let _ = (context, scale);
                Box::new(())
            }
        }
    }

    fn size(&self) -> (u32, u32) {
        self.spec.size
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
// Laws (spec §4, as Lemon Delay's)

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
    /// Hide the cursor and hold it in place (knob drags).
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

/// A drag: `v` = the unrounded normalised value it has reached (steps would swallow small moves), over `px` for the
/// whole range.
#[derive(Clone, Debug)]
struct Drag {
    id: String,
    v: f32,
    px: f32,
}

pub struct Logic {
    pub spec: &'static Spec,
    pub vals: Vals,
    pub params: Arc<FxParams>,
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
    pub fn new(spec: &'static Spec, params: Arc<FxParams>, shared: Arc<Shared>, ctx: Option<Arc<dyn GuiContext>>) -> Self {
        let vals = Vals::new(&*params, params.live.clone(), ctx);
        Logic { spec, vals, params, shared, view: ViewState::default(), hits: Vec::new(), drag: None, hint: String::new(), copied: None, hosted: false }
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    fn hit(&self, x: f32, y: f32) -> Option<(Target, os_ui::Rect)> {
        self.hits.iter().rev().find(|h| h.r.contains(x, y)).map(|h| (h.t.clone(), h.r))
    }

    fn hint_for(&self, t: Option<&Target>) -> String {
        t.map(|t| view::hint(self.spec, t, &self.vals)).unwrap_or_default()
    }

    fn range(&self, id: &str) -> (i32, i32) {
        self.spec.index(id).map_or((0, 1), |k| self.spec.settings[k].range)
    }

    fn edit(&mut self, id: &str, norm: f32) {
        self.vals.edit(id, norm);
    }

    /// A drag position `v` (normalised) moved onto its magnet, if it has one (spec §4).
    fn snapped(&self, id: &str, v: f32) -> f32 {
        let Some(k) = self.spec.index(id) else { return v };
        let (lo, hi) = self.range(id);
        let plain = lo as f32 + v * (hi - lo) as f32;
        match params::snap(self.spec, k, plain) {
            Some(p) => self.vals.plain_to_norm(id, p as f32),
            None => v,
        }
    }

    fn reset(&mut self, id: &str) {
        let d = self.vals.default_norm(id);
        self.edit(id, d);
    }

    fn begin_drag(&mut self, id: &str, px: f32, t: Target) -> Vec<Act> {
        self.vals.begin(id);
        self.drag = Some(Drag { id: id.into(), v: self.vals.norm(id), px });
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
            (Target::Knob(_), Btn::Left) => acts = self.begin_drag(&id, KNOB_PX, t.clone()),
            (Target::Fader(_, px), Btn::Left) => acts = self.begin_drag(&id, *px, t.clone()),
            (Target::Choice(_, v), Btn::Left) => {
                let n = self.vals.plain_to_norm(&id, *v as f32);
                self.edit(&id, n);
            }
        }
        self.hint = self.hint_for(Some(&t));
        acts
    }

    /// Mouse movement during a locked drag (logical px since the last event).
    pub fn delta(&mut self, _dx: f32, dy: f32, m: Mods) -> Vec<Act> {
        let Some(Drag { id, v, px }) = self.drag.clone() else { return vec![] };
        let f = if m.ctrl { FINE } else { 1.0 };
        let v = (v - dy * f / px).clamp(0.0, 1.0);
        // The drag keeps its own position; the setting lands on a magnet (Shift drags freely).
        let shown = if m.shift { v } else { self.snapped(&id, v) };
        self.vals.set(&id, shown);
        self.drag = Some(Drag { id, v, px });
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
        let (lo, hi) = self.range(&id);
        let steps = (hi - lo) as f32;
        let step = if steps > WHEEL_STEPS { (steps / WHEEL_STEPS).round() } else { 1.0 };
        let cur = self.vals.plain(&id);
        let mut plain = (cur + notches.signum() * step).clamp(lo as f32, hi as f32);
        if let (Some(k), false) = (self.spec.index(&id), m.shift) {
            plain = params::snap_step(self.spec, k, cur.round() as i32, plain.round() as i32) as f32;
        }
        self.vals.edit_plain(&id, plain);
        self.hint = self.hint_for(Some(&t));
        vec![Act::Repaint]
    }

    pub fn menu(&self, req: &MenuReq) -> MenuDef<Cmd> {
        match req {
            MenuReq::Param(id) => {
                let mut m = MenuDef::new(MenuStyle::Context);
                m.head(&view::name(self.spec, id), Cmd::None);
                m.add(MenuItem::item("Reset"), Cmd::Reset(id.clone()));
                m.sep(Cmd::None);
                m.add(MenuItem::item("Copy value"), Cmd::CopyValue(id.clone()));
                m.add(MenuItem::item("Paste value").enabled(self.copied.is_some()), Cmd::PasteValue(id.clone()));
                m.add(MenuItem::item("Type in value..."), Cmd::TypeIn(id.clone()));
                // Automation belongs to the host: offered when Lemon Studio hosts the editor (host link).
                m.sep(Cmd::None);
                m.add(MenuItem::item("Create automation clip").enabled(self.hosted), Cmd::Automate(id.clone()));
                m.sep(Cmd::None);
                m.add(MenuItem::item(&format!("About {}...", self.spec.name)), Cmd::About);
                m
            }
            MenuReq::About => {
                let mut m = MenuDef::new(MenuStyle::Context);
                m.head(self.spec.name, Cmd::None);
                m.add(MenuItem::item(&format!("Version {}", env!("CARGO_PKG_VERSION"))).enabled(false), Cmd::None);
                m.add(MenuItem::item(self.spec.about).enabled(false), Cmd::None);
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
            Cmd::Automate(id) => return self.spec.index(&id).map(Act::Automate).into_iter().collect(),
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
        if let Some(n) = self.vals.parse(id, text) {
            self.edit(id, n);
        }
    }

    /// Paints the editor; `hint` is drawn when the host does not show it.
    pub fn paint(&mut self, c: &mut os_ui::Canvas, palette: &os_theme::Palette) {
        let hint = if self.hosted { String::new() } else { self.hint.clone() };
        self.hits = view::paint(self.spec, c, palette, &self.vals, &self.view, &hint);
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
    use crate::params::FxParams;
    use crate::{Shared, Spec};

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

    pub fn open(
        parent: *mut c_void,
        scale: f32,
        spec: &'static Spec,
        params: Arc<FxParams>,
        shared: Arc<Shared>,
        ctx: Arc<dyn GuiContext>,
        slot: Arc<AtomicIsize>,
    ) -> Handle {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let px = |v: u32| (v as f32 * scale).round() as u32;
        let size = (px(spec.size.0), px(spec.size.1));
        shared.editor_open.store(true, Ordering::Relaxed);
        unsafe {
            // One window procedure for all the effects; `register` names the class after it, so each plugin module
            // has its own.
            let class = pw::register("LemonSmallFxView", Some(wndproc));
            let win = Box::new(Win {
                hwnd: std::ptr::null_mut(),
                canvas: Canvas::new(size.0, size.1, scale).expect("canvas"),
                buf: vec![0; (size.0 * size.1) as usize],
                scale,
                size,
                logic: Logic::new(spec, params, shared.clone(), Some(ctx)),
                palette: os_plugkit::palette(),
                last_hint: String::new(),
                lock: CursorLock::default(),
                tracking: false,
                slot: slot.clone(),
            });
            let title = pw::wide(spec.name);
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
                Cmd::Metrics => os_hostlink::pack_size(self.logic.spec.size.0, self.logic.spec.size.1),
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
    use crate::params::FxParams;
    use crate::{Shared, Spec};

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

    pub fn open(
        parent: px::Parent,
        scale: f32,
        spec: &'static Spec,
        params: Arc<FxParams>,
        shared: Arc<Shared>,
        ctx: Arc<dyn GuiContext>,
        slot: &px::Slot,
    ) -> Handle {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let size = ((spec.size.0 as f32 * scale).round() as u32, (spec.size.1 as f32 * scale).round() as u32);
        let sh = shared.clone();
        // One page; joined, the editor keeps its size (it has no strip row of its own).
        let wake = slot.clone();
        let link = Peer::register(parent as u64, vec![PageIcon::Plugin], spec.size, move || px::notify(&wake));
        let peer = link.clone();
        let thread = px::spawn(parent, size, slot, move |win| {
            let Ok(canvas) = Canvas::new(size.0, size.1, scale) else { return };
            let logic = Logic::new(spec, params, shared, Some(ctx));
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
    use crate::tests::SPECS;
    use crate::{balance, chorus, fast_distortion, soft_clipper, stereo_enhancer, stereo_shaper, transient_processor, Control};

    fn logic(spec: &'static Spec) -> Logic {
        let mut l = Logic::new(spec, Arc::new(FxParams::new(spec)), Arc::new(Shared::default()), None);
        paint(&mut l);
        l
    }

    fn paint(l: &mut Logic) {
        let mut c = os_ui::Canvas::new(l.spec.size.0, l.spec.size.1, 1.0).unwrap();
        l.paint(&mut c, &os_plugkit::palette());
    }

    fn centre(l: &Logic, t: &Target) -> (f32, f32) {
        let h = l.hits.iter().rev().find(|h| &h.t == t).unwrap_or_else(|| panic!("no hit {t:?}"));
        (h.r.cx(), h.r.cy())
    }

    #[test]
    fn every_setting_has_a_control_and_controls_do_not_overlap() {
        for spec in SPECS {
            let l = logic(spec);
            let (w, h) = (spec.size.0 as f32, spec.size.1 as f32);
            for s in spec.settings {
                assert!(l.hits.iter().any(|h| h.t.id() == s.id), "{} {}", spec.name, s.id);
            }
            for (i, a) in l.hits.iter().enumerate() {
                assert!(a.r.x >= 0.0 && a.r.y >= 0.0 && a.r.x + a.r.w <= w && a.r.y + a.r.h <= h, "{} {:?}", spec.name, a.t);
                for b in &l.hits[i + 1..] {
                    let apart = a.r.x + a.r.w <= b.r.x || b.r.x + b.r.w <= a.r.x || a.r.y + a.r.h <= b.r.y || b.r.y + b.r.h <= a.r.y;
                    assert!(apart, "{}: {:?} overlaps {:?}", spec.name, a.t, b.t);
                }
            }
            // Controls sit inside a card (a tile is its own); the display too. Cards, the hint well and the wordmark
            // stay apart.
            let cards: Vec<os_ui::Rect> = spec.cards.iter().map(|k| os_ui::Rect::span(k.x0, k.x1, k.y0, k.y1)).collect();
            let within = |r: os_ui::Rect, k: &os_ui::Rect| r.x >= k.x && r.right() <= k.right() && r.y >= k.y && r.bottom() <= k.bottom();
            for c in spec.controls {
                if matches!(c, Control::Tile { .. }) {
                    continue;
                }
                for r in l.hits.iter().filter(|h| h.t.id() == c.id()).map(|h| h.r) {
                    assert!(cards.iter().any(|k| within(r, k)), "{} {} outside its card", spec.name, c.id());
                }
            }
            if let Some(d) = &spec.display {
                assert!(cards.iter().any(|k| within(d.r, k)), "{} display", spec.name);
            }
            let logo = l.hits.iter().find(|h| h.t == Target::Logo).unwrap().r;
            let mut boxes = cards.clone();
            boxes.extend(spec.hint);
            for (i, a) in boxes.iter().enumerate() {
                assert!(a.x >= 0.0 && a.y >= 0.0 && a.right() <= w && a.bottom() <= h, "{} {a:?}", spec.name);
                let apart = |b: &os_ui::Rect| a.right() <= b.x || b.right() <= a.x || a.bottom() <= b.y || b.bottom() <= a.y;
                assert!(boxes[i + 1..].iter().all(apart), "{}: {a:?} overlaps", spec.name);
                assert!(Some(*a) == spec.hint || apart(&logo), "{}: {a:?} under the wordmark", spec.name);
            }
        }
    }

    #[test]
    fn knob_drags_wheel_magnets_and_reset() {
        let mut l = logic(balance::SPEC);
        let t = Target::Knob("balance".into());
        let (x, y) = centre(&l, &t);
        let acts = l.down(x, y, Btn::Left, Mods::default(), false);
        assert!(acts.contains(&Act::Lock));
        // 200 px = the whole range (-128..128): 25 px down = -32.
        for _ in 0..25 {
            l.delta(0.0, 1.0, Mods::default());
        }
        l.up(Btn::Left);
        assert_eq!(l.params.v(0), -32);
        assert_eq!(l.hint, "Balance: 25.0% left");
        // Back up to within the magnet of the centre (3.84 of 256): lands on it; Shift lets the drag sit there.
        l.down(x, y, Btn::Left, Mods::default(), false);
        for _ in 0..23 {
            l.delta(0.0, -1.0, Mods::default());
        }
        assert_eq!(l.vals.plain("balance"), 0.0);
        assert_eq!(l.hint, "Balance: Centred");
        l.delta(0.0, 0.0, Mods { shift: true, ..Mods::default() });
        assert_eq!(l.vals.plain("balance"), -3.0);
        l.up(Btn::Left);
        // The wheel: two steps of the 256, stopping on the centre.
        l.vals.edit_plain("balance", -1.0);
        l.wheel(x, y, 1.0, Mods::default());
        assert_eq!(l.vals.plain("balance"), 0.0);
        l.wheel(x, y, 1.0, Mods::default());
        assert_eq!(l.vals.plain("balance"), 2.0);
        // Alt click and middle click reset.
        l.down(x, y, Btn::Left, Mods { alt: true, ..Mods::default() }, false);
        assert_eq!(l.vals.plain("balance"), 0.0);
        let acts = l.down(x, y, Btn::Right, Mods::default(), false);
        assert!(matches!(acts.last(), Some(Act::Menu(MenuReq::Param(id), _, _)) if id == "balance"));
    }

    #[test]
    fn selectors_pick_their_value() {
        let mut l = logic(stereo_enhancer::SPEC);
        let (x, y) = centre(&l, &Target::Choice("invert".into(), 2));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(l.params.v(os_engine::effects::stereo_enhancer::INVERT), 2);
        assert_eq!(l.hint, "Inversion: Right");
        let (x, y) = centre(&l, &Target::Choice("placement".into(), 0));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(l.params.v(os_engine::effects::stereo_enhancer::PLACEMENT), 0);
        assert_eq!(l.hint, "Phase offset placement: Pre");
        let mut l = logic(fast_distortion::SPEC);
        let (x, y) = centre(&l, &Target::Choice("type".into(), 1));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(l.params.v(os_engine::effects::fast_dist::TYPE), 1);
        assert_eq!(l.hint, "Type: B");
        let mut l = logic(stereo_shaper::SPEC);
        let (x, y) = centre(&l, &Target::Choice("position".into(), -1));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(l.params.v(6), -1);
        assert_eq!(l.hint, "Effects position: Off");
    }

    /// A fader drags over its travel; a shape button steps through the shapes; a whole tile turns its dial.
    #[test]
    fn faders_shape_buttons_and_tiles() {
        let mut l = logic(stereo_shaper::SPEC);
        let t = Target::Fader("left".into(), 204.0);
        let (x, y) = centre(&l, &t);
        l.down(x, y, Btn::Left, Mods::default(), false);
        // The travel (204 px) is the whole range (-25600..25600): 51 px down from 100 % (12800) is 0.
        for _ in 0..51 {
            l.delta(0.0, 1.0, Mods { shift: true, ..Mods::default() });
        }
        l.up(Btn::Left);
        assert_eq!(l.params.v(1), 0);
        assert!(l.hint.starts_with("Left: "), "{}", l.hint);
        let mut l = logic(transient_processor::SPEC);
        let k = transient_processor::SPEC.index("attack_shape").unwrap();
        let start = l.params.v(k);
        for i in 1..=3 {
            let h = l.hits.iter().find(|h| h.t.id() == "attack_shape").unwrap().clone();
            l.down(h.r.cx(), h.r.cy(), Btn::Left, Mods::default(), false);
            paint(&mut l);
            assert_eq!(l.params.v(k), (start + i) % 3);
        }
        assert!(l.hint.starts_with("Attack shape: "), "{}", l.hint);
        let mut l = logic(chorus::SPEC);
        let h = l.hits.iter().find(|h| h.t.id() == "wave1").unwrap().clone();
        assert_eq!(h.t, Target::Knob("wave1".into()));
        // The tile's right end (the value text) turns the dial too: up to the top.
        l.down(h.r.right() - 4.0, h.r.cy(), Btn::Left, Mods::default(), false);
        for _ in 0..200 {
            l.delta(0.0, -1.0, Mods::default());
        }
        l.up(Btn::Left);
        assert_eq!(l.params.v(os_engine::effects::chorus::WAVE[0]), 1024);
        assert_eq!(l.hint, "LFO 1 wave: Sharp sines");
    }

    #[test]
    fn menus_offer_reset_type_in_automation_and_about() {
        let mut l = logic(soft_clipper::SPEC);
        let m = l.menu(&MenuReq::Param("post".into()));
        assert_eq!(m.find("Reset"), Some(Cmd::Reset("post".into())));
        assert_eq!(m.find("Type in value..."), Some(Cmd::TypeIn("post".into())));
        assert_eq!(m.find("About Lemon Soft Clipper..."), Some(Cmd::About));
        l.vals.edit_plain("post", 10.0);
        l.pick(&MenuReq::Param("post".into()), Cmd::Reset("post".into()), 0.0, 0.0);
        assert_eq!(l.vals.plain("post"), 128.0);
        l.typed("post", "-6");
        assert_eq!(l.vals.text("post"), "-6.0dB");
        // A threshold is never silent: "-inf" changes nothing.
        l.typed("threshold", "-inf");
        assert_eq!(l.vals.plain("threshold"), 100.0);
        l.typed("threshold", "-6");
        assert_eq!(l.vals.plain("threshold"), 64.0);
        assert_eq!(l.pick(&MenuReq::Param("post".into()), Cmd::Automate("post".into()), 0.0, 0.0), vec![Act::Automate(1)]);
        let (x, y) = centre(&l, &Target::Logo);
        assert!(matches!(l.down(x, y, Btn::Left, Mods::default(), false).last(), Some(Act::Menu(MenuReq::About, _, _))));
        assert_eq!(l.hint, "Lemon Soft Clipper");
        assert!(l.wheel(x, y, 1.0, Mods::default()).is_empty());
        let about = l.menu(&MenuReq::About);
        assert_eq!(about.find("Lemon Studio's soft clipper"), Some(Cmd::None));
    }

    /// `SMALLFX_PNG=<dir> cargo test -p lemon-small-fx -- --ignored`: the editors at a few settings, as PNGs.
    #[test]
    #[ignore]
    fn render_editor() {
        let Some(dir) = std::env::var_os("SMALLFX_PNG") else { return };
        let save = |l: &mut Logic, name: &str, scale: f32| {
            let (w, h) = ((l.spec.size.0 as f32 * scale) as u32, (l.spec.size.1 as f32 * scale) as u32);
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
        let settings: [(&Spec, &str, &[(&str, i32)]); 7] = [
            (soft_clipper::SPEC, "soft_clipper", &[("threshold", 64), ("post", 150)]),
            (fast_distortion::SPEC, "fast_distortion", &[("pre", 192), ("threshold", 3), ("type", 1), ("mix", 100), ("post", 110)]),
            (balance::SPEC, "balance", &[("balance", -70), ("volume", 220)]),
            (stereo_enhancer::SPEC, "stereo_enhancer", &[("separation", -54), ("phase", -248), ("pan", -18), ("invert", 1)]),
            (
                transient_processor::SPEC,
                "transient_processor",
                &[("attack", 160), ("drive", 38), ("release", 120), ("attack_shape", 2), ("split", 30), ("balance", 50), ("on", 0)],
            ),
            (chorus::SPEC, "chorus", &[("delay", 40), ("depth", 4375), ("stereo", 149), ("speed1", 3229), ("wave1", 587), ("wave3", 1024), ("cross", 65), ("cutoff", 4531), ("wet", 491)]),
            (stereo_shaper::SPEC, "stereo_shaper", &[("rl", 8076), ("left", 8076), ("right", -8076), ("lr", 8076), ("delay", 1408), ("phase", -2496), ("position", 0)]),
        ];
        for (spec, name, values) in settings {
            let mut l = logic(spec);
            l.hint = format!("{}: {}", spec.settings[0].name, l.vals.text(spec.settings[0].id));
            save(&mut l, &format!("{name}_default"), 1.0);
            save(&mut l, &format!("{name}_default_x2"), 2.0);
            for (id, v) in values {
                l.vals.edit_plain(id, *v as f32);
            }
            save(&mut l, &format!("{name}_set"), 1.0);
        }
    }
}
