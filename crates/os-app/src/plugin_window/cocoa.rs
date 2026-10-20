// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 ariymarkowitz
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Plugin windows on macOS: the frame (`super`) in an AppKit view. Docked, as on Windows: a subview of the main window's
//! view (winit's, the owner), above the app's own drawing (softbuffer's layer in the same view), masked to the workspace
//! minus the app's windows and popups above it (a shape layer on the frame's layer; clicks outside the mask fall through
//! to the main window) and stacked with the app's windows. The menu arrow's "Detached" moves the same view into a
//! borderless panel of its own: a floating panel, above the main window while Lemon Studio is active and hidden while
//! another application is (owned windows on Windows stay above their owner in the same way). The plugin parents its
//! editor into a child view below the strip, in points: editors follow the window's backing scale themselves.
//!
//! The frame is drawn with the os-ui canvas at the backing scale and handed to its layer as an image (`updateLayer`).
//! Screen points (`FrameEvent::Menu` and the like, `default_position`, `pointer`) are winit's screen coordinates on
//! macOS: device px at the main window's scale from the top-left of the primary screen.
//!
//! One local event monitor serves every frame: a press inside a frame (its editor included) brings it to the front of
//! the app's windows, and keys that go to a plugin's editor still play the typing keyboard (`take_keys`).
//!
// spec: ui/plugin-window.md, plugins/host-link.md

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::rc::{Rc, Weak};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message};
use objc2_app_kit::{
    NSBackingStoreType, NSEvent, NSEventMask, NSEventModifierFlags, NSEventType, NSPanel, NSResponder, NSScreen, NSTrackingArea, NSTrackingAreaOptions, NSView, NSWindow,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_core_foundation::{CFRetained, CGFloat, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo, CGImageComponentInfo, CGImagePixelFormatInfo, CGMutablePath,
};
use objc2_foundation::{NSComparisonResult, NSPoint, NSRect, NSSize};
use objc2_quartz_core::{CAShapeLayer, CATransaction};

use super::*;

/// A key pressed or released while a plugin's editor has the keyboard focus (the typing keyboard keeps playing).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyMsg {
    /// The key's virtual key code (`NSEvent.keyCode`), as winit's `PhysicalKeyExtScancode` expects it on macOS.
    pub scancode: u32,
    pub pressed: bool,
    /// Control or Command.
    pub ctrl: bool,
    /// Option.
    pub alt: bool,
}

/// Window level of the lifted popup: above the floating plugin panels (`NSPopUpMenuWindowLevel`).
const LIFT_LEVEL: isize = 101;

thread_local! {
    /// The open frames (the event monitor and `row_window_at` look them up).
    static FRAMES: RefCell<Vec<Weak<Frame>>> = const { RefCell::new(Vec::new()) };
    /// The main window's view, once a frame was made for it (`screen_to_main`).
    static OWNER: Cell<isize> = const { Cell::new(0) };
    /// Keys for the typing keyboard since the last `take_keys`.
    static KEYS: RefCell<Vec<KeyMsg>> = const { RefCell::new(Vec::new()) };
    /// The event monitor is installed.
    static MONITOR: Cell<bool> = const { Cell::new(false) };
}

/// Keys seen since the last call (UI thread).
pub fn take_keys() -> Vec<KeyMsg> {
    KEYS.with(|k| std::mem::take(&mut *k.borrow_mut()))
}

/// Docked frames are views AppKit composites over the main window's own layer, which keeps the whole canvas: nothing
/// they uncover needs presenting again (Windows and Linux do).
pub fn take_exposed() -> bool {
    false
}

// ---------------------------------------------------------------------------------------------------------------
// Screen points: device px at the main window's scale, from the primary screen's top-left (winit's convention). AppKit's
// are points from the primary screen's bottom-left.

/// Height of the primary screen (the one AppKit's screen coordinates start from), points.
fn primary_height(mtm: MainThreadMarker) -> CGFloat {
    NSScreen::screens(mtm).firstObject().map_or(0.0, |s| s.frame().size.height)
}

/// An AppKit screen point as a screen point at scale `k`.
fn to_px(mtm: MainThreadMarker, p: NSPoint, k: CGFloat) -> (i32, i32) {
    ((p.x * k).round() as i32, ((primary_height(mtm) - p.y) * k).round() as i32)
}

/// A screen point at scale `k` as an AppKit screen point.
fn from_px(mtm: MainThreadMarker, (x, y): (i32, i32), k: CGFloat) -> NSPoint {
    NSPoint::new(f64::from(x) / k, primary_height(mtm) - f64::from(y) / k)
}

/// The view behind a handle the app holds (`Plugins::owner`, `FrameWindow::hwnd`).
///
/// # Safety
/// `h` must be 0 or a live `NSView`.
unsafe fn view_of<'a>(h: isize) -> Option<&'a NSView> {
    unsafe { (h as *const NSView).as_ref() }
}

/// The scale of the main window's view `owner` (1 without a window).
fn scale_of(owner: &NSView) -> CGFloat {
    owner.window().map_or(1.0, |w| w.backingScaleFactor())
}

/// A point of the main window's view (points) on the screen (AppKit's).
fn owner_to_screen(owner: &NSView, p: NSPoint) -> NSPoint {
    let w = owner.convertPoint_toView(p, None);
    owner.window().map_or(w, |win| win.convertPointToScreen(w))
}

/// An AppKit screen point in the main window's view (points; the view is flipped: top-left origin).
fn screen_to_owner(owner: &NSView, p: NSPoint) -> NSPoint {
    let w = owner.window().map_or(p, |win| win.convertPointFromScreen(p));
    owner.convertPoint_fromView(w, None)
}

/// A screen point in the main window's view (physical px), for the app's popups (`App::screen_to_canvas`).
pub fn screen_to_main(x: i32, y: i32) -> Option<(i32, i32)> {
    let mtm = MainThreadMarker::new()?;
    let owner = unsafe { view_of(OWNER.with(Cell::get)) }?;
    let k = scale_of(owner);
    let p = screen_to_owner(owner, from_px(mtm, (x, y), k));
    Some(((p.x * k).round() as i32, (p.y * k).round() as i32))
}

