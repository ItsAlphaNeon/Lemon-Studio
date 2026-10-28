// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The tiling layout's split views (a Lemon Studio addition): the playlist, the piano roll and the mixer are either
//! maximized (filling the workspace card) or snapped into a split of it, each its own card; none of them floats. A
//! preset says how the card is split into three places; the windows shown take their places and a place left empty
//! goes to its neighbour, so two windows share the card and one alone fills it. The channel rack, plugin and tool
//! windows keep floating over them.
//!
//! A title bar dragged onto another snapped window swaps the two; the gaps between them drag to resize the split. The
//! preset, the order and the splits are kept for the next start (`settings.txt`).

use crate::app::{State, Win};
use crate::views::GUTTER;
use os_ui::Rect;

/// A way to split the card into three places.
pub struct Preset {
    pub name: &'static str,
    /// The outer split stacks its two parts (top and bottom); else they sit side by side.
    pub outer_rows: bool,
    /// The place alone on its side of the outer split is the first (top or left: place 0, places 1 and 2 share the
    /// other side); else the last (place 2, places 0 and 1 share the first side).
    pub single_first: bool,
}

pub const PRESETS: [Preset; 2] = [
    // place 0 on top, places 1 and 2 side by side below
    Preset { name: "Top and bottom", outer_rows: true, single_first: true },
    // places 0 and 1 stacked on the left, place 2 on the right
    Preset { name: "Left and right", outer_rows: false, single_first: false },
];

/// The split views' settings: the preset, which window is in which place, and each preset's splits (the first part's
/// share: the outer split's, then the inner one's).
#[derive(Clone, Debug, PartialEq)]
pub struct Tiles {
    pub preset: usize,
    pub order: [Win; 3],
    pub share: [[f32; 2]; 2],
}

impl Default for Tiles {
    fn default() -> Self {
        Tiles { preset: 0, order: [Win::Playlist, Win::PianoRoll, Win::Mixer], share: [[0.5, 0.5], [0.6, 0.5]] }
    }
}

const KEY: &str = "tiles";

/// The shares a split can be dragged to.
const SHARE: (f32, f32) = (0.2, 0.8);

impl Tiles {
    /// The kept settings, or the defaults. Text: preset; the three windows in their places; the four shares.
    pub fn load() -> Tiles {
        crate::settings_file::get(KEY).and_then(|v| Tiles::parse(&v)).unwrap_or_default()
    }

    fn parse(v: &str) -> Option<Tiles> {
        let mut it = v.split_whitespace();
        let preset = it.next()?.parse::<usize>().ok().filter(|&p| p < PRESETS.len())?;
        let mut order = [Win::Playlist; 3];
        for o in &mut order {
            *o = match it.next()? {
                "playlist" => Win::Playlist,
                "piano" => Win::PianoRoll,
                "mixer" => Win::Mixer,
                _ => return None,
            };
        }
        if order.iter().any(|w| order.iter().filter(|o| *o == w).count() > 1) {
            return None;
        }
        let mut share = [[0.5; 2]; 2];
        for s in share.iter_mut().flatten() {
            *s = it.next()?.parse::<f32>().ok()?.clamp(SHARE.0, SHARE.1);
        }
        Some(Tiles { preset, order, share })
    }

    pub fn save(&self) {
        let name = |w: Win| match w {
            Win::PianoRoll => "piano",
            Win::Mixer => "mixer",
            _ => "playlist",
        };
        let shares: Vec<String> = self.share.iter().flatten().map(|s| format!("{s:.3}")).collect();
        let v = format!("{} {} {} {} {}", self.preset, name(self.order[0]), name(self.order[1]), name(self.order[2]), shares.join(" "));
        crate::settings_file::write(KEY, &v);
    }
}

/// A gap between two snapped windows: dragging it moves split `node` (0 the outer, 1 the inner) of `parent`, between
/// the windows `sides` (first part, second part).
#[derive(Clone, Debug, PartialEq)]
pub struct Divider {
    pub node: usize,
    /// The split stacks its parts (the gap is horizontal).
    pub rows: bool,
    pub gap: Rect,
    pub parent: Rect,
    pub sides: [Vec<Win>; 2],
}

/// The least a snapped window keeps across a split (px): its controls stay apart (a mixer's routing row clear of
/// its separation knobs). A split stops there; a card too small for both splits them evenly.
fn least(w: Win, rows: bool) -> f32 {
    match (w, rows) {
        (Win::Mixer, true) => 260.0,
        (Win::Mixer, false) => 480.0,
        (_, true) => 180.0,
        (_, false) => 360.0,
    }
}

