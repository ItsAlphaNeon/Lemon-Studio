// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! AppKit parts of a Lemon plugin editor (macOS), with the API and model of `x11` so the plugins run their Linux editor
//! loop unchanged: the editor's view (a flipped `NSView`, a subview of the host's) is made on the main thread in
//! `spawn`, as AppKit wants, and the editor runs on a GUI thread of its own; the view's event methods queue what
//! happens for that thread (`Window::next`), and the frames it paints go back to the main thread as the view's layer
//! contents. Popup menus painted with `os_ui::kit::menu` (never native menus) and the "Type in value" box are
//! borderless panels the GUI thread asks the main thread for and then runs like the X11 ones; event monitors stand in
//! for X11's grabs (a press elsewhere cancels, keys go to the box). The cursor is held during a knob drag by hiding it
//! and detaching it from the mouse. A view can take dropped files, and WAV audio goes to and from the clipboard.
//! Positions are device px (points times the window's backing scale), screen positions device px from the top left of
//! the main screen. The GUI thread never waits on the main thread past the editor's closing (the host closes editors on
//! the main thread, waiting for the GUI thread).

use std::cell::Cell;
use std::collections::VecDeque;
use std::ffi::{c_void, CString};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
use objc2::{msg_send, sel, AllocAnyThread, ClassType, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBackingStoreType, NSCursor, NSDragOperation, NSEvent, NSEventMask, NSEventModifierFlags, NSEventType, NSPanel, NSPasteboard,
    NSPasteboardTypeFileURL, NSPopUpMenuWindowLevel, NSScreen, NSTrackingArea, NSTrackingAreaOptions, NSView, NSWindowStyleMask,
};
use objc2_core_foundation::{kCFRunLoopCommonModes, CFRetained, CFRunLoop};
use objc2_core_graphics::{
    CGAssociateMouseAndMouseCursorPosition, CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGDisplayBounds, CGImage,
    CGImageAlphaInfo, CGImageByteOrderInfo, CGMainDisplayID,
};
use objc2_foundation::{NSArray, NSData, NSPoint, NSRect, NSSize, NSString, NSURL};
use os_theme::{Color, Palette};
use os_ui::kit::menu as kmenu;
use os_ui::{Canvas, Font, Rect};

pub use crate::input::{keysym, keysym_char, Button, Input};
use crate::MenuDef;

/// The host's view: its `NSView *`, as an address.
pub type Parent = usize;

/// The editor's polling period (overlay expiry, meters): the Win32 editors' timer.
const TICK: Duration = Duration::from_millis(30);
/// Points of precise (trackpad) scrolling that make one wheel notch.
const POINTS_PER_NOTCH: f64 = 24.0;
/// Audio on the clipboard: WAV (the type macOS gives `.wav` files; Windows' `CF_WAVE`).
const WAV_TYPE: &str = "com.microsoft.waveform-audio";

// -------------------------------------------------------------------------------------------------------------------
// The GUI thread

/// What the main thread saw, for the GUI thread: on which surface (the editor's view, a popup, a box), with the
/// modifiers held then.
struct Raw {
    surface: u64,
    ev: Ev,
    /// Ctrl (Command or Control), Shift, Alt (Option).
    mods: (bool, bool, bool),
}

/// A pointer position: on the surface (device px) and on the screen.
#[derive(Clone, Copy)]
struct At {
    x: f32,
    y: f32,
    sx: i32,
    sy: i32,
}

enum Ev {
    Down { at: At, btn: Button, dbl: bool },
    Up { at: At, btn: Button },
    /// The pointer moved; the mouse moved this far (device px; what drives a held drag).
    Move { at: At, dx: f32, dy: f32 },
    Wheel { at: At, notches: f32 },
    Leave,
    Key { sym: u32 },
    Expose,
    Drop(String),
    /// A press outside every surface while a popup or box is open.
    Outside,
}

struct Shared {
    state: Mutex<State>,
    cv: Condvar,
    quit: AtomicBool,
    /// The cursor is hidden and held (main thread).
    held: AtomicBool,
}

#[derive(Default)]
struct State {
    events: VecDeque<Raw>,
    woke: bool,
}

impl Shared {
    fn push(&self, r: Raw) {
        if let Ok(mut s) = self.state.lock() {
            s.events.push_back(r);
        }
        self.cv.notify_all();
    }

    fn quitting(&self) -> bool {
        self.quit.load(Ordering::Acquire)
    }
}

/// Wakes an editor's GUI thread from any thread: a parameter changed (look again) or the editor closes.
#[derive(Clone)]
pub struct Waker(Arc<Shared>);

impl Waker {
    /// Asks the GUI thread to look again (a parameter changed). Any thread.
    pub fn wake(&self) {
        if let Ok(mut s) = self.0.state.lock() {
            s.woke = true;
        }
        self.0.cv.notify_all();
    }

    fn quit(&self) {
        self.0.quit.store(true, Ordering::Release);
        self.wake();
    }
}

/// Where an editor keeps its open window's waker, for parameter notifications (`Editor::param_value_changed` runs on
/// any thread).
pub type Slot = Arc<Mutex<Option<Waker>>>;

/// A parameter changed: the open editor (if any) looks again.
pub fn notify(slot: &Slot) {
    if let Ok(s) = slot.lock() {
        if let Some(w) = s.as_ref() {
            w.wake();
        }
    }
}

/// The editor's GUI thread. Dropping it (on the main thread) closes the view and ends the thread.
pub struct EditorThread {
    waker: Waker,
    slot: Slot,
    thread: Option<JoinHandle<()>>,
}

impl Drop for EditorThread {
    fn drop(&mut self) {
        if let Ok(mut s) = self.slot.lock() {
            *s = None;
        }
        self.waker.quit();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let shared = self.waker.0.clone();
        match MainThreadMarker::new() {
            Some(_) => close_all(&shared),
            None => on_main(move |_| close_all(&shared)),
        }
    }
}

/// The backing scale of the host's view (its window's, else the main screen's): the editor's scale, as macOS hosts
/// never set one (sizes are points there). Main thread.
pub fn backing_scale(parent: Parent) -> f32 {
    let Some(mtm) = MainThreadMarker::new() else { return 1.0 };
    // SAFETY: the host passes a valid NSView (or null).
    let view = unsafe { (parent as *const NSView).as_ref() };
    let s = view.and_then(|v| v.window()).map(|w| w.backingScaleFactor()).or_else(|| NSScreen::mainScreen(mtm).map(|s| s.backingScaleFactor()));
    s.filter(|s| *s > 0.0).map_or(1.0, |s| s as f32)
}

