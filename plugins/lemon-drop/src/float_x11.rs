// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The floating display (X11; XWayland under Wayland): the display in a borderless top-level window of its own at the
//! state's window rectangle, see-through wherever the display paints nothing (a 32-bit visual, which needs a compositor).
//! Its body drags it and its lower right corner sizes it, both through the window manager; the new rectangle is kept in
//! the state. Right click: the display menu; middle click: hold.
//!
//! It is opened by the editor's tick when the state is Visible, then lives on its own thread after the editor closes,
//! until Visible goes off or the plugin goes. Keep in front: above every window. On desktop: not transient for the host's
//! window, so it stays when the host is minimised. Solid off: clicks go through it. Mini view: the mini rectangle.
//!
// spec: plugins/lemon-drop.md "Floating display"

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use os_plugkit::gui::{self as px, Button, Input, Window};
use os_theme::Palette;
use os_ui::{Canvas, Rect};

use crate::editor::{Logic, MenuReq, SeeThrough};
use crate::state::Rect4;
use crate::{DropParams, Shared};

/// The lower right corner that sizes the window, device px.
const GRIP: i32 = 14;
/// The smallest window side, logical px.
const MIN_SIDE: f32 = 24.0;
/// A drag belongs to the window manager, which does not say when it ends: the rectangle goes into the state once the
/// window has been still this long.
const SETTLE: Duration = Duration::from_millis(300);

/// Opens the floating display when the state is Visible and it is not open. From the editor's tick; `editor` is the
/// editor's window, whose top owner the floating one stays with.
pub fn ensure(params: &Arc<DropParams>, shared: &Arc<Shared>, scale: f32, editor: u32) {
    if shared.float.load(Ordering::Acquire) != 0 || !params.current().1.visible {
        return;
    }
    if !open(params.clone(), shared.clone(), scale, editor) {
        // No 32-bit visual: not asked again on every tick.
        shared.float.store(-1, Ordering::Release);
    }
}

/// Closes the floating display (the plugin is going) and waits for its thread.
pub fn close(shared: &Shared) {
    let thread = shared.float_thread.lock().ok().and_then(|mut t| t.take());
    drop(thread);
    shared.float.store(0, Ordering::Release);
}

/// A rectangle in logical screen px as device px (x, y, w, h).
fn device(r: Rect4, scale: f32) -> [i32; 4] {
    let d = |v: i32| (v as f32 * scale).round() as i32;
    let min = (MIN_SIDE * scale).round() as i32;
    let (x, y) = (d(r[0]), d(r[1]));
    [x, y, (d(r[2]) - x).max(min), (d(r[3]) - y).max(min)]
}

/// Device px as logical screen px.
fn logical(r: [i32; 4], scale: f32) -> Rect4 {
    let l = |v: i32| (v as f32 / scale).round() as i32;
    [l(r[0]), l(r[1]), l(r[0] + r[2]), l(r[1] + r[3])]
}

fn overlaps(a: [i32; 4], b: [i32; 4]) -> bool {
    a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3]
}

/// `r` moved when it is on no monitor (a project from another screen layout): to the middle of the one `near` is on.
fn on_screen(r: [i32; 4], monitors: &[[i32; 4]], near: Option<[i32; 4]>) -> [i32; 4] {
    if monitors.iter().any(|&m| overlaps(r, m)) {
        return r;
    }
    let m = near.and_then(|n| monitors.iter().find(|&&m| overlaps(n, m))).or(monitors.first());
    let Some(m) = m else { return r };
    [m[0] + (m[2] - r[2]).max(0) / 2, m[1] + (m[3] - r[3]).max(0) / 2, r[2], r[3]]
}

fn open(params: Arc<DropParams>, shared: Arc<Shared>, scale: f32, editor: u32) -> bool {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    let logic = Logic::new(params, shared.clone());
    let placed = logic.window_rect();
    let at = device(placed, scale);
    let min = (MIN_SIDE * scale).round() as u32;
    let owned = shared.clone();
    // Marked before the thread can end, which clears it.
    shared.float.store(1, Ordering::Release);
    let thread = px::spawn_floating(at, min, crate::NAME, move |win| {
        let host = win.top_owner(editor);
        let near = host.and_then(|h| win.rect_of(h));
        let at = on_screen(at, &win.monitors(), near);
        if let Ok(canvas) = Canvas::new(at[2] as u32, at[3] as u32, scale) {
            let f = Float {
                win,
                logic,
                host,
                scale,
                canvas,
                at,
                styled: None,
                placed,
                dragged: false,
                settling: None,
                grip: false,
                last: Instant::now(),
                palette: os_plugkit::palette(),
                buffers: SeeThrough::default(),
                frame: Vec::new(),
            };
            f.run();
        }
        owned.float.store(0, Ordering::Release);
    });
    let Some(thread) = thread else {
        return false;
    };
    if let Ok(mut t) = shared.float_thread.lock() {
        *t = Some(thread);
    }
    true
}

struct Float {
    win: Window,
    logic: Logic,
    /// The host's top window: the owner unless On desktop.
    host: Option<u32>,
    scale: f32,
    canvas: Canvas,
    /// The window's place and size (x, y, w, h, device px).
    at: [i32; 4],
    /// (On desktop, Keep in front, Solid) as last applied.
    styled: Option<(bool, bool, bool)>,
    /// The state's rectangle the window was last put at.
    placed: Rect4,
    /// The user has dragged it since it was last put: where it goes now is theirs, kept in the state. (A window manager
    /// that adjusts a window it was told to put somewhere does not change the project.)
    dragged: bool,
    /// When it last moved during a drag.
    settling: Option<Instant>,
    /// The pointer shows the sizing cursor.
    grip: bool,
    last: Instant,
    palette: Palette,
    buffers: SeeThrough,
    frame: Vec<u32>,
}

