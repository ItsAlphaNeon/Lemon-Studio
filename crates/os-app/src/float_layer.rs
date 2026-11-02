// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Shift+F and the status bar's button: the floating windows (`crate::floating`) and the plugin windows of their own
//! slide away together and come back together, as a scratchpad does on Hyprland. Moved windows keep their places: the
//! slide is added while they paint (`lift`), and those down leave `State::z`, so they are neither painted nor clicked.
//!
//! Timed as Omarchy's Hyprland `specialWorkspace` animation (`hypr/looknfeel.lua`): speed 3 (300 ms), easeOutQuint,
//! style slidevert: down one plane's height, and back up from there. With Animations off they go at once.
//!
//! A floating window that opens while the layer is down brings it back, and so does the system's close shortcut. The
//! mixer or channel rack detached into a window of its own stays where it is.

use std::time::Instant;

use crate::app::{State, Win};
use crate::floating;

/// How long a slide takes (seconds).
pub const SLIDE: f32 = 0.3;

#[derive(Debug, Clone, Copy)]
struct Slide {
    from: f32,
    start: Instant,
}

/// Where the layer is.
#[derive(Debug, Default)]
pub struct FloatLayer {
    /// Shift+F or the button was pressed; the next tick acts on it.
    pub pressed: bool,
    down: bool,
    /// The slide under way.
    slide: Option<Slide>,
    /// How far down it is: 0 in place, 1 a plane's height below.
    at: f32,
    /// While down: `State::z` as it was when the windows left it.
    kept: Option<Vec<Win>>,
    /// While down: the plugin windows of their own that went with it.
    native: Vec<usize>,
}

impl FloatLayer {
    pub fn is_down(&self) -> bool {
        self.down
    }

    pub fn sliding(&self) -> bool {
        self.slide.is_some()
    }

    /// How far down it is: 0 in place, 1 a plane's height below.
    pub fn at(&self) -> f32 {
        self.at
    }

    /// Plugin window `row` is down with the layer and stays out of `State::z`.
    pub fn keeps(&self, row: usize) -> bool {
        self.kept.as_ref().is_some_and(|k| k.contains(&Win::Hosted(row)))
    }
}

/// Floating window `w` is in the layer: it floats in the app's own window.
fn member(s: &State, w: Win) -> bool {
    floating::floats(w) && !s.is_detached(w)
}

/// Whether anything is there to slide away.
pub fn has_windows(s: &State, native: &[usize]) -> bool {
    !native.is_empty() || s.z.iter().any(|&w| member(s, w))
}

/// The stack with the layer's windows in it, as a saved session wants it.
pub fn whole_z(s: &State) -> Vec<Win> {
    let Some(kept) = &s.float_layer.kept else { return s.z.clone() };
    // Back where they were, those that went on existing; one brought forward meanwhile stays in front.
    let raised = |w: Win| member(s, w) && s.z.contains(&w);
    let mut out: Vec<Win> = kept.iter().copied().filter(|&w| if member(s, w) { !raised(w) } else { s.z.contains(&w) }).collect();
    out.extend(s.z.iter().copied().filter(|w| !kept.contains(w) || raised(*w)));
    out
}

/// Starts toward `down`, from wherever it is; `motion` off goes at once. Coming up puts the windows back in the stack
/// first: they show the whole way.
pub fn go(s: &mut State, down: bool, native: Vec<usize>, motion: bool, now: Instant) {
    if down {
        s.float_layer.native = native;
    } else {
        s.z = whole_z(s);
        s.float_layer.kept = None;
        s.float_layer.native.clear();
    }
    s.float_layer.down = down;
    let goal = if down { 1.0 } else { 0.0 };
    if motion && s.float_layer.at != goal {
        s.float_layer.slide = Some(Slide { from: s.float_layer.at, start: now });
    } else {
        s.float_layer.at = goal;
        s.float_layer.slide = None;
        settle(s);
    }
}

/// Moves the slide on to `now`. Returns whether it is under way or has just ended.
pub fn step(s: &mut State, now: Instant) -> bool {
    let Some(slide) = s.float_layer.slide else { return false };
    let goal = if s.float_layer.down { 1.0 } else { 0.0 };
    let t = now.duration_since(slide.start).as_secs_f32() / SLIDE;
    if t >= 1.0 {
        s.float_layer.at = goal;
        s.float_layer.slide = None;
        settle(s);
    } else {
        s.float_layer.at = slide.from + (goal - slide.from) * crate::tiles::ease(t);
    }
    true
}

/// At the bottom the windows leave the stack, and the keyboard with them.
fn settle(s: &mut State) {
    if s.float_layer.down && s.float_layer.kept.is_none() {
        s.float_layer.kept = Some(s.z.clone());
        let z: Vec<Win> = s.z.iter().copied().filter(|&w| !member(s, w)).collect();
        s.z = z;
    }
}

/// A window opened or brought forward while the layer is down: it comes back for it.
pub fn wanted_back(s: &State, native: &[usize]) -> bool {
    let l = &s.float_layer;
    l.down && l.slide.is_none() && (s.z.iter().any(|&w| member(s, w)) || native.iter().any(|r| !l.native.contains(r)))
}