/// The mouse on the screen (screen px), once a frame was opened.
pub fn pointer() -> Option<(i32, i32)> {
    let mtm = MainThreadMarker::new()?;
    let owner = unsafe { view_of(OWNER.with(Cell::get)) }?;
    Some(to_px(mtm, NSEvent::mouseLocation(), scale_of(owner)))
}

/// Screen position for a new plugin window: right of the main window's left edge, below its toolbar (screen px; the
/// offset is logical, so the window lands on the same part of the layout at every scale).
pub fn default_position(owner: isize, scale: f32) -> (i32, i32) {
    let (Some(mtm), Some(view)) = (MainThreadMarker::new(), unsafe { view_of(owner) }) else { return (0, 0) };
    let (x, y) = to_px(mtm, owner_to_screen(view, NSPoint::new(0.0, 0.0)), f64::from(scale));
    (x + (420.0 * scale).round() as i32, y + (160.0 * scale).round() as i32)
}

// ---------------------------------------------------------------------------------------------------------------
// The native classes.

define_class!(
    // SAFETY: NSView has no subclassing requirements; no `Drop`.
    #[unsafe(super(NSView, NSResponder))]
    #[thread_kind = MainThreadOnly]
    #[name = "LemonStudioPluginFrame"]
    #[ivars = Weak<Frame>]
    struct FrameView;

    impl FrameView {
        /// Top-left origin, as the frame is laid out.
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        /// The first click on an inactive panel acts at once, as everywhere in the app.
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _e: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(wantsUpdateLayer))]
        fn wants_update_layer(&self) -> bool {
            true
        }

        #[unsafe(method(updateLayer))]
        fn update_layer(&self) {
            if let Some(f) = self.ivars().upgrade() {
                f.render();
            }
        }

        /// Docked: only the shown parts take the mouse; the rest belongs to what the app draws over the frame.
        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, p: NSPoint) -> Option<Retained<NSView>> {
            let local = match unsafe { self.superview() } {
                Some(s) => self.convertPoint_fromView(p, Some(&s)),
                None => p,
            };
            if self.ivars().upgrade().is_some_and(|f| f.shows(local)) {
                unsafe { msg_send![super(self), hitTest: p] }
            } else {
                None
            }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, e: &NSEvent) {
            if let Some(f) = self.ivars().upgrade() {
                f.press(e, Btn::Left);
            }
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, e: &NSEvent) {
            if let Some(f) = self.ivars().upgrade() {
                f.press(e, Btn::Right);
            }
        }

        #[unsafe(method(otherMouseDown:))]
        fn other_mouse_down(&self, e: &NSEvent) {
            if let (Some(f), 2) = (self.ivars().upgrade(), e.buttonNumber()) {
                f.press(e, Btn::Middle);
            }
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, e: &NSEvent) {
            if let Some(f) = self.ivars().upgrade() {
                let (x, y) = f.local(e);
                let r = f.state.borrow_mut().left_up(x, y);
                f.reply(r);
            }
        }

        /// The other buttons' releases and middle drags mean nothing to the frame; a docked one keeps them from the main
        /// window behind it (it would get releases without their presses).
        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, _e: &NSEvent) {}

        #[unsafe(method(otherMouseUp:))]
        fn other_mouse_up(&self, _e: &NSEvent) {}

        #[unsafe(method(otherMouseDragged:))]
        fn other_mouse_dragged(&self, _e: &NSEvent) {}

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, e: &NSEvent) {
            if let Some(f) = self.ivars().upgrade() {
                f.motion(e);
            }
        }

        #[unsafe(method(rightMouseDragged:))]
        fn right_mouse_dragged(&self, e: &NSEvent) {
            if let Some(f) = self.ivars().upgrade() {
                f.motion(e);
            }
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, e: &NSEvent) {
            if let Some(f) = self.ivars().upgrade() {
                f.motion(e);
            }
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, e: &NSEvent) {
            if let Some(f) = self.ivars().upgrade() {
                f.wheel(e);
            }
        }

        /// Keys a plugin's editor leaves would go on up to the main window's view: the event monitor already gave them
        /// to the typing keyboard (`take_keys`).
        #[unsafe(method(keyDown:))]
        fn key_down(&self, _e: &NSEvent) {}

        #[unsafe(method(keyUp:))]
        fn key_up(&self, _e: &NSEvent) {}

        #[unsafe(method(flagsChanged:))]
        fn flags_changed(&self, _e: &NSEvent) {}

        /// Moved to a screen with another scale: the app redraws the frame and reopens the editor (spec:
        /// ui/ui-scaling.md §3).
        #[unsafe(method(viewDidChangeBackingProperties))]
        fn backing_changed(&self) {
            let (Some(f), Some(w)) = (self.ivars().upgrade(), self.window()) else { return };
            let k = w.backingScaleFactor() as f32;
            if k != f.scale.get() {
                f.state.borrow_mut().events.push(FrameEvent::Dpi(k));
                os_clap::wake();
            }
        }
    }
);

impl FrameView {
    fn new(mtm: MainThreadMarker, frame: Weak<Frame>, rect: NSRect) -> Retained<FrameView> {
        let this = mtm.alloc::<FrameView>().set_ivars(frame);
        unsafe { msg_send![super(this), initWithFrame: rect] }
    }
}

define_class!(
    // SAFETY: NSPanel has no subclassing requirements; no `Drop`.
    #[unsafe(super(NSPanel, NSWindow, NSResponder))]
    #[thread_kind = MainThreadOnly]
    #[name = "LemonStudioPluginPanel"]
    struct FramePanel;

    impl FramePanel {
        /// Borderless windows do not take the keyboard by default; plugin editors have text fields.
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key(&self) -> bool {
            true
        }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main(&self) -> bool {
            false
        }

        /// Keys that no view of the panel takes end here (the typing keyboard has them already, see `monitor`), not in
        /// AppKit's beep.
        #[unsafe(method(keyDown:))]
        fn key_down(&self, _e: &NSEvent) {}

        #[unsafe(method(keyUp:))]
        fn key_up(&self, _e: &NSEvent) {}
    }
);

