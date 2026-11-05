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

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use os_ui::{Canvas, Layer, Rect, Sprite};

use crate::anim::{self, Level, Pendulum, Spring, Tween};
use crate::app::Win;

/// What can be open at once and appears and goes the same way: a popup menu, its submenu, a menu bar dropdown and the
/// plugin palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Popup = 0,
    Sub = 1,
    Dropdown = 2,
    Palette = 3,
}

/// A popup menu opens in 120 ms, a submenu in 100 ms (spec: ui/animations.md §4.2).
const MENU_OPEN: Duration = Duration::from_millis(120);
const SUB_OPEN: Duration = Duration::from_millis(100);
/// How far a menu slides on opening (toward where it was opened from), a submenu sideways, and how small it starts.
const MENU_SLIDE: f32 = 6.0;
const SUB_SLIDE: f32 = 8.0;
const MENU_ZOOM: f32 = 0.97;
/// A menu sent away (Esc, a click elsewhere) fades in 70 ms; one chosen from goes at once and leaves the chosen row
/// fading with FL's time constant (§4.2). Under 2 % it is gone.
const MENU_CLOSE: Duration = Duration::from_millis(70);
const ROW_TAU: Duration = Duration::from_millis(145);
const ROW_GONE: f64 = 0.02;
/// The plugin palette opens in 180 ms from 96 % and closes in 120 ms to 97 % (§4.3). At Playful its rows also fade in
/// one after another, 12 ms apart for the first ten, over 120 ms each.
const PALETTE_OPEN: Duration = Duration::from_millis(180);
const PALETTE_ZOOM: f32 = 0.96;
const PALETTE_CLOSE: Duration = Duration::from_millis(120);
const PALETTE_CLOSE_ZOOM: f32 = 0.97;
const ROW_STAGGER: f64 = 0.012;
const ROW_STAGGERED: usize = 10;
const ROW_FADE: f64 = 0.120;
/// A control's drawn value follows its value with the time constant Smoothing gives (half-life value x 0.25 ms, so
/// 46 ms at the default 128), at most 15 ms while something is dragged, and has arrived within 0.1 % of its travel
/// (§4.5). It sets off with one frame's move, so a jump shows on the frame it happens.
const DRAG_TAU: f64 = 0.015;
const FIRST_STEP: f64 = 0.016;
const ARRIVED: f32 = 0.001;
/// Controls' drawn values are forgotten past this many (they are few; this is only a bound).
const MAX_DRAWN: usize = 4096;
/// The highlight bar's spring: critically damped, settled in about 60 ms.
const GLIDE_HZ: f64 = 15.5;
/// Hover helpers fade in over 120 ms and out over 200 ms (§4.6).
const HOVER_IN: f64 = 0.120;
const HOVER_OUT: f64 = 0.200;
/// Toolbar buttons light in 145 ms and go out in 340 ms (FL's, §2.2).
pub const BUTTON_HOVER: (f64, f64) = (0.145, 0.340);
pub const SLOT_HOVER: (f64, f64) = (HOVER_IN, HOVER_OUT);
/// A functional flash lasts 400 ms and peaks at 60 % (§4.7).
pub const FLASH: Duration = Duration::from_millis(400);
const FLASH_PEAK: f32 = 0.6;
/// A loading sweep crosses a clip every 1.2 s (§4.8).
pub const SWEEP: Duration = Duration::from_millis(1200);
/// The drag tag: where it hangs from (below and right of the mouse), its height, and how wide it may grow (§4.1).
pub const TAG_PIN: (f32, f32) = (12.0, 14.0);
pub const TAG_H: f32 = 24.0;
pub const TAG_MAX_W: f32 = 220.0;
/// A cancelled drag's tag falls 48 px and fades out over 400 ms (FL's: about 47 px and 400 ms).
const FALL: Duration = Duration::from_millis(400);
const FALL_DROP: f32 = 48.0;