/// Where the snapped windows are, and the gaps between them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub tiles: Vec<(Win, Rect)>,
    pub dividers: Vec<Divider>,
}

/// Splits `r` at `share` of it, the gutter between the two parts.
fn split(r: Rect, rows: bool, share: f32) -> (Rect, Rect, Rect) {
    if rows {
        let h = ((r.h - GUTTER) * share).round();
        (Rect::new(r.x, r.y, r.w, h), Rect::new(r.x, r.y + h, r.w, GUTTER), Rect::new(r.x, r.y + h + GUTTER, r.w, r.h - h - GUTTER))
    } else {
        let w = ((r.w - GUTTER) * share).round();
        (Rect::new(r.x, r.y, w, r.h), Rect::new(r.x + w, r.y, GUTTER, r.h), Rect::new(r.x + w + GUTTER, r.y, r.w - w - GUTTER, r.h))
    }
}

/// The plan for the windows snapped now (`State::snapped`); empty outside the tiling layout.
pub fn plan(s: &State) -> Plan {
    plan_of(s, |w| s.snapped(w))
}

/// The plan as it would be with the maximized window back in the split: where each open window sits in it.
pub fn plan_all(s: &State) -> Plan {
    plan_of(s, |w| s.tiled() && s.shown(w))
}

fn plan_of(s: &State, in_split: impl Fn(Win) -> bool) -> Plan {
    let mut out = Plan::default();
    if !s.tiled() {
        return out;
    }
    let t = &s.tiles;
    let p = &PRESETS[t.preset];
    let share = t.share[t.preset];
    let at = |place: usize| Some(t.order[place]).filter(|&w| in_split(w));
    let (single, pair) = if p.single_first { (at(0), [at(1), at(2)]) } else { (at(2), [at(0), at(1)]) };
    let pair: Vec<Win> = pair.into_iter().flatten().collect();
    let work = s.work_rect();
    // the pair's side: one window, or the inner split (across the outer one)
    let place_pair = |out: &mut Plan, r: Rect| match pair[..] {
        [w] => out.tiles.push((w, r)),
        [a, b] => {
            let (ra, gap, rb) = split(r, !p.outer_rows, share[1]);
            out.tiles.push((a, ra));
            out.tiles.push((b, rb));
            out.dividers.push(Divider { node: 1, rows: !p.outer_rows, gap, parent: r, sides: [vec![a], vec![b]] });
        }
        _ => {}
    };
    match single {
        None => place_pair(&mut out, work),
        Some(w) if pair.is_empty() => out.tiles.push((w, work)),
        Some(w) => {
            let (first, gap, second) = split(work, p.outer_rows, share[0]);
            let (rs, rp) = if p.single_first { (first, second) } else { (second, first) };
            out.tiles.push((w, rs));
            place_pair(&mut out, rp);
            let sides = if p.single_first { [vec![w], pair.clone()] } else { [pair.clone(), vec![w]] };
            out.dividers.push(Divider { node: 0, rows: p.outer_rows, gap, parent: work, sides });
        }
    }
    out
}

/// The snapped window `w`'s rect, if it is snapped.
pub fn rect_of(s: &State, w: Win) -> Option<Rect> {
    plan(s).tiles.into_iter().find(|&(o, _)| o == w).map(|(_, r)| r)
}

/// Drags split `node` to the mouse at `(x, y)`.
pub fn drag(s: &mut State, node: usize, (x, y): (f32, f32)) {
    let Some(d) = plan(s).dividers.into_iter().find(|d| d.node == node) else { return };
    let r = d.parent;
    let len = if d.rows { r.h } else { r.w } - GUTTER;
    let share = if d.rows { (y - GUTTER / 2.0 - r.y) / len } else { (x - GUTTER / 2.0 - r.x) / len };
    let side = |i: usize| d.sides[i].iter().map(|&w| least(w, d.rows)).fold(0.0, f32::max) / len.max(1.0);
    let (lo, hi) = (side(0).max(SHARE.0), (1.0 - side(1)).min(SHARE.1));
    let p = s.tiles.preset;
    s.tiles.share[p][node] = if lo <= hi { share.clamp(lo, hi) } else { 0.5 };
}

/// Split `node` back to the preset's default (a double click on its gap).
pub fn reset(s: &mut State, node: usize) {
    let p = s.tiles.preset;
    s.tiles.share[p][node] = Tiles::default().share[p][node];
    s.tiles.save();
}

