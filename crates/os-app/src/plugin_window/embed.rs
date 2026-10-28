// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! A plugin window drawn inside Lemon Studio's own window (Linux): the frame (`super`) painted on the app's canvas, the
//! plugin's editor painted from what it hands over (`os_plugkit::embed`). For the plugins compiled into Lemon Studio,
//! whose editors draw in-process; other plugins keep a window of their own (`x11`).
//!
//! To the rest of the app it is a docked window: it joins `State::z` as `Win::Hosted`, painted and clicked like the
//! drawn windows. Its place and the points it reports are the canvas's logical px; the points it hands the app for
//! menus are screen px, as the other frames' (`Inner::to_screen`).

use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;

use os_hostlink::PageIcon;
use os_plugkit::embed::{Surface, View};
use os_plugkit::input::{Button, Input};
use os_ui::{Canvas, Rect};

use super::*;

/// Where a press went, for the moves and the release that follow.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Grab {
    /// The title bar, strip or Misc page (`State`).
    Frame,
    /// The title bar moving the window: the mouse and the window's top-left at the press.
    Move { at: (f32, f32), from: (f32, f32) },
    /// The plugin's editor.
    Editor,
}

struct Inner {
    st: State,
    surface: Surface,
    /// Top-left on the canvas (logical px).
    pos: (f32, f32),
    /// The canvas's scale (device px per logical px).
    k: f32,
    /// The frame painted (its own canvas, the frame's size).
    chrome: Canvas,
    chrome_dirty: bool,
    /// Bumped when the chrome is painted again: with the editor's serial, what the app compares to repaint it.
    chrome_serial: u64,
    grab: Option<Grab>,
    /// The mouse is over the editor (it gets a `Leave` when it goes).
    over_editor: bool,
    /// The editor's last press, for double clicks: when, where.
    last_press: Option<(std::time::Instant, (f32, f32))>,
    /// The app's workspace (logical px): a maximize fills it.
    work: Rect,
    /// The canvas's top-left in the main window (device px), for the screen points the app is handed.
    canvas_off: (i32, i32),
    /// Placed in the workspace yet (it opens centred in it, `sync`).
    placed: bool,
    /// The editor's size as `sync` last saw it (device px): a change is the editor resizing itself.
    seen_size: Option<(u32, u32)>,
}

/// A plugin window in Lemon Studio's own window. Cheap to clone: the app's state holds one per open window to paint
/// it (`State::embeds`).
#[derive(Clone)]
pub struct EmbedFrame(Rc<RefCell<Inner>>);

impl std::fmt::Debug for EmbedFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "EmbedFrame({:?})", self.rect())
    }
}

/// A logical length in device px.
fn dev(v: f32, k: f32) -> i32 {
    (v * k).round() as i32
}

impl Inner {
    fn size(&self) -> (f32, f32) {
        self.st.size()
    }

    /// The editor's top-left on the canvas (device px).
    fn editor_at(&self) -> (i32, i32) {
        (dev(self.pos.0 + self.st.border(), self.k), dev(self.pos.1 + self.st.area_y(), self.k))
    }

    /// The editor's part of the window on the canvas (logical px).
    fn editor_area(&self) -> Rect {
        let (w, h) = self.size();
        let (b, ay) = (self.st.border(), self.st.area_y());
        Rect::new(self.pos.0 + b, self.pos.1 + ay, w - 2.0 * b, h - ay - b)
    }

    /// Canvas point (`x`, `y`) goes to the editor: over it, its page showing, and not on a card's resize edge (which
    /// lies over the editor's own edge, the card having no frame around it).
    fn takes(&self, x: f32, y: f32) -> bool {
        !self.st.misc && self.editor_area().contains(x, y) && !matches!(part_at(&self.st, x - self.pos.0, y - self.pos.1), Some(Part::Edge { .. }))
    }

    /// A frame point (logical px from its top-left) as a screen point (device px): the main window's, for an X11 one.
    fn to_screen(&self, x: f32, y: f32) -> (i32, i32) {
        let p = (dev(self.pos.0 + x, self.k) + self.canvas_off.0, dev(self.pos.1 + y, self.k) + self.canvas_off.1);
        super::x11::main_to_screen(p)
    }

