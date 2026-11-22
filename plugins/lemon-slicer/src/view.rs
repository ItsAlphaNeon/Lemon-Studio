// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Slicer editor: painting and hit regions (platform independent), in the Lemon "Pith" plugin look on a 6 px
//! grid: the header band (the lemon mark and "SLICER", the loop's BPM and BEATS boxes, the Load / Slicing / Dump
//! buttons), the slice well (the loop, its slices, the chosen one lit), and the cards of the chosen slice, the play
//! knobs and the switches.
//!
// spec: plugins/lemon-slicer.md §5, ui/lemon-design.md §11.1

use os_theme::design::lemon as brand;
use os_theme::Palette;
use os_ui::kit::lemon::{self, plug};
use os_ui::kit::{inst, State as Kst};
use os_ui::{Canvas, Color, Font, Rect};

use crate::params::{ATTACK_ID, DECAY_ID, DECLICK_ID, PITCH_ID, TO_END_ID};
use crate::state::State;

/// Editor size in logical pixels.
pub const W: u32 = 640;
pub const H: u32 = 300;

/// Every card 6 px from the editor's edges and from its neighbours.
const GAP: f32 = 6.0;
/// A rect from its left, right, top and bottom edges (right and bottom exclusive).
const fn edges(x0: f32, x1: f32, y0: f32, y1: f32) -> Rect {
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}
const RIGHT: f32 = W as f32 - GAP;
const BOTTOM: f32 = H as f32 - GAP;

/// The header band, its centre line.
const HEAD: (f32, f32) = (GAP, 36.0);
const HEAD_Y: f32 = (HEAD.0 + HEAD.1) / 2.0;
pub const LOGO: Rect = edges(GAP, 132.0, HEAD.0, HEAD.1);
/// The loop's tempo and length boxes (their captions to their left).
pub const BPM_BOX: Rect = edges(180.0, 246.0, HEAD_Y - 9.0, HEAD_Y + 9.0);
pub const BEATS_BOX: Rect = edges(306.0, 360.0, HEAD_Y - 9.0, HEAD_Y + 9.0);
/// The header buttons.
pub const LOAD: Rect = edges(438.0, 492.0, HEAD_Y - 11.0, HEAD_Y + 11.0);
pub const SLICE: Rect = edges(498.0, 564.0, HEAD_Y - 11.0, HEAD_Y + 11.0);
pub const DUMP: Rect = edges(570.0, RIGHT, HEAD_Y - 11.0, HEAD_Y + 11.0);
/// The slice well and the picture inside it.
pub const WELL: Rect = edges(GAP, RIGHT, HEAD.1 + GAP, 186.0);
pub const WAVE: Rect = edges(GAP + 6.0, RIGHT - 6.0, HEAD.1 + GAP + 6.0, 180.0);
/// The cards under the well.
const ROW: (f32, f32) = (192.0, BOTTOM);
pub const SLICE_CARD: Rect = edges(GAP, 216.0, ROW.0, ROW.1);
pub const PLAY_CARD: Rect = edges(222.0, 432.0, ROW.0, ROW.1);
pub const SWITCH_CARD: Rect = edges(438.0, RIGHT, ROW.0, ROW.1);
/// A card's title band.
const BAND: f32 = 18.0;
/// The cards' control line and the captions' top (one line for every card).
const CTRL_Y: f32 = 244.0;
const CAPTION_Y: f32 = 270.0;
/// The slice's name field.
pub const NAME: Rect = edges(GAP + 8.0, 216.0 - 8.0, ROW.0 + BAND + 6.0, ROW.0 + BAND + 24.0);
pub const KEY_BOX: Rect = edges(48.0, 108.0, CTRL_Y - 10.0, CTRL_Y + 10.0);
const REV_X: f32 = 164.0;
const KNOB_X: [f32; 3] = [270.0, 327.0, 384.0];
const SWITCH_X: [f32; 2] = [494.0, 578.0];
const KNOB_R: f32 = 17.0;

const SMALL: Font = Font::new(10);
const LABEL: Font = Font::new(11);
const PILL: Font = Font::new(10).bold();

/// Read access to parameter values by id.
pub trait Model {
    fn norm(&self, id: &str) -> f32;
    fn text(&self, id: &str) -> String;
}

impl Model for os_plugkit::vals::Vals {
    fn norm(&self, id: &str) -> f32 {
        os_plugkit::vals::Vals::norm(self, id)
    }
    fn text(&self, id: &str) -> String {
        os_plugkit::vals::Vals::text(self, id)
    }
}

