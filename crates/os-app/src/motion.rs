// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's motions between frames, at the level General ▸ Display ▸ Animations sets (`crate::anim`): popup
//! menus appearing and their highlight gliding, the drag tag swinging, hover helpers fading, clips flashing.
//!
//! Painting reads and moves them (the state keeps them in a `RefCell`, as it does the cables) and notes where it drew
//! one that still moves (`Motion::live`), and in which window; every tick repaints there (`App::motion_tick`) and the
//! shell ticks at animation rate while there is any (`App::animating`). A paint of a window starts its list afresh
//! (`Motion::begin_frame`), so a motion that has ended stops asking for frames by itself.
// spec: ui/animations.md

use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use os_ui::{Canvas, Layer, Rect};

use crate::anim::{self, Level, Pendulum, Spring, Tween};
use crate::app::Win;

/// The menus that can be open at once: a popup menu, its submenu, and a menu bar dropdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Popup = 0,
    Sub = 1,
    Dropdown = 2,
}

/// A popup menu opens in 120 ms, a submenu in 100 ms (spec: ui/animations.md §4.2).
const MENU_OPEN: Duration = Duration::from_millis(120);
const SUB_OPEN: Duration = Duration::from_millis(100);
/// How far a menu slides on opening (toward where it was opened from), a submenu sideways, and how small it starts.
const MENU_SLIDE: f32 = 6.0;
const SUB_SLIDE: f32 = 8.0;
const MENU_ZOOM: f32 = 0.97;
/// The highlight bar's spring: critically damped, settled in about 60 ms.
const GLIDE_HZ: f64 = 15.5;
/// Hover helpers fade in over 120 ms and out over 200 ms (§4.6).
const HOVER_IN: f64 = 0.120;
const HOVER_OUT: f64 = 0.200;
/// A functional flash lasts 400 ms and peaks at 60 % (§4.7).
pub const FLASH: Duration = Duration::from_millis(400);
const FLASH_PEAK: f32 = 0.6;
/// The drag tag: where it hangs from (below and right of the mouse), its height, and how wide it may grow (§4.1).
pub const TAG_PIN: (f32, f32) = (12.0, 14.0);
pub const TAG_H: f32 = 24.0;
pub const TAG_MAX_W: f32 = 220.0;

#[derive(Default)]
pub struct Motion {
    /// The window being painted: None for the main one, or a detached one.
    win: Option<Win>,
    /// Where motions that still move were drawn by the last paint of each window (logical px of that window), and
    /// when.
    live: Vec<Live>,
    menus: [Option<Opened>; 3],
    glides: [Option<Glide>; 3],
    tag: Option<Tag>,
    hovers: Vec<Hover>,
    flash: Option<Flash>,
}

struct Opened {
    key: u64,
    at: Instant,
}

/// A highlight bar: x, y, width, height, each on its spring; the row it goes to and when it was last stepped.
struct Glide {
    key: u64,
    rect: [Spring; 4],
    to: Rect,
    last: Instant,
}

/// The drag tag's swing.
struct Tag {
    pend: Pendulum,
    level: Level,
    last: Instant,
}

/// Something drawn in `r` of window `win` (None: the main one) at `at` still moves.
#[derive(Debug, Clone, Copy)]
pub struct Live {
    pub win: Option<Win>,
    pub r: Rect,
    at: Instant,
}

/// A hover helper's opacity, 0 to 1, and which way it goes.
struct Hover {
    key: u64,
    val: f64,
    on: bool,
    last: Instant,
}

/// Clips lit together: those playing `what`, since `at`.
struct Flash {
    what: u64,
    at: Instant,
}

/// How a menu shows this frame (`Motion::menu_begin`, then `menu_end` once it is painted).
pub struct MenuDraw {
    layer: Option<Layer>,
    alpha: f32,
    zoom: f32,
    dx: f32,
    dy: f32,
    pivot: (f32, f32),
}

/// A key for what `v` is (a menu's entries and where it opened, a clip's source).
pub fn key(v: impl Hash) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

/// Whether menus' highlight bars glide (from Polished up), or jump.
pub fn glides() -> bool {
    !anim::instant() && anim::level() >= Level::Polished
}

fn since(now: Instant, then: Instant) -> f64 {
    now.saturating_duration_since(then).as_secs_f64()
}

