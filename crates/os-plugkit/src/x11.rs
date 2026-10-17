// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! X11 parts of a Lemon plugin editor (Linux; XWayland under Wayland): the editor's window, a child of the host's, run
//! on a GUI thread of its own with its own connection, as other plugin toolkits do on Linux (any host works, none has to
//! run the editor's loop); painting a canvas into it; the cursor held in place during a knob drag (XInput 2 raw
//! motion, else pointer motion with the pointer warped back); popup menus painted with `os_ui::kit::menu` (never native menus) and the "Type in value" box. The Win32
//! counterpart is `win`; `mac` has this API on macOS, and the input types are shared (`input`).

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use os_theme::{Color, Palette};
use os_ui::kit::menu as kmenu;
use os_ui::{Canvas, Font, Rect};
use x11rb::connection::{Connection, RequestConnection as _};
use x11rb::protocol::xinput::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    ConnectionExt as _, CreateGCAux, CreateWindowAux, EventMask, GrabMode, GrabStatus, ImageFormat, ImageOrder, KeyButMask, NotifyDetail,
    VisualClass, WindowClass,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

pub use crate::input::{keysym, keysym_char, Button, Input};
use crate::MenuDef;

/// The host's window: an X11 window id.
pub type Parent = u32;

/// The editor's polling period (overlay expiry, meters): the Win32 editors' timer.
const TICK: Duration = Duration::from_millis(30);
/// Two presses closer than this are a double click (ms of server time).
const DOUBLE_CLICK_MS: u32 = 400;

// -------------------------------------------------------------------------------------------------------------------
// The GUI thread

/// Wakes an editor's GUI thread from any thread: a parameter changed (look again) or the editor closes.
#[derive(Clone)]
pub struct Waker(Arc<Pipe>);

struct Pipe {
    rx: OwnedFd,
    tx: OwnedFd,
    quit: AtomicBool,
}

impl Waker {
    fn new() -> Option<Waker> {
        let mut fds = [0; 2];
        // SAFETY: pipe2 writes two new descriptors into `fds`.
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_NONBLOCK | libc::O_CLOEXEC) } != 0 {
            return None;
        }
        // SAFETY: both descriptors are fresh and owned here.
        let (rx, tx) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        Some(Waker(Arc::new(Pipe { rx, tx, quit: AtomicBool::new(false) })))
    }

    /// Asks the GUI thread to look again (a parameter changed). Any thread.
    pub fn wake(&self) {
        // SAFETY: one byte from a valid buffer into our own pipe; when it is full a wake is pending anyway.
        unsafe { libc::write(self.0.tx.as_raw_fd(), [1u8].as_ptr().cast(), 1) };
    }

    fn quit(&self) {
        self.0.quit.store(true, Ordering::Release);
        self.wake();
    }

    fn quitting(&self) -> bool {
        self.0.quit.load(Ordering::Acquire)
    }

    fn drain(&self) {
        let mut b = [0u8; 64];
        // SAFETY: reads into a valid buffer from our own non-blocking pipe.
        while unsafe { libc::read(self.0.rx.as_raw_fd(), b.as_mut_ptr().cast(), b.len()) } > 0 {}
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

/// The editor's GUI thread. Dropping it closes the window and ends the thread.
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
    }
}

/// Opens an editor window of `size` device px in the host's X11 window `parent` and runs `run` with it on a thread of
/// its own. `None` when there is no X11 display.
pub fn spawn(parent: Parent, size: (u32, u32), slot: &Slot, run: impl FnOnce(Window) + Send + 'static) -> Option<EditorThread> {
    let waker = Waker::new()?;
    let win = match Window::open(parent, size, waker.clone()) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("plugin editor: {e}");
            return None;
        }
    };
    if let Ok(mut s) = slot.lock() {
        *s = Some(waker.clone());
    }
    let thread = std::thread::Builder::new().name("plugin editor".into()).spawn(move || run(win)).ok()?;
    Some(EditorThread { waker, slot: slot.clone(), thread: Some(thread) })
}

