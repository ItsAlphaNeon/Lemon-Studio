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

use os_model::Clip;
use os_ui::{Canvas, Layer, Rect, Sprite};

use crate::anim::{self, Level, Pendulum, Spring, Tween};
use crate::app::Win;
use crate::plugin_palette::Row;

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
/// When the palette's list rearranges, its rows go to their new places in this time (spec: ui/animations.md §4.3).
const LIST_MOVE: Duration = Duration::from_millis(160);
/// A control's drawn value follows its value with the time constant Smoothing gives (value x 0.25 ms, so
/// 32 ms at the default 128), at most 15 ms while something is dragged, and has arrived within 0.1 % of its travel
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
/// A menu behind its open submenu shows at this opacity (General ▸ Display ▸ Transparent menus, spec: ui/animations.md
/// §4.2), going there and back in this time.
const MENU_BEHIND: f32 = 0.7;
const MENU_BEHIND_TIME: f64 = 0.120;
/// With General ▸ Display ▸ Transparent windows on, a window that isn't the active one shows at this opacity, going
/// there and back in this time (spec: ui/animations.md §4.9).
const WINDOW_SEE: f32 = 0.85;
const WINDOW_FADE: f64 = 0.150;
/// At Playful a menu's captions enter one after another: this much later per row, up to the last, each from this far
/// left, going half way in 80 ms (FL's at Entertain me, §2.2) and showing over 120 ms.
const CAPTION_STAGGER: f64 = 0.003;
const CAPTION_STAGGER_MAX: f64 = 0.090;
const CAPTION_SLIDE: f32 = 5.0;
const CAPTION_TAU: f64 = 0.115;
const CAPTION_SHOW: f64 = 0.120;
/// It is close enough to its place this long after it sets off (a quarter px).
const CAPTION_TIME: f64 = 0.350;
/// Toolbar buttons light in 145 ms and go out in 340 ms (FL's, §2.2).
pub const BUTTON_HOVER: (f64, f64) = (0.145, 0.340);
pub const SLOT_HOVER: (f64, f64) = (HOVER_IN, HOVER_OUT);
/// A filled slot's name shifts in and back in these times, FL's (§2.2).
pub const NAME_SHIFT: (f64, f64) = (0.150, 0.220);
/// Knobs' rings light and go out as time constants, as FL's do (§2.2).
const KNOB_HOVER: (f64, f64) = (0.060, 0.170);
/// A follow this close to its end is there.
const HOVER_ARRIVED: f64 = 0.01;
/// How long a hover that is on is kept unpainted (s), to fade out when the mouse leaves it, however long it rests there.
/// Past that, its control is taken to be gone.
const HOVER_REST: f64 = 3600.0;
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
/// A deleted clip's ripple, FL's at Entertain me (spec: ui/animations.md §2.2, §4.10): the fill starts at 35 % of the
/// ripple's colour and is gone in 210 ms; the outline starts at 65 %, fades with this time constant and grows 4 px; by
/// 700 ms nothing shows.
const RIPPLE_TIME: f64 = 0.700;
const RIPPLE_FILL: f32 = 0.35;
const RIPPLE_FILL_TIME: f64 = 0.210;
const RIPPLE_LINE: f32 = 0.65;
const RIPPLE_LINE_TAU: f64 = 0.190;
const RIPPLE_GROW: f32 = 4.0;
const RIPPLE_GROW_TAU: f64 = 0.145;
/// How far outside its clip a ripple ever draws (px): how far it grows, and half its line and a pixel's rounding.
pub const RIPPLE_REACH: f32 = RIPPLE_GROW + 2.0;
/// Ripples at once, past which further deleted clips simply go (only a bound, for thousands deleted together).
const RIPPLE_MAX: usize = 256;
/// A clip the magnet moved is drawn on its way there with this time constant: the step is felt rather than seen
/// (§4.10). Lemon Studio's own: FL's clip jumps. The one number to change if it should be more or less.
const SNAP_TAU: f64 = 0.024;
/// A clip this close to its place (px) is there.
const SNAP_ARRIVED: f32 = 0.25;
/// A clip's glide is forgotten after this long if no paint asked for it (the clip scrolled away or went).
const SNAP_REST: f64 = 0.5;

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
    /// The palette's list as last painted and its rows on their way.
    list: PaletteList,
    tag: Option<Tag>,
    /// The tag of a cancelled drag, falling away.
    fall: Option<Fall>,
    hovers: Vec<Hover>,
    flash: Option<Flash>,
    /// When loading sweeps started (they all go in step).
    sweeps: Option<Instant>,
    /// Deleted clips rippling away.
    ripples: Vec<Ripple>,
    /// Clips on their way to where the magnet put them, by what they are known by (a whole selection can be).
    clip_glides: HashMap<u64, ClipGlide>,
}