impl Motion {
    /// Window `win` (None: the main one) paints: what moves there is noted again as it is drawn. Hover fades no paint
    /// has asked about for a second are forgotten (their clip scrolled away).
    pub fn begin_frame(&mut self, win: Option<Win>) {
        self.win = win;
        self.live.retain(|l| l.win != win);
        let now = anim::now();
        self.hovers.retain(|h| since(now, h.last) < 1.0);
        if self.flash.as_ref().is_some_and(|f| since(now, f.at) > FLASH.as_secs_f64()) {
            self.flash = None;
        }
    }

    /// Something drawn in `r` (of the window being painted) still moves: the next frame paints it again.
    pub fn live(&mut self, r: Rect) {
        if r.w > 0.0 && r.h > 0.0 {
            self.live.push(Live { win: self.win, r, at: anim::now() });
        }
    }

    /// Where to paint again on this tick. A window that has not painted what it noted for a second (closed, hidden)
    /// is no longer asked to.
    pub fn damage(&mut self) -> &[Live] {
        let now = anim::now();
        self.live.retain(|l| since(now, l.at) < 1.0);
        &self.live
    }

    /// Whether anything still moves: the shell keeps ticking at animation rate.
    pub fn moving(&self) -> bool {
        !self.live.is_empty() || self.tag.as_ref().is_some_and(|t| t.pend.moving())
    }

    // ------------------------------------------------------------------------------------------------ menus

    /// The menu in `slot` closed (or was never open): it opens afresh next time.
    pub fn menu_closed(&mut self, slot: Slot) {
        self.menus[slot as usize] = None;
        self.glides[slot as usize] = None;
    }

    /// Menu `slot`, which is `key` (its entries and where it opened), is about to be painted in `r` (the canvas's
    /// logical px, `k` times the window's), opened from `anchor`. While it appears, it is painted into a layer that
    /// `menu_end` draws faded, slid and scaled (spec: ui/animations.md §4.2): Subtle fades it in, Polished slides it
    /// from the anchor too, Playful also grows it from 97 %. A submenu slides out sideways from its row. At Off it is
    /// there at once.
    pub fn menu_begin(&mut self, c: &mut Canvas, slot: Slot, key: u64, r: Rect, anchor: (f32, f32), k: f32) -> MenuDraw {
        let now = anim::now();
        let i = slot as usize;
        if self.menus[i].as_ref().is_none_or(|o| o.key != key) {
            self.menus[i] = Some(Opened { key, at: now });
        }
        let level = anim::level();
        let full = if slot == Slot::Sub { SUB_OPEN } else { MENU_OPEN };
        let at = self.menus[i].as_ref().map_or(now, |o| o.at);
        let tw = Tween::new(at, level.dur(full));
        let pivot = (anchor.0.clamp(r.x, r.right()), anchor.1.clamp(r.y, r.bottom()));
        let mut d = MenuDraw { layer: None, alpha: 1.0, zoom: 1.0, dx: 0.0, dy: 0.0, pivot };
        if anim::instant() || tw.done(now) {
            return d;
        }
        let u = tw.eased(now) as f32;
        d.alpha = u;
        if level >= Level::Polished {
            if slot == Slot::Sub {
                // Out of its row: from the left when it opens to the right of it.
                let side = if r.cx() >= anchor.0 { 1.0 } else { -1.0 };
                d.dx = -SUB_SLIDE * (1.0 - u) * side;
            } else {
                // Away from where it was opened: a menu below its anchor starts higher up.
                let side = if r.cy() >= anchor.1 { 1.0 } else { -1.0 };
                d.dy = -MENU_SLIDE * (1.0 - u) * side;
            }
        }
        if level == Level::Playful && slot != Slot::Sub {
            d.zoom = MENU_ZOOM + (1.0 - MENU_ZOOM) * u;
        }
        let reach = r.outset(SUB_SLIDE + 2.0);
        self.live(Rect::new(reach.x * k, reach.y * k, reach.w * k, reach.h * k));
        d.layer = Some(c.begin_layer(r.outset(1.0), true));
        d
    }

    /// Draws the menu painted since `menu_begin` as it shows this frame.
    pub fn menu_end(c: &mut Canvas, d: MenuDraw) {
        if let Some(l) = d.layer
            && let Some(s) = c.take_layer(l)
        {
            c.sprite(&s, d.pivot.0, d.pivot.1, 0.0, d.zoom, d.dx, d.dy, d.alpha);
        }
    }

