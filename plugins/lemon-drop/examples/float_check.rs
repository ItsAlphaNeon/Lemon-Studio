// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! `cargo run -p lemon-drop --example float_check -- <dir> [--screen]` (macOS): Lemon Drop's real editor and floating
//! display in a real window, with a test signal, each feature checked from the outside: the panel's own settings, the
//! frames its layer holds (read back in this process), the state the mouse leaves behind. Plugin windows need the main
//! thread on macOS, which tests never get, so this is a program.
//!
//! Scripted events go through AppKit's own queue in this process, so the real cursor never moves and the keyboard focus
//! stays where it is. The application never activates; "active" is faked by replacing `-[NSApplication isActive]`.
//! Pictures go to `<dir>`. Exits 1 when a check failed.
//!
//! `--screen` also takes pictures of the composed screen with `screencapture`: over a coloured window, it shows that the
//! window server blends the frames. macOS may ask the user about it first, so it is off unless asked for.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    std::process::exit(check::run());
}

#[cfg(target_os = "macos")]
mod check {
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use lemon_drop::editor::{DropEditor, Logic, SeeThrough};
    use lemon_drop::state::{Mode, Rect4, State};
    use lemon_drop::view::{self, Ctl};
    use lemon_drop::{DropParams, Shared};
    use nih_plug::prelude::*;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, Bool, Imp, Sel};
    use objc2::{msg_send, sel, ClassType, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSEvent, NSEventMask, NSEventModifierFlags, NSEventType, NSScreen, NSView, NSWindow,
        NSWindowCollectionBehavior, NSWindowStyleMask,
    };
    use objc2_core_foundation::{CFData, CGPoint};
    use objc2_core_graphics::{CGDataProvider, CGEvent, CGEventType, CGImage, CGMouseButton, CGScrollEventUnit};
    use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSPoint, NSRect, NSSize, NSString};

    // ---------------------------------------------------------------------------------------------------------------
    // Hooks into AppKit from the outside

    static ACTIVE: AtomicBool = AtomicBool::new(true);

    extern "C-unwind" fn fake_is_active(_: *mut AnyObject, _: Sel) -> Bool {
        Bool::new(ACTIVE.load(Ordering::Relaxed))
    }

    /// Frames handed to each layer (`-[CALayer setContents:]`), by layer address.
    static FRAMES: Mutex<Option<HashMap<usize, usize>>> = Mutex::new(None);
    static REAL_SET_CONTENTS: Mutex<Option<Imp>> = Mutex::new(None);
    static PAINTS: AtomicUsize = AtomicUsize::new(0);

    extern "C-unwind" fn counting_set_contents(layer: *mut AnyObject, s: Sel, contents: *mut AnyObject) {
        if let Ok(mut f) = FRAMES.lock() {
            *f.get_or_insert_with(HashMap::new).entry(layer as usize).or_default() += 1;
        }
        PAINTS.fetch_add(1, Ordering::Relaxed);
        let real = REAL_SET_CONTENTS.lock().ok().and_then(|r| *r);
        if let Some(real) = real {
            // SAFETY: the real implementation of this method, which has this signature.
            let real: extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject) = unsafe { std::mem::transmute(real) };
            real(layer, s, contents);
        }
    }

    fn frames_of(layer: usize) -> usize {
        FRAMES.lock().ok().and_then(|f| f.as_ref().and_then(|m| m.get(&layer).copied())).unwrap_or(0)
    }

    fn install_hooks() {
        let class = NSApplication::class();
        // SAFETY: same signature as the method replaced.
        unsafe {
            class.instance_method(sel!(isActive)).unwrap().set_implementation(std::mem::transmute::<extern "C-unwind" fn(*mut AnyObject, Sel) -> Bool, Imp>(fake_is_active));
            let layer = objc2_quartz_core::CALayer::class();
            let real = layer.instance_method(sel!(setContents:)).unwrap().set_implementation(std::mem::transmute::<extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject), Imp>(counting_set_contents));
            *REAL_SET_CONTENTS.lock().unwrap() = Some(real);
        }
    }

    // ---------------------------------------------------------------------------------------------------------------
    // A host, as far as the editor cares

    struct Ctx;

    impl GuiContext for Ctx {
        fn plugin_api(&self) -> PluginApi {
            PluginApi::Clap
        }
        fn request_resize(&self) -> bool {
            false
        }
        unsafe fn raw_begin_set_parameter(&self, _: ParamPtr) {}
        unsafe fn raw_set_parameter_normalized(&self, _: ParamPtr, _: f32) {}
        unsafe fn raw_end_set_parameter(&self, _: ParamPtr) {}
        fn get_state(&self) -> PluginState {
            unimplemented!()
        }
        fn set_state(&self, _: PluginState) {}
    }

    /// The audio thread: a stereo signal into the scope.
    fn feed(shared: Arc<Shared>, stop: Arc<AtomicBool>) {
        let mut n = 0u64;
        while !stop.load(Ordering::Relaxed) {
            if let Ok(mut s) = shared.scope.lock() {
                let len = s.l.len();
                for _ in 0..441 {
                    let t = n as f32 / 44100.0;
                    let v = 0.45 * (t * 220.0 * std::f32::consts::TAU).sin() + 0.15 * (t * 1760.0 * std::f32::consts::TAU).sin();
                    let p = s.pos;
                    s.l[p] = v;
                    s.r[p] = 0.45 * (t * 220.0 * std::f32::consts::TAU).cos();
                    s.pos = (p + 1) % len;
                    s.written += 1;
                    n += 1;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    // ---------------------------------------------------------------------------------------------------------------
    // The run

    struct Run {
        mtm: MainThreadMarker,
        app: Retained<NSApplication>,
        params: Arc<DropParams>,
        dir: PathBuf,
        screen: bool,
        failed: usize,
        passed: usize,
    }

    /// Bottom-left AppKit rect from the main screen's top-left rectangle.
    fn height(mtm: MainThreadMarker) -> f64 {
        NSScreen::screens(mtm).firstObject().map_or(0.0, |s| s.frame().size.height)
    }

    fn rect4(mtm: MainThreadMarker, f: NSRect) -> Rect4 {
        let t = (height(mtm) - f.origin.y - f.size.height).round() as i32;
        let l = f.origin.x.round() as i32;
        [l, t, l + f.size.width.round() as i32, t + f.size.height.round() as i32]
    }

    impl Run {
        fn check(&mut self, ok: bool, what: &str) {
            println!("{} {what}", if ok { "  ok  " } else { " FAIL " });
            if ok {
                self.passed += 1;
            } else {
                self.failed += 1;
            }
        }

        /// Runs AppKit's queue and run loop for `ms`.
        fn pump(&self, ms: u64) {
            let until = NSDate::dateWithTimeIntervalSinceNow(ms as f64 / 1000.0);
            // SAFETY: a static run loop mode.
            let mode = unsafe { NSDefaultRunLoopMode };
            loop {
                let more = objc2::rc::autoreleasepool(|_| match self.app.nextEventMatchingMask_untilDate_inMode_dequeue(NSEventMask::Any, Some(&until), mode, true) {
                    Some(e) => {
                        self.app.sendEvent(&e);
                        true
                    }
                    None => false,
                });
                if !more {
                    break;
                }
            }
        }

        /// Changes the state as the host or the other display would.
        fn edit(&self, f: impl FnOnce(&mut State)) {
            let (v, mut st) = self.params.current();
            f(&mut st);
            assert!(self.params.edited(v, &st));
        }

        /// The floating window: the app's one non-opaque panel on the screen (one closed may be alive until its last
        /// reference goes).
        fn float(&self) -> Option<Retained<NSWindow>> {
            self.app.windows().iter().find(|w| w.isVisible() && !w.isOpaque() && w.styleMask().contains(NSWindowStyleMask::NonactivatingPanel))
        }

        fn floats(&self) -> usize {
            self.app.windows().iter().filter(|w| w.isVisible() && !w.isOpaque() && w.styleMask().contains(NSWindowStyleMask::NonactivatingPanel)).count()
        }

        /// An open popup menu: a panel of ours at the popup level.
        fn popup(&self) -> Option<Retained<NSWindow>> {
            self.app.windows().iter().find(|w| w.level() == 101 && w.isVisible())
        }

        /// A mouse event at screen point (`x`, `y`; points from the main screen's top left) for `w`.
        fn mouse(&self, w: &NSWindow, ty: NSEventType, x: f64, y: f64, flags: NSEventModifierFlags, clicks: isize) -> Retained<NSEvent> {
            let f = w.frame();
            let p = NSPoint::new(x - f.origin.x, height(self.mtm) - y - f.origin.y);
            NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(ty, p, flags, 0.0, w.windowNumber(), None, 0, clicks, 1.0).unwrap()
        }

        /// A press or release of the middle button: only a CG event says which button it is.
        fn middle(&self, x: f64, y: f64, down: bool) {
            let ty = if down { CGEventType::OtherMouseDown } else { CGEventType::OtherMouseUp };
            let cg = CGEvent::new_mouse_event(None, ty, CGPoint::new(x, y), CGMouseButton::Center).unwrap();
            let e = NSEvent::eventWithCGEvent(&cg).unwrap();
            assert_eq!(e.buttonNumber(), 2);
            // Straight to the view under it (a CG event not from the window server names no window).
            let view = self.float().and_then(|f| f.contentView()).unwrap();
            // SAFETY: the view handles these.
            unsafe {
                if down {
                    let _: () = msg_send![&*view, otherMouseDown: &*e];
                } else {
                    let _: () = msg_send![&*view, otherMouseUp: &*e];
                }
            }
        }

        fn post(&self, w: &NSWindow, ty: NSEventType, x: f64, y: f64) {
            let clicks = isize::from(!matches!(ty, NSEventType::MouseMoved));
            self.app.postEvent_atStart(&self.mouse(w, ty, x, y, NSEventModifierFlags::empty(), clicks), false);
        }

        /// A click at a control of the editor in the host's window `w`; `clicks` 2: the second press of a double click.
        fn click_ctl(&self, w: &NSWindow, c: Ctl, clicks: isize) {
            let st = self.params.current().1;
            let r = view::layout(&st).into_iter().find(|(k, _)| *k == c).unwrap().1;
            let f = w.frame();
            let (x, y) = (f.origin.x + f64::from(r.cx()), height(self.mtm) - f.origin.y - f.size.height + f64::from(r.cy()));
            self.app.postEvent_atStart(&self.mouse(w, NSEventType::LeftMouseDown, x, y, NSEventModifierFlags::empty(), clicks), false);
            self.pump(60);
            self.app.postEvent_atStart(&self.mouse(w, NSEventType::LeftMouseUp, x, y, NSEventModifierFlags::empty(), clicks), false);
            self.pump(80);
        }

        /// A wheel notch up over a control of the editor (its view handles it directly).
        fn wheel_ctl(&self, w: &NSWindow, view: &NSView, c: Ctl, notches: i32) {
            let st = self.params.current().1;
            let r = view::layout(&st).into_iter().find(|(k, _)| *k == c).unwrap().1;
            let f = w.frame();
            let cg = CGEvent::new_scroll_wheel_event2(None, CGScrollEventUnit::Line, 1, notches, 0, 0).unwrap();
            // A CG event from no window has its place in the window as its place on the screen.
            CGEvent::set_location(Some(&cg), CGPoint::new(f64::from(r.cx()), height(self.mtm) - (f.size.height - f64::from(r.cy()))));
            let e = NSEvent::eventWithCGEvent(&cg).unwrap();
            // SAFETY: the view handles it.
            let _: () = unsafe { msg_send![view, scrollWheel: &*e] };
            self.pump(80);
        }

        /// Types `text` into an open type-in box, then Return.
        fn type_text(&self, text: &str) {
            for c in text.chars() {
                self.key(0, &c.to_string());
                self.pump(15);
            }
            self.key(36, "\r");
            self.pump(150);
        }

        /// The pointer moves over `w`'s content view (what its tracking area reports).
        fn hover(&self, w: &NSWindow, view: &NSView, x: f64, y: f64) {
            let e = self.mouse(w, NSEventType::MouseMoved, x, y, NSEventModifierFlags::empty(), 0);
            // SAFETY: the view handles `mouseMoved:`.
            let _: () = unsafe { msg_send![view, mouseMoved: &*e] };
        }

        fn key(&self, code: u16, chars: &str) {
            let s = NSString::from_str(chars);
            let e = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
                NSEventType::KeyDown,
                NSPoint::new(0.0, 0.0),
                NSEventModifierFlags::empty(),
                0.0,
                self.popup().map_or(0, |w| w.windowNumber()),
                None,
                &s,
                &s,
                false,
                code,
            )
            .unwrap();
            self.app.postEvent_atStart(&e, false);
        }

        /// A window's picture with its alpha: the frame its layer holds, as the window server gets it. Also kept as a PNG.
        fn shoot(&self, w: &NSWindow, name: &str) -> Option<tiny_skia::Pixmap> {
            self.shoot_view(&*w.contentView()?, name)
        }

        /// A view's picture (see `shoot`). Beside the PNG, another with `_over` in its name: the picture blended over
        /// magenta as the window server blends it, to look at (a PNG viewer's white hides white labels).
        fn shoot_view(&self, view: &NSView, name: &str) -> Option<tiny_skia::Pixmap> {
            let layer = view.layer()?;
            // SAFETY: a plain read; the plugin kit gives its layers CGImages.
            let contents = unsafe { layer.contents() }?;
            let image = unsafe { &*(Retained::as_ptr(&contents).cast::<CGImage>()) };
            let (pw, ph) = (CGImage::width(Some(image)), CGImage::height(Some(image)));
            let data = CGDataProvider::data(CGImage::data_provider(Some(image)).as_deref())?;
            // SAFETY: the image's own bytes, alive with `data`.
            let bytes = unsafe { std::slice::from_raw_parts(CFData::byte_ptr(&data), CFData::length(&data) as usize) };
            let mut p = tiny_skia::Pixmap::new(pw as u32, ph as u32)?;
            // Little-endian premultiplied ARGB words are B G R A in memory (an opaque view's frames skip the A).
            let opaque = view.isOpaque();
            for (d, c) in p.data_mut().chunks_exact_mut(4).zip(bytes.chunks_exact(4)) {
                d.copy_from_slice(&[c[2], c[1], c[0], if opaque { 255 } else { c[3] }]);
            }
            p.save_png(self.dir.join(format!("{name}.png"))).ok()?;
            let mut over = p.clone();
            for c in over.data_mut().chunks_exact_mut(4) {
                let k = 255 - u32::from(c[3]);
                c[0] = (u32::from(c[0]) + k).min(255) as u8;
                c[2] = (u32::from(c[2]) + k).min(255) as u8;
                c[3] = 255;
            }
            over.save_png(self.dir.join(format!("{name}_over.png"))).ok()?;
            Some(p)
        }

        /// What is on the screen in `r`, everything composed (only with `--screen`: macOS may ask the user first).
        fn shoot_screen(&self, r: Rect4, name: &str) -> Option<tiny_skia::Pixmap> {
            if !self.screen {
                return None;
            }
            let path = self.dir.join(format!("{name}.png"));
            let region = format!("-R{},{},{},{}", r[0], r[1], r[2] - r[0], r[3] - r[1]);
            let ok = std::process::Command::new("screencapture").arg("-x").arg(region).arg(&path).status().is_ok_and(|s| s.success());
            if !ok {
                return None;
            }
            tiny_skia::Pixmap::load_png(&path).ok()
        }
    }

    /// CPU time this process has used, seconds.
    fn cpu() -> f32 {
        // SAFETY: a plain query into a local.
        unsafe {
            let mut t = std::mem::zeroed::<Timespec>();
            clock_gettime(12, &mut t); // CLOCK_PROCESS_CPUTIME_ID
            t.sec as f32 + t.nsec as f32 / 1e9
        }
    }

    #[repr(C)]
    struct Timespec {
        sec: i64,
        nsec: i64,
    }

    unsafe extern "C" {
        fn clock_gettime(clock: i32, t: *mut Timespec) -> i32;
    }

    /// A pixel's (r, g, b, a) straight, at a fraction across the picture.
    fn px(p: &tiny_skia::Pixmap, fx: f32, fy: f32) -> (u8, u8, u8, u8) {
        let (x, y) = (((p.width() - 1) as f32 * fx) as u32, ((p.height() - 1) as f32 * fy) as u32);
        let c = p.pixel(x, y).unwrap().demultiply();
        (c.red(), c.green(), c.blue(), c.alpha())
    }

    fn differs(a: &tiny_skia::Pixmap, b: &tiny_skia::Pixmap) -> bool {
        a.data() != b.data()
    }

    /// A plain window of a colour on the screen, behind the display: what it should show through.
    fn backdrop(mtm: MainThreadMarker, r: Rect4) -> Retained<NSWindow> {
        let h = height(mtm);
        let rect = NSRect::new(NSPoint::new(f64::from(r[0]), h - f64::from(r[3])), NSSize::new(f64::from(r[2] - r[0]), f64::from(r[3] - r[1])));
        // SAFETY: a plain window, kept by the caller until the end.
        let w = unsafe { NSWindow::initWithContentRect_styleMask_backing_defer(NSWindow::alloc(mtm), rect, NSWindowStyleMask::Borderless, NSBackingStoreType::Buffered, false) };
        // SAFETY: as above.
        unsafe { w.setReleasedWhenClosed(false) };
        w.setBackgroundColor(Some(&NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 0.0, 1.0, 1.0)));
        w.setLevel(0);
        w
    }

    pub fn run() -> i32 {
        let dir = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "/tmp/float_check".into()));
        let screen = std::env::args().any(|a| a == "--screen");
        let _ = std::fs::create_dir_all(&dir);
        let mtm = MainThreadMarker::new().expect("the main thread");
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        app.finishLaunching();
        install_hooks();

        // The host's window, with the editor in it.
        let frame = NSRect::new(NSPoint::new(40.0, 120.0), NSSize::new(f64::from(lemon_drop::view::W), f64::from(lemon_drop::view::H)));
        // As Lemon Studio's main window is (winit's `with_decorations(false)`): it draws its own title bar.
        let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::Resizable | NSWindowStyleMask::Miniaturizable;
        // SAFETY: a plain window, kept until the end.
        let host = unsafe { NSWindow::initWithContentRect_styleMask_backing_defer(NSWindow::alloc(mtm), frame, style, NSBackingStoreType::Buffered, false) };
        // SAFETY: as above.
        unsafe { host.setReleasedWhenClosed(false) };
        host.setTitle(&NSString::from_str("Lemon Drop float check (test window)"));
        host.orderFrontRegardless();
        let content = host.contentView().unwrap();

        let params = Arc::new(DropParams::default());
        let shared = Arc::new(Shared::default());
        let mut st = State::lemon_default();
        st.window = [520, 180, 821, 305];
        params.set_state(st.to_bytes());
        let stop = Arc::new(AtomicBool::new(false));
        {
            let (s, st) = (shared.clone(), stop.clone());
            std::thread::spawn(move || feed(s, st));
        }

        let mut r = Run { mtm, app, params: params.clone(), dir: dir.clone(), screen, failed: 0, passed: 0 };

        println!("== Opening");
        r.check(r.floats() == 0, "no floating window before the editor opens");
        let editor = DropEditor::new(params.clone(), shared.clone());
        let parent: *const NSView = &*content;
        let handle = editor.spawn(ParentWindowHandle::AppKitNsView(parent as *mut c_void), Arc::new(Ctx));
        r.pump(500);
        let Some(float) = r.float() else {
            r.check(false, "the editor's tick opens the floating window");
            return 1;
        };
        r.check(true, "the editor's tick opens the floating window");
        r.pump(300);
        r.check(r.floats() == 1, "exactly one floating window after more ticks");
        r.check(rect4(mtm, float.frame()) == [520, 180, 821, 305], &format!("at the state's rectangle: {:?}", rect4(mtm, float.frame())));
        r.check(!float.isOpaque() && !float.hasShadow(), "clear and shadowless");
        r.check(float.isVisible(), "shown while the host is active");
        r.check(float.level() == 3, &format!("owned: floats above the host's windows (level {})", float.level()));
        r.check(!float.ignoresMouseEvents(), "Solid: takes the mouse");
        r.check(!float.canBecomeKeyWindow() && !float.canBecomeMainWindow(), "never takes the keyboard or activates");
        r.check(shared.viewers.load(Ordering::Relaxed) == 2, "two viewers feed the scope: the editor and the window");
        let editor_layer = content.subviews().firstObject().and_then(|v| v.layer()).map_or(0, |l| Retained::as_ptr(&l) as usize);
        let float_view = float.contentView().unwrap();
        let float_layer = float_view.layer().map_or(0, |l| Retained::as_ptr(&l) as usize);

        println!("== See-through");
        let plain = r.shoot(&float, "default_window").expect("a window picture");
        let (_, _, _, corner) = px(&plain, 0.002, 0.01);
        let (_, _, _, middle) = px(&plain, 0.5, 0.5);
        r.check(corner < 8, &format!("the rounded corner outside the glass is see-through (alpha {corner})"));
        r.check(middle > 100, &format!("the glass paints (alpha {middle})"));
        let (w0, h0) = (plain.width(), plain.height());
        r.check((w0, h0) == (602, 250) || (w0, h0) == (301, 125), &format!("the picture is {w0} x {h0} px"));

        // Over a magenta backdrop, Background off: what the window server composes.
        let area = [500, 160, 841, 325];
        let back = backdrop(mtm, area);
        back.orderFrontRegardless();
        float.orderFrontRegardless();
        r.edit(|s| s.background = false);
        r.pump(400);
        if let Some(composed) = r.shoot_screen(area, "over_backdrop_no_background") {
            let (rr, gg, bb, _) = px(&composed, 0.12, 0.2);
            r.check(rr > 200 && gg < 60 && bb > 200, &format!("Background off: the backdrop shows through the display ({rr},{gg},{bb})"));
        }
        let alone = r.shoot(&float, "no_background_window").expect("a window picture");
        let opaque: usize = alone.data().chunks_exact(4).filter(|p| p[3] > 128).count();
        let clear: usize = alone.data().chunks_exact(4).filter(|p| p[3] < 8).count();
        let total = alone.data().len() / 4;
        r.check(opaque > 50 && opaque < total / 2, &format!("Background off: the picture still paints ({opaque} of {total} px)"));
        r.check(clear > total / 2, &format!("Background off: most of the window is clear ({clear} of {total} px)"));
        let later = {
            r.pump(120);
            r.shoot(&float, "no_background_later").expect("a window picture")
        };
        r.check(differs(&alone, &later), "the picture moves with the signal");
        r.edit(|s| s.background = true);
        r.pump(200);
        if let Some(composed) = r.shoot_screen(area, "over_backdrop_with_background") {
            let (rr, gg, bb, _) = px(&composed, 0.5, 0.5);
            r.check((rr, gg, bb) != (255, 0, 255), &format!("Background on: the glass over the backdrop ({rr},{gg},{bb})"));
        }

        println!("== Frame rate");
        let (f0, e0, t0, c0) = (frames_of(float_layer), frames_of(editor_layer), Instant::now(), cpu());
        r.pump(2000);
        let secs = t0.elapsed().as_secs_f32();
        let (ffps, efps) = ((frames_of(float_layer) - f0) as f32 / secs, (frames_of(editor_layer) - e0) as f32 / secs);
        println!("       floating window {ffps:.1} frames/s, editor {efps:.1} frames/s, {:.0} % of a core", (cpu() - c0) / secs * 100.0);
        r.check(ffps > 40.0, &format!("the floating display animates smoothly ({ffps:.0} frames/s)"));
        r.check(efps > 20.0, &format!("the editor animates ({efps:.0} frames/s)"));

        // The editor with the mouse moving over it (the PR's open question).
        let editor_view = content.subviews().firstObject().unwrap();
        let (e0, t0, c0) = (frames_of(editor_layer), Instant::now(), cpu());
        let fr = host.frame();
        let (ox, oy) = (fr.origin.x, height(mtm) - fr.origin.y - fr.size.height);
        let mut i = 0.0f64;
        while t0.elapsed() < Duration::from_secs(2) {
            r.hover(&host, &editor_view, ox + 60.0 + (i * 3.0) % 250.0, oy + 80.0 + (i * 2.0) % 60.0);
            i += 1.0;
            r.pump(4);
        }
        let secs = t0.elapsed().as_secs_f32();
        let moves = i as f32 / secs;
        let efps = (frames_of(editor_layer) - e0) as f32 / secs;
        println!("       editor under {moves:.0} mouse moves/s: {efps:.1} frames/s, {:.0} % of a core", (cpu() - c0) / secs * 100.0);
        r.check(efps > 20.0, &format!("the editor keeps animating with the mouse over it ({efps:.0} frames/s)"));
        r.check(efps < 90.0, &format!("the mouse does not make it draw more than its tick ({efps:.0} frames/s)"));

        // What a frame costs.
        {
            let mut l = Logic::new(params.clone(), shared.clone());
            let (mut c, area) = (os_ui::Canvas::new(602, 250, 2.0).unwrap(), os_ui::Rect::new(0.0, 0.0, 301.0, 125.0));
            let (mut b, mut out) = (SeeThrough::default(), vec![0u32; 602 * 250]);
            let t = Instant::now();
            for _ in 0..50 {
                l.tick(0.015);
                l.paint_see_through(&mut c, area, &mut b, &mut out);
            }
            println!("       a see-through frame at 2x: {:.2} ms", t.elapsed().as_secs_f32() * 1000.0 / 50.0);
        }

        println!("== Moving and sizing with the mouse");
        let [l, t, rr, bb] = rect4(mtm, float.frame()).map(f64::from);
        let (cx, cy) = ((l + rr) / 2.0, (t + bb) / 2.0);
        r.post(&float, NSEventType::LeftMouseDown, cx, cy);
        r.pump(60);
        for k in 1..=20 {
            let k = f64::from(k);
            r.post(&float, NSEventType::LeftMouseDragged, cx + k * 3.0, cy + k * 2.0);
            r.pump(16);
        }
        r.pump(60);
        r.check(rect4(mtm, float.frame()) == [580, 220, 881, 345], &format!("a drag on the body moves it: {:?}", rect4(mtm, float.frame())));
        r.check(params.current().1.window == [520, 180, 821, 305], "the state keeps the old rectangle until the drop");
        r.post(&float, NSEventType::LeftMouseUp, cx + 60.0, cy + 40.0);
        r.pump(100);
        r.check(params.current().1.window == [580, 220, 881, 345], &format!("the drop stores it: {:?}", params.current().1.window));
        r.check(rect4(mtm, float.frame()) == [580, 220, 881, 345], "the window stays where it was dropped");

        let (gx, gy) = (881.0 - 5.0, 345.0 - 5.0);
        r.post(&float, NSEventType::LeftMouseDown, gx, gy);
        r.pump(60);
        for k in 1..=10 {
            let k = f64::from(k);
            r.post(&float, NSEventType::LeftMouseDragged, gx + k * 10.0, gy + k * 6.0);
            r.pump(16);
        }
        r.pump(100);
        r.check(rect4(mtm, float.frame()) == [580, 220, 981, 405], &format!("a drag on the lower right corner sizes it: {:?}", rect4(mtm, float.frame())));
        let big = r.shoot(&float, "sized").expect("a window picture");
        r.check(big.width() == 802 && big.height() == 370, &format!("the picture follows at the window's size: {} x {}", big.width(), big.height()));
        // Too small: held at the least size, from the same top left.
        r.post(&float, NSEventType::LeftMouseDragged, gx - 1000.0, gy - 1000.0);
        r.pump(100);
        r.check(rect4(mtm, float.frame()) == [580, 220, 604, 244], &format!("it stops at 24 points a side: {:?}", rect4(mtm, float.frame())));
        r.post(&float, NSEventType::LeftMouseDragged, gx + 100.0, gy + 60.0);
        r.pump(60);
        r.post(&float, NSEventType::LeftMouseUp, gx + 100.0, gy + 60.0);
        r.pump(100);
        r.check(params.current().1.window == [580, 220, 981, 405], &format!("the new size goes into the state: {:?}", params.current().1.window));
        r.pump(80);
        let edited_elsewhere = {
            let (v, mut st) = params.current();
            st.gain = 7;
            params.edited(v, &st)
        };
        r.pump(80);
        r.check(edited_elsewhere && params.current().1.window == [580, 220, 981, 405] && params.current().1.gain == 7, "another display's edit keeps the rectangle, and the other way round");

        // The rectangle from outside: a preset or a project moves the window.
        r.edit(|s| s.window = [520, 180, 821, 305]);
        r.pump(150);
        r.check(rect4(mtm, float.frame()) == [520, 180, 821, 305], "a rectangle changed from outside moves and sizes the window");

        println!("== Menu and hold");
        let (cx, cy) = (670.0, 240.0);
        let before = r.shoot(&float, "before_menu").expect("a window picture");
        r.post(&float, NSEventType::RightMouseDown, cx, cy);
        r.pump(200);
        let popup = r.popup();
        r.check(popup.is_some(), "a right click opens the display menu");
        if let Some(p) = &popup {
            r.check(p.level() > float.level(), "the menu is above the floating window");
            let f = rect4(mtm, p.frame());
            r.shoot_screen([f[0] - 10, f[1] - 10, f[2] + 10, f[3] + 10], "menu");
        }
        r.post(&float, NSEventType::RightMouseUp, cx, cy);
        r.pump(100);
        r.check(r.popup().is_some(), "the release of that click leaves it open");
        let a = r.shoot(&float, "under_menu_a").expect("a window picture");
        r.pump(150);
        let b = r.shoot(&float, "under_menu_b").expect("a window picture");
        r.check(differs(&a, &b) && differs(&before, &a), "the picture keeps moving under the menu");
        // Down to Background (Visible, Mini view, Background), Return.
        for _ in 0..3 {
            r.key(125, "\u{f701}");
            r.pump(40);
        }
        r.key(36, "\r");
        r.pump(200);
        r.check(r.popup().is_none(), "picking closes the menu");
        r.check(!params.current().1.background, "the menu's Background edits the state");
        r.pump(60);
        r.edit(|s| s.background = true);
        r.pump(60);
        r.post(&float, NSEventType::RightMouseDown, cx, cy);
        r.pump(150);
        r.key(53, "\u{1b}");
        r.pump(150);
        r.check(r.popup().is_none(), "Escape closes the menu");
        // A click elsewhere: the host's window.
        r.post(&float, NSEventType::RightMouseDown, cx, cy);
        r.pump(150);
        r.post(&host, NSEventType::LeftMouseDown, ox + 30.0, oy + 30.0);
        r.pump(150);
        r.post(&host, NSEventType::LeftMouseUp, ox + 30.0, oy + 30.0);
        r.pump(150);
        r.check(r.popup().is_none(), "a click on the host's window closes the menu");
        // A click on the floating window itself: it closes the menu and does not start a drag.
        let at = rect4(mtm, float.frame());
        r.post(&float, NSEventType::RightMouseDown, cx, cy);
        r.pump(150);
        r.post(&float, NSEventType::LeftMouseDown, cx, cy);
        r.pump(100);
        r.post(&float, NSEventType::LeftMouseDragged, cx + 50.0, cy + 50.0);
        r.pump(100);
        r.post(&float, NSEventType::LeftMouseUp, cx + 50.0, cy + 50.0);
        r.pump(100);
        r.check(r.popup().is_none() && rect4(mtm, float.frame()) == at, "a click on the window closes its menu without moving it");

        // A middle click holds the picture.
        r.middle(cx, cy, true);
        r.pump(100);
        r.middle(cx, cy, false);
        r.pump(200);
        let held_a = r.shoot(&float, "held_a").expect("a window picture");
        r.pump(200);
        let held_b = r.shoot(&float, "held_b").expect("a window picture");
        r.check(!differs(&held_a, &held_b), "a middle click holds the picture");
        let f0 = frames_of(float_layer);
        r.pump(300);
        r.check(frames_of(float_layer) == f0, "an unchanged picture is not sent to the window server again");
        r.middle(cx, cy, true);
        r.pump(100);
        r.middle(cx, cy, false);
        r.pump(200);
        let free = r.shoot(&float, "held_c").expect("a window picture");
        r.check(differs(&held_b, &free), "another frees it");

        // The reading under the pointer.
        r.hover(&float, &float_view, 600.0, 240.0);
        r.pump(100);
        r.shoot(&float, "pointer_reading");

        println!("== Keep in front, On desktop, Solid, the host going away");
        ACTIVE.store(false, Ordering::Relaxed);
        r.pump(120);
        r.check(!float.isVisible(), "owned: gone while another application is active");
        r.check(!float.isMiniaturized(), "and it is only hidden");
        ACTIVE.store(true, Ordering::Relaxed);
        r.pump(120);
        r.check(float.isVisible(), "owned: back when the host is active again");

        r.edit(|s| s.keep_in_front = true);
        r.pump(120);
        r.check(float.level() == 4, &format!("Keep in front: above the floating level (level {})", float.level()));
        ACTIVE.store(false, Ordering::Relaxed);
        r.pump(120);
        r.check(float.isVisible(), "Keep in front: stays while another application is active");
        r.check(float.collectionBehavior().contains(NSWindowCollectionBehavior::CanJoinAllSpaces), "Keep in front: on every Space");
        r.edit(|s| s.keep_in_front = false);
        r.pump(120);
        r.check(!float.isVisible(), "Keep in front off while inactive: gone at once");
        ACTIVE.store(true, Ordering::Relaxed);
        r.pump(120);
        r.check(float.isVisible() && float.level() == 3, "and back when active");

        r.edit(|s| s.on_desktop = true);
        r.pump(120);
        r.check(float.level() == 0, &format!("On desktop: a window like others (level {})", float.level()));
        ACTIVE.store(false, Ordering::Relaxed);
        r.pump(120);
        r.check(float.isVisible(), "On desktop: stays while another application is active");
        r.check(!float.canHide(), "On desktop: stays when the host is hidden");
        ACTIVE.store(true, Ordering::Relaxed);
        host.miniaturize(None);
        r.pump(900);
        r.check(host.isMiniaturized(), "(the host's window is minimised)");
        r.check(float.isVisible(), "On desktop: stays when the host is minimised");
        r.edit(|s| s.on_desktop = false);
        r.pump(150);
        r.check(!float.isVisible(), "owned: gone while the host is minimised");
        host.deminiaturize(None);
        r.pump(900);
        r.check(float.isVisible(), "owned: back when the host's window is back");

        // The host hidden (Command-H).
        r.app.hide(None);
        r.pump(300);
        r.check(!float.isVisible(), "owned: gone while the host is hidden");
        r.app.unhideWithoutActivation();
        r.pump(300);
        r.check(float.isVisible(), "owned: back when the host is shown again");
        r.edit(|s| s.on_desktop = true);
        r.pump(120);
        r.app.hide(None);
        r.pump(300);
        r.check(float.isVisible(), "On desktop: stays while the host is hidden");
        r.app.unhideWithoutActivation();
        r.pump(300);
        r.edit(|s| s.on_desktop = false);
        r.pump(120);
        host.orderFrontRegardless();
        float.orderFrontRegardless();

        r.edit(|s| s.solid = false);
        r.pump(120);
        r.check(float.ignoresMouseEvents(), "Solid off: clicks go through");
        // At the display's left end (a system dialog can sit in the middle of the screen, above everything).
        let (ax, ay) = (540.0, height(mtm) - 240.0);
        let hit = NSWindow::windowNumberAtPoint_belowWindowWithWindowNumber(NSPoint::new(ax, ay), 0, mtm);
        r.check(hit != float.windowNumber(), &format!("Solid off: the window server's window at the point is another ({hit})"));
        r.edit(|s| s.solid = true);
        r.pump(120);
        r.check(!float.ignoresMouseEvents(), "Solid on: takes the mouse again");
        let t0 = Instant::now();
        let mut hit = 0;
        while t0.elapsed() < Duration::from_millis(1500) {
            hit = NSWindow::windowNumberAtPoint_belowWindowWithWindowNumber(NSPoint::new(ax, ay), 0, mtm);
            if hit == float.windowNumber() {
                break;
            }
            r.pump(20);
        }
        println!("       hit test settled after {:?}", t0.elapsed());
        r.check(hit == float.windowNumber(), &format!("Solid on: the window server's window at the point is the display ({hit}, the display is {})", float.windowNumber()));
        // With nothing painted: the fill's alpha of 1 still takes the mouse.
        r.edit(|s| {
            s.background = false;
            s.bevel = 0;
            s.mode = Mode::Meter;
        });
        r.pump(200);
        let hit = NSWindow::windowNumberAtPoint_belowWindowWithWindowNumber(NSPoint::new(525.0, height(mtm) - 185.0), 0, mtm);
        r.check(hit == float.windowNumber(), &format!("Solid on, Background off: a corner painted with nothing still takes the mouse ({hit})"));
        r.edit(|s| {
            s.background = true;
            s.mode = Mode::Oscilloscope;
        });

        println!("== Mini view and rectangles on no screen");
        r.edit(|s| {
            s.mini = true;
            s.mini_window = [40, 60, 87, 107];
        });
        r.pump(150);
        r.check(rect4(mtm, float.frame()) == [40, 60, 87, 107], &format!("Mini view: its own rectangle: {:?}", rect4(mtm, float.frame())));
        r.shoot(&float, "mini");
        r.edit(|s| s.mini = false);
        r.pump(150);
        r.check(rect4(mtm, float.frame()) == [520, 180, 821, 305], "Mini view off: back to the big one");
        r.edit(|s| s.window = [9000, 9000, 9301, 9125]);
        r.pump(150);
        let f = float.frame();
        let vis = NSScreen::mainScreen(mtm).unwrap().visibleFrame();
        let (mx, my) = (vis.origin.x + (vis.size.width - 301.0) / 2.0, vis.origin.y + (vis.size.height - 125.0) / 2.0);
        r.check((f.origin.x - mx).abs() < 1.5 && (f.origin.y - my).abs() < 1.5 && f.size.width == 301.0, &format!("a rectangle on no screen shows in the middle of the screen: {:?}", rect4(mtm, f)));
        r.check(params.current().1.window == [9000, 9000, 9301, 9125], "the state keeps it until it is moved");
        r.edit(|s| s.window = [520, 180, 821, 305]);
        r.pump(150);

        println!("== Presets over the desktop");
        back.orderFrontRegardless();
        float.orderFrontRegardless();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/os-app/factory-presets/Lemon Drop/Themes");
        let keep = params.current().1;
        for name in ["Scope over desktop", "Spectrum over desktop", "Vectorscope over desktop", "Meter over desktop", "Acrylic oscilloscope", "Frosted glass"] {
            let b = std::fs::read(root.join(format!("{name}.ospreset"))).unwrap();
            let blob = b.splitn(3, |&c| c == b'\n').nth(2).unwrap();
            let mut st = State::from_bytes(&lemon_drop::fl_from_plugin_state(blob).unwrap()).unwrap();
            // The presets' own rectangles and windows flags are the original's: shown here at the test's place.
            st.window = [520, 180, 821, 305];
            st.mini = false;
            st.visible = true;
            st.keep_in_front = false;
            st.on_desktop = false;
            params.set_state(st.to_bytes());
            r.pump(700);
            r.check(rect4(mtm, float.frame()) == [520, 180, 821, 305], &format!("preset {name}: the state replaced from outside is shown"));
            let f = name.replace(' ', "_");
            r.shoot_screen(area, &format!("preset_{f}"));
        }
        params.set_state(keep.to_bytes());
        r.pump(150);
        back.orderOut(None);

        println!("== The editor's controls");
        r.edit(|s| s.window = [520, 180, 821, 305]);
        r.pump(100);
        let float = r.float().unwrap();
        for (mode, name) in [(Mode::Oscilloscope, "oscilloscope"), (Mode::Spectrum, "spectrum"), (Mode::Meter, "meter"), (Mode::Vectorscope, "vectorscope")] {
            r.click_ctl(&host, Ctl::Tab(mode), 1);
            r.pump(500);
            r.check(params.current().1.mode == mode, &format!("the {name} tab edits the state"));
            let (editor_pic, float_pic) = (r.shoot_view(&editor_view, &format!("editor_{name}")), r.shoot(&float, &format!("float_{name}")));
            r.check(float_pic.is_some() && editor_pic.is_some(), &format!("{name}: both displays draw"));
        }
        // With Background off, by the editor's own toggle, seen by the floating window.
        r.click_ctl(&host, Ctl::Background, 1);
        r.check(!params.current().1.background, "the Background toggle edits the state");
        for (mode, name) in [(Mode::Oscilloscope, "oscilloscope"), (Mode::Spectrum, "spectrum"), (Mode::Meter, "meter"), (Mode::Vectorscope, "vectorscope")] {
            r.click_ctl(&host, Ctl::Tab(mode), 1);
            r.pump(500);
            r.shoot(&float, &format!("float_clear_{name}"));
        }
        r.click_ctl(&host, Ctl::Background, 1);
        r.check(params.current().1.background, "and back");
        for (c, get, name) in [
            (Ctl::Front, (|s: &State| s.keep_in_front) as fn(&State) -> bool, "Keep in front"),
            (Ctl::Solid, |s| s.solid, "Solid"),
            (Ctl::Desktop, |s| s.on_desktop, "On desktop"),
            (Ctl::Mini, |s| s.mini, "Mini view"),
        ] {
            let was = get(&params.current().1);
            r.click_ctl(&host, c, 1);
            r.check(get(&params.current().1) != was, &format!("the editor's {name} toggle edits the state"));
            r.click_ctl(&host, c, 1);
        }
        // A list opens the kit's menu; the keyboard goes to it though the application is not key.
        r.click_ctl(&host, Ctl::Tab(Mode::Meter), 1);
        r.click_ctl(&host, Ctl::MeterMode, 1);
        r.pump(150);
        r.check(r.popup().is_some(), "a list control opens its menu");
        if let Some(p) = r.popup() {
            r.shoot(&p, "editor_list_menu");
        }
        let was = params.current().1.meter_mode;
        r.key(125, "\u{f701}");
        r.pump(40);
        r.key(36, "\r");
        r.pump(200);
        r.check(params.current().1.meter_mode != was && r.popup().is_none(), &format!("picking in it edits the state ({was} to {})", params.current().1.meter_mode));
        // The title, typed into the box.
        r.click_ctl(&host, Ctl::Tab(Mode::Oscilloscope), 1);
        r.click_ctl(&host, Ctl::Title, 1);
        r.pump(150);
        r.check(r.popup().is_some(), "the title opens the type-in box");
        r.type_text("Float check");
        r.check(params.current().1.title == b"Float check" && r.popup().is_none(), &format!("typing a title edits the state: {:?}", String::from_utf8_lossy(&params.current().1.title)));
        r.pump(200);
        r.shoot(&float, "float_titled");
        // The wheel, and a double click back to the default.
        let fade = params.current().1.gain;
        r.wheel_ctl(&host, &editor_view, Ctl::Gain, 2);
        r.check(params.current().1.gain != fade, &format!("the wheel on a knob edits the state ({fade} to {})", params.current().1.gain));
        // (A single press would hold the real cursor for a knob drag, so knob drags are not run here: `Logic`'s tests
        // cover them, and every Lemon editor shares the held cursor.)
        r.click_ctl(&host, Ctl::Gain, 2);
        r.check(params.current().1.gain == view::value(&State::new_default(), Ctl::Gain, 0), &format!("a double click resets it ({})", params.current().1.gain));
        // The right button on the display: the editor's menu of the same display.
        r.pump(100);
        let d = view::DISPLAY;
        let f = host.frame();
        let (x, y) = (f.origin.x + f64::from(d.cx()), height(mtm) - f.origin.y - f.size.height + f64::from(d.cy()));
        r.post(&host, NSEventType::RightMouseDown, x, y);
        r.pump(200);
        r.check(r.popup().is_some(), "a right click on the editor's display opens the menu");
        r.key(53, "\u{1b}");
        r.pump(150);
        r.post(&host, NSEventType::RightMouseUp, x, y);
        r.pump(100);
        // The editor's Hold freezes its own picture, not the floating one's.
        r.click_ctl(&host, Ctl::Hold, 1);
        r.pump(150);
        let (ea, fa) = (r.shoot_view(&editor_view, "editor_held_a").unwrap(), r.shoot(&float, "float_not_held_a").unwrap());
        r.pump(200);
        let (eb, fb) = (r.shoot_view(&editor_view, "editor_held_b").unwrap(), r.shoot(&float, "float_not_held_b").unwrap());
        r.check(!differs(&ea, &eb), "Hold freezes the editor's display");
        r.check(differs(&fa, &fb), "and not the floating window's: each holds its own");
        r.click_ctl(&host, Ctl::Hold, 1);

        println!("== Going");
        r.edit(|s| s.visible = false);
        r.pump(200);
        r.check(r.floats() == 0, "Visible off closes the floating window");
        r.check(shared.viewers.load(Ordering::Relaxed) == 1, "only the editor still feeds on the scope");
        r.edit(|s| s.visible = true);
        r.pump(300);
        r.check(r.floats() == 1, "Visible on opens it again");
        let float = r.float().unwrap();
        // Visible off from its own menu: Return on the first row.
        let f = rect4(mtm, float.frame());
        let (cx, cy) = (f64::from(f[0] + f[2]) / 2.0, f64::from(f[1] + f[3]) / 2.0);
        r.post(&float, NSEventType::RightMouseDown, cx, cy);
        r.pump(150);
        r.key(125, "\u{f701}");
        r.pump(40);
        r.key(36, "\r");
        r.pump(300);
        r.check(r.floats() == 0 && !params.current().1.visible, "the menu's Visible closes it");
        r.edit(|s| s.visible = true);
        r.pump(300);
        r.check(r.floats() == 1, "and the editor's tick opens it again");
        let float = r.float().unwrap();
        let float_layer = float.contentView().and_then(|v| v.layer()).map_or(0, |l| Retained::as_ptr(&l) as usize);

        // The editor closes; the floating window stays and keeps animating, on its own.
        drop(handle);
        r.pump(300);
        r.check(r.floats() == 1, "it stays after the editor closes");
        r.check(shared.viewers.load(Ordering::Relaxed) == 1, "it alone feeds on the scope now");
        // Time to profile it from outside: FLOAT_CHECK_SOAK=<seconds>.
        if let Some(n) = std::env::var("FLOAT_CHECK_SOAK").ok().and_then(|v| v.parse::<u64>().ok()) {
            println!("       soaking {n} s, pid {}", std::process::id());
            r.pump(n * 1000);
        }
        let (f0, t0, c0) = (frames_of(float_layer), Instant::now(), cpu());
        r.pump(2000);
        let secs = t0.elapsed().as_secs_f32();
        println!("       floating window alone: {:.1} frames/s, {:.0} % of a core", (frames_of(float_layer) - f0) as f32 / secs, (cpu() - c0) / secs * 100.0);
        r.check(frames_of(float_layer) > f0 + 40, "it keeps animating without the editor");
        r.edit(|s| s.keep_in_front = true);
        r.pump(100);
        r.check(float.level() == 4, "it still reads the state");
        r.edit(|s| s.keep_in_front = false);

        // A silent signal: what it costs to wait.
        stop.store(true, Ordering::Relaxed);
        r.pump(300);
        let (f0, t0, c0) = (frames_of(float_layer), Instant::now(), cpu());
        r.pump(2000);
        let secs = t0.elapsed().as_secs_f32();
        println!("       floating window, no signal: {:.1} frames/s, {:.0} % of a core", (frames_of(float_layer) - f0) as f32 / secs, (cpu() - c0) / secs * 100.0);

        // The plugin goes.
        let t = Instant::now();
        lemon_drop::float::close(&shared);
        let took = t.elapsed();
        r.pump(100);
        r.check(r.floats() == 0, "the plugin going closes it");
        r.check(!float.isVisible(), "the panel is off the screen");
        r.check(shared.viewers.load(Ordering::Relaxed) == 0, "nothing feeds on the scope any more");
        r.check(took < Duration::from_millis(500), &format!("closing does not hang ({took:?})"));

        // With a menu open at the moment the plugin goes.
        let editor = DropEditor::new(params.clone(), shared.clone());
        let handle = editor.spawn(ParentWindowHandle::AppKitNsView(parent as *mut c_void), Arc::new(Ctx));
        r.pump(400);
        let float = r.float().unwrap();
        let f = rect4(mtm, float.frame());
        r.post(&float, NSEventType::RightMouseDown, f64::from(f[0] + 40), f64::from(f[1] + 40));
        r.pump(200);
        r.check(r.popup().is_some(), "(a menu is open)");
        drop(handle);
        let t = Instant::now();
        lemon_drop::float::close(&shared);
        let took = t.elapsed();
        r.pump(200);
        r.check(r.floats() == 0 && r.popup().is_none(), "the plugin going closes the window and its menu");
        r.check(took < Duration::from_millis(500), &format!("and does not hang ({took:?})"));

        host.orderOut(None);
        back.close();
        host.close();
        println!("\n{} passed, {} failed. Pictures in {}", r.passed, r.failed, dir.display());
        i32::from(r.failed > 0)
    }
}
