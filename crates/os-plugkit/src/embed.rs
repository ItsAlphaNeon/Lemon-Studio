// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Editors drawn inside Lemon Studio's own window (Linux): no window of the editor's own, no X11.
//!
//! The plugins compiled into Lemon Studio share this crate with the app, so the two meet in-process, as the host link's
//! peers do (`os_hostlink`). The app offers a [`Surface`] under a token ([`offer`]) and hands the token to the plugin as
//! the X11 window to open its editor in. The editor's [`spawn`](crate::x11::spawn) claims it ([`claim`]) instead of
//! opening a window: its [`Window`] then takes its input from the app ([`Surface::push`]) and hands it its pixels
//! ([`Window::blit`]); menus and the type-in box are overlays the app draws above everything, with every input going to
//! the editor while one is up ([`Surface::view`]). A copy of a plugin loaded from a file has its own registry and never
//! sees an offer, so the app offers one only to the plugins compiled into it.
//!
//! Coordinates are the editor's device px from its top-left; overlays may reach past it, within [`Surface::set_room`].

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use os_theme::{Color, Palette};
use os_ui::kit::menu as kmenu;
use os_ui::{Canvas, Font, Rect};

use crate::input::{keysym, keysym_char, Button, Input};
use crate::x11::Waker;
use crate::{MenuDef, Opener};

/// The editor's polling period, as the X11 editors' (`x11::TICK`).
const TICK: Duration = Duration::from_millis(30);

/// An editor's pixels: XRGB rows of `w`.
#[derive(Clone, Debug)]
pub struct Pixels {
    pub px: Arc<Vec<u32>>,
    pub w: u32,
    pub h: u32,
}

/// A menu or type-in box above everything, at (`x`, `y`) from the editor's top-left (device px).
#[derive(Clone, Debug)]
pub struct Overlay {
    pub x: i32,
    pub y: i32,
    pub pixels: Pixels,
}

/// What the app shows of an editor now.
#[derive(Clone, Debug, Default)]
pub struct View {
    /// The editor's last frame.
    pub frame: Option<Pixels>,
    /// Open menus and boxes, bottom to top: while there is one, every input goes to the editor.
    pub overlays: Vec<Overlay>,
    /// The size the editor asked for (device px; `Window::resize`), or the size it opened at.
    pub size: (u32, u32),
    /// The editor holds the cursor (a knob drag): moves arrive as `Input::Delta`.
    pub locked: bool,
    /// The editor wants the keyboard (a text field has it): the app sends it the keys while its window is in front.
    pub wants_keys: bool,
    /// Bumped by every change above: the app repaints the editor when it differs.
    pub serial: u64,
}

struct Inner {
    token: u32,
    state: Mutex<State>,
    /// The editor's GUI thread, woken by input (`Surface::push`).
    editor: Mutex<Option<Waker>>,
    /// Wakes the app's loop: there is something new to show.
    host: Box<dyn Fn() + Send + Sync>,
}

#[derive(Default)]
struct State {
    inputs: VecDeque<(Input, (bool, bool, bool))>,
    view: View,
    /// Where overlays may go (left, top, right, bottom; device px from the editor's top-left): the app's window.
    room: [i32; 4],
    claimed: bool,
}

/// The meeting point of the app and one editor.
#[derive(Clone)]
pub struct Surface(Arc<Inner>);

fn offers() -> MutexGuard<'static, HashMap<u32, Surface>> {
    static OFFERS: OnceLock<Mutex<HashMap<u32, Surface>>> = OnceLock::new();
    OFFERS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// App side: a surface for an editor of `size` device px, under a token of its own (`Surface::token`), which the app
/// passes to the plugin as the window to open its editor in. `wake` wakes the app's loop.
pub fn offer(size: (u32, u32), wake: impl Fn() + Send + Sync + 'static) -> Surface {
    // Far above the ids an X server hands out (its resource ids stay below 2^29).
    static NEXT: AtomicU32 = AtomicU32::new(0xe000_0000);
    let token = NEXT.fetch_add(1, Ordering::Relaxed);
    let state = State { view: View { size, ..View::default() }, room: [i32::MIN / 2, i32::MIN / 2, i32::MAX / 2, i32::MAX / 2], ..State::default() };
    let s = Surface(Arc::new(Inner { token, state: Mutex::new(state), editor: Mutex::new(None), host: Box::new(wake) }));
    offers().insert(token, s.clone());
    s
}