impl Float {
    fn run(mut self) {
        // The first frame before the window shows, in its place: X11 has no window to ask for its size before this.
        if self.at != device(self.placed, self.scale) {
            self.win.set_rect(self.at);
        }
        self.render();
        self.win.map();
        self.restyle();
        while let Some(input) = self.win.next() {
            match input {
                Input::Tick => {
                    if !self.tick() {
                        return;
                    }
                }
                Input::Down { x, y, btn, .. } => self.down(x, y, btn),
                Input::Move { x, y } => {
                    let grip = self.on_grip(x, y);
                    if grip != self.grip {
                        self.grip = grip;
                        self.win.set_grip_cursor(grip);
                    }
                    self.logic.point(Some((x / self.scale, y / self.scale)));
                }
                Input::Leave => self.logic.point(None),
                _ => {}
            }
        }
    }

    fn on_grip(&self, x: f32, y: f32) -> bool {
        x as i32 >= self.at[2] - GRIP && y as i32 >= self.at[3] - GRIP
    }

    fn down(&mut self, x: f32, y: f32, btn: Button) {
        match btn {
            Button::Left => {
                self.dragged = true;
                self.win.manager_drag(self.on_grip(x, y));
            }
            Button::Middle => self.logic.toggle_hold(),
            Button::Right => {
                let (sx, sy) = self.win.to_root(x as i32, y as i32);
                let def = self.logic.menu(&MenuReq::Display);
                if let Some(cmd) = self.win.popup(sx, sy, &def, &self.palette, self.scale) {
                    self.logic.pick(&MenuReq::Display, cmd, 0.0, 0.0);
                }
            }
        }
    }

    /// The tick: the analysis, the state's window settings, a new picture. False: the window should go (Visible off).
    fn tick(&mut self) -> bool {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f32();
        self.last = now;
        self.logic.tick(dt);
        if !self.logic.st.visible {
            return false;
        }
        self.restyle();
        self.follow();
        if self.settling.is_none() {
            let want = self.logic.window_rect();
            if want != self.placed {
                // Moved from outside (a preset, Mini view): put the window there.
                self.placed = want;
                self.dragged = false;
                let near = self.host.and_then(|h| self.win.rect_of(h));
                let r = on_screen(device(want, self.scale), &self.win.monitors(), near);
                self.win.set_rect(r);
                self.resized(r);
            }
        }
        self.render();
        true
    }

    /// Owner, topmost and click-through from the state, when they changed.
    fn restyle(&mut self) {
        let st = &self.logic.st;
        let want = (st.on_desktop, st.keep_in_front, st.solid);
        if self.styled == Some(want) {
            return;
        }
        self.styled = Some(want);
        let (desktop, front, solid) = want;
        self.win.set_owner(if desktop { None } else { self.host });
        self.win.set_above(front);
        self.win.set_click_through(!solid);
    }

    /// The window's real place, which the window manager changes during a drag; once a drag has settled, into the state.
    fn follow(&mut self) {
        if self.win.take_configured() {
            if let Some(r) = self.win.rect_of(self.win.win) {
                if r != self.at {
                    self.resized(r);
                    if self.dragged {
                        self.settling = Some(Instant::now());
                    }
                }
            }
        }
        if self.settling.is_some_and(|t| t.elapsed() >= SETTLE) {
            self.settling = None;
            self.logic.set_window_rect(logical(self.at, self.scale));
            self.placed = self.logic.window_rect();
        }
    }

    fn resized(&mut self, r: [i32; 4]) {
        if (r[2], r[3]) != (self.at[2], self.at[3]) {
            self.canvas.resize(r[2].max(1) as u32, r[3].max(1) as u32, self.scale);
        }
        self.at = r;
    }

    /// Paints the display and puts it on the screen with its alpha.
    fn render(&mut self) {
        let (w, h) = (self.at[2].max(1), self.at[3].max(1));
        let area = Rect::new(0.0, 0.0, w as f32 / self.scale, h as f32 / self.scale);
        self.frame.resize((w * h) as usize, 0);
        self.logic.paint_see_through(&mut self.canvas, area, &mut self.buffers, &mut self.frame);
        self.win.blit(&self.frame, w as u32, h as u32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangles_survive_device_and_back() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let r = [600, 200, 901, 325];
            assert_eq!(logical(device(r, scale), scale), r, "at scale {scale}");
        }
    }

    #[test]
    fn a_tiny_rectangle_takes_the_smallest_size() {
        assert_eq!(device([10, 10, 10, 10], 2.0), [20, 20, 48, 48]);
    }

    #[test]
    fn a_rectangle_off_every_monitor_moves_to_the_middle_of_the_hosts() {
        let monitors = [[0, 0, 1920, 1080], [1920, 0, 1000, 800]];
        assert_eq!(on_screen([100, 100, 200, 100], &monitors, None), [100, 100, 200, 100], "on a monitor: left alone");
        assert_eq!(on_screen([1486, 1166, 59, 59], &monitors, None), [930, 510, 59, 59], "the first monitor");
        assert_eq!(on_screen([5000, 5000, 200, 100], &monitors, Some([2000, 10, 50, 50])), [2320, 350, 200, 100], "the host's");
    }
}
