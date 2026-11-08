// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The plugin editor: a child of the host's window (Win32, X11 on Linux, an NSView on macOS) that paints the display
//! and the controls with the os-ui canvas, and the editor logic (platform independent). Every tick it takes the
//! audio thread's new samples into the analysis and draws the display again. Edits write the state back as the
//! plugin's saved state; a state replaced from outside (a preset, the host) is read again on the next tick.
//!
// spec: plugins/lemon-drop.md "Editor", plugins/host-link.md

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};

use nih_plug::prelude::*;
use os_plugkit::MenuDef;
use os_ui::kit::lemon::plug;
use os_ui::kit::menu::{Mark, MenuItem, MenuStyle};

use crate::analysis::Analyzer;
use crate::display::{self, Pointer};
use crate::state::*;
use crate::view::{self, Ctl, Kind, Ui};
use crate::{DropParams, Shared};

pub struct DropEditor {
    pub params: Arc<DropParams>,
    pub shared: Arc<Shared>,
    /// DPI scale as f32 bits.
    pub scale: AtomicU32,
    #[cfg_attr(not(windows), allow(dead_code))]
    hwnd: Arc<AtomicIsize>,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    slot: os_plugkit::gui::Slot,
}

impl DropEditor {
    pub fn new(params: Arc<DropParams>, shared: Arc<Shared>) -> Self {
        DropEditor {
            params,
            shared,
            scale: AtomicU32::new(1f32.to_bits()),
            hwnd: Arc::new(AtomicIsize::new(0)),
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            slot: Default::default(),
        }
    }
}