    /// Where the highlight bar of menu `slot` (which is `key`) is drawn this frame, on its way to `to`, the row under
    /// the mouse (spec: ui/animations.md §4.2): from Polished up it glides there on a critically damped spring;
    /// below, and when it first shows, it is there at once. `area` (the menu, window px) is painted again while it
    /// moves.
    pub fn glide(&mut self, slot: Slot, key: u64, to: Option<Rect>, area: Rect) -> Option<Rect> {
        let i = slot as usize;
        let Some(to) = to else {
            self.glides[i] = None;
            return None;
        };
        if !glides() {
            self.glides[i] = None;
            return Some(to);
        }
        let now = anim::now();
        let g = match &mut self.glides[i] {
            Some(g) if g.key == key => g,
            g => {
                *g = Some(Glide { key, rect: [to.x, to.y, to.w, to.h].map(|v| Spring::at(f64::from(v))), to, last: now });
                return Some(to);
            }
        };
        if g.to != to {
            // A new row: it sets off from where it is now, from this frame.
            g.to = to;
            g.last = now;
            for (s, v) in g.rect.iter_mut().zip([to.x, to.y, to.w, to.h]) {
                s.target = f64::from(v);
            }
        }
        let dt = since(now, g.last).min(0.05);
        g.last = now;
        let mut still = true;
        for s in &mut g.rect {
            still &= s.step(dt, GLIDE_HZ, 1.0, 0.2);
        }
        let [x, y, w, h] = g.rect.map(|s| s.x as f32);
        if !still {
            self.live(area);
        }
        Some(Rect::new(x, y, w, h))
    }

    // ------------------------------------------------------------------------------------------------ drag tag

    /// The tick: the drag tag follows the mouse at (`x`, `y`) (None: nothing is dragged). It swings from Polished up
    /// (spec: ui/animations.md §4.1); below, it hangs rigid. Returns where to paint again when it moved.
    pub fn tag_tick(&mut self, at: Option<(f32, f32)>) -> Option<Rect> {
        let level = anim::level();
        let Some((x, y)) = at.filter(|_| !anim::instant() && level >= Level::Polished) else {
            self.tag = None;
            return None;
        };
        let now = anim::now();
        let t = match &mut self.tag {
            Some(t) if t.level == level => t,
            t => t.insert(Tag { pend: Pendulum::new(level), level, last: now }),
        };
        let dt = since(now, t.last);
        t.last = now;
        let was = t.pend.moving();
        t.pend.update(f64::from(x), dt);
        // The frame it comes to rest on is painted too.
        (was || t.pend.moving()).then(|| tag_reach(x, y))
    }

    /// The drag tag's angle (radians, clockwise on screen): 0 hanging straight down.
    pub fn tag_angle(&self) -> f32 {
        // The pendulum's angle is positive with the card's bottom to the right, which is a turn the other way.
        self.tag.as_ref().map_or(0.0, |t| -(t.pend.theta as f32))
    }

    // ------------------------------------------------------------------------------------------------ hover fades

    /// How much of hover helper `key` (a clip's handles, a slot's arrow) shows this frame, 0 to 1, as the mouse is
    /// over it (`on`) or not: 120 ms in, 200 ms out from Subtle up, at once at Off (spec: ui/animations.md §4.6).
    /// `area` (window px) is painted again while it fades.
    pub fn hover(&mut self, key: u64, on: bool, area: Rect) -> f32 {
        let now = anim::now();
        let pos = self.hovers.iter().position(|h| h.key == key);
        if anim::instant() {
            if let Some(i) = pos {
                self.hovers.swap_remove(i);
            }
            return if on { 1.0 } else { 0.0 };
        }
        let i = match pos {
            Some(i) => i,
            None if on => {
                self.hovers.push(Hover { key, val: 0.0, on, last: now });
                self.hovers.len() - 1
            }
            None => return 0.0,
        };
        let h = &mut self.hovers[i];
        if h.on != on {
            // It turns from this frame (it may have sat still, unpainted, for a while).
            h.on = on;
            h.last = now;
        }
        let dt = since(now, h.last);
        h.last = now;
        h.val = if on { (h.val + dt / HOVER_IN).min(1.0) } else { (h.val - dt / HOVER_OUT).max(0.0) };
        let v = h.val;
        if (on && v < 1.0) || (!on && v > 0.0) {
            self.live(area);
        }
        if !on && v <= 0.0 {
            self.hovers.swap_remove(i);
        }
        anim::ease_out(v) as f32
    }

    // ------------------------------------------------------------------------------------------------ flashes

    /// Lights everything that is `what` (the clips playing one source, after a double-click on one of them): a pulse
    /// up to 60 % and back over 400 ms; at Off a still tint for as long (feedback: it shows at every level, spec:
    /// ui/animations.md §4.7).
    pub fn flash(&mut self, what: u64) {
        if anim::still() {
            return;
        }
        self.flash = Some(Flash { what, at: anim::now() });
    }

