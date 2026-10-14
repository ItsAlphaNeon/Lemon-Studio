// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Plugin windows on Linux: the frame (`super`) in an X11 window. Plugin editors on Linux embed only in X11 windows (CLAP
//! `x11`, VST3 `X11EmbedWindowID`), so the app runs its main window on X11 too (XWayland under Wayland, see `main`):
//! then a frame docks as on Windows, a child of the main window clipped to the workspace minus the app's windows above
//! it (the Shape extension), or detaches into a top-level window transient for the main window.
//!
//! When the main window is a native Wayland one (no X server to put it on) an X11 window cannot be its child: frames are
//! always top-level, and since they cannot be owned by it either, they are kept above other windows while Lemon Studio is
//! the active application (the main window has the focus, or a frame is the active window) and stacked normally
//! otherwise (`main_focused`). While a blocking dialog is open they step back (`unpinned`): the app waits for its answer
//! and cannot follow the focus, so a frame kept above would hide the dialog.
//!
//! One connection serves every frame (each plugin opens its own for its editor). The app reads its events on the UI tick
//! (`pump`) and repaints what changed after handling them (`paint_dirty`). Frames answer the window manager's pings
//! (`_NET_WM_PING`), so when the app stops responding the window manager can still close a frame by ending Lemon Studio.
//!
// spec: ui/plugin-window.md

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::rc::{Rc, Weak};

use x11rb::connection::{Connection, RequestConnection as _};
use x11rb::properties::WmSizeHints;
use x11rb::protocol::xproto::{
    self, AtomEnum, ClientMessageEvent, ConfigureWindowAux, ConnectionExt as _, CreateGCAux, CreateWindowAux, EventMask, ImageFormat, ImageOrder,
    KeyButMask, PropMode, StackMode, VisualClass, WindowClass,
};
use x11rb::protocol::shape::{self, ConnectionExt as _};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use super::*;

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        WM_PROTOCOLS,
        WM_DELETE_WINDOW,
        WM_CLIENT_MACHINE,
        _NET_WM_PING,
        WM_CHANGE_STATE,
        UTF8_STRING,
        _NET_WM_NAME,
        _NET_WM_PID,
        _NET_WM_ICON,
        _NET_WM_MOVERESIZE,
        _NET_ACTIVE_WINDOW,
        _NET_WM_STATE,
        _NET_WM_STATE_ABOVE,
        _MOTIF_WM_HINTS,
    }
}

/// `_NET_WM_MOVERESIZE` direction: move the window with the pointer.
const MOVERESIZE_MOVE: u32 = 8;
/// `WM_CHANGE_STATE`: iconify.
const ICONIC_STATE: u32 = 3;
/// Two presses closer than this are a double click (ms of server time).
const DOUBLE_CLICK_MS: u32 = 400;

struct Conn {
    c: RustConnection,
    root: xproto::Window,
    /// The screen's depth (24): images are sent at it.
    depth: u8,
    atoms: Atoms,
    /// Device pixels per logical one: the X resources' `Xft.dpi` / 96 (1 when the compositor scales X11 windows itself).
    scale: f32,
    /// Bytes of a 32-bit pixel in the server's order.
    lsb_first: bool,
    /// Most bytes one request may carry: images go in bands below it.
    max_request: usize,
    frames: RefCell<HashMap<xproto::Window, Weak<Frame>>>,
    /// A frame is the window manager's active window (`_NET_ACTIVE_WINDOW` on the root).
    frame_active: Cell<bool>,
    /// The frames are kept above other windows now (see the module notes).
    above: Cell<bool>,
    /// The main window when it is an X11 one (0 under Wayland): frames dock into it.
    owner: Cell<xproto::Window>,
}

thread_local! {
    static CONN: RefCell<Option<Rc<Conn>>> = const { RefCell::new(None) };
    /// The main window has the keyboard focus (`main_focused`), known before the connection opens.
    static MAIN_FOCUSED: Cell<bool> = const { Cell::new(true) };
    /// Blocking dialogs open now (`unpinned`): the frames are not kept above while there is one.
    static MODAL: Cell<u32> = const { Cell::new(0) };
    /// A docked frame moved, shrank or went: part of the main window was uncovered (`take_exposed`).
    static EXPOSED: Cell<bool> = const { Cell::new(false) };
}

/// Whether part of the main window was uncovered (a docked frame moved, was clipped or closed, or the X server exposed
/// it) since the last call: the next present must show the whole canvas, not only the app's own damage (UI thread).
pub fn take_exposed() -> bool {
    EXPOSED.with(|e| e.replace(false))
}

/// `take_exposed` would be true: the app presents again even with nothing of its own to paint.
pub fn exposed() -> bool {
    EXPOSED.with(Cell::get)
}

fn expose() {
    EXPOSED.with(|e| e.set(true));
}