/// Opens an editor view of `size` device px (at [`backing_scale`]) in the host's view `parent` and runs `run` with it
/// on a thread of its own. Main thread; `None` elsewhere.
pub fn spawn(parent: Parent, size: (u32, u32), slot: &Slot, run: impl FnOnce(Window) + Send + 'static) -> Option<EditorThread> {
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("plugin editor: not opened on the main thread");
        return None;
    };
    // SAFETY: the host passes a valid NSView (or null).
    let parent_view = unsafe { (parent as *const NSView).as_ref() }?;
    let scale = backing_scale(parent);
    let shared = Arc::new(Shared { state: Mutex::default(), cv: Condvar::new(), quit: AtomicBool::new(false), held: AtomicBool::new(false) });
    let s = f64::from(scale);
    let view = new_view(mtm, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(f64::from(size.0) / s, f64::from(size.1) / s)));
    // Stays at the parent's top left (where child windows sit on Windows and X11) when the parent changes size.
    view.setAutoresizingMask(if parent_view.isFlipped() { NSAutoresizingMaskOptions::ViewMaxYMargin } else { NSAutoresizingMaskOptions::ViewMinYMargin });
    parent_view.addSubview(&view);
    place(&view);
    let id = register(view, None, &shared, scale);
    let waker = Waker(shared.clone());
    let win = Window {
        shared: shared.clone(),
        id,
        scale,
        mods: (false, false, false),
        locked: false,
        pointer: (0, 0),
        next_tick: Instant::now() + TICK,
        raw: VecDeque::new(),
        queue: VecDeque::new(),
    };
    let Ok(thread) = std::thread::Builder::new().name("plugin editor".into()).spawn(move || run(win)) else {
        close_all(&shared);
        return None;
    };
    if let Ok(mut s) = slot.lock() {
        *s = Some(waker.clone());
    }
    Some(EditorThread { waker, slot: slot.clone(), thread: Some(thread) })
}

// -------------------------------------------------------------------------------------------------------------------
// The main thread: surfaces (the editor's view, popups, boxes) and their view class

/// AppKit objects kept where the GUI thread can see them: only touched, and dropped, on the main thread.
struct Main<T>(T);

// SAFETY: see above; the GUI thread only reads the other fields of what holds one.
unsafe impl<T> Send for Main<T> {}

/// A view of ours and, for a popup or box, its panel.
struct Surface {
    id: u64,
    shared: Arc<Shared>,
    scale: f32,
    view: Main<Retained<NSView>>,
    panel: Option<Main<Retained<NSPanel>>>,
    /// The frame shown, and the newest one painted, not shown yet.
    image: Option<CFRetained<CGImage>>,
    pending: Option<CFRetained<CGImage>>,
}

static SURFACES: Mutex<Vec<Surface>> = Mutex::new(Vec::new());
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// The event monitors of an open popup or box, by the editor surface that opened it.
static MONITORS: Mutex<Vec<(u64, Main<Retained<AnyObject>>)>> = Mutex::new(Vec::new());

fn surfaces() -> std::sync::MutexGuard<'static, Vec<Surface>> {
    SURFACES.lock().unwrap_or_else(|e| e.into_inner())
}

fn register(view: Retained<NSView>, panel: Option<Retained<NSPanel>>, shared: &Arc<Shared>, scale: f32) -> u64 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    surfaces().push(Surface { id, shared: shared.clone(), scale, view: Main(view), panel: panel.map(Main), image: None, pending: None });
    id
}

/// The surface a view of ours shows. Main thread.
fn target(v: &NSView) -> Option<(u64, Arc<Shared>, f32)> {
    let p: *const NSView = v;
    surfaces().iter().find(|s| Retained::as_ptr(&s.view.0) == p).map(|s| (s.id, s.shared.clone(), s.scale))
}

/// A surface's view. Main thread.
fn view_of(id: u64) -> Option<Retained<NSView>> {
    surfaces().iter().find(|s| s.id == id).map(|s| s.view.0.clone())
}

/// Closes a surface: its panel, its view. Main thread.
fn close(id: u64) {
    let s = {
        let mut l = surfaces();
        let Some(i) = l.iter().position(|s| s.id == id) else { return };
        l.remove(i)
    };
    if let Some(p) = &s.panel {
        p.0.orderOut(None);
        p.0.close();
    }
    s.view.0.removeFromSuperview();
}

/// The editor is gone: every surface it opened, its monitors and its held cursor. Main thread.
fn close_all(shared: &Arc<Shared>) {
    let ids: Vec<u64> = surfaces().iter().filter(|s| Arc::ptr_eq(&s.shared, shared)).map(|s| s.id).collect();
    for id in ids {
        remove_monitors(id);
        close(id);
    }
    hold_cursor(shared, false);
}

/// Runs `f` on the main thread, soon (the main run loop, in its common modes: also while a menu is tracked or a
/// dialog runs).
fn on_main(f: impl FnOnce(MainThreadMarker) + Send + 'static) {
    let Some(rl) = CFRunLoop::main() else { return };
    let f = Cell::new(Some(f));
    let block = RcBlock::new(move || {
        if let Some(f) = f.take() {
            // SAFETY: the main run loop runs its blocks on the main thread.
            f(unsafe { MainThreadMarker::new_unchecked() });
        }
    });
    // SAFETY: a valid run loop, mode and block.
    unsafe { rl.perform_block(kCFRunLoopCommonModes.map(|m| &**m), Some(&block)) };
    rl.wake_up();
}

/// Height of the main screen (points): screen positions are measured down from its top.
fn main_height() -> f64 {
    CGDisplayBounds(CGMainDisplayID()).size.height
}

/// Hides the cursor and detaches it from the mouse (`on`), or shows it again. Main thread.
fn hold_cursor(shared: &Shared, on: bool) {
    if shared.held.swap(on, Ordering::AcqRel) == on {
        return;
    }
    if on {
        NSCursor::hide();
        let _ = CGAssociateMouseAndMouseCursorPosition(false);
    } else {
        let _ = CGAssociateMouseAndMouseCursorPosition(true);
        NSCursor::unhide();
    }
}