impl FramePanel {
    /// A hidden detached frame's panel at `rect` (AppKit screen coordinates).
    fn new(mtm: MainThreadMarker, rect: NSRect) -> Retained<FramePanel> {
        let this = mtm.alloc::<FramePanel>().set_ivars(());
        let p: Retained<FramePanel> =
            unsafe { msg_send![super(this), initWithContentRect: rect, styleMask: NSWindowStyleMask::Borderless, backing: NSBackingStoreType::Buffered, defer: false] };
        unsafe { p.setReleasedWhenClosed(false) };
        // Above the main window while Lemon Studio is active, hidden while another application is.
        p.setFloatingPanel(true);
        p.setHidesOnDeactivate(true);
        p.setBecomesKeyOnlyIfNeeded(false);
        p.setHasShadow(true);
        p.setCollectionBehavior(NSWindowCollectionBehavior::FullScreenAuxiliary);
        p
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The frame.

/// A plugin window's native parts.
struct Frame {
    mtm: MainThreadMarker,
    view: Retained<FrameView>,
    /// The child view the plugin's editor goes into.
    area: Retained<NSView>,
    /// The main window's view (winit's); `None` in tests.
    owner: Option<Retained<NSView>>,
    /// Detached: the panel the frame is the content of.
    panel: RefCell<Option<Retained<FramePanel>>>,
    state: RefCell<State>,
    canvas: RefCell<Canvas>,
    /// The backing scale the frame is drawn at.
    scale: Cell<f32>,
    /// Docked and clipped: the parts shown (frame points); `None`: all of it.
    shown: RefCell<Option<Vec<NSRect>>>,
    /// Wheel movement short of a whole notch (trackpads).
    wheel: Cell<f32>,
}

/// A plugin window. Dropping it removes the native views (close the plugin's editor first).
pub struct FrameWindow {
    f: Rc<Frame>,
}

/// AppKit rect of a point rect.
fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}

impl FrameWindow {
    /// A hidden frame for an editor of `editor` logical px (points) at screen position `pos` (screen px, top-left): a
    /// view in `owner` (the main window's view; docked) or a panel of its own (`detached`). `raise` shows it.
    pub fn new(owner: isize, info: ChannelInfo, editor: (f32, f32), scale: f32, pos: (i32, i32), detached: bool) -> Result<FrameWindow, String> {
        let mtm = MainThreadMarker::new().ok_or("plugin windows open on the main thread")?;
        install_monitor(mtm);
        let owner_view = unsafe { view_of(owner) }.map(|v| v.retain());
        // Without a main window (tests) there is nothing to dock into.
        let detached = detached || owner_view.is_none();
        if owner != 0 {
            OWNER.with(|o| o.set(owner));
        }
        let state = State::new(info, editor, detached);
        let (w, h) = state.size();
        let ay = state.area_y();
        let main_k = owner_view.as_deref().map_or(f64::from(scale.max(1.0)), scale_of);
        let f = Rc::new_cyclic(|me: &Weak<Frame>| {
            let view = FrameView::new(mtm, me.clone(), rect(0.0, 0.0, f64::from(w), f64::from(h)));
            view.setWantsLayer(true);
            let area = NSView::initWithFrame(NSView::alloc(mtm), rect(f64::from(BORDER), f64::from(ay), f64::from(editor.0), f64::from(editor.1)));
            view.addSubview(&area);
            // Hover over the frame (hints, the Misc page), whether or not its window has the keyboard.
            let opts = NSTrackingAreaOptions::MouseMoved | NSTrackingAreaOptions::ActiveAlways | NSTrackingAreaOptions::InVisibleRect;
            let track = unsafe { NSTrackingArea::initWithRect_options_owner_userInfo(NSTrackingArea::alloc(), NSRect::ZERO, opts, Some(track_owner(&view)), None) };
            view.addTrackingArea(&track);
            view.setHidden(true);
            Frame {
                mtm,
                view,
                area,
                owner: owner_view.clone(),
                panel: RefCell::new(None),
                state: RefCell::new(state),
                // Sized at the real scale once placed (`layout`).
                canvas: RefCell::new(Canvas::new(1, 1, 1.0).expect("canvas")),
                scale: Cell::new(scale),
                shown: RefCell::new(None),
                wheel: Cell::new(0.0),
            }
        });
        let at = from_px(mtm, pos, main_k);
        match &f.owner {
            Some(o) if !detached => {
                o.addSubview(&f.view);
                f.view.setFrameOrigin(screen_to_owner(o, at));
            }
            _ => {
                let p = FramePanel::new(mtm, rect(at.x, at.y - f64::from(h), f64::from(w), f64::from(h)));
                p.setContentView(Some(&f.view));
                p.setFrameTopLeftPoint(at);
                *f.panel.borrow_mut() = Some(p);
            }
        }
        f.scale.set(f.view.window().map_or(scale, |w| w.backingScaleFactor() as f32));
        f.layout();
        FRAMES.with(|fr| fr.borrow_mut().push(Rc::downgrade(&f)));
        Ok(FrameWindow { f })
    }

    /// In a panel of its own, not in the main window.
    pub fn detached(&self) -> bool {
        self.f.state.borrow().detached
    }

    /// Docks the window into `owner` (a view of the main window, same place on screen) or detaches it (a panel of its
    /// own). The plugin's editor stays embedded.
    pub fn set_detached(&self, owner: isize, on: bool) {
        let f = &self.f;
        let Some(o) = f.owner.clone().filter(|o| Retained::as_ptr(o) as isize == owner) else { return };
        if f.state.borrow().detached == on {
            return;
        }
        {
            let mut st = f.state.borrow_mut();
            st.detached = on;
            st.clip = None;
            st.clip_key = None;
        }
        let (w, h) = f.state.borrow().size();
        if on {
            let at = owner_to_screen(&o, f.view.frame().origin);
            f.unmask();
            f.view.removeFromSuperview();
            let p = FramePanel::new(f.mtm, rect(at.x, at.y - f64::from(h), f64::from(w), f64::from(h)));
            p.setContentView(Some(&f.view));
            p.setFrameTopLeftPoint(at);
            *f.panel.borrow_mut() = Some(p);
        } else if let Some(p) = f.panel.borrow_mut().take() {
            let fr = p.frame();
            let at = NSPoint::new(fr.origin.x, fr.origin.y + fr.size.height);
            p.setContentView(None);
            p.orderOut(None);
            p.close();
            o.addSubview(&f.view);
            f.view.setFrameOrigin(screen_to_owner(&o, at));
        }
        self.raise();
    }