/// What a region of the editor does when clicked or dragged.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// Vertical drag / wheel; menu on right click.
    Knob(String),
    /// A switch parameter (click toggles; menu on right click).
    Switch(String),
    Bpm,
    Beats,
    Load,
    Slicing,
    Dump,
    /// The well: a click chooses the slice under it and plays it.
    Well,
    /// The chosen slice's key (a menu) and reverse switch.
    Key,
    Rev,
    Logo,
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub r: Rect,
    pub t: Target,
}

/// The pointer, the pressed control and the chosen slice.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewState {
    pub mouse: Option<(f32, f32)>,
    pub pressed: Option<Target>,
    pub chosen: usize,
}

/// What a paint shows besides the parameters.
pub struct Look<'a> {
    pub state: &'a State,
    /// The loop's length in frames (0 = no sample).
    pub frames: usize,
    /// The waveform's low and high per column of `WAVE` (empty = no sample).
    pub peaks: &'a [(f32, f32)],
    /// The dump is offered (hosted by Lemon Studio, a loop with slices).
    pub can_dump: bool,
}

struct Ui<'a> {
    c: &'a mut Canvas,
    t: &'a Palette,
    m: &'a dyn Model,
    st: &'a ViewState,
    hits: Vec<Hit>,
}

impl Ui<'_> {
    fn hit(&mut self, r: Rect, t: Target) {
        self.hits.push(Hit { r, t });
    }
    fn state(&self, r: Rect, t: &Target) -> Kst {
        let pressed = self.st.pressed.as_ref() == Some(t);
        let hover = !pressed && self.st.pressed.is_none() && self.st.mouse.is_some_and(|(x, y)| r.contains(x, y));
        Kst { hover, pressed, ..Kst::NORMAL }
    }
}

/// The name of key `k` (60 = C5).
pub fn key_name(k: i32) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{}{}", NAMES[k.rem_euclid(12) as usize], k.div_euclid(12))
}

/// The frame of the loop at x in the well.
pub fn frame_at(x: f32, frames: usize) -> usize {
    (((x - WAVE.x) / WAVE.w).clamp(0.0, 1.0) as f64 * frames as f64) as usize
}

fn x_of(frame: u32, frames: usize) -> f32 {
    (WAVE.x + f64::from(frame) as f32 / frames.max(1) as f32 * WAVE.w).round()
}

/// `s` cut to `max_w`, "…" in place of what is cut from its end (whole when it fits).
fn elide(c: &mut Canvas, f: Font, s: &str, max_w: f32) -> String {
    if c.text_width(f, s) <= max_w {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let cut = |n: usize| format!("{}…", chars[..n].iter().collect::<String>().trim_end());
    // The longest head that fits with its "…".
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if c.text_width(f, &cut(mid)) <= max_w { lo = mid } else { hi = mid - 1 }
    }
    cut(lo)
}

/// File path `p` cut to `max_w`: folders go first ("C:/Users/…/loop.wav"), so the file's name stays; a name too long
/// on its own loses its end.
fn elide_path(c: &mut Canvas, f: Font, p: &str, max_w: f32) -> String {
    if c.text_width(f, p) <= max_w {
        return p.to_string();
    }
    let Some(sep) = p.rfind(['/', '\\']) else { return elide(c, f, p, max_w) };
    let (dir, name) = (&p[..sep], &p[sep..]);
    let dir: Vec<char> = dir.chars().collect();
    let cut = |n: usize| format!("{}…{name}", dir[..n].iter().collect::<String>());
    if c.text_width(f, &cut(0)) > max_w {
        return elide(c, f, &cut(0), max_w);
    }
    let (mut lo, mut hi) = (0, dir.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if c.text_width(f, &cut(mid)) <= max_w { lo = mid } else { hi = mid - 1 }
    }
    cut(lo)
}

/// A hint cut to `max_w`: a file's path at its start ("<path>: why") loses folders first, so the file's name and the
/// reason stay; anything else loses its end.
fn fit_hint(c: &mut Canvas, f: Font, s: &str, max_w: f32) -> String {
    if c.text_width(f, s) <= max_w {
        return s.to_string();
    }
    if let Some((path, why)) = s.split_once(": ").filter(|(p, _)| p.contains(['/', '\\'])) {
        let rest = format!(": {why}");
        let room = max_w - c.text_width(f, &rest);
        if room > c.text_width(f, "…/name.wav") {
            return elide_path(c, f, path, room) + &rest;
        }
    }
    elide(c, f, s, max_w)
}