/// The snapped window `w` dropped at `(x, y)`: onto another snapped window, the two change places. Returns whether
/// they did.
pub fn drop(s: &mut State, w: Win, (x, y): (f32, f32)) -> bool {
    let Some((other, _)) = plan(s).tiles.into_iter().find(|&(o, r)| o != w && r.contains(x, y)) else { return false };
    swap(s, w, other);
    true
}

/// Snapped windows `a` and `b` change places.
pub fn swap(s: &mut State, a: Win, b: Win) {
    let o = &mut s.tiles.order;
    if let (Some(i), Some(j)) = (o.iter().position(|&p| p == a), o.iter().position(|&p| p == b)) {
        o.swap(i, j);
        s.tiles.save();
    }
}

/// A direction on the screen (Ctrl+Alt+arrows).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// The window of `rects` nearest to `from` in direction `d`: its centre past `from`'s that way, the nearest by how
/// far that way plus twice how far across (a window straight across wins over a nearer one off to the side).
pub fn neighbour(rects: &[(Win, Rect)], from: Rect, d: Dir) -> Option<Win> {
    let (cx, cy) = (from.cx(), from.cy());
    rects
        .iter()
        .filter_map(|&(w, r)| {
            let (dx, dy) = (r.cx() - cx, r.cy() - cy);
            let (along, across) = match d {
                Dir::Left => (-dx, dy),
                Dir::Right => (dx, dy),
                Dir::Up => (-dy, dx),
                Dir::Down => (dy, dx),
            };
            (along > 0.5).then_some((w, along + 2.0 * across.abs()))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(w, _)| w)
}

/// How long a split window takes to mute or come back (Omarchy's Hyprland `border` animation, `hypr/looknfeel.lua`:
/// speed 5.39, easeOutQuint).
pub const VEIL_FADE: f32 = 0.539;

/// `x` (0..1 of the time) through the veils' and moves' curve.
pub fn ease(x: f32) -> f32 {
    bezier(EASE_OUT_QUINT, x)
}

/// The split windows shown side by side: the snapped ones, while a maximized one does not cover them.
pub fn side_by_side(s: &State) -> Vec<(Win, Rect)> {
    if !s.tiled() || s.docked().is_some() {
        return Vec::new();
    }
    plan(s).tiles
}

/// How muted each split window should be (by `Win::slot`): with two or three side by side, all but the one last in
/// front 1. The floating windows are a layer of their own: one taking the keyboard changes nothing here.
pub fn veil_target(s: &State) -> [f32; 3] {
    let shown = side_by_side(s);
    let front = s.z.iter().rev().copied().find(|w| w.slot().is_some());
    let mut out = [0.0; 3];
    if shown.len() >= 2 {
        for (w, _) in shown.iter().filter(|t| Some(t.0) != front) {
            if let Some(i) = w.slot() {
                out[i] = 1.0;
            }
        }
    }
    out
}

/// The next preset (the status bar's split button).
pub fn next_preset(s: &mut State) {
    s.tiles.preset = (s.tiles.preset + 1) % PRESETS.len();
    s.tiles.save();
    s.message = Some(format!("Split: {}", PRESETS[s.tiles.preset].name));
}

// ---------------------------------------------------------------------------------------------------------- motion

/// Where the playlist, mixer and piano roll show in the tiling layout, back to front: the maximized one across the card,
/// the snapped ones (also those it covers) in their places. What the animations go between.
pub fn targets(s: &State) -> Vec<(Win, Rect)> {
    if !s.tiled() {
        return Vec::new();
    }
    let p = plan(s);
    s.z.iter()
        .filter(|w| w.slot().is_some())
        .filter_map(|&w| if s.is_max(w) { Some((w, s.work_rect())) } else { p.tiles.iter().find(|t| t.0 == w).copied() })
        .collect()
}

/// The animation, timed as Omarchy's Hyprland window moves (`hypr/looknfeel.lua`): 379 ms, easeOutQuint. Windows
/// opening or closing do not animate: one opening is at its place at once, under the moving ones, and one closing
/// leaves its last look there (`Snapshot`) for them to move over.
const MOVE: f32 = 0.379;

/// The curve (CSS cubic-bezier control points).
const EASE_OUT_QUINT: (f32, f32, f32, f32) = (0.23, 1.0, 0.32, 1.0);

/// `x` (0..1) through the cubic bezier from (0, 0) to (1, 1) with control points `c`.
fn bezier((x1, y1, x2, y2): (f32, f32, f32, f32), x: f32) -> f32 {
    if x <= 0.0 || x >= 1.0 {
        return x.clamp(0.0, 1.0);
    }
    let at = |a: f32, b: f32, t: f32| 3.0 * a * t * (1.0 - t) * (1.0 - t) + 3.0 * b * t * t * (1.0 - t) + t * t * t;
    // the curve's x is monotonic: find its t by bisection
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..24 {
        let mid = 0.5 * (lo + hi);
        if at(x1, x2, mid) < x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    at(y1, y2, 0.5 * (lo + hi))
}

/// The rect `k` of the way from `a` to `b`, on whole px (as every window's place is: views lay out from it and check
/// their parts against each other exactly).
fn lerp(a: Rect, b: Rect, k: f32) -> Rect {
    let l = |p: f32, q: f32| (p + (q - p) * k).round();
    let (x0, y0, x1, y1) = (l(a.x, b.x), l(a.y, b.y), l(a.right(), b.right()), l(a.bottom(), b.bottom()));
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

/// A change of the windows' places under way: `elapsed` seconds into it (`App` moves it on before each frame).
#[derive(Clone, Debug, PartialEq)]
pub struct Anim {
    pub elapsed: f32,
    /// Windows moving (from, to); windows opening (to); the places of windows closing (from) with their last look,
    /// once taken from the frame before (`App::paint_region`).
    moving: Vec<(Win, Rect, Rect)>,
    opening: Vec<(Win, Rect)>,
    pub closing: Vec<(Rect, Option<os_ui::Snapshot>)>,
}

impl Anim {
    /// From the places shown (`from`, back to front) to `to`; `None` when no window moves (one only opening or
    /// closing shows or goes at once).
    pub fn new(from: &[(Win, Rect)], to: &[(Win, Rect)]) -> Option<Anim> {
        let was = |w: Win| from.iter().find(|f| f.0 == w).map(|f| f.1);
        let moving: Vec<_> = to.iter().filter_map(|&(w, r)| was(w).filter(|&f| f != r).map(|f| (w, f, r))).collect();
        let opening: Vec<_> = to.iter().copied().filter(|&(w, _)| was(w).is_none()).collect();
        let closing: Vec<_> = from.iter().filter(|f| !to.iter().any(|t| t.0 == f.0)).map(|f| (f.1, None)).collect();
        (!moving.is_empty()).then_some(Anim { elapsed: 0.0, moving, opening, closing })
    }

    pub fn done(&self) -> bool {
        self.elapsed >= MOVE
    }

    /// Where moving window `w` shows now; `None`: at its place.
    pub fn at(&self, w: Win) -> Option<Rect> {
        let &(_, a, b) = self.moving.iter().find(|m| m.0 == w)?;
        Some(lerp(a, b, bezier(EASE_OUT_QUINT, self.elapsed / MOVE)))
    }

    /// The windows opening, at their places: painted under the moving ones.
    pub fn opening(&self) -> &[(Win, Rect)] {
        &self.opening
    }

    /// The places shown now, back to front as `to` (to start another change from here).
    pub fn shown(&self, to: &[(Win, Rect)]) -> Vec<(Win, Rect)> {
        to.iter().map(|&(w, r)| (w, self.at(w).unwrap_or(r))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;

    fn lemon() -> App {
        let mut app = App::new(None);
        app.state.screen = (1536.0, 864.0);
        app.state.bar.tiling = true;
        app.state.wins.iter_mut().for_each(|w| (w.maxed, w.mini) = (false, false));
        app.state.z.retain(|w| w.slot().is_none());
        app
    }

    /// One snapped window fills the card; two share it by the preset; three fill its three places, a gutter apart.
    #[test]
    fn snapped_windows_fill_the_card() {
        let mut app = lemon();
        let work = app.state.work_rect();
        app.state.raise(Win::Playlist);
        assert_eq!(plan(&app.state).tiles, vec![(Win::Playlist, work)]);
        app.state.raise(Win::PianoRoll);
        let p = plan(&app.state);
        let (pl, pr) = (p.tiles[0].1, p.tiles[1].1);
        assert_eq!((p.tiles[0].0, p.tiles[1].0), (Win::Playlist, Win::PianoRoll));
        assert_eq!((pl.y, pl.w, pr.bottom(), pr.y - pl.bottom()), (work.y, work.w, work.bottom(), GUTTER), "playlist over the piano roll");
        app.state.raise(Win::Mixer);
        let p = plan(&app.state);
        let get = |w: Win| p.tiles.iter().find(|t| t.0 == w).unwrap().1;
        let (pr, mx) = (get(Win::PianoRoll), get(Win::Mixer));
        assert_eq!((pr.y, mx.y, mx.x - pr.right(), mx.right()), (mx.y, pr.y, GUTTER, work.right()), "piano roll beside the mixer");
        assert_eq!(p.dividers.len(), 2);
        // Left and right: the mixer alone on the right.
        next_preset(&mut app.state);
        let p = plan(&app.state);
        let get = |w: Win| p.tiles.iter().find(|t| t.0 == w).unwrap().1;
        assert_eq!((get(Win::Mixer).h, get(Win::Mixer).right()), (work.h, work.right()));
        assert_eq!(get(Win::Playlist).x, work.x);
    }

    /// The curve ends where it should; a swap moves both windows; a window opening or closing alone does not
    /// animate, beside a move its place holds an empty card.
    #[test]
    fn motion() {
        assert!(bezier(EASE_OUT_QUINT, 0.0) == 0.0 && bezier(EASE_OUT_QUINT, 1.0) == 1.0);
        assert!(bezier(EASE_OUT_QUINT, 0.3) > 0.8, "fast out of the gate");
        let (a, b) = (Rect::new(0.0, 0.0, 100.0, 50.0), Rect::new(0.0, 56.0, 100.0, 50.0));
        let mut m = Anim::new(&[(Win::Playlist, a), (Win::Mixer, b)], &[(Win::Playlist, b), (Win::Mixer, a), (Win::PianoRoll, b)]).unwrap();
        assert_eq!(m.opening(), &[(Win::PianoRoll, b)]);
        let c = Anim::new(&[(Win::Playlist, a), (Win::Mixer, b)], &[(Win::Mixer, a), (Win::PianoRoll, b)]).unwrap();
        assert_eq!(c.closing, vec![(a, None)]);
        assert_eq!(m.at(Win::Playlist), Some(a));
        m.elapsed = MOVE;
        assert_eq!(m.at(Win::Playlist), Some(b));
        assert!(m.done());
        assert_eq!(Anim::new(&[(Win::Playlist, a), (Win::Mixer, b)], &[(Win::Playlist, a)]), None, "a close alone");
        assert_eq!(Anim::new(&[(Win::Playlist, a)], &[(Win::Playlist, a), (Win::Mixer, b)]), None, "an open alone");
    }

    /// A maximized window covers the card and is not in the plan; restoring it snaps it back.
    #[test]
    fn maximize_and_restore() {
        let mut app = lemon();
        app.state.raise(Win::Playlist);
        app.state.raise(Win::Mixer);
        app.act(crate::app::Action::MaxWin(Win::Playlist));
        assert!(app.state.is_max(Win::Playlist) && app.state.in_front(Win::Playlist));
        assert_eq!(app.state.win_rect(Win::Playlist), app.state.work_rect());
        assert_eq!(plan(&app.state).tiles.iter().map(|t| t.0).collect::<Vec<_>>(), vec![Win::Mixer]);
        app.act(crate::app::Action::MaxWin(Win::Playlist));
        assert_eq!(plan(&app.state).tiles.len(), 2);
        // Raising a snapped window brings the split back from a maximized one.
        app.act(crate::app::Action::MaxWin(Win::Mixer));
        app.state.raise(Win::Playlist);
        assert!(!app.state.is_max(Win::Mixer) && plan(&app.state).tiles.len() == 2);
    }

    /// F7 / F9 each show their window in its place and hide it again, whichever is in front; a maximized one hides
    /// on its key, and a snapped one hidden behind it shows.
    #[test]
    fn keys_toggle_each_window() {
        use crate::app::Action::{MaxWin, ToggleWin};
        let mut app = lemon();
        let shown = |app: &App| plan(&app.state).tiles.iter().map(|t| t.0).collect::<Vec<_>>();
        app.act(ToggleWin(Win::Mixer));
        app.act(ToggleWin(Win::PianoRoll));
        assert_eq!(shown(&app), vec![Win::PianoRoll, Win::Mixer]);
        app.act(ToggleWin(Win::Mixer));
        assert_eq!(shown(&app), vec![Win::PianoRoll], "the mixer hides though the piano roll is in front");
        app.act(ToggleWin(Win::Mixer));
        app.act(MaxWin(Win::Mixer));
        app.act(ToggleWin(Win::PianoRoll));
        assert_eq!(shown(&app), vec![Win::PianoRoll, Win::Mixer], "the piano roll comes out from behind, the split back");
        app.act(MaxWin(Win::Mixer));
        app.act(ToggleWin(Win::Mixer));
        assert!(!app.state.shown(Win::Mixer), "maximized, its key hides it");
    }

    /// A window alone neither maximizes nor splits; a maximized window closed opens again into the split, and a window
    /// opening while another is maximized brings the split back with it in.
    #[test]
    fn alone_and_reopening() {
        use crate::app::Action::{MaxWin, ToggleWin};
        let mut app = lemon();
        app.act(ToggleWin(Win::Mixer));
        app.act(MaxWin(Win::Mixer));
        assert!(app.state.alone() && !app.state.wins[1].maxed, "alone: nothing to maximize over");
        app.act(ToggleWin(Win::Playlist));
        app.act(MaxWin(Win::Mixer));
        assert!(app.state.is_max(Win::Mixer));
        app.act(ToggleWin(Win::PianoRoll));
        assert_eq!(plan(&app.state).tiles.len(), 3, "opening the piano roll splits the three");
        app.act(MaxWin(Win::Mixer));
        app.act(ToggleWin(Win::Mixer));
        app.act(ToggleWin(Win::Mixer));
        assert!(app.state.snapped(Win::Mixer), "closed maximized, it opens into the split");
    }

    /// Floating windows are a layer above the split ones: bringing a snapped window forward (and giving it the
    /// keyboard) leaves the channel rack painted, and clicked, on top.
    #[test]
    fn floats_stay_on_top() {
        let Ok(mut c) = os_ui::Canvas::new(1536, 864, 1.0) else { return };
        let mut app = lemon();
        app.state.raise(Win::Playlist);
        app.state.raise(Win::ChannelRack);
        app.state.raise(Win::Mixer);
        assert_eq!(app.state.z.last(), Some(&Win::Mixer), "the mixer has the keyboard");
        assert_eq!(app.state.paint_order().last(), Some(&Win::ChannelRack));
        app.paint(&mut c, None);
        let rack = crate::views::rack::rect(&app.state);
        let hit = app.hits.iter().rev().find(|h| h.rect.contains(rack.cx(), rack.cy())).map(|h| h.win);
        assert_eq!(hit, Some(Some(Win::ChannelRack)));
    }

    /// Ctrl+Alt+arrows give the keyboard to the split window that way (the channel rack is no stop); with Shift the split
    /// windows change places. F maximizes the window under the mouse; while one is maximized, the arrows maximize the
    /// one that way instead, and F brings the split back. The other split windows are muted.
    #[test]
    fn window_keys() {
        use crate::app::Action::ToggleWin;
        use winit::keyboard::{Key, NamedKey};
        let Ok(mut c) = os_ui::Canvas::new(1536, 864, 1.0) else { return };
        let mut app = lemon();
        app.state.hide(Win::ChannelRack);
        for w in [Win::Playlist, Win::PianoRoll, Win::Mixer] {
            app.act(ToggleWin(w));
        }
        let at = |app: &App, w: Win| rect_of(&app.state, w).unwrap_or_else(|| panic!("{w:?} not snapped: {:?}", app.state.z));
        let arrow = |app: &mut App, k: NamedKey, shift: bool| {
            app.shift = shift;
            app.key(&Key::Named(k), true, true);
            app.shift = false;
        };
        let f = |app: &mut App, c: &mut os_ui::Canvas, (x, y): (f32, f32)| {
            app.paint(c, None);
            app.mouse = (x, y);
            app.key(&Key::Character("f".into()), false, false);
        };
        let look = |app: &mut App, c: &mut os_ui::Canvas, w: Win| {
            app.take_damage();
            app.paint(c, None);
            let r = at(app, w);
            let p = c.pixmap.pixel(r.cx() as u32, r.cy() as u32).unwrap();
            (p.red(), p.green(), p.blue())
        };

        app.state.raise(Win::Playlist);
        let lit = look(&mut app, &mut c, Win::Playlist);
        arrow(&mut app, NamedKey::ArrowDown, false);
        let below = app.state.focused().unwrap();
        assert!(below != Win::Playlist && at(&app, below).y > at(&app, Win::Playlist).y, "{below:?} is below");
        assert!(look(&mut app, &mut c, Win::Playlist) != lit, "without the keyboard, the playlist is muted");
        arrow(&mut app, NamedKey::ArrowUp, false);
        assert_eq!(app.state.focused(), Some(Win::Playlist));

        arrow(&mut app, NamedKey::ArrowDown, true);
        assert_eq!(app.state.focused(), Some(Win::Playlist), "the keyboard moves with the window");
        assert!(at(&app, Win::Playlist).y > at(&app, below).y, "the playlist and {below:?} changed places");

        let m = at(&app, Win::Mixer);
        f(&mut app, &mut c, (m.cx(), m.cy()));
        assert_eq!(app.state.docked(), Some(Win::Mixer), "F maximizes the window under the mouse");
        let w = app.state.work_rect();
        let mut px = |app: &mut App, veil: f32| {
            app.state.veil[1] = veil;
            app.invalidate();
            app.paint(&mut c, None);
            c.pixmap.data().to_vec()
        };
        assert!(px(&mut app, 1.0) == px(&mut app, 0.0), "maximized at {w:?}, the mixer is never muted");
        let all = plan_all(&app.state).tiles;
        let from = all.iter().find(|t| t.0 == Win::Mixer).unwrap().1;
        let others: Vec<(Win, Rect)> = all.into_iter().filter(|t| t.0 != Win::Mixer).collect();
        let (k, d) = [(NamedKey::ArrowUp, Dir::Up), (NamedKey::ArrowLeft, Dir::Left), (NamedKey::ArrowRight, Dir::Right)]
            .into_iter()
            .find(|&(_, d)| neighbour(&others, from, d).is_some())
            .unwrap();
        let next = neighbour(&others, from, d).unwrap();
        arrow(&mut app, k, false);
        assert_eq!(app.state.docked(), Some(next), "maximized, the arrows maximize the window that way");
        assert!(!app.state.is_max(Win::Mixer));
        let w = app.state.work_rect();
        f(&mut app, &mut c, (w.cx(), w.cy()));
        assert_eq!(app.state.docked(), None, "F again: the split");

        // The channel rack is no stop: from the playlist, an arrow goes straight to the split window that way, the
        // rack open or not; from the rack, the arrows reach a split window.
        app.state.raise(Win::ChannelRack);
        app.state.raise(Win::Playlist);
        let tiles = plan(&app.state).tiles;
        let from = tiles.iter().find(|t| t.0 == Win::Playlist).unwrap().1;
        let others: Vec<(Win, Rect)> = tiles.into_iter().filter(|t| t.0 != Win::Playlist).collect();
        let (k, want) = [(NamedKey::ArrowUp, Dir::Up), (NamedKey::ArrowDown, Dir::Down), (NamedKey::ArrowLeft, Dir::Left), (NamedKey::ArrowRight, Dir::Right)]
            .into_iter()
            .find_map(|(k, d)| neighbour(&others, from, d).map(|w| (k, w)))
            .unwrap();
        arrow(&mut app, k, false);
        assert_eq!(app.state.focused(), Some(want), "{k:?} from the playlist, past the rack");
        app.state.raise(Win::ChannelRack);
        let away = [NamedKey::ArrowLeft, NamedKey::ArrowRight, NamedKey::ArrowUp, NamedKey::ArrowDown].into_iter().any(|k| {
            arrow(&mut app, k, false);
            let gone = app.state.focused().is_some_and(|w| w.slot().is_some());
            app.state.raise(Win::ChannelRack);
            gone
        });
        assert!(away, "from the rack the arrows reach a split window");
        arrow(&mut app, NamedKey::ArrowLeft, true);
        assert_eq!(app.state.focused(), Some(Win::ChannelRack), "a floating window changes places with none");
    }

    /// The floating windows leave the split ones as they are: the channel rack taking the keyboard mutes or brightens
    /// none of them.
    #[test]
    fn floats_leave_the_veils() {
        let mut app = lemon();
        app.state.hide(Win::ChannelRack);
        app.state.raise(Win::Mixer);
        app.state.raise(Win::Playlist);
        app.take_damage();
        let before = app.state.veil;
        assert_eq!(before, [0.0, 1.0, 0.0], "the mixer muted under the playlist in front");
        app.state.raise(Win::ChannelRack);
        app.take_damage();
        assert_eq!(app.state.veil, before);
        app.state.no_focus = true;
        app.take_damage();
        assert_eq!(app.state.veil, before, "nor does a click on no window");
    }

    /// A split window's clicks stay in its card: the playlist's last track row, cut by its bottom edge, takes none of
    /// the mixer's below it, though the playlist is in front.
    #[test]
    fn clicks_stay_in_their_window() {
        let Ok(mut c) = os_ui::Canvas::new(1536, 864, 1.0) else { return };
        let mut app = lemon();
        app.state.hide(Win::ChannelRack);
        app.state.raise(Win::Mixer);
        app.state.raise(Win::Playlist);
        app.paint(&mut c, None);
        let m = rect_of(&app.state, Win::Mixer).unwrap();
        for y in (m.y as i32 + 1..m.bottom() as i32).step_by(7) {
            for x in (m.x as i32 + 1..m.right() as i32).step_by(23) {
                let w = app.top_hit(x as f32, y as f32).and_then(|h| h.win);
                assert!(w == Some(Win::Mixer), "({x}, {y}) in the mixer at {m:?} clicks {w:?}");
            }
        }
    }

    /// The mixer's cables hang over the gap under it, to the window below or the status bar.
    #[test]
    fn mixer_cables_hang_over_the_gap() {
        let mut app = lemon();
        app.state.raise(Win::Mixer);
        app.state.raise(Win::Playlist);
        let mx = rect_of(&app.state, Win::Mixer).unwrap();
        // top and bottom: the playlist on top (place 0), the mixer below
        assert_eq!(app.state.mixer_hang().map(|h| h.bottom()), Some(crate::views::status_rect(&app.state).y));
        let pl = rect_of(&app.state, Win::Playlist).unwrap();
        assert!(drop(&mut app.state, Win::Mixer, (pl.cx(), pl.cy())));
        let mx2 = rect_of(&app.state, Win::Mixer).unwrap();
        assert!(mx2.y < mx.y, "the mixer on top now");
        assert_eq!(app.state.mixer_hang(), Some(Rect::new(mx2.x, mx2.bottom(), mx2.w, GUTTER)));
    }

    /// The gap drags the split, within bounds; a title bar dropped on another window swaps them; all is kept.
    #[test]
    fn drag_drop_and_keep() {
        let mut app = lemon();
        app.state.raise(Win::Playlist);
        app.state.raise(Win::PianoRoll);
        let work = app.state.work_rect();
        drag(&mut app.state, 0, (0.0, work.y + 0.25 * work.h));
        let pl = rect_of(&app.state, Win::Playlist).unwrap();
        assert!((pl.h / work.h - 0.25).abs() < 0.02, "{}", pl.h);
        drag(&mut app.state, 0, (0.0, work.y));
        let pl = rect_of(&app.state, Win::Playlist).unwrap();
        assert!((pl.h - least(Win::Playlist, true)).abs() <= 1.0, "stops at the playlist's least height: {}", pl.h);
        // a double click on the gap: the default split
        let gap = plan(&app.state).dividers[0].gap;
        app.mouse_move(gap.cx(), gap.cy());
        for _ in 0..2 {
            app.act(crate::app::Action::TileSplit(0));
            app.mouse_up();
        }
        assert_eq!(app.state.tiles.share[0][0], Tiles::default().share[0][0]);
        let pr = rect_of(&app.state, Win::PianoRoll).unwrap();
        assert!(drop(&mut app.state, Win::Playlist, (pr.cx(), pr.cy())));
        assert_eq!(rect_of(&app.state, Win::PianoRoll).unwrap().y, work.y, "swapped");
        assert_eq!(Tiles::load(), app.state.tiles);
        assert_eq!(Tiles::parse("0 playlist playlist mixer 0.5 0.5 0.5 0.5"), None);
    }
}


#[cfg(test)]
mod frames {
    use super::*;
    use crate::app::App;
    /// Every frame of every move paints without a view's checks tripping (fractional places once crashed the playlist's:
    /// its parts, worked out two ways, differed by float rounding).
    #[test]
    fn frames_paint() {
        let Ok(mut c) = os_ui::Canvas::new(1536, 864, 1.25) else { return };
        let mut app = App::new(None);
        app.state.bar.tiling = true;
        app.state.wins.iter_mut().for_each(|w| (w.maxed, w.mini) = (false, false));
        app.state.z.retain(|w| w.slot().is_none());
        for w in [Win::Playlist, Win::PianoRoll, Win::Mixer] {
            app.state.raise(w);
        }
        app.paint(&mut c, None);
        // the playlist across x 700..1000 on fractional px (where a view's checks met float rounding)
        let (a0, a1) = (Rect::new(700.0, 60.0, 600.0, 400.0), Rect::new(1000.7, 60.3, 500.0, 400.0));
        for i in 0..40 {
            let Some(mut m) = Anim::new(&[(Win::Playlist, a0)], &[(Win::Playlist, a1)]) else { break };
            m.elapsed = 0.0093 * i as f32;
            app.state.tile_anim = Some(m);
            app.paint(&mut c, None);
        }
        let all = targets(&app.state);
        for (from, to) in [(all.clone(), all.iter().rev().map(|t| t.0).zip(all.iter().map(|t| t.1)).collect::<Vec<_>>()), (all[..2].to_vec(), all.clone()), (all.clone(), all[..2].to_vec())] {
            for i in 0..40 {
                let Some(mut a) = Anim::new(&from, &to) else { break };
                a.elapsed = 0.01 * i as f32;
                app.state.tile_anim = Some(a);
                app.paint(&mut c, None);
            }
        }
    }
}
