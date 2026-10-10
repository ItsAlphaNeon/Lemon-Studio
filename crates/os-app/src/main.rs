//! OpenStudio: the application window.

mod app;
mod browser;
mod icons;
mod knobs;
mod logo;
mod monitor;
mod piano;
mod patterns;
mod playlist;
#[cfg(windows)]
mod plugin_window;
mod plugins;
mod transport;
mod views;

use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::ModifiersState;
use winit::window::{CursorIcon, ResizeDirection, Window, WindowId};

use app::App;
use os_ui::Canvas;

struct Shell {
    app: App,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    canvas: Option<Canvas>,
    mods: ModifiersState,
    last_click: Instant,
    /// While a scroll-bar right-drag runs: the physical cursor position it is held at, and the
    /// virtual (logical) mouse position fed to the app.
    lock: Option<((f64, f64), (f32, f32))>,
    cursor: app::Cursor,
    /// Main-window border in device pixels (left, top, right, bottom); zero when maximized.
    inset: (u32, u32, u32, u32),
    /// The border edge under the mouse (drags resize the OS window).
    edge: Option<ResizeDirection>,
    /// A title-bar move or edge resize done by hand: cursor and window rect (screen px) at the press, and the edge
    /// (None = move). The system's own move / size loops block the event loop, which froze all drawing.
    grab: Option<Grab>,
}

#[derive(Clone, Copy)]
struct Grab {
    cursor: (i32, i32),
    rect: (i32, i32, i32, i32),
    edge: Option<ResizeDirection>,
}

/// Cursor position and outer window rect (left, top, right, bottom) in screen pixels.
#[cfg(windows)]
fn screen_state(window: &Window) -> Option<((i32, i32), (i32, i32, i32, i32))> {
    use windows_sys::Win32::Foundation::{POINT, RECT};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetWindowRect};
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let RawWindowHandle::Win32(h) = window.window_handle().ok()?.as_raw() else { return None };
    let (mut p, mut r): (POINT, RECT) = unsafe { std::mem::zeroed() };
    unsafe {
        GetCursorPos(&mut p);
        GetWindowRect(h.hwnd.get() as _, &mut r);
    }
    Some(((p.x, p.y), (r.left, r.top, r.right, r.bottom)))
}

#[cfg(not(windows))]
fn screen_state(_: &Window) -> Option<((i32, i32), (i32, i32, i32, i32))> {
    None
}

/// Move / resize the window to `rect` (left, top, right, bottom) in one call.
#[cfg(windows)]
fn set_rect(window: &Window, r: (i32, i32, i32, i32)) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER};
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    if let Ok(h) = window.window_handle() {
        if let RawWindowHandle::Win32(h) = h.as_raw() {
            unsafe { SetWindowPos(h.hwnd.get() as _, std::ptr::null_mut(), r.0, r.1, r.2 - r.0, r.3 - r.1, SWP_NOZORDER | SWP_NOACTIVATE) };
        }
    }
}

#[cfg(not(windows))]
fn set_rect(_: &Window, _: (i32, i32, i32, i32)) {}

impl Grab {
    /// The window rect for the cursor at `cursor`, never below `min` (physical px).
    fn rect_at(&self, cursor: (i32, i32), min: (i32, i32)) -> (i32, i32, i32, i32) {
        use ResizeDirection::*;
        let (dx, dy) = (cursor.0 - self.cursor.0, cursor.1 - self.cursor.1);
        let (mut l, mut t, mut r, mut b) = self.rect;
        let Some(e) = self.edge else { return (l + dx, t + dy, r + dx, b + dy) };
        if matches!(e, West | NorthWest | SouthWest) {
            l = (l + dx).min(r - min.0);
        }
        if matches!(e, East | NorthEast | SouthEast) {
            r = (r + dx).max(l + min.0);
        }
        if matches!(e, North | NorthWest | NorthEast) {
            t = (t + dy).min(b - min.1);
        }
        if matches!(e, South | SouthWest | SouthEast) {
            b = (b + dy).max(t + min.1);
        }
        (l, t, r, b)
    }
}

