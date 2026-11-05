// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The editor on Windows: a child window on the host's GUI thread (`os_plugkit::win`) painting `zest_ui`, what `gui` is
//! on Linux and macOS. The host link goes through window messages here (spec: `plugins/host-link.md`).

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::Arc;

use nih_plug::prelude::GuiContext;
use os_hostlink::{Cmd, PageIcon};
use os_plugkit::win::{self as pw, CursorLock};
use os_plugkit::MenuDef;
use os_theme::Palette;
use os_ui::kit::menu::MenuStyle;
use os_ui::Canvas;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::WM_MOUSELEAVE;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_BACK, VK_DOWN, VK_ESCAPE, VK_RETURN, VK_UP};
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use zest_ui::view;
use zest_ui::{Act, Btn, Mods, Ui};

use super::{PluginModel, ZestParams};
use crate::PendingSlop;

const TIMER: usize = 1;

/// The page tabs the host shows: Main (plug), Effects (waveform), Advanced (node graph).
const PAGE_ICONS: [PageIcon; 3] = [PageIcon::Plugin, PageIcon::Sample, PageIcon::Envelope];

struct Win {
    hwnd: HWND,
    canvas: Canvas,
    buf: Vec<u32>,
    scale: f32,
    /// Window size in device px.
    dev: (u32, u32),
    /// The size in logical px now.
    size: (u32, u32),
    /// The size asked for (logical, standalone), last applied.
    asked: (u32, u32),
    params: Arc<ZestParams>,
    pending: PendingSlop,
    scope: Arc<crate::scope::Scope>,
    bands: crate::scope::Bands,
    model: PluginModel,
    ui: Ui,
    palette: Palette,
    last_sig: u64,
    last_hint: String,
    last_page: usize,
    lock: CursorLock,
    tracking: bool,
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

fn mods() -> Mods {
    let (ctrl, shift, alt) = pw::mods();
    Mods { ctrl, shift, alt }
}

pub fn open(parent: *mut c_void, scale: f32, params: Arc<ZestParams>, pending: PendingSlop, scope: Arc<crate::scope::Scope>, ctx: Arc<dyn GuiContext>) -> Handle {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    let memo = params.editor.read().map(|m| m.clone()).unwrap_or_default();
    let (w, h) = super::clamp_size(memo.w, memo.h);
    let dev = ((w as f32 * scale).round() as u32, (h as f32 * scale).round() as u32);
    let model = PluginModel::new(&params, Some(ctx));
    let mut ui = Ui::with_defaults(super::table());
    ui.set_page(memo.page);
    if let Some(p) = &memo.preset {
        ui.set_current(Some(std::path::Path::new(p)), None);
    }
    unsafe {
        let class = pw::register("ZestView", Some(wndproc));
        let mut win = Box::new(Win {
            hwnd: std::ptr::null_mut(),
            canvas: Canvas::new(dev.0, dev.1, scale).expect("canvas"),
            buf: vec![0; (dev.0 * dev.1) as usize],
            scale,
            dev,
            size: (w, h),
            asked: (w, h),
            params,
            pending,
            scope,
            bands: Default::default(),
            model,
            ui,
            palette: os_plugkit::palette(),
            last_sig: 0,
            last_hint: String::new(),
            last_page: memo.page,
            lock: CursorLock::default(),
            tracking: false,
        });
        win.relayout();
        let title = pw::wide("Zest");
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
            0,
            0,
            dev.0 as i32,
            dev.1 as i32,
            parent as HWND,
            std::ptr::null_mut(),
            GetModuleHandleW(std::ptr::null()),
            Box::into_raw(win) as *const c_void,
        );
        SetTimer(hwnd, TIMER, 30, None);
        Handle(hwnd as isize)
    }
}

