//! Plugin windows on Linux: the frame (`super`) in an X11 window, always top-level. Plugin editors on Linux embed only in
//! X11 windows (CLAP `x11`, VST3 `X11EmbedWindowID`), so under Wayland the frame is an XWayland window; an X11 window
//! cannot be a child of the Wayland main window, so it is never docked. Under X11 it is transient for the main window.
//!
//! One connection serves every frame (each plugin opens its own for its editor). The app reads its events on the UI tick
//! (`pump`) and repaints what changed after handling them (`paint_dirty`).
//!
//! On Windows a detached frame is owned by the main window, so it stays above it. An X11 window cannot be owned by a
//! Wayland one, so instead the frames are kept above other windows while Lemon Studio is the active application (the
//! main window has the focus, or a frame is the active window) and stacked normally otherwise (`main_focused`).
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
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use super::*;

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        WM_PROTOCOLS,
        WM_DELETE_WINDOW,
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
}

thread_local! {
    static CONN: RefCell<Option<Rc<Conn>>> = const { RefCell::new(None) };
    /// The main window has the keyboard focus (`main_focused`), known before the connection opens.
    static MAIN_FOCUSED: Cell<bool> = const { Cell::new(true) };
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
    let above = MAIN_FOCUSED.with(Cell::get);
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
    });
    CONN.with(|c| *c.borrow_mut() = Some(conn.clone()));
    Ok(conn)
}

/// A plugin window's native parts.
struct Frame {
    conn: Rc<Conn>,
    win: xproto::Window,
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
    /// A hidden frame for an editor of `editor` logical px; `raise` shows it. The window manager places it (transient for
    /// `owner`, the main window, when that is an X11 window too). The scale is the X server's (`Xft.dpi`), not
    /// `_scale`: under Wayland the main window's scale does not apply to X11 windows.
    pub fn new(owner: isize, info: ChannelInfo, editor: (f32, f32), _scale: f32, _pos: (i32, i32), _detached: bool) -> Result<FrameWindow, String> {
        let conn = conn()?;
        let fail = |e: &dyn std::fmt::Display| format!("could not create the plugin window: {e}");
        let k = conn.scale;
        let state = State::new(info, editor, true);
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
            c.create_window(x11rb::COPY_DEPTH_FROM_PARENT, win, conn.root, 0, 0, px(w, k), px(h, k), 0, WindowClass::INPUT_OUTPUT, x11rb::COPY_FROM_PARENT, &CreateWindowAux::new().event_mask(events))?;
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
            c.change_property32(PropMode::REPLACE, win, a.WM_PROTOCOLS, AtomEnum::ATOM, &[a.WM_DELETE_WINDOW])?;
            c.change_property32(PropMode::REPLACE, win, a._NET_WM_PID, AtomEnum::CARDINAL, &[std::process::id()])?;
            // No decorations: the frame draws its own title bar (flags: decorations; decorations: none).
            c.change_property32(PropMode::REPLACE, win, a._MOTIF_WM_HINTS, a._MOTIF_WM_HINTS, &[2, 0, 0, 0, 0])?;
            if let Some(icon) = icon() {
                c.change_property32(PropMode::REPLACE, win, a._NET_WM_ICON, AtomEnum::CARDINAL, &icon)?;
            }
            if owner != 0 {
                c.change_property32(PropMode::REPLACE, win, AtomEnum::WM_TRANSIENT_FOR, AtomEnum::WINDOW, &[owner as u32])?;
            }
            // Opened from the active application: it starts above (taken when the window is mapped).
            if conn.above.get() {
                c.change_property32(PropMode::REPLACE, win, a._NET_WM_STATE, AtomEnum::ATOM, &[a._NET_WM_STATE_ABOVE])?;
            }
            fixed_size(c, win, px(w, k), px(h, k))?;
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

    /// Always: an X11 window is never a child of the main window.
    pub fn detached(&self) -> bool {
        true
    }

    /// Docking is not possible here (see `detached`).
    pub fn set_detached(&self, _owner: isize, _on: bool) {}

    /// Docked windows only.
    pub fn clip_to(&self, _work: [i32; 4], _covers: &[[i32; 4]]) {}

    /// The native frame window (an X11 window id).
    pub fn hwnd(&self) -> isize {
        self.f.win as isize
    }

    /// The child window the plugin's editor goes into (an X11 window id, as plugin APIs pass it).
    pub fn area(&self) -> *mut c_void {
        self.f.area as usize as *mut c_void
    }

    /// Shows the window and brings it to the front.
    pub fn raise(&self) {
        let (c, a, win) = (&self.f.conn.c, &self.f.conn.atoms, self.f.win);
        let _ = c.map_window(win);
        let _ = c.configure_window(win, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE));
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
        f.conn.frames.borrow_mut().remove(&f.win);
        let _ = f.conn.c.free_gc(f.gc);
        let _ = f.conn.c.destroy_window(f.win);
        let _ = f.conn.c.flush();
    }
}

