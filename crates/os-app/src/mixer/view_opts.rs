// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The mixer's Options ▸ View ▸ Names at top, Routing cables, Compact plugin list, Colorful mixer ▸ and Lines between
//! tracks (spec: ui/mixer.md §6.2.1, from the manual). They are switches of the window, kept in the settings file: the
//! spec does not say where the original keeps them, and none is a project setting or an undo step.

use super::*;
use crate::prefs::{Check, Combo};

/// Names at top: the track names read from the top of their column instead of from above the LED.
pub const NAMES_TOP: Check = Check { key: "mixer_names_top", label: "Names at top", default: false };
/// Routing cables: the current track's cables into its sends are drawn (the default template shows them).
pub const CABLES: Check = Check { key: "mixer_routing_cables", label: "Routing cables", default: true };
/// Compact plugin list: the Extra wide layout's plugin lists leave out empty slots. Lemon Studio draws only the Wide
/// layout, so this switch is kept for that layout and changes nothing drawn yet.
pub const COMPACT: Check = Check { key: "mixer_compact_plugins", label: "Compact plugin list", default: false };
/// Lines between tracks: a light line divides the strips.
pub const LINES: Check = Check { key: "mixer_track_lines", label: "Lines between tracks", default: false };
/// Colorful mixer ▸: how much of a coloured track's strip its colour fills; Off colours only the strip's top.
pub const COLORFUL: Combo =
    Combo { key: "mixer_colorful", label: "Colorful mixer", choices: &["Off", "Low", "Medium", "High"], default: 0 };

/// A View ▸ entry (`MCmd::ViewOpt`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Opt {
    NamesTop,
    Cables,
    Compact,
    Lines,
    /// Colorful mixer ▸ Off (0), Low, Medium, High (3).
    Colorful(u8),
}

/// The entries after Waveforms, in the manual's order (Track inspector on left side, Alternative mixer highlighting
/// and Always show external input levels are not here yet).
pub fn items() -> Vec<PItem> {
    let check = |c: &'static Check, o: Opt| {
        PItem::new(c.label, Cmd::Mixer(MCmd::ViewOpt(o))).mark(Mark::Check(crate::prefs::check(c)))
    };
    let level = colorful();
    let colorful = COLORFUL
        .choices
        .iter()
        .enumerate()
        .map(|(i, l)| {
            PItem::new(*l, Cmd::Mixer(MCmd::ViewOpt(Opt::Colorful(i as u8)))).mark(Mark::Radio(usize::from(level) == i))
        })
        .collect();
    vec![
        check(&NAMES_TOP, Opt::NamesTop),
        check(&CABLES, Opt::Cables),
        check(&COMPACT, Opt::Compact),
        PItem::sub(COLORFUL.label, colorful),
        check(&LINES, Opt::Lines),
    ]
}

/// Runs a View ▸ entry: the switch flips, or the colour level is chosen.
pub fn run(o: Opt) {
    match o {
        Opt::NamesTop => _ = crate::prefs::toggle(&NAMES_TOP),
        Opt::Cables => _ = crate::prefs::toggle(&CABLES),
        Opt::Compact => _ = crate::prefs::toggle(&COMPACT),
        Opt::Lines => _ = crate::prefs::toggle(&LINES),
        Opt::Colorful(i) => crate::prefs::set_choice(&COLORFUL, usize::from(i)),
    }
}

pub fn names_top() -> bool {
    crate::prefs::check(&NAMES_TOP)
}

pub fn cables() -> bool {
    crate::prefs::check(&CABLES)
}

pub fn lines() -> bool {
    crate::prefs::check(&LINES)
}

/// Colorful mixer's level: 0 Off .. 3 High.
pub fn colorful() -> u8 {
    crate::prefs::index(&COLORFUL) as u8
}

/// How much of a coloured track's colour goes into its strip's body at the current level (0 at Off: only the header
/// band is coloured then, `HEADER_TINT`).
pub fn body_tint() -> f32 {
    [0.0, 0.12, 0.22, 0.34][usize::from(colorful().min(3))]
}

