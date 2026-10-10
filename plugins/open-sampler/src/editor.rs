//! The plugin editor window: a Win32 child of the host's window that paints `view` with the os-ui canvas and turns
//! mouse input into parameter edits through nih-plug's `GuiContext`.
//!
// spec: plugins/plugin-formats.md (editors), plugins/open-sampler.md (Interaction), ui/knobs.md (drag / wheel)

use std::any::Any;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use nih_plug::prelude::*;

use crate::params::SamplerParams;
use crate::view::{self, Model, SampleView, Target, ViewState};
use crate::Shared;

pub struct SamplerEditor {
    pub params: Arc<SamplerParams>,
    pub shared: Arc<Shared>,
    /// DPI scale as f32 bits.
    pub scale: AtomicU32,
}

impl SamplerEditor {
    pub fn new(params: Arc<SamplerParams>, shared: Arc<Shared>) -> Self {
        SamplerEditor { params, shared, scale: AtomicU32::new(1f32.to_bits()) }
    }
}

impl Editor for SamplerEditor {
    fn spawn(&self, parent: ParentWindowHandle, context: Arc<dyn GuiContext>) -> Box<dyn Any + Send> {
        let scale = f32::from_bits(self.scale.load(Ordering::Relaxed));
        match parent {
            #[cfg(windows)]
            ParentWindowHandle::Win32Hwnd(hwnd) => Box::new(win32::open(hwnd, scale, self.params.clone(), self.shared.clone(), context)),
            _ => Box::new(()),
        }
    }

    fn size(&self) -> (u32, u32) {
        (view::EDITOR_W, view::EDITOR_H)
    }

    fn set_scale_factor(&self, factor: f32) -> bool {
        self.scale.store(factor.to_bits(), Ordering::Relaxed);
        true
    }

    fn param_value_changed(&self, _id: &str, _normalized_value: f32) {}
    fn param_modulation_changed(&self, _id: &str, _modulation_offset: f32) {}
    fn param_values_changed(&self) {}
}

/// Parameter lookup by id for the view.
pub struct Ptrs(pub HashMap<String, ParamPtr>);

impl Ptrs {
    pub fn new(params: &SamplerParams) -> Self {
        Ptrs(params.param_map().into_iter().map(|(id, p, _)| (id, p)).collect())
    }
}

impl Model for Ptrs {
    fn norm(&self, id: &str) -> f32 {
        // SAFETY: the pointers come from the params object, which outlives the editor.
        self.0.get(id).map_or(0.0, |p| unsafe { p.modulated_normalized_value() })
    }
    fn plain(&self, id: &str) -> f32 {
        self.0.get(id).map_or(0.0, |p| unsafe { p.modulated_plain_value() })
    }
    fn text(&self, id: &str) -> String {
        self.0.get(id).map_or_else(String::new, |p| unsafe { p.normalized_value_to_string(p.modulated_normalized_value(), true) })
    }
    fn name(&self, id: &str) -> String {
        self.0.get(id).map_or_else(String::new, |p| unsafe { p.name().to_string() })
    }
}

/// What the mouse is doing (editor logic, independent of the window system).
#[derive(Clone, Debug)]
enum Drag {
    Knob { id: String, y0: f32, v0: f32 },
    Spin { id: String, y0: f32, v0: f32 },
    Slider { id: String, track: os_ui::Rect },
    KeyScroll { x0: f32, s0: f32 },
}

/// Editor behaviour: hit testing and parameter edits.
pub struct Logic {
    pub ptrs: Ptrs,
    pub ctx: Arc<dyn GuiContext>,
    pub view: ViewState,
    pub hits: Vec<view::Hit>,
    drag: Option<Drag>,
    /// Hint text shown while hovering / dragging a control.
    pub hint: String,
}

pub enum Effect {
    None,
    Repaint,
    OpenFile,
    ClearFile,
}

impl Logic {
    pub fn new(ptrs: Ptrs, ctx: Arc<dyn GuiContext>) -> Self {
        Logic { ptrs, ctx, view: ViewState::default(), hits: Vec::new(), drag: None, hint: String::new() }
    }

    fn at(&self, x: f32, y: f32) -> Option<Target> {
        self.hits.iter().rev().find(|h| h.r.contains(x, y)).map(|h| h.t.clone())
    }