/// The connection, opened on first use (`DISPLAY`: XWayland under Wayland).
fn conn() -> Result<Rc<Conn>, String> {
    if let Some(c) = CONN.with(|c| c.borrow().clone()) {
        return Ok(c);
    }
    let err = |e: &dyn std::fmt::Display| format!("plugin windows need an X11 display (XWayland under Wayland): {e}");
    let (c, screen) = RustConnection::connect(None).map_err(|e| err(&e))?;
    let setup = c.setup();
    let s = &setup.roots[screen];
    // 24-bit true colour in 32-bit pixels, red in the high byte: the canvas's XRGB as it is.
    let visual = s.allowed_depths.iter().filter(|d| d.depth == s.root_depth).flat_map(|d| &d.visuals).find(|v| v.visual_id == s.root_visual);
    let fits = visual.is_some_and(|v| v.class == VisualClass::TRUE_COLOR && v.red_mask == 0xff_0000 && v.green_mask == 0xff00 && v.blue_mask == 0xff)
        && setup.pixmap_formats.iter().any(|f| f.depth == s.root_depth && f.bits_per_pixel == 32);
    if !fits {
        return Err(err(&"the screen is not 24-bit true colour"));
    }
    let (root, depth) = (s.root, s.root_depth);
    let lsb_first = setup.image_byte_order == ImageOrder::LSB_FIRST;
    let atoms = Atoms::new(&c).map_err(|e| err(&e))?.reply().map_err(|e| err(&e))?;
    let dpi = x11rb::resource_manager::new_from_default(&c).ok().and_then(|db| db.get_value::<u32>("Xft.dpi", "").ok().flatten());
    let scale = dpi.filter(|&d| d >= 96).map_or(1.0, |d| d as f32 / 96.0);
    let max_request = c.maximum_request_bytes();
    // Root property changes say which window is active.
    c.change_window_attributes(root, &xproto::ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE)).map_err(|e| err(&e))?;
    let above = MAIN_FOCUSED.with(Cell::get) && MODAL.with(Cell::get) == 0;
    let conn = Rc::new(Conn {
        c,
        root,
        depth,
        atoms,
        scale,
        lsb_first,
        max_request,
        frames: RefCell::new(HashMap::new()),
        frame_active: Cell::new(false),
        above: Cell::new(above),
        owner: Cell::new(0),
    });
    CONN.with(|c| *c.borrow_mut() = Some(conn.clone()));
    Ok(conn)
}

/// A plugin window's native parts.
struct Frame {
    conn: Rc<Conn>,
    win: xproto::Window,
    /// Docked: the position in the main window (physical px), kept here so clipping needs no round trip.
    pos: Cell<(i32, i32)>,
    /// The child window the plugin's editor goes into.
    area: xproto::Window,
    gc: xproto::Gcontext,
    state: RefCell<State>,
    canvas: RefCell<Canvas>,
    scale: f32,
    /// The frame needs painting (`paint_dirty`).
    dirty: Cell<bool>,
    /// The last left press (server time, position) for double clicks.
    last_press: Cell<Option<(u32, i16, i16)>>,
}

/// A plugin window. Dropping it destroys the native window (close the plugin's editor first).
pub struct FrameWindow {
    f: Rc<Frame>,
}

/// Physical px of a logical length (at least 1).
fn px(v: f32, k: f32) -> u16 {
    (v * k).round().clamp(1.0, f32::from(u16::MAX)) as u16
}

fn pixel(c: Color) -> u32 {
    u32::from(c.r) << 16 | u32::from(c.g) << 8 | u32::from(c.b)
}

