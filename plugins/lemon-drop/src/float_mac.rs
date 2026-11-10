// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The floating display (AppKit): the display in a borderless panel of its own on the screen, at the state's window
//! rectangle (points from the main screen's top left), see-through wherever the display paints nothing, so with
//! Background off only the picture floats over the desktop. Its body drags it and its lower right corner sizes it; the
//! new rectangle is kept in the state. Right click (or Control-click): the display menu; middle click: hold.
//!
//! The editor's tick opens it when the state is Visible. It has a GUI thread of its own, as an editor has, and lives on
//! after the editor closes, until Visible goes off or the plugin goes. Keep in front: above every window. On desktop: it
//! does not go with the host, which it otherwise does as the app's detached plugin windows do (gone while another
//! application is active, and while the host is hidden or minimised). Solid off: clicks go through it. Mini view: the
//! mini rectangle.
//!
// spec: plugins/lemon-drop.md "Floating display"

use std::sync::Arc;
use std::time::{Duration, Instant};

use os_plugkit::gui::{Button, Floating, Input, Window};
use os_theme::Palette;
use os_ui::{Canvas, Rect};

use crate::editor::{Logic, MenuReq, SeeThrough, TIMER_MS};
use crate::state::Rect4;
use crate::{DropParams, Shared};

/// The lower right corner that sizes the window, points.
const GRIP: f32 = 12.0;
/// The smallest window side, points.
const MIN_SIDE: i32 = 24;

/// Opens the floating display when the state is Visible and it is not open. On `editor`'s GUI thread (its tick).
pub fn ensure(params: &Arc<DropParams>, shared: &Arc<Shared>, editor: &Window) {
    {
        let Ok(mut open) = shared.float_thread.lock() else { return };
        if open.as_ref().is_some_and(|t| !t.finished()) {
            return;
        }
        // It went by itself (Visible off) and its thread has ended: this does not wait.
        *open = None;
    }
    let st = params.current().1;
    if !st.visible {
        return;
    }
    let (p, s) = (params.clone(), shared.clone());
    let opened = editor.floating(sized(if st.mini { st.mini_window } else { st.window }), move |win| {
        if let Some(f) = Float::new(win, p, s) {
            f.run();
        }
    });
    if let (Some(t), Ok(mut open)) = (opened, shared.float_thread.lock()) {
        *open = Some(t);
    }
}

/// Closes the floating display (the plugin is going), waiting for its thread. Any thread.
pub fn close(shared: &Shared) {
    let open = shared.float_thread.lock().ok().and_then(|mut o| o.take());
    drop(open);
}

/// A rectangle at least `MIN_SIDE` a side.
fn sized(r: Rect4) -> Rect4 {
    [r[0], r[1], r[0] + (r[2] - r[0]).max(MIN_SIDE), r[1] + (r[3] - r[1]).max(MIN_SIDE)]
}

#[derive(Clone, Copy)]
enum DragKind {
    Move,
    Size,
}

/// A drag of the window: the pointer (points) and the window's rectangle at the press.
#[derive(Clone, Copy)]
struct Drag {
    kind: DragKind,
    from: (f32, f32),
    at: Rect4,
}

/// The display's picture: what the menu's modal loop keeps going without the window's own state.
struct Picture {
    logic: Logic,
    canvas: Canvas,
    /// The window's size, points, and the backing scale.
    size: (f32, f32),
    scale: f32,
    buffers: SeeThrough,
    frame: Vec<u32>,
    /// The frame on the screen, to leave it there when nothing moved.
    shown: Vec<u32>,
    last: Instant,
}

impl Picture {
    fn device(&self) -> (u32, u32) {
        (((self.size.0 * self.scale).round() as u32).max(1), ((self.size.1 * self.scale).round() as u32).max(1))
    }

    /// A window of `size` points at backing scale `scale`.
    fn set_size(&mut self, size: (f32, f32), scale: f32) {
        if (size, scale) == (self.size, self.scale) {
            return;
        }
        (self.size, self.scale) = (size, scale);
        let (w, h) = self.device();
        self.canvas.resize(w, h, scale);
    }

    /// The analysis, with the time since the last tick.
    fn tick(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f32();
        self.last = now;
        self.logic.tick(dt);
    }

    /// Paints the display and puts it on the screen, unless it is the same picture.
    fn show(&mut self, win: &Window) {
        let (w, h) = self.device();
        self.frame.resize((w * h) as usize, 0);
        let area = Rect::new(0.0, 0.0, self.size.0, self.size.1);
        self.logic.paint_see_through(&mut self.canvas, area, &mut self.buffers, &mut self.frame);
        if self.frame != self.shown {
            win.blit_alpha(&self.frame, w, h);
            self.shown.clone_from(&self.frame);
        }
    }
}

struct Float {
    win: Window,
    pic: Picture,
    palette: Palette,
    /// The state's rectangle the window was last put at.
    placed: Rect4,
    drag: Option<Drag>,
    /// The sizing cursor is on.
    gripped: bool,
    /// The state's Solid as last seen.
    solid: bool,
}