    fn begin(&self, id: &str) {
        if let Some(p) = self.ptrs.0.get(id) {
            unsafe { self.ctx.raw_begin_set_parameter(*p) };
        }
    }
    fn set(&self, id: &str, norm: f32) {
        if let Some(p) = self.ptrs.0.get(id) {
            unsafe { self.ctx.raw_set_parameter_normalized(*p, norm.clamp(0.0, 1.0)) };
        }
    }
    fn end(&self, id: &str) {
        if let Some(p) = self.ptrs.0.get(id) {
            unsafe { self.ctx.raw_end_set_parameter(*p) };
        }
    }
    /// One complete edit (begin, set, end).
    fn edit(&self, id: &str, norm: f32) {
        self.begin(id);
        self.set(id, norm);
        self.end(id);
    }
    fn plain_to_norm(&self, id: &str, plain: f32) -> f32 {
        self.ptrs.0.get(id).map_or(0.0, |p| unsafe { p.preview_normalized(plain) })
    }
    fn step(&self, id: &str, from: f32, up: bool) -> f32 {
        self.ptrs.0.get(id).map_or(from, |p| unsafe {
            if up { p.next_normalized_step(from, false) } else { p.previous_normalized_step(from, false) }
        })
    }
    fn default(&self, id: &str) -> f32 {
        self.ptrs.0.get(id).map_or(0.0, |p| unsafe { p.default_normalized_value() })
    }

    fn set_hint(&mut self, id: &str) {
        self.hint = format!("{}: {}", self.ptrs.name(id), self.ptrs.text(id));
    }

    pub fn down(&mut self, x: f32, y: f32, right: bool, double: bool) -> Effect {
        let Some(t) = self.at(x, y) else { return Effect::None };
        // The right button only steps choices backwards: it never starts a drag (there is no right-button-up
        // handling, so a drag started here would never end).
        if right && !matches!(t, Target::Cycle(_)) {
            return Effect::None;
        }
        match t {
            Target::Knob(id) if double => {
                self.edit(&id, self.default(&id));
                self.set_hint(&id);
            }
            Target::Knob(id) => {
                self.begin(&id);
                self.drag = Some(Drag::Knob { v0: self.ptrs.norm(&id), id, y0: y });
            }
            Target::Spin(id) => {
                self.begin(&id);
                self.drag = Some(Drag::Spin { v0: self.ptrs.norm(&id), id, y0: y });
            }
            Target::Slider(id, track) => {
                self.begin(&id);
                self.set(&id, (x - track.x) / track.w);
                self.set_hint(&id);
                self.drag = Some(Drag::Slider { id, track });
            }
            Target::Toggle(id) => {
                let v = self.ptrs.norm(&id);
                self.edit(&id, if v > 0.5 { 0.0 } else { 1.0 });
            }
            Target::Set(id, plain) => self.edit(&id, self.plain_to_norm(&id, plain)),
            Target::Cycle(id) => {
                let v = self.ptrs.norm(&id);
                let mut n = self.step(&id, v, !right);
                if n == v {
                    // Wrap around at either end.
                    n = if right { 1.0 } else { 0.0 };
                }
                self.edit(&id, n);
                self.set_hint(&id);
            }
            Target::Tab(i) => self.view.tab = i,
            Target::SubTab(i) => self.view.sub = i,
            Target::FileOpen => return Effect::OpenFile,
            Target::FileClear => return Effect::ClearFile,
            Target::Key(k) => {
                self.edit("root", self.plain_to_norm("root", k as f32));
                self.set_hint("root");
            }
            Target::RootReset => {
                self.edit("root", self.default("root"));
                self.edit("fine", self.default("fine"));
            }
            Target::KeyScroll => self.drag = Some(Drag::KeyScroll { x0: x, s0: self.view.key_scroll }),
        }
        Effect::Repaint
    }

    pub fn middle(&mut self, x: f32, y: f32) -> Effect {
        match self.at(x, y).and_then(|t| t.param().map(str::to_string)) {
            Some(id) => {
                self.edit(&id, self.default(&id));
                self.set_hint(&id);
                Effect::Repaint
            }
            None => Effect::None,
        }
    }