/// The view class of editors, popups and boxes: registered once per loaded module, under a name of its own (this
/// module's address), so two plugin modules (or two builds of one) never share a class with another's code.
fn view_class() -> &'static AnyClass {
    static CLASS: OnceLock<&'static AnyClass> = OnceLock::new();
    CLASS.get_or_init(|| {
        let base = format!("LemonPluginView{:x}", view_class as fn() -> &'static AnyClass as usize);
        let mut b = (0..)
            .find_map(|n| {
                let name = CString::new(if n == 0 { base.clone() } else { format!("{base}_{n}") }).ok()?;
                ClassBuilder::new(&name, NSView::class())
            })
            .expect("a free class name");
        // SAFETY: each function has the selector's signature.
        unsafe {
            for s in [sel!(isFlipped), sel!(isOpaque), sel!(wantsUpdateLayer), sel!(acceptsFirstResponder)] {
                b.add_method(s, yes as extern "C-unwind" fn(_, _) -> _);
            }
            b.add_method(sel!(acceptsFirstMouse:), first_mouse as extern "C-unwind" fn(_, _, _) -> _);
            for s in [
                sel!(mouseDown:),
                sel!(mouseUp:),
                sel!(rightMouseDown:),
                sel!(rightMouseUp:),
                sel!(otherMouseDown:),
                sel!(otherMouseUp:),
                sel!(mouseMoved:),
                sel!(mouseDragged:),
                sel!(rightMouseDragged:),
                sel!(otherMouseDragged:),
                sel!(scrollWheel:),
                sel!(mouseEntered:),
                sel!(mouseExited:),
                sel!(keyDown:),
            ] {
                b.add_method(s, on_event as extern "C-unwind" fn(_, _, _));
            }
            b.add_method(sel!(draggingEntered:), drag_enter as extern "C-unwind" fn(_, _, _) -> _);
            b.add_method(sel!(performDragOperation:), drag_drop as extern "C-unwind" fn(_, _, _) -> _);
            b.add_method(sel!(updateLayer), update_layer as extern "C-unwind" fn(_, _));
            b.add_method(sel!(viewDidChangeBackingProperties), expose as extern "C-unwind" fn(_, _));
            b.add_method(sel!(viewDidMoveToWindow), expose as extern "C-unwind" fn(_, _));
        }
        b.register()
    })
}

extern "C-unwind" fn yes(_: &NSView, _: Sel) -> Bool {
    Bool::YES
}

/// A click into an inactive window acts at once, as on Windows and X11.
extern "C-unwind" fn first_mouse(_: &NSView, _: Sel, _: Option<&NSEvent>) -> Bool {
    Bool::YES
}

/// Shows the surface's newest frame.
extern "C-unwind" fn update_layer(v: &NSView, _: Sel) {
    let p: *const NSView = v;
    let image = surfaces().iter().find(|s| Retained::as_ptr(&s.view.0) == p).and_then(|s| s.image.clone());
    if let (Some(layer), Some(image)) = (v.layer(), image) {
        let contents: *const CGImage = &*image;
        // SAFETY: a CGImage is a valid layer contents object.
        unsafe { layer.setContents(Some(&*contents.cast::<AnyObject>())) };
    }
}

/// The view moved to another window or screen: paint again (at the scale the editor opened with; a screen of another
/// backing scale shows the frame scaled to fit).
extern "C-unwind" fn expose(v: &NSView, _: Sel) {
    if let Some((id, shared, _)) = target(v) {
        shared.push(Raw { surface: id, ev: Ev::Expose, mods: (false, false, false) });
    }
}

/// A file dragged over a view that takes files (`Window::accept_files`) is copied in.
extern "C-unwind" fn drag_enter(_: &NSView, _: Sel, sender: Option<&AnyObject>) -> NSDragOperation {
    if sender.and_then(dragged_file).is_some() { NSDragOperation::Copy } else { NSDragOperation::None }
}

extern "C-unwind" fn drag_drop(v: &NSView, _: Sel, sender: Option<&AnyObject>) -> Bool {
    let (Some(path), Some((id, shared, _))) = (sender.and_then(dragged_file), target(v)) else { return Bool::NO };
    shared.push(Raw { surface: id, ev: Ev::Drop(path), mods: (false, false, false) });
    Bool::YES
}

/// The (first) file of a drag.
fn dragged_file(sender: &AnyObject) -> Option<String> {
    // SAFETY: AppKit passes an `NSDraggingInfo`.
    let pb: Option<Retained<NSPasteboard>> = unsafe { msg_send![sender, draggingPasteboard] };
    // SAFETY: a static pasteboard type.
    let url = pb?.stringForType(unsafe { NSPasteboardTypeFileURL })?;
    NSURL::URLWithString(&url)?.to_file_path().map(|p| p.to_string_lossy().into_owned())
}

thread_local! {
    /// The left press going on is a Control-click: a right click (the macOS way), released as one.
    static CONTROL_CLICK: Cell<bool> = const { Cell::new(false) };
}

/// Ctrl (Command or Control: the macOS habit), Shift, Alt (Option).
fn mods_of(f: NSEventModifierFlags) -> (bool, bool, bool) {
    (f.contains(NSEventModifierFlags::Command) || f.contains(NSEventModifierFlags::Control), f.contains(NSEventModifierFlags::Shift), f.contains(NSEventModifierFlags::Option))
}

/// Where an event happened: on view `v` and on the screen (device px at `scale`).
fn at(v: &NSView, e: &NSEvent, scale: f32) -> At {
    let loc = e.locationInWindow();
    let p = v.convertPoint_fromView(loc, None);
    let s = v.window().map_or(loc, |w| w.convertPointToScreen(loc));
    let k = f64::from(scale);
    At { x: (p.x * k) as f32, y: (p.y * k) as f32, sx: (s.x * k).round() as i32, sy: ((main_height() - s.y) * k).round() as i32 }
}