    /// Top-left `p` kept as the app's floating windows are (`win_clamp::keep`): its title bar never above the
    /// workspace, so it can always be dragged back, and some of it in view at either side and the bottom.
    fn kept(&self, p: (f32, f32)) -> (f32, f32) {
        if self.work.w <= 0.0 {
            return p;
        }
        crate::win_clamp::keep(p.0, p.1, self.size().0, self.work)
    }

    /// Paints the frame again into its canvas when it changed.
    fn repaint(&mut self) {
        if !self.chrome_dirty {
            return;
        }
        self.chrome_dirty = false;
        let (w, h) = self.size();
        let (dw, dh) = (dev(w, self.k).max(1) as u32, dev(h, self.k).max(1) as u32);
        if (self.chrome.pixmap.width(), self.chrome.pixmap.height()) != (dw, dh) || self.chrome.scale != self.k {
            self.chrome.resize(dw, dh, self.k);
        }
        super::paint(&mut self.chrome, &self.st);
        self.chrome_serial += 1;
    }

    /// Does what the frame asked for after an input.
    fn reply(&mut self, r: Reply, at: (f32, f32)) {
        match r {
            Reply::None => {}
            Reply::Repaint | Reply::Relayout => self.chrome_dirty = true,
            Reply::Capture | Reply::Hold => {
                self.chrome_dirty = true;
                self.grab = Some(Grab::Frame);
            }
            Reply::Move => self.grab = Some(Grab::Move { at, from: self.pos }),
            // A window in the app has nowhere to go minimized: it closes (its channel's name button opens it again).
            Reply::Minimize => self.st.events.push(FrameEvent::Close),
            Reply::Maximize => {
                let top_left = (dev(self.pos.0, self.k), dev(self.pos.1, self.k));
                let w = self.work;
                let target = Some(((dev(w.x, self.k), dev(w.y, self.k)), (w.w, w.h)));
                if let Some(at) = self.st.maximize(top_left, target) {
                    self.pos = (at.0 as f32 / self.k, at.1 as f32 / self.k);
                }
                self.chrome_dirty = true;
            }
        }
    }
}

impl EmbedFrame {
    /// A frame for an editor of `editor` (logical px) at canvas point `pos` (`None`: centred in the workspace), drawn at
    /// `scale`.
    pub fn new(info: ChannelInfo, editor: (f32, f32), scale: f32, pos: Option<(f32, f32)>) -> Result<EmbedFrame, String> {
        let k = if scale > 0.0 { scale } else { 1.0 };
        let st = State::new(info, editor, false);
        let size = ((editor.0 * k).round() as u32, (editor.1 * k).round() as u32);
        let surface = os_plugkit::embed::offer(size, os_clap::wake);
        let (w, h) = st.size();
        let chrome = Canvas::new(dev(w, k).max(1) as u32, dev(h, k).max(1) as u32, k)?;
        let inner = Inner {
            st,
            surface,
            pos: pos.unwrap_or((0.0, 0.0)),
            k,
            chrome,
            chrome_dirty: true,
            chrome_serial: 0,
            grab: None,
            over_editor: false,
            last_press: None,
            work: Rect::new(0.0, 0.0, 0.0, 0.0),
            canvas_off: (0, 0),
            placed: pos.is_some(),
            seen_size: None,
        };
        Ok(EmbedFrame(Rc::new(RefCell::new(inner))))
    }

    // ---------------------------------------------------------------------------------------------------------------
    // What every plugin window answers (as `x11::FrameWindow`).

    pub fn detached(&self) -> bool {
        false
    }

    /// Always in the app's window: there is no detaching it.
    pub fn set_detached(&self, _owner: isize, _on: bool) {}

    /// Drawn by the app, under and over its other windows by their order: nothing to clip.
    pub fn clip_to(&self, _work: [i32; 4], _covers: &[[i32; 4]]) {}

    /// The token the editor was opened with, standing for a window.
    pub fn hwnd(&self) -> isize {
        self.0.borrow().surface.token() as isize
    }

    /// The token the plugin opens its editor "in" (as an X11 window): it claims the surface behind it.
    pub fn area(&self) -> *mut c_void {
        self.0.borrow().surface.token() as usize as *mut c_void
    }