    /// Clips a docked window to `work` minus the `covers` (the app's windows and popups above it), all in the main
    /// window's view (physical px). Moving the window by its title bar re-clips with the same rects.
    pub fn clip_to(&self, work: [i32; 4], covers: &[[i32; 4]]) {
        if self.detached() {
            return;
        }
        self.f.state.borrow_mut().clip = Some((work, covers.to_vec()));
        self.f.apply_clip();
    }

    /// The frame's view (the same docked and detached; drops find their plugin by it).
    pub fn hwnd(&self) -> isize {
        Retained::as_ptr(&self.f.view) as isize
    }

    /// The child view the plugin's editor goes into (an `NSView`, as plugin APIs take it).
    pub fn area(&self) -> *mut c_void {
        Retained::as_ptr(&self.f.area) as *mut c_void
    }

    /// Shows the window and brings it to the front (a docked one in front of the other docked windows).
    pub fn raise(&self) {
        let f = &self.f;
        f.view.setHidden(false);
        match f.panel.borrow().as_ref() {
            Some(p) => p.makeKeyAndOrderFront(None),
            None => f.front(),
        }
        f.view.setNeedsDisplay(true);
    }

    pub fn set_info(&self, info: ChannelInfo) {
        let mut st = self.f.state.borrow_mut();
        if st.info != info && st.drag.is_none() && st.misc_ui.drag.is_none() {
            st.info = info;
            self.f.view.setNeedsDisplay(true);
        }
    }

    /// The plugin's own page tabs (a Lemon plugin joined through the host link), replacing the plug tab.
    pub fn set_pages(&self, pages: Vec<PageIcon>, active: usize) {
        let mut st = self.f.state.borrow_mut();
        if !pages.is_empty() && (st.pages != pages || st.page != active) {
            st.page = active.min(pages.len() - 1);
            st.pages = pages;
            self.f.view.setNeedsDisplay(true);
        }
    }

    /// The plugin changed its page itself.
    pub fn set_page(&self, page: usize) {
        let mut st = self.f.state.borrow_mut();
        if st.page != page && page < st.pages.len() {
            st.page = page;
            self.f.view.setNeedsDisplay(true);
        }
    }

    /// The plugin's hint line (parameter under the mouse).
    pub fn set_hint(&self, hint: String) {
        let mut st = self.f.state.borrow_mut();
        if st.hint != hint {
            st.hint = hint;
            self.f.view.setNeedsDisplay(true);
        }
    }

    /// The loaded preset's name for the Presets box.
    pub fn set_preset(&self, name: Option<String>) {
        let mut st = self.f.state.borrow_mut();
        if st.preset != name {
            st.preset = name;
            self.f.view.setNeedsDisplay(true);
        }
    }

    /// The backing scale the frame is drawn at (2 on Retina screens).
    pub fn scale(&self) -> f32 {
        self.f.scale.get()
    }

    /// Redraws the frame at another backing scale (same size in points). The editor inside is the caller's: it reopens
    /// it, so that its sizes are measured at the new scale (spec: ui/ui-scaling.md §3).
    pub fn set_scale(&self, scale: f32) {
        if scale <= 0.0 || self.f.scale.get() == scale {
            return;
        }
        self.f.scale.set(scale);
        self.f.state.borrow_mut().clip_key = None;
        self.f.layout();
    }

    /// Follows a new editor size (plugins may resize).
    pub fn set_editor_size(&self, editor: (f32, f32)) {
        if self.f.state.borrow().editor != editor {
            self.f.state.borrow_mut().editor = editor;
            self.f.layout();
        } else {
            // Editors move their views as they resize, before or after the frame follows them.
            self.f.pin_editor();
        }
    }

    /// Shows or hides the gear strip (mixer effects open without it).
    pub fn set_strip(&self, on: bool) {
        if self.f.state.borrow().strip != on {
            self.f.state.borrow_mut().strip = on;
            self.f.layout();
        }
    }

