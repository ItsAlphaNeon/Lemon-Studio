// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! The piano roll's snap (the toolbar's magnet button): the Snap menu and Stock note lengths, and where notes, the
//! play-start marker, time selections and event-panel points land.
// spec: ui/piano-snap.md

use crate::app::State;
use crate::piano::{self, Geom};
use crate::piano_tools::NCmd;
use crate::playlist::{Cmd, Mark, PItem, Popup, Snap};

/// The toolbar's global snap, which Main follows. Its control is not available yet and shows Line (§8).
pub const GLOBAL: Snap = Snap::Line;

/// From this zoom (px per beat) up, Line does not snap (§4: FL's maximum zoom; §8).
const NO_LINE_PPB: f32 = 768.0;
/// Line takes the smallest unit at least this wide (§4).
const LINE_MIN_PX: f32 = 10.0;
/// Events and Markers catch a target this close (§3).
const CATCH_PX: f32 = 5.0;

/// The magnet button's setting (§2). Kept while the program runs, not saved (§8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PianoSnap {
    /// The global snap.
    #[default]
    Main,
    /// Line, Cell, (none) and the fixed units, as the playlist names them.
    Grid(Snap),
    /// The starts and ends of the pattern's notes.
    Events,
    /// The pattern's time markers.
    Markers,
}

impl PianoSnap {
    pub fn label(self) -> &'static str {
        match self {
            PianoSnap::Main => "Main",
            PianoSnap::Grid(sn) => sn.label(),
            PianoSnap::Events => "Events",
            PianoSnap::Markers => "Markers",
        }
    }

    /// The grid setting, Main resolved; None for Events and Markers.
    fn grid(self) -> Option<Snap> {
        match self {
            PianoSnap::Main => Some(GLOBAL),
            PianoSnap::Grid(sn) => Some(sn),
            PianoSnap::Events | PianoSnap::Markers => None,
        }
    }
}

/// Line's unit at a zoom (§4): the smallest of step, 1/2 beat, beat and bar at least 10 px wide; one tick from
/// 768 px per beat up.
pub fn line_unit(ppq: u32, px_per_beat: f32) -> u32 {
    if px_per_beat >= NO_LINE_PPB {
        return 1;
    }
    let px = |u: u32| u as f32 * px_per_beat / ppq as f32;
    [ppq / 4, ppq / 2, ppq].into_iter().find(|&u| u > 0 && px(u) >= LINE_MIN_PX).unwrap_or(ppq * 4).max(1)
}

/// A setting's unit in ticks at a zoom (§3); None for (none), Events and Markers.
pub fn unit_of(snap: PianoSnap, ppq: u32, px_per_beat: f32) -> Option<u32> {
    match snap.grid()? {
        Snap::None => None,
        Snap::Line | Snap::Cell => Some(line_unit(ppq, px_per_beat)),
        sn => Some(sn.ticks(ppq, 0.0)),
    }
}

fn ppq(s: &State) -> u32 {
    u32::from(s.project.ppq.max(4))
}

/// The current setting's unit at the piano roll's zoom.
pub fn unit(s: &State) -> Option<u32> {
    unit_of(s.piano.snap, ppq(s), s.piano.px_per_beat)
}

/// The global snap's unit at the piano roll's zoom: Quick quantize uses it (§6, §8).
pub fn global_unit(s: &State) -> u32 {
    unit_of(PianoSnap::Main, ppq(s), s.piano.px_per_beat).unwrap_or(1)
}

/// What Events and Markers catch (§3): the starts and ends of the pattern's notes (the other channels' too while
/// ghost channels show), or its time markers. `skip`: notes being dragged.
fn targets(s: &State, skip: &[usize]) -> Vec<u32> {
    match s.piano.snap {
        PianoSnap::Events => {
            let ch = piano::channel(s);
            piano::notes(s)
                .iter()
                .enumerate()
                .filter(|&(i, n)| !skip.contains(&i) && (s.piano.ghosts || Some(n.channel) == ch))
                .flat_map(|(_, n)| [n.position, n.position + piano::shown_len(s, n)])
                .collect()
        }
        PianoSnap::Markers => s.project.pattern(s.pattern).map_or(Vec::new(), |p| p.markers.iter().map(|m| m.position).collect()),
        _ => Vec::new(),
    }
}

/// The target nearest `tick` within 5 px.
fn catch(targets: &[u32], tick: f32, ppt: f32) -> Option<u32> {
    targets
        .iter()
        .map(|&t| (t, (t as f32 - tick).abs() * ppt))
        .filter(|&(_, px)| px <= CATCH_PX)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(t, _)| t)
}

/// Where a click at `tick` lands (§5): the nearest unit (Cell: the start of the unit it is in), the tick under the
/// pointer without a unit, a note edge or marker within 5 px for Events and Markers. Alt: no snapping.
pub fn place(s: &State, g: &Geom, tick: f32, alt: bool) -> u32 {
    let t = tick.max(0.0);
    if alt {
        return t as u32;
    }
    if let Some(c) = catch(&targets(s, &[]), t, g.ppt) {
        return c;
    }
    match unit(s) {
        Some(u) if s.piano.snap.grid() == Some(Snap::Cell) => t as u32 / u * u,
        Some(u) => (t / u as f32).round() as u32 * u,
        None => t as u32,
    }
}