/// Editor side: whether the host offered a surface under `token` (the window it gave): the editor will be an embedded
/// one (`claim`, which `x11::spawn` does).
pub fn offered(token: u32) -> bool {
    offers().contains_key(&token)
}

/// Editor side: the surface offered under `token` (the window the host gave), if any; taking it makes the editor an
/// embedded one.
pub fn claim(token: u32) -> Option<Surface> {
    let s = offers().get(&token).cloned()?;
    s.lock().claimed = true;
    Some(s)
}

impl Surface {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn changed(&self, f: impl FnOnce(&mut View)) {
        {
            let mut st = self.lock();
            f(&mut st.view);
            st.view.serial += 1;
        }
        (self.0.host)();
    }

    /// The token the editor is opened with (as an X11 window id).
    pub fn token(&self) -> u32 {
        self.0.token
    }

    /// An editor took the surface: it draws here.
    pub fn claimed(&self) -> bool {
        self.lock().claimed
    }

    /// Input for the editor, with Ctrl, Shift and Alt as they were.
    pub fn push(&self, input: Input, mods: (bool, bool, bool)) {
        self.lock().inputs.push_back((input, mods));
        if let Some(w) = self.0.editor.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            w.wake();
        }
    }

    /// What there is to show.
    pub fn view(&self) -> View {
        self.lock().view.clone()
    }

    /// Where the editor's menus may go: the app's window, from the editor's top-left (device px).
    pub fn set_room(&self, room: [i32; 4]) {
        self.lock().room = room;
    }

    /// The editor is gone: no new editor claims this surface.
    pub fn withdraw(&self) {
        offers().remove(&self.0.token);
    }
}

/// An editor drawn in the app's window: the embedded counterpart of the X11 editor window, with the same calls.
pub struct Window {
    s: Surface,
    waker: Waker,
    mods: (bool, bool, bool),
    /// The pointer at the last input (device px from the editor's top-left): menus pick where a release happens.
    pointer: (i32, i32),
    next_tick: Instant,
}

impl Window {
    pub(crate) fn new(s: Surface, waker: Waker) -> Window {
        *s.0.editor.lock().unwrap_or_else(|e| e.into_inner()) = Some(waker.clone());
        Window { s, waker, mods: (false, false, false), pointer: (0, 0), next_tick: Instant::now() + TICK }
    }

    pub fn idle(&self) -> bool {
        self.s.lock().inputs.is_empty()
    }

    pub fn mods(&self) -> (bool, bool, bool) {
        self.mods
    }

    fn take(&mut self) -> Option<Input> {
        let (i, m) = self.s.lock().inputs.pop_front()?;
        self.mods = m;
        match i {
            Input::Down { x, y, .. } | Input::Move { x, y } | Input::Wheel { x, y, .. } => self.pointer = (x as i32, y as i32),
            _ => {}
        }
        Some(i)
    }

    /// The next input, waiting for it; `None` when the editor closes.
    pub fn next(&mut self) -> Option<Input> {
        loop {
            if self.waker.quitting() {
                return None;
            }
            if let Some(i) = self.take() {
                return Some(i);
            }
            let now = Instant::now();
            if now >= self.next_tick {
                self.next_tick = now + TICK;
                return Some(Input::Tick);
            }
            let ms = (self.next_tick - now).as_millis() as i32 + 1;
            // A wake with no input waiting: a parameter changed.
            if self.waker.wait(ms) && self.idle() {
                return if self.waker.quitting() { None } else { Some(Input::Tick) };
            }
        }
    }

    /// The next input of a menu or box; `None` when the editor closes.
    fn modal(&mut self) -> Option<Input> {
        loop {
            if self.waker.quitting() {
                return None;
            }
            if let Some(i) = self.take() {
                return Some(i);
            }
            self.waker.wait(100);
        }
    }

