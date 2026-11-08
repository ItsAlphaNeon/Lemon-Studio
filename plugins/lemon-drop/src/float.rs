// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The floating display (Win32): the display in a borderless window of its own on the screen, at the state's window
//! rectangle, see-through wherever the display paints nothing (a layered window with per-pixel alpha, so with
//! Background off only the picture floats over the desktop). Its body drags it and its lower right corner sizes it;
//! the new rectangle is kept in the state. Right click: the display menu; middle click: hold.
//!
//! It is opened by the editor's tick on the host's UI thread when the state is Visible, then lives on its own after
//! the editor closes, until Visible goes off or the plugin goes. Keep in front: above every window. On desktop: not
//! owned by the host's window, so it stays when the host is minimised. Solid off: clicks go through it. Mini view:
//! the mini rectangle.
//!
// spec: plugins/lemon-drop.md "Floating display"

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use os_plugkit::win as pw;
use os_theme::Palette;
use os_ui::{Canvas, Rect};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, GetMonitorInfoW, HBITMAP, HDC, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTONULL, MONITORINFO, MonitorFromRect, MonitorFromWindow,
    ScreenToClient, SelectObject,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Controls::WM_MOUSELEAVE;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::editor::{Logic, MenuReq, SeeThrough, TIMER_MS};
use crate::state::Rect4;
use crate::{DropParams, Shared};

const TIMER: usize = 1;
/// The lower right corner that sizes the window, device px.
const GRIP: i32 = 14;
/// The smallest window side, logical px.
const MIN_SIDE: f32 = 24.0;

/// Opens the floating display when the state is Visible and it is not open. On the host's UI thread (the editor's
/// tick); `editor` is the editor's window, whose top owner owns the floating one.
pub fn ensure(params: &Arc<DropParams>, shared: &Arc<Shared>, scale: f32, editor: HWND) {
    if shared.float.load(Ordering::Acquire) != 0 {
        return;
    }
    if !params.current().1.visible {
        return;
    }
    // SAFETY: plain Win32 calls on this thread; the window is ours.
    unsafe { open(params.clone(), shared.clone(), scale, editor) };
}

/// Closes the floating display (the plugin is going). From any thread: the window's own thread destroys it.
pub fn close(shared: &Shared) {
    let h = shared.float.swap(0, Ordering::AcqRel) as HWND;
    if h.is_null() {
        return;
    }
    // SAFETY: `h` is our window or one already gone (the calls then fail harmlessly).
    unsafe {
        if GetWindowThreadProcessId(h, std::ptr::null_mut()) == GetCurrentThreadId() {
            DestroyWindow(h);
        } else {
            SendMessageTimeoutW(h, WM_CLOSE, 0, 0, SMTO_ABORTIFHUNG | SMTO_BLOCK, 2000, std::ptr::null_mut());
        }
    }
}

enum Drag {
    Move { from: POINT, at: RECT },
    Size { from: POINT, at: RECT },
}

struct Float {
    hwnd: HWND,
    logic: Logic,
    shared: Arc<Shared>,
    /// The host's top window: the owner unless On desktop.
    host: HWND,
    scale: f32,
    canvas: Canvas,
    dc: HDC,
    bitmap: HBITMAP,
    bits: *mut u32,
    /// The window's size, device px.
    size: (i32, i32),
    /// (On desktop, Keep in front, Solid) as last applied.
    styled: Option<(bool, bool, bool)>,
    /// The state's rectangle the window was last put at.
    placed: Rect4,
    drag: Option<Drag>,
    tracking: bool,
    last: Instant,
    palette: Palette,
    /// The two paintings of a frame.
    buffers: SeeThrough,
}

impl Drop for Float {
    fn drop(&mut self) {
        // SAFETY: the memory DC and bitmap made for this window, no longer used.
        unsafe {
            DeleteDC(self.dc);
            DeleteObject(self.bitmap);
        }
    }
}

/// A rectangle in logical screen px as device px.
fn device(r: Rect4, scale: f32) -> RECT {
    let d = |v: i32| (v as f32 * scale).round() as i32;
    let min = (MIN_SIDE * scale).round() as i32;
    let (left, top) = (d(r[0]), d(r[1]));
    RECT { left, top, right: left + (d(r[2]) - left).max(min), bottom: top + (d(r[3]) - top).max(min) }
}

/// Device px as logical screen px.
fn logical(r: RECT, scale: f32) -> Rect4 {
    let l = |v: i32| (v as f32 / scale).round() as i32;
    [l(r.left), l(r.top), l(r.right), l(r.bottom)]
}

/// `r` moved onto a monitor when it is on none (a project from another screen layout): to the middle of `near`'s.
unsafe fn on_screen(r: RECT, near: HWND) -> RECT {
    unsafe {
        if !MonitorFromRect(&r, MONITOR_DEFAULTTONULL).is_null() {
            return r;
        }
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(MonitorFromWindow(near, MONITOR_DEFAULTTONEAREST), &mut mi) == 0 {
            return r;
        }
        let w = mi.rcWork;
        let (rw, rh) = (r.right - r.left, r.bottom - r.top);
        let (x, y) = (w.left + ((w.right - w.left) - rw).max(0) / 2, w.top + ((w.bottom - w.top) - rh).max(0) / 2);
        RECT { left: x, top: y, right: x + rw, bottom: y + rh }
    }
}