    /// The app stacks it (`State::raise`).
    pub fn raise(&self) {}

    pub fn set_info(&self, info: ChannelInfo) {
        let mut i = self.0.borrow_mut();
        if i.st.info != info {
            i.st.info = info;
            i.chrome_dirty = true;
        }
    }

    pub fn set_pages(&self, pages: Vec<PageIcon>, active: usize) {
        let mut i = self.0.borrow_mut();
        if i.st.pages != pages || i.st.page != active {
            i.st.pages = pages;
            i.st.page = active;
            i.chrome_dirty = true;
        }
    }

    pub fn set_resizable(&self, min: Option<(f32, f32)>) {
        let mut i = self.0.borrow_mut();
        if i.st.resize_min != min {
            i.st.resize_min = min;
            i.chrome_dirty = true;
        }
    }

    /// The room a window may take: the workspace.
    pub fn work_size(&self) -> Option<(f32, f32)> {
        let w = self.0.borrow().work;
        (w.w > 0.0 && w.h > 0.0).then_some((w.w, w.h))
    }

    pub fn chrome(&self) -> (f32, f32) {
        self.0.borrow().st.chrome()
    }

    #[cfg(test)]
    pub(crate) fn push_event(&self, e: FrameEvent) {
        self.0.borrow_mut().st.events.push(e);
    }

    pub fn set_page(&self, page: usize) {
        let mut i = self.0.borrow_mut();
        if i.st.page != page && page < i.st.pages.len() {
            i.st.page = page;
            i.chrome_dirty = true;
        }
    }

    pub fn set_hint(&self, hint: String) {
        let mut i = self.0.borrow_mut();
        if i.st.hint != hint {
            i.st.hint = hint;
            i.chrome_dirty = true;
        }
    }

    pub fn set_preset(&self, name: Option<String>) {
        let mut i = self.0.borrow_mut();
        if i.st.preset != name {
            i.st.preset = name;
            i.chrome_dirty = true;
        }
    }

    /// The scale the frame and its editor are drawn at: the canvas's.
    pub fn scale(&self) -> f32 {
        self.0.borrow().k
    }

    /// A point of the editor (logical px from its top-left) on the screen: where a menu the plugin asked for opens.
    pub fn editor_to_screen(&self, x: f32, y: f32) -> (i32, i32) {
        let i = self.0.borrow();
        let ay = i.st.area_y();
        i.to_screen(i.st.border() + x, ay + y)
    }

    pub fn set_editor_size(&self, editor: (f32, f32)) {
        let mut i = self.0.borrow_mut();
        if i.st.editor != editor {
            i.st.editor = editor;
            i.chrome_dirty = true;
        }
    }

    pub fn set_strip(&self, on: bool) {
        let mut i = self.0.borrow_mut();
        if i.st.strip != on {
            i.st.strip = on;
            i.chrome_dirty = true;
        }
    }

    pub fn take_events(&self) -> Vec<FrameEvent> {
        std::mem::take(&mut self.0.borrow_mut().st.events)
    }

    // ---------------------------------------------------------------------------------------------------------------
    // The app's side: place, painting, input.

    /// The window on the canvas (logical px).
    pub fn rect(&self) -> Rect {
        let i = self.0.borrow();
        let (w, h) = i.size();
        Rect::new(i.pos.0, i.pos.1, w, h)
    }

    /// The editor's part of it (logical px); `None` while the Misc page covers it.
    pub fn editor_rect(&self) -> Option<Rect> {
        let i = self.0.borrow();
        (!i.st.misc).then(|| i.editor_area())
    }

    /// Moves the window's top-left to canvas point `pos` (logical px).
    pub fn set_pos(&self, pos: (f32, f32)) {
        self.0.borrow_mut().pos = pos;
    }