/// A deleted clip rippling away. It is kept as a clip, so that it scrolls and zooms with the view.
struct Ripple {
    clip: Clip,
    selected: bool,
    arrangement: Option<u16>,
    at: Instant,
}

/// One frame of a ripple: how far it has grown (px), and how strong its fill and outline are (0 to 1).
pub struct RippleDraw {
    pub clip: Clip,
    pub selected: bool,
    pub arrangement: Option<u16>,
    pub grow: f32,
    pub fill: f32,
    pub line: f32,
}

/// A clip on its way to where the magnet put it: how far from there its edges and its top were at `at` (px).
struct ClipGlide {
    left: f32,
    right: f32,
    down: f32,
    at: Instant,
}

/// The plugin palette's list as last painted, and how it goes to a new arrangement: what stays slides, what is new
/// fades in, what went fades out (spec: ui/animations.md §4.3).
#[derive(Default)]
struct PaletteList {
    rows: Vec<Row>,
    tops: Vec<f32>,
    scroll: f32,
    /// Rows on their way: how far from their place they set off (px), or None for one fading in.
    moves: HashMap<u64, Option<f32>>,
    /// Rows fading out, and how far they are below the list's top (px).
    gone: Vec<(Row, f32)>,
    at: Option<(Instant, Duration)>,
}

impl PaletteList {
    /// How far the move is, 0 to 1 (eased), now: None when none is under way.
    fn progress(&self, now: Instant) -> Option<f32> {
        self.at.map(|(at, len)| Tween::new(at, len).eased(now) as f32)
    }

    /// How far row `key` is from its place now.
    fn off(&self, key: u64, u: f32) -> f32 {
        match self.moves.get(&key) {
            Some(Some(from)) => from * (1.0 - u),
            _ => 0.0,
        }
    }
}

/// What a row of the palette's list is known by as the list rearranges.
fn row_key(row: &Row) -> u64 {
    match row {
        Row::Head { label, major } => key(("head", label, major)),
        Row::Plugin { effect, at } => key(("plugin", effect, at)),
        Row::Note(text) => key(("note", text)),
    }
}