    pub fn take_events(&self) -> Vec<FrameEvent> {
        std::mem::take(&mut self.f.state.borrow_mut().events)
    }
}

impl Drop for FrameWindow {
    fn drop(&mut self) {
        let f = &self.f;
        FRAMES.with(|fr| fr.borrow_mut().retain(|w| w.upgrade().is_some_and(|g| !Rc::ptr_eq(&g, f))));
        // An editor that held the main window's keyboard gives it back to the main window's view (the typing keyboard).
        let window = f.view.window();
        let held = window.as_ref().and_then(|w| w.firstResponder()).and_then(|r| r.downcast::<NSView>().ok()).is_some_and(|v| v.isDescendantOf(&f.view));
        f.view.removeFromSuperview();
        if let Some(p) = f.panel.borrow_mut().take() {
            p.setContentView(None);
            p.orderOut(None);
            p.close();
        } else if let (true, Some(w), Some(o)) = (held, window, &f.owner) {
            w.makeFirstResponder(Some(o));
        }
    }
}

/// Puts the main window's docked frames `hwnds` (bottom to top) in that order when they are not already: the subview
/// order (clicks) and the layers' (drawing, above the app's own layer).
pub fn stack(parent: isize, hwnds: &[isize]) {
    let Some(owner) = (unsafe { view_of(parent) }) else { return };
    order(owner, hwnds);
}

/// Orders `owner`'s subviews as `hwnds` (bottom to top; others below them) and lifts their layers above the app's.
fn order(owner: &NSView, hwnds: &[isize]) {
    unsafe extern "C-unwind" fn by_order(a: NonNull<NSView>, b: NonNull<NSView>, ctx: *mut c_void) -> NSComparisonResult {
        let order = unsafe { &*(ctx as *const Vec<isize>) };
        let at = |v: NonNull<NSView>| order.iter().position(|&h| h == v.as_ptr() as isize).map_or(-1, |i| i as isize);
        match at(a).cmp(&at(b)) {
            std::cmp::Ordering::Less => NSComparisonResult::Ascending,
            std::cmp::Ordering::Equal => NSComparisonResult::Same,
            std::cmp::Ordering::Greater => NSComparisonResult::Descending,
        }
    }
    let now: Vec<isize> = owner.subviews().iter().map(|v| Retained::as_ptr(&v) as isize).filter(|h| hwnds.contains(h)).collect();
    if now != hwnds {
        let want = hwnds.to_vec();
        unsafe { owner.sortSubviewsUsingFunction_context(by_order, &want as *const Vec<isize> as *mut c_void) };
    }
    // softbuffer draws the app in a layer of the same view: the frames' layers go above it, in this order.
    for v in owner.subviews().iter() {
        let Some(n) = hwnds.iter().position(|&h| h == Retained::as_ptr(&v) as isize) else { continue };
        if let Some(l) = v.layer() {
            let z = 1.0 + n as CGFloat;
            if l.zPosition() != z {
                l.setZPosition(z);
            }
        }
    }
}

impl Frame {
    /// The docked frames of the same main window, bottom to top.
    fn siblings(&self) -> Vec<isize> {
        let Some(o) = &self.owner else { return Vec::new() };
        let frames: Vec<isize> = FRAMES.with(|fr| fr.borrow().iter().filter_map(Weak::upgrade).map(|f| Retained::as_ptr(&f.view) as isize).collect());
        o.subviews().iter().map(|v| Retained::as_ptr(&v) as isize).filter(|h| frames.contains(h) || *h == Retained::as_ptr(&self.view) as isize).collect()
    }

    /// A docked frame goes in front of the other docked frames at once (the app restacks on its tick).
    fn front(&self) {
        let (Some(o), false) = (&self.owner, self.state.borrow().detached) else { return };
        let me = Retained::as_ptr(&self.view) as isize;
        let mut v = self.siblings();
        v.retain(|&h| h != me);
        v.push(me);
        order(o, &v);
    }

    /// The main window's scale (screen points are measured at it).
    fn main_scale(&self) -> CGFloat {
        self.owner.as_deref().map_or(f64::from(self.scale.get()), scale_of)
    }

    /// A frame point (logical px) on the screen (screen px).
    fn to_screen(&self, x: f32, y: f32) -> (i32, i32) {
        let w = self.view.convertPoint_toView(NSPoint::new(f64::from(x), f64::from(y)), None);
        let s = self.view.window().map_or(w, |win| win.convertPointToScreen(w));
        to_px(self.mtm, s, self.main_scale())
    }

    /// The frame's top-left on the screen (screen px).
    fn top_left(&self) -> (i32, i32) {
        self.to_screen(0.0, 0.0)
    }

    /// Moves the frame's top-left to screen point `at` (screen px).
    fn place(&self, at: (i32, i32)) {
        let p = from_px(self.mtm, at, self.main_scale());
        match (self.panel.borrow().as_ref(), &self.owner) {
            (Some(panel), _) => panel.setFrameTopLeftPoint(p),
            (None, Some(o)) => self.view.setFrameOrigin(screen_to_owner(o, p)),
            (None, None) => {}
        }
    }

    /// Resizes the frame to its state (strip shown or not, editor size) and moves the editor area.
    fn layout(&self) {
        let st = self.state.borrow();
        let (fw, fh) = st.size();
        let (ay, ed, show) = (st.area_y(), st.editor, !st.misc);
        drop(st);
        let (w, h) = (f64::from(fw), f64::from(fh));
        match self.panel.borrow().as_ref() {
            // Panels grow from their bottom-left: keep the top-left where it is.
            Some(p) => {
                let fr = p.frame();
                p.setFrame_display(rect(fr.origin.x, fr.origin.y + fr.size.height - h, w, h), true);
            }
            // Docked: the main window's view is flipped, the top-left stays.
            None => self.view.setFrameSize(NSSize::new(w, h)),
        }
        self.area.setFrame(rect(f64::from(BORDER), f64::from(ay), f64::from(ed.0), f64::from(ed.1)));
        self.pin_editor();
        self.area.setHidden(!show);
        let k = self.scale.get();
        let px = |v: f32| (v * k).round().max(1.0) as u32;
        self.canvas.borrow_mut().resize(px(fw), px(fh), k);
        self.state.borrow_mut().clip_key = None;
        self.apply_clip();
        self.view.setNeedsDisplay(true);
    }

    /// Keeps the editor's views at the area's top-left, as on Windows and X11. The area is not flipped, so a view in it
    /// keeps its distance from the bottom when the area changes height, and editors place themselves against the
    /// area's height when they open or resize (before the frame follows them): Vital, resized from its own settings,
    /// hung below a gap at the top; Surge, placed at the origin of a smaller area, was cut off at the top.
    fn pin_editor(&self) {
        let h = self.area.frame().size.height;
        for v in self.area.subviews().iter() {
            let f = v.frame();
            let y = h - f.size.height;
            if f.origin.x != 0.0 || f.origin.y != y {
                v.setFrameOrigin(NSPoint::new(0.0, y));
            }
        }
    }

    /// Paints the frame into its layer (`updateLayer`).
    fn render(&self) {
        let mut canvas = self.canvas.borrow_mut();
        super::paint(&mut canvas, &self.state.borrow());
        let (w, h) = (canvas.pixmap.width() as usize, canvas.pixmap.height() as usize);
        let mut xrgb = vec![0u32; w * h];
        canvas.copy_to_xrgb(&mut xrgb);
        drop(canvas);
        if let (Some(img), Some(layer)) = (image(xrgb, w, h), self.view.layer()) {
            unsafe { layer.setContents(Some(img.as_ref())) };
        }
    }