    /// How strongly something that is `what` is lit this frame, 0 to `FLASH_PEAK`; `area` (window px) is painted
    /// again until the flash is over.
    pub fn flash_of(&mut self, what: u64, area: Rect) -> f32 {
        let Some(f) = self.flash.as_ref().filter(|f| f.what == what) else { return 0.0 };
        let u = since(anim::now(), f.at) / FLASH.as_secs_f64();
        if u >= 1.0 {
            // One more frame clears it.
            return 0.0;
        }
        self.live(area);
        if anim::level() == Level::Off {
            return FLASH_PEAK;
        }
        // Up quickly, down slowly.
        let a = if u < 0.25 { u / 0.25 } else { 1.0 - (u - 0.25) / 0.75 };
        FLASH_PEAK * anim::ease_out(a) as f32
    }
}

/// Everything the drag tag hanging from the mouse at (`x`, `y`) can cover, whichever way it swings: turned, a wide
/// card's top corner rises above its pin.
pub fn tag_reach(x: f32, y: f32) -> Rect {
    let r = (TAG_MAX_W / 2.0 + 6.0).hypot(TAG_H + 8.0) + 2.0;
    let (px, py) = (x + TAG_PIN.0, y + TAG_PIN.1);
    Rect::new(px - r, py - r, 2.0 * r, 2.0 * r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menus_appear_by_time() {
        anim::set_clock(true);
        let Ok(mut c) = Canvas::new(200, 200, 1.0) else { return };
        let mut m = Motion::default();
        let r = Rect::new(20.0, 20.0, 100.0, 80.0);
        let d = m.menu_begin(&mut c, Slot::Popup, 1, r, (20.0, 20.0), 1.0);
        assert!(d.layer.is_some() && d.alpha == 0.0 && d.zoom == MENU_ZOOM && d.dy == -MENU_SLIDE, "it starts small, clear and higher up");
        Motion::menu_end(&mut c, d);
        assert!(m.moving());
        anim::advance(60);
        m.begin_frame(None);
        let d = m.menu_begin(&mut c, Slot::Popup, 1, r, (20.0, 20.0), 1.0);
        assert!(d.alpha > 0.8 && d.alpha < 1.0 && d.dy < 0.0 && d.dy > -1.0, "eased: most of the way at half time ({}, {})", d.alpha, d.dy);
        Motion::menu_end(&mut c, d);
        anim::advance(61);
        m.begin_frame(None);
        let d = m.menu_begin(&mut c, Slot::Popup, 1, r, (20.0, 20.0), 1.0);
        assert!(d.layer.is_none() && !m.moving(), "open after 120 ms");
        // Another menu (other entries, or opened elsewhere) appears afresh; Subtle only fades, quicker.
        anim::set_level(Level::Subtle);
        let d = m.menu_begin(&mut c, Slot::Popup, 2, r, (20.0, 20.0), 1.0);
        assert!(d.layer.is_some() && d.dy == 0.0 && d.zoom == 1.0);
        Motion::menu_end(&mut c, d);
        anim::advance(75);
        m.begin_frame(None);
        assert!(m.menu_begin(&mut c, Slot::Popup, 2, r, (20.0, 20.0), 1.0).layer.is_none(), "70 ms at Subtle");
        // A submenu opening to the right of its row comes out of it from the left.
        anim::set_level(Level::Polished);
        let d = m.menu_begin(&mut c, Slot::Sub, 3, Rect::new(120.0, 30.0, 60.0, 40.0), (120.0, 30.0), 1.0);
        assert!(d.dx == -SUB_SLIDE && d.dy == 0.0 && d.zoom == 1.0);
        Motion::menu_end(&mut c, d);
        anim::set_level(Level::Off);
        assert!(m.menu_begin(&mut c, Slot::Popup, 4, r, (20.0, 20.0), 1.0).layer.is_none(), "at once at Off");
        anim::set_level(Level::Playful);
        anim::set_clock(false);
    }

    #[test]
    fn highlight_glides_between_rows() {
        anim::set_clock(true);
        let mut m = Motion::default();
        let area = Rect::new(0.0, 0.0, 100.0, 200.0);
        let (a, b) = (Rect::new(2.0, 10.0, 96.0, 20.0), Rect::new(2.0, 70.0, 96.0, 20.0));
        assert_eq!(m.glide(Slot::Popup, 1, Some(a), area), Some(a), "it shows where it is first");
        anim::advance(16);
        let r = m.glide(Slot::Popup, 1, Some(b), area).unwrap();
        assert_eq!(r.y, 10.0, "sets off from the old row");
        let mut ys = vec![];
        for _ in 0..10 {
            anim::advance(16);
            m.begin_frame(None);
            ys.push(m.glide(Slot::Popup, 1, Some(b), area).unwrap().y);
        }
        assert!(ys[0] > 10.0 && ys[0] < 70.0 && ys.windows(2).all(|w| w[1] >= w[0]) && ys.iter().all(|&y| y <= 70.0), "no overshoot: {ys:?}");
        assert!(ys[3] > 66.0, "about there in 60 ms: {ys:?}");
        assert_eq!(*ys.last().unwrap(), 70.0);
        assert!(!m.moving());
        // Subtle: it jumps.
        anim::set_level(Level::Subtle);
        assert_eq!(m.glide(Slot::Popup, 1, Some(a), area), Some(a));
        anim::set_level(Level::Playful);
        anim::set_clock(false);
    }

    #[test]
    fn hover_fades_in_and_out() {
        anim::set_clock(true);
        let mut m = Motion::default();
        let area = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert_eq!(m.hover(7, true, area), 0.0);
        anim::advance(60);
        let half = m.hover(7, true, area);
        assert!(half > 0.5 && half < 1.0, "{half}");
        anim::advance(70);
        assert_eq!(m.hover(7, true, area), 1.0);
        m.begin_frame(None);
        assert_eq!(m.hover(7, true, area), 1.0);
        assert!(!m.moving(), "still once in");
        anim::advance(100);
        let out = m.hover(7, false, area);
        assert_eq!(out, 1.0, "fading out starts from this frame");
        anim::advance(100);
        let out = m.hover(7, false, area);
        assert!(out > 0.0 && out < 1.0 && m.moving(), "{out}");
        anim::advance(120);
        assert_eq!(m.hover(7, false, area), 0.0);
        assert!(m.hovers.is_empty());
        // At Off: at once.
        anim::set_level(Level::Off);
        assert_eq!(m.hover(8, true, area), 1.0);
        anim::set_level(Level::Playful);
        anim::set_clock(false);
    }

    #[test]
    fn flashes_pulse_and_hold_at_off() {
        anim::set_clock(true);
        let mut m = Motion::default();
        let area = Rect::new(0.0, 0.0, 10.0, 10.0);
        m.flash(5);
        assert_eq!(m.flash_of(5, area), 0.0, "it rises");
        assert_eq!(m.flash_of(6, area), 0.0, "only what was asked for");
        anim::advance(100);
        assert!((m.flash_of(5, area) - FLASH_PEAK).abs() < 1e-3);
        anim::advance(200);
        let late = m.flash_of(5, area);
        assert!(late > 0.0 && late < FLASH_PEAK);
        anim::advance(101);
        m.begin_frame(None);
        assert_eq!(m.flash_of(5, area), 0.0);
        assert!(!m.moving());
        anim::set_level(Level::Off);
        m.flash(5);
        assert_eq!(m.flash_of(5, area), FLASH_PEAK, "a still tint at Off");
        anim::advance(399);
        assert_eq!(m.flash_of(5, area), FLASH_PEAK);
        anim::advance(2);
        assert_eq!(m.flash_of(5, area), 0.0);
        anim::set_level(Level::Playful);
        anim::set_clock(false);
    }

    #[test]
    fn tag_swings_from_polished() {
        anim::set_clock(true);
        let mut m = Motion::default();
        assert!(m.tag_tick(Some((100.0, 100.0))).is_none(), "it hangs still at first");
        let mut x = 100.0;
        for _ in 0..20 {
            anim::advance(16);
            x += 16.0;
            assert!(m.tag_tick(Some((x, 100.0))).is_some());
        }
        assert!(m.tag_angle() > 0.2, "moving right, its bottom trails left: a clockwise turn ({})", m.tag_angle());
        let mut n = 0;
        while m.tag_tick(Some((x, 100.0))).is_some() {
            anim::advance(16);
            n += 1;
            assert!(n < 400, "it comes to rest");
        }
        assert!(m.tag_angle().abs() < 0.01);
        assert!(m.tag_tick(None).is_none() && m.tag.is_none(), "dropped");
        anim::set_level(Level::Subtle);
        assert!(m.tag_tick(Some((0.0, 0.0))).is_none() && m.tag.is_none(), "rigid below Polished");
        anim::set_level(Level::Playful);
        anim::set_clock(false);
    }
}