    /// What the app keeps it up to date with on every tick: the canvas's scale, size (device px) and place in the main
    /// window, the workspace (logical px) and the app's theme.
    pub fn sync(&self, k: f32, canvas: (u32, u32), canvas_off: (i32, i32), work: Rect, look: &os_theme::Palette) {
        let mut i = self.0.borrow_mut();
        if i.k != k {
            i.k = k;
            i.chrome_dirty = true;
        }
        // The Lemon design draws it as a card (`State::card`); the Classic one keeps FL's frame. A theme change paints
        // it again.
        if i.st.look.as_ref() != Some(look) {
            i.st.card = os_ui::kit::lemon::on(look);
            i.st.look = Some(look.clone());
            i.chrome_dirty = true;
        }
        i.canvas_off = canvas_off;
        i.work = work;
        if !i.placed && work.w > 0.0 {
            i.placed = true;
            let (w, h) = i.size();
            i.pos = ((work.x + (work.w - w) / 2.0).max(work.x).round(), (work.y + (work.h - h) / 3.0).max(work.y).round());
        }
        // A maximized window keeps filling the workspace.
        if i.st.maxed.is_some() {
            let size = i.size();
            if i.pos != (work.x, work.y) || size != (work.w, work.h) {
                i.pos = (work.x, work.y);
                if let Some((w, h)) = i.st.fill((work.w, work.h)) {
                    if (w as f32, h as f32) != i.st.editor {
                        i.st.events.push(FrameEvent::Resize(w, h));
                    }
                }
            }
        }
        // Kept in the workspace when it changes too (the browser shown, the window resized).
        if i.st.maxed.is_none() && !matches!(i.grab, Some(Grab::Move { .. })) {
            let p = i.pos;
            i.pos = i.kept(p);
        }
        // The editor resized itself (not the size it was offered): the frame follows.
        let size = i.surface.view().size;
        if i.seen_size.is_some_and(|s| s != size) {
            let editor = ((size.0 as f32 / k).round(), (size.1 as f32 / k).round());
            if editor != i.st.editor {
                i.st.editor = editor;
                i.chrome_dirty = true;
            }
        }
        i.seen_size = Some(size);
        let (ex, ey) = i.editor_at();
        i.surface.set_room([-ex, -ey, canvas.0 as i32 - ex, canvas.1 as i32 - ey]);
    }

    /// What the editor shows now.
    pub fn view(&self) -> View {
        self.0.borrow().surface.view()
    }

    /// Changes to repaint for: the editor's and the frame's.
    pub fn serial(&self) -> (u64, u64) {
        let mut i = self.0.borrow_mut();
        i.repaint();
        (i.surface.view().serial, i.chrome_serial)
    }

    /// A menu or box of the editor is open: every input goes to the editor.
    pub fn modal(&self) -> bool {
        !self.view().overlays.is_empty()
    }

    /// The editor holds the cursor (a knob drag).
    pub fn locked(&self) -> bool {
        self.view().locked
    }

    /// The editor draws here: it claimed what was offered.
    pub fn claimed(&self) -> bool {
        self.0.borrow().surface.claimed()
    }

    /// Paints the window on the app's canvas (inside the canvas's clip).
    pub fn paint(&self, c: &mut Canvas) {
        let mut i = self.0.borrow_mut();
        i.repaint();
        let at = (dev(i.pos.0, i.k), dev(i.pos.1, i.k));
        c.paint_pixmap(&i.chrome.pixmap, at);
        if i.st.misc {
            return;
        }
        let area = i.editor_area();
        // A card's editor fills it to its bottom edge: its bottom corners follow the card's, inside the outline.
        let radius = match i.st.look.as_ref().filter(|_| i.st.card) {
            Some(t) => {
                let lw = i.k.round().max(1.0) / i.k;
                (os_ui::kit::lemon::r_panel(t) - lw).max(0.0) * i.k
            }
            None => 0.0,
        };
        let v = i.surface.view();
        if let Some(f) = v.frame {
            c.push_clip(area);
            // the editor may be smaller than its area for a moment (resizing): round the area's corners, not its own
            let (ex, ey) = i.editor_at();
            let full = (f.w.min((area.w * i.k).round() as u32), f.h.min((area.h * i.k).round() as u32));
            if radius > 0.0 && full == (f.w, f.h) {
                c.paint_xrgb_rounded(&f.px, (f.w, f.h), (ex, ey), radius);
            } else {
                c.paint_xrgb(&f.px, (f.w, f.h), (ex, ey));
            }
            c.pop_clip();
        }
    }