impl FrameWindow {
    /// A hidden frame for an editor of `editor` logical px; `raise` shows it. Docked (a child of `owner`, the main
    /// window, at screen position `pos`) unless `detached` or the main window is not an X11 one (`owner` 0); a detached
    /// frame is placed by the window manager, transient for `owner`. The scale is the X server's (`Xft.dpi`), not
    /// `_scale`: under Wayland the main window's scale does not apply to X11 windows.
    pub fn new(owner: isize, info: ChannelInfo, editor: (f32, f32), _scale: f32, pos: (i32, i32), detached: bool) -> Result<FrameWindow, String> {
        let conn = conn()?;
        let fail = |e: &dyn std::fmt::Display| format!("could not create the plugin window: {e}");
        let k = conn.scale;
        let owner = owner as xproto::Window;
        set_owner(&conn, owner);
        let docked = owner != 0 && !detached;
        let state = State::new(info, editor, !docked);
        let (parent, at) = if docked { (owner, from_root(&conn, owner, pos)) } else { (conn.root, (0, 0)) };
        let (w, h) = state.size();
        let ay = state.area_y();
        let title = state.info.name.clone();
        let c = &conn.c;
        let a = &conn.atoms;
        let win = c.generate_id().map_err(|e| fail(&e))?;
        let area = c.generate_id().map_err(|e| fail(&e))?;
        let gc = c.generate_id().map_err(|e| fail(&e))?;
        let events = EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION | EventMask::STRUCTURE_NOTIFY;
        let canvas = Canvas::new(u32::from(px(w, k)), u32::from(px(h, k)), k)?;
        let made = (|| -> Result<(), Box<dyn std::error::Error>> {
            // No background: every expose is painted from the canvas.
            c.create_window(x11rb::COPY_DEPTH_FROM_PARENT, win, parent, at.0 as i16, at.1 as i16, px(w, k), px(h, k), 0, WindowClass::INPUT_OUTPUT, x11rb::COPY_FROM_PARENT, &CreateWindowAux::new().event_mask(events))?;
            c.create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                area,
                win,
                px(BORDER, k) as i16,
                px(ay, k) as i16,
                px(editor.0, k),
                px(editor.1, k),
                0,
                WindowClass::INPUT_OUTPUT,
                x11rb::COPY_FROM_PARENT,
                &CreateWindowAux::new().background_pixel(pixel(c_body())),
            )?;
            c.create_gc(gc, win, &CreateGCAux::new().graphics_exposures(0))?;
            c.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
            c.change_property8(PropMode::REPLACE, win, a._NET_WM_NAME, a.UTF8_STRING, title.as_bytes())?;
            // The desktop entry's id: the frame is grouped with Lemon Studio and gets its icon.
            c.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, b"lemonstudio\0lemonstudio\0")?;
            c.change_property32(PropMode::REPLACE, win, a.WM_PROTOCOLS, AtomEnum::ATOM, &[a.WM_DELETE_WINDOW, a._NET_WM_PING])?;
            // The process and its host: the window manager can end an unresponsive Lemon Studio from a frame.
            c.change_property32(PropMode::REPLACE, win, a._NET_WM_PID, AtomEnum::CARDINAL, &[std::process::id()])?;
            if let Ok(host) = std::fs::read_to_string("/proc/sys/kernel/hostname") {
                c.change_property8(PropMode::REPLACE, win, a.WM_CLIENT_MACHINE, AtomEnum::STRING, host.trim().as_bytes())?;
            }
            // No decorations: the frame draws its own title bar (flags: decorations; decorations: none).
            c.change_property32(PropMode::REPLACE, win, a._MOTIF_WM_HINTS, a._MOTIF_WM_HINTS, &[2, 0, 0, 0, 0])?;
            if let Some(icon) = icon() {
                c.change_property32(PropMode::REPLACE, win, a._NET_WM_ICON, AtomEnum::CARDINAL, &icon)?;
            }
            if owner != 0 {
                c.change_property32(PropMode::REPLACE, win, AtomEnum::WM_TRANSIENT_FOR, AtomEnum::WINDOW, &[owner as u32])?;
            }
            // Opened from the active application: it starts above (taken when the window is mapped). Under X11 the main
            // window owns it instead (transient for it).
            if !docked && conn.above.get() {
                c.change_property32(PropMode::REPLACE, win, a._NET_WM_STATE, AtomEnum::ATOM, &[a._NET_WM_STATE_ABOVE])?;
            }
            fixed_size(c, win, px(w, k), px(h, k))?;
            // Clicks in the plugin's editor come here first (then go on to it, `pump`): they bring the window to the
            // front of the app's windows, as clicks on the frame do.
            for b in [xproto::ButtonIndex::M1, xproto::ButtonIndex::M2, xproto::ButtonIndex::M3] {
                c.grab_button(false, area, EventMask::BUTTON_PRESS, xproto::GrabMode::SYNC, xproto::GrabMode::ASYNC, x11rb::NONE, x11rb::NONE, b, xproto::ModMask::ANY)?;
            }
            c.map_window(area)?;
            // The plugin parents its editor into `area` through its own connection: it must exist on the server first.
            c.flush()?;
            Ok(())
        })();
        if let Err(e) = made {
            let _ = c.destroy_window(win);
            let _ = c.flush();
            return Err(fail(&e));
        }
        let f = Rc::new(Frame {
            conn: conn.clone(),
            win,
            pos: Cell::new(at),
            area,
            gc,
            state: RefCell::new(state),
            canvas: RefCell::new(canvas),
            scale: k,
            dirty: Cell::new(true),
            last_press: Cell::new(None),
        });
        conn.frames.borrow_mut().insert(win, Rc::downgrade(&f));
        Ok(FrameWindow { f })
    }

    /// A top-level window, not a child of the main window (always under Wayland).
    pub fn detached(&self) -> bool {
        self.f.state.borrow().detached
    }

    /// Docks the window into `owner` (a child, same place on screen) or detaches it (a top-level window transient for
    /// `owner`). The plugin's editor stays embedded. Only when the main window is an X11 one.
    pub fn set_detached(&self, owner: isize, on: bool) {
        let f = &self.f;
        let owner = owner as xproto::Window;
        if owner == 0 || f.state.borrow().detached == on {
            return;
        }
        {
            let mut st = f.state.borrow_mut();
            st.detached = on;
            st.clip = None;
            st.clip_key = None;
        }
        let (c, root) = (&f.conn.c, f.conn.root);
        let screen = f.to_screen(0.0, 0.0);
        let _ = c.unmap_window(f.win);
        let _ = shape_reset(c, f.win);
        if on {
            expose();
            let (w, h) = {
                let cv = f.canvas.borrow();
                (cv.pixmap.width() as u16, cv.pixmap.height() as u16)
            };
            let _ = size_hints(c, f.win, w, h, Some(screen));
            let _ = c.reparent_window(f.win, root, screen.0 as i16, screen.1 as i16);
        } else {
            // A managed window goes back to the root when the window manager lets it go (it may have put it in a frame
            // of its own): wait for that, or the manager's reparent would undo ours.
            let _ = c.flush();
            let start = std::time::Instant::now();
            while start.elapsed() < std::time::Duration::from_millis(500) {
                let parent = c.query_tree(f.win).ok().and_then(|t| t.reply().ok()).map(|t| t.parent);
                if parent == Some(root) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let at = from_root(&f.conn, owner, screen);
            f.pos.set(at);
            let _ = c.reparent_window(f.win, owner, at.0 as i16, at.1 as i16);
        }
        self.raise();
    }

    /// Clips a docked window to `work` minus the `covers` (the app's windows and popups above it), all in the main
    /// window (physical px). Moving the window by its title bar re-clips with the same rects.
    pub fn clip_to(&self, work: [i32; 4], covers: &[[i32; 4]]) {
        if self.detached() {
            return;
        }
        self.f.state.borrow_mut().clip = Some((work, covers.to_vec()));
        self.f.apply_clip();
    }

    /// The native frame window (an X11 window id).
    pub fn hwnd(&self) -> isize {
        self.f.win as isize
    }

    /// The child window the plugin's editor goes into (an X11 window id, as plugin APIs pass it).
    pub fn area(&self) -> *mut c_void {
        self.f.area as usize as *mut c_void
    }

    /// Shows the window and brings it to the front (a docked one in front of the other docked windows).
    pub fn raise(&self) {
        let (c, a, win) = (&self.f.conn.c, &self.f.conn.atoms, self.f.win);
        let _ = c.map_window(win);
        let _ = c.configure_window(win, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE));
        if !self.detached() {
            self.f.dirty.set(true);
            self.f.paint();
            return;
        }
        // Source 2: asked for by the user (a pager), which focus stealing prevention lets through.
        let active = ClientMessageEvent::new(32, win, a._NET_ACTIVE_WINDOW, [2, x11rb::CURRENT_TIME, 0, 0, 0]);
        let _ = c.send_event(false, self.f.conn.root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, active);
        self.f.dirty.set(true);
        self.f.paint();
    }

    pub fn set_info(&self, info: ChannelInfo) {
        let mut st = self.f.state.borrow_mut();
        if st.info != info && st.drag.is_none() && st.misc_ui.drag.is_none() {
            st.info = info;
            self.f.dirty.set(true);
        }
    }

    /// A Lemon plugin joined the strip (host link): its page tabs replace the plug tab.
    pub fn set_pages(&self, pages: Vec<PageIcon>, active: usize) {
        let mut st = self.f.state.borrow_mut();
        if !pages.is_empty() && (st.pages != pages || st.page != active) {
            st.page = active.min(pages.len() - 1);
            st.pages = pages;
            self.f.dirty.set(true);
        }
    }

    /// The plugin changed its page itself.
    pub fn set_page(&self, page: usize) {
        let mut st = self.f.state.borrow_mut();
        if st.page != page && page < st.pages.len() {
            st.page = page;
            self.f.dirty.set(true);
        }
    }

    /// The plugin's hint line (parameter under the mouse).
    pub fn set_hint(&self, hint: String) {
        let mut st = self.f.state.borrow_mut();
        if st.hint != hint {
            st.hint = hint;
            self.f.dirty.set(true);
        }
    }

    /// The loaded preset's name for the Presets box.
    pub fn set_preset(&self, name: Option<String>) {
        let mut st = self.f.state.borrow_mut();
        if st.preset != name {
            st.preset = name;
            self.f.dirty.set(true);
        }
    }

    /// The scale the frame is drawn at (the X server's).
    pub fn scale(&self) -> f32 {
        self.f.scale
    }

    /// Follows a new editor size (plugins may resize).
    pub fn set_editor_size(&self, editor: (f32, f32)) {
        if self.f.state.borrow().editor != editor {
            self.f.state.borrow_mut().editor = editor;
            self.f.layout();
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
        if !f.state.borrow().detached {
            expose();
        }
        f.conn.frames.borrow_mut().remove(&f.win);
        let _ = f.conn.c.free_gc(f.gc);
        let _ = f.conn.c.destroy_window(f.win);
        let _ = f.conn.c.flush();
        // An editor that held the keyboard left it with nobody.
        focus_main();
    }
}