/// The view's mouse and key methods: what happened, for the GUI thread.
extern "C-unwind" fn on_event(v: &NSView, _: Sel, e: Option<&NSEvent>) {
    let Some(e) = e else { return };
    let Some((id, shared, scale)) = target(v) else { return };
    let flags = e.modifierFlags();
    let mut mods = mods_of(flags);
    // Every second press of a run is a double click.
    let dbl = e.clickCount() >= 2 && e.clickCount() % 2 == 0;
    let ev = match e.r#type() {
        NSEventType::LeftMouseDown => {
            let right = flags.contains(NSEventModifierFlags::Control) && !flags.contains(NSEventModifierFlags::Command);
            CONTROL_CLICK.set(right);
            // That Control is the click's, not Ctrl held.
            if right {
                mods.0 = false;
            }
            Ev::Down { at: at(v, e, scale), btn: if right { Button::Right } else { Button::Left }, dbl }
        }
        NSEventType::LeftMouseUp => Ev::Up { at: at(v, e, scale), btn: if CONTROL_CLICK.get() { Button::Right } else { Button::Left } },
        NSEventType::RightMouseDown => Ev::Down { at: at(v, e, scale), btn: Button::Right, dbl },
        NSEventType::RightMouseUp => Ev::Up { at: at(v, e, scale), btn: Button::Right },
        NSEventType::OtherMouseDown if e.buttonNumber() == 2 => Ev::Down { at: at(v, e, scale), btn: Button::Middle, dbl },
        NSEventType::OtherMouseUp if e.buttonNumber() == 2 => Ev::Up { at: at(v, e, scale), btn: Button::Middle },
        NSEventType::MouseMoved | NSEventType::LeftMouseDragged | NSEventType::RightMouseDragged | NSEventType::OtherMouseDragged => {
            Ev::Move { at: at(v, e, scale), dx: (e.deltaX() * f64::from(scale)) as f32, dy: (e.deltaY() * f64::from(scale)) as f32 }
        }
        NSEventType::ScrollWheel => {
            let d = e.scrollingDeltaY();
            if d == 0.0 {
                return;
            }
            // A wheel scrolls by lines (a notch each), a trackpad by points.
            let notches = if e.hasPreciseScrollingDeltas() { d / POINTS_PER_NOTCH } else { d };
            Ev::Wheel { at: at(v, e, scale), notches: notches as f32 }
        }
        NSEventType::MouseExited => Ev::Leave,
        NSEventType::KeyDown => match key_sym(e) {
            Some(sym) => Ev::Key { sym },
            None => return,
        },
        _ => return,
    };
    shared.push(Raw { surface: id, ev, mods });
}

/// The X11 key symbol of a key press: the editing keys by key code, else the character typed (without Command or
/// Control's changes).
fn key_sym(e: &NSEvent) -> Option<u32> {
    // <HIToolbox/Events.h> virtual key codes.
    let sym = match e.keyCode() {
        51 => keysym::BACKSPACE,
        36 => keysym::RETURN,
        76 => keysym::KP_ENTER,
        53 => keysym::ESCAPE,
        123 => keysym::LEFT,
        124 => keysym::RIGHT,
        125 => keysym::DOWN,
        126 => keysym::UP,
        115 => keysym::HOME,
        119 => keysym::END,
        117 => keysym::DELETE,
        _ => {
            let f = e.modifierFlags();
            let plain = f.contains(NSEventModifierFlags::Command) || f.contains(NSEventModifierFlags::Control);
            let s = if plain { e.charactersIgnoringModifiers() } else { e.characters() }?;
            let c = s.to_string().chars().next()?;
            // Control characters, and function keys (private use characters).
            if c.is_control() || ('\u{f700}'..='\u{f8ff}').contains(&c) {
                return None;
            }
            match u32::from(c) {
                c @ (0x20..=0x7e | 0xa0..=0xff) => c,
                c => 0x0100_0000 + c,
            }
        }
    };
    Some(sym)
}

/// A view of our class, layer-backed (frames are its layer's contents), told about the mouse moving over it and
/// leaving it, whichever window is key.
fn new_view(_mtm: MainThreadMarker, frame: NSRect) -> Retained<NSView> {
    // SAFETY: our class is an NSView subclass; `initWithFrame:` is its initialiser.
    let view: Retained<NSView> = unsafe {
        let a: Allocated<NSView> = msg_send![view_class(), alloc];
        msg_send![a, initWithFrame: frame]
    };
    view.setWantsLayer(true);
    let opts = NSTrackingAreaOptions::MouseEnteredAndExited | NSTrackingAreaOptions::MouseMoved | NSTrackingAreaOptions::ActiveAlways | NSTrackingAreaOptions::InVisibleRect;
    let owner: &AnyObject = &view;
    // SAFETY: the owner is the view, which outlives its own tracking area.
    let area = unsafe { NSTrackingArea::initWithRect_options_owner_userInfo(NSTrackingArea::alloc(), NSRect::ZERO, opts, Some(owner), None) };
    view.addTrackingArea(&area);
    view
}

/// Keeps a view at its parent's top left, the parent flipped or not.
fn place(v: &NSView) {
    // SAFETY: the superview is read on the main thread.
    let Some(p) = (unsafe { v.superview() }) else { return };
    let y = if p.isFlipped() { 0.0 } else { p.bounds().size.height - v.frame().size.height };
    v.setFrameOrigin(NSPoint::new(0.0, y));
}

/// A frame as an image: XRGB rows of `w`, in the display's colour space as Lemon Studio's own windows are (softbuffer's),
/// so an editor's colours match the frame around it.
fn image(px: &[u32], w: u32, h: u32) -> Option<CFRetained<CGImage>> {
    let n = w as usize * h as usize;
    if n == 0 || px.len() < n {
        return None;
    }
    unsafe extern "C-unwind" fn release(info: *mut c_void, _: NonNull<c_void>, _: usize) {
        // SAFETY: `info` is the box made below, released once.
        drop(unsafe { Box::from_raw(info.cast::<Vec<u32>>()) });
    }
    let data = Box::new(px[..n].to_vec());
    let bytes = data.as_ptr();
    let info = Box::into_raw(data);
    // SAFETY: `bytes` stays valid until the provider releases `info`.
    let Some(provider) = (unsafe { CGDataProvider::with_data(info.cast(), bytes.cast(), n * 4, Some(release)) }) else {
        // SAFETY: the provider was not made, so nothing else owns `info`.
        drop(unsafe { Box::from_raw(info) });
        return None;
    };
    let space = CGColorSpace::new_device_rgb();
    // XRGB words in memory order: 32-bit little endian, the top byte skipped.
    let bits = CGBitmapInfo(CGImageAlphaInfo::NoneSkipFirst.0 | CGImageByteOrderInfo::Order32Little.0);
    // SAFETY: the provider holds `w * h` 32-bit pixels; no decode array.
    unsafe {
        CGImage::new(w as usize, h as usize, 8, 32, w as usize * 4, space.as_deref(), bits, Some(&provider), std::ptr::null(), false, CGColorRenderingIntent::RenderingIntentDefault)
    }
}