    pub fn moved(&mut self, x: f32, y: f32) -> Effect {
        match self.drag.clone() {
            Some(Drag::Knob { id, y0, v0 }) => {
                // 1/1024 of the travel per pixel (spec: ui/knobs.md).
                self.set(&id, v0 + (y0 - y) / 1024.0);
                self.set_hint(&id);
                Effect::Repaint
            }
            Some(Drag::Spin { id, y0, v0 }) => {
                let steps = ((y0 - y) / 8.0).trunc() as i32;
                let mut v = v0;
                for _ in 0..steps.unsigned_abs() {
                    v = self.step(&id, v, steps > 0);
                }
                self.set(&id, v);
                self.set_hint(&id);
                Effect::Repaint
            }
            Some(Drag::Slider { id, track }) => {
                self.set(&id, (x - track.x) / track.w);
                self.set_hint(&id);
                Effect::Repaint
            }
            Some(Drag::KeyScroll { x0, s0 }) => {
                self.view.key_scroll = (s0 + (x - x0) * 2.0).clamp(0.0, view::keyboard_max_scroll());
                Effect::Repaint
            }
            None => {
                let hint = match self.at(x, y).as_ref().and_then(Target::param) {
                    Some(id) => format!("{}: {}", self.ptrs.name(id), self.ptrs.text(id)),
                    None => String::new(),
                };
                if hint != self.hint {
                    self.hint = hint;
                    Effect::Repaint
                } else {
                    Effect::None
                }
            }
        }
    }

    pub fn up(&mut self) -> Effect {
        match self.drag.take() {
            Some(Drag::Knob { id, .. } | Drag::Spin { id, .. } | Drag::Slider { id, .. }) => {
                self.end(&id);
                Effect::Repaint
            }
            Some(Drag::KeyScroll { .. }) => Effect::Repaint,
            None => Effect::None,
        }
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Wheel: 1/64 of a knob's travel per notch, one step on spin boxes and choices.
    pub fn wheel(&mut self, x: f32, y: f32, notches: f32) -> Effect {
        match self.at(x, y) {
            Some(Target::Knob(id) | Target::Slider(id, _)) => {
                self.edit(&id, self.ptrs.norm(&id) + notches / 64.0);
                self.set_hint(&id);
                Effect::Repaint
            }
            Some(Target::Spin(id) | Target::Cycle(id)) => {
                let v = self.step(&id, self.ptrs.norm(&id), notches > 0.0);
                self.edit(&id, v);
                self.set_hint(&id);
                Effect::Repaint
            }
            Some(Target::Key(_) | Target::KeyScroll) => {
                self.view.key_scroll = (self.view.key_scroll - notches * 40.0).clamp(0.0, view::keyboard_max_scroll());
                Effect::Repaint
            }
            _ => Effect::None,
        }
    }

    /// A cheap fingerprint of all parameter values (repaint when it changes).
    pub fn signature(&self) -> u64 {
        let mut h: u64 = 1469598103934665603;
        for p in self.ptrs.0.values() {
            let v = unsafe { p.modulated_normalized_value() }.to_bits();
            h = (h ^ u64::from(v)).wrapping_mul(1099511628211);
        }
        h
    }
}

/// The current sample for the waveform: the processed one when it matches the loaded one, else the raw sample.
pub fn with_sample<R>(shared: &Shared, params: &SamplerParams, f: impl FnOnce(SampleView) -> R) -> R {
    let path = params.sample_path.read().map(|p| p.clone()).unwrap_or_default();
    let file = path.rsplit(['/', '\\']).next().unwrap_or("").to_string();
    let processed = shared.processed.lock().ok().and_then(|p| p.as_ref().and_then(|p| p.sample.clone()));
    let raw = shared.raw.lock().ok().and_then(|r| r.clone());
    let s = processed.or(raw);
    f(SampleView { sample: s.as_deref(), file: &file })
}

#[cfg(windows)]
mod win32 {
    use std::ffi::c_void;
    use std::sync::Arc;

    use nih_plug::prelude::GuiContext;
    use os_ui::Canvas;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{
        BeginPaint, EndPaint, InvalidateRect, SetDIBitsToDevice, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, PAINTSTRUCT,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OPENFILENAMEW};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, SetFocus};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    use super::{Effect, Logic, Ptrs};
    use crate::params::SamplerParams;
    use crate::view::{self, ViewState};
    use crate::Shared;

    const TIMER: usize = 1;

