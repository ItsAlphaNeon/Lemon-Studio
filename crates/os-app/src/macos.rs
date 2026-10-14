//! The main window on macOS, where winit leaves gaps: its frame in screen coordinates (title-bar moves and edge resizes
//! are done by hand, as on Windows: winit cannot start a system resize of a borderless window here) and quitting from
//! the menu bar or the Dock, which AppKit would do without asking to save.
//!
//! Screen coordinates are winit's: device px from the top-left of the primary screen.

use std::sync::atomic::{AtomicBool, Ordering};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{sel, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSApplicationTerminateReply, NSEvent, NSScreen, NSView, NSWindow};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

/// Quit was chosen (menu bar, Dock, Cmd+Q) and not handled yet.
static QUIT: AtomicBool = AtomicBool::new(false);

/// Whether the user asked to quit since the last call: the app asks about unsaved changes, then ends the event loop.
pub fn take_quit() -> bool {
    QUIT.swap(false, Ordering::AcqRel)
}

/// `-applicationShouldTerminate:` for winit's application delegate: the quit is cancelled and handed to the app, which
/// asks about unsaved changes before it ends (winit would let AppKit end the process at once).
extern "C-unwind" fn should_terminate(_this: &AnyObject, _cmd: Sel, _app: &NSApplication) -> NSApplicationTerminateReply {
    QUIT.store(true, Ordering::Release);
    crate::wake_ui();
    NSApplicationTerminateReply::TerminateCancel
}

/// Routes quitting through the app (`take_quit`). Call once the event loop runs (winit's delegate is set then).
pub fn hook_quit() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let app = NSApplication::sharedApplication(mtm);
    // SAFETY: the delegate winit installed is alive for the event loop's lifetime.
    let Some(delegate) = (unsafe { app.delegate() }) else { return };
    let delegate: &AnyObject = (*delegate).as_ref();
    let class = delegate.class();
    let imp: unsafe extern "C-unwind" fn() = unsafe {
        std::mem::transmute::<extern "C-unwind" fn(&AnyObject, Sel, &NSApplication) -> NSApplicationTerminateReply, unsafe extern "C-unwind" fn()>(
            should_terminate,
        )
    };
    // NSUInteger (self, _cmd, sender): the method's type encoding.
    let types = c"Q@:@";
    // SAFETY: adds a method the class does not have (winit's delegate does not implement it), with a matching
    // signature and encoding.
    unsafe { objc2::ffi::class_addMethod(std::ptr::from_ref(class).cast_mut(), sel!(applicationShouldTerminate:), imp, types.as_ptr()) };
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