// -------------------------------------------------------------------------------------------------------------------
// The window

/// Key symbols by key code (the server's keyboard mapping).
struct Keymap {
    min: u8,
    per: u8,
    syms: Vec<u32>,
}

impl Keymap {
    fn load(c: &RustConnection) -> Keymap {
        let s = c.setup();
        let (min, count) = (s.min_keycode, s.max_keycode.saturating_sub(s.min_keycode).saturating_add(1));
        let r = c.get_keyboard_mapping(min, count).ok().and_then(|r| r.reply().ok());
        r.map_or(Keymap { min, per: 0, syms: Vec::new() }, |r| Keymap { min, per: r.keysyms_per_keycode, syms: r.keysyms })
    }

    fn sym(&self, code: u8, state: u16) -> u32 {
        let base = usize::from(code.saturating_sub(self.min)) * usize::from(self.per);
        let at = |l: usize| if l < usize::from(self.per) { self.syms.get(base + l).copied().unwrap_or(0) } else { 0 };
        let (lo, hi) = (at(0), at(1));
        let shift = state & u16::from(KeyButMask::SHIFT) != 0;
        // Num Lock (Mod2) picks the keypad's digits.
        let keypad = (0xff80..=0xffbd).contains(&hi);
        if keypad && state & u16::from(KeyButMask::MOD2) != 0 {
            return if shift { lo } else { hi };
        }
        if shift && hi != 0 { hi } else { lo }
    }
}

/// An editor's X11 window and connection (owned by the GUI thread).
pub struct Window {
    c: RustConnection,
    pub win: u32,
    root: u32,
    root_size: (i32, i32),
    depth: u8,
    gc: u32,
    lsb_first: bool,
    max_request: usize,
    waker: Waker,
    keys: Keymap,
    /// An invisible cursor for held drags.
    blank: u32,
    /// XInput 2 is there: held drags ask for raw motion (XWayland may not send any).
    xi: bool,
    /// Raw motion arrived during this held drag: it drives the drag, not pointer motion.
    raw_seen: bool,
    /// Held drag by pointer motion: the last position seen, and a warp back to the anchor not echoed yet.
    prev: (i16, i16),
    warping: bool,
    /// The pointer on the screen and the modifier state at the last event.
    pointer: (i16, i16),
    state: u16,
    /// The last press (server time, button, position) for double clicks.
    last_press: Option<(u32, u8, i16, i16)>,
    /// Held cursor: where it is held (screen px).
    lock: Option<(i16, i16)>,
    next_tick: Instant,
    /// Inputs read but not handed out yet (`next` reads every pending event at once).
    queue: std::collections::VecDeque<Input>,
}