unsafe fn open(params: Arc<DropParams>, shared: Arc<Shared>, scale: f32, editor: HWND) {
    unsafe {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let logic = Logic::new(params, shared.clone());
        let placed = logic.window_rect();
        let r = on_screen(device(placed, scale), editor);
        let size = (r.right - r.left, r.bottom - r.top);
        let Ok(canvas) = Canvas::new(size.0 as u32, size.1 as u32, scale) else {
            return;
        };
        let host = GetAncestor(editor, GA_ROOTOWNER);
        let owner = if logic.st.on_desktop { std::ptr::null_mut() } else { host };
        let mut f = Box::new(Float {
            hwnd: std::ptr::null_mut(),
            logic,
            shared: shared.clone(),
            host,
            scale,
            canvas,
            dc: CreateCompatibleDC(std::ptr::null_mut()),
            bitmap: std::ptr::null_mut(),
            bits: std::ptr::null_mut(),
            size: (0, 0),
            styled: None,
            placed,
            drag: None,
            tracking: false,
            last: Instant::now(),
            palette: os_plugkit::palette(),
            buffers: SeeThrough::default(),
        });
        if f.dc.is_null() || !f.set_size(size) {
            return;
        }
        let class = pw::register("LemonDropFloat", Some(wndproc));
        let title = pw::wide(crate::NAME);
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            r.left,
            r.top,
            size.0,
            size.1,
            owner,
            std::ptr::null_mut(),
            GetModuleHandleW(std::ptr::null()),
            Box::into_raw(f) as *const std::ffi::c_void,
        );
        if hwnd.is_null() {
            return;
        }
        shared.float.store(hwnd as isize, Ordering::Release);
        if let Some(f) = state(hwnd) {
            f.restyle();
            f.render();
        }
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetTimer(hwnd, TIMER, TIMER_MS, None);
    }
}