    /// Whether frame point `p` (points) is shown (docked frames are clipped).
    fn shows(&self, p: NSPoint) -> bool {
        self.shown.borrow().as_ref().is_none_or(|rs| rs.iter().any(|r| p.x >= r.origin.x && p.y >= r.origin.y && p.x < r.origin.x + r.size.width && p.y < r.origin.y + r.size.height))
    }

    /// No clip (detached).
    fn unmask(&self) {
        *self.shown.borrow_mut() = None;
        if let Some(l) = self.view.layer() {
            l.setZPosition(0.0);
            unsafe { l.setMask(None) };
        }
    }

    /// Sets a docked frame's mask for its current place from `State::clip`: the workspace minus the rects covering it.
    /// Its editor is masked with it (a sublayer).
    fn apply_clip(&self) {
        if self.panel.borrow().is_some() {
            return;
        }
        let k = self.main_scale();
        let fr = self.view.frame();
        let me = [
            (fr.origin.x * k).round() as i32,
            (fr.origin.y * k).round() as i32,
            ((fr.origin.x + fr.size.width) * k).round() as i32,
            ((fr.origin.y + fr.size.height) * k).round() as i32,
        ];
        let rects = {
            let mut st = self.state.borrow_mut();
            let Some((work, covers)) = st.clip.as_ref() else { return };
            // Only what overlaps the window matters.
            let hit = |c: &[i32; 4]| c[0] < me[2] && c[2] > me[0] && c[1] < me[3] && c[3] > me[1];
            let covers: Vec<[i32; 4]> = covers.iter().copied().filter(hit).collect();
            let work = *work;
            let key: Vec<i32> = me.into_iter().chain(work).chain(covers.iter().flatten().copied()).collect();
            if st.clip_key.as_ref() == Some(&key) {
                return;
            }
            st.clip_key = Some(key);
            let inside = work[0] <= me[0] && work[1] <= me[1] && work[2] >= me[2] && work[3] >= me[3];
            (!inside || !covers.is_empty()).then(|| visible(me, work, &covers))
        };
        let Some(layer) = self.view.layer() else { return };
        // The mask's own changes must not animate.
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        match rects {
            None => {
                *self.shown.borrow_mut() = None;
                unsafe { layer.setMask(None) };
            }
            Some(rs) => {
                let local: Vec<NSRect> = rs
                    .iter()
                    .map(|r| rect(f64::from(r[0] - me[0]) / k, f64::from(r[1] - me[1]) / k, f64::from(r[2] - r[0]) / k, f64::from(r[3] - r[1]) / k))
                    .collect();
                let path = CGMutablePath::new();
                // The mask takes the flipped view's coordinates (y down), as AppKit draws its layer.
                for r in &local {
                    unsafe { CGMutablePath::add_rect(Some(&path), std::ptr::null(), CGRect::new(CGPoint::new(r.origin.x, r.origin.y), CGSize::new(r.size.width, r.size.height))) };
                }
                let mask = CAShapeLayer::new();
                mask.setFrame(layer.bounds());
                mask.setPath(Some(&path));
                unsafe { layer.setMask(Some(&mask)) };
                *self.shown.borrow_mut() = Some(local);
            }
        }
        CATransaction::commit();
    }

    /// Event point in the frame (logical px, top-left origin).
    fn local(&self, e: &NSEvent) -> (f32, f32) {
        let p = self.view.convertPoint_fromView(e.locationInWindow(), None);
        (p.x as f32, p.y as f32)
    }

    fn press(&self, e: &NSEvent, b: Btn) {
        let (x, y) = self.local(e);
        let screen = |x: f32, y: f32| self.to_screen(x, y);
        let r = {
            let mut st = self.state.borrow_mut();
            match b {
                Btn::Left => st.left_down(x, y, e.clickCount() == 2, &screen),
                Btn::Right => st.right_down(x, y, &screen),
                Btn::Middle => st.middle_down(x, y, &screen),
            }
        };
        self.reply(r);
    }

    fn motion(&self, e: &NSEvent) {
        let moving = self.state.borrow().moving;
        if let Some((cx, cy, wx, wy)) = moving {
            let (x, y) = to_px(self.mtm, NSEvent::mouseLocation(), self.main_scale());
            self.place((wx + x - cx, wy + y - cy));
            // Re-clip at once for the new place.
            self.apply_clip();
            return;
        }
        let (x, y) = self.local(e);
        let flags = e.modifierFlags();
        let fine = flags.intersects(NSEventModifierFlags::Control | NSEventModifierFlags::Command);
        let r = self.state.borrow_mut().motion(x, y, fine);
        self.reply(r);
    }

    fn wheel(&self, e: &NSEvent) {
        // Trackpads scroll by pixels: 40 to the notch, as in the main window.
        let d = e.scrollingDeltaY() as f32;
        let total = self.wheel.get() + if e.hasPreciseScrollingDeltas() { d / 40.0 } else { d };
        let notches = total.trunc();
        self.wheel.set(total - notches);
        if notches != 0.0 {
            let (x, y) = self.local(e);
            let r = self.state.borrow_mut().wheel(x, y, notches);
            self.reply(r);
        }
    }

    /// Does what an input event asked of the window.
    fn reply(&self, r: Reply) {
        match r {
            Reply::None => {}
            // AppKit sends a press's drags to the view that took it: nothing to capture.
            Reply::Repaint | Reply::Capture | Reply::Hold => self.view.setNeedsDisplay(true),
            Reply::Relayout => self.layout(),
            Reply::Move => {
                // Moved by hand (`motion`), docked or not, from the cursor and the frame at the press.
                let (cx, cy) = to_px(self.mtm, NSEvent::mouseLocation(), self.main_scale());
                let (wx, wy) = self.top_left();
                self.state.borrow_mut().moving = Some((cx, cy, wx, wy));
                self.view.setNeedsDisplay(true);
            }
            Reply::Minimize => {
                // A docked window hides (its channel's name button shows it again); a borderless panel cannot go to
                // the Dock, so a detached one hides too.
                match self.panel.borrow().as_ref() {
                    Some(p) => p.orderOut(None),
                    None => self.view.setHidden(true),
                }
            }
        }
    }

