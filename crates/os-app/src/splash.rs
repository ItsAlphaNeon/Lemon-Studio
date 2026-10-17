// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The startup splash: General ▸ Miscellaneous "Show startup splash screen" (`prefs::SHOW_SPLASH`). With the check on,
//! a borderless window centred on the primary monitor shows the Lemon Studio logo mark (`logo.rs`) from launch until
//! the main window shows its first frame; with it off there is none. A program setting, read once at start (`main`);
//! `--screenshot`, the helper processes and tests never open it.
//!
//! The window runs on a thread of its own, so it stays drawn while the UI thread loads the project and the plugins.
//! Windows only for now: elsewhere `open` shows nothing.
// spec: ui/settings-choices.md "Miscellaneous: naming, detached plugins and startup"

use os_ui::Canvas;

/// The splash's side in logical px (a square).
pub const SIDE: f32 = 320.0;

/// Whether this start shows the splash.
pub fn wanted() -> bool {
    crate::prefs::check(&crate::prefs::SHOW_SPLASH)
}

/// The splash's picture at UI scale `scale`: its side in device px and its pixels (`0x00RRGGBB`, rows top down). The
/// logo mark in the theme's pill colour (lemon in the Lemon design) on its workspace background.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn picture(scale: f32) -> Option<(u32, Vec<u32>)> {
    let side = (SIDE * scale).round().max(1.0) as u32;
    let mut c = Canvas::new(side, side, scale).ok()?;
    let t = crate::app::design_theme(crate::app::saved_design(), true).colors;
    c.clear(t.workspace);
    crate::icons::logo(&mut c, SIDE / 2.0, SIDE / 2.0, SIDE * 0.35, t.pill);
    let mut px = vec![0; (side * side) as usize];
    c.copy_to_xrgb(&mut px);
    Some((side, px))
}

/// The splash while it shows: dropping it closes the window.
pub struct Splash {
    #[cfg(windows)]
    shared: std::sync::Arc<std::sync::Mutex<win::Shared>>,
}

/// Opens the splash when its check is on (`wanted`). None when it is off, or where there is no splash yet.
pub fn open() -> Option<Splash> {
    if !wanted() {
        return None;
    }
    #[cfg(windows)]
    return win::open().map(|shared| Splash { shared });
    #[cfg(not(windows))]
    None
}

impl Drop for Splash {
    fn drop(&mut self) {
        #[cfg(windows)]
        win::close(&self.shared);
    }
}

#[cfg(windows)]
mod win {
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::sync::{Arc, Mutex};

    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{
        BeginPaint, EndPaint, GetMonitorInfoW, MonitorFromPoint, SetDIBitsToDevice, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, MONITORINFO,
        MONITOR_DEFAULTTOPRIMARY, PAINTSTRUCT,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, LoadCursorW, PostMessageW, PostQuitMessage, RegisterClassW,
        ShowWindow, TranslateMessage, IDC_APPSTARTING, MA_NOACTIVATE, MSG, SW_SHOWNOACTIVATE, WM_CLOSE, WM_DESTROY, WM_ERASEBKGND, WM_MOUSEACTIVATE,
        WM_PAINT, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
    };

    /// Between the UI thread and the splash's: the window once made (0 until then), and whether it was closed.
    #[derive(Default)]
    pub struct Shared {
        hwnd: isize,
        closed: bool,
    }

    thread_local! {
        /// The splash thread's picture: side (device px) and pixels.
        static PICTURE: RefCell<(i32, Vec<u32>)> = const { RefCell::new((0, Vec::new())) };
    }

    const CLASS: &str = "LemonStudioSplash";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    pub fn open() -> Option<Arc<Mutex<Shared>>> {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let s = shared.clone();
        std::thread::Builder::new().name("splash".into()).spawn(move || run(&s)).ok()?;
        Some(shared)
    }

    /// Closes the window, or keeps it from being made if the thread has not got that far.
    pub fn close(shared: &Mutex<Shared>) {
        let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
        g.closed = true;
        if g.hwnd != 0 {
            // SAFETY: a window of the splash thread, or one already gone (the post then fails harmlessly).
            unsafe { PostMessageW(g.hwnd as HWND, WM_CLOSE, 0, 0) };
        }
    }