/// Border of the restored main window (spec ui/main-window.md §12), logical px: left, top, right, bottom.
const BORDER: (f32, f32, f32, f32) = (6.0, 2.0, 6.0, 6.0);
/// Depth of the resize zone along the top edge and the size of the corner squares (logical px).
const TOP_GRIP: f32 = 4.0;
const CORNER: f32 = 14.0;

fn insets(window: &Window) -> (u32, u32, u32, u32) {
    if window.is_maximized() || window.fullscreen().is_some() {
        return (0, 0, 0, 0);
    }
    let k = window.scale_factor() as f32;
    let d = |v: f32| (v * k).round() as u32;
    (d(BORDER.0), d(BORDER.1), d(BORDER.2), d(BORDER.3))
}

/// Draw the border into the frame buffer around the canvas (§12): 1 px outline, a 1 px highlight
/// along the top, the frame, and a 1 px inner line at the left, right and bottom of the canvas.
fn draw_border(buf: &mut [u32], w: usize, h: usize, (l, t, r, b): (u32, u32, u32, u32)) {
    if l == 0 || t == 0 || w < 2 * (l + r) as usize || h < 2 * (t + b) as usize {
        return;
    }
    let (l, t, r, b) = (l as usize, t as usize, r as usize, b as usize);
    let mut rect = |x0: usize, y0: usize, x1: usize, y1: usize, c: u32| {
        for y in y0.min(h)..y1.min(h) {
            buf[y * w + x0.min(w)..y * w + x1.min(w)].fill(c);
        }
    };
    let (frame, outline, light, inner) = (0x2e3234, 0x202426, 0x393d3f, 0x2a2d2f);
    rect(0, 0, w, t, frame);
    rect(0, h - b, w, h, frame);
    rect(0, 0, l, h, frame);
    rect(w - r, 0, w, h, frame);
    if t > 1 {
        rect(1, 1, w - 1, 2, light);
    }
    rect(l - 1, t, l, h - b + 1, inner);
    rect(w - r, t, w - r + 1, h - b + 1, inner);
    rect(l - 1, h - b, w - r + 1, h - b + 1, inner);
    rect(0, 0, w, 1, outline);
    rect(0, h - 1, w, h, outline);
    rect(0, 0, 1, h, outline);
    rect(w - 1, 0, w, h, outline);
}

impl Shell {
    /// Show the cursor the app wants (resize arrows over edges and note ends).
    fn sync_cursor(&mut self, window: &Window) {
        let c = match self.edge {
            Some(ResizeDirection::East | ResizeDirection::West) => app::Cursor::EwResize,
            Some(ResizeDirection::North | ResizeDirection::South) => app::Cursor::NsResize,
            Some(ResizeDirection::NorthWest | ResizeDirection::SouthEast) => app::Cursor::NwseResize,
            Some(_) => app::Cursor::NeswResize,
            None => self.app.cursor(),
        };
        if c != self.cursor {
            self.cursor = c;
            window.set_cursor(match c {
                app::Cursor::Default => CursorIcon::Default,
                app::Cursor::EwResize => CursorIcon::EwResize,
                app::Cursor::NsResize => CursorIcon::NsResize,
                app::Cursor::NwseResize => CursorIcon::NwseResize,
                app::Cursor::NeswResize => CursorIcon::NeswResize,
            });
        }
    }