    /// Paints the editor's open menus and boxes, above everything.
    pub fn paint_overlays(&self, c: &mut Canvas) {
        let i = self.0.borrow();
        let (ex, ey) = i.editor_at();
        for o in i.surface.view().overlays {
            c.paint_xrgb(&o.pixels.px, (o.pixels.w, o.pixels.h), (ex + o.x, ey + o.y));
        }
    }

    /// Where the open menus and boxes are (logical px), for repainting.
    pub fn overlays_rect(&self) -> Option<Rect> {
        let i = self.0.borrow();
        let (ex, ey) = i.editor_at();
        let k = i.k;
        i.surface.view().overlays.iter().map(|o| Rect::new((ex + o.x) as f32 / k, (ey + o.y) as f32 / k, o.pixels.w as f32 / k, o.pixels.h as f32 / k)).reduce(|a, b| a.union(&b))
    }

    /// A canvas point (logical px) as the editor's (device px from its top-left).
    fn editor_point(i: &Inner, x: f32, y: f32) -> (f32, f32) {
        let (ex, ey) = i.editor_at();
        (x * i.k - ex as f32, y * i.k - ey as f32)
    }

    /// A press at canvas point (`x`, `y`). Whatever it went to keeps the moves and the release.
    pub fn press(&self, x: f32, y: f32, btn: Button, mods: (bool, bool, bool)) {
        let mut i = self.0.borrow_mut();
        let in_editor = i.takes(x, y);
        if in_editor || !i.surface.view().overlays.is_empty() {
            let p = Self::editor_point(&i, x, y);
            let now = std::time::Instant::now();
            let dbl = btn == Button::Left && i.last_press.is_some_and(|(t, q)| now.duration_since(t).as_millis() < 400 && (q.0 - p.0).abs() < 4.0 && (q.1 - p.1).abs() < 4.0);
            i.last_press = if dbl { None } else { Some((now, p)) };
            i.grab = Some(Grab::Editor);
            i.surface.push(Input::Down { x: p.0, y: p.1, btn, dbl }, mods);
            return;
        }
        let (fx, fy) = (x - i.pos.0, y - i.pos.1);
        let inner = &mut *i;
        let (pos, k, off) = (inner.pos, inner.k, inner.canvas_off);
        let to_screen = move |x: f32, y: f32| super::x11::main_to_screen((dev(pos.0 + x, k) + off.0, dev(pos.1 + y, k) + off.1));
        let r = match btn {
            Button::Left => {
                let now = std::time::Instant::now();
                let dbl = inner.last_press.is_some_and(|(t, q)| now.duration_since(t).as_millis() < 400 && (q.0 - fx).abs() < 4.0 && (q.1 - fy).abs() < 4.0);
                inner.last_press = if dbl { None } else { Some((now, (fx, fy))) };
                inner.st.left_down(fx, fy, dbl, &to_screen)
            }
            Button::Right => inner.st.right_down(fx, fy, &to_screen),
            Button::Middle => inner.st.middle_down(fx, fy, &to_screen),
        };
        inner.reply(r, (x, y));
        if inner.grab.is_none() {
            inner.grab = Some(Grab::Frame);
        }
    }

    /// The mouse moved to canvas point (`x`, `y`). Whether the window took it (pressed on, or over it).
    pub fn motion(&self, x: f32, y: f32, mods: (bool, bool, bool)) {
        let mut i = self.0.borrow_mut();
        match i.grab {
            Some(Grab::Move { at, from }) => {
                let want = ((from.0 + x - at.0).round(), (from.1 + y - at.1).round());
                i.pos = i.kept(want);
                return;
            }
            Some(Grab::Editor) => {
                let p = Self::editor_point(&i, x, y);
                i.surface.push(Input::Move { x: p.0, y: p.1 }, mods);
                return;
            }
            Some(Grab::Frame) => {
                let (fx, fy) = (x - i.pos.0, y - i.pos.1);
                let r = i.st.motion(fx, fy, mods.0);
                i.reply(r, (x, y));
                return;
            }
            None => {}
        }
        if !i.surface.view().overlays.is_empty() {
            let p = Self::editor_point(&i, x, y);
            i.surface.push(Input::Move { x: p.0, y: p.1 }, mods);
            return;
        }
        let over = i.takes(x, y);
        if over {
            let p = Self::editor_point(&i, x, y);
            i.surface.push(Input::Move { x: p.0, y: p.1 }, mods);
            if i.st.hover.take().is_some() {
                i.chrome_dirty = true;
            }
        } else {
            if i.over_editor {
                i.surface.push(Input::Leave, mods);
            }
            let (fx, fy) = (x - i.pos.0, y - i.pos.1);
            let r = i.st.motion(fx, fy, mods.0);
            i.reply(r, (x, y));
        }
        i.over_editor = over;
    }