unsafe fn state(hwnd: HWND) -> Option<&'static mut Win> {
    let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Win;
    if p.is_null() {
        None
    } else {
        Some(unsafe { &mut *p })
    }
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
                w.poll();
                0
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_RBUTTONDOWN | WM_RBUTTONDBLCLK | WM_MBUTTONDOWN => {
                SetFocus(hwnd);
                SetCapture(hwnd);
                let (x, y) = w.mouse(lp);
                let btn = match msg {
                    WM_RBUTTONDOWN | WM_RBUTTONDBLCLK => Btn::Right,
                    WM_MBUTTONDOWN => Btn::Middle,
                    _ => Btn::Left,
                };
                let acts = w.ui.down(x, y, btn, mods(), msg == WM_LBUTTONDBLCLK, &mut w.model);
                w.act(acts);
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
                        let k = w.k();
                        let acts = w.ui.delta(dx as f32 / k, dy as f32 / k, mods(), &mut w.model);
                        w.act(acts);
                    }
                    return 0;
                }
                let (x, y) = w.mouse(lp);
                let acts = w.ui.moved(x, y, &w.model);
                w.act(acts);
                0
            }
            WM_MOUSELEAVE => {
                w.tracking = false;
                if !w.lock.locked() && !w.ui.dragging() {
                    let acts = w.ui.leave();
                    w.act(acts);
                }
                0
            }
            WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP => {
                w.lock.unlock();
                ReleaseCapture();
                let acts = w.ui.up(&mut w.model);
                w.act(acts);
                0
            }
            WM_CAPTURECHANGED => {
                // Another window took the mouse (not this one capturing again): end the drag.
                if w.ui.dragging() && lp as HWND != hwnd {
                    w.lock.unlock();
                    let acts = w.ui.up(&mut w.model);
                    w.act(acts);
                }
                0
            }
            WM_MOUSEWHEEL => {
                // Wheel positions are screen coordinates.
                let mut r: RECT = std::mem::zeroed();
                GetWindowRect(hwnd, &mut r);
                let (sx, sy) = pw::xy(lp);
                let k = w.k();
                let notches = ((wp >> 16) & 0xffff) as u16 as i16 as f32 / 120.0;
                let acts = w.ui.wheel((sx - r.left) as f32 / k, (sy - r.top) as f32 / k, notches, mods(), &mut w.model);
                w.act(acts);
                0
            }
            WM_KEYDOWN => {
                let Some(sym) = keysym(wp as u16) else { return DefWindowProcW(hwnd, msg, wp, lp) };
                let acts = w.ui.key(sym, None);
                if acts.is_empty() {
                    return DefWindowProcW(hwnd, msg, wp, lp);
                }
                w.act(acts);
                0
            }
            WM_CHAR => {
                let Some(c) = char::from_u32(wp as u32).filter(|c| !c.is_control()) else { return DefWindowProcW(hwnd, msg, wp, lp) };
                let acts = w.ui.key(c as u32, Some(c));
                if acts.is_empty() {
                    return DefWindowProcW(hwnd, msg, wp, lp);
                }
                w.act(acts);
                0
            }
            WM_DESTROY => {
                KillTimer(hwnd, TIMER);
                w.lock.unlock();
                w.remember();
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// The X11 key symbol `zest_ui` takes for a key that types no character.
fn keysym(vk: u16) -> Option<u32> {
    match vk {
        VK_BACK => Some(0xff08),
        VK_RETURN => Some(0xff0d),
        VK_ESCAPE => Some(0xff1b),
        VK_UP => Some(0xff52),
        VK_DOWN => Some(0xff54),
        _ => None,
    }
}

impl Win {
    /// The canvas's scale: device px per layout px (the zoom of small windows included).
    fn k(&self) -> f32 {
        let (z, _, _) = view::fit(self.dev.0 as f32 / self.scale, self.dev.1 as f32 / self.scale, self.ui.hosted);
        self.scale * z
    }

    fn mouse(&self, l: LPARAM) -> (f32, f32) {
        let (x, y) = pw::xy(l);
        let k = self.k();
        (x as f32 / k, y as f32 / k)
    }

    fn screen(&self, x: f32, y: f32) -> (i32, i32) {
        let k = self.k();
        pw::to_screen(self.hwnd, (x * k) as i32, (y * k) as i32)
    }

    fn invalidate(&self) {
        unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) };
    }

    /// Lays out for the window's size now.
    fn relayout(&mut self) {
        let (z, lw, lh) = view::fit(self.dev.0 as f32 / self.scale, self.dev.1 as f32 / self.scale, self.ui.hosted);
        self.ui.size = (lw, lh);
        self.canvas.resize(self.dev.0, self.dev.1, self.scale * z);
    }

    /// Resizes the window to `w` x `h` logical px.
    fn resize(&mut self, w: u32, h: u32) {
        self.size = (w, h);
        let dev = ((w as f32 * self.scale).round() as u32, (h as f32 * self.scale).round() as u32);
        if dev != self.dev {
            self.dev = dev;
            unsafe { SetWindowPos(self.hwnd, std::ptr::null_mut(), 0, 0, dev.0 as i32, dev.1 as i32, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE) };
        }
        self.relayout();
        self.invalidate();
    }

    /// Keeps the page and the preset in the plugin's state.
    fn remember(&self) {
        if let Ok(mut m) = self.params.editor.write() {
            m.page = self.ui.page;
            m.preset = self.ui.current_path().map(|p| p.to_string_lossy().into_owned());
        }
    }

    fn tell_host(&self) {
        if self.ui.hosted {
            os_hostlink::to_host(self.hwnd as isize, Cmd::Changed, 0);
        }
    }

    /// Timer: values changed elsewhere, the hint and page for the host, a size the host asked for.
    fn poll(&mut self) {
        let mut dirty = false;
        // The visualizer: a repaint only while its columns move (none while silent).
        let (levels, pitch, dt) = self.bands.next(&self.scope);
        dirty |= self.ui.viz.step(&levels, pitch, dt);
        dirty |= self.model.vals.expire();
        let sig = self.model.vals.signature();
        if sig != self.last_sig {
            self.last_sig = sig;
            dirty = true;
        }
        if self.ui.line() != self.last_hint {
            self.last_hint = self.ui.line().to_string();
            self.tell_host();
        }
        if self.ui.page != self.last_page {
            self.last_page = self.ui.page;
            self.remember();
            self.tell_host();
        }
        // The host resized the window (set_size).
        let memo = self.params.editor.read().map(|m| (m.w, m.h)).unwrap_or(self.asked);
        if memo != self.asked && !self.ui.hosted {
            self.asked = memo;
            self.resize(memo.0, memo.1);
        }
        if dirty {
            self.invalidate();
        }
    }

    fn act(&mut self, acts: Vec<Act>) {
        let mut repaint = false;
        for a in acts {
            match a {
                Act::Repaint => repaint = true,
                Act::Lock => self.lock.lock(self.hwnd),
                Act::Page(_) => {
                    self.remember();
                    repaint = true;
                }
                Act::Menu(req, x, y) => {
                    let mut def = MenuDef::new(MenuStyle::List);
                    for (i, it) in req.items.iter().enumerate() {
                        def.add(it.clone(), i);
                    }
                    def.current = req.current;
                    let (sx, sy) = self.screen(x, y);
                    if let Some(row) = pw::popup(self.hwnd, sx, sy, &def, &self.palette, self.scale) {
                        let acts = self.ui.pick(&req, row, &mut self.model);
                        self.act(acts);
                    }
                    repaint = true;
                }
                Act::TypeIn(t, text, x, y) => {
                    let (sx, sy) = self.screen(x, y);
                    if let Some(s) = pw::type_in(self.hwnd, sx, sy, &text, self.scale) {
                        let acts = self.ui.typed(&t, &s, &mut self.model);
                        self.act(acts);
                    }
                    repaint = true;
                }
                Act::Load(path) => {
                    match super::load(&path, &mut self.model, &self.pending) {
                        Ok(got) => {
                            if let Ok(mut m) = self.params.editor.write() {
                                m.spread = Some(super::spread_text(got.spread));
                            }
                            self.ui.loaded(&path, got.author)
                        }
                        Err(e) => self.ui.load_failed(&path, &e),
                    }
                    self.remember();
                    repaint = true;
                }
                Act::Save(path) => {
                    let path: PathBuf = path;
                    let spread = self.params.editor.read().ok().and_then(|m| m.spread.as_deref().and_then(|t| zest_preset::parse(t).ok())).map(|f| f.spread).unwrap_or_default();
                    match super::save(&path, &self.model, spread) {
                        Ok(()) => self.ui.saved(&path),
                        Err(e) => self.ui.save_failed(&e),
                    }
                    self.remember();
                    repaint = true;
                }
            }
        }
        if repaint {
            self.invalidate();
        }
    }

    /// Answers the host (spec `plugins/host-link.md`): the pages, the hint line. Unknown commands return 0.
    unsafe fn host_link(&mut self, wp: WPARAM, lp: LPARAM) -> LRESULT {
        let Some(cmd) = Cmd::from_usize(wp) else { return 0 };
        match cmd {
            Cmd::Hello => {
                // Joined Lemon Studio's strip: the editor leaves its tab row and foot to the host and shrinks by them.
                if !self.ui.hosted {
                    self.ui.hosted = true;
                    let (w, h) = self.asked;
                    self.resize(w, h - (view::BASE_H - view::base_h(true)) as u32);
                }
                os_hostlink::VERSION as LRESULT
            }
            Cmd::PageCount => PAGE_ICONS.len() as LRESULT,
            Cmd::PageIcon => PAGE_ICONS.get(lp as usize).copied().unwrap_or(PageIcon::Plugin) as LRESULT,
            Cmd::GetPage => self.ui.page as LRESULT,
            Cmd::SetPage => {
                self.ui.set_page((lp.max(0) as usize).min(PAGE_ICONS.len() - 1));
                self.last_page = self.ui.page;
                self.remember();
                self.invalidate();
                self.ui.page as LRESULT
            }
            Cmd::Metrics => os_hostlink::pack_size(self.size.0, self.size.1),
            Cmd::Hint => {
                if lp != 0 {
                    // SAFETY: the host passes a `HintBuf` of its own process (spec: host-link.md).
                    unsafe { &mut *(lp as *mut os_hostlink::HintBuf) }.set(self.ui.line());
                }
                1
            }
            Cmd::Changed | Cmd::Note | Cmd::Automate | Cmd::Audio | Cmd::Resizable | Cmd::SetSize | Cmd::Notes => 0,
        }
    }

    fn paint(&mut self) {
        let (hdc, ps) = pw::begin_paint(self.hwnd);
        self.ui.paint(&mut self.canvas, &self.model);
        self.buf.resize((self.dev.0 * self.dev.1) as usize, 0);
        self.canvas.copy_to_xrgb(&mut self.buf);
        pw::blit(hdc, &self.buf, self.dev.0, self.dev.1);
        pw::end_paint(self.hwnd, &ps);
    }
}
