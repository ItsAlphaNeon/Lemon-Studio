// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Win32 parts of a Lemon plugin editor: window classes, painting a canvas into a window, the cursor held in place
//! during a drag, popup menus painted with `os_ui::kit::menu` (never native menus), and the "Type in value" box.

use std::ffi::c_void;
use std::time::Instant;

use os_theme::Palette;
use os_ui::kit::menu::{self as kmenu, MenuStyle};
use os_ui::Canvas;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, ClientToScreen, CreateFontW, DeleteObject, EndPaint, GetMonitorInfoW, InvalidateRect, MonitorFromPoint, SetDIBitsToDevice, UpdateWindow, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HDC, MONITORINFO, MONITOR_DEFAULTTONEAREST, PAINTSTRUCT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::EM_SETSEL;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetCapture, GetFocus, GetKeyState, ReleaseCapture, SetCapture, SetFocus, VK_CONTROL, VK_DOWN, VK_ESCAPE, VK_MENU, VK_RETURN, VK_SHIFT, VK_UP};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::{MenuDef, Opener};

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Registers a window class named `<kind><address of proc>`: one class per module, so two plugin DLLs (or two copies
/// of one) never share a class with another module's window procedure.
pub fn register(kind: &str, proc_: WNDPROC) -> Vec<u16> {
    let class = wide(&format!("{kind}{:x}", proc_.map_or(0, |p| p as usize)));
    unsafe {
        let wc = WNDCLASSW {
            style: CS_DBLCLKS,
            lpfnWndProc: proc_,
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: GetModuleHandleW(std::ptr::null()),
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class.as_ptr(),
        };
        RegisterClassW(&wc);
    }
    class
}

/// Modifier keys held now.
pub fn mods() -> (bool, bool, bool) {
    let down = |vk: u16| unsafe { GetKeyState(i32::from(vk)) } < 0;
    (down(VK_CONTROL), down(VK_SHIFT), down(VK_MENU))
}

/// Client coordinates of a mouse message.
pub fn xy(l: LPARAM) -> (i32, i32) {
    ((l & 0xffff) as u16 as i16 as i32, ((l >> 16) & 0xffff) as u16 as i16 as i32)
}

/// Screen position of a client point.
pub fn to_screen(hwnd: HWND, x: i32, y: i32) -> (i32, i32) {
    let mut p = POINT { x, y };
    unsafe { ClientToScreen(hwnd, &mut p) };
    (p.x, p.y)
}

/// Copies `px` (XRGB rows of `w`) into the device context at (0, 0).
pub fn blit(hdc: HDC, px: &[u32], w: u32, h: u32) {
    unsafe {
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
        SetDIBitsToDevice(hdc, 0, 0, w, h, 0, 0, 0, h, px.as_ptr() as *const c_void, &bmi, DIB_RGB_COLORS);
    }
}

/// The cursor hidden and held in place during a drag: movement arrives as deltas from the anchor.
#[derive(Default)]
pub struct CursorLock {
    anchor: Option<POINT>,
}

impl CursorLock {
    pub fn locked(&self) -> bool {
        self.anchor.is_some()
    }

    /// Hides the cursor where it is and captures the mouse for `hwnd`.
    pub fn lock(&mut self, hwnd: HWND) {
        unsafe {
            let mut p = POINT { x: 0, y: 0 };
            GetCursorPos(&mut p);
            self.anchor = Some(p);
            if ShowCursor(0) >= 0 {
                while ShowCursor(0) >= 0 {}
            }
            if GetCapture() != hwnd {
                SetCapture(hwnd);
            }
        }
    }

    /// The movement since the last call (screen px), putting the cursor back on the anchor. `None` when not locked.
    pub fn delta(&mut self) -> Option<(i32, i32)> {
        let a = self.anchor?;
        let mut p = POINT { x: 0, y: 0 };
        unsafe { GetCursorPos(&mut p) };
        let d = (p.x - a.x, p.y - a.y);
        if d != (0, 0) {
            unsafe { SetCursorPos(a.x, a.y) };
        }
        Some(d)
    }

    /// Shows the cursor again where the drag started.
    pub fn unlock(&mut self) {
        if let Some(p) = self.anchor.take() {
            unsafe {
                SetCursorPos(p.x, p.y);
                while ShowCursor(1) < 0 {}
            }
        }
    }
}

/// Begins a paint: the device context and the paint struct to end it with.
pub fn begin_paint(hwnd: HWND) -> (HDC, PAINTSTRUCT) {
    unsafe {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        (hdc, ps)
    }
}

pub fn end_paint(hwnd: HWND, ps: &PAINTSTRUCT) {
    unsafe { EndPaint(hwnd, ps) };
}

// -------------------------------------------------------------------------------------------------------------------
// Popup menus: an owned popup window painted with `kit::menu`, run modally until a pick or a cancel.

struct Pop {
    canvas: Canvas,
    items: Vec<os_ui::kit::menu::MenuItem>,
    layout: kmenu::MenuLayout,
    style: MenuStyle,
    hover: Option<usize>,
    current: Option<usize>,
    palette: Palette,
    scale: f32,
    buf: Vec<u32>,
}

enum PopRes<C> {
    Pick(C),
    Cancel,
    /// The mouse went back to the parent menu.
    Back,
}

unsafe extern "system" fn pop_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_PAINT {
            let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Pop;
            let (hdc, ps) = begin_paint(hwnd);
            if !p.is_null() {
                let pop = &mut *p;
                pop.canvas.begin_frame(None);
                kmenu::paint(&mut pop.canvas, &pop.palette, &pop.items, &pop.layout, pop.style, 0.0, 0.0, pop.hover, pop.current);
                let (w, h) = ((pop.layout.w * pop.scale).ceil() as u32, (pop.layout.h * pop.scale).ceil() as u32);
                pop.buf.resize((w * h) as usize, 0);
                pop.canvas.copy_to_xrgb(&mut pop.buf);
                blit(hdc, &pop.buf, w, h);
            }
            end_paint(hwnd, &ps);
            return 0;
        }
        if msg == WM_ERASEBKGND {
            return 1;
        }
        if msg == WM_MOUSEACTIVATE {
            return MA_NOACTIVATE as LRESULT;
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}

/// Work area of the monitor at a screen point.
fn work_area(x: i32, y: i32) -> RECT {
    unsafe {
        let mon = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(mon, &mut mi);
        mi.rcWork
    }
}

/// Shows a menu at screen (`x`, `y`); the picked entry's command.
pub fn popup<C: Clone>(owner: HWND, x: i32, y: i32, def: &MenuDef<C>, pal: &Palette, scale: f32) -> Option<C> {
    match popup_at(owner, x, y, def, pal, scale, None) {
        PopRes::Pick(c) => Some(c),
        _ => None,
    }
}

fn popup_at<C: Clone>(owner: HWND, x: i32, y: i32, def: &MenuDef<C>, pal: &Palette, scale: f32, parent: Option<Opener>) -> PopRes<C> {
    if crate::no_modal() {
        return PopRes::Cancel;
    }
    unsafe {
        let class = register("OpenStudioPluginPopup", Some(pop_proc));
        let mut measure = Canvas::new(8, 8, scale).expect("canvas");
        let layout = kmenu::layout(&mut measure, &def.items, def.style);
        let (w, h) = ((layout.w * scale).ceil() as i32, (layout.h * scale).ceil() as i32);
        let wa = work_area(x, y);
        let mut px = x;
        let mut py = y;
        if px + w > wa.right {
            px = match parent {
                Some(p) => p.menu[0] - w,
                None => wa.right - w,
            };
        }
        if py + h > wa.bottom {
            py = (wa.bottom - h).max(wa.top);
        }
        let pop = Box::new(Pop {
            canvas: Canvas::new(w.max(1) as u32, h.max(1) as u32, scale).expect("canvas"),
            items: def.items.clone(),
            layout,
            style: def.style,
            hover: None,
            current: def.current,
            palette: pal.clone(),
            scale,
            buf: Vec::new(),
        });
        let root = GetAncestor(owner, GA_ROOT);
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            class.as_ptr(),
            std::ptr::null(),
            WS_POPUP,
            px,
            py,
            w,
            h,
            root,
            std::ptr::null_mut(),
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null(),
        );
        let pop = Box::into_raw(pop);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, pop as isize);
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetCapture(hwnd);
        let rect = RECT { left: px, top: py, right: px + w, bottom: py + h };
        let menu = [px, py, px + w, py + h];
        let opened = Instant::now();
        let mut start = POINT { x: 0, y: 0 };
        GetCursorPos(&mut start);
        let mut moved = false;
        let item_at = |pop: &Pop, sx: i32, sy: i32| -> Option<usize> {
            let (lx, ly) = ((sx - px) as f32 / scale, (sy - py) as f32 / scale);
            pop.layout.item_at(&pop.items, lx, ly)
        };
        let inside = |r: &RECT, sx: i32, sy: i32| sx >= r.left && sx < r.right && sy >= r.top && sy < r.bottom;
        let result = 'run: loop {
            let mut msg: MSG = std::mem::zeroed();
            if GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) <= 0 {
                PostQuitMessage(msg.wParam as i32);
                break PopRes::Cancel;
            }
            if GetCapture() != hwnd && msg.message != WM_PAINT && msg.message != WM_TIMER {
                SetCapture(hwnd);
            }
            let mut cur = POINT { x: 0, y: 0 };
            GetCursorPos(&mut cur);
            match msg.message {
                WM_MOUSEMOVE => {
                    if (cur.x - start.x).abs() + (cur.y - start.y).abs() > 2 {
                        moved = true;
                    }
                    if parent.is_some_and(|p| p.takes_back(menu, (cur.x, cur.y))) {
                        break 'run PopRes::Back;
                    }
                    let hov = if inside(&rect, cur.x, cur.y) { item_at(&*pop, cur.x, cur.y) } else { None };
                    let hov = hov.filter(|&i| def.items[i].clickable());
                    if hov != (*pop).hover {
                        (*pop).hover = hov;
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                        UpdateWindow(hwnd);
                    }
                    // Submenu: opens while its row is hovered, until the pointer moves on to another row of this menu.
                    if let Some((i, sub)) = hov.and_then(|i| Some((i, def.sub(i)?))) {
                        let row = (&(*pop).layout.rows)[i];
                        let sx = px + ((row.x + row.w) * scale) as i32 - 2;
                        let sy = py + (row.y * scale) as i32 - 2;
                        match popup_at(owner, sx, sy, sub, pal, scale, Some(Opener::new(menu, row, scale))) {
                            PopRes::Pick(c) => break 'run PopRes::Pick(c),
                            PopRes::Cancel => break 'run PopRes::Cancel,
                            PopRes::Back => {
                                SetCapture(hwnd);
                            }
                        }
                    }
                }
                WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_NCLBUTTONDOWN | WM_NCRBUTTONDOWN => {
                    if !inside(&rect, cur.x, cur.y) {
                        break 'run PopRes::Cancel;
                    }
                    moved = true;
                }
                WM_LBUTTONUP | WM_RBUTTONUP => {
                    // The release of the click that opened the menu does not pick.
                    if !moved && opened.elapsed().as_millis() < 400 {
                        continue;
                    }
                    if inside(&rect, cur.x, cur.y) {
                        if let Some(i) = item_at(&*pop, cur.x, cur.y) {
                            if def.items[i].clickable() && def.sub(i).is_none() {
                                break 'run PopRes::Pick(def.cmds[i].clone());
                            }
                        }
                    }
                }
                WM_KEYDOWN | WM_SYSKEYDOWN => {
                    let vk = msg.wParam as u16;
                    if vk == VK_ESCAPE {
                        break 'run PopRes::Cancel;
                    }
                    let n = def.items.len();
                    if vk == VK_DOWN || vk == VK_UP {
                        let mut i = (*pop).hover.or((*pop).current).map_or(if vk == VK_DOWN { n - 1 } else { 0 }, |i| i);
                        for _ in 0..n {
                            i = if vk == VK_DOWN { (i + 1) % n } else { (i + n - 1) % n };
                            if def.items[i].clickable() {
                                break;
                            }
                        }
                        (*pop).hover = Some(i);
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                    } else if vk == VK_RETURN {
                        if let Some(i) = (*pop).hover {
                            if def.items[i].clickable() && def.sub(i).is_none() {
                                break 'run PopRes::Pick(def.cmds[i].clone());
                            }
                        }
                    }
                }
                WM_CANCELMODE | WM_ACTIVATEAPP => break 'run PopRes::Cancel,
                _ => {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        };
        ReleaseCapture();
        DestroyWindow(hwnd);
        drop(Box::from_raw(pop));
        result
    }
}