    /// The editor's new frame.
    pub fn blit(&self, px: &[u32], w: u32, h: u32) {
        let n = (w * h) as usize;
        if px.len() < n {
            return;
        }
        let p = Pixels { px: Arc::new(px[..n].to_vec()), w, h };
        self.s.changed(|v| v.frame = Some(p));
    }

    /// The editor's new size (device px).
    pub fn resize(&self, w: u32, h: u32) {
        self.s.changed(|v| v.size = (w.max(1), h.max(1)));
    }

    /// Menus and boxes are placed from the editor's top-left: the point as it is.
    pub fn to_root(&self, x: i32, y: i32) -> (i32, i32) {
        (x, y)
    }

    pub fn dialog<R: Send + 'static>(&self, f: impl FnOnce() -> R + Send + 'static) -> Option<R> {
        Some(f())
    }

    pub fn locked(&self) -> bool {
        self.s.lock().view.locked
    }

    /// The app hides the cursor and holds it: movement arrives as `Input::Delta`.
    pub fn lock(&mut self) {
        if !self.locked() {
            self.s.changed(|v| v.locked = true);
        }
    }

    pub fn unlock(&mut self) {
        if self.locked() {
            self.s.changed(|v| v.locked = false);
        }
    }

    /// Whether a text field of the editor has the keyboard: the app then sends it the keys.
    pub fn set_wants_keys(&self, on: bool) {
        if self.s.lock().view.wants_keys != on {
            self.s.changed(|v| v.wants_keys = on);
        }
    }

    fn room(&self) -> [i32; 4] {
        self.s.lock().room
    }

    fn show(&self, c: &Canvas, x: i32, y: i32, replace: bool) {
        let (w, h) = (c.pixmap.width(), c.pixmap.height());
        let mut px = vec![0u32; (w * h) as usize];
        c.copy_to_xrgb(&mut px);
        let o = Overlay { x, y, pixels: Pixels { px: Arc::new(px), w, h } };
        self.s.changed(|v| {
            if replace {
                v.overlays.pop();
            }
            v.overlays.push(o);
        });
    }

    fn close_overlay(&self) {
        self.s.changed(|v| {
            v.overlays.pop();
        });
    }

    // ---------------------------------------------------------------------------------------------------------------
    // Popup menus, as the X11 editors' (`x11::Window::popup`), drawn as overlays.

    /// Shows a menu at (`x`, `y`); the picked entry's command.
    pub fn popup<C: Clone>(&mut self, x: i32, y: i32, def: &MenuDef<C>, pal: &Palette, scale: f32) -> Option<C> {
        if crate::no_modal() {
            return None;
        }
        self.unlock();
        match self.popup_at(x, y, def, pal, scale, None) {
            PopRes::Pick(c) => Some(c),
            _ => None,
        }
    }

    fn popup_at<C: Clone>(&mut self, x: i32, y: i32, def: &MenuDef<C>, pal: &Palette, scale: f32, parent: Option<Opener>) -> PopRes<C> {
        let Ok(mut measure) = Canvas::new(8, 8, scale) else { return PopRes::Cancel };
        let layout = kmenu::layout(&mut measure, &def.items, def.style);
        let (w, h) = ((layout.w * scale).ceil() as i32, (layout.h * scale).ceil() as i32);
        let room = self.room();
        let (mut px, mut py) = (x, y);
        if px + w > room[2] {
            px = match parent {
                Some(p) => p.menu[0] - w,
                None => room[2] - w,
            };
        }
        if py + h > room[3] {
            py = (room[3] - h).max(room[1]);
        }
        let Ok(mut canvas) = Canvas::new(w.max(1) as u32, h.max(1) as u32, scale) else { return PopRes::Cancel };
        let mut hover: Option<usize> = None;
        let paint = |canvas: &mut Canvas, hover: Option<usize>| {
            canvas.begin_frame(None);
            kmenu::paint(canvas, pal, &def.items, &layout, def.style, 0.0, 0.0, hover, def.current);
        };
        paint(&mut canvas, hover);
        self.show(&canvas, px, py, false);
        let rect = [px, py, px + w, py + h];
        let inside = |r: &[i32; 4], (sx, sy): (i32, i32)| sx >= r[0] && sx < r[2] && sy >= r[1] && sy < r[3];
        let item_at = |(sx, sy): (i32, i32)| layout.item_at(&def.items, (sx - px) as f32 / scale, (sy - py) as f32 / scale);
        let opened = Instant::now();
        let start = self.pointer;
        let mut moved = false;
        let result = loop {
            let Some(e) = self.modal() else { break PopRes::Cancel };
            match e {
                Input::Move { .. } => {
                    let cur = self.pointer;
                    if (cur.0 - start.0).abs() + (cur.1 - start.1).abs() > 2 {
                        moved = true;
                    }
                    if parent.is_some_and(|p| p.takes_back(rect, cur)) {
                        break PopRes::Back;
                    }
                    let hov = if inside(&rect, cur) { item_at(cur) } else { None }.filter(|&i| def.items[i].clickable());
                    if hov != hover {
                        hover = hov;
                        paint(&mut canvas, hover);
                        self.show(&canvas, px, py, true);
                    }
                    if let Some((i, sub)) = hov.and_then(|i| Some((i, def.sub(i)?))) {
                        let row = layout.rows[i];
                        let sx = px + ((row.x + row.w) * scale) as i32 - 2;
                        let sy = py + (row.y * scale) as i32 - 2;
                        match self.popup_at(sx, sy, sub, pal, scale, Some(Opener::new(rect, row, scale))) {
                            PopRes::Pick(c) => break PopRes::Pick(c),
                            PopRes::Cancel => break PopRes::Cancel,
                            PopRes::Back => {}
                        }
                    }
                }
                Input::Down { .. } => {
                    if !inside(&rect, self.pointer) {
                        break PopRes::Cancel;
                    }
                    moved = true;
                }
                Input::Up { btn: Button::Left | Button::Right } => {
                    // The release of the click that opened the menu does not pick.
                    if !moved && opened.elapsed() < Duration::from_millis(400) {
                        continue;
                    }
                    let cur = self.pointer;
                    if let Some(i) = inside(&rect, cur).then(|| item_at(cur)).flatten() {
                        if def.items[i].clickable() && def.sub(i).is_none() {
                            break PopRes::Pick(def.cmds[i].clone());
                        }
                    }
                }
                Input::Key { sym, .. } => {
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
                            self.show(&canvas, px, py, true);
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
        self.close_overlay();
        result
    }

    // ---------------------------------------------------------------------------------------------------------------
    // "Type in value", as the X11 editors' (`x11::Window::type_in`), drawn as an overlay.

    pub fn type_in(&mut self, x: i32, y: i32, text: &str, scale: f32) -> Option<String> {
        if crate::no_modal() {
            return None;
        }
        self.unlock();
        let (w, h) = ((140.0 * scale).round() as i32, (22.0 * scale).round() as i32);
        let room = self.room();
        let x = x.min(room[2] - w).max(room[0]);
        let y = y.min(room[3] - h).max(room[1]);
        let mut canvas = Canvas::new(w as u32, h as u32, scale).ok()?;
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
        self.show(&canvas, x, y, false);
        let r = [x, y, x + w, y + h];
        let result = loop {
            let Some(e) = self.modal() else { break None };
            match e {
                Input::Down { .. } => {
                    let (sx, sy) = self.pointer;
                    if sx < r[0] || sx >= r[2] || sy < r[1] || sy >= r[3] {
                        break None;
                    }
                    all = false;
                    caret = s.len();
                }
                Input::Key { sym, .. } => {
                    let ctrl = self.mods.0;
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
                    self.show(&canvas, x, y, true);
                }
                _ => {}
            }
        };
        self.close_overlay();
        result
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        *self.s.0.editor.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.s.changed(|v| {
            v.overlays.clear();
            v.locked = false;
        });
    }
}

enum PopRes<C> {
    Pick(C),
    Cancel,
    /// The mouse went back to the parent menu.
    Back,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MenuDef;
    use os_ui::kit::menu::{MenuItem, MenuStyle};
    use std::sync::atomic::AtomicUsize;

    fn editor(size: (u32, u32)) -> (Surface, Arc<AtomicUsize>) {
        let woken = Arc::new(AtomicUsize::new(0));
        let w = woken.clone();
        (offer(size, move || drop(w.fetch_add(1, Ordering::Relaxed))), woken)
    }

    /// The editor claims what the app offered; its frames and size reach the app, the app's input reaches it.
    #[test]
    fn frames_and_input_go_both_ways() {
        let (s, woken) = editor((40, 30));
        assert!(!s.claimed());
        let slot = crate::x11::Slot::default();
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = got.clone();
        let t = crate::x11::spawn(s.token(), (40, 30), &slot, move |mut win| {
            win.blit(&vec![0x00ff_0000; 40 * 30], 40, 30);
            while let Some(i) = win.next() {
                if i != Input::Tick {
                    g.lock().unwrap().push((i.clone(), win.mods()));
                    if matches!(i, Input::Key { .. }) {
                        win.resize(50, 20);
                    }
                }
            }
        })
        .expect("an embedded editor needs no display");
        let wait = |f: &dyn Fn() -> bool| {
            let t0 = Instant::now();
            while !f() && t0.elapsed() < Duration::from_secs(2) {
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        wait(&|| s.view().frame.is_some());
        assert!(s.claimed());
        let f = s.view().frame.expect("its frame");
        assert_eq!((f.w, f.h, f.px[0]), (40, 30, 0x00ff_0000));
        assert!(woken.load(Ordering::Relaxed) > 0, "the app was woken");
        s.push(Input::Down { x: 3.0, y: 4.0, btn: Button::Left, dbl: false }, (true, false, false));
        s.push(Input::Key { sym: keysym::RETURN, text: None }, (false, false, false));
        wait(&|| s.view().size == (50, 20));
        let got = got.lock().unwrap().clone();
        assert_eq!(got[0], (Input::Down { x: 3.0, y: 4.0, btn: Button::Left, dbl: false }, (true, false, false)));
        assert!(matches!(got[1].0, Input::Key { sym: keysym::RETURN, .. }));
        drop(t);
        s.withdraw();
        assert!(claim(s.token()).is_none(), "withdrawn");
    }

    /// A menu is an overlay: the app shows it, the editor gets the clicks on it, and it goes once a row is picked.
    #[test]
    fn menus_are_overlays() {
        let (s, _) = editor((200, 100));
        let slot = crate::x11::Slot::default();
        let picked = Arc::new(Mutex::new(None));
        let p = picked.clone();
        let t = crate::x11::spawn(s.token(), (200, 100), &slot, move |mut win| {
            let mut def = MenuDef::new(MenuStyle::List);
            def.add(MenuItem::item("One"), 1);
            def.add(MenuItem::item("Two"), 2);
            *p.lock().unwrap() = Some(win.popup(10, 10, &def, &crate::palette(), 1.0));
        })
        .unwrap();
        let t0 = Instant::now();
        while s.view().overlays.is_empty() && t0.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(5));
        }
        let o = s.view().overlays[0].clone();
        assert_eq!((o.x, o.y), (10, 10));
        // the second row: below the first
        let mut m = Canvas::new(8, 8, 1.0).unwrap();
        let mut def = MenuDef::new(MenuStyle::List);
        def.add(MenuItem::item("One"), 1);
        def.add(MenuItem::item("Two"), 2);
        let layout = kmenu::layout(&mut m, &def.items, def.style);
        let row = layout.rows[1];
        let (x, y) = (10.0 + row.x + row.w / 2.0, 10.0 + row.y + row.h / 2.0);
        std::thread::sleep(Duration::from_millis(450));
        s.push(Input::Move { x, y }, (false, false, false));
        s.push(Input::Down { x, y, btn: Button::Left, dbl: false }, (false, false, false));
        s.push(Input::Up { btn: Button::Left }, (false, false, false));
        let t0 = Instant::now();
        while picked.lock().unwrap().is_none() && t0.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(5));
        }
        drop(t);
        assert_eq!(*picked.lock().unwrap(), Some(Some(2)));
        assert!(s.view().overlays.is_empty(), "the menu went");
    }
}