/// The well's message when the loop's file is missing: "<path> could not be read", the path cut to fit the well.
pub(crate) fn unread(c: &mut Canvas, path: &str) -> String {
    const TAIL: &str = " could not be read";
    let room = WAVE.w - 24.0 - c.text_width(LABEL, TAIL);
    elide_path(c, LABEL, path, room) + TAIL
}

/// Paints the editor. `hint` (not hosted: the host shows it otherwise) goes in the well's top-left corner.
pub fn paint(c: &mut Canvas, t: &Palette, m: &dyn Model, st: &ViewState, look: &Look, hint: &str) -> Vec<Hit> {
    let mut ui = Ui { c, t, m, st, hits: Vec::new() };
    ui.c.begin_frame(None);
    ui.c.clear(plug::BG);
    header(&mut ui, look);
    well(&mut ui, look, hint);
    slice_card(&mut ui, look);
    titled(&mut ui, PLAY_CARD, "PLAY");
    for (i, (id, cap)) in [(PITCH_ID, "PITCH"), (ATTACK_ID, "ATTACK"), (DECAY_ID, "DECAY")].into_iter().enumerate() {
        knob(&mut ui, id, KNOB_X[i], cap, id == PITCH_ID);
    }
    titled(&mut ui, SWITCH_CARD, "OPTIONS");
    for (i, (id, cap)) in [(DECLICK_ID, "DECLICK"), (TO_END_ID, "PLAY TO END")].into_iter().enumerate() {
        let target = Target::Switch(id.into());
        let on = ui.m.norm(id) > 0.5;
        switch(&mut ui, target, SWITCH_X[i], on, cap, false);
    }
    ui.hits
}

/// A card with its title centred in a band and a line under the band.
fn titled(ui: &mut Ui, r: Rect, title: &str) {
    lemon::plug_panel(ui.c, r);
    ui.c.fill_rect(Rect::new(r.x + 1.0, r.y + BAND, r.w - 2.0, 1.0), plug::PANEL_LINE);
    ui.c.text_center(SMALL.bold(), Rect::new(r.x, r.y, r.w, BAND), plug::DIM, title);
}

fn header(ui: &mut Ui, look: &Look) {
    // The emblem: the lemon mark and "SLICER" on one line.
    let f = Font::condensed(18, 0).bold();
    let s: f32 = 6.0;
    let (wl, gap) = (2.2 * s, (s * 1.3).round());
    let x = LOGO.x + 4.0;
    let lh = ui.c.line_height(f);
    lemon::lemon_mark(ui.c, x + wl / 2.0, HEAD_Y, s, brand::MAIN);
    lemon::spaced(ui.c, f, x + wl + gap, (HEAD_Y - lh / 2.0).round(), plug::WORDMARK, "SLICER", 3.0);
    ui.hit(LOGO, Target::Logo);
    let st = look.state;
    let has = look.frames > 0;
    value_box(ui, BPM_BOX, Target::Bpm, "BPM", &format!("{:.3}", st.bpm()), has);
    value_box(ui, BEATS_BOX, Target::Beats, "BEATS", &format!("{:.2}", st.beats()), has);
    button(ui, LOAD, Target::Load, "LOAD", false);
    button(ui, SLICE, Target::Slicing, "SLICING", !has);
    button(ui, DUMP, Target::Dump, "DUMP", !look.can_dump);
}

/// A value box with its caption to its left.
fn value_box(ui: &mut Ui, r: Rect, t: Target, caption: &str, text: &str, enabled: bool) {
    let w = ui.c.text_width(SMALL.bold(), caption);
    let lh = ui.c.line_height(SMALL.bold());
    ui.c.text(SMALL.bold(), (r.x - 6.0 - w).round(), (r.cy() - lh / 2.0).round(), plug::DIM, caption);
    let mut st = ui.state(r, &t);
    st.disabled = !enabled;
    lemon::field(ui.c, r, st);
    ui.c.text_center(SMALL.bold(), r, if !enabled { plug::OFF } else if st.hover { brand::TEXT } else { brand::MAIN }, text);
    ui.hit(r, t);
}