    struct Win {
        hwnd: HWND,
        canvas: Canvas,
        buf: Vec<u32>,
        crop: Vec<u32>,
        scale: f32,
        size: (u32, u32),
        /// The host's strip row carries our page tabs (host link, spec `plugins/host-link.md`).
        hosted: bool,
        params: Arc<SamplerParams>,
        shared: Arc<Shared>,
        logic: Logic,
        last_sig: u64,
        last_view: ViewState,
        last_ver: u64,
    }

    /// Owns the window; dropping it closes the editor.
    pub struct Handle(isize);

    // SAFETY: the handle is only an HWND value; the wrapper drops it on the GUI thread.
    unsafe impl Send for Handle {}

    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe { DestroyWindow(self.0 as HWND) };
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    /// One class per DLL instance: the name carries this module's address so two OpenStudio plugins never share it.
    fn class_name() -> Vec<u16> {
        wide(&format!("OpenSamplerView{:x}", wndproc as *const () as usize))
    }

    pub fn open(parent: *mut c_void, scale: f32, params: Arc<SamplerParams>, shared: Arc<Shared>, ctx: Arc<dyn GuiContext>) -> Handle {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let size = ((view::EDITOR_W as f32 * scale).round() as u32, (view::EDITOR_H as f32 * scale).round() as u32);
        let full = ((view::W as f32 * scale).round() as u32, (view::H as f32 * scale).round() as u32);
        unsafe {
            let inst = GetModuleHandleW(std::ptr::null());
            let class = class_name();
            let wc = WNDCLASSW {
                style: CS_DBLCLKS,
                lpfnWndProc: Some(wndproc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: inst,
                hIcon: std::ptr::null_mut(),
                hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
                hbrBackground: std::ptr::null_mut(),
                lpszMenuName: std::ptr::null(),
                lpszClassName: class.as_ptr(),
            };
            RegisterClassW(&wc);
            let ptrs = Ptrs::new(&params);
            let canvas = Canvas::new(full.0, full.1, scale).expect("canvas");
            let win = Box::new(Win {
                hwnd: std::ptr::null_mut(),
                canvas,
                buf: vec![0; (full.0 * full.1) as usize],
                crop: Vec::new(),
                scale,
                size: full,
                hosted: false,
                params,
                shared,
                logic: Logic::new(ptrs, ctx),
                last_sig: 0,
                last_view: ViewState::default(),
                last_ver: 0,
            });
            let title = wide("Open Sampler");
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
                inst,
                Box::into_raw(win) as *const c_void,
            );
            SetTimer(hwnd, TIMER, 30, None);
            Handle(hwnd as isize)
        }
    }

