// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The shell's side of Options ▸ Detached (Lemon Studio's design, spec: ui/mixer.md §6.2): each detached window
//! (`App::detached_windows`) gets a native window of its own, made and dropped on the main event loop as the app asks,
//! with its own surface and canvas. The app paints the window into it whole (`App::paint_detached`) and takes its
//! mouse input on that window's surface (`App::on_surface`); its keys go where the main window's do. Its title bar,
//! buttons and edges move, minimize, maximize and resize the native window (`App::native`); closing it closes the
//! window as its close button does.

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

/// Opens `w`'s native window: where it was last (`App::detached_frame`), else over the place it had in the main window,
/// at the size it had there. Owned by the main window on Windows (above it, out of the taskbar, minimized with it).
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
    let window = Rc::new(el.create_window(attrs).ok()?);
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
                self.app.on_surface(win, |a| match (state, button) {
                    (ElementState::Pressed, MouseButton::Left) => a.mouse_down(),
                    (ElementState::Pressed, MouseButton::Right) => a.mouse_down_right(),
                    (ElementState::Pressed, MouseButton::Middle) => a.mouse_down_middle(),
                    (ElementState::Released, MouseButton::Left | MouseButton::Right | MouseButton::Middle) => a.mouse_up(),
                    _ => {}
                });
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
}