impl Float {
    fn new(mut win: Window, params: Arc<DropParams>, shared: Arc<Shared>) -> Option<Float> {
        win.set_tick(Duration::from_millis(u64::from(TIMER_MS)));
        let logic = Logic::new(params, shared);
        let placed = logic.window_rect();
        let r = sized(placed);
        let (scale, size) = (win.scale(), ((r[2] - r[0]) as f32, (r[3] - r[1]) as f32));
        let device = (((size.0 * scale).round() as u32).max(1), ((size.1 * scale).round() as u32).max(1));
        let canvas = Canvas::new(device.0, device.1, scale).ok()?;
        let pic = Picture { logic, canvas, size, scale, buffers: SeeThrough::default(), frame: Vec::new(), shown: Vec::new(), last: Instant::now() };
        let solid = pic.logic.st.solid;
        let mut f = Float { win, pic, palette: os_plugkit::palette(), placed, drag: None, gripped: false, solid };
        f.pic.show(&f.win);
        f.settle();
        Some(f)
    }

    fn run(mut self) {
        while let Some(input) = self.win.next() {
            match input {
                Input::Tick => {
                    if !self.tick() {
                        return;
                    }
                }
                Input::Expose => {
                    self.pic.set_size(self.pic.size, self.win.scale());
                    self.pic.shown.clear();
                    self.pic.show(&self.win);
                }
                Input::Down { x, y, btn, .. } => self.down(x, y, btn),
                Input::Move { x, y } => self.moved(x, y),
                Input::Up { btn: Button::Left } => self.dropped(),
                Input::Leave => {
                    self.pic.logic.point(None);
                    self.grip(false);
                }
                _ => {}
            }
        }
    }

    /// The tick: the analysis, the state's window settings, a new picture. False: the window should go (Visible off).
    fn tick(&mut self) -> bool {
        self.pic.tick();
        if !self.pic.logic.st.visible {
            return false;
        }
        self.settle();
        if self.drag.is_none() {
            let want = self.pic.logic.window_rect();
            if want != self.placed {
                // Moved from outside (a preset, Mini view): put the window there.
                self.placed = want;
                let r = sized(want);
                self.win.place(r);
                self.pic.set_size(((r[2] - r[0]) as f32, (r[3] - r[1]) as f32), self.pic.scale);
            }
        }
        self.pic.show(&self.win);
        true
    }

    /// On the desktop as the state says, and on or off the screen as the host stands.
    fn settle(&mut self) {
        let st = &self.pic.logic.st;
        let (solid, f) = (st.solid, Floating { owned: !st.on_desktop, front: st.keep_in_front, solid: st.solid });
        self.win.set_floating(f);
        if solid != self.solid {
            self.solid = solid;
            // Going click-through, the window hears nothing more of the pointer.
            if !solid {
                self.pic.logic.point(None);
                self.grip(false);
            }
        }
    }

    /// The pointer on the screen, points.
    fn pointer(&self) -> (f32, f32) {
        let (p, k) = (self.win.pointer(), self.win.scale());
        (p.0 as f32 / k, p.1 as f32 / k)
    }

    fn in_grip(&self, x: f32, y: f32) -> bool {
        x >= self.pic.size.0 - GRIP && y >= self.pic.size.1 - GRIP
    }

    fn grip(&mut self, on: bool) {
        if on != self.gripped {
            self.gripped = on;
            self.win.sizing_cursor(on);
        }
    }

    fn down(&mut self, x: f32, y: f32, btn: Button) {
        let (x, y) = (x / self.pic.scale, y / self.pic.scale);
        match btn {
            Button::Left => {
                self.win.raise();
                if let Some(at) = self.win.frame() {
                    let kind = if self.in_grip(x, y) { DragKind::Size } else { DragKind::Move };
                    self.drag = Some(Drag { kind, from: self.pointer(), at });
                }
            }
            Button::Middle => self.pic.logic.toggle_hold(),
            Button::Right => {
                self.win.raise();
                let (p, scale) = (self.win.pointer(), self.win.scale());
                let def = self.pic.logic.menu(&MenuReq::Display);
                let pic = &mut self.pic;
                let live = &mut |w: &Window| {
                    pic.tick();
                    pic.show(w);
                };
                if let Some(cmd) = self.win.popup_live(p.0, p.1, &def, &self.palette, scale, live) {
                    self.pic.logic.pick(&MenuReq::Display, cmd, 0.0, 0.0);
                }
            }
        }
    }

    fn moved(&mut self, x: f32, y: f32) {
        let Some(d) = self.drag else {
            let (x, y) = (x / self.pic.scale, y / self.pic.scale);
            self.pic.logic.point(Some((x, y)));
            self.grip(self.in_grip(x, y));
            return;
        };
        let p = self.pointer();
        let (dx, dy) = ((p.0 - d.from.0).round() as i32, (p.1 - d.from.1).round() as i32);
        match d.kind {
            DragKind::Move => self.win.set_frame([d.at[0] + dx, d.at[1] + dy, d.at[2] + dx, d.at[3] + dy]),
            DragKind::Size => {
                let r = sized([d.at[0], d.at[1], d.at[2] + dx, d.at[3] + dy]);
                self.win.set_frame(r);
                self.pic.set_size(((r[2] - r[0]) as f32, (r[3] - r[1]) as f32), self.pic.scale);
                self.pic.show(&self.win);
            }
        }
    }

    /// The drag ended: the window's new rectangle into the state.
    fn dropped(&mut self) {
        if self.drag.take().is_none() {
            return;
        }
        if let Some(r) = self.win.frame() {
            self.pic.logic.set_window_rect(r);
            self.placed = self.pic.logic.window_rect();
        }
    }
}