    /// A press inside the frame (its editor included): it goes to the front of the app's windows.
    fn clicked(&self) {
        self.front();
        self.state.borrow_mut().events.push(FrameEvent::Raise);
        os_clap::wake();
    }
}

/// A view as the owner of its tracking area (it gets the mouse moves).
fn track_owner(v: &NSView) -> &AnyObject {
    v
}

/// An image of XRGB pixels (`w` x `h`) for a layer's contents.
fn image(xrgb: Vec<u32>, w: usize, h: usize) -> Option<CFRetained<CGImage>> {
    unsafe extern "C-unwind" fn release(_info: *mut c_void, data: NonNull<c_void>, size: usize) {
        let slice = std::ptr::slice_from_raw_parts_mut(data.cast::<u32>().as_ptr(), size / 4);
        // SAFETY: the boxed slice handed out below.
        drop(unsafe { Box::from_raw(slice) });
    }
    if w == 0 || h == 0 || xrgb.len() < w * h {
        return None;
    }
    let len = w * h * 4;
    let data = Box::into_raw(xrgb.into_boxed_slice()).cast::<c_void>();
    // SAFETY: `data` holds `len` bytes until `release` frees it.
    let provider = unsafe { CGDataProvider::with_data(std::ptr::null_mut(), data, len, Some(release)) }?;
    let space = CGColorSpace::new_device_rgb()?;
    // 32-bit little-endian XRGB, as the canvas copies it out (and softbuffer shows the main window).
    let info = CGBitmapInfo(CGImageAlphaInfo::NoneSkipFirst.0 | CGImageComponentInfo::Integer.0 | CGImageByteOrderInfo::Order32Little.0 | CGImagePixelFormatInfo::Packed.0);
    unsafe { CGImage::new(w, h, 8, 32, w * 4, Some(&space), info, Some(&provider), std::ptr::null(), false, CGColorRenderingIntent::RenderingIntentDefault) }
}

// ---------------------------------------------------------------------------------------------------------------
// The event monitor: presses in frames, keys in editors.

/// The frame a press at window point `p` of `window` lands in (its editor included), as AppKit would deliver it.
fn frame_at(window: &NSWindow, p: NSPoint) -> Option<Rc<Frame>> {
    let content = window.contentView()?;
    let q = match unsafe { content.superview() } {
        Some(s) => s.convertPoint_fromView(p, None),
        None => p,
    };
    let hit = content.hitTest(q)?;
    FRAMES.with(|fr| fr.borrow().iter().filter_map(Weak::upgrade).find(|f| hit.isDescendantOf(&f.view)))
}

/// Whether the keyboard goes into a frame of `window` (a plugin's editor, in the main window or a panel).
fn keys_in_frame(window: &NSWindow) -> bool {
    let Some(v) = window.firstResponder().and_then(|r| r.downcast::<NSView>().ok()) else {
        // The panel itself (no view has the keyboard): still a frame's.
        return FRAMES.with(|fr| fr.borrow().iter().filter_map(Weak::upgrade).any(|f| f.panel.borrow().as_ref().is_some_and(|p| std::ptr::eq(Retained::as_ptr(p).cast::<NSWindow>(), window))));
    };
    FRAMES.with(|fr| fr.borrow().iter().filter_map(Weak::upgrade).any(|f| v.isDescendantOf(&f.view)))
}

fn monitor(e: &NSEvent) {
    let Some(window) = MainThreadMarker::new().and_then(|mtm| e.window(mtm)) else { return };
    let t = e.r#type();
    if t == NSEventType::KeyDown || t == NSEventType::KeyUp {
        if t == NSEventType::KeyDown && e.isARepeat() || !keys_in_frame(&window) {
            return;
        }
        let flags = e.modifierFlags();
        let k = KeyMsg {
            scancode: u32::from(e.keyCode()),
            pressed: t == NSEventType::KeyDown,
            ctrl: flags.intersects(NSEventModifierFlags::Control | NSEventModifierFlags::Command),
            alt: flags.contains(NSEventModifierFlags::Option),
        };
        KEYS.with(|q| q.borrow_mut().push(k));
        os_clap::wake();
        return;
    }
    if let Some(f) = frame_at(&window, e.locationInWindow()) {
        f.clicked();
    }
}

/// Installs the event monitor once (it stays for the app's life).
fn install_monitor(_mtm: MainThreadMarker) {
    if MONITOR.with(|m| m.replace(true)) {
        return;
    }
    let mask = NSEventMask::KeyDown | NSEventMask::KeyUp | NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown | NSEventMask::OtherMouseDown;
    let block = RcBlock::new(|e: NonNull<NSEvent>| -> *mut NSEvent {
        monitor(unsafe { e.as_ref() });
        e.as_ptr()
    });
    let token = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &block) };
    std::mem::forget(token);
}

/// The frame (`FrameWindow::hwnd`) under screen point (x, y) (screen px), the topmost one.
pub fn row_window_at(x: i32, y: i32) -> Option<isize> {
    let mtm = MainThreadMarker::new()?;
    let owner = unsafe { view_of(OWNER.with(Cell::get)) }?;
    let p = from_px(mtm, (x, y), scale_of(owner));
    let n = NSWindow::windowNumberAtPoint_belowWindowWithWindowNumber(p, 0, mtm);
    let window = objc2_app_kit::NSApplication::sharedApplication(mtm).windowWithWindowNumber(n)?;
    frame_at(&window, window.convertPointFromScreen(p)).map(|f| Retained::as_ptr(&f.view) as isize)
}

// ---------------------------------------------------------------------------------------------------------------
// Lifted overlay, as on Windows: an app popup opened from a detached plugin window is painted in the main window, under
// the panel (it floats above the main window). While one is open, its pixels are shown in a small panel above the plugin
// window, and its mouse input goes to the app as if it happened in the main window (`take_lift_input`).