impl Window {
    fn open(parent: u32, (w, h): (u32, u32), waker: Waker) -> Result<Window, String> {
        let err = |e: &dyn std::fmt::Display| format!("no X11 display for the editor: {e}");
        let (c, screen) = RustConnection::connect(None).map_err(|e| err(&e))?;
        let setup = c.setup();
        let s = &setup.roots[screen];
        let visual = s.allowed_depths.iter().filter(|d| d.depth == s.root_depth).flat_map(|d| &d.visuals).find(|v| v.visual_id == s.root_visual);
        let fits = visual.is_some_and(|v| v.class == VisualClass::TRUE_COLOR && v.red_mask == 0xff_0000 && v.green_mask == 0xff00 && v.blue_mask == 0xff)
            && setup.pixmap_formats.iter().any(|f| f.depth == s.root_depth && f.bits_per_pixel == 32);
        if !fits {
            return Err(err(&"the screen is not 24-bit true colour"));
        }
        let (root, depth) = (s.root, s.root_depth);
        let root_size = (i32::from(s.width_in_pixels), i32::from(s.height_in_pixels));
        let lsb_first = setup.image_byte_order == ImageOrder::LSB_FIRST;
        let fail = |e: &dyn std::fmt::Display| format!("could not create the editor window: {e}");
        let win = c.generate_id().map_err(|e| fail(&e))?;
        let gc = c.generate_id().map_err(|e| fail(&e))?;
        let blank = c.generate_id().map_err(|e| fail(&e))?;
        let px = |v: u32| v.clamp(1, u32::from(u16::MAX)) as u16;
        let events = EventMask::EXPOSURE
            | EventMask::BUTTON_PRESS
            | EventMask::BUTTON_RELEASE
            | EventMask::POINTER_MOTION
            | EventMask::LEAVE_WINDOW
            | EventMask::KEY_PRESS
            | EventMask::STRUCTURE_NOTIFY;
        (|| -> Result<(), Box<dyn std::error::Error>> {
            c.create_window(x11rb::COPY_DEPTH_FROM_PARENT, win, parent, 0, 0, px(w), px(h), 0, WindowClass::INPUT_OUTPUT, x11rb::COPY_FROM_PARENT, &CreateWindowAux::new().event_mask(events))?;
            c.create_gc(gc, win, &CreateGCAux::new().graphics_exposures(0))?;
            // An invisible cursor: a cleared 1 x 1 bitmap as its own mask.
            let (pix, pgc) = (c.generate_id()?, c.generate_id()?);
            c.create_pixmap(1, pix, root, 1, 1)?;
            c.create_gc(pgc, pix, &CreateGCAux::new().foreground(0))?;
            c.poly_fill_rectangle(pix, pgc, &[x11rb::protocol::xproto::Rectangle { x: 0, y: 0, width: 1, height: 1 }])?;
            c.create_cursor(blank, pix, pix, 0, 0, 0, 0, 0, 0, 0, 0)?;
            c.free_gc(pgc)?;
            c.free_pixmap(pix)?;
            c.map_window(win)?;
            c.flush()?;
            Ok(())
        })()
        .map_err(|e| fail(&e))?;
        let xi = c.extension_information(xinput::X11_EXTENSION_NAME).ok().flatten().is_some()
            && c.xinput_xi_query_version(2, 0).ok().and_then(|r| r.reply().ok()).is_some_and(|r| r.major_version >= 2);
        let keys = Keymap::load(&c);
        let max_request = c.maximum_request_bytes();
        Ok(Window {
            c,
            win,
            root,
            root_size,
            depth,
            gc,
            lsb_first,
            max_request,
            waker,
            keys,
            blank,
            xi,
            raw_seen: false,
            prev: (0, 0),
            warping: false,
            pointer: (0, 0),
            state: 0,
            last_press: None,
            lock: None,
            next_tick: Instant::now() + TICK,
            queue: std::collections::VecDeque::new(),
        })
    }

    /// No input is waiting: a good moment to paint (once per burst of events).
    pub fn idle(&self) -> bool {
        self.queue.is_empty()
    }

    /// Ctrl, Shift, Alt held at the last event.
    pub fn mods(&self) -> (bool, bool, bool) {
        let on = |m: KeyButMask| self.state & u16::from(m) != 0;
        (on(KeyButMask::CONTROL), on(KeyButMask::SHIFT), on(KeyButMask::MOD1))
    }

