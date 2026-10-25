// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Plugin windows on Windows: the frame (`super`) in a Win32 window, docked (a child of the main window, clipped to the
//! workspace and stacked with the app's own windows) until the menu arrow's "Detached" makes it a top-level window owned
//! by the main window. The plugin parents its editor into a child window below the strip.
//!
// spec: ui/plugin-window.md, plugins/host-link.md

use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, Once};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, ClientToScreen, CombineRgn, CreateRectRgn, DeleteObject, EndPaint, GetWindowRgn, InvalidateRect, RedrawWindow, ScreenToClient, SetWindowRgn, NULLREGION, RDW_ALLCHILDREN, RDW_INVALIDATE,
    RDW_UPDATENOW, RGN_AND, RGN_COPY, RGN_DIFF, SetDIBitsToDevice, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, PAINTSTRUCT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetActiveWindow, GetFocus, GetKeyState, ReleaseCapture, SetCapture, SetFocus, VK_CONTROL, VK_MENU};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use super::*;

/// A key pressed or released while a plugin window has the keyboard focus (the typing keyboard keeps playing).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyMsg {
    /// Hardware scancode as winit's `PhysicalKeyExtScancode` expects it (extended keys carry 0xE000).
    pub scancode: u32,
    pub pressed: bool,
    pub ctrl: bool,
    pub alt: bool,
}

static KEYS: Mutex<Vec<KeyMsg>> = Mutex::new(Vec::new());

/// (editor area window, message) since the last `take_link`.
static LINK: Mutex<Vec<(isize, LinkMsg)>> = Mutex::new(Vec::new());

/// Link messages seen since the last call (UI thread).
pub fn take_link() -> Vec<(isize, LinkMsg)> {
    LINK.lock().map(|mut k| std::mem::take(&mut *k)).unwrap_or_default()
}
/// A docked window moved, shrank, was clipped smaller or went: part of the main window was uncovered (`take_exposed`).
static EXPOSED: AtomicBool = AtomicBool::new(false);

/// Whether a docked plugin window uncovered part of the main window since the last call (moved, shrank, hid, was
/// clipped under a popup, detached or closed): the next present must show the whole canvas, not only the app's own
/// damage (UI thread).
pub fn take_exposed() -> bool {
    EXPOSED.swap(false, Ordering::Relaxed)
}

/// The system asks the main window to repaint what a docked window uncovers, but a present of only the app's damage
/// validates the whole window: the uncovered part would keep showing the plugin window.
fn expose() {
    EXPOSED.store(true, Ordering::Relaxed);
}

/// Keys seen since the last call (UI thread).
pub fn take_keys() -> Vec<KeyMsg> {
    KEYS.lock().map(|mut k| std::mem::take(&mut *k)).unwrap_or_default()
}

static KEY_HOOK: Once = Once::new();

/// Thread-wide keyboard hook: a plugin's editor (any plugin, any child window) swallows key messages, so the key
/// events are copied here whenever the focus is inside one of our plugin frames, and the app plays them.
unsafe extern "system" fn key_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if code == 0 {
            let focus = GetFocus();
            if !focus.is_null() {
                let in_frame = frame_of(focus).is_some();
                let up = lp & (1 << 31) != 0;
                let repeat = !up && lp & (1 << 30) != 0;
                if in_frame && !repeat {
                    let mut sc = ((lp >> 16) & 0xff) as u32;
                    if lp & (1 << 24) != 0 {
                        sc |= 0xE000;
                    }
                    let down = |vk: u16| GetKeyState(i32::from(vk)) < 0;
                    if let Ok(mut k) = KEYS.lock() {
                        k.push(KeyMsg { scancode: sc, pressed: !up, ctrl: down(VK_CONTROL), alt: down(VK_MENU) });
                    }
                }
            }
        }
        CallNextHookEx(std::ptr::null_mut(), code, wp, lp)
    }
}

/// The plugin frame `w` is or lies inside: docked frames are children of the main window, so walk up to the frame.
unsafe fn frame_of(mut w: HWND) -> Option<HWND> {
    unsafe {
        while !w.is_null() {
            let mut name = [0u16; 64];
            let n = GetClassNameW(w, name.as_mut_ptr(), 64) as usize;
            if String::from_utf16_lossy(&name[..n]) == FRAME_CLASS {
                return Some(w);
            }
            w = GetAncestor(w, GA_PARENT);
        }
        None
    }
}

/// Gives the keyboard back to the main window when a docked plugin editor holds it, or nobody does. A plugin takes it
/// with a click (for its text fields) and keeps it: a docked frame is a child of the main window, so a click elsewhere
/// in the main window leaves the window active and Windows leaves the focus where it was; only the typing keyboard
/// (`key_hook`) would still work. When the focused editor closes, the focus goes to nobody. A detached frame needs
/// nothing: clicking the main window activates it, which moves the focus. Called on clicks in the main window.
pub fn focus_main() {
    unsafe {
        let focus = GetFocus();
        if focus.is_null() {
            let active = GetActiveWindow();
            if !active.is_null() {
                SetFocus(active);
            }
            return;
        }
        let Some(frame) = frame_of(focus) else { return };
        if GetWindowLongPtrW(frame, GWL_STYLE) as u32 & WS_CHILD == 0 {
            return;
        }
        let main = GetAncestor(frame, GA_ROOT);
        if !main.is_null() {
            SetFocus(main);
        }
    }
}

fn install_key_hook() {
    KEY_HOOK.call_once(|| unsafe {
        SetWindowsHookExW(WH_KEYBOARD, Some(key_hook), std::ptr::null_mut(), GetCurrentThreadId());
    });
}

struct Win {
    hwnd: HWND,
    area: HWND,
    canvas: Canvas,
    buf: Vec<u32>,
    scale: f32,
    state: Rc<RefCell<State>>,
    /// A held drag (`Reply::Hold`): the hidden cursor's screen point, kept there, and the virtual window point (logical
    /// px) the mouse's motion moves.
    held: Option<(POINT, (f32, f32))>,
}