/// Shows a frame on a surface: the newest frame waits for the main thread, which is asked once per batch.
fn show_frame(id: u64, px: &[u32], w: u32, h: u32) {
    let Some(img) = image(px, w, h) else { return };
    let ask = {
        let mut l = surfaces();
        let Some(s) = l.iter_mut().find(|s| s.id == id) else { return };
        s.pending.replace(img).is_none()
    };
    if ask {
        on_main(move |_| {
            let view = {
                let mut l = surfaces();
                let Some(s) = l.iter_mut().find(|s| s.id == id) else { return };
                if let Some(p) = s.pending.take() {
                    s.image = Some(p);
                }
                s.view.0.clone()
            };
            view.setNeedsDisplay(true);
        });
    }
}

/// A borderless panel at screen rectangle `r` (x, y, w, h device px) above other windows, showing a new surface; it
/// never becomes key (the host keeps the keyboard; the monitors pass keys on). Main thread.
fn make_panel(mtm: MainThreadMarker, r: [i32; 4], shared: &Arc<Shared>, scale: f32) -> Option<u64> {
    // The editor closed while this was on its way.
    if shared.quitting() {
        return None;
    }
    let s = f64::from(scale);
    let (x, y, w, h) = (f64::from(r[0]) / s, f64::from(r[1]) / s, f64::from(r[2].max(1)) / s, f64::from(r[3].max(1)) / s);
    let rect = NSRect::new(NSPoint::new(x, main_height() - y - h), NSSize::new(w, h));
    let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
    let panel = NSPanel::initWithContentRect_styleMask_backing_defer(NSPanel::alloc(mtm), rect, style, NSBackingStoreType::Buffered, false);
    // SAFETY: the panel is kept (and closed) by its surface.
    unsafe { panel.setReleasedWhenClosed(false) };
    panel.setLevel(NSPopUpMenuWindowLevel);
    panel.setHasShadow(true);
    panel.setHidesOnDeactivate(false);
    panel.setAcceptsMouseMovedEvents(true);
    let view = new_view(mtm, NSRect::new(NSPoint::new(0.0, 0.0), rect.size));
    panel.setContentView(Some(&view));
    let id = register(view, Some(panel.clone()), shared, scale);
    panel.orderFrontRegardless();
    Some(id)
}

/// Whether a window is one of our panels. Main thread.
fn is_panel(w: *const AnyObject) -> bool {
    surfaces().iter().any(|s| s.panel.as_ref().is_some_and(|p| Retained::as_ptr(&p.0).cast::<AnyObject>() == w))
}

/// While a popup or box of editor surface `id` is open: a press anywhere else (this app's other windows, other apps)
/// cancels it and is not passed on, and keys go to it. Main thread.
fn add_monitors(id: u64, shared: &Arc<Shared>) {
    if shared.quitting() {
        return;
    }
    let presses = NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown | NSEventMask::OtherMouseDown;
    let sh = shared.clone();
    let local = RcBlock::new(move |e: NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit passes a valid event.
        let ev = unsafe { e.as_ref() };
        match ev.r#type() {
            NSEventType::KeyDown => {
                if let Some(sym) = key_sym(ev) {
                    sh.push(Raw { surface: id, ev: Ev::Key { sym }, mods: mods_of(ev.modifierFlags()) });
                }
                std::ptr::null_mut()
            }
            NSEventType::LeftMouseDown | NSEventType::RightMouseDown | NSEventType::OtherMouseDown => {
                // SAFETY: monitors run on the main thread.
                let w = ev.window(unsafe { MainThreadMarker::new_unchecked() });
                if w.as_ref().is_some_and(|w| is_panel(Retained::as_ptr(w).cast())) {
                    return e.as_ptr();
                }
                sh.push(Raw { surface: id, ev: Ev::Outside, mods: (false, false, false) });
                std::ptr::null_mut()
            }
            _ => e.as_ptr(),
        }
    });
    let sh = shared.clone();
    let global = RcBlock::new(move |_: NonNull<NSEvent>| sh.push(Raw { surface: id, ev: Ev::Outside, mods: (false, false, false) }));
    // SAFETY: the handler returns the event or null.
    let a = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(presses | NSEventMask::KeyDown, &local) };
    let b = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(presses, &global);
    if let Ok(mut m) = MONITORS.lock() {
        m.extend(a.into_iter().chain(b).map(|o| (id, Main(o))));
    }
}

/// Main thread.
fn remove_monitors(id: u64) {
    let gone: Vec<_> = match MONITORS.lock() {
        Ok(mut m) => {
            let (gone, keep) = std::mem::take(&mut *m).into_iter().partition(|(i, _)| *i == id);
            *m = keep;
            gone
        }
        Err(_) => return,
    };
    for (_, o) in gone {
        // SAFETY: a monitor AppKit returned.
        unsafe { NSEvent::removeMonitor(&o.0) };
    }
}

// -------------------------------------------------------------------------------------------------------------------
// The window

/// An editor's view, as its GUI thread sees it.
pub struct Window {
    shared: Arc<Shared>,
    /// The editor view's surface.
    id: u64,
    scale: f32,
    /// Ctrl, Shift, Alt at the last event.
    mods: (bool, bool, bool),
    locked: bool,
    /// The pointer on the screen at the last event.
    pointer: (i32, i32),
    next_tick: Instant,
    /// Events taken from the main thread, not looked at yet.
    raw: VecDeque<Raw>,
    /// Inputs made, not handed out yet.
    queue: VecDeque<Input>,
}

impl Window {
    /// No input is waiting: a good moment to paint (once per burst of events).
    pub fn idle(&self) -> bool {
        self.queue.is_empty()
    }

    /// Ctrl, Shift, Alt held at the last event (Command counts as Ctrl).
    pub fn mods(&self) -> (bool, bool, bool) {
        self.mods
    }

    /// The next input, waiting for it; `None` when the editor closes.
    pub fn next(&mut self) -> Option<Input> {
        loop {
            if self.shared.quitting() {
                return None;
            }
            if let Some(i) = self.queue.pop_front() {
                return Some(i);
            }
            while let Some(r) = self.raw.pop_front() {
                if let Some(i) = self.convert(r) {
                    self.queue.push_back(i);
                }
            }
            if !self.queue.is_empty() {
                continue;
            }
            let now = Instant::now();
            if now >= self.next_tick {
                self.next_tick = now + TICK;
                return Some(Input::Tick);
            }
            if self.wait(self.next_tick) {
                return if self.shared.quitting() { None } else { Some(Input::Tick) };
            }
        }
    }