/// A move's or resize's distance (§5, §8): rounded to the unit, whole ticks without one; for Events and Markers
/// the dragged edge (at `from`) goes to a target within 5 px. Alt: no snapping. `skip`: the dragged notes.
pub fn delta(s: &State, g: &Geom, from: u32, dt: f32, alt: bool, skip: &[usize]) -> i64 {
    if alt {
        return dt.round() as i64;
    }
    if let Some(c) = catch(&targets(s, skip), from as f32 + dt, g.ppt) {
        return i64::from(c) - i64::from(from);
    }
    match unit(s) {
        Some(u) => (dt / u as f32).round() as i64 * i64::from(u),
        None => dt.round() as i64,
    }
}

/// The start of the unit `tick` is in (the tick itself without a unit): the event panel's channel lanes set a
/// point in every unit the pointer crosses (§8).
pub fn cell(s: &State, tick: f32) -> u32 {
    let t = tick.max(0.0) as u32;
    unit(s).map_or(t, |u| t - t % u)
}

/// The Snap menu (§2). The caller opens it 11 px left of and below the button's centre (§1).
pub fn snap_menu(s: &State, x: f32, y: f32) -> Popup {
    let now = s.piano.snap;
    let radio = |p: PianoSnap| PItem::new(p.label(), Cmd::Piano(NCmd::Snap(p))).mark(Mark::Radio(now == p));
    let mut items = vec![radio(PianoSnap::Main), PItem::sep(), radio(PianoSnap::Grid(Snap::Line)), radio(PianoSnap::Grid(Snap::Cell)), PItem::sep()];
    items.extend(Snap::MENU.iter().filter(|sn| !matches!(sn, Snap::Line | Snap::Cell)).map(|&sn| radio(PianoSnap::Grid(sn))));
    items.extend([PItem::sep(), radio(PianoSnap::Events), radio(PianoSnap::Markers)]);
    Popup { x, y, items, sub: None }
}

/// The right-click menu (§7): the length of the next notes drawn.
pub fn lengths_menu(x: f32, y: f32) -> Popup {
    let mut items = vec![PItem::head("Stock note lengths")];
    items.extend(Snap::MENU.iter().filter(|sn| !matches!(sn, Snap::Line | Snap::Cell | Snap::None)).map(|&sn| PItem::new(sn.label(), Cmd::Piano(NCmd::StockLength(sn)))));
    Popup { x, y, items, sub: None }
}

/// Pick a Snap menu entry.
pub fn set(s: &mut State, p: PianoSnap) -> String {
    s.piano.snap = p;
    format!("Piano roll snap: {}", p.label())
}

/// Pick a stock note length: the next notes drawn get it, the snap is unchanged (§7).
pub fn set_length(s: &mut State, sn: Snap) -> String {
    s.piano.last_len = sn.ticks(ppq(s), 0.0);
    format!("New notes: {}", sn.label())
}

/// The button's hint (§8).
pub fn hint(s: &State) -> String {
    format!("Snap to grid: {} (right-click: stock note lengths)", s.piano.snap.label())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_follows_the_zoom() {
        // §4 at PPQ 96: a step down to 10 px wide, then 1/2 beat, beat, bar; none at FL's maximum zoom.
        assert_eq!(line_unit(96, 768.0), 1);
        assert_eq!(line_unit(96, 1600.0), 1);
        assert_eq!(line_unit(96, 747.8), 24);
        assert_eq!(line_unit(96, 96.0), 24);
        assert_eq!(line_unit(96, 40.0), 24);
        assert_eq!(line_unit(96, 39.0), 48);
        assert_eq!(line_unit(96, 20.0), 48);
        assert_eq!(line_unit(96, 19.0), 96);
        assert_eq!(line_unit(96, 10.0), 96);
        assert_eq!(line_unit(96, 6.0), 384);
        // The fixed units at PPQ 96 and the settings without one.
        let u = |p: PianoSnap| unit_of(p, 96, 96.0);
        assert_eq!(u(PianoSnap::Main), Some(24));
        assert_eq!(u(PianoSnap::Grid(Snap::Cell)), Some(24));
        assert_eq!(u(PianoSnap::Grid(Snap::SixthStep)), Some(4));
        assert_eq!(u(PianoSnap::Grid(Snap::ThirdStep)), Some(8));
        assert_eq!(u(PianoSnap::Grid(Snap::ThirdBeat)), Some(32));
        assert_eq!(u(PianoSnap::Grid(Snap::Bar)), Some(384));
        assert_eq!(u(PianoSnap::Grid(Snap::None)), None);
        assert_eq!(u(PianoSnap::Events), None);
        assert_eq!(u(PianoSnap::Markers), None);
    }

    #[test]
    fn menus_match_the_spec() {
        let app = crate::app::App::new(None);
        let s = &app.state;
        let labels = |p: &Popup| p.items.iter().map(|i| if i.sep { "-".to_string() } else { i.label.to_string() }).collect::<Vec<_>>().join(",");
        assert_eq!(
            labels(&snap_menu(s, 0.0, 0.0)),
            "Main,-,Line,Cell,-,(none),1/6 step,1/4 step,1/3 step,1/2 step,Step,1/6 beat,1/4 beat,1/3 beat,1/2 beat,Beat,Bar,-,Events,Markers"
        );
        let l = labels(&lengths_menu(0.0, 0.0));
        assert_eq!(l, "Stock note lengths,1/6 step,1/4 step,1/3 step,1/2 step,Step,1/6 beat,1/4 beat,1/3 beat,1/2 beat,Beat,Bar");
    }
}