/// A plugin window. Dropping it destroys the native window (close the plugin's editor first).
pub struct FrameWindow {
    hwnd: HWND,
    area: HWND,
    state: Rc<RefCell<State>>,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

const FRAME_CLASS: &str = "OpenStudioPluginFrame";
const AREA_CLASS: &str = "OpenStudioPluginArea";

impl FrameWindow {
    /// A hidden frame for an editor of `editor` logical px at screen position `pos` (physical px): a child of `owner`
    /// (docked) or a top-level window owned by it (`detached`).
    pub fn new(owner: isize, info: ChannelInfo, editor: (f32, f32), scale: f32, pos: (i32, i32), detached: bool) -> Result<FrameWindow, String> {
        install_key_hook();
        // Without a main window (tests) there is nothing to dock into.
        let detached = detached || owner == 0;
        let state = Rc::new(RefCell::new(State::new(info, editor, detached)));
        let (w, h) = state.borrow().size();
        let px = |v: f32| (v * scale).round() as i32;
        unsafe {
            let inst = GetModuleHandleW(std::ptr::null());
            let (fc, ac) = (wide(FRAME_CLASS), wide(AREA_CLASS));
            let cursor = LoadCursorW(std::ptr::null_mut(), IDC_ARROW);
            for (class, proc_) in [(&fc, frame_proc as WndProc), (&ac, area_proc as WndProc)] {
                let wc = WNDCLASSW {
                    style: CS_DBLCLKS,
                    lpfnWndProc: Some(proc_),
                    cbClsExtra: 0,
                    cbWndExtra: 0,
                    hInstance: inst,
                    hIcon: std::ptr::null_mut(),
                    hCursor: cursor,
                    hbrBackground: std::ptr::null_mut(),
                    lpszMenuName: std::ptr::null(),
                    lpszClassName: class.as_ptr(),
                };
                RegisterClassW(&wc); // fails harmlessly once registered
            }
            let canvas = Canvas::new(px(w) as u32, px(h) as u32, scale)?;
            let win = Box::new(Win { hwnd: std::ptr::null_mut(), area: std::ptr::null_mut(), canvas, buf: Vec::new(), scale, state: state.clone(), held: None });
            let title = wide(&state.borrow().info.name);
            let mut p = POINT { x: pos.0, y: pos.1 };
            if !detached {
                ScreenToClient(owner as HWND, &mut p);
            }
            let hwnd = CreateWindowExW(
                0,
                fc.as_ptr(),
                title.as_ptr(),
                frame_style(detached),
                p.x,
                p.y,
                px(w),
                px(h),
                owner as HWND,
                std::ptr::null_mut(),
                inst,
                Box::into_raw(win) as *const c_void,
            );
            if hwnd.is_null() {
                return Err("could not create the plugin window".into());
            }
            // Files dropped on the frame from the desktop (spec: plugins/presets.md "Drag and drop").
            windows_sys::Win32::UI::Shell::DragAcceptFiles(hwnd, 1);
            let ay = state.borrow().area_y();
            let area = CreateWindowExW(
                0,
                ac.as_ptr(),
                std::ptr::null(),
                WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN,
                px(BORDER),
                px(ay),
                px(editor.0),
                px(editor.1),
                hwnd,
                std::ptr::null_mut(),
                inst,
                std::ptr::null(),
            );
            if let Some(w) = win_of(hwnd) {
                w.area = area;
            }
            Ok(FrameWindow { hwnd, area, state })
        }
    }

    pub fn detached(&self) -> bool {
        self.state.borrow().detached
    }

    /// Docks the window into `owner` (a child, same place on screen) or detaches it (a top-level window owned by
    /// `owner`). The plugin's editor stays embedded.
    pub fn set_detached(&self, owner: isize, on: bool) {
        if self.state.borrow().detached == on {
            return;
        }
        {
            let mut st = self.state.borrow_mut();
            st.detached = on;
            st.clip = None;
            st.clip_key = None;
        }
        unsafe {
            let mut r: RECT = std::mem::zeroed();
            GetWindowRect(self.hwnd, &mut r);
            SetWindowRgn(self.hwnd, std::ptr::null_mut(), 0);
            ShowWindow(self.hwnd, SW_HIDE);
            let mut p = POINT { x: r.left, y: r.top };
            if on {
                expose();
                SetParent(self.hwnd, std::ptr::null_mut());
                SetWindowLongPtrW(self.hwnd, GWL_STYLE, frame_style(true) as isize);
                SetWindowLongPtrW(self.hwnd, GWLP_HWNDPARENT, owner);
            } else {
                SetWindowLongPtrW(self.hwnd, GWLP_HWNDPARENT, 0);
                SetWindowLongPtrW(self.hwnd, GWL_STYLE, frame_style(false) as isize);
                SetParent(self.hwnd, owner as HWND);
                ScreenToClient(owner as HWND, &mut p);
            }
            SetWindowPos(self.hwnd, HWND_TOP, p.x, p.y, 0, 0, SWP_NOSIZE | SWP_FRAMECHANGED);
        }
        self.raise();
    }

    /// Clips a docked window to `work` minus the `covers` (the app's windows and popups above it), all in the parent's
    /// client area (physical px). Moving the window by its title bar re-clips with the same rects.
    pub fn clip_to(&self, work: [i32; 4], covers: &[[i32; 4]]) {
        if self.detached() {
            return;
        }
        let changed = self.state.borrow().clip.as_ref().is_none_or(|c| c.0 != work);
        self.state.borrow_mut().clip = Some((work, covers.to_vec()));
        if changed {
            if let Some(w) = unsafe { win_of(self.hwnd) } {
                unsafe { refit(self.hwnd, w) };
            }
        }
        // The workspace may have shrunk: the window keeps its part in it (`crate::win_clamp::keep_px`).
        if work[2] > work[0] && work[3] > work[1] {
            let k = self.scale();
            self.place(|me| crate::win_clamp::keep_px((me[0], me[1]), me[2] - me[0], work, k));
        }
        unsafe { apply_clip(self.hwnd, &self.state) };
    }

    /// VIEW > Into workspace: a docked window partly outside the workspace it was clipped to moves fully back in,
    /// keeping its size (the plugin's editor sets it). Detached windows stay where they are.
    pub fn move_into_workspace(&self) {
        let work = self.state.borrow().clip.as_ref().map(|c| c.0);
        let Some(work) = work.filter(|_| !self.detached()) else { return };
        self.place(|me| crate::win_clamp::inside_px(me, work));
        unsafe { apply_clip(self.hwnd, &self.state) };
    }

    /// Moves the docked window to where `to` puts it, given its rect in the parent's client area (physical px).
    fn place(&self, to: impl FnOnce([i32; 4]) -> (i32, i32)) {
        unsafe {
            let me = parent_rect(self.hwnd);
            let p = to(me);
            if p != (me[0], me[1]) {
                SetWindowPos(self.hwnd, std::ptr::null_mut(), p.0, p.1, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
                expose();
            }
        }
    }

    /// The frame's top-left on screen (physical px), `None` while it is minimized.
    pub fn position(&self) -> Option<(i32, i32)> {
        let mut r: RECT = unsafe { std::mem::zeroed() };
        (unsafe { IsIconic(self.hwnd) == 0 && GetWindowRect(self.hwnd, &mut r) != 0 }).then_some((r.left, r.top))
    }

    /// The native frame window (drops find their plugin by it; tests drive and capture it).
    pub fn hwnd(&self) -> isize {
        self.hwnd as isize
    }

    /// The child window the plugin's editor goes into.
    pub fn area(&self) -> *mut c_void {
        self.area
    }

    pub fn raise(&self) {
        unsafe {
            ShowWindow(self.hwnd, SW_SHOWNORMAL);
            SetWindowPos(self.hwnd, HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
            if self.detached() {
                SetForegroundWindow(self.hwnd);
            }
        }
    }

    pub fn set_info(&self, info: ChannelInfo) {
        let mut st = self.state.borrow_mut();
        if st.info != info && st.drag.is_none() && st.misc_ui.drag.is_none() {
            st.info = info;
            drop(st);
            unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) };
        }
    }