    /// Waits until `until` for events (into `raw`) or a wake; whether it was a wake.
    fn wait(&mut self, until: Instant) -> bool {
        let Ok(mut s) = self.shared.state.lock() else { return false };
        while s.events.is_empty() && !s.woke && !self.shared.quitting() {
            let now = Instant::now();
            if now >= until {
                break;
            }
            s = match self.shared.cv.wait_timeout(s, until - now) {
                Ok((s, _)) => s,
                Err(_) => return false,
            };
        }
        self.raw.extend(s.events.drain(..));
        std::mem::take(&mut s.woke)
    }

    /// The next event of any surface, for modal loops; `None` when the editor closes.
    fn event(&mut self) -> Option<Raw> {
        loop {
            if self.shared.quitting() {
                return None;
            }
            if let Some(r) = self.raw.pop_front() {
                if r.surface == self.id {
                    self.mods = r.mods;
                }
                return Some(r);
            }
            self.wait(Instant::now() + Duration::from_millis(100));
        }
    }

    fn convert(&mut self, r: Raw) -> Option<Input> {
        // Late events of a closed popup or box.
        if r.surface != self.id {
            return None;
        }
        self.mods = r.mods;
        match r.ev {
            Ev::Down { at, btn, dbl } => {
                self.pointer = (at.sx, at.sy);
                // The keyboard is left where it is (the host's typing keyboard keeps playing): a type-in box takes
                // the keys while it is open.
                Some(Input::Down { x: at.x, y: at.y, btn, dbl })
            }
            Ev::Up { at, btn } => {
                self.pointer = (at.sx, at.sy);
                Some(Input::Up { btn })
            }
            // A held drag: the mouse's own movement (the cursor stays where the drag started).
            Ev::Move { dx, dy, .. } if self.locked => ((dx, dy) != (0.0, 0.0)).then_some(Input::Delta { dx, dy }),
            Ev::Move { at, .. } => {
                self.pointer = (at.sx, at.sy);
                Some(Input::Move { x: at.x, y: at.y })
            }
            Ev::Wheel { at, notches } => Some(Input::Wheel { x: at.x, y: at.y, notches }),
            Ev::Leave => Some(Input::Leave),
            Ev::Key { sym } => Some(Input::Key { sym, text: keysym_char(sym) }),
            Ev::Expose => Some(Input::Expose),
            Ev::Drop(path) => Some(Input::Drop { path }),
            Ev::Outside => None,
        }
    }