/// A small edit box at screen (`x`, `y`) for "Type in value"; Enter accepts, Esc or a click elsewhere cancels.
pub fn type_in(owner: HWND, x: i32, y: i32, text: &str, scale: f32) -> Option<String> {
    if crate::no_modal() {
        return None;
    }
    unsafe {
        let class = wide("EDIT");
        let init = wide(text);
        let (w, h) = ((140.0 * scale) as i32, (22.0 * scale) as i32);
        let root = GetAncestor(owner, GA_ROOT);
        let edit = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            class.as_ptr(),
            init.as_ptr(),
            WS_POPUP | WS_BORDER | ES_AUTOHSCROLL as u32,
            x,
            y,
            w,
            h,
            root,
            std::ptr::null_mut(),
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null(),
        );
        if edit.is_null() {
            return None;
        }
        let face = wide("Segoe UI");
        let font = CreateFontW(-(14.0 * scale) as i32, 0, 0, 0, 400, 0, 0, 0, 0, 0, 0, 5, 0, face.as_ptr());
        SendMessageW(edit, WM_SETFONT, font as WPARAM, 1);
        ShowWindow(edit, SW_SHOW);
        SetForegroundWindow(edit);
        SetFocus(edit);
        SendMessageW(edit, EM_SETSEL, 0, -1);
        let result = loop {
            let mut msg: MSG = std::mem::zeroed();
            if GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) <= 0 {
                PostQuitMessage(msg.wParam as i32);
                break None;
            }
            if msg.message == WM_KEYDOWN && msg.wParam as u16 == VK_RETURN {
                let mut buf = vec![0u16; 256];
                let n = GetWindowTextW(edit, buf.as_mut_ptr(), buf.len() as i32) as usize;
                break Some(String::from_utf16_lossy(&buf[..n]));
            }
            if msg.message == WM_KEYDOWN && msg.wParam as u16 == VK_ESCAPE {
                break None;
            }
            if matches!(msg.message, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_NCLBUTTONDOWN) && msg.hwnd != edit {
                break None;
            }
            if GetFocus() != edit && msg.message == WM_KEYDOWN {
                break None;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        };
        DestroyWindow(edit);
        DeleteObject(font);
        SetFocus(owner);
        result
    }
}