/// What the see-through of the menu in `slot` is known by among the hover fades.
fn behind_key(slot: Slot) -> u64 {
    key(("menu behind", slot as u32))
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
                let a = (-t / ROW_TAU.as_secs_f64()).exp();
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
    /// What it is, and the window it is in: controls in two windows can be at the same place in each.
    win: Option<Win>,
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
        self.hovers.retain(|h| since(now, h.last) < if h.on { HOVER_REST } else { 1.0 });
        if self.flash.as_ref().is_some_and(|f| since(now, f.at) > FLASH.as_secs_f64()) {
            self.flash = None;
        }
        // Of a window that was closed meanwhile.
        self.takes.retain(|t| since(now, t.noted) < 1.0);
        self.ghosts.retain(|g| since(now, g.at) < 1.0);
        self.ripples.retain(|r| since(now, r.at) < RIPPLE_TIME);
        self.clip_glides.retain(|_, g| since(now, g.at) < SNAP_REST);
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

    /// Whether something fading or falling is drawn over window `win`'s frame. It is no part of what the frame's pixels
    /// show, so they can't simply be moved.
    pub fn over_frame(&self, win: Option<Win>) -> bool {
        self.takes.iter().any(|t| t.win == win)
            || self.ghosts.iter().any(|g| g.win == win)
            || self.fall.as_ref().is_some_and(|f| f.win == win)
            || (win.is_none() && !self.glows.is_empty())
            || !self.ripples.is_empty()
            || !self.clip_glides.is_empty()
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
                let shrink = if anim::level() >= Level::Polished { PALETTE_CLOSE_ZOOM } else { 1.0 };
                let fade = if i == Slot::Palette as usize { Fade::Close(PALETTE_CLOSE, shrink) } else { Fade::Close(MENU_CLOSE, 1.0) };
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
        // A quarter of a ms for each unit of Smoothing: FL's, measured from 0 to 100 % of the slider (§2.2).
        let mut tau = f64::from(crate::smooth::smoothing()) * 0.000_25;
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

    /// The palette's list is about to be painted as `rows` at `tops`, scrolled by `scroll`, `view` px of it showing.
    /// A new arrangement sets the rows off from where they are drawn; a scroll doesn't, nor one while it opens.
    pub fn palette_list(&mut self, rows: &[Row], tops: &[f32], scroll: f32, view: f32) {
        let now = anim::now();
        let opening = self.menus[Slot::Palette as usize].as_ref().is_none_or(|o| since(now, o.at) < anim::level().dur(PALETTE_OPEN).as_secs_f64());
        let list = &mut self.list;
        if list.at.is_some_and(|(at, len)| now >= at + len) {
            list.at = None;
            list.moves.clear();
            list.gone.clear();
        }
        if list.rows == rows {
            list.scroll = scroll;
            return;
        }
        if anim::instant() || opening {
            list.at = None;
            list.moves.clear();
            list.gone.clear();
        } else {
            let u = list.progress(now).unwrap_or(0.0);
            let was: HashMap<u64, f32> = list.rows.iter().zip(&list.tops).map(|(r, top)| {
                let k = row_key(r);
                (k, top - list.scroll + list.off(k, u))
            }).collect();
            let mut moves = HashMap::new();
            for (r, top) in rows.iter().zip(tops) {
                let k = row_key(r);
                match was.get(&k) {
                    Some(y) if (y - (top - scroll)).abs() >= 0.5 => {
                        moves.insert(k, Some(y - (top - scroll)));
                    }
                    Some(_) => {}
                    None => {
                        moves.insert(k, None);
                    }
                }
            }
            let stays: std::collections::HashSet<u64> = rows.iter().map(row_key).collect();
            list.gone = list.rows.iter().filter(|r| !stays.contains(&row_key(r))).filter_map(|r| {
                let y = was.get(&row_key(r)).copied()?;
                (y + r.height() > 0.0 && y < view).then(|| (r.clone(), y))
            }).collect();
            list.moves = moves;
            list.at = Some((now, anim::level().dur(LIST_MOVE)));
        }
        list.rows = rows.to_vec();
        list.tops = tops.to_vec();
        list.scroll = scroll;
    }

    /// How far above or below its place `row` of the palette's list is drawn now, and its opacity.
    pub fn list_row(&self, row: &Row) -> (f32, f32) {
        let Some(u) = self.list.progress(anim::now()) else { return (0.0, 1.0) };
        match self.list.moves.get(&row_key(row)) {
            Some(Some(from)) => (from * (1.0 - u), 1.0),
            Some(None) => (0.0, u),
            None => (0.0, 1.0),
        }
    }

    /// The rows fading out of the palette's list: each, how far below the list's top it is, and its opacity.
    pub fn list_gone(&self) -> Vec<(Row, f32, f32)> {
        let Some(u) = self.list.progress(anim::now()) else { return Vec::new() };
        self.list.gone.iter().map(|(r, y)| (r.clone(), *y, 1.0 - u)).collect()
    }

    /// How opaque window `win` shows this frame, going see-through while it is `inactive` and back when it isn't
    /// (spec: ui/animations.md §4.9). `area`, in surface `on` (None: the main window), is painted again while it moves.
    pub fn window_opacity(&mut self, win: Win, on: Option<Win>, inactive: bool, area: Rect) -> f32 {
        let surface = std::mem::replace(&mut self.win, on);
        let fade = self.hover_timed(key(("window", win)), inactive, area, (WINDOW_FADE, WINDOW_FADE));
        self.win = surface;
        1.0 - fade * (1.0 - WINDOW_SEE)
    }

    /// Whether the palette's list is on its way to a new arrangement.
    pub fn list_moving(&self) -> bool {
        self.list.at.is_some()
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
                // A new menu starts opaque, whatever the last one in its place had open.
                let behind = behind_key(slot);
                self.hovers.retain(|h| h.key != behind);
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

    /// While menu `slot` has a submenu open (`open`) it goes see-through behind it, and back when that closes, with
    /// General ▸ Display ▸ Transparent menus on, from Subtle up (spec: ui/animations.md §4.2). Call it right after
    /// `menu_begin`, before the menu paints.
    pub fn menu_behind(&mut self, c: &mut Canvas, d: &mut MenuDraw, slot: Slot, open: bool, r: Rect, k: f32) {
        if anim::instant() || !crate::prefs::check(&crate::prefs::TRANSPARENT_MENUS) {
            return;
        }
        let reach = r.outset(1.0);
        let area = Rect::new(reach.x * k, reach.y * k, reach.w * k, reach.h * k);
        let fade = self.hover_timed(behind_key(slot), open, area, (MENU_BEHIND_TIME, MENU_BEHIND_TIME));
        if fade > 0.0 {
            d.alpha *= 1.0 - fade * (1.0 - MENU_BEHIND);
            if d.layer.is_none() {
                d.layer = Some(c.begin_layer(reach, true));
            }
        }
    }

    /// Where caption `row` of menu `slot` is this frame, entering at Playful (spec: ui/animations.md §4.2): how far
    /// left of its place, and its opacity. Rows from the first on come after one another, up to a limit.
    pub fn caption(&self, slot: Slot, row: usize) -> (f32, f32) {
        if anim::instant() || anim::level() != Level::Playful {
            return (0.0, 1.0);
        }
        let Some(o) = &self.menus[slot as usize] else { return (0.0, 1.0) };
        let t = since(anim::now(), o.at) - (row as f64 * CAPTION_STAGGER).min(CAPTION_STAGGER_MAX);
        if t >= CAPTION_TIME {
            return (0.0, 1.0);
        }
        let t = t.max(0.0);
        (CAPTION_SLIDE * (-t / CAPTION_TAU).exp() as f32, (t / CAPTION_SHOW).min(1.0) as f32)
    }

    /// Paints menu `slot`, in canvas px `r` with `k` window px each, again while its captions enter.
    pub fn captions_live(&mut self, slot: Slot, r: Rect, k: f32) {
        if anim::instant() || anim::level() != Level::Playful {
            return;
        }
        let Some(o) = &self.menus[slot as usize] else { return };
        if since(anim::now(), o.at) < CAPTION_TIME + CAPTION_STAGGER_MAX {
            self.live(Rect::new(r.x * k, r.y * k, r.w * k, r.h * k));
        }
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
        anim::ease_out(f64::from(self.hover_linear(key, on, area, times))) as f32
    }

    /// As `hover_timed`, going at an even pace: for what is shaped by its own curve.
    pub fn hover_linear(&mut self, key: u64, on: bool, area: Rect, times: (f64, f64)) -> f32 {
        self.hover_step(key, on, area, |v, dt| if on { (v + dt / times.0).min(1.0) } else { (v - dt / times.1).max(0.0) }) as f32
    }

    /// A knob's ring, as `hover`: it goes most of the way at once and takes its time over the rest.
    pub fn hover_knob(&mut self, key: u64, on: bool, area: Rect) -> f32 {
        self.hover_step(key, on, area, |v, dt| {
            let (to, tau) = if on { (1.0, KNOB_HOVER.0) } else { (0.0, KNOB_HOVER.1) };
            let next = anim::follow(v, to, dt, tau);
            if (next - to).abs() < HOVER_ARRIVED { to } else { next }
        }) as f32
    }

    /// The fade `key` moves on by `step` (its value and the time since the last paint give the new value).
    fn hover_step(&mut self, key: u64, on: bool, area: Rect, step: impl Fn(f64, f64) -> f64) -> f64 {
        let now = anim::now();
        let win = self.win;
        let pos = self.hovers.iter().position(|h| h.key == key && h.win == win);
        if anim::instant() {
            if let Some(i) = pos {
                self.hovers.swap_remove(i);
            }
            return if on { 1.0 } else { 0.0 };
        }
        let i = match pos {
            Some(i) => i,
            None if on => {
                self.hovers.push(Hover { win, key, val: 0.0, on, last: now });
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
        h.val = step(h.val, dt);
        let v = h.val;
        if (on && v < 1.0) || (!on && v > 0.0) {
            self.live(area);
        }
        if !on && v <= 0.0 {
            self.hovers.swap_remove(i);
        }
        v
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

    // ------------------------------------------------------------------------------------------------ clips

    /// `clip` was deleted from `arrangement`: it ripples away, from Polished up (spec: ui/animations.md §4.10).
    /// `selected`: it showed as selected, which colours the ripple.
    pub fn ripple(&mut self, clip: Clip, selected: bool, arrangement: Option<u16>) {
        if glides() && self.ripples.len() < RIPPLE_MAX {
            self.ripples.push(Ripple { clip, selected, arrangement, at: anim::now() });
        }
    }

    /// Clips came back or changed (an undo or a redo): what was rippling away has no place over them.
    pub fn ripples_over(&mut self) {
        self.ripples.clear();
    }

    /// The ripples to draw this frame.
    pub fn ripples(&mut self) -> Vec<RippleDraw> {
        let now = anim::now();
        self.ripples.retain(|r| since(now, r.at) < RIPPLE_TIME);
        self.ripples
            .iter()
            .map(|r| {
                let t = since(now, r.at);
                RippleDraw {
                    clip: r.clip,
                    selected: r.selected,
                    arrangement: r.arrangement,
                    grow: RIPPLE_GROW * (1.0 - (-t / RIPPLE_GROW_TAU).exp()) as f32,
                    fill: RIPPLE_FILL * (1.0 - t / RIPPLE_FILL_TIME).max(0.0) as f32,
                    line: RIPPLE_LINE * (-t / RIPPLE_LINE_TAU).exp() as f32,
                }
            })
            .collect()
    }

    /// The magnet moved a dragged clip: its left and right edges and its top were `left`, `right` and `down` px from
    /// where they are now. It is drawn on its way there; the clip itself is already there, and sounds there (§4.10).
    /// Not at Off or at Smoothing 0. A jump during a glide sets off from where the clip is drawn. Returns how far the
    /// drawing now has to go, which is what to paint again; None if it just jumps.
    pub fn clip_jumped(&mut self, key: u64, left: f32, right: f32, down: f32) -> Option<(f32, f32, f32)> {
        if anim::instant() || crate::smooth::smoothing() <= 0.0 {
            return None;
        }
        let now = anim::now();
        // Counted from a frame ago: the jump already shows on the frame it happens.
        let at = now.checked_sub(Duration::from_secs_f64(FIRST_STEP)).unwrap_or(now);
        let Some(g) = self.clip_glides.get_mut(&key) else {
            self.clip_glides.insert(key, ClipGlide { left, right, down, at });
            return Some((left, right, down));
        };
        let k = (-since(now, g.at) / SNAP_TAU).exp() as f32;
        (g.left, g.right, g.down, g.at) = (g.left * k + left, g.right * k + right, g.down * k + down, at);
        Some((g.left, g.right, g.down))
    }

    /// Whether any clip is on its way, which spares asking for each.
    pub fn clips_gliding(&self) -> bool {
        !self.clip_glides.is_empty()
    }

    /// How far from its place the clip `key` is drawn this frame: its left edge, right edge and top (px). None once
    /// it is there.
    pub fn clip_glide(&mut self, key: u64) -> Option<(f32, f32, f32)> {
        let g = self.clip_glides.get(&key)?;
        let k = (-since(anim::now(), g.at) / SNAP_TAU).exp() as f32;
        let at = (g.left * k, g.right * k, g.down * k);
        if at.0.abs().max(at.1.abs()).max(at.2.abs()) < SNAP_ARRIVED {
            self.clip_glides.remove(&key);
            return None;
        }
        Some(at)
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
