// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The main window on macOS, where winit leaves gaps: its frame in screen coordinates (title-bar moves and edge resizes
//! are done by hand, as on Windows: winit cannot start a system resize of a borderless window here), maximizing,
//! quitting from the menu bar or the Dock, which AppKit would do without asking to save, and files opened from the
//! Finder (they come as an Apple Event, not on the command line).
//!
//! Maximizing is done here too: winit asks AppKit whether a borderless window is zoomed by giving it a title bar for a
//! moment, and a window whose style changes while a mouse button is down never gets that button's release.
//!
//! Screen coordinates are winit's: device px from the top-left of the primary screen.

use std::cell::Cell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{sel, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSApplicationTerminateReply, NSEvent, NSScreen, NSView, NSWindow};
use objc2_foundation::{NSArray, NSPoint, NSRect, NSSize, NSURL};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

/// Quit was chosen (menu bar, Dock, Cmd+Q, logging out) and not answered yet.
static QUIT: AtomicBool = AtomicBool::new(false);

/// Files the Finder asked the app to open, not handled yet.
static OPENED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

thread_local! {
    /// The frame the maximized window goes back to.
    static RESTORE: Cell<Option<NSRect>> = const { Cell::new(None) };
}

/// Whether the user asked to quit since the last call: the app asks about unsaved changes and answers with `answer_quit`.
pub fn take_quit() -> bool {
    QUIT.swap(false, Ordering::AcqRel)
}

/// The answer to a quit (`take_quit`). `false` cancels it (a logout or shutdown that asked stops too); `true` lets
/// AppKit end the process, which winit reports as the loop exiting (the app shuts down in `exiting`) — winit cannot end
/// its loop by itself while AppKit waits for this answer.
pub fn answer_quit(quit: bool) {
    if let Some(mtm) = MainThreadMarker::new() {
        NSApplication::sharedApplication(mtm).replyToApplicationShouldTerminate(quit);
    }
}

/// `-applicationShouldTerminate:` for winit's application delegate: the answer waits (`NSTerminateLater`) for the app,
/// which asks about unsaved changes first (winit would let AppKit end the process at once).
extern "C-unwind" fn should_terminate(_this: &AnyObject, _cmd: Sel, _app: &NSApplication) -> NSApplicationTerminateReply {
    QUIT.store(true, Ordering::Release);
    crate::wake_ui();
    NSApplicationTerminateReply::TerminateLater
}

/// Files the Finder asked the app to open since the last call (a double-clicked project, a file dropped on the Dock
/// icon), oldest first.
pub fn take_opened() -> Vec<PathBuf> {
    OPENED.lock().map(|mut o| std::mem::take(&mut *o)).unwrap_or_default()
}

/// `-application:openURLs:` for winit's application delegate: the files go to the app (`take_opened`).
extern "C-unwind" fn open_urls(_this: &AnyObject, _cmd: Sel, _app: &NSApplication, urls: &NSArray<NSURL>) {
    let paths = urls.iter().filter_map(|u| u.path()).map(|p| PathBuf::from(p.to_string()));
    if let Ok(mut o) = OPENED.lock() {
        o.extend(paths);
    }
    crate::wake_ui();
}

/// Routes quitting (`take_quit`) and files opened from the Finder (`take_opened`) through the app. Call once the event
/// loop is built (winit sets its delegate then) and before it runs: the files the app is launched to open arrive
/// before the launch ends.
pub fn hook_app() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let app = NSApplication::sharedApplication(mtm);
    let Some(delegate) = app.delegate() else { return };
    let delegate: &AnyObject = (*delegate).as_ref();
    let class = std::ptr::from_ref(delegate.class()).cast_mut();
    // SAFETY: each adds a method winit's delegate class does not implement, with the matching signature and type
    // encoding (NSUInteger (self, _cmd, sender); void (self, _cmd, application, urls)).
    unsafe {
        let imp = std::mem::transmute::<extern "C-unwind" fn(&AnyObject, Sel, &NSApplication) -> NSApplicationTerminateReply, unsafe extern "C-unwind" fn()>(
            should_terminate,
        );
        objc2::ffi::class_addMethod(class, sel!(applicationShouldTerminate:), imp, c"Q@:@".as_ptr());
        let imp = std::mem::transmute::<extern "C-unwind" fn(&AnyObject, Sel, &NSApplication, &NSArray<NSURL>), unsafe extern "C-unwind" fn()>(open_urls);
        objc2::ffi::class_addMethod(class, sel!(application:openURLs:), imp, c"v@:@@".as_ptr());
    }
    // AppKit notes what a delegate answers to when it is set: set again, it sees the new methods.
    let delegate = app.delegate();
    app.setDelegate(delegate.as_deref());
}