/// The window manager keeps the window at this size (the frame follows its editor, the user does not resize it).
fn fixed_size(c: &RustConnection, win: xproto::Window, w: u16, h: u16) -> Result<(), Box<dyn std::error::Error>> {
    size_hints(c, win, w, h, None)
}

/// `fixed_size`, and the place on the screen the window manager should keep (a window just detached stays put).
fn size_hints(c: &RustConnection, win: xproto::Window, w: u16, h: u16, at: Option<(i32, i32)>) -> Result<(), Box<dyn std::error::Error>> {
    let mut hints = WmSizeHints::new();
    hints.min_size = Some((i32::from(w), i32::from(h)));
    hints.max_size = Some((i32::from(w), i32::from(h)));
    hints.position = at.map(|(x, y)| (x11rb::properties::WmSizeHintsSpecification::UserSpecified, x, y));
    hints.set_normal_hints(c, win)?;
    Ok(())
}

/// `_NET_WM_ICON` data: width, height, then ARGB pixels (the app icon at 64 px).
fn icon() -> Option<Vec<u32>> {
    let (rgba, w, h) = crate::icon_rgba()?;
    // A 4 x 4 box down to 64 px: the 256 px icon is a large property for a window that may open often.
    let n = 4usize;
    let (w, h) = (w as usize, h as usize);
    let (ow, oh) = (w / n, h / n);
    let mut out = Vec::with_capacity(2 + ow * oh);
    out.extend([ow as u32, oh as u32]);
    for y in 0..oh {
        for x in 0..ow {
            let mut sum = [0u32; 4];
            for dy in 0..n {
                for dx in 0..n {
                    let i = ((y * n + dy) * w + x * n + dx) * 4;
                    for (s, v) in sum.iter_mut().zip(&rgba[i..i + 4]) {
                        *s += u32::from(*v);
                    }
                }
            }
            let [r, g, b, a] = sum.map(|s| s / (n * n) as u32);
            out.push(a << 24 | r << 16 | g << 8 | b);
        }
    }
    Some(out)
}

impl Frame {
    /// Resizes the window to its state (strip shown or not, editor size) and moves the editor area.
    fn layout(&self) {
        let st = self.state.borrow();
        let (fw, fh) = st.size();
        let (ay, ed, show) = (st.area_y(), st.editor, !st.misc);
        drop(st);
        let (k, c) = (self.scale, &self.conn.c);
        let (w, h) = (px(fw, k), px(fh, k));
        let _ = fixed_size(c, self.win, w, h);
        let _ = c.configure_window(self.win, &ConfigureWindowAux::new().width(u32::from(w)).height(u32::from(h)));
        let area = ConfigureWindowAux::new().x(i32::from(px(BORDER, k))).y(i32::from(px(ay, k))).width(u32::from(px(ed.0, k))).height(u32::from(px(ed.1, k)));
        let _ = c.configure_window(self.area, &area);
        let _ = if show { c.map_window(self.area) } else { c.unmap_window(self.area) };
        self.canvas.borrow_mut().resize(u32::from(w), u32::from(h), k);
        self.dirty.set(true);
        self.paint();
    }