    /// The resize edge at physical position (x, y) of the restored window, if any.
    fn edge_at(&self, window: &Window, x: f64, y: f64) -> Option<ResizeDirection> {
        if self.inset == (0, 0, 0, 0) {
            return None;
        }
        let size = window.inner_size();
        let (w, h) = (f64::from(size.width), f64::from(size.height));
        let k = window.scale_factor();
        let (l, r, b) = (f64::from(self.inset.0), f64::from(self.inset.2), f64::from(self.inset.3));
        let t = f64::from(TOP_GRIP) * k;
        let c = f64::from(CORNER) * k;
        let (left, right, top, bottom) = (x < l, x >= w - r, y < t, y >= h - b);
        let (near_l, near_r, near_t, near_b) = (x < c, x >= w - c, y < c, y >= h - c);
        use ResizeDirection::*;
        Some(if (top && near_l) || (left && near_t) {
            NorthWest
        } else if (top && near_r) || (right && near_t) {
            NorthEast
        } else if (bottom && near_l) || (left && near_b) {
            SouthWest
        } else if (bottom && near_r) || (right && near_b) {
            SouthEast
        } else if left {
            West
        } else if right {
            East
        } else if top {
            North
        } else if bottom {
            South
        } else {
            return None;
        })
    }

    /// Logical app coordinates of a physical window position (the canvas sits inside the border).
    fn to_app(&self, window: &Window, x: f64, y: f64) -> (f32, f32) {
        let k = window.scale_factor();
        (((x - f64::from(self.inset.0)) / k) as f32, ((y - f64::from(self.inset.1)) / k) as f32)
    }
}