impl Editor for DropEditor {
    fn spawn(&self, parent: ParentWindowHandle, context: Arc<dyn GuiContext>) -> Box<dyn Any + Send> {
        let scale = f32::from_bits(self.scale.load(Ordering::Relaxed));
        let (params, shared) = (self.params.clone(), self.shared.clone());
        match parent {
            #[cfg(windows)]
            ParentWindowHandle::Win32Hwnd(hwnd) => Box::new(win32::open(hwnd, scale, params, shared, context, self.hwnd.clone())),
            #[cfg(target_os = "linux")]
            ParentWindowHandle::X11Window(window) => Box::new(gui::open(window, scale, params, shared, context, &self.slot)),
            #[cfg(target_os = "macos")]
            ParentWindowHandle::AppKitNsView(v) => {
                let scale = os_plugkit::gui::backing_scale(v as usize);
                Box::new(gui::open(v as usize, scale, params, shared, context, &self.slot))
            }
            _ => {
                let _ = (context, scale, params, shared);
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

/// Pixels of drag for a knob's whole range.
pub const KNOB_PX: f32 = 250.0;
/// Ctrl: this much finer.
const FINE: f32 = 1.0 / 20.0;
/// The wheel moves this part of the range per notch (at least one step).
const WHEEL_PART: f32 = 1.0 / 32.0;
/// The editor's tick, seconds.
pub const TICK: f32 = 0.03;
/// The windows' timer, ms (Windows rounds it to its 15.6 ms clock).
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub const TIMER_MS: u32 = 15;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuReq {
    Target,
    SpecRes,
    MeterMode,
    VecMode,
    Display,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cmd {
    None,
    Target(usize),
    Res(i32),
    MeterMode(i32),
    VecMode(i32),
    /// None: auto visual mode.
    Mode(Option<Mode>),
    Visible,
    Mini,
    Background,
    KeepInFront,
    OnDesktop,
    Hold,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TypeTarget {
    Title,
    Value(Ctl),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Repaint,
    /// Hide and lock the cursor: a knob drag reports deltas.
    Lock,
    Capture,
    Menu(MenuReq, f32, f32),
    TypeIn(TypeTarget, f32, f32),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Btn {
    Left,
    Right,
    Middle,
}

struct Drag {
    ctl: Ctl,
    /// The value as dragged, unrounded.
    at: f32,
}

/// The two paintings a see-through frame is worked out from (`Logic::paint_see_through`).
#[derive(Default)]
pub struct SeeThrough {
    over_black: Vec<u32>,
    over_white: Vec<u32>,
}

/// A frame as premultiplied ARGB from the same frame in XRGB painted over black and over white: over black a pixel is
/// its colour premultiplied, and the backdrop's share of it is white minus black.
pub fn see_through(over_black: &[u32], over_white: &[u32], out: &mut [u32]) {
    let ch = |v: u32, s: u32| (v >> s) & 0xff;
    for ((o, &b), &w) in out.iter_mut().zip(over_black).zip(over_white) {
        let through = (ch(w, 16).saturating_sub(ch(b, 16)) + ch(w, 8).saturating_sub(ch(b, 8)) + ch(w, 0).saturating_sub(ch(b, 0))) / 3;
        let a = 255 - through;
        *o = (a << 24) | (ch(b, 16).min(a) << 16) | (ch(b, 8).min(a) << 8) | ch(b, 0).min(a);
    }
}

pub struct Logic {
    params: Arc<DropParams>,
    shared: Arc<Shared>,
    version: u64,
    /// The edits count as of the state in hand.
    edits: u64,
    pub st: State,
    pub an: Analyzer,
    /// Samples taken from the audio thread so far.
    seen: u64,
    hover: Option<Ctl>,
    pressed: Option<Ctl>,
    drag: Option<Drag>,
    /// The colour target the colour knobs edit.
    pub target: usize,
    /// The display is frozen (the HOLD button, a middle click on it, or the host's "Hold visualization").
    pub held: bool,
    ptr: Option<(f32, f32)>,
    pub hint: String,
    pub hosted: bool,
    l: Vec<f32>,
    r: Vec<f32>,
}

impl Drop for Logic {
    fn drop(&mut self) {
        self.shared.viewers.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Logic {
    pub fn new(params: Arc<DropParams>, shared: Arc<Shared>) -> Self {
        let edits = params.edits.load(Ordering::Acquire);
        let (version, st) = params.current();
        let rate = shared.rate.load(Ordering::Relaxed) as f32;
        let seen = shared.scope.lock().map(|s| s.written).unwrap_or(0);
        shared.viewers.fetch_add(1, Ordering::Relaxed);
        Logic {
            params,
            shared,
            version,
            edits,
            st,
            an: Analyzer::new(rate),
            seen,
            hover: None,
            pressed: None,
            drag: None,
            target: WAVEFORM,
            held: false,
            ptr: None,
            hint: String::new(),
            hosted: false,
            l: Vec::new(),
            r: Vec::new(),
        }
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    fn reload(&mut self) {
        self.edits = self.params.edits.load(Ordering::Acquire);
        let (v, st) = self.params.current();
        self.version = v;
        self.st = st;
    }

    /// Before an edit: the state as it stands, when another display edited it or it was replaced since.
    fn sync(&mut self) {
        let stale = self.params.edits.load(Ordering::Acquire) != self.edits || self.params.state_version.load(Ordering::Acquire) != self.version;
        if stale && self.drag.is_none() {
            self.reload();
        }
    }

    fn commit(&mut self) {
        if self.params.edited(self.version, &self.st) {
            self.edits = self.params.edits.load(Ordering::Acquire);
        } else {
            self.reload();
        }
    }

    fn held(&self) -> bool {
        self.held || self.params.hold.value()
    }

    /// The tick: a state replaced from outside, the new samples into the analysis. True: repaint.
    pub fn poll(&mut self) -> bool {
        self.tick(TICK)
    }

    /// `poll` with the time since the last tick (`dt` seconds).
    pub fn tick(&mut self, dt: f32) -> bool {
        let edited = self.params.edits.load(Ordering::Acquire) != self.edits;
        if (self.params.state_version.load(Ordering::Acquire) != self.version || edited) && self.drag.is_none() {
            self.reload();
        }
        self.an.set_rate(self.shared.rate.load(Ordering::Relaxed) as f32);
        let mut new = 0;
        if let Ok(s) = self.shared.scope.lock() {
            new = (s.written.saturating_sub(self.seen) as usize).min(s.l.len());
            self.seen = s.written;
            s.latest(new, &mut self.l, &mut self.r);
        }
        if !self.held() {
            let (l, r) = (std::mem::take(&mut self.l), std::mem::take(&mut self.r));
            self.an.push(&self.st, &l[..new.min(l.len())], &r[..new.min(r.len())], dt.clamp(0.001, 0.25));
            (self.l, self.r) = (l, r);
        }
        true
    }

    fn ui(&self) -> Ui {
        Ui { hover: self.hover, pressed: self.pressed, target: self.target, held: self.held() }
    }

    fn set_hint(&mut self, c: Option<Ctl>) {
        self.hint = c.map(|c| view::hint(&self.st, c, self.target)).unwrap_or_default();
    }

    pub fn down(&mut self, x: f32, y: f32, btn: Btn, m: Mods, dbl: bool) -> Vec<Act> {
        self.sync();
        let Some(c) = view::hit(&self.st, x, y) else {
            return vec![];
        };
        self.pressed = Some(c);
        match (kind(c), btn) {
            (Kind::Area, Btn::Middle) | (Kind::Button, Btn::Left) => {
                self.held = !self.held;
                vec![Act::Repaint]
            }
            (Kind::Area, Btn::Right) => vec![Act::Menu(MenuReq::Display, x, y)],
            (Kind::Area, _) => vec![],
            (Kind::Knob { def, .. } | Kind::Fader { def, .. }, Btn::Left) if dbl || m.alt => {
                view::set_value(&mut self.st, c, self.target, def);
                self.commit();
                self.set_hint(Some(c));
                vec![Act::Repaint]
            }
            (Kind::Knob { .. } | Kind::Fader { .. }, Btn::Left) => {
                self.drag = Some(Drag { ctl: c, at: view::value(&self.st, c, self.target) as f32 });
                vec![Act::Lock, Act::Capture, Act::Repaint]
            }
            (Kind::Knob { .. } | Kind::Fader { .. }, Btn::Right) => {
                vec![Act::TypeIn(TypeTarget::Value(c), x, y)]
            }
            (Kind::Toggle | Kind::Tab, Btn::Left) => {
                view::toggle(&mut self.st, c);
                self.commit();
                vec![Act::Repaint]
            }
            (Kind::List, Btn::Left | Btn::Right) => {
                let req = match c {
                    Ctl::Target => MenuReq::Target,
                    Ctl::SpecRes => MenuReq::SpecRes,
                    Ctl::MeterMode => MenuReq::MeterMode,
                    _ => MenuReq::VecMode,
                };
                vec![Act::Menu(req, x, y)]
            }
            (Kind::Text, Btn::Left) => vec![Act::TypeIn(TypeTarget::Title, x, y)],
            _ => vec![],
        }
    }

    pub fn delta(&mut self, _dx: f32, dy: f32, m: Mods) -> Vec<Act> {
        let Some(d) = self.drag.as_mut() else {
            return vec![];
        };
        let (min, max) = match kind(d.ctl) {
            Kind::Knob { min, max, .. } | Kind::Fader { min, max, .. } => (min, max),
            _ => return vec![],
        };
        let wrap = matches!(kind(d.ctl), Kind::Knob { wrap: true, .. });
        let per_px = (max - min) as f32 / KNOB_PX * if m.ctrl { FINE } else { 1.0 };
        d.at -= dy * per_px;
        if !wrap {
            d.at = d.at.clamp(min as f32, max as f32);
        }
        let (c, v) = (d.ctl, d.at.round() as i32);
        view::set_value(&mut self.st, c, self.target, v);
        self.commit();
        self.set_hint(Some(c));
        vec![Act::Repaint]
    }

    pub fn up(&mut self, _btn: Btn) -> Vec<Act> {
        self.drag = None;
        self.pressed = None;
        vec![Act::Repaint]
    }

    pub fn moved(&mut self, x: f32, y: f32, _m: Mods) -> Vec<Act> {
        let c = view::hit(&self.st, x, y);
        self.ptr = (c == Some(Ctl::Display)).then_some((x, y));
        let changed = c != self.hover;
        self.hover = c;
        if changed {
            self.set_hint(c.filter(|&c| c != Ctl::Display));
        }
        vec![Act::Repaint]
    }

    pub fn leave(&mut self) -> Vec<Act> {
        self.hover = None;
        self.ptr = None;
        self.hint.clear();
        vec![Act::Repaint]
    }

    pub fn wheel(&mut self, x: f32, y: f32, notches: f32, _m: Mods) -> Vec<Act> {
        self.sync();
        let Some(c) = view::hit(&self.st, x, y) else {
            return vec![];
        };
        let (min, max, wrap) = match kind(c) {
            Kind::Knob { min, max, wrap, .. } => (min, max, wrap),
            Kind::Fader { min, max, .. } => (min, max, false),
            _ => return vec![],
        };
        let step = (((max - min) as f32 * WHEEL_PART).round() as i32).max(1);
        let mut v = view::value(&self.st, c, self.target) + (notches.round() as i32) * step;
        if !wrap {
            v = v.clamp(min, max);
        }
        view::set_value(&mut self.st, c, self.target, v);
        self.commit();
        self.set_hint(Some(c));
        vec![Act::Repaint]
    }

    pub fn menu(&self, req: &MenuReq) -> MenuDef<Cmd> {
        match req {
            MenuReq::Target => {
                let mut m = MenuDef::new(MenuStyle::List);
                let groups = TARGET_GROUPS.iter().chain(std::iter::once(&EYE_CANDY));
                for (i, &(head, first, n)) in groups.enumerate() {
                    if i == 6 {
                        m.add(MenuItem::Column, Cmd::None);
                    }
                    m.head(head, Cmd::None);
                    for k in first..first + n {
                        let row = m.add(MenuItem::item(TARGET_NAMES[k]), Cmd::Target(k));
                        if k == self.target {
                            m.current = Some(row);
                        }
                    }
                }
                m
            }
            MenuReq::SpecRes => {
                let mut m = MenuDef::new(MenuStyle::List);
                for r in 0..=8 {
                    let row = m.add(MenuItem::item(&format!("{} bands", crate::analysis::bands(r))), Cmd::Res(r));
                    if r == self.st.spectrum_res {
                        m.current = Some(row);
                    }
                }
                m
            }
            MenuReq::MeterMode => {
                let mut m = MenuDef::new(MenuStyle::List);
                for &(v, name) in &METER_MODES {
                    let row = m.add(MenuItem::item(name), Cmd::MeterMode(v));
                    if v == self.st.meter_mode {
                        m.current = Some(row);
                    }
                }
                m
            }
            MenuReq::VecMode => {
                let mut m = MenuDef::new(MenuStyle::List);
                for (v, name) in VECTOR_MODES.iter().enumerate() {
                    let row = m.add(MenuItem::item(name), Cmd::VecMode(v as i32));
                    if v as i32 == self.st.vector_mode {
                        m.current = Some(row);
                    }
                }
                m
            }
            MenuReq::Display => {
                let mut m = MenuDef::new(MenuStyle::Context);
                m.head("Window", Cmd::None);
                m.add(MenuItem::item("Visible").mark(Mark::Check(self.st.visible)), Cmd::Visible);
                m.add(MenuItem::item("Mini view").mark(Mark::Check(self.st.mini)), Cmd::Mini);
                m.add(MenuItem::item("Background").mark(Mark::Check(self.st.background)), Cmd::Background);
                m.add(MenuItem::item("Keep in front").mark(Mark::Check(self.st.keep_in_front)), Cmd::KeepInFront);
                m.add(MenuItem::item("On desktop").mark(Mark::Check(self.st.on_desktop)), Cmd::OnDesktop);
                m.add(MenuItem::item("Hold").shortcut("Middle click").mark(Mark::Check(self.held)), Cmd::Hold);
                m.head("Mode", Cmd::None);
                m.add(MenuItem::item("Auto").mark(Mark::Radio(self.st.auto_mode)), Cmd::Mode(None));
                for mode in Mode::ALL {
                    m.add(MenuItem::item(mode.name()).mark(Mark::Radio(!self.st.auto_mode && self.st.mode == mode)), Cmd::Mode(Some(mode)));
                }
                m
            }
        }
    }

    pub fn pick(&mut self, _req: &MenuReq, cmd: Cmd, _x: f32, _y: f32) -> Vec<Act> {
        self.sync();
        match cmd {
            Cmd::None => return vec![],
            Cmd::Target(k) => {
                self.target = k.min(TARGETS - 1);
                return vec![Act::Repaint];
            }
            Cmd::Hold => {
                self.held = !self.held;
                return vec![Act::Repaint];
            }
            Cmd::Res(r) => self.st.spectrum_res = r.clamp(0, 8),
            Cmd::MeterMode(v) => self.st.meter_mode = v,
            Cmd::VecMode(v) => self.st.vector_mode = v.clamp(0, 3),
            Cmd::Mode(None) => self.st.auto_mode = true,
            Cmd::Mode(Some(m)) => {
                self.st.auto_mode = false;
                self.st.mode = m;
            }
            Cmd::Visible => self.st.visible = !self.st.visible,
            Cmd::Background => self.st.background = !self.st.background,
            Cmd::Mini => self.st.mini = !self.st.mini,
            Cmd::KeepInFront => self.st.keep_in_front = !self.st.keep_in_front,
            Cmd::OnDesktop => {
                self.st.on_desktop = !self.st.on_desktop;
                self.st.desktop_flag = self.st.on_desktop;
            }
        }
        self.commit();
        vec![Act::Repaint]
    }

    pub fn value_text(&self, t: &TypeTarget) -> String {
        match t {
            TypeTarget::Title => self.st.title_text(),
            TypeTarget::Value(c) => view::value(&self.st, *c, self.target).to_string(),
        }
    }

    pub fn typed(&mut self, t: &TypeTarget, text: &str) {
        self.sync();
        match t {
            TypeTarget::Title => {
                // Single-byte text, as the original stores it.
                self.st.title = text.chars().map(|c| if (c as u32) < 256 { c as u8 } else { b'?' }).take(255).collect();
            }
            TypeTarget::Value(c) => {
                let Ok(v) = text.trim().parse::<f32>() else {
                    return;
                };
                let (min, max) = match kind(*c) {
                    Kind::Knob { min, max, .. } | Kind::Fader { min, max, .. } => (min, max),
                    _ => return,
                };
                view::set_value(&mut self.st, *c, self.target, (v.round() as i32).clamp(min, max));
            }
        }
        self.commit();
    }

    // ---- The floating display window

    /// The pointer over the floating display (logical px in it), for the reading under it.
    pub fn point(&mut self, at: Option<(f32, f32)>) {
        self.ptr = at;
    }

    /// A middle click: freezes or frees the display.
    pub fn toggle_hold(&mut self) {
        self.held = !self.held;
    }

    /// The floating window's rectangle as stored (the mini view's while it is on), logical screen px.
    pub fn window_rect(&self) -> Rect4 {
        if self.st.mini { self.st.mini_window } else { self.st.window }
    }

    /// The floating window was moved or resized to `r` (logical screen px): kept in the state.
    pub fn set_window_rect(&mut self, r: Rect4) {
        self.sync();
        if self.window_rect() == r {
            return;
        }
        if self.st.mini {
            self.st.mini_window = r;
        } else {
            self.st.window = r;
        }
        self.commit();
    }

    /// The floating window's picture: the display filling `area`, as premultiplied ARGB words in `out` (the canvas's
    /// `w * h` of them). Drawn text blends with the pixels under it and leaves them opaque, so the frame is painted
    /// twice, over black and over white: where the two differ the display let the backdrop through, and by how much is
    /// the pixel's transparency.
    pub fn paint_see_through(&mut self, c: &mut os_ui::Canvas, area: os_ui::Rect, buffers: &mut SeeThrough, out: &mut [u32]) {
        let held = self.held();
        let solid = self.st.solid;
        for (back, buf) in [(os_ui::Color::rgb(0x000000), &mut buffers.over_black), (os_ui::Color::rgb(0xffffff), &mut buffers.over_white)] {
            c.clear(back);
            if solid {
                // All but invisible, so the whole window takes the mouse (a fully clear pixel lets clicks through).
                c.fill_rect(area, os_ui::Color { r: 0, g: 0, b: 0, a: 1 });
            }
            display::paint(c, &self.st, &self.an, area, Pointer { at: self.ptr, held });
            buf.resize(out.len(), 0);
            c.copy_to_xrgb(buf);
        }
        see_through(&buffers.over_black, &buffers.over_white, out);
    }

    pub fn paint(&mut self, c: &mut os_ui::Canvas) {
        c.clear(plug::BG);
        let held = self.held();
        display::paint(c, &self.st, &self.an, view::DISPLAY, Pointer { at: self.ptr, held });
        view::paint_panel(c, &self.st, &self.ui());
    }
}

use view::kind;

// ---------------------------------------------------------------------------------------------------------------
// The window

#[cfg(windows)]
mod win32 {
    use std::ffi::c_void;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicIsize, Ordering};

    use nih_plug::prelude::GuiContext;
    use os_plugkit::win::{self as pw, CursorLock};
    use os_theme::Palette;
    use os_ui::Canvas;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Controls::WM_MOUSELEAVE;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    use super::{Act, Btn, Logic, Mods};
    use crate::view;
    use crate::{DropParams, Shared};

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
        /// When the last tick ran.
        last: std::time::Instant,
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

    fn mods() -> Mods {
        let (ctrl, shift, alt) = pw::mods();
        Mods { ctrl, shift, alt }
    }

    pub fn open(
        parent: *mut c_void,
        scale: f32,
        params: Arc<DropParams>,
        shared: Arc<Shared>,
        _ctx: Arc<dyn GuiContext>,
        slot: Arc<AtomicIsize>,
    ) -> Handle {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let logic = Logic::new(params, shared);
        let px = |v: u32| (v as f32 * scale).round() as u32;
        let size = (px(view::W), px(view::H));
        unsafe {
            let class = pw::register("LemonDropView", Some(wndproc));
            let win = Box::new(Win {
                hwnd: std::ptr::null_mut(),
                canvas: Canvas::new(size.0, size.1, scale).expect("canvas"),
                buf: vec![0; (size.0 * size.1) as usize],
                scale,
                size,
                logic,
                palette: os_plugkit::palette(),
                last_hint: String::new(),
                lock: CursorLock::default(),
                tracking: false,
                slot: slot.clone(),
                last: std::time::Instant::now(),
            });
            let title = pw::wide(crate::NAME);
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
            SetTimer(hwnd, TIMER, super::TIMER_MS, None);
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
            let Some(w) = state(hwnd) else {
                return DefWindowProcW(hwnd, msg, wp, lp);
            };
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
                    w.run(acts);
                    0
                }
                WM_MOUSEMOVE => {
                    if !w.tracking {
                        let mut tme = TRACKMOUSEEVENT {
                            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                            dwFlags: TME_LEAVE,
                            hwndTrack: hwnd,
                            dwHoverTime: 0,
                        };
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

        /// The tick: the analysis and one repaint (input only changes the state; the tick draws it, so a moving mouse
        /// never starves the animation). The floating display opens when it should be visible.
        fn poll(&mut self) {
            let now = std::time::Instant::now();
            let dt = now.duration_since(self.last).as_secs_f32();
            self.last = now;
            if self.logic.tick(dt) {
                self.invalidate();
            }
            crate::float::ensure(&self.logic.params, &self.logic.shared, self.scale, self.hwnd);
            if self.logic.hint != self.last_hint {
                self.last_hint = self.logic.hint.clone();
                if self.logic.hosted {
                    os_hostlink::to_host(self.hwnd as isize, os_hostlink::Cmd::Changed, 0);
                }
            }
        }

        /// Carries out what the logic asked for; repaints wait for the tick.
        fn run(&mut self, acts: Vec<Act>) {
            for a in acts {
                match a {
                    Act::Repaint => {}
                    Act::Lock => self.lock.lock(self.hwnd),
                    Act::Capture => unsafe {
                        SetCapture(self.hwnd);
                    },
                    Act::Menu(req, x, y) => {
                        let def = self.logic.menu(&req);
                        let (sx, sy) = self.screen(x, y);
                        if let Some(cmd) = pw::popup(self.hwnd, sx, sy, &def, &self.palette, self.scale) {
                            let acts = self.logic.pick(&req, cmd, x, y);
                            self.run(acts);
                        }
                    }
                    Act::TypeIn(f, x, y) => {
                        let (sx, sy) = self.screen(x, y);
                        let cur = self.logic.value_text(&f);
                        if let Some(t) = pw::type_in(self.hwnd, sx, sy, &cur, self.scale) {
                            self.logic.typed(&f, &t);
                        }
                    }
                }
            }
        }

        /// Answers the host (spec `plugins/host-link.md`): one page, the hint line.
        unsafe fn host_link(&mut self, wp: WPARAM, lp: LPARAM) -> LRESULT {
            use os_hostlink::{Cmd, PageIcon};
            let Some(cmd) = Cmd::from_usize(wp) else {
                return 0;
            };
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
                Cmd::Changed | Cmd::Note | Cmd::Automate | Cmd::Audio | Cmd::Resizable | Cmd::SetSize | Cmd::Notes => 0,
            }
        }

        fn paint(&mut self) {
            let (hdc, ps) = pw::begin_paint(self.hwnd);
            self.logic.paint(&mut self.canvas);
            self.buf.resize((self.size.0 * self.size.1) as usize, 0);
            self.canvas.copy_to_xrgb(&mut self.buf);
            pw::blit(hdc, &self.buf, self.size.0, self.size.1);
            pw::end_paint(self.hwnd, &ps);
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod gui {
    use std::sync::Arc;

    use nih_plug::prelude::GuiContext;
    use os_hostlink::{Cmd, PageIcon, Peer};
    use os_plugkit::gui::{self as px, Button, Input, Window};
    use os_theme::Palette;
    use os_ui::Canvas;

    use super::{Act, Btn, Logic, Mods};
    use crate::view;
    use crate::{DropParams, Shared};

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
        /// When the last tick ran.
        last: std::time::Instant,
    }

    impl Drop for Ed {
        fn drop(&mut self) {
            self.link.unregister();
        }
    }

    /// Owns the editor thread; dropping it closes the editor.
    pub struct Handle(#[allow(dead_code)] Option<px::EditorThread>);

    pub fn open(parent: px::Parent, scale: f32, params: Arc<DropParams>, shared: Arc<Shared>, _ctx: Arc<dyn GuiContext>, slot: &px::Slot) -> Handle {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let size = ((view::W as f32 * scale).round() as u32, (view::H as f32 * scale).round() as u32);
        let wake = slot.clone();
        let link = Peer::register(parent as u64, vec![PageIcon::Plugin], (view::W, view::H), move || px::notify(&wake));
        let peer = link.clone();
        let thread = px::spawn(parent, size, slot, move |win| {
            let Ok(canvas) = Canvas::new(size.0, size.1, scale) else {
                return;
            };
            let logic = Logic::new(params, shared);
            let ed = Ed {
                win,
                canvas,
                buf: Vec::new(),
                scale,
                size,
                logic,
                palette: os_plugkit::palette(),
                last_hint: String::new(),
                dirty: true,
                link: peer,
                last: std::time::Instant::now(),
            };
            ed.run();
        });
        if thread.is_none() {
            link.unregister();
        }
        Handle(thread)
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
            #[cfg(target_os = "macos")]
            self.win.set_tick(std::time::Duration::from_millis(u64::from(super::TIMER_MS)));
            while let Some(input) = self.win.next() {
                match input {
                    Input::Expose => self.dirty = true,
                    // Input only changes the state; the tick draws it, so a moving mouse never starves the animation.
                    Input::Tick => {
                        self.poll();
                        if self.dirty {
                            self.paint();
                        }
                    }
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
            let now = std::time::Instant::now();
            let dt = now.duration_since(self.last).as_secs_f32();
            self.last = now;
            if self.logic.tick(dt) {
                self.dirty = true;
            }
            // The floating display opens when it should be visible.
            #[cfg(target_os = "macos")]
            crate::float::ensure(&self.logic.params, &self.logic.shared, &self.win);
            #[cfg(target_os = "linux")]
            crate::float::ensure(&self.logic.params, &self.logic.shared, self.scale, self.win.win);
            if self.logic.hint != self.last_hint {
                self.last_hint.clone_from(&self.logic.hint);
                self.link.set_hint(&self.logic.hint);
                self.link.to_host(Cmd::Changed, 0);
                self.dirty = true;
            }
            self.logic.hosted = self.link.hosted();
        }

        fn act(&mut self, acts: Vec<Act>) {
            for a in acts {
                match a {
                    Act::Repaint => self.dirty = true,
                    Act::Lock => self.win.lock(),
                    Act::Capture => {}
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
                        let cur = self.logic.value_text(&f);
                        if let Some(t) = self.win.type_in(sx, sy, &cur, self.scale) {
                            self.logic.typed(&f, &t);
                        }
                        self.dirty = true;
                    }
                }
            }
        }

        fn paint(&mut self) {
            self.dirty = false;
            self.logic.paint(&mut self.canvas);
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
        Logic::new(Arc::new(DropParams::default()), Arc::new(Shared::default()))
    }

    fn centre(l: &Logic, c: Ctl) -> (f32, f32) {
        let r = view::layout(&l.st).into_iter().find(|(k, _)| *k == c).unwrap().1;
        (r.cx(), r.cy())
    }

    fn saved(l: &Logic) -> State {
        l.params.current().1
    }

    #[test]
    fn knobs_drag_wheel_and_reset_into_the_saved_state() {
        let mut l = logic();
        let (x, y) = centre(&l, Ctl::Fade);
        l.down(x, y, Btn::Left, Mods::default(), false);
        l.delta(0.0, KNOB_PX / 2.0, Mods::default());
        l.up(Btn::Left);
        assert!((saved(&l).fade - NEVER / 2).abs() <= 1, "{}", saved(&l).fade);
        l.wheel(x, y, 1.0, Mods::default());
        assert!(saved(&l).fade > NEVER / 2);
        l.down(x, y, Btn::Left, Mods::default(), true);
        assert_eq!(saved(&l).fade, NEVER);
    }

    #[test]
    fn tabs_toggles_and_menus_edit_the_state() {
        let mut l = logic();
        let (x, y) = centre(&l, Ctl::Tab(Mode::Meter));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(saved(&l).mode, Mode::Meter);
        let (x, y) = centre(&l, Ctl::MeterMode);
        let acts = l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(acts, vec![Act::Menu(MenuReq::MeterMode, x, y)]);
        let cmd = l.menu(&MenuReq::MeterMode).find("RMS").unwrap();
        l.pick(&MenuReq::MeterMode, cmd, x, y);
        assert_eq!(saved(&l).meter_mode, 2);
        let (x, y) = centre(&l, Ctl::Background);
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert!(!saved(&l).background);
    }

    #[test]
    fn the_colour_menu_picks_what_the_colour_knobs_edit() {
        let mut l = logic();
        let cmd = l.menu(&MenuReq::Target).find("Spectrum gradient color 3").unwrap();
        l.pick(&MenuReq::Target, cmd, 0.0, 0.0);
        let (x, y) = centre(&l, Ctl::Light);
        l.wheel(x, y, -2.0, Mods::default());
        assert_eq!(saved(&l).paints[SPECTRUM + 2].light, 120 - 16);
    }

    #[test]
    fn titles_are_typed() {
        let mut l = logic();
        l.typed(&TypeTarget::Title, "Lemon Drop");
        assert_eq!(saved(&l).title, b"Lemon Drop");
    }

    #[test]
    fn two_displays_keep_each_others_edits() {
        // The editor and the floating window: one's edit is seen by the other, and neither undoes it.
        let mut ed = logic();
        let mut fl = Logic::new(ed.params.clone(), ed.shared.clone());
        assert_eq!(ed.shared.viewers.load(Ordering::Relaxed), 2);
        ed.pick(&MenuReq::Display, Cmd::Background, 0.0, 0.0);
        assert!(!saved(&ed).background);
        fl.set_window_rect([10, 20, 310, 145]);
        let st = saved(&ed);
        assert!(!st.background, "the window move kept the editor's edit");
        assert_eq!(st.window, [10, 20, 310, 145]);
        fl.poll();
        ed.poll();
        assert!(!fl.st.background);
        assert_eq!(ed.st.window, [10, 20, 310, 145]);
    }

    #[test]
    fn a_state_replaced_from_outside_is_read_again() {
        let mut l = logic();
        let mut st = State::new_default();
        st.gain = 55;
        l.params.set_state(st.to_bytes());
        l.poll();
        assert_eq!(l.st.gain, 55);
    }

    #[test]
    fn the_editor_takes_the_new_samples() {
        let mut l = logic();
        {
            let mut s = l.shared.scope.lock().unwrap();
            for i in 0..1000 {
                s.l[i] = 0.5;
                s.r[i] = 0.5;
            }
            s.pos = 1000;
            s.written = 1000;
        }
        l.poll();
        assert!((l.an.meter[0] + 6.02).abs() < 0.1, "{}", l.an.meter[0]);
        let shared = l.shared.clone();
        assert_eq!(shared.viewers.load(Ordering::Relaxed), 1);
        drop(l);
        assert_eq!(shared.viewers.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn see_through_frames_take_their_alpha_from_the_two_paintings() {
        // Clear, half white, half black, opaque red, and a pixel whose colours round past its alpha.
        let black = [0x000000, 0x808080, 0x000000, 0xff0000, 0x105010];
        let white = [0xffffff, 0xffffff, 0x7f7f7f, 0xff0000, 0xefefef];
        let mut out = [0; 5];
        see_through(&black, &white, &mut out);
        assert_eq!(out[0], 0x0000_0000, "clear");
        assert_eq!(out[1], 0x8080_8080, "half white");
        assert_eq!(out[2], 0x8000_0000, "half black");
        assert_eq!(out[3], 0xffff_0000, "opaque");
        let a = out[4] >> 24;
        assert!([16, 8, 0].iter().all(|s| (out[4] >> s) & 0xff <= a), "colours stay premultiplied: {:08x}", out[4]);
    }

    /// The floating display's frame: clear where the display paints nothing, the picture's colours where it does, with
    /// Solid's all but invisible fill only while Solid is on.
    #[test]
    fn the_floating_picture_is_see_through_without_a_background() {
        let mut l = logic();
        l.st.background = false;
        l.st.bevel = 0;
        l.st.mode = Mode::Oscilloscope;
        l.st.title.clear();
        l.commit();
        let (mut c, area, mut buffers) = (os_ui::Canvas::new(200, 100, 1.0).unwrap(), os_ui::Rect::new(0.0, 0.0, 200.0, 100.0), SeeThrough::default());
        let mut frame = vec![0u32; 200 * 100];
        l.st.solid = false;
        l.paint_see_through(&mut c, area, &mut buffers, &mut frame);
        assert_eq!(frame[0], 0, "a corner is clear with Solid off");
        l.st.solid = true;
        l.paint_see_through(&mut c, area, &mut buffers, &mut frame);
        assert_eq!(frame[0] >> 24, 1, "with Solid on it holds the all but invisible fill");
        l.st.background = true;
        l.paint_see_through(&mut c, area, &mut buffers, &mut frame);
        // The glass is translucent itself: it lets some of the desktop through, but far more paint than Solid's fill.
        assert!(frame[100 * 50 + 100] >> 24 > 100, "the background shows: {:08x}", frame[100 * 50 + 100]);
    }

    fn render(l: &mut Logic, path: &std::path::Path) {
        let scale = 2.0;
        let (w, h) = ((view::W as f32 * scale) as u32, (view::H as f32 * scale) as u32);
        let mut c = os_ui::Canvas::new(w, h, scale).unwrap();
        l.paint(&mut c);
        let mut buf = vec![0u32; (w * h) as usize];
        c.copy_to_xrgb(&mut buf);
        let mut pm = tiny_skia::Pixmap::new(w, h).unwrap();
        for (d, s) in pm.data_mut().chunks_exact_mut(4).zip(&buf) {
            d.copy_from_slice(&[(s >> 16) as u8, (s >> 8) as u8, *s as u8, 255]);
        }
        pm.save_png(path).unwrap();
    }

    /// Every factory preset's display, rendered with a signal for its mode, for looking at:
    /// `DROP_PNG=<dir> cargo test -p lemon-drop -- --ignored render_presets` (one PNG per preset).
    #[test]
    #[ignore]
    fn render_presets() {
        let Ok(dir) = std::env::var("DROP_PNG") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/os-app/factory-presets/Lemon Drop");
        let mut walk = vec![root.clone()];
        let rate = 44100.0f32;
        while let Some(d) = walk.pop() {
            for e in std::fs::read_dir(&d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    walk.push(p);
                    continue;
                }
                let b = std::fs::read(&p).unwrap();
                let blob = b.splitn(3, |&c| c == b'\n').nth(2).unwrap().to_vec();
                let fl = crate::fl_from_plugin_state(&blob).unwrap();
                let mut l = logic();
                l.params.set_state(fl);
                l.poll();
                let mode = display::shown_mode(&l.st, display::box_rect(&l.st, view::DISPLAY));
                for block in 0..120usize {
                    let (a, b): (Vec<f32>, Vec<f32>) = (0..1323)
                        .map(|i| {
                            let t = (block * 1323 + i) as f32 / rate;
                            let s = |f: f32| (2.0 * std::f32::consts::PI * f * t).sin();
                            match mode {
                                Mode::Vectorscope => (0.4 * s(220.0) + 0.1 * s(330.0), 0.4 * (2.0 * std::f32::consts::PI * 220.0 * t).cos()),
                                Mode::Spectrum => {
                                    let n = ((block * 1323 + i) as f32 * 12.9898).sin() * 43758.547;
                                    let noise = (n - n.floor() - 0.5) * 0.2;
                                    let v = 0.3 * s(110.0) + 0.15 * s(880.0) + 0.08 * s(3520.0) + noise;
                                    (v, v)
                                }
                                Mode::Meter => (0.45 * s(220.0), 0.3 * s(220.0)),
                                Mode::Oscilloscope => (0.35 * s(220.0) + 0.12 * s(660.0), 0.35 * s(220.0) + 0.12 * s(660.0)),
                            }
                        })
                        .unzip();
                    l.an.push(&l.st.clone(), &a, &b, TICK);
                }
                let rel = p.strip_prefix(&root).unwrap().with_extension("png");
                let name = rel.to_string_lossy().replace(['/', '\\'], "_");
                render(&mut l, &dir.join(name));
            }
        }
    }

    /// Editor renders for looking at: `DROP_PNG=<dir> cargo test -p lemon-drop -- --ignored render_editor`.
    #[test]
    #[ignore]
    fn render_editor() {
        let Ok(dir) = std::env::var("DROP_PNG") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        let rate = 44100.0;
        let sig = |k: usize, i: usize| -> (f32, f32) {
            let t = i as f32 / rate;
            let s = |f: f32| (2.0 * std::f32::consts::PI * f * t).sin();
            match k {
                0 => (0.5 * s(440.0), 0.5 * s(440.0)),
                1 => (0.4 * s(110.0) + 0.2 * s(1760.0), 0.4 * s(110.0) - 0.15 * s(2637.0)),
                2 => (0.6 * s(220.0), 0.6 * (2.0 * std::f32::consts::PI * 220.0 * t).cos()),
                _ => {
                    let f = 20.0 * 1000f32.powf((t / 3.0).min(1.0));
                    let ph = 2.0 * std::f32::consts::PI * 20.0 * 3.0 / 1000f32.ln() * (1000f32.powf((t / 3.0).min(1.0)) - 1.0);
                    let _ = f;
                    (0.5 * ph.sin(), 0.5 * ph.sin())
                }
            }
        };
        for (name, mode, k) in
            [("scope", Mode::Oscilloscope, 0), ("spectrum", Mode::Spectrum, 3), ("meter", Mode::Meter, 1), ("vector", Mode::Vectorscope, 2)]
        {
            let mut l = logic();
            l.st.mode = mode;
            l.commit();
            for block in 0..100 {
                let (a, b): (Vec<f32>, Vec<f32>) = (0..1323).map(|i| sig(k, block * 1323 + i)).unzip();
                l.an.push(&l.st.clone(), &a, &b, TICK);
            }
            render(&mut l, &dir.join(format!("drop_{name}.png")));
        }
    }
}
