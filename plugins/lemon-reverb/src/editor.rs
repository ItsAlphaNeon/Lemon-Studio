// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The plugin editor: a child of the host's window (Win32, X11 on Linux, an NSView on macOS) that paints `view` with
//! the os-ui canvas, and the editor logic (platform independent): knob and fader drags with the cursor held in place,
//! the switches, wheel and reset gestures, the parameter menus. Edits show and sound at once (`os_plugkit::vals`). The
//! room visual turns all the time: the editor repaints on every timer tick.
//!
// spec: plugins/lemon-reverb.md (§1 controls, §2 room visual, §3 hints, §4 interaction), plugins/host-link.md

use std::any::Any;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;

use nih_plug::prelude::*;
use os_engine::effects::reverb::{self as dsp, PARAMS};
use os_plugkit::vals::Vals;
use os_plugkit::MenuDef;
use os_ui::kit::menu::{MenuItem, MenuStyle};

use crate::params::{self, ReverbParams, DEFS};
use crate::room;
use crate::view::{self, Target, ViewState};
use crate::Shared;

pub struct ReverbEditor {
    pub params: Arc<ReverbParams>,
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

impl ReverbEditor {
    pub fn new(params: Arc<ReverbParams>, shared: Arc<Shared>) -> Self {
        ReverbEditor {
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

impl Editor for ReverbEditor {
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
    /// Hide the cursor and hold it in place (knob and fader drags).
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
/// the whole range.
#[derive(Clone, Debug)]
struct Drag {
    id: String,
    v: f32,
    px: f32,
}

/// Every parameter id in the host's order: the knobs, then the switches.
pub fn ids() -> impl Iterator<Item = &'static str> {
    DEFS.iter().map(|d| d.id).chain([params::TEMPO_ID, params::MID_SIDE_ID])
}

pub struct Logic {
    pub vals: Vals,
    pub params: Arc<ReverbParams>,
    pub shared: Arc<Shared>,
    pub view: ViewState,
    pub hits: Vec<view::Hit>,
    drag: Option<Drag>,
    /// Hint text (a tab before the right-aligned part).
    pub hint: String,
    copied: Option<f32>,
    pub hosted: bool,
    /// The room visual's clock: it starts a turn, corner first, when the editor opens.
    opened: Instant,
    /// A fixed turn angle instead of the clock (tests, screenshots).
    pub still: Option<f32>,
}

impl Logic {
    pub fn new(params: Arc<ReverbParams>, shared: Arc<Shared>, ctx: Option<Arc<dyn GuiContext>>) -> Self {
        let vals = Vals::new(&*params, params.live.clone(), ctx);
        let mut l = Logic { vals, params, shared, view: ViewState::default(), hits: Vec::new(), drag: None, hint: String::new(), copied: None, hosted: false, opened: Instant::now(), still: None };
        l.sync_tempo_text();
        l
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    fn hit(&self, x: f32, y: f32) -> Option<(Target, os_ui::Rect)> {
        self.hits.iter().rev().find(|h| h.r.contains(x, y)).map(|h| (h.t.clone(), h.r))
    }

    fn name(id: &str) -> String {
        match id {
            params::TEMPO_ID => params::TEMPO_NAME.into(),
            params::MID_SIDE_ID => params::MID_SIDE_NAME.into(),
            _ => params::index(id).map_or_else(|| id.to_string(), |k| DEFS[k].name.to_string()),
        }
    }

    /// The pre-delay's text follows the shown tempo switch (also before the host confirms it).
    fn sync_tempo_text(&mut self) {
        self.params.tempo_shown.store(self.vals.norm(params::TEMPO_ID) > 0.5, Ordering::Relaxed);
    }

    fn hint_for(&mut self, t: Option<&Target>) -> String {
        self.sync_tempo_text();
        match t {
            Some(Target::Room) => {
                let k = self.knobs();
                let l = room::look(&k);
                format!("Room size {}, {} sides\tdiffusion {}", k[dsp::ROOM], l.sides, k[dsp::DIFFUSION])
            }
            Some(t) => view::hint(t, &self.vals, &Self::name),
            None => String::new(),
        }
    }

    fn range(id: &str) -> (i32, i32) {
        params::index(id).map_or((0, 1), |k| dsp::RANGES[k])
    }

    fn reset(&mut self, id: &str) {
        let d = self.vals.default_norm(id);
        self.vals.edit(id, d);
        self.sync_tempo_text();
    }

    /// The knob values shown.
    pub fn knobs(&self) -> [i32; PARAMS] {
        std::array::from_fn(|k| self.vals.plain(DEFS[k].id).round() as i32)
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
        let mut acts = vec![Act::Repaint];
        match (&t, btn) {
            (Target::Knob(id) | Target::Fader(id) | Target::Switch(id), Btn::Left) if m.alt => self.reset(id),
            (Target::Knob(id) | Target::Fader(id) | Target::Switch(id), Btn::Middle) => self.reset(id),
            (Target::Knob(id), Btn::Left) => acts = self.begin_drag(&id.clone(), KNOB_PX, t.clone()),
            (Target::Fader(id), Btn::Left) => acts = self.begin_drag(&id.clone(), view::FADER_BOT - view::FADER_TOP, t.clone()),
            (Target::Switch(id), Btn::Left) => {
                let on = self.vals.norm(id) > 0.5;
                self.vals.edit(id, if on { 0.0 } else { 1.0 });
                self.sync_tempo_text();
            }
            (Target::Knob(id) | Target::Fader(id) | Target::Switch(id), Btn::Right) => acts.push(Act::Menu(MenuReq::Param(id.clone()), x, y)),
            (Target::Logo, Btn::Left | Btn::Right) => acts.push(Act::Menu(MenuReq::About, x, y)),
            _ => {}
        }
        self.hint = self.hint_for(Some(&t));
        acts
    }

    /// Mouse movement during a locked drag (logical px since the last event).
    pub fn delta(&mut self, _dx: f32, dy: f32, m: Mods) -> Vec<Act> {
        let Some(Drag { id, v, px }) = self.drag.clone() else { return vec![] };
        let f = if m.ctrl { FINE } else { 1.0 };
        let v = (v - dy * f / px).clamp(0.0, 1.0);
        self.vals.set(&id, v);
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

    /// Wheel: one step per notch (1/128 of the range for the knobs with many steps); a switch flips.
    pub fn wheel(&mut self, x: f32, y: f32, notches: f32, _m: Mods) -> Vec<Act> {
        let Some((t, _)) = self.hit(x, y) else { return vec![] };
        let d = notches.signum();
        match &t {
            Target::Knob(id) | Target::Fader(id) => {
                let (lo, hi) = Self::range(id);
                let steps = (hi - lo) as f32;
                let step = if steps > WHEEL_STEPS { (steps / WHEEL_STEPS).round() } else { 1.0 };
                let plain = (self.vals.plain(id) + d * step).clamp(lo as f32, hi as f32);
                self.vals.edit_plain(id, plain);
            }
            Target::Switch(id) => {
                self.vals.edit(id, if d > 0.0 { 1.0 } else { 0.0 });
                self.sync_tempo_text();
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
                m.head(&Self::name(id), Cmd::None);
                m.add(MenuItem::item("Reset"), Cmd::Reset(id.clone()));
                m.sep(Cmd::None);
                m.add(MenuItem::item("Copy value"), Cmd::CopyValue(id.clone()));
                m.add(MenuItem::item("Paste value").enabled(self.copied.is_some()), Cmd::PasteValue(id.clone()));
                if params::index(id).is_some() {
                    m.add(MenuItem::item("Type in value..."), Cmd::TypeIn(id.clone()));
                }
                // Automation belongs to the host: offered when Lemon Studio hosts the editor (host link).
                m.sep(Cmd::None);
                m.add(MenuItem::item("Create automation clip").enabled(self.hosted), Cmd::Automate(id.clone()));
                m
            }
            MenuReq::About => {
                let mut m = MenuDef::new(MenuStyle::Context);
                m.head("Lemon Reverb", Cmd::None);
                m.add(MenuItem::item(&format!("Version {}", env!("CARGO_PKG_VERSION"))).enabled(false), Cmd::None);
                m.add(MenuItem::item("Lemon Studio's recreation of Fruity Reeverb 2").enabled(false), Cmd::None);
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
                    self.sync_tempo_text();
                }
            }
            Cmd::TypeIn(id) => return vec![Act::TypeIn(id, x, y)],
            Cmd::Automate(id) => return ids().position(|i| i == id).map(Act::Automate).into_iter().collect(),
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

    /// The room visual's turn angle now.
    pub fn angle(&self) -> f32 {
        self.still.unwrap_or_else(|| room::angle_at(self.opened.elapsed().as_secs_f64()))
    }

    /// Paints the editor; `hint` is drawn when the host does not show it.
    pub fn paint(&mut self, c: &mut os_ui::Canvas, palette: &os_theme::Palette) {
        self.sync_tempo_text();
        let look = view::Look { room: room::look(&self.knobs()), angle: self.angle() };
        let hint = if self.hosted { String::new() } else { self.hint.clone() };
        self.hits = view::paint(c, palette, &self.vals, &self.view, &look, &hint);
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
    use crate::params::ReverbParams;
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

    pub fn open(parent: *mut c_void, scale: f32, params: Arc<ReverbParams>, shared: Arc<Shared>, ctx: Arc<dyn GuiContext>, slot: Arc<AtomicIsize>) -> Handle {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let px = |v: u32| (v as f32 * scale).round() as u32;
        let size = (px(view::W), px(view::H));
        shared.editor_open.store(true, Ordering::Relaxed);
        unsafe {
            let class = pw::register("LemonReverbView", Some(wndproc));
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
            let title = pw::wide("Lemon Reverb");
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
            // About 33 frames a second: the room visual turns.
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

        /// Timer / parameter change: expire confirmed overlay values and repaint (the room visual turns).
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
                Cmd::Changed | Cmd::Note | Cmd::Automate => 0,
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
    use crate::params::ReverbParams;
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

    pub fn open(parent: px::Parent, scale: f32, params: Arc<ReverbParams>, shared: Arc<Shared>, ctx: Arc<dyn GuiContext>, slot: &px::Slot) -> Handle {
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

        /// Timer / parameter change: expire confirmed overlay values and repaint (the room visual turns).
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
    use os_engine::effects::reverb::{DEFAULTS, ROOM, WET};

    fn logic() -> Logic {
        let mut l = Logic::new(Arc::new(ReverbParams::default()), Arc::new(Shared::default()), None);
        l.still = Some(0.4);
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
    fn every_parameter_has_a_control() {
        let l = logic();
        for d in DEFS.iter() {
            assert!(l.hits.iter().any(|h| matches!(&h.t, Target::Knob(id) | Target::Fader(id) if id == d.id)), "{}", d.id);
        }
        for id in [params::TEMPO_ID, params::MID_SIDE_ID] {
            centre(&l, &Target::Switch(id.into()));
        }
        assert_eq!(ids().count(), PARAMS + 2);
    }

    #[test]
    fn knob_drags_wheel_and_reset() {
        let mut l = logic();
        let t = Target::Knob("size".into());
        let (x, y) = centre(&l, &t);
        let acts = l.down(x, y, Btn::Left, Mods::default(), false);
        assert!(acts.contains(&Act::Lock));
        // 200 px = the whole range (1..100): 40 px up = +19.8.
        for _ in 0..40 {
            l.delta(0.0, -1.0, Mods::default());
        }
        l.up(Btn::Left);
        assert_eq!(l.params.v(ROOM), 70);
        assert_eq!(l.hint, "Room size: 70");
        l.wheel(x, y, 1.0, Mods::default());
        assert_eq!(l.vals.plain("size"), 71.0);
        l.down(x, y, Btn::Left, Mods { alt: true, ..Mods::default() }, false);
        assert_eq!(l.vals.plain("size"), 50.0);
        let acts = l.down(x, y, Btn::Right, Mods::default(), false);
        assert!(matches!(acts.last(), Some(Act::Menu(MenuReq::Param(id), _, _)) if id == "size"));
    }

    #[test]
    fn faders_move_over_their_travel() {
        let mut l = logic();
        let t = Target::Fader("wet".into());
        let (x, y) = centre(&l, &t);
        l.down(x, y, Btn::Left, Mods::default(), false);
        // The travel is the whole range: all the way down = 0.
        l.delta(0.0, view::FADER_BOT - view::FADER_TOP, Mods::default());
        l.up(Btn::Left);
        assert_eq!(l.params.v(WET), 0);
        assert_eq!(l.hint, "Wet level: 0%");
        l.typed("wet", "100%");
        assert_eq!(l.vals.plain("wet"), 128.0);
    }

    #[test]
    fn switches_toggle_and_the_predelay_text_follows_tempo() {
        let mut l = logic();
        l.vals.edit_plain("delay", 192.0);
        let (kx, ky) = centre(&l, &Target::Knob("delay".into()));
        l.moved(kx, ky, Mods::default());
        assert_eq!(l.hint, "Predelay: 500ms");
        let (x, y) = centre(&l, &Target::Switch(params::TEMPO_ID.into()));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert!(l.params.tempo_sync());
        assert_eq!(l.hint, "Tempo-based predelay\ton");
        l.moved(kx, ky, Mods::default());
        assert_eq!(l.hint, "Predelay: 4:00");
        let (x, y) = centre(&l, &Target::Switch(params::MID_SIDE_ID.into()));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert!(l.params.mid_side());
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert!(!l.params.mid_side());
    }

    #[test]
    fn hints_read_the_original_texts() {
        let mut l = logic();
        for (t, want) in [
            (Target::Knob("lcut".into()), "Low cut: 75Hz"),
            (Target::Knob("hcut".into()), "High cut: 4.0kHz"),
            (Target::Knob("dec".into()), "Decay time: 1.5sec"),
            (Target::Knob("sep".into()), "Stereo separation: Original"),
            (Target::Knob("speed".into()), "Mod Speed: 33%"),
            (Target::Fader("er".into()), "Early reflection level: 100%"),
            (Target::Room, "Room size 50, 28 sides\tdiffusion 100"),
        ] {
            let (x, y) = centre(&l, &t);
            l.moved(x, y, Mods::default());
            assert_eq!(l.hint, want);
        }
        assert_eq!(l.knobs(), DEFAULTS);
    }

    #[test]
    fn menus_offer_reset_type_in_and_about() {
        let mut l = logic();
        let m = l.menu(&MenuReq::Param("dec".into()));
        assert_eq!(m.find("Reset"), Some(Cmd::Reset("dec".into())));
        assert_eq!(m.find("Type in value..."), Some(Cmd::TypeIn("dec".into())));
        assert!(l.menu(&MenuReq::Param(params::TEMPO_ID.into())).find("Type in value...").is_none());
        l.vals.edit_plain("dec", 100.0);
        l.pick(&MenuReq::Param("dec".into()), Cmd::Reset("dec".into()), 0.0, 0.0);
        assert_eq!(l.vals.plain("dec"), 15.0);
        assert_eq!(l.pick(&MenuReq::Param("tempo".into()), Cmd::Automate("tempo".into()), 0.0, 0.0), vec![Act::Automate(PARAMS)]);
        let (x, y) = centre(&l, &Target::Logo);
        assert!(matches!(l.down(x, y, Btn::Left, Mods::default(), false).last(), Some(Act::Menu(MenuReq::About, _, _))));
    }

    /// `REVERB_PNG=<dir> cargo test -p lemon-reverb -- --ignored`: the editor at a few settings, as PNGs.
    #[test]
    #[ignore]
    fn render_editor() {
        let Some(dir) = std::env::var_os("REVERB_PNG") else { return };
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
        l.hint = "Room size: 50".into();
        save(&mut l, "reverb_default", 1.0);
        save(&mut l, "reverb_default_x2", 2.0);
        l.hint.clear();
        // FL's The Venue preset.
        for (id, v) in [("lcut", 400), ("hcut", 60), ("delay", 31), ("size", 60), ("diff", 65), ("dec", 23), ("damp", 37), ("bass", 70), ("cross", 330), ("wet", 102)] {
            l.vals.edit_plain(id, v as f32);
        }
        l.still = Some(0.25);
        save(&mut l, "reverb_venue", 1.0);
        for (id, v) in [("lcut", 33), ("hcut", 21), ("delay", 27), ("size", 90), ("diff", 100), ("dec", 45), ("damp", 31), ("bass", 150), ("cross", 298), ("wet", 90)] {
            l.vals.edit_plain(id, v as f32);
        }
        save(&mut l, "reverb_cathedral", 1.0);
        for (id, v) in [("lcut", 19), ("hcut", 221), ("size", 12), ("diff", 0), ("dec", 200), ("damp", 221), ("bass", 100), ("wet", 160)] {
            l.vals.edit_plain(id, v as f32);
        }
        l.vals.edit(params::MID_SIDE_ID, 1.0);
        save(&mut l, "reverb_small_triangle", 1.0);
    }
}