    /// The plugin's own page tabs (a Lemon plugin joined through the host link), replacing the plug tab.
    pub fn set_pages(&self, pages: Vec<PageIcon>, active: usize) {
        let mut st = self.state.borrow_mut();
        if !pages.is_empty() && (st.pages != pages || st.page != active) {
            st.page = active.min(pages.len() - 1);
            st.pages = pages;
            drop(st);
            unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) };
        }
    }

    /// The plugin's smallest editor size when the frame's edges may resize it (`None`: a fixed size).
    pub fn set_resizable(&self, min: Option<(f32, f32)>) {
        let mut st = self.state.borrow_mut();
        if st.resize_min != min {
            st.resize_min = min;
            if min.is_none() {
                st.sizing = None;
            }
        }
    }

    /// The plugin changed its page itself.
    pub fn set_page(&self, page: usize) {
        let mut st = self.state.borrow_mut();
        if st.page != page && page < st.pages.len() {
            st.page = page;
            drop(st);
            unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) };
        }
    }

    /// The plugin's hint line (parameter under the mouse).
    pub fn set_hint(&self, hint: String) {
        let mut st = self.state.borrow_mut();
        if st.hint != hint {
            st.hint = hint;
            drop(st);
            unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) };
        }
    }

    /// The loaded preset's name for the Presets box.
    pub fn set_preset(&self, name: Option<String>) {
        let mut st = self.state.borrow_mut();
        if st.preset != name {
            st.preset = name;
            drop(st);
            unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) };
        }
    }

    /// The DPI scale the frame is drawn at.
    pub fn scale(&self) -> f32 {
        unsafe { win_of(self.hwnd) }.map_or(1.0, |w| w.scale)
    }

    /// A point of the editor (logical px from its top-left) on the screen (screen px): where a menu the plugin asked
    /// for opens (spec: plugins/vst3-hosting.md §9).
    pub fn editor_to_screen(&self, x: f32, y: f32) -> (i32, i32) {
        let ay = self.state.borrow().area_y();
        to_screen(self.hwnd, self.scale())(BORDER + x, ay + y)
    }

    /// Redraws the frame at another DPI scale (same logical size). The editor inside is the caller's: plugins take
    /// their scale when the editor opens (spec: ui/ui-scaling.md §3).
    pub fn set_scale(&self, scale: f32) {
        let Some(w) = (unsafe { win_of(self.hwnd) }) else { return };
        if scale <= 0.0 || w.scale == scale {
            return;
        }
        w.scale = scale;
        self.state.borrow_mut().clip_key = None;
        unsafe { layout(self.hwnd) };
    }

    /// Follows a new editor size (plugins may resize).
    pub fn set_editor_size(&self, editor: (f32, f32)) {
        if self.state.borrow().editor != editor {
            self.state.borrow_mut().editor = editor;
            unsafe { layout(self.hwnd) };
        }
    }

    /// Shows or hides the gear strip (mixer effects open without it).
    pub fn set_strip(&self, on: bool) {
        if self.state.borrow().strip != on {
            self.state.borrow_mut().strip = on;
            unsafe { layout(self.hwnd) };
        }
    }

    pub fn take_events(&self) -> Vec<FrameEvent> {
        std::mem::take(&mut self.state.borrow_mut().events)
    }
}

impl Drop for FrameWindow {
    fn drop(&mut self) {
        if !self.detached() {
            expose();
        }
        unsafe { DestroyWindow(self.hwnd) };
    }
}

type WndProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;

fn frame_style(detached: bool) -> u32 {
    if detached { WS_POPUP | WS_CLIPCHILDREN } else { WS_CHILD | WS_CLIPSIBLINGS | WS_CLIPCHILDREN }
}