fn ns_window(window: &Window) -> Option<Retained<NSWindow>> {
    let RawWindowHandle::AppKit(h) = window.window_handle().ok()?.as_raw() else { return None };
    // SAFETY: winit's live content view.
    let view: &NSView = unsafe { h.ns_view.cast().as_ref() };
    view.window()
}

/// Height of the primary screen (points): AppKit counts y upward from its bottom.
fn primary_height(mtm: MainThreadMarker) -> f64 {
    NSScreen::screens(mtm).firstObject().map_or(0.0, |s| s.frame().size.height)
}

/// Cursor position and outer window rect (left, top, right, bottom) in screen px.
pub fn screen_state(window: &Window) -> Option<((i32, i32), (i32, i32, i32, i32))> {
    let mtm = MainThreadMarker::new()?;
    let win = ns_window(window)?;
    let (k, top) = (window.scale_factor(), primary_height(mtm));
    let p = NSEvent::mouseLocation();
    let f = win.frame();
    let px = |v: f64| (v * k).round() as i32;
    let (l, t) = (px(f.origin.x), px(top - f.origin.y - f.size.height));
    Some(((px(p.x), px(top - p.y)), (l, t, l + px(f.size.width), t + px(f.size.height))))
}

/// Moves / resizes the window to `r` (left, top, right, bottom; screen px) in one call.
pub fn set_rect(window: &Window, r: (i32, i32, i32, i32)) {
    let (Some(mtm), Some(win)) = (MainThreadMarker::new(), ns_window(window)) else { return };
    let (k, top) = (window.scale_factor(), primary_height(mtm));
    let pt = |v: i32| f64::from(v) / k;
    let size = NSSize::new(pt(r.2 - r.0), pt(r.3 - r.1));
    win.setFrame_display(NSRect::new(NSPoint::new(pt(r.0), top - pt(r.1) - size.height), size), true);
}

/// The work area of the window's screen (points, AppKit's coordinates): the screen without the menu bar and the Dock.
fn work_area(win: &NSWindow) -> Option<NSRect> {
    win.screen().map(|s| s.visibleFrame())
}

/// Whether the window fills its screen's work area.
pub fn is_maximized(window: &Window) -> bool {
    let Some(win) = ns_window(window) else { return false };
    let (f, Some(w)) = (win.frame(), work_area(&win)) else { return false };
    let near = |a: f64, b: f64| (a - b).abs() < 1.0;
    near(f.origin.x, w.origin.x) && near(f.origin.y, w.origin.y) && near(f.size.width, w.size.width) && near(f.size.height, w.size.height)
}

/// Fills the screen's work area with the window, or gives it its restored frame back (for a window that started
/// maximized: winit's size, centred in the work area).
pub fn set_maximized(window: &Window, on: bool) {
    let Some(win) = ns_window(window) else { return };
    let Some(work) = work_area(&win) else { return };
    if on == is_maximized(window) {
        return;
    }
    let frame = if on {
        RESTORE.set(Some(win.frame()));
        work
    } else {
        RESTORE.take().unwrap_or_else(|| {
            let k = window.scale_factor();
            let size = window.inner_size();
            let (w, h) = ((f64::from(size.width) / k).min(work.size.width), (f64::from(size.height) / k).min(work.size.height));
            let (w, h) = if (w, h) == (work.size.width, work.size.height) { (w * 0.9, h * 0.85) } else { (w, h) };
            NSRect::new(NSPoint::new(work.origin.x + (work.size.width - w) / 2.0, work.origin.y + (work.size.height - h) / 2.0), NSSize::new(w, h))
        })
    };
    win.setFrame_display(frame, true);
}