#[derive(Default)]
pub struct Motion {
    /// The window being painted: None for the main one, or a detached one.
    win: Option<Win>,
    /// Where motions that still move were drawn by the last paint of each window (logical px of that window), and
    /// when.
    live: Vec<Live>,
    menus: [Option<Opened>; 4],
    glides: [Option<Glide>; 4],
    /// The menu row just chosen: its menu and where it is (window px). It leaves a ghost if that closes the menu.
    chosen: Option<(Slot, Rect)>,
    /// What closed and has its picture taken from the last frame when its window next paints (`before_paint`).
    takes: Vec<Take>,
    /// Pictures of what closed, fading over the frame.
    ghosts: Vec<Ghost>,
    /// What controls last drew, by what they are. Kept while they rest, so that a jump after any time still glides.
    drawn: HashMap<u64, Drawn>,
    /// Something is being dragged with the mouse: controls follow closely.
    dragging: bool,
    /// Where a drag would drop, and where it just would have, glowing.
    glows: Vec<Glow>,
    tag: Option<Tag>,
    /// The tag of a cancelled drag, falling away.
    fall: Option<Fall>,
    hovers: Vec<Hover>,
    flash: Option<Flash>,
    /// When loading sweeps started (they all go in step).
    sweeps: Option<Instant>,
}

/// A drop target's glow, in window `win`: its opacity, 0 to 1, and which way it goes.
struct Glow {
    win: Win,
    r: Rect,
    val: f64,
    on: bool,
    last: Instant,
}

/// A control's picture: the value it shows, the value it goes to, and when it was last stepped.
struct Drawn {
    shown: f32,
    to: f32,
    last: Instant,
}

/// An open menu: what it is, when it opened, where it was last painted (window px) and in which window.
struct Opened {
    key: u64,
    at: Instant,
    rect: Rect,
    win: Option<Win>,
}

/// How a picture of something closed goes away.
#[derive(Debug, Clone, Copy)]
enum Fade {
    /// A chosen menu row: an exponential fade.
    Row,
    /// A menu or the palette: faded out over this long, shrinking to this much of its size.
    Close(Duration, f32),
}

/// A picture to take from window `win`'s last frame.
struct Take {
    r: Rect,
    win: Option<Win>,
    fade: Fade,
    noted: Instant,
}

/// A picture of something closed, over the frame of window `win`, since `at`.
struct Ghost {
    sprite: Sprite,
    r: Rect,
    win: Option<Win>,
    fade: Fade,
    at: Instant,
}

impl Ghost {
    /// How it shows at `now`: its opacity and size. None: it is gone.
    fn show(&self, now: Instant) -> Option<(f32, f32)> {
        if anim::instant() {
            return None;
        }
        let t = since(now, self.at);
        let level = anim::level();
        match self.fade {
            Fade::Row => {
                let a = (-t / level.dur(ROW_TAU).as_secs_f64()).exp();
                (a > ROW_GONE).then_some((a as f32, 1.0))
            }
            Fade::Close(dur, zoom) => {
                let u = t / level.dur(dur).as_secs_f64();
                let e = anim::ease_out(u) as f32;
                (u < 1.0).then_some((1.0 - e, 1.0 + (zoom - 1.0) * e))
            }
        }
    }
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

/// A cancelled drag's tag: its name, the mouse it hung from, its angle then, its window (None: the main one) and
/// when it was let go.
struct Fall {
    label: String,
    x: f32,
    y: f32,
    angle: f32,
    win: Option<Win>,
    at: Instant,
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
        // One still on is kept while it rests unpainted, to fade out when the mouse leaves it.
        self.hovers.retain(|h| since(now, h.last) < if h.on { 60.0 } else { 1.0 });
        if self.flash.as_ref().is_some_and(|f| since(now, f.at) > FLASH.as_secs_f64()) {
            self.flash = None;
        }
        // Of a window that was closed meanwhile.
        self.takes.retain(|t| since(now, t.noted) < 1.0);
        self.ghosts.retain(|g| since(now, g.at) < 1.0);
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

    /// Row `r` (window px) of the menu in `slot` was chosen. If that closes the menu, the row stays a moment as a
    /// fading picture (FL's ghost, spec: ui/animations.md §4.2).
    pub fn chose(&mut self, slot: Slot, r: Rect) {
        self.chosen = Some((slot, r));
    }