    /// Paints the frame when it changed.
    fn paint(&self) {
        if !self.dirty.replace(false) {
            return;
        }
        let mut canvas = self.canvas.borrow_mut();
        super::paint(&mut canvas, &self.state.borrow());
        let (w, h) = (canvas.pixmap.width() as usize, canvas.pixmap.height() as usize);
        let mut xrgb = vec![0u32; w * h];
        canvas.copy_to_xrgb(&mut xrgb);
        drop(canvas);
        put_pixels(&self.conn, self.win, self.gc, &xrgb, w, h);
    }

    /// Sets a docked window's shape for its current place from `State::clip`: the workspace minus the rects covering
    /// it. The X server exposes what comes into view (the plugin's editor included) and the main window under what
    /// goes out of it.
    fn apply_clip(&self) {
        let (w, h) = {
            let c = self.canvas.borrow();
            (c.pixmap.width() as i32, c.pixmap.height() as i32)
        };
        let (x, y) = self.pos.get();
        let me = [x, y, x + w, y + h];
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
        let c = &self.conn.c;
        let _ = match rects {
            None => shape_reset(c, self.win),
            Some(rs) => {
                let rs: Vec<xproto::Rectangle> = rs
                    .iter()
                    .map(|r| xproto::Rectangle { x: (r[0] - x) as i16, y: (r[1] - y) as i16, width: (r[2] - r[0]) as u16, height: (r[3] - r[1]) as u16 })
                    .collect();
                c.shape_rectangles(shape::SO::SET, shape::SK::BOUNDING, xproto::ClipOrdering::UNSORTED, self.win, 0, 0, &rs).map(|_| ())
            }
        };
        let _ = c.flush();
        expose();
    }

    /// A window point (logical px) on the X screen (physical px).
    fn to_screen(&self, x: f32, y: f32) -> (i32, i32) {
        let k = self.scale;
        let t = self.conn.c.translate_coordinates(self.win, self.conn.root, (x * k) as i16, (y * k) as i16);
        t.ok().and_then(|t| t.reply().ok()).map_or((0, 0), |r| (i32::from(r.dst_x), i32::from(r.dst_y)))
    }

    /// Does what an input event asked of the window. `root`: the pointer on the screen at the event (a title-bar move
    /// starts from there).
    fn reply(&self, r: Reply, root: (i16, i16)) {
        let (c, a) = (&self.conn.c, &self.conn.atoms);
        let to_root = EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY;
        match r {
            Reply::None => {}
            // The press grabs the pointer until the release: drags keep their motion without more.
            Reply::Repaint | Reply::Capture => self.dirty.set(true),
            Reply::Relayout => self.layout(),
            Reply::Move if !self.state.borrow().detached => {
                // Docked: moved by hand within the main window (the press's grab brings the motion here).
                let (x, y) = self.pos.get();
                self.state.borrow_mut().moving = Some((i32::from(root.0), i32::from(root.1), x, y));
                self.dirty.set(true);
            }
            Reply::Move => {
                // The window manager moves it (it needs the pointer the press grabbed).
                let _ = c.ungrab_pointer(x11rb::CURRENT_TIME);
                let data = [root.0 as u32, root.1 as u32, MOVERESIZE_MOVE, 1, 1];
                let _ = c.send_event(false, self.conn.root, to_root, ClientMessageEvent::new(32, self.win, a._NET_WM_MOVERESIZE, data));
                let _ = c.flush();
            }
            Reply::Minimize if !self.state.borrow().detached => {
                // A docked window hides (its channel's name button shows it again).
                let _ = c.unmap_window(self.win);
                let _ = c.flush();
                expose();
            }
            Reply::Minimize => {
                let _ = c.send_event(false, self.conn.root, to_root, ClientMessageEvent::new(32, self.win, a.WM_CHANGE_STATE, [ICONIC_STATE, 0, 0, 0, 0]));
                let _ = c.flush();
            }
        }
    }

