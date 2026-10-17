// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! `--editor out.bmp`: observe a native plugin's editor in our own process (black-box). Creates a host
//! window off every screen, asks the plugin to show its editor in it (plugin dispatcher `ShowEditor`,
//! parent HWND in Value, public SDK), reports what the plugin created (`EditorHandle`, its class,
//! styles and size), pumps messages and `Idle` for a while, captures the window with PrintWindow
//! (no input, nothing on screen), then hides the editor again (ShowEditor with 0).
#![cfg(windows)]

use std::ffi::c_void;
use std::time::{Duration, Instant};

use os_native::Plugin;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::Storage::Xps::PrintWindow;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe extern "system" fn wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    DefWindowProcW(h, m, w, l)
}

/// Creates the off-screen host window (top-level, never activated).
pub fn host_window(w: i32, h: i32) -> HWND {
    unsafe {
        let inst = GetModuleHandleW(std::ptr::null());
        let class = wide("OSNativeProbeHost");
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: inst, lpszClassName: class.as_ptr(), hbrBackground: (COLOR_WINDOW + 1) as _, ..std::mem::zeroed() };
        RegisterClassW(&wc);
        let title = wide("native editor probe");
        let hwnd = CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP | WS_CLIPCHILDREN,
            -20000,
            -20000,
            w,
            h,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            inst,
            std::ptr::null(),
        );
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        hwnd
    }
}

pub fn pump(p: &mut Plugin, ms: u64) {
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        unsafe {
            let mut m: MSG = std::mem::zeroed();
            while PeekMessageW(&mut m, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&m);
                DispatchMessageW(&m);
            }
        }
        p.idle();
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn class_of(h: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(h, buf.as_mut_ptr(), 256) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn rect_of(h: HWND) -> RECT {
    let mut r: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetWindowRect(h, &mut r) };
    r
}

/// Describes a window and its direct children (one level), for the report.
pub fn describe(h: HWND, depth: usize) {
    let r = rect_of(h);
    let style = unsafe { GetWindowLongPtrW(h, GWL_STYLE) } as u32;
    let ex = unsafe { GetWindowLongPtrW(h, GWL_EXSTYLE) } as u32;
    let parent = unsafe { GetParent(h) };
    let owner = unsafe { GetWindow(h, GW_OWNER) };
    println!(
        "{:indent$}hwnd {:#x} class {:?} size {}x{} style {style:#x} ex {ex:#x} parent {:#x} owner {:#x} visible {}",
        "",
        h as usize,
        class_of(h),
        r.right - r.left,
        r.bottom - r.top,
        parent as usize,
        owner as usize,
        unsafe { IsWindowVisible(h) } != 0,
        indent = depth * 2
    );
    if depth >= 2 {
        return;
    }
    let mut c = unsafe { GetWindow(h, GW_CHILD) };
    let mut n = 0;
    while !c.is_null() && n < 12 {
        describe(c, depth + 1);
        c = unsafe { GetWindow(c, GW_HWNDNEXT) };
        n += 1;
    }
}

/// PrintWindow of `hwnd` into a top-down 32-bit BMP.
pub fn capture(hwnd: HWND, path: &std::path::Path) {
    unsafe {
        let r = rect_of(hwnd);
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        if w <= 0 || h <= 0 {
            println!("capture: empty window");
            return;
        }
        let dc = GetDC(std::ptr::null_mut());
        let mem = CreateCompatibleDC(dc);
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader = BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB, ..std::mem::zeroed() };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        let old = SelectObject(mem, bmp);
        PrintWindow(hwnd, mem, 2);
        let px = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize);
        let mut f = Vec::new();
        f.extend_from_slice(b"BM");
        f.extend_from_slice(&((54 + px.len()) as u32).to_le_bytes());
        f.extend_from_slice(&[0, 0, 0, 0]);
        f.extend_from_slice(&54u32.to_le_bytes());
        f.extend_from_slice(&40u32.to_le_bytes());
        f.extend_from_slice(&w.to_le_bytes());
        f.extend_from_slice(&(-h).to_le_bytes());
        f.extend_from_slice(&1u16.to_le_bytes());
        f.extend_from_slice(&32u16.to_le_bytes());
        f.extend_from_slice(&[0; 24]);
        f.extend_from_slice(px);
        std::fs::write(path, f).expect("write bmp");
        SelectObject(mem, old);
        DeleteObject(bmp);
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), dc);
        println!("captured {}x{} -> {}", w, h, path.display());
    }
}