impl ApplicationHandler for Shell {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("OpenStudio")
            .with_decorations(false)
            .with_inner_size(LogicalSize::new(1920.0, 1080.0))
            // Layouts are measured at 1920x1080 and are not responsive yet; below this they break.
            .with_min_inner_size(LogicalSize::new(1024.0, 600.0))
            .with_maximized(true);
        // App icon: resource 1, embedded by build.rs from assets/openstudio.ico.
        #[cfg(windows)]
        let attrs = {
            use winit::dpi::PhysicalSize;
            use winit::platform::windows::{IconExtWindows, WindowAttributesExtWindows};
            use winit::window::Icon;
            let icon = |s: u32| Icon::from_resource(1, Some(PhysicalSize::new(s, s))).ok();
            attrs.with_window_icon(icon(16)).with_taskbar_icon(icon(32))
        };
        let window = Rc::new(el.create_window(attrs).expect("create window"));
        let context = softbuffer::Context::new(window.clone()).expect("graphics context");
        self.surface = Some(softbuffer::Surface::new(&context, window.clone()).expect("surface"));
        // Hosted plugins: windows are owned by the main window; scanning loads the .clap files found.
        #[cfg(windows)]
        {
            use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(h) = window.window_handle() {
                if let RawWindowHandle::Win32(w) = h.as_raw() {
                    self.app.plugins.owner = w.hwnd.get();
                }
            }
        }
        self.app.plugins.scale = window.scale_factor() as f32;
        self.app.scan_plugins();
        self.window = Some(window);
        el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(16)));
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        // Animate while playing; otherwise repaint a few times a second (peaks, clock).
        let ms = if self.app.animating() { 16 } else { 100 };
        el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(ms)));
        if self.app.tick() {
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
    }

    /// Raw mouse motion drives a locked scroll-bar drag; the hidden cursor is held at the anchor.
    fn device_event(&mut self, _el: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        let (DeviceEvent::MouseMotion { delta }, Some((anchor, virt)), Some(window)) = (event, self.lock, self.window.clone()) else { return };
        let scale = window.scale_factor();
        let v = (virt.0 + (delta.0 / scale) as f32, virt.1 + (delta.1 / scale) as f32);
        self.lock = Some((anchor, v));
        self.app.mouse_move(v.0, v.1);
        let _ = window.set_cursor_position(winit::dpi::PhysicalPosition::new(anchor.0, anchor.1));
        window.request_redraw();
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(window) = self.window.clone() else { return };
        let scale = window.scale_factor() as f32;
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::ModifiersChanged(m) => {
                self.mods = m.state();
                self.app.ctrl = self.mods.control_key();
                self.app.shift = self.mods.shift_key();
                self.app.alt = self.mods.alt_key();
            }
            WindowEvent::CursorMoved { position, .. } => {
                // Locked: turn the offset from the anchor into virtual movement, then warp back.
                // Locked: movement comes from raw motion (`device_event`); positions are ignored,
                // since hiding the cursor makes Windows report a jump to the window centre.
                if self.lock.is_some() {
                    return;
                }
                if let (Some(g), Some((cursor, _))) = (self.grab, screen_state(&window)) {
                    let k = window.scale_factor();
                    let min = ((1024.0 * k) as i32, (600.0 * k) as i32);
                    set_rect(&window, g.rect_at(cursor, min));
                    return;
                }
                self.edge = if self.app.drag.is_none() { self.edge_at(&window, position.x, position.y) } else { None };
                let (mx, my) = self.to_app(&window, position.x, position.y);
                self.app.mouse_move(mx, my);
                self.sync_cursor(&window);
                if self.app.has_damage() {
                    window.request_redraw();
                }
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                if state == ElementState::Pressed {
                    if let Some(dir) = self.edge {
                        match screen_state(&window) {
                            Some((cursor, rect)) => self.grab = Some(Grab { cursor, rect, edge: Some(dir) }),
                            None => {
                                let _ = window.drag_resize_window(dir);
                            }
                        }
                        return;
                    }
                    let double = self.last_click.elapsed() < Duration::from_millis(350);
                    self.last_click = Instant::now();
                    self.app.mouse_down();
                    if self.app.cursor_locked() {
                        let (mx, my) = self.app.mouse;
                        let anchor = (f64::from(mx * scale) + f64::from(self.inset.0), f64::from(my * scale) + f64::from(self.inset.1));
                        self.lock = Some((anchor, (mx, my)));
                        window.set_cursor_visible(false);
                    }
                    if self.app.want_drag_window {
                        self.app.want_drag_window = false;
                        if double {
                            window.set_maximized(!window.is_maximized());
                        } else if window.is_maximized() {
                            // Dragging a maximized window restores it under the cursor, then moves it.
                            if let Some(((cx, cy), (l, _, r, _))) = screen_state(&window) {
                                let frac = f64::from(cx - l) / f64::from((r - l).max(1));
                                let grab_y = (self.app.mouse.1 * scale) as i32 + self.inset.1 as i32;
                                window.set_maximized(false);
                                if let Some((_, (nl, nt, nr, nb))) = screen_state(&window) {
                                    let w = nr - nl;
                                    let left = cx - (frac * f64::from(w)) as i32;
                                    let rect = (left, cy - grab_y, left + w, cy - grab_y + (nb - nt));
                                    set_rect(&window, rect);
                                    self.grab = Some(Grab { cursor: (cx, cy), rect, edge: None });
                                }
                            }
                        } else {
                            match screen_state(&window) {
                                Some((cursor, rect)) => self.grab = Some(Grab { cursor, rect, edge: None }),
                                None => {
                                    let _ = window.drag_window();
                                }
                            }
                        }
                    }
                } else {
                    self.grab = None;
                    if let Some((anchor, _)) = self.lock.take() {
                        let _ = window.set_cursor_position(winit::dpi::PhysicalPosition::new(anchor.0, anchor.1));
                        window.set_cursor_visible(true);
                    }
                    self.app.mouse_up();
                    if self.app.resize_cursor && self.app.drag.is_none() {
                        self.app.resize_cursor = false;
                    }
                    self.sync_cursor(&window);
                }
                window.request_redraw();
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Right, .. } => {
                self.app.mouse_down_right();
                if self.app.cursor_locked() {
                    let (mx, my) = self.app.mouse;
                    let anchor = (f64::from(mx * scale) + f64::from(self.inset.0), f64::from(my * scale) + f64::from(self.inset.1));
                    self.lock = Some((anchor, (mx, my)));
                    window.set_cursor_visible(false);
                }
                window.request_redraw();
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Middle, .. } => {
                self.app.mouse_down_middle();
                window.request_redraw();
            }
            WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Right | MouseButton::Middle, .. } => {
                if let Some((anchor, _)) = self.lock.take() {
                    // Release where the drag started, as if the cursor never moved.
                    let _ = window.set_cursor_position(winit::dpi::PhysicalPosition::new(anchor.0, anchor.1));
                    window.set_cursor_visible(true);
                }
                self.app.mouse_up();
                window.request_redraw();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x, y),
                    MouseScrollDelta::PixelDelta(p) => (p.x as f32 / 40.0, p.y as f32 / 40.0),
                };
                self.app.wheel(dx, dy, self.mods.shift_key(), self.mods.control_key());
                window.request_redraw();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
                // The typing keyboard plays notes by physical key (spec: ui/typing-keyboard.md).
                if let winit::keyboard::PhysicalKey::Code(code) = event.physical_key {
                    if self.app.typing_key(code, pressed, self.mods.control_key(), self.mods.alt_key()) {
                        if self.app.has_damage() {
                            window.request_redraw();
                        }
                        return;
                    }
                }
                if pressed && !event.repeat {
                    self.app.key(&event.logical_key, self.mods.control_key(), self.mods.alt_key());
                    window.request_redraw();
                }
            }
            // Audio files dropped on the playlist become audio clips (spec: ui/playlist.md §9); a dropped project opens.
            WindowEvent::DroppedFile(path) => {
                // No mouse moves arrive during a drag and drop: take the cursor position now.
                if let (Some(((cx, cy), _)), Ok(p)) = (screen_state(&window), window.inner_position()) {
                    let (mx, my) = self.to_app(&window, f64::from(cx - p.x), f64::from(cy - p.y));
                    self.app.mouse_move(mx, my);
                }
                self.app.drop_file(path);
                window.request_redraw();
            }
            WindowEvent::HoveredFile(_) => {}
            WindowEvent::Focused(false) => {
                self.grab = None;
                self.app.typing_release_all();
                window.request_redraw();
            }
            WindowEvent::Resized(_) => {
                self.inset = insets(&window);
                self.app.invalidate();
                window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                let size = window.inner_size();
                let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else { return };
                let inset = insets(&window);
                if inset != self.inset {
                    self.inset = inset;
                    self.app.invalidate();
                }
                // The canvas fills the window inside the border.
                let cw = size.width.saturating_sub(inset.0 + inset.2).max(1);
                let ch = size.height.saturating_sub(inset.1 + inset.3).max(1);
                let off = (inset.0 as usize, inset.1 as usize);
                let stride = size.width as usize;
                if self.canvas.is_none() {
                    match Canvas::new(cw, ch, scale) {
                        Ok(c) => self.canvas = Some(c),
                        Err(e) => {
                            eprintln!("openstudio: {e}");
                            el.exit();
                            return;
                        }
                    }
                }
                let canvas = self.canvas.as_mut().expect("canvas");
                if canvas.pixmap.width() != cw || canvas.pixmap.height() != ch || canvas.scale != scale {
                    canvas.resize(cw, ch, scale);
                    self.app.invalidate();
                }
                // Paint only what changed, then present only that region.
                let damage = self.app.take_damage();
                if let Some(d) = damage {
                    self.app.paint(canvas, d);
                }
                if let Some(surface) = self.surface.as_mut() {
                    if surface.resize(w, h).is_ok() {
                        if let Ok(mut buf) = surface.buffer_mut() {
                            let age = buf.age();
                            match damage {
                                Some(Some(r)) if age > 0 => {
                                    let px = canvas.device_rect(r);
                                    canvas.copy_region_to_xrgb_at(&mut buf, stride, px, off);
                                    match (NonZeroU32::new(px.2 as u32), NonZeroU32::new(px.3 as u32)) {
                                        (Some(rw), Some(rh)) => {
                                            let (x, y) = ((px.0 + off.0) as u32, (px.1 + off.1) as u32);
                                            let _ = buf.present_with_damage(&[softbuffer::Rect { x, y, width: rw, height: rh }]);
                                        }
                                        _ => {
                                            let _ = buf.present();
                                        }
                                    }
                                }
                                // Nothing painted (the OS asked for a redraw): the buffer already
                                // holds the last frame unless it was discarded.
                                None => {
                                    if age == 0 {
                                        canvas.copy_region_to_xrgb_at(&mut buf, stride, (0, 0, cw as usize, ch as usize), off);
                                        draw_border(&mut buf, stride, size.height as usize, inset);
                                    }
                                    let _ = buf.present();
                                }
                                _ => {
                                    canvas.copy_region_to_xrgb_at(&mut buf, stride, (0, 0, cw as usize, ch as usize), off);
                                    draw_border(&mut buf, stride, size.height as usize, inset);
                                    let _ = buf.present();
                                }
                            }
                        }
                    }
                }
                if self.app.want_minimize {
                    self.app.want_minimize = false;
                    window.set_minimized(true);
                }
                if self.app.want_maximize {
                    self.app.want_maximize = false;
                    window.set_maximized(!window.is_maximized());
                }
                if self.app.want_close {
                    el.exit();
                }
            }
            _ => {}
        }
    }
}