    /// The splash thread: makes the window, then serves it until it is closed.
    fn run(shared: &Mutex<Shared>) {
        // SAFETY: plain Win32 calls on this thread's own window; the structs are zeroed C structs.
        unsafe {
            // The primary monitor, in physical px: winit made the process per-monitor DPI aware when the event loop
            // was built, before this.
            let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
            let mut mi: MONITORINFO = std::mem::zeroed();
            mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(mon, &mut mi) == 0 {
                return;
            }
            let (mut dx, mut dy) = (96, 96);
            if GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) != 0 {
                dx = 96;
            }
            // The UI scale there: the monitor's, or Main GUI scaling's (spec: ui/ui-scaling.md §1).
            let scale = crate::gui_scale(f64::from(dx) / 96.0) as f32;
            let Some((side, px)) = super::picture(scale) else { return };
            let side = side as i32;
            PICTURE.with(|p| *p.borrow_mut() = (side, px));
            let r = mi.rcMonitor;
            let (x, y) = (r.left + (r.right - r.left - side) / 2, r.top + (r.bottom - r.top - side) / 2);
            let inst = GetModuleHandleW(std::ptr::null());
            let class = wide(CLASS);
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(splash_proc);
            wc.hInstance = inst;
            wc.hCursor = LoadCursorW(std::ptr::null_mut(), IDC_APPSTARTING);
            wc.lpszClassName = class.as_ptr();
            RegisterClassW(&wc);
            let title = wide("Lemon Studio");
            // A tool window takes no taskbar button; never activated, so the main window gets the focus as before.
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class.as_ptr(),
                title.as_ptr(),
                WS_POPUP,
                x,
                y,
                side,
                side,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                inst,
                std::ptr::null(),
            );
            if hwnd.is_null() {
                return;
            }
            {
                let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
                if g.closed {
                    DestroyWindow(hwnd);
                    return;
                }
                g.hwnd = hwnd as isize;
            }
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    unsafe extern "system" fn splash_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        // SAFETY: called by the system on the splash thread for its own window.
        unsafe {
            match msg {
                WM_ERASEBKGND => 1,
                WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
                WM_PAINT => {
                    let mut ps: PAINTSTRUCT = std::mem::zeroed();
                    let hdc = BeginPaint(hwnd, &mut ps);
                    PICTURE.with(|p| {
                        let (side, px) = &*p.borrow();
                        if px.len() == (*side * *side) as usize {
                            let mut bmi: BITMAPINFO = std::mem::zeroed();
                            bmi.bmiHeader = BITMAPINFOHEADER {
                                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                                biWidth: *side,
                                biHeight: -*side,
                                biPlanes: 1,
                                biBitCount: 32,
                                biCompression: BI_RGB,
                                ..std::mem::zeroed()
                            };
                            let n = *side as u32;
                            SetDIBitsToDevice(hdc, 0, 0, n, n, 0, 0, 0, n, px.as_ptr() as *const c_void, &bmi, DIB_RGB_COLORS);
                        }
                    });
                    EndPaint(hwnd, &ps);
                    0
                }
                WM_DESTROY => {
                    PostQuitMessage(0);
                    0
                }
                _ => DefWindowProcW(hwnd, msg, wp, lp),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prefs::{check, find_check, set_check, SHOW_SPLASH, SILENT_STARTUP};
    use os_ui::Color;

    /// "Show startup splash screen" (spec: ui/settings-choices.md "Miscellaneous: naming, detached plugins and
    /// startup"): on as found, a program setting; with it off the start opens no splash.
    #[test]
    fn the_splash_follows_its_check() {
        assert_eq!(find_check("Show startup splash screen").map(|c| c.key), Some(SHOW_SPLASH.key));
        assert!(wanted(), "on by default");
        set_check(&SHOW_SPLASH, false);
        assert!(!wanted());
        assert!(open().is_none(), "off: no splash");
        set_check(&SHOW_SPLASH, true);
        assert!(wanted());
    }

    /// The splash (spec, as above): the logo mark of `logo.rs` centred on the theme's dark background, 320 logical px
    /// square at the UI scale.
    #[test]
    fn the_splash_shows_the_logo_mark() {
        let t = crate::app::design_theme(crate::app::saved_design(), true).colors;
        let xrgb = |c: Color| (u32::from(c.r) << 16) | (u32::from(c.g) << 8) | u32::from(c.b);
        let (bg, mark) = (xrgb(t.workspace), xrgb(t.pill));
        for scale in [1.0, 1.5] {
            let (side, px) = picture(scale).expect("canvas");
            assert_eq!(side, (SIDE * scale) as u32, "scale {scale}");
            let n = side as usize;
            assert_eq!(px.len(), n * n);
            for i in [0, n - 1, n * (n - 1), n * n - 1] {
                assert_eq!(px[i], bg, "corner, scale {scale}");
            }
            assert!(px.iter().filter(|&&p| p == mark).count() > n * n / 10, "the mark is drawn, scale {scale}");
            // The drawn part sits in the middle.
            let (mut x0, mut y0, mut x1, mut y1) = (n, n, 0, 0);
            for (i, _) in px.iter().enumerate().filter(|&(_, &p)| p != bg) {
                let (x, y) = (i % n, i / n);
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
            let off = |a: usize, b: usize| ((a + b) as f32 / 2.0 - n as f32 / 2.0).abs() / n as f32;
            assert!(off(x0, x1) < 0.03 && off(y0, y1) < 0.03, "centred, scale {scale}: {x0}..{x1}, {y0}..{y1} of {n}");
            assert!(x0 > n / 20 && x1 < n - n / 20, "with a margin, scale {scale}");
        }
    }

    /// "Silent startup" (spec, as above): stored and shown, off as found. Lemon Studio has no startup sound, so it
    /// silences nothing yet.
    #[test]
    fn silent_startup_is_stored() {
        assert_eq!(find_check("Silent startup").map(|c| c.key), Some(SILENT_STARTUP.key));
        assert!(!check(&SILENT_STARTUP), "off by default");
        set_check(&SILENT_STARTUP, true);
        assert!(check(&SILENT_STARTUP));
        set_check(&SILENT_STARTUP, false);
    }
}