/// Runs the editor observation on a loaded plugin.
pub fn run(p: &mut Plugin, out: &std::path::Path) {
    let host = host_window(900, 700);
    println!("host window {:#x}", host as usize);
    println!("editor handle before ShowEditor: {:#x}", p.editor_handle());
    let r = p.dispatch(0, 0, host as isize);
    println!("ShowEditor(parent) returned {r}; editor handle {:#x}", p.editor_handle());
    pump(p, 300);
    let ed = p.editor_handle() as HWND;
    if !ed.is_null() {
        describe(ed, 0);
    }
    println!("host window children:");
    describe(host, 0);
    // Fit the host to the editor so the capture shows all of it.
    if !ed.is_null() {
        let r = rect_of(ed);
        unsafe { SetWindowPos(host, std::ptr::null_mut(), 0, 0, (r.right - r.left).max(1), (r.bottom - r.top).max(1), SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE) };
    }
    pump(p, 300);
    capture(host, out);
    pump(p, 200);
    capture(host, out);
    let r = p.dispatch(0, 0, 0);
    println!("ShowEditor(0) returned {r}; editor handle {:#x}", p.editor_handle());
    pump(p, 200);
    let ed0 = p.editor_handle() as HWND;
    if !ed0.is_null() {
        println!("after hide: IsWindow {}", unsafe { IsWindow(ed0) } != 0);
        if unsafe { IsWindow(ed0) } != 0 {
            describe(ed0, 0);
        }
    }

    // Re-show in a second parent (dock <-> detach moves the editor to another frame).
    let host2 = host_window(900, 700);
    let r = p.dispatch(0, 0, host2 as isize);
    pump(p, 200);
    println!("ShowEditor(second parent) returned {r}; editor handle {:#x}", p.editor_handle());
    let ed2 = p.editor_handle() as HWND;
    if !ed2.is_null() {
        describe(ed2, 0);
    }
    unsafe { DestroyWindow(host) };
    pump(p, 100);

    // Destroy the parent while the editor is still shown in it, then show it again elsewhere.
    unsafe { DestroyWindow(host2) };
    pump(p, 200);
    let ed3 = p.editor_handle() as HWND;
    println!("parent destroyed under the editor: editor handle {:#x} IsWindow {}", ed3 as usize, unsafe { IsWindow(ed3) } != 0);
    let host3 = host_window(900, 700);
    let r = p.dispatch(0, 0, host3 as isize);
    pump(p, 300);
    println!("ShowEditor(third parent) returned {r}; editor handle {:#x}", p.editor_handle());
    let ed4 = p.editor_handle() as HWND;
    if !ed4.is_null() && unsafe { IsWindow(ed4) } != 0 {
        describe(ed4, 0);
        let r = rect_of(ed4);
        unsafe { SetWindowPos(host3, std::ptr::null_mut(), 0, 0, (r.right - r.left).max(1), (r.bottom - r.top).max(1), SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE) };
        pump(p, 200);
        let out3 = out.with_extension("reshown.bmp");
        capture(host3, &out3);
        pump(p, 100);
        capture(host3, &out3);
    }
    // Leave the editor shown: the plugin is destroyed with its editor open (app exit path).
    println!("destroying the plugin with its editor shown");
    let _ = ed4;
    EDITOR_AT_DROP.store(p.editor_handle() as usize, std::sync::atomic::Ordering::SeqCst);
}

/// The editor handle when the plugin was dropped, to check afterwards whether the plugin destroyed it.
pub static EDITOR_AT_DROP: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn after_drop() {
    let h = EDITOR_AT_DROP.load(std::sync::atomic::Ordering::SeqCst) as HWND;
    if !h.is_null() {
        println!("after DestroyObject: editor IsWindow {}", unsafe { IsWindow(h) } != 0);
    }
}