    /// Which of the `Slot`s are closed now. What was open and painted closes: it opens afresh next time, and unless it
    /// was closed by choosing from it, its picture fades away. Choosing closes a menu and any others open at once, and
    /// only the chosen row fades. At Off everything just goes.
    pub fn menus_closed(&mut self, closed: [bool; 4]) {
        let now = anim::now();
        let chosen = self.chosen.take();
        let was_open = |s: &Self, c: Slot| s.menus[c as usize].as_ref().map(|o| o.win);
        let picked = chosen.filter(|c| closed[c.0 as usize]).and_then(|c| was_open(self, c.0).map(|win| (c.1, win)));
        let quiet = picked.is_some() || anim::instant();
        for (i, shut) in closed.into_iter().enumerate() {
            if !shut {
                continue;
            }
            self.glides[i] = None;
            let Some(o) = self.menus[i].take() else { continue };
            if !quiet {
                let fade = if i == Slot::Palette as usize { Fade::Close(PALETTE_CLOSE, PALETTE_CLOSE_ZOOM) } else { Fade::Close(MENU_CLOSE, 1.0) };
                self.takes.push(Take { r: o.rect, win: o.win, fade, noted: now });
            }
        }
        if let Some((r, win)) = picked
            && !anim::instant()
        {
            self.takes.push(Take { r, win, fade: Fade::Row, noted: now });
        }
    }

    /// What a control that is `key` and whose value is `v` (0 to 1) draws this frame (spec: ui/animations.md §4.5): `v`
    /// when it first shows or rests; after a jump it follows `v` by the Smoothing slider, closer while something is
    /// being dragged. `area` (window px) is painted again until it has arrived. At Off or Smoothing 0, always `v`.
    pub fn value(&mut self, key: u64, v: f32, area: Rect) -> f32 {
        let mut tau = f64::from(crate::smooth::smoothing()) * 0.000_25 / std::f64::consts::LN_2;
        if self.dragging {
            tau = tau.min(DRAG_TAU);
        }
        if anim::instant() || tau <= 0.0 {
            self.drawn.clear();
            return v;
        }
        if self.drawn.len() >= MAX_DRAWN {
            self.drawn.clear();
        }
        let now = anim::now();
        let d = self.drawn.entry(key).or_insert(Drawn { shown: v, to: v, last: now });
        if d.shown == v {
            *d = Drawn { shown: v, to: v, last: now };
            return v;
        }
        let dt = if d.shown == d.to { FIRST_STEP } else { since(now, d.last) };
        (d.to, d.last) = (v, now);
        let x = anim::follow(f64::from(d.shown), f64::from(v), dt, tau) as f32;
        d.shown = if (x - v).abs() < ARRIVED { v } else { x };
        let shown = d.shown;
        if shown != v {
            self.live(area);
        }
        shown
    }

    /// Window `win` is about to paint and its canvas still shows the last frame: the pictures noted for it are taken
    /// now. Their fades count from here, so a chosen item that takes long to run doesn't use them up. `dragging`: the
    /// mouse is dragging something.
    pub fn before_paint(&mut self, c: &mut Canvas, win: Option<Win>, dragging: bool) {
        self.dragging = dragging;
        if self.takes.iter().all(|t| t.win != win) {
            return;
        }
        let now = anim::now();
        let (here, rest): (Vec<Take>, Vec<Take>) = std::mem::take(&mut self.takes).into_iter().partition(|t| t.win == win);
        self.takes = rest;
        for t in here {
            let l = c.begin_layer(t.r, false);
            if let Some(sprite) = c.take_layer(l) {
                self.ghosts.push(Ghost { sprite, r: t.r, win, fade: t.fade, at: now });
            }
        }
    }

    /// Draws what is fading over the frame of the window being painted, last of all.
    pub fn paint_ghosts(&mut self, c: &mut Canvas) {
        let now = anim::now();
        let win = self.win;
        let live = &mut self.live;
        self.ghosts.retain(|g| {
            if g.win != win {
                return true;
            }
            let Some((alpha, zoom)) = g.show(now) else { return false };
            c.sprite(&g.sprite, g.r.cx(), g.r.cy(), 0.0, zoom, 0.0, 0.0, alpha);
            live.push(Live { win, r: g.r.outset(1.0), at: now });
            true
        });
    }