unsafe fn state(hwnd: HWND) -> Option<&'static mut Float> {
    let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Float;
    if p.is_null() { None } else { Some(unsafe { &mut *p }) }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            let cs = lp as *const CREATESTRUCTW;
            let f = (*cs).lpCreateParams as *mut Float;
            (*f).hwnd = hwnd;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, f as isize);
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        if msg == WM_NCDESTROY {
            let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Float;
            if !p.is_null() {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(p));
            }
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        let Some(f) = state(hwnd) else {
            return DefWindowProcW(hwnd, msg, wp, lp);
        };
        match msg {
            WM_TIMER => {
                if !f.tick() {
                    DestroyWindow(hwnd);
                }
                0
            }
            WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                let (x, y) = pw::xy(lp);
                let (from, at) = (cursor(), window_rect(hwnd));
                let grip = x >= f.size.0 - GRIP && y >= f.size.1 - GRIP;
                f.drag = Some(if grip { Drag::Size { from, at } } else { Drag::Move { from, at } });
                SetCapture(hwnd);
                0
            }
            WM_MOUSEMOVE => {
                if !f.tracking {
                    let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                    TrackMouseEvent(&mut tme);
                    f.tracking = true;
                }
                f.dragged();
                if f.drag.is_none() {
                    let (x, y) = pw::xy(lp);
                    f.logic.point(Some((x as f32 / f.scale, y as f32 / f.scale)));
                }
                0
            }
            WM_MOUSELEAVE => {
                f.tracking = false;
                f.logic.point(None);
                0
            }
            WM_LBUTTONUP => {
                ReleaseCapture();
                f.dropped();
                0
            }
            WM_CAPTURECHANGED => {
                f.dropped();
                0
            }
            WM_RBUTTONUP => {
                let p = cursor();
                let (def, palette, scale) = (f.logic.menu(&MenuReq::Display), f.palette.clone(), f.scale);
                if let Some(cmd) = pw::popup(hwnd, p.x, p.y, &def, &palette, scale) {
                    // The window may be gone after a nested message loop: look it up again.
                    if let Some(f) = state(hwnd) {
                        f.logic.pick(&MenuReq::Display, cmd, 0.0, 0.0);
                    }
                }
                0
            }
            WM_MBUTTONDOWN => {
                f.logic.toggle_hold();
                0
            }
            WM_SETCURSOR => {
                let mut p = cursor();
                ScreenToClient(hwnd, &mut p);
                let grip = p.x >= f.size.0 - GRIP && p.y >= f.size.1 - GRIP;
                SetCursor(LoadCursorW(std::ptr::null_mut(), if grip { IDC_SIZENWSE } else { IDC_ARROW }));
                1
            }
            WM_CLOSE => {
                DestroyWindow(hwnd);
                0
            }
            WM_DESTROY => {
                KillTimer(hwnd, TIMER);
                let _ = f.shared.float.compare_exchange(hwnd as isize, 0, Ordering::AcqRel, Ordering::Acquire);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

fn cursor() -> POINT {
    let mut p = POINT { x: 0, y: 0 };
    // SAFETY: a plain query into a local.
    unsafe { GetCursorPos(&mut p) };
    p
}

fn window_rect(hwnd: HWND) -> RECT {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    // SAFETY: a plain query into a local.
    unsafe { GetWindowRect(hwnd, &mut r) };
    r
}

impl Float {
    /// The tick: the analysis, the state's window settings, a new picture. False: the window should go (Visible off).
    fn tick(&mut self) -> bool {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f32();
        self.last = now;
        self.logic.tick(dt);
        if !self.logic.st.visible {
            return false;
        }
        self.restyle();
        if self.drag.is_none() {
            let want = self.logic.window_rect();
            if want != self.placed {
                // Moved from outside (a preset, Mini view): put the window there.
                self.placed = want;
                // SAFETY: our window.
                let r = unsafe { on_screen(device(want, self.scale), self.hwnd) };
                let size = (r.right - r.left, r.bottom - r.top);
                unsafe { SetWindowPos(self.hwnd, std::ptr::null_mut(), r.left, r.top, size.0, size.1, SWP_NOZORDER | SWP_NOACTIVATE) };
                self.set_size(size);
            }
        }
        self.render();
        true
    }

    /// Owner, click-through and topmost from the state, when they changed.
    fn restyle(&mut self) {
        let st = &self.logic.st;
        let want = (st.on_desktop, st.keep_in_front, st.solid);
        if self.styled == Some(want) {
            return;
        }
        self.styled = Some(want);
        let (desktop, front, solid) = want;
        // SAFETY: our window; the owner is the host's live top window.
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWLP_HWNDPARENT, if desktop { 0 } else { self.host as isize });
            let ex = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
            let through = WS_EX_TRANSPARENT as isize;
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, if solid { ex & !through } else { ex | through });
            let z = if front { HWND_TOPMOST } else { HWND_NOTOPMOST };
            SetWindowPos(self.hwnd, z, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED);
        }
    }

    /// The bitmap and canvas for a window of `size` device px. False when the bitmap could not be made.
    fn set_size(&mut self, size: (i32, i32)) -> bool {
        let size = (size.0.max(1), size.1.max(1));
        if size == self.size && !self.bitmap.is_null() {
            return true;
        }
        // SAFETY: our memory DC; the old bitmap is deselected by selecting the new one before it is deleted.
        unsafe {
            let mut bmi: BITMAPINFO = std::mem::zeroed();
            bmi.bmiHeader = BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size.0,
                biHeight: -size.1,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..std::mem::zeroed()
            };
            let mut bits = std::ptr::null_mut();
            let bitmap = CreateDIBSection(self.dc, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
            if bitmap.is_null() || bits.is_null() {
                return false;
            }
            SelectObject(self.dc, bitmap);
            if !self.bitmap.is_null() {
                DeleteObject(self.bitmap);
            }
            self.bitmap = bitmap;
            self.bits = bits.cast();
        }
        self.size = size;
        self.canvas.resize(size.0 as u32, size.1 as u32, self.scale);
        true
    }

    /// Paints the display and puts it on the screen with its alpha.
    fn render(&mut self) {
        let (w, h) = self.size;
        let n = (w * h) as usize;
        let area = Rect::new(0.0, 0.0, w as f32 / self.scale, h as f32 / self.scale);
        // SAFETY: the DIB section's pixels, `w * h` of them, live as long as the bitmap; our window and DC.
        unsafe {
            let out = std::slice::from_raw_parts_mut(self.bits, n);
            self.logic.paint_see_through(&mut self.canvas, area, &mut self.buffers, out);
            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
            let size = SIZE { cx: w, cy: h };
            let from = POINT { x: 0, y: 0 };
            UpdateLayeredWindow(self.hwnd, std::ptr::null_mut(), std::ptr::null(), &size, self.dc, &from, 0, &blend, ULW_ALPHA);
        }
    }

    /// The mouse moved while dragging: the window follows (body) or grows (corner).
    fn dragged(&mut self) {
        let p = cursor();
        match self.drag {
            Some(Drag::Move { from, at }) => unsafe {
                // SAFETY: our window.
                SetWindowPos(self.hwnd, std::ptr::null_mut(), at.left + p.x - from.x, at.top + p.y - from.y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
            },
            Some(Drag::Size { from, at }) => {
                let min = (MIN_SIDE * self.scale).round() as i32;
                let size = ((at.right - at.left + p.x - from.x).max(min), (at.bottom - at.top + p.y - from.y).max(min));
                if self.set_size(size) {
                    self.render();
                }
            }
            None => {}
        }
    }

    /// The drag ended: the window's new rectangle into the state.
    fn dropped(&mut self) {
        if self.drag.take().is_none() {
            return;
        }
        let r = logical(window_rect(self.hwnd), self.scale);
        self.logic.set_window_rect(r);
        self.placed = self.logic.window_rect();
    }
}