/// A header button with its name inside and a small arrow when it opens a menu.
fn button(ui: &mut Ui, r: Rect, t: Target, label: &str, disabled: bool) {
    let mut st = ui.state(r, &t);
    st.disabled = disabled;
    let ink = lemon::plug_button(ui.c, r, 6.0, st);
    let menu = t != Target::Load;
    let tr = if menu { Rect::new(r.x, r.y, r.w - 8.0, r.h) } else { r };
    ui.c.text_center(PILL, tr, ink, label);
    if menu {
        let (x, y) = (r.right() - 10.0, r.cy());
        ui.c.fill_polygon(&[(x - 3.0, y - 1.5), (x + 3.0, y - 1.5), (x, y + 2.0)], ink);
    }
    ui.hit(r, t);
}

/// The loop in its well: the waveform, the slices' lines (the chosen slice lit), their numbers along the top.
fn well(ui: &mut Ui, look: &Look, hint: &str) {
    lemon::plug_well(ui.c, WELL, 8.0);
    let st = look.state;
    if look.frames == 0 || look.peaks.is_empty() {
        let msg = if st.path.is_empty() { "No loop: LOAD opens one".to_string() } else { unread(ui.c, &st.path()) };
        ui.c.text_center(LABEL, WAVE, plug::FAINT, &msg);
    } else {
        let n = st.slices.len();
        let chosen = ui.st.chosen.min(n.saturating_sub(1));
        if let Some(s) = st.slices.get(chosen) {
            let x0 = x_of(s.start, look.frames);
            let x1 = st.slices.get(chosen + 1).map_or(WAVE.right(), |s| x_of(s.start, look.frames));
            ui.c.fill_rect(edges(x0, x1.max(x0 + 1.0), WAVE.y, WAVE.bottom()), brand::MAIN.with_alpha(36));
        }
        let mid = WAVE.cy();
        let half = WAVE.h / 2.0 - 2.0;
        // Adjacent columns: cells, so they tile at any scale (spec: ui/ui-scaling.md §4).
        for (i, &(lo, hi)) in look.peaks.iter().enumerate() {
            let x = WAVE.x + i as f32;
            let (y0, y1) = ((mid - hi.clamp(-1.0, 1.0) * half).floor(), (mid - lo.clamp(-1.0, 1.0) * half).ceil());
            ui.c.fill_cell(Rect::new(x, y0, 1.0, (y1 - y0).max(1.0)), plug::DIM.with_alpha(200));
        }
        let mut last_label = f32::NEG_INFINITY;
        for (i, s) in st.slices.iter().enumerate() {
            let x = x_of(s.start, look.frames).min(WAVE.right() - 1.0);
            let col = if i == chosen { brand::MAIN } else { plug::FAINT };
            ui.c.fill_rect(Rect::new(x, WAVE.y, 1.0, WAVE.h), col);
            let label = (i + 1).to_string();
            let w = ui.c.text_width(SMALL, &label);
            if x + 3.0 > last_label + 4.0 && x + 3.0 + w < WAVE.right() {
                ui.c.text(SMALL, x + 3.0, WAVE.y + 1.0, if i == chosen { brand::MAIN } else { plug::FAINT }, &label);
                last_label = x + 3.0 + w;
            }
        }
    }
    if !hint.is_empty() {
        let (left, right) = hint.split_once('\t').unwrap_or((hint, ""));
        let s = if right.is_empty() { left.to_string() } else { format!("{left}  ({right})") };
        let s = fit_hint(ui.c, SMALL, &s, WAVE.w - 12.0);
        let tw = ui.c.text_width(SMALL, &s).min(WAVE.w - 12.0);
        let lh = ui.c.line_height(SMALL);
        let y = WAVE.bottom() - lh - 5.0;
        ui.c.fill_rrect(Rect::new(WAVE.x + 1.0, y - 2.0, tw + 10.0, lh + 4.0), 4.0, Color::rgb(0x1a1a1a).with_alpha(170));
        ui.c.text_clip(SMALL, WAVE.x + 6.0, y, WAVE.w - 12.0, brand::TEXT, &s);
    }
    ui.hit(WAVE, Target::Well);
}