    /// The held cursor moved this far (logical px): to the editor holding it.
    pub fn delta(&self, dx: f32, dy: f32, mods: (bool, bool, bool)) {
        let i = self.0.borrow();
        i.surface.push(Input::Delta { dx: dx * i.k, dy: dy * i.k }, mods);
    }

    /// The mouse left the window (or another window is over it now).
    pub fn leave(&self) {
        let mut i = self.0.borrow_mut();
        if i.over_editor && i.grab.is_none() {
            i.over_editor = false;
            i.surface.push(Input::Leave, (false, false, false));
        }
        if i.grab.is_none() && i.st.hover.take().is_some() {
            i.chrome_dirty = true;
        }
    }

    /// A button went up at canvas point (`x`, `y`).
    pub fn release(&self, x: f32, y: f32, btn: Button, mods: (bool, bool, bool)) {
        let mut i = self.0.borrow_mut();
        match i.grab.take() {
            Some(Grab::Editor) => {
                let p = Self::editor_point(&i, x, y);
                i.surface.push(Input::Move { x: p.0, y: p.1 }, mods);
                i.surface.push(Input::Up { btn }, mods);
            }
            None if !i.surface.view().overlays.is_empty() => {
                let p = Self::editor_point(&i, x, y);
                i.surface.push(Input::Move { x: p.0, y: p.1 }, mods);
                i.surface.push(Input::Up { btn }, mods);
            }
            Some(Grab::Move { .. }) => i.st.release(),
            Some(Grab::Frame) | None => {
                let (fx, fy) = (x - i.pos.0, y - i.pos.1);
                let r = if btn == Button::Left { i.st.left_up(fx, fy) } else { Reply::None };
                i.st.release();
                i.chrome_dirty = true;
                i.reply(r, (x, y));
                // a release starts no drag
                i.grab = None;
            }
        }
    }

    /// The wheel turned `notches` (up positive) at canvas point (`x`, `y`).
    pub fn wheel(&self, x: f32, y: f32, notches: f32, mods: (bool, bool, bool)) {
        let mut i = self.0.borrow_mut();
        if i.takes(x, y) {
            let p = Self::editor_point(&i, x, y);
            i.surface.push(Input::Wheel { x: p.0, y: p.1, notches }, mods);
        } else {
            let (fx, fy) = (x - i.pos.0, y - i.pos.1);
            let r = i.st.wheel(fx, fy, notches);
            i.reply(r, (x, y));
        }
    }

    /// A key for the editor (X11 key symbol).
    pub fn key(&self, sym: u32, text: Option<char>, mods: (bool, bool, bool)) {
        self.0.borrow().surface.push(Input::Key { sym, text }, mods);
    }

    /// A press went somewhere else: a drag in the frame ends.
    pub fn cancel(&self) {
        let mut i = self.0.borrow_mut();
        if i.grab.take().is_some() {
            i.st.release();
            i.chrome_dirty = true;
        }
    }

    /// The frame's title-row button at canvas point (`x`, `y`), if any: whether it is the close button (else maximize or
    /// minimize).
    pub fn button_at(&self, x: f32, y: f32) -> Option<bool> {
        let i = self.0.borrow();
        if i.takes(x, y) || !i.surface.view().overlays.is_empty() {
            return None;
        }
        match part_at(&i.st, x - i.pos.0, y - i.pos.1)? {
            Part::Close => Some(true),
            Part::Minimize | Part::Maximize => Some(false),
            _ => None,
        }
    }

    /// A press or drag is under way in the window.
    pub fn grabbed(&self) -> bool {
        self.0.borrow().grab.is_some()
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.surface.withdraw();
    }
}