    /// The next input, waiting for it; `None` when the editor closes (or the display goes).
    pub fn next(&mut self) -> Option<Input> {
        loop {
            if self.waker.quitting() {
                return None;
            }
            if let Some(i) = self.queue.pop_front() {
                return Some(i);
            }
            loop {
                match self.c.poll_for_event() {
                    Ok(Some(e)) => {
                        if let Some(i) = self.convert(e) {
                            self.queue.push_back(i);
                        }
                    }
                    Ok(None) => break,
                    Err(_) => return None,
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
            let ms = (self.next_tick - now).as_millis() as i32 + 1;
            if self.wait(ms) {
                return if self.waker.quitting() { None } else { Some(Input::Tick) };
            }
        }
    }

    /// Waits up to `ms` for the display or a wake; whether it was a wake.
    fn wait(&self, ms: i32) -> bool {
        let _ = self.c.flush();
        let mut fds = [
            libc::pollfd { fd: self.c.stream().as_raw_fd(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: self.waker.0.rx.as_raw_fd(), events: libc::POLLIN, revents: 0 },
        ];
        // SAFETY: a valid array of two `pollfd`s.
        unsafe { libc::poll(fds.as_mut_ptr(), 2, ms) };
        let woke = fds[1].revents & libc::POLLIN != 0;
        if woke {
            self.waker.drain();
        }
        woke
    }

    /// The next event of any window, for modal loops; `None` when the editor closes.
    fn event(&mut self) -> Option<Event> {
        loop {
            if self.waker.quitting() {
                return None;
            }
            match self.c.poll_for_event() {
                Ok(Some(e)) => return Some(e),
                Ok(None) => {}
                Err(_) => return None,
            }
            self.wait(100);
        }
    }

    fn track(&mut self, root: (i16, i16), state: KeyButMask) {
        self.pointer = root;
        self.state = u16::from(state);
    }

    fn convert(&mut self, e: Event) -> Option<Input> {
        let btn = |d: u8| match d {
            1 => Some(Button::Left),
            2 => Some(Button::Middle),
            3 => Some(Button::Right),
            _ => None,
        };
        match e {
            Event::Expose(e) if e.window == self.win && e.count == 0 => Some(Input::Expose),
            Event::ButtonPress(e) if e.event == self.win => {
                self.track((e.root_x, e.root_y), e.state);
                let (x, y) = (f32::from(e.event_x), f32::from(e.event_y));
                if e.detail == 4 || e.detail == 5 {
                    return Some(Input::Wheel { x, y, notches: if e.detail == 4 { 1.0 } else { -1.0 } });
                }
                let b = btn(e.detail)?;
                let dbl = self.last_press.is_some_and(|(t, d, px, py)| {
                    d == e.detail && e.time.wrapping_sub(t) < DOUBLE_CLICK_MS && (px - e.event_x).abs() < 4 && (py - e.event_y).abs() < 4
                });
                self.last_press = if dbl { None } else { Some((e.time, e.detail, e.event_x, e.event_y)) };
                // The keyboard is left where it is (the host's typing keyboard keeps playing): a type-in box grabs it
                // while it is open.
                Some(Input::Down { x, y, btn: b, dbl })
            }
            Event::ButtonRelease(e) if e.event == self.win => {
                self.track((e.root_x, e.root_y), e.state);
                btn(e.detail).map(|b| Input::Up { btn: b })
            }
            Event::MotionNotify(e) if e.event == self.win => {
                self.track((e.root_x, e.root_y), e.state);
                match self.lock {
                    // Raw motion drives a held drag once it arrives.
                    Some(_) if self.raw_seen => None,
                    // Else the pointer's own motion: the distance from the last position, the pointer warped back to
                    // the anchor (one warp at a time; its echo, an event at the anchor, is not a movement).
                    Some(a) => {
                        let pos = (e.root_x, e.root_y);
                        if self.warping && pos == a {
                            self.warping = false;
                            self.prev = a;
                            return None;
                        }
                        let (dx, dy) = (pos.0 - self.prev.0, pos.1 - self.prev.1);
                        self.prev = pos;
                        if !self.warping && pos != a {
                            self.warp(a);
                            self.warping = true;
                        }
                        ((dx, dy) != (0, 0)).then(|| Input::Delta { dx: f32::from(dx), dy: f32::from(dy) })
                    }
                    None => Some(Input::Move { x: f32::from(e.event_x), y: f32::from(e.event_y) }),
                }
            }
            Event::XinputRawMotion(e) if self.lock.is_some() => {
                self.raw_seen = true;
                let (dx, dy) = raw_xy(&e);
                if dx == 0.0 && dy == 0.0 {
                    return None;
                }
                // The hidden pointer stays where the drag started.
                if let Some(a) = self.lock {
                    self.warp(a);
                }
                Some(Input::Delta { dx, dy })
            }
            Event::LeaveNotify(e) if e.event == self.win && e.detail != NotifyDetail::INFERIOR => Some(Input::Leave),
            Event::KeyPress(e) if e.event == self.win => {
                self.state = u16::from(e.state);
                let sym = self.keys.sym(e.detail, self.state);
                Some(Input::Key { sym, text: keysym_char(sym) })
            }
            _ => None,
        }
    }

    fn warp(&self, (x, y): (i16, i16)) {
        let _ = self.c.warp_pointer(x11rb::NONE, self.root, 0, 0, 0, 0, x, y);
    }

    /// Copies `px` (XRGB rows of `w`) into the window at (0, 0).
    pub fn blit(&self, px: &[u32], w: u32, h: u32) {
        put(&self.c, self.win, self.gc, self.depth, self.lsb_first, self.max_request, px, w, h);
        let _ = self.c.flush();
    }

    /// Resizes the window (device px).
    pub fn resize(&self, w: u32, h: u32) {
        let aux = x11rb::protocol::xproto::ConfigureWindowAux::new().width(w.max(1)).height(h.max(1));
        let _ = self.c.configure_window(self.win, &aux);
        let _ = self.c.flush();
    }

    /// A window point on the screen (device px).
    pub fn to_root(&self, x: i32, y: i32) -> (i32, i32) {
        let (x, y) = (x.clamp(-32768, 32767) as i16, y.clamp(-32768, 32767) as i16);
        let t = self.c.translate_coordinates(self.win, self.root, x, y);
        t.ok().and_then(|t| t.reply().ok()).map_or((i32::from(x), i32::from(y)), |r| (i32::from(r.dst_x), i32::from(r.dst_y)))
    }

    /// Runs a desktop dialog (a file dialog): here, at once on this thread (`mac` runs it on the main thread).
    pub fn dialog<R: Send + 'static>(&self, f: impl FnOnce() -> R + Send + 'static) -> Option<R> {
        Some(f())
    }

    pub fn locked(&self) -> bool {
        self.lock.is_some()
    }

    /// Hides the cursor and holds it where it is: movement arrives as `Input::Delta`.
    pub fn lock(&mut self) {
        if self.lock.is_some() {
            return;
        }
        let mask = EventMask::POINTER_MOTION | EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE;
        let _ = self.c.grab_pointer(false, self.win, mask, GrabMode::ASYNC, GrabMode::ASYNC, x11rb::NONE, self.blank, x11rb::CURRENT_TIME);
        if self.xi {
            let m = xinput::EventMask { deviceid: 1, mask: vec![xinput::XIEventMask::RAW_MOTION] };
            let _ = self.c.xinput_xi_select_events(self.root, &[m]);
        }
        self.lock = Some(self.pointer);
        (self.prev, self.warping, self.raw_seen) = (self.pointer, false, false);
        let _ = self.c.flush();
    }

    /// Shows the cursor again where the drag started.
    pub fn unlock(&mut self) {
        let Some(a) = self.lock.take() else { return };
        if self.xi {
            let m = xinput::EventMask { deviceid: 1, mask: vec![xinput::XIEventMask::from(0u32)] };
            let _ = self.c.xinput_xi_select_events(self.root, &[m]);
        }
        self.warp(a);
        let _ = self.c.ungrab_pointer(x11rb::CURRENT_TIME);
        let _ = self.c.flush();
    }

    /// Grabs pointer and keyboard for a modal window (retried while it is being mapped).
    fn grab(&self, w: u32) {
        let mask = EventMask::POINTER_MOTION | EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE;
        for _ in 0..40 {
            let ok = self
                .c
                .grab_pointer(false, w, mask, GrabMode::ASYNC, GrabMode::ASYNC, x11rb::NONE, x11rb::NONE, x11rb::CURRENT_TIME)
                .ok()
                .and_then(|r| r.reply().ok())
                .is_some_and(|r| r.status == GrabStatus::SUCCESS);
            if ok {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = self.c.grab_keyboard(false, w, x11rb::CURRENT_TIME, GrabMode::ASYNC, GrabMode::ASYNC).ok().and_then(|r| r.reply().ok());
    }

    fn ungrab(&self) {
        let _ = self.c.ungrab_pointer(x11rb::CURRENT_TIME);
        let _ = self.c.ungrab_keyboard(x11rb::CURRENT_TIME);
        let _ = self.c.flush();
    }

    /// An override-redirect window (popup, box) at screen `r` (x, y, w, h), mapped.
    fn overlay(&self, r: [i32; 4]) -> Option<(u32, u32)> {
        let (win, gc) = (self.c.generate_id().ok()?, self.c.generate_id().ok()?);
        let aux = CreateWindowAux::new().override_redirect(1).event_mask(EventMask::EXPOSURE);
        let px = |v: i32| v.clamp(1, i32::from(u16::MAX)) as u16;
        self.c
            .create_window(x11rb::COPY_DEPTH_FROM_PARENT, win, self.root, r[0] as i16, r[1] as i16, px(r[2]), px(r[3]), 0, WindowClass::INPUT_OUTPUT, x11rb::COPY_FROM_PARENT, &aux)
            .ok()?;
        self.c.create_gc(gc, win, &CreateGCAux::new().graphics_exposures(0)).ok()?;
        self.c.map_window(win).ok()?;
        let _ = self.c.flush();
        Some((win, gc))
    }

    fn close_overlay(&self, (win, gc): (u32, u32)) {
        let _ = self.c.free_gc(gc);
        let _ = self.c.destroy_window(win);
        let _ = self.c.flush();
    }

    fn show(&self, (win, gc): (u32, u32), c: &Canvas) {
        let (w, h) = (c.pixmap.width(), c.pixmap.height());
        let mut px = vec![0u32; (w * h) as usize];
        c.copy_to_xrgb(&mut px);
        put(&self.c, win, gc, self.depth, self.lsb_first, self.max_request, &px, w, h);
        let _ = self.c.flush();
    }

    // ---------------------------------------------------------------------------------------------------------------
    // Popup menus: an override-redirect window painted with `kit::menu`, run modally until a pick or a cancel.

    /// Shows a menu at screen (`x`, `y`); the picked entry's command.
    pub fn popup<C: Clone>(&mut self, x: i32, y: i32, def: &MenuDef<C>, pal: &Palette, scale: f32) -> Option<C> {
        if crate::no_modal() {
            return None;
        }
        self.unlock();
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
        let (mut px, mut py) = (x, y);
        if px + w > self.root_size.0 {
            px = match parent {
                Some(p) => p[0] - w,
                None => self.root_size.0 - w,
            };
        }
        if py + h > self.root_size.1 {
            py = (self.root_size.1 - h).max(0);
        }
        let Ok(mut canvas) = Canvas::new(w.max(1) as u32, h.max(1) as u32, scale) else { return PopRes::Cancel };
        let Some(pop) = self.overlay([px, py, w, h]) else { return PopRes::Cancel };
        self.grab(pop.0);
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
        let start = (i32::from(self.pointer.0), i32::from(self.pointer.1));
        let mut moved = false;
        let mut sub_open: Option<usize> = None;
        let result = loop {
            let Some(e) = self.event() else { break PopRes::Cancel };
            match e {
                Event::Expose(e) if e.window == pop.0 => self.show(pop, &canvas),
                Event::MotionNotify(e) => {
                    let cur = (i32::from(e.root_x), i32::from(e.root_y));
                    self.pointer = (e.root_x, e.root_y);
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
                                    PopRes::Back => self.grab(pop.0),
                                }
                            }
                        }
                        Some(_) => {}
                        None => sub_open = None,
                    }
                }
                Event::ButtonPress(e) => {
                    if !inside(&rect, (i32::from(e.root_x), i32::from(e.root_y))) {
                        break PopRes::Cancel;
                    }
                    moved = true;
                }
                Event::ButtonRelease(e) if e.detail == 1 || e.detail == 3 => {
                    // The release of the click that opened the menu does not pick.
                    if !moved && opened.elapsed() < Duration::from_millis(400) {
                        continue;
                    }
                    let cur = (i32::from(e.root_x), i32::from(e.root_y));
                    if let Some(i) = inside(&rect, cur).then(|| item_at(cur)).flatten() {
                        if def.items[i].clickable() && def.sub(i).is_none() {
                            break PopRes::Pick(def.cmds[i].clone());
                        }
                    }
                }
                Event::KeyPress(e) => {
                    let sym = self.keys.sym(e.detail, u16::from(e.state));
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
        let x = x.min(self.root_size.0 - w).max(0);
        let y = y.min(self.root_size.1 - h).max(0);
        let mut canvas = Canvas::new(w as u32, h as u32, scale).ok()?;
        let boxw = self.overlay([x, y, w, h])?;
        self.grab(boxw.0);
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
            match e {
                Event::Expose(e) if e.window == boxw.0 => self.show(boxw, &canvas),
                Event::ButtonPress(e) => {
                    let (sx, sy) = (i32::from(e.root_x), i32::from(e.root_y));
                    if sx < r[0] || sx >= r[2] || sy < r[1] || sy >= r[3] {
                        break None;
                    }
                    all = false;
                    caret = s.len();
                }
                Event::KeyPress(e) => {
                    let state = u16::from(e.state);
                    let sym = self.keys.sym(e.detail, state);
                    let ctrl = state & u16::from(KeyButMask::CONTROL) != 0;
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
        let _ = self.c.free_cursor(self.blank);
        let _ = self.c.free_gc(self.gc);
        let _ = self.c.destroy_window(self.win);
        let _ = self.c.flush();
    }
}

enum PopRes<C> {
    Pick(C),
    Cancel,
    /// The mouse went back to the parent menu.
    Back,
}

/// The x and y of a raw motion event (device units, accelerated).
fn raw_xy(e: &xinput::RawMotionEvent) -> (f32, f32) {
    let mut vals = e.axisvalues.iter();
    let (mut dx, mut dy) = (0.0f64, 0.0f64);
    for axis in 0..e.valuator_mask.len() * 32 {
        if e.valuator_mask[axis / 32] & (1 << (axis % 32)) == 0 {
            continue;
        }
        let Some(v) = vals.next() else { break };
        let f = f64::from(v.integral) + f64::from(v.frac) / 4_294_967_296.0;
        match axis {
            0 => dx = f,
            1 => dy = f,
            _ => {}
        }
    }
    (dx as f32, dy as f32)
}

/// Puts XRGB pixels into a window in bands of whole rows, each small enough for one request.
#[allow(clippy::too_many_arguments)]
fn put(c: &RustConnection, win: u32, gc: u32, depth: u8, lsb_first: bool, max_request: usize, px: &[u32], w: u32, h: u32) {
    let (w, h) = (w as usize, h as usize);
    if w == 0 || h == 0 || px.len() < w * h {
        return;
    }
    let rows = (max_request.saturating_sub(64) / (w * 4)).clamp(1, h);
    for (i, band) in px[..w * h].chunks(w * rows).enumerate() {
        let bytes: Vec<u8> = band.iter().flat_map(|p| if lsb_first { p.to_le_bytes() } else { p.to_be_bytes() }).collect();
        let _ = c.put_image(ImageFormat::Z_PIXMAP, win, gc, w as u16, (band.len() / w) as u16, 0, (i * rows) as i16, 0, depth, &bytes);
    }
}