/// The chosen slice: its number and name, its key and its reverse switch.
fn slice_card(ui: &mut Ui, look: &Look) {
    let st = look.state;
    let n = st.slices.len();
    let chosen = ui.st.chosen.min(n.saturating_sub(1));
    let title = if n == 0 { "SLICE".to_string() } else { format!("SLICE {} OF {n}", chosen + 1) };
    titled(ui, SLICE_CARD, &title);
    let slice = st.slices.get(chosen);
    lemon::field(ui.c, NAME, Kst { disabled: slice.is_none(), ..Kst::NORMAL });
    let name = slice.map(|s| s.name()).filter(|s| !s.is_empty()).unwrap_or_else(|| if slice.is_some() { "(no name)".into() } else { String::new() });
    // A long name ends in "…" inside the field.
    let name = elide(ui.c, LABEL, &name, NAME.w - 12.0);
    ui.c.text_center(LABEL, NAME,if slice.is_some_and(|s| !s.name.is_empty()) { brand::TEXT } else { plug::FAINT }, &name);
    // The key box: the key it plays on, lemon when set apart from its own.
    let mut kst = ui.state(KEY_BOX, &Target::Key);
    kst.disabled = slice.is_none();
    lemon::field(ui.c, KEY_BOX, kst);
    if let Some(s) = slice {
        let col = if s.root >= 0 { brand::MAIN } else if kst.hover { brand::TEXT } else { plug::DIM };
        ui.c.text_center(SMALL.bold(), KEY_BOX, col, &key_name(st.key_of(chosen)));
    }
    inst::caption(ui.c, ui.t, KEY_BOX.cx(), CAPTION_Y, "KEY", slice.is_none());
    ui.hit(KEY_BOX, Target::Key);
    switch(ui, Target::Rev, REV_X, slice.is_some_and(|s| s.reversed()), "REVERSE", slice.is_none());
}

/// A knob (hit area 40 px across), caption under it.
fn knob(ui: &mut Ui, id: &str, cx: f32, caption: &str, bipolar: bool) {
    let target = Target::Knob(id.into());
    let hr = Rect::new(cx - KNOB_R - 3.0, CTRL_Y - KNOB_R - 3.0, 2.0 * KNOB_R + 6.0, 2.0 * KNOB_R + 6.0);
    let st = ui.state(hr, &target);
    lemon::dial(ui.c, cx, CTRL_Y, KNOB_R, 2.8, ui.m.norm(id), bipolar, brand::MAIN, st);
    inst::caption(ui.c, ui.t, cx, CAPTION_Y, caption, false);
    ui.hit(hr, target);
}