define_class!(
    // SAFETY: NSView has no subclassing requirements; no `Drop`.
    #[unsafe(super(NSView, NSResponder))]
    #[thread_kind = MainThreadOnly]
    #[name = "LemonStudioLiftedPopup"]
    struct LiftView;

    impl LiftView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _e: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(wantsUpdateLayer))]
        fn wants_update_layer(&self) -> bool {
            true
        }

        #[unsafe(method(updateLayer))]
        fn update_layer(&self) {
            LIFT.with(|l| {
                if let (Some(l), Some(layer)) = (l.borrow().as_ref(), self.layer()) {
                    unsafe { layer.setContents(l.image.as_ref().map(|i| i.as_ref() as &AnyObject)) };
                }
            });
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, e: &NSEvent) {
            lift_input(self, e, Some(LiftInput::Down(1)));
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, e: &NSEvent) {
            lift_input(self, e, Some(LiftInput::Down(3)));
        }

        #[unsafe(method(otherMouseDown:))]
        fn other_mouse_down(&self, e: &NSEvent) {
            lift_input(self, e, Some(LiftInput::Down(2)));
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, e: &NSEvent) {
            lift_input(self, e, Some(LiftInput::Up));
        }

        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, e: &NSEvent) {
            lift_input(self, e, Some(LiftInput::Up));
        }

        #[unsafe(method(otherMouseUp:))]
        fn other_mouse_up(&self, e: &NSEvent) {
            lift_input(self, e, Some(LiftInput::Up));
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, e: &NSEvent) {
            lift_input(self, e, None);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, e: &NSEvent) {
            lift_input(self, e, None);
        }
    }
);

struct Lift {
    panel: Retained<NSPanel>,
    view: Retained<LiftView>,
    /// Main-window position of the lifted rect's top-left (physical px), and the main window's scale.
    origin: (i32, i32),
    scale: CGFloat,
    image: Option<CFRetained<CGImage>>,
}

thread_local! {
    static LIFT: RefCell<Option<Lift>> = const { RefCell::new(None) };
    static LIFT_INPUT: RefCell<Vec<LiftInput>> = const { RefCell::new(Vec::new()) };
}

/// Queues a mouse event on the lifted popup for the app: where it is in the main window, then the button.
fn lift_input(view: &LiftView, e: &NSEvent, then: Option<LiftInput>) {
    let p = view.convertPoint_fromView(e.locationInWindow(), None);
    let Some(at) = LIFT.with(|l| l.borrow().as_ref().map(|l| LiftInput::Move(l.origin.0 + (p.x * l.scale).round() as i32, l.origin.1 + (p.y * l.scale).round() as i32))) else {
        return;
    };
    LIFT_INPUT.with(|q| q.borrow_mut().extend(Some(at).into_iter().chain(then)));
    os_clap::wake();
}

/// Shows `pixels` (`size`, XRGB) above the detached window, at `origin` in main window `main` (its view; physical px).
pub fn lift_show(main: isize, _frame: isize, origin: (i32, i32), size: (i32, i32), pixels: Vec<u32>) {
    let (Some(mtm), Some(owner)) = (MainThreadMarker::new(), unsafe { view_of(main) }) else { return };
    if size.0 <= 0 || size.1 <= 0 {
        return;
    }
    let k = scale_of(owner);
    let at = owner_to_screen(owner, NSPoint::new(f64::from(origin.0) / k, f64::from(origin.1) / k));
    let (w, h) = (f64::from(size.0) / k, f64::from(size.1) / k);
    let img = image(pixels, size.0 as usize, size.1 as usize);
    // The panel and view are taken out before AppKit is called: showing the panel draws it (`update_layer`), which reads
    // `LIFT` again.
    let shown = LIFT.with(|l| {
        let mut l = l.borrow_mut();
        if l.is_none() {
            let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
            let panel = NSPanel::initWithContentRect_styleMask_backing_defer(NSPanel::alloc(mtm), rect(at.x, at.y - h, w, h), style, NSBackingStoreType::Buffered, false);
            unsafe { panel.setReleasedWhenClosed(false) };
            panel.setLevel(LIFT_LEVEL);
            panel.setHidesOnDeactivate(true);
            panel.setHasShadow(false);
            let view: Retained<LiftView> = unsafe { msg_send![super(mtm.alloc::<LiftView>().set_ivars(())), initWithFrame: rect(0.0, 0.0, w, h)] };
            view.setWantsLayer(true);
            let opts = NSTrackingAreaOptions::MouseMoved | NSTrackingAreaOptions::ActiveAlways | NSTrackingAreaOptions::InVisibleRect;
            let track = unsafe { NSTrackingArea::initWithRect_options_owner_userInfo(NSTrackingArea::alloc(), NSRect::ZERO, opts, Some(track_owner(&view)), None) };
            view.addTrackingArea(&track);
            panel.setContentView(Some(&view));
            *l = Some(Lift { panel, view, origin, scale: k, image: None });
        }
        let lift = l.as_mut()?;
        lift.origin = origin;
        lift.scale = k;
        lift.image = img;
        Some((lift.panel.clone(), lift.view.clone()))
    });
    let Some((panel, view)) = shown else { return };
    panel.setFrame_display(rect(at.x, at.y - h, w, h), false);
    view.setNeedsDisplay(true);
    panel.orderFront(None);
}

/// The lifted popup closed.
pub fn lift_hide() {
    let l = LIFT.with(|l| l.borrow_mut().take());
    if let Some(l) = l {
        l.panel.orderOut(None);
        l.panel.close();
    }
}

/// Mouse input on the lifted popup since the last call (UI thread).
pub fn take_lift_input() -> Vec<LiftInput> {
    LIFT_INPUT.with(|q| std::mem::take(&mut *q.borrow_mut()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Off the main thread (where tests run) there are no plugin windows, and asking says so.
    #[test]
    fn frames_need_the_main_thread() {
        let info = ChannelInfo { name: "Keys".into(), target: "Insert 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None };
        if MainThreadMarker::new().is_none() {
            assert!(FrameWindow::new(0, info, (480.0, 270.0), 2.0, (0, 0), false).is_err());
            assert_eq!(screen_to_main(10, 10), None);
        }
    }
}
