// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The shell's side of Options ▸ Detached (Lemon Studio's design, spec: ui/mixer.md §6.2): each detached window
//! (`App::detached_windows`) gets a native window of its own, made and dropped on the main event loop as the app asks,
//! with its own surface and canvas. The app paints the window into it whole (`App::paint_detached`) and takes its
//! mouse input on that window's surface (`App::on_surface`); its keys go where the main window's do, acting on its
//! surface while it has the keyboard (`App::key_on`). Its title bar, buttons and edges move, minimize, maximize and
//! resize the native window (`App::native`); closing it closes the window as its close button does. A browser item
//! dragged out of the main window goes to the window under the cursor on screen (`Shell::cross_move`), and a knob or
//! scroll-bar drag in it holds the cursor there (`Shell::lock_on`).

use std::num::NonZeroU32;
use std::rc::Rc;

use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::{CursorIcon, ResizeDirection, Window};

use crate::app::{self, App, Native, Win, EDGE_B, EDGE_L, EDGE_R, EDGE_T};
use crate::{ctrl, frame_of, place_k, ui_scale, Shell};
use os_ui::Canvas;

/// A detached window's native window.
pub(crate) struct DetWin {
    pub win: Win,
    pub window: Rc<Window>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
    canvas: Option<Canvas>,
    cursor: app::Cursor,
    /// It was the app's top window (`State::z`) at the last sync: raised since (F9, VIEW), it comes to the front.
    top: bool,
}

/// The native window's title.
fn title(w: Win) -> &'static str {
    match w {
        Win::ChannelRack => "Channel rack",
        _ => "Mixer",
    }
}

/// The window system's resize for an app window edge mask (`app::EDGE_L` ...).
pub(crate) fn resize_direction(edges: u8) -> Option<ResizeDirection> {
    let (l, r, t, b) = (edges & EDGE_L != 0, edges & EDGE_R != 0, edges & EDGE_T != 0, edges & EDGE_B != 0);
    Some(match (l, r, t, b) {
        (true, false, true, false) => ResizeDirection::NorthWest,
        (false, true, true, false) => ResizeDirection::NorthEast,
        (true, false, false, true) => ResizeDirection::SouthWest,
        (false, true, false, true) => ResizeDirection::SouthEast,
        (true, false, false, false) => ResizeDirection::West,
        (false, true, false, false) => ResizeDirection::East,
        (false, false, true, false) => ResizeDirection::North,
        (false, false, false, true) => ResizeDirection::South,
        _ => return None,
    })
}

/// The surface under screen point `p` (physical px): the first of the detached windows' client rects `rects` (x, y, w,
/// h; topmost first) that holds it, else the main window's (`None`): the detached windows are above the main window.
pub(crate) fn surface_at(p: (i32, i32), rects: &[(Win, [i32; 4])]) -> Option<Win> {
    rects.iter().find(|(_, [x, y, w, h])| p.0 >= *x && p.0 < x + w && p.1 >= *y && p.1 < y + h).map(|d| d.0)
}