/// Puts `parent`'s docked windows `hwnds` (bottom to top) in that native order when they are not already.
pub fn stack(parent: isize, hwnds: &[isize]) {
    if hwnds.len() < 2 {
        return;
    }
    unsafe {
        // The children's current order, bottom to top.
        let mut now = Vec::new();
        let mut c = GetWindow(parent as HWND, GW_CHILD);
        while !c.is_null() {
            if hwnds.contains(&(c as isize)) {
                now.push(c as isize);
            }
            c = GetWindow(c, GW_HWNDNEXT);
        }
        now.reverse();
        if now == hwnds {
            return;
        }
        for &h in hwnds {
            SetWindowPos(h as HWND, HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }
}

/// A child window's rect in its parent's client area (physical px).
unsafe fn parent_rect(hwnd: HWND) -> [i32; 4] {
    unsafe {
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(hwnd, &mut r);
        let parent = GetParent(hwnd);
        let (mut a, mut b) = (POINT { x: r.left, y: r.top }, POINT { x: r.right, y: r.bottom });
        ScreenToClient(parent, &mut a);
        ScreenToClient(parent, &mut b);
        [a.x, a.y, b.x, b.y]
    }
}

/// Sets a docked window's region for its current position from `State::clip`. The parts that come into view are
/// repainted, the plugin's editor included: the system redraws only the frame itself, and an editor left alone keeps
/// showing whatever was seen through the old region. The parts that go out of view uncover the main window (`expose`).
unsafe fn apply_clip(hwnd: HWND, state: &RefCell<State>) {
    let (me, work, covers) = {
        let mut st = state.borrow_mut();
        let Some((work, covers)) = st.clip.as_ref() else { return };
        let me = unsafe { parent_rect(hwnd) };
        // Only what overlaps the window matters.
        let hit = |c: &[i32; 4]| c[0] < me[2] && c[2] > me[0] && c[1] < me[3] && c[3] > me[1];
        let covers: Vec<[i32; 4]> = covers.iter().copied().filter(hit).collect();
        let work = *work;
        let key: Vec<i32> = me.into_iter().chain(work).chain(covers.iter().flatten().copied()).collect();
        if st.clip_key.as_ref() == Some(&key) {
            return;
        }
        st.clip_key = Some(key);
        (me, work, covers)
    };
    let inside = work[0] <= me[0] && work[1] <= me[1] && work[2] >= me[2] && work[3] >= me[3];
    let (ox, oy, w, h) = (me[0], me[1], me[2] - me[0], me[3] - me[1]);
    unsafe {
        let whole = CreateRectRgn(0, 0, w, h);
        let rgn = if inside && covers.is_empty() {
            std::ptr::null_mut()
        } else {
            let rgn = CreateRectRgn(0, 0, w, h);
            let r = CreateRectRgn(work[0] - ox, work[1] - oy, work[2] - ox, work[3] - oy);
            CombineRgn(rgn, rgn, r, RGN_AND);
            DeleteObject(r);
            for c in &covers {
                let r = CreateRectRgn(c[0] - ox, c[1] - oy, c[2] - ox, c[3] - oy);
                CombineRgn(rgn, rgn, r, RGN_DIFF);
                DeleteObject(r);
            }
            rgn
        };
        // What the new region shows that the old one hid (no region = the whole window).
        let old = CreateRectRgn(0, 0, 0, 0);
        if GetWindowRgn(hwnd, old) == 0 {
            CombineRgn(old, whole, std::ptr::null_mut(), RGN_COPY);
        }
        let gained = CreateRectRgn(0, 0, 0, 0);
        CombineRgn(gained, if rgn.is_null() { whole } else { rgn }, old, RGN_DIFF);
        // What the old region showed that the new one hides: the main window there comes into view (a popup opened
        // over this window). While the app has damage of its own (meters moving during playback) it would present
        // only that, and the uncovered part would keep showing this window over the popup.
        let lost = CreateRectRgn(0, 0, 0, 0);
        if CombineRgn(lost, old, if rgn.is_null() { whole } else { rgn }, RGN_DIFF) > NULLREGION {
            expose();
        }
        SetWindowRgn(hwnd, rgn, 1); // the system owns `rgn` now
        RedrawWindow(hwnd, std::ptr::null(), gained, RDW_INVALIDATE | RDW_ALLCHILDREN);
        for r in [whole, old, gained, lost] {
            DeleteObject(r);
        }
    }
}

unsafe fn win_of(hwnd: HWND) -> Option<&'static mut Win> {
    let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Win;
    if p.is_null() { None } else { Some(unsafe { &mut *p }) }
}

/// Resizes the frame to its state (strip shown or not, editor size) and moves the editor area.
unsafe fn layout(hwnd: HWND) {
    let Some(w) = (unsafe { win_of(hwnd) }) else { return };
    let st = w.state.borrow();
    let (fw, fh) = st.size();
    let px = |v: f32| (v * w.scale).round() as i32;
    let tab_plugin = !st.misc;
    let (ay, ed, detached) = (st.area_y(), st.editor, st.detached);
    drop(st);
    unsafe {
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(hwnd, &mut r);
        if !detached && (px(fw) < r.right - r.left || px(fh) < r.bottom - r.top) {
            expose();
        }
        SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, px(fw), px(fh), SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
        SetWindowPos(w.area, std::ptr::null_mut(), px(BORDER), px(ay), px(ed.0), px(ed.1), SWP_NOZORDER | SWP_NOACTIVATE);
        ShowWindow(w.area, if tab_plugin { SW_SHOW } else { SW_HIDE });
    }
    w.canvas.resize(px(fw) as u32, px(fh) as u32, w.scale);
    unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
}

unsafe extern "system" fn area_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_ERASEBKGND {
            return 1;
        }
        if msg == os_hostlink::message() {
            // Plugin -> host (spec: plugins/host-link.md): queue it and wake the app so it is handled at once.
            let m = match os_hostlink::Cmd::from_usize(wp) {
                Some(os_hostlink::Cmd::Changed) => LinkMsg::Changed,
                Some(os_hostlink::Cmd::Note) => {
                    let (k, v, on) = os_hostlink::unpack_note(lp);
                    LinkMsg::Note(k, v, on)
                }
                Some(os_hostlink::Cmd::Automate) => LinkMsg::Automate(lp.max(0) as usize),
                Some(os_hostlink::Cmd::Audio) => LinkMsg::Audio(lp != 0),
                _ => return 0,
            };
            if let Ok(mut q) = LINK.lock() {
                q.push((hwnd as isize, m));
            }
            os_clap::wake();
            return 1;
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}

fn client_point(hwnd: HWND, lp: LPARAM, scale: f32, screen: bool) -> (f32, f32) {
    let mut p = POINT { x: (lp & 0xffff) as u16 as i16 as i32, y: ((lp >> 16) & 0xffff) as u16 as i16 as i32 };
    if screen {
        unsafe { ScreenToClient(hwnd, &mut p) };
    }
    (p.x as f32 / scale, p.y as f32 / scale)
}

unsafe extern "system" fn frame_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
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
        let Some(w) = win_of(hwnd) else { return DefWindowProcW(hwnd, msg, wp, lp) };
        match msg {
            WM_NCHITTEST => {
                // The whole frame is client area; the title bar is moved by hand (WM_LBUTTONDOWN).
                HTCLIENT as LRESULT
            }
            WM_ERASEBKGND => 1,
            WM_DPICHANGED => {
                // Only top-level (detached) windows get this: moved onto a monitor with another DPI. The app redraws
                // the frame and reopens the editor at the new scale on its next tick (spec: ui/ui-scaling.md §3).
                let k = (wp & 0xffff) as f32 / 96.0;
                w.state.borrow_mut().events.push(FrameEvent::Dpi(k));
                os_clap::wake();
                0
            }
            WM_PAINT => {
                let mut ps: PAINTSTRUCT = std::mem::zeroed();
                let hdc = BeginPaint(hwnd, &mut ps);
                paint(&mut w.canvas, &w.state.borrow());
                let (cw, ch) = (w.canvas.size().0 * w.scale, w.canvas.size().1 * w.scale);
                let (cw, ch) = (cw.round() as u32, ch.round() as u32);
                w.buf.resize((cw * ch) as usize, 0);
                w.canvas.copy_to_xrgb(&mut w.buf);
                let mut bmi: BITMAPINFO = std::mem::zeroed();
                bmi.bmiHeader = BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: cw as i32,
                    biHeight: -(ch as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    ..std::mem::zeroed()
                };
                SetDIBitsToDevice(hdc, 0, 0, cw, ch, 0, 0, 0, ch, w.buf.as_ptr() as *const c_void, &bmi, DIB_RGB_COLORS);
                EndPaint(hwnd, &ps);
                0
            }
            WM_MOUSEACTIVATE => {
                // Any click brings the window to the front of the app's stack (spec: ui/plugin-window.md "Docking").
                SetWindowPos(hwnd, HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                w.state.borrow_mut().events.push(FrameEvent::Raise);
                os_clap::wake();
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_PARENTNOTIFY if matches!((wp & 0xffff) as u32, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN) => {
                // A click inside the plugin's editor.
                SetWindowPos(hwnd, HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                w.state.borrow_mut().events.push(FrameEvent::Raise);
                os_clap::wake();
                0
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                let (x, y) = client_point(hwnd, lp, w.scale, false);
                let r = w.state.borrow_mut().left_down(x, y, msg == WM_LBUTTONDBLCLK, &to_screen(hwnd, w.scale));
                reply(hwnd, w, r);
                0
            }
            WM_RBUTTONDOWN => {
                let (x, y) = client_point(hwnd, lp, w.scale, false);
                let r = w.state.borrow_mut().right_down(x, y, &to_screen(hwnd, w.scale));
                reply(hwnd, w, r);
                0
            }
            WM_MBUTTONDOWN => {
                let (x, y) = client_point(hwnd, lp, w.scale, false);
                let r = w.state.borrow_mut().middle_down(x, y, &to_screen(hwnd, w.scale));
                reply(hwnd, w, r);
                0
            }
            WM_MOUSEMOVE => {
                if let Some((anchor, virt)) = w.held {
                    // A held drag: the motion away from the anchor moves the virtual point; the cursor goes back.
                    let mut c: POINT = std::mem::zeroed();
                    GetCursorPos(&mut c);
                    if (c.x, c.y) == (anchor.x, anchor.y) {
                        return 0;
                    }
                    let v = (virt.0 + (c.x - anchor.x) as f32 / w.scale, virt.1 + (c.y - anchor.y) as f32 / w.scale);
                    w.held = Some((anchor, v));
                    SetCursorPos(anchor.x, anchor.y);
                    let fine = GetKeyState(VK_CONTROL as i32) < 0;
                    let r = w.state.borrow_mut().motion(v.0, v.1, fine);
                    reply(hwnd, w, r);
                    return 0;
                }
                let (x, y) = client_point(hwnd, lp, w.scale, false);
                let moving = w.state.borrow().moving;
                if let Some((cx, cy, wx, wy)) = moving {
                    let mut c: POINT = std::mem::zeroed();
                    GetCursorPos(&mut c);
                    // Screen position; a docked window is placed in its parent's client area.
                    let mut p = POINT { x: wx + c.x - cx, y: wy + c.y - cy };
                    let docked = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 & WS_CHILD != 0;
                    if docked {
                        ScreenToClient(GetParent(hwnd), &mut p);
                    }
                    let to = kept(hwnd, w, docked, (p.x, p.y));
                    SetWindowPos(hwnd, std::ptr::null_mut(), to.0, to.1, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
                    if docked {
                        // Paint what the move uncovered (the main window, other docked plugin windows and their
                        // editors) before the next move. Left to ordinary WM_PAINTs, which wait behind the mouse
                        // moves, the windows below lag behind and show trails of this one while it is dragged.
                        apply_clip(hwnd, &w.state);
                        expose();
                        RedrawWindow(GetParent(hwnd), std::ptr::null(), std::ptr::null_mut(), RDW_UPDATENOW | RDW_ALLCHILDREN);
                    }
                    return 0;
                }
                let fine = GetKeyState(VK_CONTROL as i32) < 0;
                let r = w.state.borrow_mut().motion(x, y, fine);
                reply(hwnd, w, r);
                0
            }
            WM_LBUTTONUP => {
                // Before ReleaseCapture: its WM_CAPTURECHANGED lets an armed button go without acting.
                let (x, y) = match w.held {
                    Some((_, v)) => v,
                    None => client_point(hwnd, lp, w.scale, false),
                };
                let r = w.state.borrow_mut().left_up(x, y);
                ReleaseCapture();
                unhold(w);
                reply(hwnd, w, r);
                0
            }
            WM_CAPTURECHANGED => {
                w.state.borrow_mut().release();
                unhold(w);
                0
            }
            WM_MOUSEWHEEL => {
                let (x, y) = client_point(hwnd, lp, w.scale, true);
                let notches = ((wp >> 16) & 0xffff) as u16 as i16 as f32 / 120.0;
                let r = w.state.borrow_mut().wheel(x, y, notches);
                reply(hwnd, w, r);
                0
            }
            WM_CLOSE => {
                w.state.borrow_mut().events.push(FrameEvent::Close);
                0
            }
            WM_DROPFILES => {
                use windows_sys::Win32::UI::Shell::{DragFinish, DragQueryFileW, HDROP};
                let drop = wp as HDROP;
                let n = DragQueryFileW(drop, u32::MAX, std::ptr::null_mut(), 0);
                for i in 0..n {
                    let len = DragQueryFileW(drop, i, std::ptr::null_mut(), 0) as usize;
                    let mut buf = vec![0u16; len + 1];
                    DragQueryFileW(drop, i, buf.as_mut_ptr(), buf.len() as u32);
                    let path = std::path::PathBuf::from(String::from_utf16_lossy(&buf[..len]));
                    w.state.borrow_mut().events.push(FrameEvent::DropFile(path));
                }
                DragFinish(drop);
                os_clap::wake();
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// The frame's window point (logical px) on screen (physical px).
fn to_screen(hwnd: HWND, scale: f32) -> impl Fn(f32, f32) -> (i32, i32) {
    move |x, y| {
        let mut p = POINT { x: (x * scale) as i32, y: (y * scale) as i32 };
        unsafe { ClientToScreen(hwnd, &mut p) };
        (p.x, p.y)
    }
}

/// Does what an input event asked of the window.
unsafe fn reply(hwnd: HWND, w: &mut Win, r: Reply) {
    unsafe {
        match r {
            Reply::None => {}
            Reply::Repaint => {
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            Reply::Relayout => {
                layout(hwnd);
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            Reply::Capture => {
                SetCapture(hwnd);
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            Reply::Hold => {
                let mut c: POINT = std::mem::zeroed();
                GetCursorPos(&mut c);
                let mut p = c;
                ScreenToClient(hwnd, &mut p);
                if w.held.is_none() {
                    ShowCursor(0);
                }
                w.held = Some((c, (p.x as f32 / w.scale, p.y as f32 / w.scale)));
                SetCapture(hwnd);
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            Reply::Move => {
                // Moved by hand (WM_MOUSEMOVE): the system's move loop (HTCAPTION) blocks the UI thread.
                let mut c: POINT = std::mem::zeroed();
                let mut r: RECT = std::mem::zeroed();
                GetCursorPos(&mut c);
                GetWindowRect(hwnd, &mut r);
                w.state.borrow_mut().moving = Some((c.x, c.y, r.left, r.top));
                SetCapture(hwnd);
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            Reply::Minimize => {
                // A docked window hides (its channel's name button shows it again).
                let detached = w.state.borrow().detached;
                if !detached {
                    expose();
                }
                ShowWindow(hwnd, if detached { SW_MINIMIZE } else { SW_HIDE });
            }
            Reply::Maximize => toggle_max(hwnd, w),
        }
    }
}

/// The frame's top-left on the screen (physical px).
unsafe fn top_left(hwnd: HWND) -> (i32, i32) {
    let mut r: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetWindowRect(hwnd, &mut r) };
    (r.left, r.top)
}

/// Moves the frame's top-left to screen point `at` (physical px); a docked frame is clipped again for the place.
unsafe fn place_at(hwnd: HWND, w: &Win, at: (i32, i32)) {
    unsafe {
        if w.state.borrow().detached {
            SetWindowPos(hwnd, std::ptr::null_mut(), at.0, at.1, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
            return;
        }
        let mut p = POINT { x: at.0, y: at.1 };
        ScreenToClient(GetParent(hwnd), &mut p);
        SetWindowPos(hwnd, std::ptr::null_mut(), p.x, p.y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        apply_clip(hwnd, &w.state);
        expose();
    }
}

/// Maximizes the frame to its workspace, or restores it.
unsafe fn toggle_max(hwnd: HWND, w: &Win) {
    let (top_left, target) = unsafe { (top_left(hwnd), max_target(hwnd, w)) };
    let at = w.state.borrow_mut().maximize(top_left, target);
    if let Some(at) = at {
        unsafe {
            place_at(hwnd, w, at);
            InvalidateRect(hwnd, std::ptr::null(), 0);
        }
    }
}

/// What a maximized frame fills: docked, the app's workspace; detached, the work area of the monitor it is on (the
/// screen less the taskbar). Its top-left (screen px) and size (logical px).
unsafe fn max_target(hwnd: HWND, w: &Win) -> Option<((i32, i32), (f32, f32))> {
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    let k = w.scale;
    let size = |r: [i32; 4]| ((r[2] - r[0]) as f32 / k, (r[3] - r[1]) as f32 / k);
    let docked = {
        let st = w.state.borrow();
        if st.detached { None } else { Some(st.clip.as_ref()?.0) }
    };
    unsafe {
        if let Some(work) = docked {
            let mut p = POINT { x: work[0], y: work[1] };
            ClientToScreen(GetParent(hwnd), &mut p);
            return Some(((p.x, p.y), size(work)));
        }
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut mi) == 0 {
            return None;
        }
        let r = mi.rcWork;
        Some(((r.left, r.top), size([r.left, r.top, r.right, r.bottom])))
    }
}

/// A maximized docked frame keeps filling the workspace when it changes (the browser shown or hidden, the main window
/// resized).
unsafe fn refit(hwnd: HWND, w: &Win) {
    if w.state.borrow().detached || w.state.borrow().maxed.is_none() {
        return;
    }
    let Some((at, size)) = (unsafe { max_target(hwnd, w) }) else { return };
    let fill = {
        let mut st = w.state.borrow_mut();
        let fill = st.fill(size);
        if let Some((fw, fh)) = fill.filter(|&(fw, fh)| (fw as f32, fh as f32) != st.editor) {
            st.events.push(FrameEvent::Resize(fw, fh));
            os_clap::wake();
        }
        fill
    };
    if fill.is_some() && at != unsafe { top_left(hwnd) } {
        unsafe { place_at(hwnd, w, at) };
    }
}

/// Ends a held drag: the cursor shows again where the drag started.
unsafe fn unhold(w: &mut Win) {
    if let Some((anchor, _)) = w.held.take() {
        unsafe {
            SetCursorPos(anchor.x, anchor.y);
            ShowCursor(1);
        }
    }
}

/// Where a window moved by hand to `p` goes (spec: ui/settings-choices.md "Advanced: Don't limit windows to screen
/// (2026-10-16)"): a docked one (`p` in the parent's client area) keeps its part in the workspace it was clipped to
/// (`crate::win_clamp::keep_px`); a detached one (`p` on screen) keeps its title bar on the screens unless "Don't
/// limit windows to screen" is on (`crate::win_clamp::bar_on_screens`).
fn kept(hwnd: HWND, w: &Win, docked: bool, p: (i32, i32)) -> (i32, i32) {
    let mut r: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetWindowRect(hwnd, &mut r) };
    let width = r.right - r.left;
    if docked {
        let work = w.state.borrow().clip.as_ref().map(|c| c.0);
        return match work {
            Some(work) if work[2] > work[0] && work[3] > work[1] => crate::win_clamp::keep_px(p, width, work, w.scale),
            _ => p,
        };
    }
    if crate::prefs::check(&crate::prefs::DONT_LIMIT_WINDOWS) {
        return p;
    }
    let bar = [p.0, p.1, p.0 + width, p.1 + (TITLE_H * w.scale).round() as i32];
    crate::win_clamp::bar_on_screens(bar, &screens())
}

/// The monitors' work areas (screen px).
fn screens() -> Vec<[i32; 4]> {
    use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};
    unsafe extern "system" fn each(m: HMONITOR, _: HDC, _: *mut RECT, out: LPARAM) -> windows_sys::core::BOOL {
        unsafe {
            let mut mi: MONITORINFO = std::mem::zeroed();
            mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(m, &mut mi) != 0 {
                let r = mi.rcWork;
                (*(out as *mut Vec<[i32; 4]>)).push([r.left, r.top, r.right, r.bottom]);
            }
        }
        1
    }
    let mut out: Vec<[i32; 4]> = Vec::new();
    unsafe { EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(each), &mut out as *mut Vec<[i32; 4]> as LPARAM) };
    out
}

/// Screen position for a new plugin window: right of the owner's left edge, below its toolbar (physical px; the
/// offset is logical, so the window lands on the same part of the layout at every scale).
pub fn default_position(owner: isize, scale: f32) -> (i32, i32) {
    let mut r: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetWindowRect(owner as HWND, &mut r) };
    (r.left + (420.0 * scale).round() as i32, r.top + (160.0 * scale).round() as i32)
}

/// Screen position for a detached window opening for the first time (physical px): where `default_position` puts a new
/// window, moved onto the main window's monitor (`with_main`) or onto the primary monitor (Settings "Open detached
/// plugins with the main window"; spec: ui/settings-choices.md "Miscellaneous: naming, detached plugins and
/// startup").
pub fn first_position(owner: isize, scale: f32, with_main: bool) -> (i32, i32) {
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY};
    let p = default_position(owner, scale);
    let work = |m| unsafe {
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        (GetMonitorInfoW(m, &mut mi) != 0).then(|| [mi.rcWork.left, mi.rcWork.top, mi.rcWork.right, mi.rcWork.bottom])
    };
    let (from, to) = unsafe {
        let to = if with_main { MonitorFromWindow(owner as HWND, MONITOR_DEFAULTTOPRIMARY) } else { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) };
        (work(MonitorFromPoint(POINT { x: p.0, y: p.1 }, MONITOR_DEFAULTTONEAREST)), work(to))
    };
    let keep = ((200.0 * scale).round() as i32, (100.0 * scale).round() as i32);
    match (from, to) {
        (Some(from), Some(to)) => super::moved_to(p, from, to, keep),
        _ => p,
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Lifted overlay: an app popup opened from a detached plugin window would be painted in the main window, under the
// detached window (owned windows stay above their owner). While one is open, its pixels are shown in this small
// window above the plugin window, and its mouse input is passed to the main window as if it happened there.

struct Lift {
    hwnd: HWND,
    main: HWND,
    /// Main-window client position of the lifted rect's top-left.
    origin: (i32, i32),
    size: (i32, i32),
    buf: Vec<u32>,
}

static LIFT: Mutex<Option<LiftPtr>> = Mutex::new(None);
struct LiftPtr(Box<Lift>);
// SAFETY: only touched on the UI thread; the Mutex is just a global slot.
unsafe impl Send for LiftPtr {}

const LIFT_CLASS: &str = "OpenStudioLiftedPopup";

unsafe extern "system" fn lift_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        let Ok(g) = LIFT.lock() else { return DefWindowProcW(hwnd, msg, wp, lp) };
        let Some(l) = g.as_ref().map(|p| &p.0).filter(|l| l.hwnd == hwnd) else {
            drop(g);
            return DefWindowProcW(hwnd, msg, wp, lp);
        };
        match msg {
            WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
            WM_ERASEBKGND => 1,
            WM_PAINT => {
                let mut ps: PAINTSTRUCT = std::mem::zeroed();
                let hdc = BeginPaint(hwnd, &mut ps);
                let (w, h) = l.size;
                if l.buf.len() == (w * h) as usize {
                    let mut bmi: BITMAPINFO = std::mem::zeroed();
                    bmi.bmiHeader = BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: w,
                        biHeight: -h,
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB,
                        ..std::mem::zeroed()
                    };
                    SetDIBitsToDevice(hdc, 0, 0, w as u32, h as u32, 0, 0, 0, h as u32, l.buf.as_ptr() as *const c_void, &bmi, DIB_RGB_COLORS);
                }
                EndPaint(hwnd, &ps);
                0
            }
            WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MBUTTONDOWN | WM_MBUTTONUP => {
                let x = (lp & 0xffff) as u16 as i16 as i32 + l.origin.0;
                let y = ((lp >> 16) & 0xffff) as u16 as i16 as i32 + l.origin.1;
                let main = l.main;
                drop(g);
                PostMessageW(main, msg, wp, ((y as u32 & 0xffff) << 16 | (x as u32 & 0xffff)) as LPARAM);
                0
            }
            _ => {
                drop(g);
                DefWindowProcW(hwnd, msg, wp, lp)
            }
        }
    }
}

/// Shows `pixels` (`size`, XRGB) above plugin window `frame`, at main-window client position `origin`.
pub fn lift_show(main: isize, frame: isize, origin: (i32, i32), size: (i32, i32), pixels: Vec<u32>) {
    unsafe {
        let mut p = POINT { x: origin.0, y: origin.1 };
        ClientToScreen(main as HWND, &mut p);
        let mut g = match LIFT.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if g.is_none() {
            let inst = GetModuleHandleW(std::ptr::null());
            let class = wide(LIFT_CLASS);
            let wc = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(lift_proc),
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
            *g = Some(LiftPtr(Box::new(Lift { hwnd: std::ptr::null_mut(), main: main as HWND, origin, size, buf: Vec::new() })));
            drop(g);
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class.as_ptr(),
                std::ptr::null(),
                WS_POPUP,
                p.x,
                p.y,
                size.0,
                size.1,
                frame as HWND,
                std::ptr::null_mut(),
                inst,
                std::ptr::null(),
            );
            g = match LIFT.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            if let Some(l) = g.as_mut() {
                l.0.hwnd = hwnd;
            }
        }
        let Some(l) = g.as_mut().map(|p| &mut p.0) else { return };
        l.main = main as HWND;
        l.origin = origin;
        l.size = size;
        l.buf = pixels;
        let hwnd = l.hwnd;
        drop(g);
        SetWindowPos(hwnd, HWND_TOP, p.x, p.y, size.0, size.1, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        InvalidateRect(hwnd, std::ptr::null(), 0);
    }
}

/// Hides the lifted popup (none open).
pub fn lift_hide() {
    let l = LIFT.lock().ok().and_then(|mut g| g.take());
    if let Some(l) = l {
        unsafe { DestroyWindow(l.0.hwnd) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A docked window that closes, hides, detaches or shrinks uncovers part of the main window, which must be presented
    /// whole again or that part keeps showing the plugin window. Opens real windows:
    /// `cargo test -p os-app --bin lemonstudio docked_windows_expose_the_main_window -- --ignored`.
    #[test]
    #[ignore]
    fn docked_windows_expose_the_main_window() {
        let class = wide("STATIC");
        let owner = unsafe {
            CreateWindowExW(0, class.as_ptr(), std::ptr::null(), WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN, 0, 0, 900, 700, std::ptr::null_mut(), std::ptr::null_mut(), GetModuleHandleW(std::ptr::null()), std::ptr::null())
        };
        assert!(!owner.is_null());
        let info = ChannelInfo { name: "Keys".into(), target: "Insert 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None };
        let docked = || {
            let mut r: RECT = unsafe { std::mem::zeroed() };
            unsafe { GetWindowRect(owner, &mut r) };
            let f = FrameWindow::new(owner as isize, info.clone(), (480.0, 270.0), 1.0, (r.left + 20, r.top + 20), false).unwrap();
            f.raise();
            assert!(!f.detached());
            f
        };

        let f = docked();
        take_exposed();
        f.set_strip(false);
        assert!(take_exposed(), "shrinking");
        f.set_strip(true);
        assert!(!take_exposed(), "growing uncovers nothing");

        // The title bar's minimize button hides a docked window (on the release over it).
        let (w, _) = f.state.borrow().size();
        let at = ((w - 33.0) as isize) | (15 << 16);
        unsafe { SendMessageW(f.hwnd, WM_LBUTTONDOWN, 1, at) };
        unsafe { SendMessageW(f.hwnd, WM_LBUTTONUP, 0, at) };
        assert_eq!(unsafe { IsWindowVisible(f.hwnd) }, 0);
        assert!(take_exposed(), "hiding");

        f.set_detached(owner as isize, true);
        assert!(take_exposed(), "detaching");
        drop(f);
        assert!(!take_exposed(), "a detached window covers nothing of the main window");

        drop(docked());
        assert!(take_exposed(), "closing");

        // A popup opening over a docked window clips it smaller, which uncovers the main window under the popup; the
        // popup closing gives the window its part back, which uncovers nothing.
        let f = docked();
        let me = unsafe { parent_rect(f.hwnd) };
        let work = [me[0] - 100, me[1] - 100, me[2] + 100, me[3] + 100];
        let popup = [me[2] - 60, me[1] + 10, me[2] + 40, me[1] + 150];
        f.clip_to(work, &[]);
        take_exposed();
        f.clip_to(work, &[popup]);
        assert!(take_exposed(), "clipped under a popup");
        f.clip_to(work, &[popup]);
        assert!(!take_exposed(), "the same clip again");
        f.clip_to(work, &[]);
        assert!(!take_exposed(), "the popup closing uncovers nothing");
        drop(f);
        unsafe { DestroyWindow(owner) };
    }

    /// Spec ui/settings-choices.md "Advanced: Don't limit windows to screen (2026-10-16)": a docked window keeps 31 px
    /// of its width and its title bar in the workspace it is clipped to, also when the workspace shrinks, and Into
    /// workspace brings it fully back in, keeping its size. Opens real windows:
    /// `cargo test -p os-app --bin lemonstudio docked_windows_keep_their_part -- --ignored`.
    #[test]
    #[ignore]
    fn docked_windows_keep_their_part() {
        let class = wide("STATIC");
        let owner = unsafe {
            CreateWindowExW(0, class.as_ptr(), std::ptr::null(), WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN, 0, 0, 900, 700, std::ptr::null_mut(), std::ptr::null_mut(), GetModuleHandleW(std::ptr::null()), std::ptr::null())
        };
        assert!(!owner.is_null());
        let info = ChannelInfo { name: "Keys".into(), target: "Insert 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None };
        let mut r: RECT = unsafe { std::mem::zeroed() };
        unsafe { GetWindowRect(owner, &mut r) };
        let f = FrameWindow::new(owner as isize, info, (480.0, 270.0), 1.0, (r.left + 20, r.top + 20), false).unwrap();
        f.raise();
        let at = |x: i32, y: i32| unsafe { SetWindowPos(f.hwnd, std::ptr::null_mut(), x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE) };
        let me = || unsafe { parent_rect(f.hwnd) };
        let w = me()[2] - me()[0];
        let work = [100, 50, 800, 600];
        // Above the workspace's top: down to it.
        at(300, 10);
        f.clip_to(work, &[]);
        assert_eq!(&me()[..2], &[300, 50]);
        // Past the left, the right and the bottom.
        at(-3000, 200);
        f.clip_to(work, &[]);
        assert_eq!(&me()[..2], &[100 + 31 - w, 200]);
        at(3000, 3000);
        f.clip_to(work, &[]);
        assert_eq!(&me()[..2], &[800 - 31, 600 - 15]);
        // The workspace shrinks.
        let small = [100, 50, 600, 400];
        f.clip_to(small, &[]);
        assert_eq!(&me()[..2], &[600 - 31, 400 - 15]);
        // Into workspace: fully in, the same size.
        f.move_into_workspace();
        let r = me();
        assert_eq!(r[2] - r[0], w);
        // Moved as little as it takes: its bottom-right at the workspace's (its top at the top when it is taller).
        assert_eq!((r[2], r[3]), (600, (50 + r[3] - r[1]).max(400)));
        drop(f);
        unsafe { DestroyWindow(owner) };
    }

    /// A resizable editor's frame (Tesla) maximizes from its title bar's button or a double-click: docked, to the
    /// workspace it is clipped to, which it keeps filling when that changes; detached, to its monitor's work area. Again
    /// restores where it was (spec: ui/plugin-window.md "Frame"). Opens real windows:
    /// `cargo test -p os-app --bin lemonstudio resizable_windows_maximize -- --ignored`.
    #[test]
    #[ignore]
    fn resizable_windows_maximize() {
        let class = wide("STATIC");
        let owner = unsafe {
            CreateWindowExW(0, class.as_ptr(), std::ptr::null(), WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN, 0, 0, 1300, 900, std::ptr::null_mut(), std::ptr::null_mut(), GetModuleHandleW(std::ptr::null()), std::ptr::null())
        };
        assert!(!owner.is_null());
        let info = ChannelInfo { name: "Tesla".into(), target: "Insert 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None };
        let mut r: RECT = unsafe { std::mem::zeroed() };
        unsafe { GetWindowRect(owner, &mut r) };
        let f = FrameWindow::new(owner as isize, info, (480.0, 270.0), 1.0, (r.left + 300, r.top + 200), false).unwrap();
        f.raise();
        let me = || unsafe { parent_rect(f.hwnd) };
        // A double-click is a press and a release, then the system's double-click message and a release.
        let click = |x: f32, y: f32, double: bool| {
            let at = (x as isize) | ((y as isize) << 16);
            let presses: &[u32] = if double { &[WM_LBUTTONDOWN, WM_LBUTTONDBLCLK] } else { &[WM_LBUTTONDOWN] };
            for &m in presses {
                unsafe { SendMessageW(f.hwnd, m, 1, at) };
                unsafe { SendMessageW(f.hwnd, WM_LBUTTONUP, 0, at) };
            }
        };
        let resizes = || f.take_events().into_iter().filter_map(|e| if let FrameEvent::Resize(w, h) = e { Some((w, h)) } else { None }).collect::<Vec<_>>();
        // The plugin takes the size it is asked for (the app does this through the host link).
        let follow = |want: &[(u32, u32)]| {
            let &(w, h) = want.last().unwrap();
            f.set_editor_size((w as f32, h as f32));
        };
        let work = [40, 60, 1240, 820];
        f.clip_to(work, &[]);
        let before = me();

        // A fixed size has no button: the click there is minimize's.
        let (w, _) = f.state.borrow().size();
        click(w - 35.0, 15.0, false);
        assert!(resizes().is_empty(), "a fixed size does not maximize");
        f.raise();

        f.set_resizable(Some((400.0, 200.0)));
        let (w, _) = f.state.borrow().size();
        click(w - 35.0, 15.0, false);
        let ay = f.state.borrow().area_y();
        let full = ((1200.0 - 2.0 * BORDER) as u32, (760.0 - ay - BORDER) as u32);
        let asked = resizes();
        assert_eq!(asked, vec![full], "the editor fills the workspace");
        follow(&asked);
        assert_eq!(me(), work, "the frame is the workspace");

        // The workspace grows (the browser hidden): it keeps filling it.
        let wider = [0, 60, 1240, 820];
        f.clip_to(wider, &[]);
        let asked = resizes();
        assert_eq!(asked, vec![(full.0 + 40, full.1)]);
        follow(&asked);
        assert_eq!(me(), wider);

        // A double-click on the title bar restores the place and size before.
        click(200.0, 15.0, true);
        let asked = resizes();
        assert_eq!(asked, vec![(480, 270)]);
        follow(&asked);
        assert_eq!(me(), before);

        // Moving it leaves it as it is; the workspace changing no longer refits it.
        click(200.0, 15.0, true);
        follow(&resizes());
        click(200.0, 15.0, false);
        f.clip_to(work, &[]);
        assert!(resizes().is_empty(), "no longer maximized");
        f.set_editor_size((480.0, 270.0));

        // Detached: the monitor's work area.
        f.set_detached(owner as isize, true);
        let (w, _) = f.state.borrow().size();
        click(w - 35.0, 15.0, false);
        let mon = unsafe {
            use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
            let mut mi: MONITORINFO = std::mem::zeroed();
            mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            GetMonitorInfoW(MonitorFromWindow(f.hwnd, MONITOR_DEFAULTTONEAREST), &mut mi);
            mi.rcWork
        };
        let asked = resizes();
        assert_eq!(asked, vec![((mon.right - mon.left) as u32 - 2 * BORDER as u32, ((mon.bottom - mon.top) as f32 - ay - BORDER) as u32)]);
        follow(&asked);
        let mut r: RECT = unsafe { std::mem::zeroed() };
        unsafe { GetWindowRect(f.hwnd, &mut r) };
        assert_eq!([r.left, r.top, r.right, r.bottom], [mon.left, mon.top, mon.right, mon.bottom]);
        let (w, _) = f.state.borrow().size();
        click(w - 35.0, 15.0, false);
        assert_eq!(resizes(), vec![(480, 270)], "restored");
        drop(f);
        unsafe { DestroyWindow(owner) };
    }
}