    unsafe fn state(hwnd: HWND) -> Option<&'static mut Win> {
        let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Win;
        if p.is_null() { None } else { Some(unsafe { &mut *p }) }
    }

    /// Window-client pixels to painted-frame coordinates (the editor is a crop of the painted frame).
    fn mouse(l: LPARAM, w: &Win) -> (f32, f32) {
        let x = (l & 0xffff) as u16 as i16 as f32;
        let y = ((l >> 16) & 0xffff) as u16 as i16 as f32;
        (x / w.scale + view::CROP_X as f32, y / w.scale + w.crop_y() as f32)
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
                    let sig = w.logic.signature();
                    let ver = w.shared.version();
                    if sig != w.last_sig || ver != w.last_ver || w.logic.view != w.last_view {
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                    }
                    0
                }
                WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_RBUTTONDOWN => {
                    SetFocus(hwnd);
                    let (x, y) = mouse(lp, w);
                    let e = w.logic.down(x, y, msg == WM_RBUTTONDOWN, msg == WM_LBUTTONDBLCLK);
                    if w.logic.dragging() {
                        SetCapture(hwnd);
                    }
                    w.effect(e);
                    0
                }
                WM_MBUTTONDOWN => {
                    let (x, y) = mouse(lp, w);
                    let e = w.logic.middle(x, y);
                    w.effect(e);
                    0
                }
                WM_MOUSEMOVE => {
                    let (x, y) = mouse(lp, w);
                    let e = w.logic.moved(x, y);
                    w.effect(e);
                    0
                }
                WM_LBUTTONUP => {
                    ReleaseCapture();
                    let e = w.logic.up();
                    w.effect(e);
                    0
                }
                WM_RBUTTONUP | WM_KILLFOCUS => {
                    ReleaseCapture();
                    let e = w.logic.up();
                    w.effect(e);
                    0
                }
                WM_CAPTURECHANGED => {
                    let e = w.logic.up();
                    w.effect(e);
                    0
                }
                WM_MOUSEWHEEL => {
                    // Wheel positions are screen coordinates.
                    let mut r: RECT = std::mem::zeroed();
                    GetWindowRect(hwnd, &mut r);
                    let sx = (lp & 0xffff) as u16 as i16 as i32 - r.left;
                    let sy = ((lp >> 16) & 0xffff) as u16 as i16 as i32 - r.top;
                    let notches = ((wp >> 16) & 0xffff) as u16 as i16 as f32 / 120.0;
                    let e = w.logic.wheel(sx as f32 / w.scale + view::CROP_X as f32, sy as f32 / w.scale + w.crop_y() as f32, notches);
                    w.effect(e);
                    0
                }
                WM_DESTROY => {
                    KillTimer(hwnd, TIMER);
                    0
                }
                _ => DefWindowProcW(hwnd, msg, wp, lp),
            }
        }
    }

    impl Win {
        /// Top of the shown crop in the painted frame: the strip row is left out when the host draws it.
        fn crop_y(&self) -> u32 {
            if self.hosted { view::HOSTED_CROP_Y } else { view::CROP_Y }
        }

        /// Editor content size in logical px.
        fn content(&self) -> (u32, u32) {
            (view::EDITOR_W, if self.hosted { view::HOSTED_EDITOR_H } else { view::EDITOR_H })
        }

        /// Answers the host (spec `plugins/host-link.md`). Unknown commands return 0.
        unsafe fn host_link(&mut self, wp: WPARAM, lp: LPARAM) -> LRESULT {
            use os_hostlink::{Cmd, PageIcon};
            let Some(cmd) = Cmd::from_usize(wp) else { return 0 };
            match cmd {
                Cmd::Hello => {
                    if !self.hosted {
                        self.hosted = true;
                        // Shrink to the content below the strip row; the host asks for the size next.
                        let (cw, ch) = self.content();
                        let px = |v: u32| (v as f32 * self.scale).round() as i32;
                        unsafe {
                            SetWindowPos(self.hwnd, std::ptr::null_mut(), 0, 0, px(cw), px(ch), SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
                            InvalidateRect(self.hwnd, std::ptr::null(), 0);
                        }
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
                    unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) };
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
            }
        }

        fn effect(&mut self, e: Effect) {
            match e {
                Effect::None => return,
                Effect::Repaint => {}
                Effect::OpenFile => {
                    if let Some(path) = open_dialog(self.hwnd) {
                        if let Err(msg) = self.shared.load(&path, &self.params) {
                            self.logic.hint = msg;
                        }
                    }
                }
                Effect::ClearFile => self.shared.clear(&self.params),
            }
            unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) };
        }

        fn paint(&mut self) {
            unsafe {
                let mut ps: PAINTSTRUCT = std::mem::zeroed();
                let hdc = BeginPaint(self.hwnd, &mut ps);
                let hits = super::with_sample(&self.shared, &self.params, |sv| {
                    view::paint(&mut self.canvas, &self.logic.ptrs, &self.logic.view, &sv, &self.logic.hint, self.hosted)
                });
                self.logic.hits = hits;
                self.last_sig = self.logic.signature();
                self.last_view = self.logic.view.clone();
                self.last_ver = self.shared.version();
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

    fn open_dialog(owner: HWND) -> Option<String> {
        let mut file = vec![0u16; 1024];
        let filter: Vec<u16> = "Audio files\0*.wav;*.flac;*.ogg;*.wv\0All files\0*.*\0\0".encode_utf16().collect();
        let title = wide("Open sample");
        unsafe {
            let mut ofn: OPENFILENAMEW = std::mem::zeroed();
            ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
            ofn.hwndOwner = owner;
            ofn.lpstrFilter = filter.as_ptr();
            ofn.lpstrFile = file.as_mut_ptr();
            ofn.nMaxFile = file.len() as u32;
            ofn.lpstrTitle = title.as_ptr();
            ofn.Flags = OFN_FILEMUSTEXIST | OFN_NOCHANGEDIR;
            if GetOpenFileNameW(&mut ofn) == 0 {
                return None;
            }
        }
        let end = file.iter().position(|&c| c == 0).unwrap_or(file.len());
        Some(String::from_utf16_lossy(&file[..end]))
    }
}