    /// Runs `f` on the main thread and waits for it; `None` when the editor closes meanwhile.
    fn call<R: Send + 'static>(&self, f: impl FnOnce(MainThreadMarker) -> R + Send + 'static) -> Option<R> {
        let done = Arc::new((Mutex::new(None), Condvar::new()));
        let d = done.clone();
        let shared = self.shared.clone();
        on_main(move |mtm| {
            // Closed before the main thread got to it: nothing is shown for an editor that is gone (a file dialog).
            if shared.quitting() {
                return;
            }
            let r = f(mtm);
            if let Ok(mut s) = d.0.lock() {
                *s = Some(r);
            }
            d.1.notify_all();
        });
        let mut s = done.0.lock().ok()?;
        loop {
            if let Some(r) = s.take() {
                return Some(r);
            }
            // The host may be closing the editor on the main thread, waiting for this one.
            if self.shared.quitting() {
                return None;
            }
            s = done.1.wait_timeout(s, Duration::from_millis(50)).ok()?.0;
        }
    }

    /// Copies `px` (XRGB rows of `w`) into the window at (0, 0).
    pub fn blit(&self, px: &[u32], w: u32, h: u32) {
        show_frame(self.id, px, w, h);
    }

    /// Resizes the window (device px).
    pub fn resize(&self, w: u32, h: u32) {
        let (id, s) = (self.id, f64::from(self.scale));
        on_main(move |_| {
            if let Some(v) = view_of(id) {
                v.setFrameSize(NSSize::new(f64::from(w.max(1)) / s, f64::from(h.max(1)) / s));
                place(&v);
            }
        });
    }

    /// A window point on the screen (device px).
    pub fn to_root(&self, x: i32, y: i32) -> (i32, i32) {
        let (id, s) = (self.id, f64::from(self.scale));
        let r = self.call(move |_| {
            let v = view_of(id)?;
            let p = v.window()?.convertPointToScreen(v.convertPoint_toView(NSPoint::new(f64::from(x) / s, f64::from(y) / s), None));
            Some(((p.x * s).round() as i32, ((main_height() - p.y) * s).round() as i32))
        });
        r.flatten().unwrap_or((x, y))
    }

    /// The usable part (left, top, right, bottom; device px) of the screen showing screen point (`x`, `y`).
    fn screen_at(&self, x: i32, y: i32) -> [i32; 4] {
        let s = f64::from(self.scale);
        let r = self.call(move |mtm| {
            let h = main_height();
            let p = NSPoint::new(f64::from(x) / s, h - f64::from(y) / s);
            let has = |f: NSRect| p.x >= f.origin.x && p.x < f.origin.x + f.size.width && p.y >= f.origin.y && p.y < f.origin.y + f.size.height;
            let screen = NSScreen::screens(mtm).iter().find(|sc| has(sc.frame())).or_else(|| NSScreen::mainScreen(mtm))?;
            let f = screen.visibleFrame();
            let px = |v: f64| (v * s).round() as i32;
            Some([px(f.origin.x), px(h - f.origin.y - f.size.height), px(f.origin.x + f.size.width), px(h - f.origin.y)])
        });
        r.flatten().unwrap_or([0, 0, i32::MAX / 2, i32::MAX / 2])
    }

    /// Runs a desktop dialog (a file dialog) on the main thread, as AppKit wants, and waits for it; `None` when the
    /// editor closes meanwhile.
    pub fn dialog<R: Send + 'static>(&self, f: impl FnOnce() -> R + Send + 'static) -> Option<R> {
        self.call(move |_| f())
    }

    /// Takes files dropped on the window: they arrive as `Input::Drop`.
    pub fn accept_files(&self) {
        let id = self.id;
        on_main(move |_| {
            if let Some(v) = view_of(id) {
                // SAFETY: a static pasteboard type.
                v.registerForDraggedTypes(&NSArray::from_slice(&[unsafe { NSPasteboardTypeFileURL }]));
            }
        });
    }

    /// Puts WAV audio on the clipboard; whether it went.
    pub fn copy_audio(&self, wav: Vec<u8>) -> bool {
        let r = self.call(move |_| {
            let pb = NSPasteboard::generalPasteboard();
            pb.clearContents();
            pb.setData_forType(Some(&NSData::with_bytes(&wav)), &NSString::from_str(WAV_TYPE))
        });
        r.unwrap_or(false)
    }

    /// WAV audio on the clipboard, if any.
    pub fn paste_audio(&self) -> Option<Vec<u8>> {
        self.call(|_| NSPasteboard::generalPasteboard().dataForType(&NSString::from_str(WAV_TYPE)).map(|d| d.to_vec())).flatten()
    }

    pub fn locked(&self) -> bool {
        self.locked
    }

    /// Hides the cursor and holds it where it is: movement arrives as `Input::Delta`.
    pub fn lock(&mut self) {
        if self.locked {
            return;
        }
        self.locked = true;
        let shared = self.shared.clone();
        on_main(move |_| hold_cursor(&shared, true));
    }

    /// Shows the cursor again where the drag started.
    pub fn unlock(&mut self) {
        if !self.locked {
            return;
        }
        self.locked = false;
        let shared = self.shared.clone();
        on_main(move |_| hold_cursor(&shared, false));
    }

    /// Presses elsewhere cancel, keys come here, until `ungrab` (X11's pointer and keyboard grab).
    fn grab(&self) {
        let (id, shared) = (self.id, self.shared.clone());
        on_main(move |_| add_monitors(id, &shared));
    }

    fn ungrab(&self) {
        let id = self.id;
        on_main(move |_| remove_monitors(id));
    }

    /// A panel (popup, box) at screen `r` (x, y, w, h), shown: its surface.
    fn overlay(&self, r: [i32; 4]) -> Option<u64> {
        let (shared, scale) = (self.shared.clone(), self.scale);
        self.call(move |mtm| make_panel(mtm, r, &shared, scale)).flatten()
    }

    fn close_overlay(&self, id: u64) {
        on_main(move |_| close(id));
    }

    fn show(&self, id: u64, c: &Canvas) {
        let (w, h) = (c.pixmap.width(), c.pixmap.height());
        let mut px = vec![0u32; (w * h) as usize];
        c.copy_to_xrgb(&mut px);
        show_frame(id, &px, w, h);
    }

    // ---------------------------------------------------------------------------------------------------------------
    // Popup menus: a borderless panel painted with `kit::menu`, run modally until a pick or a cancel.

    /// Shows a menu at screen (`x`, `y`); the picked entry's command.
    pub fn popup<C: Clone>(&mut self, x: i32, y: i32, def: &MenuDef<C>, pal: &Palette, scale: f32) -> Option<C> {
        if crate::no_modal() {
            return None;
        }
        self.unlock();
        self.grab();
        let r = match self.popup_at(x, y, def, pal, scale, None) {
            PopRes::Pick(c) => Some(c),
            _ => None,
        };
        self.ungrab();
        r
    }

    fn popup_at<C: Clone>(&mut self, x: i32, y: i32, def: &MenuDef<C>, pal: &Palette, scale: f32, parent: Option<[i32; 4]>) -> PopRes<C> {
        let Ok(mut measure) = Canvas::new(8, 8, scale) else { return PopRes::Cancel };
        let layout = kmenu::layout(&mut measure, &def.items, def.style);
        let (w, h) = ((layout.w * scale).ceil() as i32, (layout.h * scale).ceil() as i32);
        let b = self.screen_at(x, y);
        let (mut px, mut py) = (x, y);
        if px + w > b[2] {
            px = match parent {
                Some(p) => p[0] - w,
                None => b[2] - w,
            };
        }
        if py + h > b[3] {
            py = (b[3] - h).max(b[1]);
        }
        let Ok(mut canvas) = Canvas::new(w.max(1) as u32, h.max(1) as u32, scale) else { return PopRes::Cancel };
        let Some(pop) = self.overlay([px, py, w, h]) else { return PopRes::Cancel };
        let mut hover: Option<usize> = None;
        let paint = |canvas: &mut Canvas, hover: Option<usize>| {
            canvas.begin_frame(None);
            kmenu::paint(canvas, pal, &def.items, &layout, def.style, 0.0, 0.0, hover, def.current);
        };
        paint(&mut canvas, hover);
        self.show(pop, &canvas);
        let rect = [px, py, px + w, py + h];
        let inside = |r: &[i32; 4], (sx, sy): (i32, i32)| sx >= r[0] && sx < r[2] && sy >= r[1] && sy < r[3];
        let item_at = |(sx, sy): (i32, i32)| layout.item_at(&def.items, (sx - px) as f32 / scale, (sy - py) as f32 / scale);
        let opened = Instant::now();
        let start = self.pointer;
        let mut moved = false;
        let mut sub_open: Option<usize> = None;
        let result = loop {
            let Some(e) = self.event() else { break PopRes::Cancel };
            match e.ev {
                Ev::Expose if e.surface == pop => self.show(pop, &canvas),
                Ev::Outside => break PopRes::Cancel,
                Ev::Move { at, .. } => {
                    let cur = (at.sx, at.sy);
                    self.pointer = cur;
                    if (cur.0 - start.0).abs() + (cur.1 - start.1).abs() > 2 {
                        moved = true;
                    }
                    if !inside(&rect, cur) && parent.is_some_and(|p| inside(&p, cur)) {
                        break PopRes::Back;
                    }
                    let hov = if inside(&rect, cur) { item_at(cur) } else { None }.filter(|&i| def.items[i].clickable());
                    if hov != hover {
                        hover = hov;
                        paint(&mut canvas, hover);
                        self.show(pop, &canvas);
                    }
                    // Submenu: opens while its row is hovered.
                    match hov {
                        Some(i) if sub_open != Some(i) => {
                            if let Some(sub) = def.sub(i) {
                                sub_open = Some(i);
                                let row = layout.rows[i];
                                let sx = px + ((row.x + row.w) * scale) as i32 - 2;
                                let sy = py + (row.y * scale) as i32 - 2;
                                match self.popup_at(sx, sy, sub, pal, scale, Some(rect)) {
                                    PopRes::Pick(c) => break PopRes::Pick(c),
                                    PopRes::Cancel => break PopRes::Cancel,
                                    PopRes::Back => {}
                                }
                            }
                        }
                        Some(_) => {}
                        None => sub_open = None,
                    }
                }
                Ev::Down { at, .. } => {
                    if !inside(&rect, (at.sx, at.sy)) {
                        break PopRes::Cancel;
                    }
                    moved = true;
                }
                Ev::Up { at, btn: Button::Left | Button::Right } => {
                    // The release of the click that opened the menu does not pick.
                    if !moved && opened.elapsed() < Duration::from_millis(400) {
                        continue;
                    }
                    let cur = (at.sx, at.sy);
                    if let Some(i) = inside(&rect, cur).then(|| item_at(cur)).flatten() {
                        if def.items[i].clickable() && def.sub(i).is_none() {
                            break PopRes::Pick(def.cmds[i].clone());
                        }
                    }
                }
                Ev::Key { sym } => {
                    let n = def.items.len();
                    match sym {
                        keysym::ESCAPE => break PopRes::Cancel,
                        keysym::DOWN | keysym::UP if n > 0 => {
                            let down = sym == keysym::DOWN;
                            let mut i = hover.or(def.current).unwrap_or(if down { n - 1 } else { 0 });
                            for _ in 0..n {
                                i = if down { (i + 1) % n } else { (i + n - 1) % n };
                                if def.items[i].clickable() {
                                    break;
                                }
                            }
                            hover = Some(i);
                            paint(&mut canvas, hover);
                            self.show(pop, &canvas);
                        }
                        keysym::RETURN | keysym::KP_ENTER => {
                            if let Some(i) = hover.filter(|&i| def.items[i].clickable() && def.sub(i).is_none()) {
                                break PopRes::Pick(def.cmds[i].clone());
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        };
        self.close_overlay(pop);
        result
    }

    // ---------------------------------------------------------------------------------------------------------------
    // "Type in value": a one-line box at a screen point. Enter accepts, Esc or a click elsewhere cancels.

    pub fn type_in(&mut self, x: i32, y: i32, text: &str, scale: f32) -> Option<String> {
        if crate::no_modal() {
            return None;
        }
        self.unlock();
        let (w, h) = ((140.0 * scale).round() as i32, (22.0 * scale).round() as i32);
        let b = self.screen_at(x, y);
        let x = x.min(b[2] - w).max(b[0]);
        let y = y.min(b[3] - h).max(b[1]);
        let mut canvas = Canvas::new(w as u32, h as u32, scale).ok()?;
        let boxw = self.overlay([x, y, w, h])?;
        self.grab();
        let mut s: Vec<char> = text.chars().collect();
        let mut caret = s.len();
        // The whole text starts selected: typing replaces it.
        let mut all = true;
        let paint = |canvas: &mut Canvas, s: &[char], caret: usize, all: bool| {
            let (lw, lh) = (w as f32 / scale, h as f32 / scale);
            let f = Font::new(14);
            canvas.begin_frame(None);
            canvas.clear(Color::rgb(0xffffff));
            for r in [Rect::new(0.0, 0.0, lw, 1.0), Rect::new(0.0, lh - 1.0, lw, 1.0), Rect::new(0.0, 0.0, 1.0, lh), Rect::new(lw - 1.0, 0.0, 1.0, lh)] {
                canvas.fill_rect(r, Color::rgb(0x646464));
            }
            let text: String = s.iter().collect();
            let line = canvas.line_height(f);
            let ty = ((lh - line) / 2.0).round();
            if all && !text.is_empty() {
                let tw = canvas.text_width(f, &text);
                canvas.fill_rect(Rect::new(4.0, ty, tw, line), Color::rgb(0x0078d7));
                canvas.text(f, 4.0, ty, Color::rgb(0xffffff), &text);
            } else {
                canvas.text(f, 4.0, ty, Color::rgb(0x000000), &text);
                let before: String = s[..caret].iter().collect();
                let cx = 4.0 + canvas.text_width(f, &before);
                canvas.fill_rect(Rect::new(cx, ty, 1.0, line), Color::rgb(0x000000));
            }
        };
        paint(&mut canvas, &s, caret, all);
        self.show(boxw, &canvas);
        let r = [x, y, x + w, y + h];
        let result = loop {
            let Some(e) = self.event() else { break None };
            match e.ev {
                Ev::Expose if e.surface == boxw => self.show(boxw, &canvas),
                Ev::Outside => break None,
                Ev::Down { at, .. } => {
                    let (sx, sy) = (at.sx, at.sy);
                    if sx < r[0] || sx >= r[2] || sy < r[1] || sy >= r[3] {
                        break None;
                    }
                    all = false;
                    caret = s.len();
                }
                Ev::Key { sym } => {
                    let ctrl = e.mods.0;
                    match sym {
                        keysym::RETURN | keysym::KP_ENTER => break Some(s.iter().collect()),
                        keysym::ESCAPE => break None,
                        keysym::BACKSPACE | keysym::DELETE if all => {
                            s.clear();
                            caret = 0;
                            all = false;
                        }
                        keysym::BACKSPACE if caret > 0 => {
                            caret -= 1;
                            s.remove(caret);
                        }
                        keysym::DELETE if caret < s.len() => {
                            s.remove(caret);
                        }
                        keysym::LEFT => {
                            caret = if all { 0 } else { caret.saturating_sub(1) };
                            all = false;
                        }
                        keysym::RIGHT => {
                            caret = if all { s.len() } else { (caret + 1).min(s.len()) };
                            all = false;
                        }
                        keysym::HOME => (caret, all) = (0, false),
                        keysym::END => (caret, all) = (s.len(), false),
                        _ if ctrl && keysym_char(sym).is_some_and(|c| c.eq_ignore_ascii_case(&'a')) => all = true,
                        _ if !ctrl => {
                            if let Some(ch) = keysym_char(sym) {
                                if all {
                                    s.clear();
                                    caret = 0;
                                    all = false;
                                }
                                s.insert(caret, ch);
                                caret += 1;
                            }
                        }
                        _ => {}
                    }
                    paint(&mut canvas, &s, caret, all);
                    self.show(boxw, &canvas);
                }
                _ => {}
            }
        };
        self.close_overlay(boxw);
        self.ungrab();
        result
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        self.unlock();
        let shared = self.shared.clone();
        on_main(move |_| close_all(&shared));
    }
}

enum PopRes<C> {
    Pick(C),
    Cancel,
    /// The mouse went back to the parent menu.
    Back,
}