/// Puts the layer's windows where the slide shows them, for a paint; `unlift` puts them back.
pub fn lift(s: &mut State) -> Vec<(Win, (f32, f32))> {
    if s.float_layer.at == 0.0 {
        return Vec::new();
    }
    // Whole device px, as Hyprland's windows land: a window between two would blur.
    let k = s.ui_scale.max(0.01);
    let dy = (s.float_layer.at * s.screen.1 * k).round() / k;
    let mut moved = Vec::new();
    for w in s.z.clone() {
        if !member(s, w) {
            continue;
        }
        if let Some(p) = floating::placed(w) {
            let at = (p.pos)(s);
            (p.set_pos)(s, (at.0, at.1 + dy));
            moved.push((w, at));
        }
        #[cfg(target_os = "linux")]
        if let Win::Hosted(row) = w
            && let Some(e) = s.embeds.get(&row)
        {
            let r = e.rect();
            e.set_pos((r.x, r.y + dy));
            moved.push((w, (r.x, r.y)));
        }
    }
    moved
}

/// After the paint: the windows back at their own places.
pub fn unlift(s: &mut State, moved: Vec<(Win, (f32, f32))>) {
    for (w, at) in moved {
        if let Some(p) = floating::placed(w) {
            (p.set_pos)(s, at);
        }
        #[cfg(target_os = "linux")]
        if let Win::Hosted(row) = w
            && let Some(e) = s.embeds.get(&row)
        {
            e.set_pos(at);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;

    fn app() -> App {
        let mut a = App::new(None);
        a.state.z = vec![Win::Playlist, Win::ChannelRack, Win::Mixer, Win::Tapper];
        a
    }

    /// The timing is Omarchy's `specialWorkspace`: 300 ms, easeOutQuint from the top down and from the bottom up.
    #[test]
    fn the_slide_keeps_hyprlands_time_and_curve() {
        assert_eq!(SLIDE, 0.3);
        let mut a = app();
        let t0 = Instant::now();
        go(&mut a.state, true, Vec::new(), true, t0);
        assert_eq!(a.state.float_layer.at(), 0.0, "starts in place");
        step(&mut a.state, t0 + std::time::Duration::from_millis(150));
        assert!((a.state.float_layer.at() - crate::tiles::ease(0.5)).abs() < 1e-5);
        assert!(a.state.z.contains(&Win::ChannelRack), "still shown on the way");
        assert!(a.state.float_layer.sliding());
        step(&mut a.state, t0 + std::time::Duration::from_millis(300));
        assert_eq!(a.state.float_layer.at(), 1.0);
        assert!(!a.state.float_layer.sliding());
        // back up the same way
        go(&mut a.state, false, Vec::new(), true, t0);
        assert!(a.state.z.contains(&Win::ChannelRack), "shown the whole way up");
        step(&mut a.state, t0 + std::time::Duration::from_millis(150));
        assert!((a.state.float_layer.at() - (1.0 - crate::tiles::ease(0.5))).abs() < 1e-5);
    }

    /// Floating windows leave the stack together and come back in their order, the split windows staying.
    #[test]
    fn the_stack_comes_back_as_it_was() {
        let mut a = app();
        go(&mut a.state, true, Vec::new(), false, Instant::now());
        assert_eq!(a.state.z, [Win::Playlist, Win::Mixer]);
        assert!(a.state.float_layer.is_down());
        assert_eq!(whole_z(&a.state), [Win::Playlist, Win::ChannelRack, Win::Mixer, Win::Tapper], "a saved session keeps them");
        go(&mut a.state, false, Vec::new(), false, Instant::now());
        assert_eq!(a.state.z, [Win::Playlist, Win::ChannelRack, Win::Mixer, Win::Tapper]);
    }

    /// A floating window opened while down brings the layer back, itself in front.
    #[test]
    fn opening_a_window_brings_the_layer_back() {
        let mut a = app();
        go(&mut a.state, true, vec![7], false, Instant::now());
        assert!(!wanted_back(&a.state, &[7]));
        assert!(wanted_back(&a.state, &[7, 8]), "another plugin's own window");
        a.state.raise(Win::ChannelRack);
        assert!(wanted_back(&a.state, &[7]));
        go(&mut a.state, false, Vec::new(), false, Instant::now());
        assert_eq!(a.state.z, [Win::Playlist, Win::Mixer, Win::Tapper, Win::ChannelRack]);
    }

    /// A slide moves what paints and leaves every window's own place alone.
    #[test]
    fn lift_moves_windows_only_for_the_paint() {
        let mut a = app();
        a.state.screen = (1600.0, 900.0);
        a.state.ui_scale = 1.0;
        let before = crate::views::rack::rect(&a.state);
        go(&mut a.state, true, Vec::new(), true, Instant::now());
        a.state.float_layer.at = 0.5;
        let moved = lift(&mut a.state);
        assert_eq!(crate::views::rack::rect(&a.state).y, before.y + 450.0);
        unlift(&mut a.state, moved);
        assert_eq!(crate::views::rack::rect(&a.state), before);
    }

    /// Shift+F, once for a held key; F alone stays the tiling layout's.
    #[test]
    fn shift_f_presses_the_layer() {
        use winit::keyboard::Key;
        let mut a = app();
        a.shift = true;
        a.key(&Key::Character("F".into()), false, false);
        assert!(a.state.float_layer.pressed);
        a.state.float_layer.pressed = false;
        a.repeat = true;
        a.key(&Key::Character("F".into()), false, false);
        assert!(!a.state.float_layer.pressed, "a held key does it once");
        a.repeat = false;
        a.shift = false;
        a.key(&Key::Character("f".into()), false, false);
        assert!(!a.state.float_layer.pressed, "not without Shift");
    }
}