/// Opens `w`'s native window: where it was last (`App::detached_frame`), else over the place it had in the main window,
/// at the size it had there. It belongs to the main window where the window system lets it: owned by it on Windows
/// (above it, out of the taskbar, minimized with it), a child window of it on macOS (above it, and moved and minimized
/// with it), transient for it under X11 (`plugin_window::set_transient`: above it, minimized with it). Wayland has no
/// way to say it through winit (no parent for a top-level window): there it is a window of its own.
fn open(el: &ActiveEventLoop, main: &Window, app: &App, w: Win) -> Option<DetWin> {
    let k = ui_scale(main);
    let (lw, lh) = app.state.detached_size(w)?;
    let (mw, mh) = app.state.detached_min(w);
    let attrs = Window::default_attributes()
        .with_title(title(w))
        .with_decorations(false)
        .with_min_inner_size(PhysicalSize::new((f64::from(mw) * k).round() as u32, (f64::from(mh) * k).round() as u32));
    let attrs = match app.detached_frame(w) {
        Some([x, y, fw, fh]) => {
            let p = place_k(main.scale_factor());
            attrs
                .with_position(PhysicalPosition::new((f64::from(x) * p).round() as i32, (f64::from(y) * p).round() as i32))
                .with_inner_size(PhysicalSize::new((f64::from(fw) * p).round() as u32, (f64::from(fh) * p).round() as u32))
        }
        None => {
            let size = PhysicalSize::new((f64::from(lw) * k).round() as u32, (f64::from(lh) * k).round() as u32);
            let attrs = attrs.with_inner_size(size);
            match (main.inner_position(), app.detached_origin(w)) {
                (Ok(p), Some(r)) => attrs.with_position(PhysicalPosition::new(
                    p.x + (f64::from(r.x) * k).round() as i32,
                    p.y + (f64::from(r.y) * k).round() as i32,
                )),
                _ => attrs,
            }
        }
    };
    #[cfg(windows)]
    let attrs = {
        use winit::platform::windows::WindowAttributesExtWindows;
        if app.plugins.owner != 0 { attrs.with_owner_window(app.plugins.owner) } else { attrs }
    };
    #[cfg(target_os = "macos")]
    let attrs = {
        use winit::raw_window_handle::HasWindowHandle;
        match main.window_handle() {
            // SAFETY: the main window's live handle, used while the window is made.
            Ok(h) => unsafe { attrs.with_parent_window(Some(h.as_raw())) },
            Err(_) => attrs,
        }
    };
    // X11 (the main window's id is the owner; 0 under Wayland): a utility window, made transient before it shows.
    #[cfg(target_os = "linux")]
    let attrs = {
        use winit::platform::x11::{WindowAttributesExtX11, WindowType};
        if app.plugins.owner != 0 { attrs.with_x11_window_type(vec![WindowType::Utility]).with_visible(false) } else { attrs }
    };
    let window = Rc::new(el.create_window(attrs).ok()?);
    #[cfg(target_os = "linux")]
    if app.plugins.owner != 0 {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let id = match window.window_handle().map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Xlib(h)) => h.window as u32,
            Ok(RawWindowHandle::Xcb(h)) => h.window.get(),
            _ => 0,
        };
        crate::plugin_window::set_transient(id, app.plugins.owner);
        window.set_visible(true);
    }
    let context = softbuffer::Context::new(window.clone()).ok()?;
    let surface = softbuffer::Surface::new(&context, window.clone()).ok()?;
    Some(DetWin { win: w, window, surface, canvas: None, cursor: app::Cursor::Default, top: true })
}

impl Shell {
    /// Opens and drops the detached windows' native windows to match the app (`App::detached_windows`), and asks those
    /// with something new to show for a frame. A window that cannot be opened goes back into the main window.
    pub(crate) fn sync_detached(&mut self, el: &ActiveEventLoop) {
        let Some(main) = self.window.clone() else { return };
        let want = self.app.detached_windows();
        self.detached.retain(|d| want.contains(&d.win));
        for w in want {
            if self.detached.iter().any(|d| d.win == w) {
                continue;
            }
            match open(el, &main, &self.app, w) {
                Some(d) => self.detached.push(d),
                None => self.app.set_detached(w, false),
            }
        }
        let top = self.app.state.z.last().copied();
        for d in &mut self.detached {
            if top == Some(d.win) && !d.top {
                d.window.focus_window();
            }
            d.top = top == Some(d.win);
            // The Channel rack's window follows its size (the graph editor opened below it, a column added).
            let k = ui_scale(&d.window);
            if let (Some((w, h)), size) = (self.app.state.detached_size(d.win), d.window.inner_size()) {
                let have = ((f64::from(size.width) / k) as f32, (f64::from(size.height) / k) as f32);
                if (have.0 - w).abs() > 1.0 || (have.1 - h).abs() > 1.0 {
                    let (mw, mh) = self.app.state.detached_min(d.win);
                    let px = |v: f32| (f64::from(v) * k).round() as u32;
                    d.window.set_min_inner_size(Some(PhysicalSize::new(px(mw), px(mh))));
                    let _ = d.window.request_inner_size(PhysicalSize::new(px(w), px(h)));
                }
            }
            if self.app.detached_dirty(d.win) {
                d.window.request_redraw();
            }
        }
    }

    /// The detached window whose native window is `id`.
    pub(crate) fn detached_of(&self, id: winit::window::WindowId) -> Option<usize> {
        self.detached.iter().position(|d| d.window.id() == id)
    }