/// How much of a coloured track's colour goes into its header band, at every level.
pub const HEADER_TINT: f32 = 0.45;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Action, Win};

    fn app() -> App {
        let mut a = App::new(None);
        a.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        a.state.z = vec![Win::Mixer];
        a
    }

    fn frame(a: &mut App, c: &mut os_ui::Canvas) {
        a.invalidate();
        a.paint(c, None);
    }

    fn strip(a: &App, n: u16) -> os_ui::Rect {
        a.hits.iter().rev().find(|h| h.action == Action::Mix(MHit::Strip(n))).map(|h| h.rect).expect("the strip")
    }

    /// The pixels of `r` in the last frame.
    fn pixels(c: &os_ui::Canvas, r: os_ui::Rect) -> Vec<(u8, u8, u8)> {
        (r.y as u32..r.bottom() as u32)
            .flat_map(|y| (r.x as u32..r.right() as u32).map(move |x| (x, y)))
            .filter_map(|(x, y)| c.pixmap.pixel(x, y).map(|p| (p.red(), p.green(), p.blue())))
            .collect()
    }

    fn view(a: &App) -> Vec<(String, Mark)> {
        let m = options_menu(&a.state, 0.0, 0.0);
        let v = m.items.iter().find(|i| i.label == "View").unwrap();
        v.children.iter().map(|i| (i.label.to_string(), i.mark.clone())).collect()
    }

    /// spec: ui/mixer.md §6.2.1: the entries in the manual's order, with their defaults (names at the bottom, cables
    /// on, plain strips, no lines); each one flips and is kept in the settings, not in the project or the undo history.
    #[test]
    fn view_entries_flip_and_are_kept() {
        let mut a = app();
        let labels: Vec<String> = view(&a).into_iter().map(|e| e.0).collect();
        assert_eq!(
            labels,
            [
                "Waveforms",
                "Names at top",
                "Routing cables",
                "Compact plugin list",
                "Colorful mixer",
                "Lines between tracks",
                "Extra properties"
            ]
        );
        let mark = |a: &App, l: &str| view(a).into_iter().find(|e| e.0 == l).unwrap().1;
        assert_eq!(
            [
                mark(&a, "Names at top"),
                mark(&a, "Routing cables"),
                mark(&a, "Compact plugin list"),
                mark(&a, "Lines between tracks")
            ],
            [Mark::Check(false), Mark::Check(true), Mark::Check(false), Mark::Check(false)]
        );
        let steps = a.state.session.history.at();
        for o in [Opt::NamesTop, Opt::Cables, Opt::Compact, Opt::Lines] {
            super::super::run(&mut a, MCmd::ViewOpt(o));
        }
        assert_eq!(
            [
                mark(&a, "Names at top"),
                mark(&a, "Routing cables"),
                mark(&a, "Compact plugin list"),
                mark(&a, "Lines between tracks")
            ],
            [Mark::Check(true), Mark::Check(false), Mark::Check(true), Mark::Check(true)]
        );
        assert_eq!(crate::settings_file::read(|s| s.get("mixer_names_top").map(str::to_string)), Some("1".into()));
        let colors = |a: &App| {
            let m = options_menu(&a.state, 0.0, 0.0);
            let v = m.items.into_iter().find(|i| i.label == "View").unwrap();
            let c = v.children.into_iter().find(|i| i.label == "Colorful mixer").unwrap();
            c.children.iter().map(|i| (i.label.to_string(), i.mark.clone())).collect::<Vec<_>>()
        };
        assert_eq!(colors(&a)[0], ("Off".into(), Mark::Radio(true)));
        super::super::run(&mut a, MCmd::ViewOpt(Opt::Colorful(2)));
        assert_eq!(colors(&a)[2], ("Medium".into(), Mark::Radio(true)));
        assert_eq!(colorful(), 2);
        assert_eq!(a.state.session.history.at(), steps, "no undo step");
        for o in [Opt::NamesTop, Opt::Cables, Opt::Compact, Opt::Lines, Opt::Colorful(0)] {
            run(o);
        }
    }

    /// Routing cables off: no cable is drawn from the current track's sends; Lines between tracks: a light line at each
    /// strip's right edge.
    #[test]
    fn routing_cables_and_lines_between_tracks_paint() {
        let Ok(mut c) = os_ui::Canvas::new(1920, 1080, 1.0) else { return };
        let mut a = app();
        // Track 1 sends to track 3 as well as to the master: two cables leave it.
        select(&mut a.state, 1, false);
        assert_eq!(route(&mut a.state, 3, false, false), None);
        frame(&mut a, &mut c);
        // Strips 1 to 3 from above their send arrows to the window's bottom, where the cable hangs.
        let below = |a: &App| {
            let (l, r) = (strip(a, 1), strip(a, 3));
            os_ui::Rect::new(l.x, l.bottom() - 80.0, r.right() - l.x, 1079.0 - (l.bottom() - 80.0))
        };
        let with = pixels(&c, below(&a));
        run(Opt::Cables);
        frame(&mut a, &mut c);
        let without = pixels(&c, below(&a));
        assert_ne!(with, without, "the cable goes");
        run(Opt::Cables);
        frame(&mut a, &mut c);
        assert_ne!(pixels(&c, below(&a)), without, "and comes back");

        let edge = |a: &App| {
            let s = strip(a, 5);
            os_ui::Rect::new(s.right() - 1.0, s.y + 300.0, 1.0, 100.0)
        };
        let plain = pixels(&c, edge(&a));
        run(Opt::Lines);
        frame(&mut a, &mut c);
        let lined = pixels(&c, edge(&a));
        let light =
            |p: &[(u8, u8, u8)]| p.iter().map(|&(r, g, b)| u32::from(r) + u32::from(g) + u32::from(b)).sum::<u32>();
        assert!(light(&lined) > light(&plain), "a lighter line divides the strips");
        run(Opt::Lines);
    }

    /// Colorful mixer: a coloured track's strip body takes more of its colour at each level; Off leaves the body
    /// plain and colours only the header band. Names at top: the name moves up its column.
    #[test]
    fn colorful_mixer_and_names_at_top_paint() {
        let Ok(mut c) = os_ui::Canvas::new(1920, 1080, 1.0) else { return };
        let mut a = app();
        a.state.selected_mixer = 1;
        track_mut(&mut a.state.session, 4).unwrap().color = Some(Rgb::new(220, 30, 30));
        frame(&mut a, &mut c);
        let body = |a: &App| {
            let s = strip(a, 4);
            os_ui::Rect::new(s.x + 3.0, s.y + 600.0, 5.0, 40.0)
        };
        let red = |p: &[(u8, u8, u8)]| {
            p.iter().map(|&(r, g, _)| i32::from(r) - i32::from(g)).sum::<i32>() / p.len().max(1) as i32
        };
        let mut last = red(&pixels(&c, body(&a)));
        assert!(last < 6, "Off: the body stays plain ({last})");
        let s = strip(&a, 4);
        let header = pixels(&c, os_ui::Rect::new(s.x + 3.0, s.y + 2.0, 6.0, 6.0));
        assert!(red(&header) > 20, "Off: the header band is coloured ({})", red(&header));
        for level in 1..=3 {
            run(Opt::Colorful(level));
            frame(&mut a, &mut c);
            let now = red(&pixels(&c, body(&a)));
            assert!(now > last, "level {level}: more colour ({now} after {last})");
            last = now;
        }
        run(Opt::Colorful(0));

        // The name: where its first lit pixels are in the name column.
        let column = |a: &App| {
            let s = strip(a, 4);
            os_ui::Rect::new(s.x + 3.0, s.y + 30.0, 14.0, 330.0)
        };
        let top_ink = |c: &os_ui::Canvas, r: os_ui::Rect| {
            (r.y as u32..r.bottom() as u32).find(|&y| {
                (r.x as u32..r.right() as u32)
                    .any(|x| c.pixmap.pixel(x, y).is_some_and(|p| p.red() > 90 && p.red() > p.green() + 30))
            })
        };
        frame(&mut a, &mut c);
        let low = top_ink(&c, column(&a)).expect("the name is drawn");
        run(Opt::NamesTop);
        frame(&mut a, &mut c);
        let high = top_ink(&c, column(&a)).expect("the name is drawn");
        assert!(high + 100 < low, "the name moves to the top ({high} vs {low})");
        run(Opt::NamesTop);
    }
}