/// `openstudio --screenshot out.png [--view playlist|mixer|piano|none] [--pattern N] [--channel I] [--no-rack] [--browser-tab N] [--browser-menu] [--browser-search TEXT] [project.flp]`:
/// paint one 1920×1080 frame offscreen (used to compare with reference measurements).
fn screenshot(args: &[String]) {
    let mut out = None;
    let mut view = "playlist".to_string();
    let mut rack = true;
    let mut browser = true;
    let mut rack_w: Option<f32> = None;
    let mut rack_roll = false;
    let mut browser_tab: Option<usize> = None;
    let mut browser_menu = false;
    let mut browser_search: Option<String> = None;
    let mut menu: Option<usize> = None;
    let mut picker: Option<(bool, usize)> = None;
    let mut picker_open = false;
    let mut lane: Option<usize> = None;
    let mut lane_menu = false;
    let mut pans: Option<Vec<f32>> = None;
    let mut path = None;
    let mut mouse: Option<(f32, f32)> = None;
    let mut pattern = None;
    let mut channel = None;
    let mut win_rect: Option<os_ui::Rect> = None;
    let mut theme: Option<std::path::PathBuf> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--screenshot" => out = it.next().cloned(),
            "--theme" => theme = it.next().map(std::path::PathBuf::from),
            "--view" => view = it.next().cloned().unwrap_or_default(),
            "--no-rack" => rack = false,
            "--no-browser" => browser = false,
            "--rack-roll" => rack_roll = true,
            "--browser-menu" => browser_menu = true,
            "--browser-search" => browser_search = it.next().cloned(),
            "--browser-tab" => browser_tab = it.next().and_then(|v| v.parse().ok()),
            "--picker" => {
                picker_open = true;
                if let Some(v) = it.next() {
                    // "g2" / "e4": filter by that category
                    let (k, n) = v.split_at(1);
                    picker = n.parse().ok().map(|n| (k == "e", n));
                }
            }
            // Hover at x,y before the frame is captured.
            "--mouse" => {
                let v: Vec<f32> = it.next().map(|v| v.split(',').filter_map(|n| n.parse().ok()).collect()).unwrap_or_default();
                if let [x, y] = v[..] {
                    mouse = Some((x, y));
                }
            }
            "--lane" => lane = it.next().and_then(|v| v.parse().ok()),
            "--pans" => pans = it.next().map(|v| v.split(',').filter_map(|p| p.parse().ok()).collect()),
            "--lane-menu" => lane_menu = true,
            "--menu" => menu = it.next().and_then(|v| v.parse().ok()),
            "--rack-width" => rack_w = it.next().and_then(|v| v.parse().ok()),
            "--pattern" => pattern = it.next().and_then(|v| v.parse().ok()),
            "--channel" => channel = it.next().and_then(|v| v.parse().ok()),
            // Float the shown window at x,y,w,h instead of maximizing it.
            "--win" => {
                let v: Vec<f32> = it.next().map(|v| v.split(',').filter_map(|n| n.parse().ok()).collect()).unwrap_or_default();
                if let [x, y, w, h] = v[..] {
                    win_rect = Some(os_ui::Rect::new(x, y, w, h));
                }
            }
            other => path = Some(std::path::PathBuf::from(other)),
        }
    }
    let mut app = App::new(path);
    if let Some(t) = theme {
        app.load_theme_file_ex(&t, false);
    }
    if let Some(p) = pattern {
        app.state.pattern = p;
    }
    if let Some(c) = channel {
        app.state.selected_channel = c;
    }
    app.state.show_browser = browser;
    if let Some(t) = browser_tab {
        app.state.browser_tab = t.min(4);
    }
    if let Some(q) = browser_search {
        app.state.browser_search = q;
        app.state.browser_search_focus = true;
    }
    if browser_menu {
        app.act(app::Action::BrowserMenu);
    }
    app.state.z = match view.as_str() {
        "mixer" => vec![app::Win::Mixer],
        "piano" => vec![app::Win::PianoRoll],
        "none" => vec![],
        _ => vec![app::Win::Playlist],
    };
    if rack {
        app.state.z.push(app::Win::ChannelRack);
    }
    app.state.rack_roll_view = rack_roll;
    app.state.menu_open = menu;
    app.state.picker_open = picker_open;
    app.state.lane_menu = lane_menu;
    // "--pans 0.2,0.8": mixer pan knob values for inserts 1.. (normalised), to check the value arcs.
    for (i, v) in pans.iter().flatten().enumerate() {
        app.state.set_knob(knobs::KnobId::MixPan(i as u16 + 1), *v);
    }
    if let Some(i) = lane {
        app.state.piano.lane = piano::Lane::ALL[i.min(piano::Lane::ALL.len() - 1)];
    }
    app.state.picker_cat = picker;
    if let Some(w) = rack_w {
        app.state.rack_size.0 = w;
    }
    // The reference views fill the workspace unless `--win` floats the window.
    if let Some(w) = app.state.z.first().copied().filter(|w| w.slot().is_some()) {
        let ws = &mut app.state.wins[w.slot().unwrap_or(0)];
        match win_rect {
            Some(r) => ws.rect = r,
            None => ws.maxed = true,
        }
    }
    if app.state.shown(app::Win::PianoRoll) {
        piano::center_keys(&mut app.state, 744.0);
    }
    let mut canvas = Canvas::new(1920, 1080, 1.0).expect("canvas");
    app.paint(&mut canvas, None);
    if let Some((x, y)) = mouse {
        app.mouse_move(x, y);
        app.paint(&mut canvas, None);
    }
    if std::env::var_os("OS_BENCH").is_some() {
        let t0 = Instant::now();
        for _ in 0..20 {
            app.paint(&mut canvas, None);
        }
        eprintln!("paint: {:.2} ms/frame", t0.elapsed().as_secs_f64() * 50.0);
        for (name, r) in [
            ("rack-sized damage", os_ui::Rect::new(317.0, 195.0, 510.0, 260.0)),
            ("fader-column damage", os_ui::Rect::new(317.0, 500.0, 50.0, 500.0)),
            ("hover-button damage", os_ui::Rect::new(975.0, 45.0, 36.0, 36.0)),
        ] {
            let t1 = Instant::now();
            for _ in 0..20 {
                app.paint(&mut canvas, Some(r));
            }
            eprintln!("  {name}: {:.2} ms/frame", t1.elapsed().as_secs_f64() * 50.0);
        }
    }
    canvas.pixmap.save_png(out.expect("--screenshot needs a path")).expect("write png");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--screenshot") {
        screenshot(&args);
        return;
    }
    let path = args.first().map(std::path::PathBuf::from);
    let event_loop = EventLoop::new().expect("event loop");
    let mut shell = Shell {
        app: App::new(path),
        window: None,
        surface: None,
        canvas: None,
        mods: ModifiersState::default(),
        last_click: Instant::now() - Duration::from_secs(10),
        lock: None,
        cursor: app::Cursor::Default,
        inset: (0, 0, 0, 0),
        edge: None,
        grab: None,
    };
    event_loop.run_app(&mut shell).expect("run");
}