/// A switch: a raised button lit lemon when on ("ON" / "OFF" inside), the caption under it.
fn switch(ui: &mut Ui, target: Target, cx: f32, on: bool, caption: &str, disabled: bool) {
    let r = Rect::new(cx - 20.0, CTRL_Y - 10.0, 40.0, 20.0);
    let hr = Rect::new(cx - 26.0, CTRL_Y - 14.0, 52.0, 28.0);
    let mut st = ui.state(hr, &target);
    st.on = on;
    st.disabled = disabled;
    let ink = lemon::plug_button(ui.c, r, 6.0, st);
    ui.c.text_center(PILL, r, ink, if on { "ON" } else { "OFF" });
    inst::caption(ui.c, ui.t, cx, CAPTION_Y, caption, disabled);
    ui.hit(hr, target);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every card and button starts on the 6 px grid, inside the editor, 6 px from its neighbours.
    #[test]
    fn the_layout_keeps_its_grid() {
        for r in [LOGO, WELL, SLICE_CARD, PLAY_CARD, SWITCH_CARD, LOAD, SLICE, DUMP] {
            assert!(r.x >= GAP && r.right() <= RIGHT && r.y >= GAP && r.bottom() <= BOTTOM, "{r:?}");
            assert_eq!(r.x % 6.0, 0.0, "{r:?}");
        }
        for (a, b) in [(SLICE_CARD, PLAY_CARD), (PLAY_CARD, SWITCH_CARD), (LOAD, SLICE), (SLICE, DUMP)] {
            assert_eq!(b.x - a.right(), GAP, "{a:?} {b:?}");
        }
        assert_eq!(SLICE_CARD.y - WELL.bottom(), GAP);
        assert_eq!(WELL.y - HEAD.1, GAP);
        assert_eq!((key_name(60), key_name(36), key_name(61)), ("C5".into(), "C3".into(), "C#5".into()));
    }

    struct Flat;
    impl Model for Flat {
        fn norm(&self, _: &str) -> f32 {
            0.0
        }
        fn text(&self, _: &str) -> String {
            String::new()
        }
    }

    /// The editor at `scale` showing `st` (a loop `frames` long, drawn from `peaks`): its width and device pixels.
    fn pixels(st: &State, frames: usize, peaks: &[(f32, f32)], scale: f32) -> (usize, Vec<u32>) {
        let (w, h) = ((W as f32 * scale).round() as u32, (H as f32 * scale).round() as u32);
        let mut c = Canvas::new(w, h, scale).unwrap();
        let look = Look { state: st, frames, peaks, can_dump: false };
        paint(&mut c, &os_plugkit::palette(), &Flat, &ViewState::default(), &look, "");
        let mut px = vec![0; (w * h) as usize];
        c.copy_to_xrgb(&mut px);
        (w as usize, px)
    }

    /// Two paints differ only inside `r` (logical px, give or take one).
    fn same_outside(a: &(usize, Vec<u32>), b: &(usize, Vec<u32>), r: Rect, k: f32) -> bool {
        let r = r.outset(1.0);
        a.1.iter().zip(&b.1).enumerate().all(|(i, (p, q))| p == q || r.contains((i % a.0) as f32 / k, (i / a.0) as f32 / k))
    }

    const LONG_PATH: &str = "C:/Users/Someone/Documents/Image-Line/Data/Patches/Packs/Loops/A folder with a long name/Breakbeat loop at 174 BPM (master).wav";

    /// A missing loop's long path stays inside the well, its folders cut and its file's name kept.
    #[test]
    fn a_missing_files_message_stays_in_the_well() {
        let mut st = State::default();
        st.set_path(LONG_PATH);
        assert!(same_outside(&pixels(&st, 0, &[], 1.0), &pixels(&State::default(), 0, &[], 1.0), WELL, 1.0));
        let mut c = Canvas::new(W, H, 1.0).unwrap();
        let msg = unread(&mut c, LONG_PATH);
        assert!(c.text_width(LABEL, &msg) <= WAVE.w - 24.0, "{msg}");
        assert!(msg.starts_with("C:/Users/") && msg.ends_with("…/Breakbeat loop at 174 BPM (master).wav could not be read"), "{msg}");
    }

    /// A long slice name ends in "…" inside its field.
    #[test]
    fn a_long_slice_name_stays_in_its_field() {
        let named = |name: &str| {
            let mut st = State::default();
            st.slices = vec![crate::state::Slice { name: name.as_bytes().to_vec(), ..crate::state::Slice::at(0) }, crate::state::Slice::at(100)];
            pixels(&st, 0, &[], 1.0)
        };
        let long = format!("Slice 1 {}", "with a name far longer than the field can hold ".repeat(2));
        assert!(same_outside(&named(&long), &named("x"), NAME, 1.0));
        let mut c = Canvas::new(W, H, 1.0).unwrap();
        let shown = elide(&mut c, LABEL, &long, NAME.w - 12.0);
        assert!(shown.starts_with("Slice 1 with") && shown.ends_with('…') && c.text_width(LABEL, &shown) <= NAME.w - 12.0, "{shown}");
    }

    /// A bad file's hint fits the well: the path loses folders, the file's name and the reason stay.
    #[test]
    fn a_bad_files_hint_fits_the_well() {
        let mut c = Canvas::new(W, H, 1.0).unwrap();
        let why = ": unsupported feature: core (probe): no suitable format reader found";
        let s = fit_hint(&mut c, SMALL, &format!("{LONG_PATH}{why}"), WAVE.w - 12.0);
        assert!(c.text_width(SMALL, &s) <= WAVE.w - 12.0, "{s}");
        assert!(s.starts_with("C:/Users/") && s.ends_with(&format!("…/Breakbeat loop at 174 BPM (master).wav{why}")), "{s}");
        // Other hints are whole when they fit.
        assert_eq!(fit_hint(&mut c, SMALL, "Slice 2  (C#5)", WAVE.w - 12.0), "Slice 2  (C#5)");
    }

    /// At 150 % the waveform's columns tile: a full-scale loop paints its middle row in one colour, no stripes.
    #[test]
    fn the_waveform_has_no_stripes_at_150_percent() {
        let k = 1.5;
        let peaks = vec![(-1.0, 1.0); WAVE.w as usize];
        let (w, px) = pixels(&State::default(), 1000, &peaks, k);
        let y = (WAVE.cy() * k) as usize;
        let row = &px[y * w + (WAVE.x * k).ceil() as usize..y * w + (WAVE.right() * k).floor() as usize];
        assert!(row.iter().all(|&p| p == row[0]), "{:x?}", row.iter().position(|&p| p != row[0]));
    }
}