    /// An event of detached window `i`'s native window (its keys go through the main window's handling).
    pub(crate) fn detached_event(&mut self, i: usize, event: WindowEvent) {
        let (win, window) = (self.detached[i].win, self.detached[i].window.clone());
        let k = ui_scale(&window);
        let press = match &event {
            WindowEvent::MouseInput { state, .. } => *state == ElementState::Pressed,
            WindowEvent::MouseWheel { .. } => true,
            _ => false,
        };
        if press && self.app.loading() {
            return;
        }
        match event {
            // Alt+F4 or the taskbar: as its close button.
            WindowEvent::CloseRequested => {
                self.app.state.hide(win);
                self.app.invalidate();
            }
            WindowEvent::CursorMoved { position, .. } => {
                // A held knob or scroll-bar drag moves by raw motion (`Shell::device_event`).
                if self.lock.is_some() {
                    return;
                }
                if self.app.cross_drag() && self.cross_move(Some(win), (position.x, position.y)) {
                    return;
                }
                let (x, y) = ((position.x / k) as f32, (position.y / k) as f32);
                let c = self.app.on_surface(win, |a| {
                    a.mouse_move(x, y);
                    a.cursor()
                });
                let d = &mut self.detached[i];
                if c != d.cursor {
                    d.cursor = c;
                    d.window.set_cursor(match c {
                        app::Cursor::Default => CursorIcon::Default,
                        app::Cursor::EwResize => CursorIcon::EwResize,
                        app::Cursor::NsResize => CursorIcon::NsResize,
                        app::Cursor::NwseResize => CursorIcon::NwseResize,
                        app::Cursor::NeswResize => CursorIcon::NeswResize,
                    });
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                match (state, button) {
                    (ElementState::Released, MouseButton::Left | MouseButton::Right | MouseButton::Middle) => {
                        self.unlock_cursor();
                        self.app.mouse_up_from(Some(win));
                    }
                    (ElementState::Pressed, MouseButton::Left | MouseButton::Right | MouseButton::Middle) => {
                        let mouse = self.app.on_surface(win, |a| {
                            match button {
                                MouseButton::Left => a.mouse_down(),
                                MouseButton::Right => a.mouse_down_right(),
                                _ => a.mouse_down_middle(),
                            }
                            a.mouse
                        });
                        // A knob or a scroll-bar right-drag holds the cursor in this window, as in the main one.
                        if self.app.cursor_locked() && button != MouseButton::Middle {
                            self.lock_cursor(&window, (f64::from(mouse.0) * k, f64::from(mouse.1) * k), mouse);
                            self.lock_on = Some(win);
                        }
                    }
                    _ => {}
                }
                self.app.want_drag_window = false;
                self.native_requests();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x, y),
                    MouseScrollDelta::PixelDelta(p) => (p.x as f32 / 40.0, p.y as f32 / 40.0),
                };
                let (shift, c) = (self.mods.shift_key(), ctrl(self.mods));
                let warp = self.app.on_surface(win, |a| {
                    a.wheel(dx, dy, shift, c);
                    a.warp.take()
                });
                if let Some((x, y)) = warp {
                    let _ = window.set_cursor_position(PhysicalPosition::new(f64::from(x) * k, f64::from(y) * k));
                }
            }
            WindowEvent::Focused(focused) => {
                self.app.main_focused(focused);
                self.app.auto_close.main_focused = focused;
                if !focused {
                    self.app.typing_release_all();
                }
            }
            WindowEvent::Resized(size) => {
                self.app.set_detached_size(win, ((f64::from(size.width) / k) as f32, (f64::from(size.height) / k) as f32));
                self.follow_detached(i);
                window.request_redraw();
            }
            WindowEvent::Moved(_) => self.follow_detached(i),
            WindowEvent::ScaleFactorChanged { .. } => window.request_redraw(),
            WindowEvent::RedrawRequested => self.paint_detached(i),
            _ => {}
        }
        self.pending |= self.app.has_damage();
        if self.app.detached_dirty(win) {
            window.request_redraw();
        }
    }

    /// The moves, resizes, minimizing and maximizing the detached windows' own frames asked for (`App::native`). The
    /// window system's move or resize takes the rest of the press, so the app's ends here.
    fn native_requests(&mut self) {
        for (w, n) in std::mem::take(&mut self.app.native) {
            let Some(d) = self.detached.iter().find(|d| d.win == w) else { continue };
            let window = d.window.clone();
            match n {
                Native::Move => {
                    self.app.on_surface(w, App::mouse_up);
                    let _ = window.drag_window();
                }
                Native::Resize(edges) => {
                    self.app.on_surface(w, App::mouse_up);
                    if let Some(dir) = resize_direction(edges) {
                        let _ = window.drag_resize_window(dir);
                    }
                }
                Native::Minimize => window.set_minimized(true),
                Native::Maximize => window.set_maximized(!window.is_maximized()),
            }
        }
    }

    /// A drag between windows (`App::cross_drag`) moved to `pos` (physical px) in the native window of surface `from`
    /// (`None`: the main window's; it has the mouse until the release, wherever the cursor goes): the move goes to the
    /// surface under the cursor on screen (`surface_at`), the topmost detached window first. False where windows have
    /// no screen position (Wayland): the move stays with `from`.
    pub(crate) fn cross_move(&mut self, from: Option<Win>, pos: (f64, f64)) -> bool {
        let Some(main) = self.window.clone() else { return false };
        let origin = match from {
            None => main.inner_position(),
            Some(w) => match self.detached.iter().find(|d| d.win == w) {
                Some(d) => d.window.inner_position(),
                None => return false,
            },
        };
        let Ok(o) = origin else { return false };
        let p = (o.x + pos.0.round() as i32, o.y + pos.1.round() as i32);
        let mut rects: Vec<(Win, [i32; 4])> = (self.detached.iter())
            .filter(|d| d.window.is_minimized() != Some(true))
            .filter_map(|d| {
                let at = d.window.inner_position().ok()?;
                let size = d.window.inner_size();
                Some((d.win, [at.x, at.y, size.width as i32, size.height as i32]))
            })
            .collect();
        let z = &self.app.state.z;
        rects.sort_by_key(|r| std::cmp::Reverse(z.iter().position(|w| *w == r.0)));
        match surface_at(p, &rects) {
            Some(w) => {
                let Some(window) = self.detached.iter().find(|d| d.win == w).map(|d| d.window.clone()) else { return false };
                let (Ok(at), k) = (window.inner_position(), ui_scale(&window)) else { return false };
                let (x, y) = ((f64::from(p.0 - at.x) / k) as f32, (f64::from(p.1 - at.y) / k) as f32);
                self.app.on_surface(w, |a| a.mouse_move(x, y));
            }
            None => {
                let Ok(at) = main.inner_position() else { return false };
                let (x, y) = self.to_app(&main, f64::from(p.0 - at.x), f64::from(p.1 - at.y));
                self.app.mouse_move(x, y);
            }
        }
        for d in &self.detached {
            if self.app.detached_dirty(d.win) {
                d.window.request_redraw();
            }
        }
        self.pending |= self.app.has_damage();
        true
    }

    /// Keeps detached window `i`'s frame for the next start (`layout`), while it is neither minimized nor maximized.
    fn follow_detached(&mut self, i: usize) {
        let d = &self.detached[i];
        if d.window.is_minimized() == Some(true) || d.window.is_maximized() {
            return;
        }
        if let Some(((x, y), (w, h))) = frame_of(&d.window) {
            self.app.set_detached_frame(d.win, [x, y, w as i32, h as i32]);
        }
    }

    /// Paints detached window `i` whole and presents it.
    fn paint_detached(&mut self, i: usize) {
        let d = &mut self.detached[i];
        let size = d.window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else { return };
        let k = ui_scale(&d.window) as f32;
        if d.canvas.is_none() {
            d.canvas = Canvas::new(size.width, size.height, k).ok();
        }
        let Some(canvas) = d.canvas.as_mut() else { return };
        if canvas.pixmap.width() != size.width || canvas.pixmap.height() != size.height || canvas.scale != k {
            canvas.resize(size.width, size.height, k);
        }
        self.app.paint_detached(d.win, canvas);
        if d.surface.resize(w, h).is_ok() {
            if let Ok(mut buf) = d.surface.buffer_mut() {
                canvas.copy_region_to_xrgb_at(&mut buf, size.width as usize, (0, 0, size.width as usize, size.height as usize), (0, 0));
                let _ = buf.present();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An app window edge (or corner) resizes the native window the same way.
    #[test]
    fn edges_resize_the_native_window() {
        assert_eq!(resize_direction(EDGE_L), Some(ResizeDirection::West));
        assert_eq!(resize_direction(EDGE_R | EDGE_B), Some(ResizeDirection::SouthEast));
        assert_eq!(resize_direction(EDGE_T), Some(ResizeDirection::North));
        assert_eq!(resize_direction(EDGE_L | EDGE_T), Some(ResizeDirection::NorthWest));
        assert_eq!(resize_direction(0), None);
    }

    /// A drag between windows goes to the topmost detached window under the cursor, else to the main window.
    #[test]
    fn a_cross_window_drag_finds_the_window_under_the_cursor() {
        let rects = [(Win::Mixer, [100, 100, 400, 300]), (Win::ChannelRack, [300, 200, 400, 300])];
        assert_eq!(surface_at((350, 250), &rects), Some(Win::Mixer), "the topmost of two");
        assert_eq!(surface_at((650, 450), &rects), Some(Win::ChannelRack));
        assert_eq!(surface_at((100, 100), &rects), Some(Win::Mixer), "its top-left pixel");
        assert_eq!(surface_at((500, 150), &rects), None, "right of the Mixer: the main window");
        assert_eq!(surface_at((50, 50), &[]), None);
    }
}