    /// A click on the window or in its editor: it goes to the front of the app's windows.
    fn clicked(&self) {
        if !self.state.borrow().detached {
            let _ = self.conn.c.configure_window(self.win, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE));
        }
        self.state.borrow_mut().events.push(FrameEvent::Raise);
        os_clap::wake();
    }

    fn event(&self, e: Event) {
        let k = self.scale;
        let screen = |x: f32, y: f32| self.to_screen(x, y);
        match e {
            Event::Expose(_) => self.dirty.set(true),
            // A click in the plugin's editor (the grab on the area; `pump` lets the editor have it).
            Event::ButtonPress(e) if e.event == self.area => self.clicked(),
            Event::ButtonPress(e) => {
                if matches!(e.detail, 1..=3) {
                    self.clicked();
                    // A click on the frame (not in the editor): keys play the typing keyboard again.
                    focus_main();
                }
                let (x, y) = (f32::from(e.event_x) / k, f32::from(e.event_y) / k);
                let r = match e.detail {
                    1 => {
                        let double = self.last_press.get().is_some_and(|(t, px, py)| {
                            e.time.wrapping_sub(t) < DOUBLE_CLICK_MS && (px - e.event_x).abs() < 4 && (py - e.event_y).abs() < 4
                        });
                        self.last_press.set(if double { None } else { Some((e.time, e.event_x, e.event_y)) });
                        self.state.borrow_mut().left_down(x, y, double, &screen)
                    }
                    2 => self.state.borrow_mut().middle_down(x, y, &screen),
                    3 => self.state.borrow_mut().right_down(x, y, &screen),
                    4 => self.state.borrow_mut().wheel(x, y, 1.0),
                    5 => self.state.borrow_mut().wheel(x, y, -1.0),
                    _ => Reply::None,
                };
                self.reply(r, (e.root_x, e.root_y));
            }
            Event::ButtonRelease(e) if e.detail == 1 => self.state.borrow_mut().release(),
            Event::MotionNotify(e) if self.state.borrow().moving.is_some() => {
                let Some((cx, cy, wx, wy)) = self.state.borrow().moving else { return };
                let at = (wx + i32::from(e.root_x) - cx, wy + i32::from(e.root_y) - cy);
                if at != self.pos.get() {
                    self.pos.set(at);
                    let _ = self.conn.c.configure_window(self.win, &ConfigureWindowAux::new().x(at.0).y(at.1));
                    // Re-clip at once for the new place; what the move uncovered is shown by the next present.
                    self.apply_clip();
                    let _ = self.conn.c.flush();
                    expose();
                    os_clap::wake();
                }
            }
            Event::MotionNotify(e) => {
                let fine = u16::from(e.state) & u16::from(KeyButMask::CONTROL) != 0;
                let r = self.state.borrow_mut().motion(f32::from(e.event_x) / k, f32::from(e.event_y) / k, fine);
                self.reply(r, (e.root_x, e.root_y));
            }
            Event::ClientMessage(e) if e.type_ == self.conn.atoms.WM_PROTOCOLS && e.data.as_data32()[0] == self.conn.atoms.WM_DELETE_WINDOW => {
                self.state.borrow_mut().events.push(FrameEvent::Close);
                os_clap::wake();
            }
            // The window manager checks the app still responds: the ping goes back to the root window.
            Event::ClientMessage(mut e) if e.type_ == self.conn.atoms.WM_PROTOCOLS && e.data.as_data32()[0] == self.conn.atoms._NET_WM_PING => {
                e.window = self.conn.root;
                let _ = self.conn.c.send_event(false, self.conn.root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, e);
                let _ = self.conn.c.flush();
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Lifted overlay, as on Windows: an app popup opened from a detached plugin window is painted in the main window, under
// the detached window (it stays above its owner). While one is open, its pixels are shown in a small window above the
// plugin window, and its mouse input goes to the app as if it happened in the main window (`take_lift_input`).

struct Lift {
    win: xproto::Window,
    gc: xproto::Gcontext,
    /// Main-window position of the lifted rect's top-left.
    origin: (i32, i32),
    size: (usize, usize),
    pixels: Vec<u32>,
    shown: bool,
}

thread_local! {
    static LIFT: RefCell<Option<Lift>> = const { RefCell::new(None) };
    static LIFT_INPUT: RefCell<Vec<LiftInput>> = const { RefCell::new(Vec::new()) };
}

/// Shows `pixels` (XRGB, `size`) above the detached window, at `origin` in main window `main` (physical px).
pub fn lift_show(main: isize, _frame: isize, origin: (i32, i32), size: (i32, i32), pixels: Vec<u32>) {
    let Ok(conn) = conn() else { return };
    if main == 0 || size.0 <= 0 || size.1 <= 0 {
        return;
    }
    let (x, y) = to_root(&conn, main as xproto::Window, origin);
    let (w, h) = (size.0.min(i32::from(u16::MAX)) as usize, size.1.min(i32::from(u16::MAX)) as usize);
    LIFT.with(|l| {
        let mut l = l.borrow_mut();
        let c = &conn.c;
        if l.is_none() {
            let (Ok(win), Ok(gc)) = (c.generate_id(), c.generate_id()) else { return };
            let events = EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION;
            // Not managed by the window manager: it sits where it is put, above the plugin window.
            let aux = CreateWindowAux::new().override_redirect(1).event_mask(events);
            if c.create_window(x11rb::COPY_DEPTH_FROM_PARENT, win, conn.root, x as i16, y as i16, w as u16, h as u16, 0, WindowClass::INPUT_OUTPUT, x11rb::COPY_FROM_PARENT, &aux).is_err() {
                return;
            }
            let _ = c.create_gc(gc, win, &CreateGCAux::new().graphics_exposures(0));
            *l = Some(Lift { win, gc, origin, size: (w, h), pixels: Vec::new(), shown: false });
        }
        let Some(lift) = l.as_mut() else { return };
        lift.origin = origin;
        lift.size = (w, h);
        lift.pixels = pixels;
        let _ = c.configure_window(lift.win, &ConfigureWindowAux::new().x(x).y(y).width(w as u32).height(h as u32).stack_mode(StackMode::ABOVE));
        if !lift.shown {
            let _ = c.map_window(lift.win);
            lift.shown = true;
        }
        put_pixels(&conn, lift.win, lift.gc, &lift.pixels, w, h);
    });
}

/// The lifted popup closed.
pub fn lift_hide() {
    let Some(conn) = CONN.with(|c| c.borrow().clone()) else { return };
    LIFT.with(|l| {
        if let Some(lift) = l.borrow_mut().as_mut().filter(|l| l.shown) {
            lift.shown = false;
            let _ = conn.c.unmap_window(lift.win);
            let _ = conn.c.flush();
        }
    });
}

/// Mouse input on the lifted popup since the last call (UI thread).
pub fn take_lift_input() -> Vec<LiftInput> {
    LIFT_INPUT.with(|q| std::mem::take(&mut *q.borrow_mut()))
}

/// An event of the lifted popup's window: repaint it, or queue its input for the app. Whether it was one.
fn lift_event(conn: &Conn, e: &Event) -> bool {
    LIFT.with(|l| {
        let l = l.borrow();
        let Some(lift) = l.as_ref() else { return false };
        let at = |x: i16, y: i16| LiftInput::Move(lift.origin.0 + i32::from(x), lift.origin.1 + i32::from(y));
        let input = match e {
            Event::Expose(x) if x.window == lift.win => {
                put_pixels(conn, lift.win, lift.gc, &lift.pixels, lift.size.0, lift.size.1);
                return true;
            }
            Event::ButtonPress(b) if b.event == lift.win => vec![at(b.event_x, b.event_y), LiftInput::Down(b.detail)],
            Event::ButtonRelease(b) if b.event == lift.win => vec![at(b.event_x, b.event_y), LiftInput::Up],
            Event::MotionNotify(m) if m.event == lift.win => vec![at(m.event_x, m.event_y)],
            _ => return false,
        };
        LIFT_INPUT.with(|q| q.borrow_mut().extend(input));
        os_clap::wake();
        true
    })
}

/// Copies XRGB rows of `w` (`h` rows) into `win` at (0, 0).
fn put_pixels(conn: &Conn, win: xproto::Window, gc: xproto::Gcontext, xrgb: &[u32], w: usize, h: usize) {
    if w == 0 || h == 0 {
        return;
    }
    let order = |p: &u32| if conn.lsb_first { p.to_le_bytes() } else { p.to_be_bytes() };
    // Bands of whole rows, each small enough for one request (the request header takes a few bytes).
    let rows = ((conn.max_request.saturating_sub(64)) / (w * 4).max(1)).clamp(1, h.max(1));
    for (i, band) in xrgb[..w * h].chunks(w * rows).enumerate() {
        let bytes: Vec<u8> = band.iter().flat_map(order).collect();
        let bh = (band.len() / w) as u16;
        let y = (i * rows) as i16;
        let _ = conn.c.put_image(ImageFormat::Z_PIXMAP, win, gc, w as u16, bh, 0, y, 0, conn.depth, &bytes);
    }
    let _ = conn.c.flush();
}

/// Back to the window's own rectangle.
fn shape_reset(c: &RustConnection, win: xproto::Window) -> Result<(), x11rb::errors::ConnectionError> {
    c.shape_mask(shape::SO::SET, shape::SK::BOUNDING, win, 0, 0, x11rb::NONE).map(|_| ())
}

/// The main window is `owner` (0: not an X11 one): docking needs it, and its exposures need a full present.
fn set_owner(conn: &Conn, owner: xproto::Window) {
    if owner == 0 || conn.owner.replace(owner) == owner {
        return;
    }
    // Our own interest in its exposures (the app's toolkit keeps its own): see `take_exposed`.
    let _ = conn.c.change_window_attributes(owner, &xproto::ChangeWindowAttributesAux::new().event_mask(EventMask::EXPOSURE));
    // Owned windows stay above their owner without help.
    keep_above(conn);
}

/// A screen point (physical px) in `win`'s coordinates.
fn from_root(conn: &Conn, win: xproto::Window, (x, y): (i32, i32)) -> (i32, i32) {
    let (x, y) = (x.clamp(-32768, 32767) as i16, y.clamp(-32768, 32767) as i16);
    let t = conn.c.translate_coordinates(conn.root, win, x, y);
    t.ok().and_then(|t| t.reply().ok()).map_or((i32::from(x), i32::from(y)), |r| (i32::from(r.dst_x), i32::from(r.dst_y)))
}

/// A point of `win` (physical px) on the screen.
fn to_root(conn: &Conn, win: xproto::Window, (x, y): (i32, i32)) -> (i32, i32) {
    let t = conn.c.translate_coordinates(win, conn.root, x.clamp(-32768, 32767) as i16, y.clamp(-32768, 32767) as i16);
    t.ok().and_then(|t| t.reply().ok()).map_or((x, y), |r| (i32::from(r.dst_x), i32::from(r.dst_y)))
}

/// A screen point (physical px) in the main window's coordinates, when it is an X11 one.
pub fn screen_to_main(x: i32, y: i32) -> Option<(i32, i32)> {
    let conn = CONN.with(|c| c.borrow().clone())?;
    let owner = conn.owner.get();
    (owner != 0).then(|| from_root(&conn, owner, (x, y)))
}

/// The pointer on the screen (physical px), when there is an X server.
pub fn pointer() -> Option<(i32, i32)> {
    let conn = conn().ok()?;
    let r = conn.c.query_pointer(conn.root).ok()?.reply().ok()?;
    Some((i32::from(r.root_x), i32::from(r.root_y)))
}

/// The window an event is about.
fn window_of(e: &Event) -> Option<xproto::Window> {
    Some(match e {
        Event::Expose(e) => e.window,
        Event::ButtonPress(e) => e.event,
        Event::ButtonRelease(e) => e.event,
        Event::MotionNotify(e) => e.event,
        Event::ClientMessage(e) => e.window,
        _ => return None,
    })
}

/// UI tick: handles the frames' window events (input, close, expose) and follows the active window.
pub fn pump() {
    let Some(conn) = CONN.with(|c| c.borrow().clone()) else { return };
    while let Ok(Some(e)) = conn.c.poll_for_event() {
        if let Event::PropertyNotify(p) = &e {
            if p.window == conn.root && p.atom == conn.atoms._NET_ACTIVE_WINDOW {
                let active = conn.c.get_property(false, conn.root, conn.atoms._NET_ACTIVE_WINDOW, AtomEnum::WINDOW, 0, 1);
                let id = active.ok().and_then(|r| r.reply().ok()).and_then(|r| r.value32().and_then(|mut v| v.next()));
                conn.frame_active.set(id.is_some_and(|id| conn.frames.borrow().contains_key(&id)));
                keep_above(&conn);
            }
            continue;
        }
        if let Event::Expose(x) = &e {
            if x.window == conn.owner.get() {
                expose();
                continue;
            }
        }
        if lift_event(&conn, &e) {
            continue;
        }
        // Frames by their window, or by their editor area (clicks in the editor).
        let frame = window_of(&e).and_then(|w| {
            let frames = conn.frames.borrow();
            frames.get(&w).and_then(Weak::upgrade).or_else(|| frames.values().filter_map(Weak::upgrade).find(|f| f.area == w))
        });
        // A click caught by an editor area's grab froze the pointer: pass it on to the editor, whatever became of the
        // frame (a no-op for clicks on the frame itself).
        let replay = match &e {
            Event::ButtonPress(b) => Some(b.time),
            _ => None,
        };
        if let Some(f) = frame {
            f.event(e);
        }
        if let Some(t) = replay {
            let _ = conn.c.allow_events(xproto::Allow::REPLAY_POINTER, t);
            let _ = conn.c.flush();
        }
    }
}

/// The main window gained or lost the keyboard focus: the frames follow (see the module notes).
pub fn main_focused(focused: bool) {
    MAIN_FOCUSED.with(|m| m.set(focused));
    if let Some(conn) = CONN.with(|c| c.borrow().clone()) {
        keep_above(&conn);
    }
}

/// While held, the frames are not kept above other windows (see the module notes).
pub struct Unpinned(());

/// A blocking dialog opens (the app waits for it on the UI thread): the frames step back until the guard drops.
pub fn unpinned() -> Unpinned {
    MODAL.with(|m| m.set(m.get() + 1));
    if let Some(conn) = CONN.with(|c| c.borrow().clone()) {
        keep_above(&conn);
    }
    Unpinned(())
}

impl Drop for Unpinned {
    fn drop(&mut self) {
        MODAL.with(|m| m.set(m.get().saturating_sub(1)));
        if let Some(conn) = CONN.with(|c| c.borrow().clone()) {
            keep_above(&conn);
        }
    }
}

/// Keeps the frames above other windows while Lemon Studio is the active application and no blocking dialog is open, and
/// only then.
fn keep_above(conn: &Conn) {
    let want = conn.owner.get() == 0 && MODAL.with(Cell::get) == 0 && (MAIN_FOCUSED.with(Cell::get) || conn.frame_active.get());
    if conn.above.replace(want) == want {
        return;
    }
    let a = &conn.atoms;
    // _NET_WM_STATE: 1 = add, 0 = remove; source 1 = an application.
    let data = [u32::from(want), a._NET_WM_STATE_ABOVE, 0, 1, 0];
    for &win in conn.frames.borrow().keys() {
        let m = ClientMessageEvent::new(32, win, a._NET_WM_STATE, data);
        let _ = conn.c.send_event(false, conn.root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, m);
    }
    let _ = conn.c.flush();
}

/// After the app's tick: repaints the frames that changed.
pub fn paint_dirty() {
    let Some(conn) = CONN.with(|c| c.borrow().clone()) else { return };
    let frames: Vec<Rc<Frame>> = conn.frames.borrow().values().filter_map(Weak::upgrade).collect();
    for f in frames {
        f.paint();
    }
}

/// A plugin window is open or a plugin editor is registered with the run loop: the app ticks at frame rate, so frames
/// and editors stay responsive.
pub fn busy() -> bool {
    CONN.with(|c| c.borrow().as_ref().is_some_and(|c| !c.frames.borrow().is_empty())) || os_host::runloop::active()
}

/// Puts `parent`'s docked windows `hwnds` (bottom to top) in that order when they are not already.
pub fn stack(parent: isize, hwnds: &[isize]) {
    let Some(conn) = CONN.with(|c| c.borrow().clone()) else { return };
    if hwnds.len() < 2 || parent == 0 {
        return;
    }
    let Some(tree) = conn.c.query_tree(parent as xproto::Window).ok().and_then(|t| t.reply().ok()) else { return };
    // The children's current order, bottom to top.
    let now: Vec<isize> = tree.children.iter().map(|&w| w as isize).filter(|w| hwnds.contains(w)).collect();
    if now == hwnds {
        return;
    }
    for &h in hwnds {
        let _ = conn.c.configure_window(h as xproto::Window, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE));
    }
    let _ = conn.c.flush();
}

/// Gives the keyboard back to the main window when a plugin editor holds it, or nobody does. A plugin takes it with a
/// click (for its text fields) and keeps it: X11 does not move it back when the user clicks elsewhere in the app (the
/// window manager sees the same active window), nor when the plugin's window closes (it goes to nobody), and the
/// typing keyboard would stay silent. Called on clicks in the main window and on frames, and when a frame closes.
pub fn focus_main() {
    let Some(conn) = CONN.with(|c| c.borrow().clone()) else { return };
    let owner = conn.owner.get();
    if owner == 0 {
        return;
    }
    let Some(focus) = conn.c.get_input_focus().ok().and_then(|r| r.reply().ok()).map(|r| r.focus) else { return };
    // 0: nobody; 1: PointerRoot (whatever window is under the pointer).
    if focus == owner || !(focus <= 1 || in_frame(&conn, focus)) {
        return;
    }
    let _ = conn.c.set_input_focus(xproto::InputFocus::PARENT, owner, x11rb::CURRENT_TIME);
    let _ = conn.c.flush();
}

/// Whether `w` is one of the frames or inside one (a plugin's editor).
fn in_frame(conn: &Conn, mut w: xproto::Window) -> bool {
    for _ in 0..32 {
        if conn.frames.borrow().contains_key(&w) {
            return true;
        }
        match conn.c.query_tree(w).ok().and_then(|t| t.reply().ok()) {
            Some(t) if t.parent != x11rb::NONE && t.parent != t.root => w = t.parent,
            _ => return false,
        }
    }
    false
}

/// Screen position for a new plugin window: right of the main window's left edge, below its toolbar (physical px; the
/// offset is logical, so the window lands on the same part of the layout at every scale). The window manager places
/// detached windows.
pub fn default_position(owner: isize, scale: f32) -> (i32, i32) {
    let Ok(conn) = conn() else { return (0, 0) };
    if owner == 0 {
        return (0, 0);
    }
    let (x, y) = to_root(&conn, owner as xproto::Window, (0, 0));
    (x + (420.0 * scale).round() as i32, y + (160.0 * scale).round() as i32)
}