/// The window manager keeps the window at this size (the frame follows its editor, the user does not resize it).
fn fixed_size(c: &RustConnection, win: xproto::Window, w: u16, h: u16) -> Result<(), Box<dyn std::error::Error>> {
    let mut hints = WmSizeHints::new();
    hints.min_size = Some((i32::from(w), i32::from(h)));
    hints.max_size = Some((i32::from(w), i32::from(h)));
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
        let order = |p: &u32| if self.conn.lsb_first { p.to_le_bytes() } else { p.to_be_bytes() };
        // Bands of whole rows, each small enough for one request (the request header takes a few bytes).
        let rows = ((self.conn.max_request.saturating_sub(64)) / (w * 4).max(1)).clamp(1, h.max(1));
        for (i, band) in xrgb.chunks(w * rows).enumerate() {
            let bytes: Vec<u8> = band.iter().flat_map(order).collect();
            let bh = (band.len() / w) as u16;
            let y = (i * rows) as i16;
            let _ = self.conn.c.put_image(ImageFormat::Z_PIXMAP, self.win, self.gc, w as u16, bh, 0, y, 0, self.conn.depth, &bytes);
        }
        let _ = self.conn.c.flush();
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
            Reply::Move => {
                // The window manager moves it (it needs the pointer the press grabbed).
                let _ = c.ungrab_pointer(x11rb::CURRENT_TIME);
                let data = [root.0 as u32, root.1 as u32, MOVERESIZE_MOVE, 1, 1];
                let _ = c.send_event(false, self.conn.root, to_root, ClientMessageEvent::new(32, self.win, a._NET_WM_MOVERESIZE, data));
                let _ = c.flush();
            }
            Reply::Minimize => {
                let _ = c.send_event(false, self.conn.root, to_root, ClientMessageEvent::new(32, self.win, a.WM_CHANGE_STATE, [ICONIC_STATE, 0, 0, 0, 0]));
                let _ = c.flush();
            }
        }
    }

    fn event(&self, e: Event) {
        let k = self.scale;
        let screen = |x: f32, y: f32| self.to_screen(x, y);
        match e {
            Event::Expose(_) => self.dirty.set(true),
            Event::ButtonPress(e) => {
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
            Event::MotionNotify(e) => {
                let fine = u16::from(e.state) & u16::from(KeyButMask::CONTROL) != 0;
                let r = self.state.borrow_mut().motion(f32::from(e.event_x) / k, f32::from(e.event_y) / k, fine);
                self.reply(r, (e.root_x, e.root_y));
            }
            Event::ClientMessage(e) if e.type_ == self.conn.atoms.WM_PROTOCOLS && e.data.as_data32()[0] == self.conn.atoms.WM_DELETE_WINDOW => {
                self.state.borrow_mut().events.push(FrameEvent::Close);
                os_clap::wake();
            }
            _ => {}
        }
    }
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
        let frame = window_of(&e).and_then(|w| conn.frames.borrow().get(&w).and_then(Weak::upgrade));
        if let Some(f) = frame {
            f.event(e);
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

/// Keeps the frames above other windows while Lemon Studio is the active application, and only then.
fn keep_above(conn: &Conn) {
    let want = MAIN_FOCUSED.with(Cell::get) || conn.frame_active.get();
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

/// Docked windows only (none here).
pub fn stack(_parent: isize, _hwnds: &[isize]) {}

/// The window manager places new windows.
pub fn default_position(_owner: isize, _scale: f32) -> (i32, i32) {
    (0, 0)
}