    /// How far row `i` of those showing in the plugin palette has faded in, 0 to 1: at Playful they come one after
    /// another as the palette opens (spec: ui/animations.md §4.3).
    pub fn palette_row(&self, i: usize) -> f32 {
        if anim::instant() || anim::level() != Level::Playful {
            return 1.0;
        }
        let Some(o) = &self.menus[Slot::Palette as usize] else { return 1.0 };
        let t = since(anim::now(), o.at) - i.min(ROW_STAGGERED) as f64 * ROW_STAGGER;
        anim::ease_out(t / ROW_FADE) as f32
    }

    /// Menu `slot`, which is `key` (its entries and where it opened), is about to be painted in `r` (the canvas's
    /// logical px, `k` times the window's), opened from `anchor`. While it appears, it is painted into a layer that
    /// `menu_end` draws faded, slid and scaled (spec: ui/animations.md §4.2): Subtle fades it in, Polished slides it
    /// from the anchor too, Playful also grows it from 97 %. A submenu slides out sideways from its row. The plugin
    /// palette (§4.3) fades in, and from Polished up grows from 96 %. At Off it is there at once.
    pub fn menu_begin(&mut self, c: &mut Canvas, slot: Slot, key: u64, r: Rect, anchor: (f32, f32), k: f32) -> MenuDraw {
        let now = anim::now();
        let i = slot as usize;
        let rect = {
            let o = r.outset(1.0);
            Rect::new(o.x * k, o.y * k, o.w * k, o.h * k)
        };
        let win = self.win;
        let at = match &mut self.menus[i] {
            Some(o) if o.key == key => {
                o.rect = rect;
                o.win = win;
                o.at
            }
            m => {
                *m = Some(Opened { key, at: now, rect, win });
                now
            }
        };
        let level = anim::level();
        let full = match slot {
            Slot::Sub => SUB_OPEN,
            Slot::Palette => PALETTE_OPEN,
            Slot::Popup | Slot::Dropdown => MENU_OPEN,
        };
        let tw = Tween::new(at, level.dur(full));
        let pivot = (anchor.0.clamp(r.x, r.right()), anchor.1.clamp(r.y, r.bottom()));
        let mut d = MenuDraw { layer: None, alpha: 1.0, zoom: 1.0, dx: 0.0, dy: 0.0, pivot };
        if anim::instant() || tw.done(now) {
            return d;
        }
        let u = tw.eased(now) as f32;
        d.alpha = u;
        match slot {
            // Out of its row: from the left when it opens to the right of it.
            Slot::Sub if level >= Level::Polished => {
                let side = if r.cx() >= anchor.0 { 1.0 } else { -1.0 };
                d.dx = -SUB_SLIDE * (1.0 - u) * side;
            }
            Slot::Palette if level >= Level::Polished => d.zoom = PALETTE_ZOOM + (1.0 - PALETTE_ZOOM) * u,
            Slot::Popup | Slot::Dropdown => {
                // Away from where it was opened: a menu below its anchor starts higher up.
                if level >= Level::Polished {
                    let side = if r.cy() >= anchor.1 { 1.0 } else { -1.0 };
                    d.dy = -MENU_SLIDE * (1.0 - u) * side;
                }
                if level == Level::Playful {
                    d.zoom = MENU_ZOOM + (1.0 - MENU_ZOOM) * u;
                }
            }
            _ => {}
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

    /// A browser drag was cancelled with the tag `label` hanging from the mouse at (`x`, `y`) of window `win` (None:
    /// the main one): from Subtle up it falls away, fading out, at the angle it had (spec: ui/animations.md §4.1). At
    /// Off it is gone at once.
    pub fn tag_fall(&mut self, label: String, x: f32, y: f32, win: Option<Win>) {
        let angle = self.tag_angle();
        self.tag = None;
        self.fall = (!anim::instant()).then(|| Fall { label, x, y, angle, win, at: anim::now() });
    }

    /// The falling tag, if one falls in window `win` this frame: its name, where it hangs from now, its angle and
    /// opacity. Where it falls is painted again until it is gone.
    pub fn falling(&mut self, win: Option<Win>) -> Option<(String, f32, f32, f32, f32)> {
        let f = self.fall.as_ref().filter(|f| f.win == win)?;
        let u = since(anim::now(), f.at) / anim::level().dur(FALL).as_secs_f64();
        let area = tag_reach(f.x, f.y);
        let area = Rect::new(area.x, area.y, area.w, area.h + FALL_DROP);
        if u >= 1.0 {
            // One more frame clears it.
            self.fall = None;
            return None;
        }
        // It sets off slowly and eases in, as a critically damped spring let go; it fades faster than it falls.
        let k = 5.6 * u;
        let drop = FALL_DROP as f64 * (1.0 - (1.0 + k) * (-k).exp());
        let alpha = (1.0 - u) * (1.0 - u);
        let out = (f.label.clone(), f.x, f.y + drop as f32, f.angle, alpha as f32);
        self.live(area);
        Some(out)
    }

    // ------------------------------------------------------------------------------------------------ hover fades

    /// How much of hover helper `key` (a clip's handles, a slot's arrow) shows this frame, 0 to 1, as the mouse is
    /// over it (`on`) or not: 120 ms in, 200 ms out from Subtle up, at once at Off (spec: ui/animations.md §4.6).
    /// `area` (window px) is painted again while it fades.
    pub fn hover(&mut self, key: u64, on: bool, area: Rect) -> f32 {
        self.hover_timed(key, on, area, (HOVER_IN, HOVER_OUT))
    }

    /// As `hover`, fading in over `times.0` s and out over `times.1` s.
    pub fn hover_timed(&mut self, key: u64, on: bool, area: Rect, times: (f64, f64)) -> f32 {
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
        h.val = if on { (h.val + dt / times.0).min(1.0) } else { (h.val - dt / times.1).max(0.0) };
        let v = h.val;
        if (on && v < 1.0) || (!on && v > 0.0) {
            self.live(area);
        }
        if !on && v <= 0.0 {
            self.hovers.swap_remove(i);
        }
        anim::ease_out(v) as f32
    }

    /// The glows to draw over window `win`: the drop target `target` (a window and a part of it) fades in as a hover
    /// helper does, the one it was fades out (spec: ui/animations.md §4.6). At Off the target just shows.
    pub fn drop_glows(&mut self, target: Option<(Win, Rect)>, win: Win) -> Vec<(Rect, f32)> {
        let here = target.filter(|t| t.0 == win).map(|t| t.1);
        if anim::instant() {
            self.glows.clear();
            return here.map(|r| (r, 1.0)).into_iter().collect();
        }
        if here.is_none() && self.glows.is_empty() {
            return Vec::new();
        }
        let now = anim::now();
        if let Some(r) = here
            && !self.glows.iter().any(|g| g.win == win && g.r == r)
        {
            self.glows.push(Glow { win, r, val: 0.0, on: true, last: now });
        }
        let mut out = Vec::new();
        let live = &mut self.live;
        let surface = self.win;
        self.glows.retain_mut(|g| {
            if g.win != win {
                return true;
            }
            let on = here == Some(g.r);
            if g.on != on {
                (g.on, g.last) = (on, now);
            }
            let dt = since(now, g.last);
            g.last = now;
            g.val = if on { (g.val + dt / HOVER_IN).min(1.0) } else { (g.val - dt / HOVER_OUT).max(0.0) };
            if (on && g.val < 1.0) || (!on && g.val > 0.0) {
                live.push(Live { win: surface, r: g.r.outset(1.0), at: now });
            }
            if !on && g.val <= 0.0 {
                return false;
            }
            out.push((g.r, anim::ease_out(g.val) as f32));
            true
        });
        out
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

    // ------------------------------------------------------------------------------------------------ status motion

    /// Where a loading sweep is in its cycle (`SWEEP`), 0 to 1, or None where it is hidden: decoration goes at
    /// Animations Off (spec: ui/animations.md §4.8). `area` (window px) is painted again while it shows.
    pub fn sweep(&mut self, area: Rect) -> Option<f32> {
        if anim::instant() {
            self.sweeps = None;
            return None;
        }
        let now = anim::now();
        let start = *self.sweeps.get_or_insert(now);
        self.live(area);
        Some((since(now, start) / SWEEP.as_secs_f64()).fract() as f32)
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
