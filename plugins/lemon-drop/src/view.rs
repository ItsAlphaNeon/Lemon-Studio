// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The editor's layout and its controls: the display on top, then the input, appearance and colour cards, then the
//! mode's settings over the mode tabs, in the Lemon "Pith" plugin look on a 6 px grid. Every control's range, default,
//! reading of the state and hint is here.
//!
// spec: plugins/lemon-drop.md "Editor", ui/lemon-design.md §11.1

use os_theme::design::lemon as brand;
use os_ui::kit::State as Look;
use os_ui::kit::lemon::{self, plug};
use os_ui::{Canvas, Color, Font, Rect};

use crate::state::*;

/// Editor size in logical pixels.
pub const W: u32 = 640;
pub const H: u32 = 482;

const GAP: f32 = 6.0;
const fn edges(x0: f32, x1: f32, y0: f32, y1: f32) -> Rect {
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}
const RIGHT: f32 = W as f32 - GAP;
const BOTTOM: f32 = H as f32 - GAP;

/// The display (its box and the mirror image under it).
pub const DISPLAY: Rect = edges(GAP, RIGHT, GAP, 252.0);
const CARDS: (f32, f32) = (258.0, 404.0);
pub const INPUT_CARD: Rect = edges(GAP, 120.0, CARDS.0, CARDS.1);
pub const LOOK_CARD: Rect = edges(126.0, 384.0, CARDS.0, CARDS.1);
pub const COLOR_CARD: Rect = edges(390.0, RIGHT, CARDS.0, CARDS.1);
pub const MODE_CARD: Rect = edges(GAP, RIGHT, 410.0, BOTTOM);
const BAND: f32 = 15.0;
const KNOB_R: f32 = 15.0;
const LABEL: Font = Font::new(10);
const SMALL: Font = Font::new(9);

/// Every control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ctl {
    Gain,
    Fade,
    Hold,
    Title,
    Visible,
    Mini,
    Background,
    Solid,
    Front,
    Desktop,
    Bevel,
    Glow,
    Corners,
    Punches,
    Target,
    Hue,
    Light,
    Sat,
    Object,
    Opacity,
    Tab(Mode),
    Auto,
    ScopeLength,
    ScopeRepeat,
    Separation,
    Pinch,
    Interpolate,
    SpecLength,
    SpecRes,
    SpecCurve,
    SpecRange,
    Natural,
    Enhanced,
    MeterScale,
    MeterHold,
    MeterMode,
    MeterDecay,
    VecLength,
    VecThick,
    VecMode,
    Additive,
    Display,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// A knob over min ..= max (a wheel when `wrap`), with its default and whether it is drawn from its centre.
    Knob {
        min: i32,
        max: i32,
        def: i32,
        bipolar: bool,
        wrap: bool,
    },
    /// The vertical gain fader.
    Fader {
        min: i32,
        max: i32,
        def: i32,
    },
    Toggle,
    /// A list box (opens a menu).
    List,
    /// The title's text box.
    Text,
    Button,
    Tab,
    Area,
}

pub fn kind(c: Ctl) -> Kind {
    let k = |min, max, def| Kind::Knob { min, max, def, bipolar: false, wrap: false };
    let b = |min, max, def| Kind::Knob { min, max, def, bipolar: true, wrap: false };
    match c {
        Ctl::Gain => Kind::Fader { min: -120, max: 120, def: 0 },
        Ctl::Fade => k(0, NEVER, NEVER),
        Ctl::Bevel => b(-256, 256, 64),
        Ctl::Corners => k(0, 512, 256),
        Ctl::Punches => b(-24, 24, 0),
        Ctl::Hue => Kind::Knob { min: 0, max: 255, def: 0, bipolar: false, wrap: true },
        Ctl::Light | Ctl::Sat | Ctl::Object | Ctl::Opacity => k(0, 256, 256),
        Ctl::ScopeLength | Ctl::VecLength => k(500, 5000, 1000),
        Ctl::ScopeRepeat => k(1000, 10000, 2000),
        Ctl::Separation => k(-1, 1000, -1),
        Ctl::SpecLength => k(500, 10000, 1500),
        Ctl::SpecCurve => k(0, 256, 0),
        Ctl::SpecRange => k(48, 144, 96),
        Ctl::MeterScale => k(0, 256, 128),
        Ctl::MeterHold => k(0, NEVER, 2500),
        Ctl::MeterDecay => k(1, 5000, 750),
        Ctl::VecThick => k(100, 600, 350),
        Ctl::Title => Kind::Text,
        Ctl::Target | Ctl::SpecRes | Ctl::MeterMode | Ctl::VecMode => Kind::List,
        Ctl::Hold => Kind::Button,
        Ctl::Tab(_) => Kind::Tab,
        Ctl::Display => Kind::Area,
        _ => Kind::Toggle,
    }
}

/// A knob's or fader's stored value; `target` is the colour target the colour knobs edit.
pub fn value(st: &State, c: Ctl, target: usize) -> i32 {
    let p = st.paints[target.min(TARGETS - 1)];
    match c {
        Ctl::Gain => st.gain,
        Ctl::Fade => st.fade,
        Ctl::Bevel => st.bevel,
        Ctl::Corners => st.corners,
        Ctl::Punches => st.punches,
        Ctl::Hue => p.hue,
        Ctl::Light => p.light,
        Ctl::Sat => p.sat,
        Ctl::Object => p.object,
        Ctl::Opacity => p.color,
        Ctl::ScopeLength => st.scope_length,
        Ctl::ScopeRepeat => st.scope_repeat,
        Ctl::Separation => st.separation,
        Ctl::SpecLength => st.spectrum_length,
        Ctl::SpecCurve => st.spectrum_curve,
        Ctl::SpecRange => st.spectrum_range,
        Ctl::MeterScale => st.meter_scale,
        Ctl::MeterHold => st.meter_hold,
        Ctl::MeterDecay => st.meter_decay,
        Ctl::VecLength => st.vector_length,
        Ctl::VecThick => st.vector_thickness,
        _ => 0,
    }
}

pub fn set_value(st: &mut State, c: Ctl, target: usize, v: i32) {
    let t = target.min(TARGETS - 1);
    let mut p = st.paints[t];
    match c {
        Ctl::Gain => st.gain = v,
        Ctl::Fade => st.fade = v,
        Ctl::Bevel => st.bevel = v,
        Ctl::Corners => st.corners = v,
        Ctl::Punches => st.punches = v,
        Ctl::Hue => p.hue = v.rem_euclid(256),
        Ctl::Light => p.light = v,
        Ctl::Sat => p.sat = v,
        Ctl::Object => p.object = v,
        Ctl::Opacity => p.color = v,
        Ctl::ScopeLength => st.scope_length = v,
        Ctl::ScopeRepeat => st.scope_repeat = v,
        Ctl::Separation => st.separation = v,
        Ctl::SpecLength => st.spectrum_length = v,
        Ctl::SpecCurve => st.spectrum_curve = v,
        Ctl::SpecRange => st.spectrum_range = v,
        Ctl::MeterScale => st.meter_scale = v,
        Ctl::MeterHold => st.meter_hold = v,
        Ctl::MeterDecay => st.meter_decay = v,
        Ctl::VecLength => st.vector_length = v,
        Ctl::VecThick => st.vector_thickness = v,
        _ => return,
    }
    if matches!(c, Ctl::Hue | Ctl::Light | Ctl::Sat | Ctl::Object | Ctl::Opacity) {
        st.set_paint(t, p);
    }
}

pub fn toggled(st: &State, c: Ctl) -> bool {
    match c {
        Ctl::Visible => st.visible,
        Ctl::Mini => st.mini,
        Ctl::Background => st.background,
        Ctl::Solid => st.solid,
        Ctl::Front => st.keep_in_front,
        Ctl::Desktop => st.on_desktop,
        Ctl::Glow => st.glow,
        Ctl::Auto => st.auto_mode,
        Ctl::Pinch => st.pinch,
        Ctl::Interpolate => st.interpolate,
        Ctl::Natural => st.natural,
        Ctl::Enhanced => st.enhanced,
        Ctl::Additive => st.additive,
        Ctl::Tab(m) => st.mode == m,
        _ => false,
    }
}

pub fn toggle(st: &mut State, c: Ctl) {
    let f = match c {
        Ctl::Visible => &mut st.visible,
        Ctl::Mini => &mut st.mini,
        Ctl::Background => &mut st.background,
        Ctl::Solid => &mut st.solid,
        Ctl::Front => &mut st.keep_in_front,
        Ctl::Desktop => {
            // The original keeps "on desktop" in two places.
            st.desktop_flag = !st.on_desktop;
            &mut st.on_desktop
        }
        Ctl::Glow => &mut st.glow,
        Ctl::Auto => &mut st.auto_mode,
        Ctl::Pinch => &mut st.pinch,
        Ctl::Interpolate => &mut st.interpolate,
        Ctl::Natural => &mut st.natural,
        Ctl::Enhanced => &mut st.enhanced,
        Ctl::Additive => &mut st.additive,
        Ctl::Tab(m) => {
            st.mode = m;
            return;
        }
        _ => return,
    };
    *f = !*f;
}

fn ms(v: i32) -> String {
    format!("{:.2} ms", v as f32 / 100.0)
}

fn pct(v: i32, full: i32) -> String {
    format!("{}%", (v as f32 / full as f32 * 100.0).round() as i32)
}

/// What a control's hint says.
pub fn hint(st: &State, c: Ctl, target: usize) -> String {
    let v = value(st, c, target);
    match c {
        Ctl::Gain => format!("Display gain: {:+.1} dB", v as f32 / 10.0),
        Ctl::Fade => {
            if v >= NEVER {
                "Fade out after silence: never".into()
            } else {
                format!("Fade out after silence: {v} ms")
            }
        }
        Ctl::Hold => "Hold: freeze the display (or middle-click it)".into(),
        Ctl::Title => format!("Title: {}", st.title_text()),
        Ctl::Visible => "Show the display".into(),
        Ctl::Mini => "Mini view (kept for the original's floating display)".into(),
        Ctl::Background => "Draw the display's background and frame".into(),
        Ctl::Solid => "Solid (kept for the original's floating display)".into(),
        Ctl::Front => "Keep in front (kept for the original's floating display)".into(),
        Ctl::Desktop => "On desktop (kept for the original's floating display)".into(),
        Ctl::Bevel => {
            if v >= 0 {
                format!("Bevel / reflection: {} reflection", pct(v, 256))
            } else {
                format!("Bevel / reflection: {} bevel", pct(-v, 256))
            }
        }
        Ctl::Glow => "Glowing border".into(),
        Ctl::Corners => format!("Rounded corners: {:.0} px", v as f32 / 512.0 * 12.0),
        Ctl::Punches => match v {
            0 => "Punches: none".into(),
            v if v < 0 => format!("Punches: round, {} px", -v),
            v => format!("Punches: square, {v} px"),
        },
        Ctl::Target => format!("Colour of: {}", TARGET_NAMES[target.min(TARGETS - 1)]),
        Ctl::Hue => format!("Hue: {v}"),
        Ctl::Light => format!("Lightness: {v}"),
        Ctl::Sat => format!("Saturation: {v}"),
        Ctl::Object => format!("Object opacity: {v}"),
        Ctl::Opacity => format!("Colour opacity: {v}"),
        Ctl::Tab(m) => format!("{} settings", m.name()),
        Ctl::Auto => "Auto visual mode: the meter when the display is tall, the oscilloscope when it is wide".into(),
        Ctl::ScopeLength | Ctl::SpecLength | Ctl::VecLength => {
            format!("Update (block length): {}", ms(v))
        }
        Ctl::ScopeRepeat => format!("Window (block repeat): {}, {:.1} ms shown", ms(v), crate::analysis::scope_span(st) * 1000.0),
        Ctl::Separation => format!("Stereo separation: {}", v.max(0) / 10),
        Ctl::Pinch => "Pinch the trace's ends".into(),
        Ctl::Interpolate => "Interpolate between samples".into(),
        Ctl::SpecRes => format!("Maximum resolution: {} bands", crate::analysis::bands(st.spectrum_res)),
        Ctl::SpecCurve => format!("Frequency curve: {}", pct(v, 256)),
        Ctl::SpecRange => format!("dB range: {v} dB"),
        Ctl::Natural => "Natural weighting: +3 dB per octave, so pink noise draws level".into(),
        Ctl::Enhanced => "Enhanced frequency".into(),
        Ctl::MeterScale => format!("Scale: {}", pct(v, 256)),
        Ctl::MeterHold => match v {
            0 => "Peak hold: none".into(),
            v if v >= NEVER => "Peak hold: forever".into(),
            v => format!("Peak hold: {v} ms"),
        },
        Ctl::MeterMode => format!("Meter: {}", METER_MODES[st.meter_mode_index()].1),
        Ctl::MeterDecay => format!("Decay speed: {}", pct(v, 5000)),
        Ctl::VecThick => format!("Thickness: {:.2}", v as f32 / 100.0),
        Ctl::VecMode => format!("Drawing: {}", VECTOR_MODES[st.vector_mode.clamp(0, 3) as usize]),
        Ctl::Additive => "Add: additive drawing".into(),
        Ctl::Display => String::new(),
    }
}

/// A control's caption.
fn caption(c: Ctl) -> &'static str {
    match c {
        Ctl::Gain => "Gain",
        Ctl::Fade => "Fade out",
        Ctl::Hold => "HOLD",
        Ctl::Visible => "Visible",
        Ctl::Mini => "Mini view",
        Ctl::Background => "Background",
        Ctl::Solid => "Solid",
        Ctl::Front => "Keep in front",
        Ctl::Desktop => "On desktop",
        Ctl::Bevel => "Reflect / bevel",
        Ctl::Glow => "Glow",
        Ctl::Corners => "Corners",
        Ctl::Punches => "Punches",
        Ctl::Hue => "Hue",
        Ctl::Light => "Lightness",
        Ctl::Sat => "Saturation",
        Ctl::Object => "Object",
        Ctl::Opacity => "Color",
        Ctl::Auto => "Auto visual mode",
        Ctl::ScopeLength | Ctl::SpecLength | Ctl::VecLength => "Update",
        Ctl::ScopeRepeat => "Window",
        Ctl::Separation => "Stereo sep.",
        Ctl::Pinch => "Pinch",
        Ctl::Interpolate => "Interpolate",
        Ctl::SpecRes => "Max res",
        Ctl::SpecCurve => "Scale",
        Ctl::SpecRange => "dB range",
        Ctl::Natural => "Natural weighting",
        Ctl::Enhanced => "Enhanced frequency",
        Ctl::MeterScale => "Scale",
        Ctl::MeterHold => "Hold time",
        Ctl::MeterMode => "Mode",
        Ctl::MeterDecay => "Decay",
        Ctl::VecThick => "Thickness",
        Ctl::VecMode => "Mode",
        Ctl::Additive => "Add",
        _ => "",
    }
}

/// The controls on show with their hit rects (a knob's rect is its dial, a toggle's covers its label).
pub fn layout(st: &State) -> Vec<(Ctl, Rect)> {
    let knob = |cx: f32, cy: f32| Rect::new(cx - KNOB_R, cy - KNOB_R, 2.0 * KNOB_R, 2.0 * KNOB_R);
    let toggle = |x: f32, y: f32, w: f32| Rect::new(x - 7.0, y - 7.0, w, 14.0);
    let mut v = vec![(Ctl::Display, DISPLAY)];
    // Input.
    let ic = INPUT_CARD;
    v.push((Ctl::Gain, Rect::new(ic.x + 14.0, ic.y + BAND + 8.0, 22.0, 84.0)));
    v.push((Ctl::Fade, knob(ic.x + 80.0, ic.y + BAND + 34.0)));
    v.push((Ctl::Hold, Rect::new(ic.x + 52.0, ic.bottom() - 34.0, 56.0, 22.0)));
    // Appearance.
    let lc = LOOK_CARD;
    v.push((Ctl::Title, Rect::new(lc.x + 8.0, lc.y + BAND + 6.0, lc.w - 16.0, 18.0)));
    let tx = lc.x + 16.0;
    for (i, c) in [Ctl::Visible, Ctl::Mini, Ctl::Background, Ctl::Solid, Ctl::Front, Ctl::Desktop].into_iter().enumerate() {
        v.push((c, toggle(tx, lc.y + BAND + 38.0 + 16.0 * i as f32, 104.0)));
    }
    let kx = lc.x + 216.0;
    v.push((Ctl::Bevel, knob(kx, lc.y + BAND + 46.0)));
    v.push((Ctl::Glow, toggle(lc.x + 130.0, lc.y + BAND + 46.0, 44.0)));
    v.push((Ctl::Corners, knob(lc.x + 152.0, lc.y + BAND + 98.0)));
    v.push((Ctl::Punches, knob(kx, lc.y + BAND + 98.0)));
    // Colours.
    let cc = COLOR_CARD;
    v.push((Ctl::Target, Rect::new(cc.x + 8.0, cc.y + BAND + 6.0, cc.w - 42.0, 18.0)));
    v.push((Ctl::Hue, Rect::new(cc.x + 18.0, cc.y + BAND + 40.0, 44.0, 44.0)));
    v.push((Ctl::Light, knob(cc.x + 104.0, cc.y + BAND + 48.0)));
    v.push((Ctl::Sat, knob(cc.x + 170.0, cc.y + BAND + 48.0)));
    v.push((Ctl::Object, knob(cc.x + 104.0, cc.y + BAND + 102.0)));
    v.push((Ctl::Opacity, knob(cc.x + 170.0, cc.y + BAND + 102.0)));
    // Mode: the tabs along the bottom, the settings over them.
    let mc = MODE_CARD;
    for (i, m) in Mode::ALL.into_iter().enumerate() {
        v.push((Ctl::Tab(m), Rect::new(mc.x + 6.0 + 96.0 * i as f32, mc.bottom() - 24.0, 92.0, 20.0)));
    }
    v.push((Ctl::Auto, toggle(mc.right() - 130.0, mc.bottom() - 14.0, 124.0)));
    let y = mc.y + 22.0;
    let at = |i: f32| mc.x + 70.0 + 96.0 * i;
    let list = |x: f32| Rect::new(x - 34.0, y - 9.0, 96.0, 18.0);
    match st.mode {
        Mode::Oscilloscope => {
            v.push((Ctl::ScopeLength, knob(at(0.0), y)));
            v.push((Ctl::ScopeRepeat, knob(at(1.0), y)));
            v.push((Ctl::Separation, knob(at(2.0), y)));
            v.push((Ctl::Pinch, toggle(at(3.4), y - 8.0, 90.0)));
            v.push((Ctl::Interpolate, toggle(at(3.4), y + 8.0, 90.0)));
        }
        Mode::Spectrum => {
            v.push((Ctl::SpecLength, knob(at(0.0), y)));
            v.push((Ctl::SpecRes, list(at(1.2))));
            v.push((Ctl::SpecCurve, knob(at(2.6), y)));
            v.push((Ctl::SpecRange, knob(at(3.6), y)));
            v.push((Ctl::Natural, toggle(at(4.5), y - 8.0, 130.0)));
            v.push((Ctl::Enhanced, toggle(at(4.5), y + 8.0, 130.0)));
        }
        Mode::Meter => {
            v.push((Ctl::MeterScale, knob(at(0.0), y)));
            v.push((Ctl::MeterHold, knob(at(1.0), y)));
            v.push((Ctl::MeterMode, list(at(2.2))));
            v.push((Ctl::MeterDecay, knob(at(3.6), y)));
        }
        Mode::Vectorscope => {
            v.push((Ctl::VecLength, knob(at(0.0), y)));
            v.push((Ctl::VecThick, knob(at(1.0), y)));
            v.push((Ctl::VecMode, list(at(2.2))));
            v.push((Ctl::Additive, toggle(at(3.4), y, 60.0)));
        }
    }
    v
}

/// The control under a point (the display last: the cards are drawn over nothing of it).
pub fn hit(st: &State, x: f32, y: f32) -> Option<Ctl> {
    layout(st).into_iter().rev().find(|(_, r)| r.contains(x, y)).map(|(c, _)| c)
}

/// What the painter needs besides the state.
#[derive(Clone, Copy, Debug)]
pub struct Ui {
    pub hover: Option<Ctl>,
    pub pressed: Option<Ctl>,
    pub target: usize,
    pub held: bool,
}

fn knob_value(st: &State, c: Ctl, target: usize) -> (f32, bool) {
    match kind(c) {
        Kind::Knob { min, max, bipolar, .. } if max > min => (((value(st, c, target).clamp(min, max) - min) as f32 / (max - min) as f32), bipolar),
        Kind::Fader { min, max, .. } if max > min => (((value(st, c, target).clamp(min, max) - min) as f32 / (max - min) as f32), true),
        _ => (0.0, false),
    }
}

fn card(c: &mut Canvas, r: Rect, title: &str) {
    lemon::plug_panel(c, r);
    c.text(SMALL, r.x + 10.0, r.y + 3.0, plug::DIM, title);
    c.fill_rect(Rect::new(r.x + 6.0, r.y + BAND, r.w - 12.0, 1.0), plug::PANEL_LINE);
}

/// Paints the cards and their controls (not the display).
pub fn paint_panel(c: &mut Canvas, st: &State, ui: &Ui) {
    card(c, INPUT_CARD, "INPUT");
    card(c, LOOK_CARD, "APPEARANCE");
    card(c, COLOR_CARD, "COLORS");
    lemon::plug_panel(c, MODE_CARD);
    for (ctl, r) in layout(st) {
        let look = Look::new(ui.hover == Some(ctl), ui.pressed == Some(ctl), false);
        match kind(ctl) {
            Kind::Knob { .. } => {
                let (v, bipolar) = knob_value(st, ctl, ui.target);
                lemon::dial(c, r.cx(), r.cy(), r.w / 2.0, 3.5, v, bipolar, brand::MAIN, look);
                let cap = caption(ctl);
                if matches!(ctl, Ctl::Hue) {
                    c.text_center(LABEL, Rect::new(r.x - 10.0, r.bottom() + 2.0, r.w + 20.0, 12.0), plug::DIM, cap);
                } else if r.y > MODE_CARD.y {
                    // Mode knobs: caption on the left.
                    c.text_right(LABEL, r.x - 6.0, r.cy() - 7.0, plug::DIM, cap);
                } else {
                    c.text_center(LABEL, Rect::new(r.x - 20.0, r.bottom() + 1.0, r.w + 40.0, 12.0), plug::DIM, cap);
                }
            }
            Kind::Fader { .. } => {
                let (v, _) = knob_value(st, ctl, ui.target);
                let track = Rect::new(r.cx() - 2.0, r.y + 4.0, 4.0, r.h - 8.0);
                c.fill_rrect(track, 2.0, plug::TRACK);
                let mid = track.cy();
                let hy = track.bottom() - v * track.h;
                c.fill_rect(Rect::new(track.x, mid.min(hy), track.w, (hy - mid).abs()), brand::MAIN);
                let cap = Rect::new(r.x, hy - 5.0, r.w, 10.0);
                let face = if look.hover || look.pressed { Color::rgb(0x5c5c5c) } else { plug::FACE };
                c.fill_rrect(cap, 3.0, plug::FACE_LINE);
                c.fill_rrect(cap.inset(1.0), 2.0, face);
                c.fill_rect(Rect::new(cap.x + 3.0, cap.cy() - 0.5, cap.w - 6.0, 1.0), brand::MAIN);
                c.text_center(LABEL, Rect::new(r.x - 8.0, r.bottom() + 2.0, r.w + 16.0, 12.0), plug::DIM, caption(ctl));
            }
            Kind::Toggle => {
                let on = toggled(st, ctl);
                lemon::toggle(c, r.x + 7.0, r.cy(), on, look);
                c.text(LABEL, r.x + 18.0, r.cy() - 7.0, if look.hover { brand::TEXT } else { plug::DIM }, caption(ctl));
            }
            Kind::List => {
                lemon::field(c, r, look);
                let text = match ctl {
                    Ctl::Target => TARGET_NAMES[ui.target.min(TARGETS - 1)].to_string(),
                    Ctl::SpecRes => format!("{} bands", crate::analysis::bands(st.spectrum_res)),
                    Ctl::MeterMode => METER_MODES[st.meter_mode_index()].1.to_string(),
                    Ctl::VecMode => VECTOR_MODES[st.vector_mode.clamp(0, 3) as usize].to_string(),
                    _ => String::new(),
                };
                c.text_clip(SMALL, r.x + 6.0, r.y + 2.0, r.w - 18.0, brand::TEXT, &text);
                c.text(SMALL, r.right() - 11.0, r.y + 2.0, plug::DIM, "\u{25be}");
                if ctl != Ctl::Target {
                    c.text_right(LABEL, r.x - 6.0, r.cy() - 7.0, plug::DIM, caption(ctl));
                } else {
                    // The swatch: the target's colour over a checker for its opacity.
                    let sw = Rect::new(r.right() + 6.0, r.y, 22.0, r.h);
                    c.fill_rrect(sw, 3.0, plug::FIELD_LINE);
                    let inner = sw.inset(2.0);
                    c.fill_rect(inner, Color::rgb(0x8a8a8a));
                    c.fill_rect(Rect::new(inner.x, inner.y, inner.w / 2.0, inner.h / 2.0), Color::rgb(0x5a5a5a));
                    c.fill_rect(Rect::new(inner.cx(), inner.cy(), inner.w / 2.0, inner.h / 2.0), Color::rgb(0x5a5a5a));
                    c.fill_rect(inner, crate::display::color(st, ui.target.min(TARGETS - 1), 1.0));
                }
            }
            Kind::Text => {
                lemon::field(c, r, look);
                c.text(SMALL, r.x + 6.0, r.y + 2.0, plug::DIM, "Title");
                c.text_clip(LABEL, r.x + 36.0, r.y + 1.0, r.w - 42.0, brand::TEXT, &st.title_text());
            }
            Kind::Button => {
                let look = Look { on: ui.held, ..look };
                let col = lemon::plug_button(c, r, 5.0, look);
                c.text_center(LABEL, r, col, caption(ctl));
            }
            Kind::Tab => {
                let on = toggled(st, ctl);
                let look = Look { on, ..look };
                let col = lemon::plug_button(c, r, 5.0, look);
                if let Ctl::Tab(m) = ctl {
                    c.text_center(LABEL, r, col, m.name());
                }
            }
            Kind::Area => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controls_do_not_overlap_and_stay_in_their_cards() {
        for m in Mode::ALL {
            let st = State { mode: m, ..State::new_default() };
            let l = layout(&st);
            for (i, (a, ra)) in l.iter().enumerate() {
                let inside = [INPUT_CARD, LOOK_CARD, COLOR_CARD, MODE_CARD, DISPLAY]
                    .iter()
                    .any(|c| ra.x >= c.x - 0.5 && ra.y >= c.y - 0.5 && ra.right() <= c.right() + 0.5 && ra.bottom() <= c.bottom() + 0.5);
                assert!(inside, "{a:?} {ra:?}");
                for (b, rb) in &l[i + 1..] {
                    if *a == Ctl::Display || *b == Ctl::Display {
                        continue;
                    }
                    assert!(ra.intersect(rb).is_empty(), "{a:?} {ra:?} overlaps {b:?} {rb:?}");
                }
            }
        }
    }

    #[test]
    fn colour_knobs_edit_the_chosen_target() {
        let mut st = State::new_default();
        set_value(&mut st, Ctl::Hue, WAVEFORM, 300);
        assert_eq!(st.paints[WAVEFORM].hue, 44);
        assert_eq!(st.words[WAVEFORM], st.paints[WAVEFORM].word(false));
        assert_eq!(st.paints[BACKGROUND], State::new_default().paints[BACKGROUND]);
    }

    #[test]
    fn every_knob_reads_its_default_on_a_fresh_state() {
        let st = State::new_default();
        for m in Mode::ALL {
            let st = State { mode: m, ..st.clone() };
            for (c, _) in layout(&st) {
                if let Kind::Knob { def, .. } | Kind::Fader { def, .. } = kind(c) {
                    if !matches!(c, Ctl::Hue | Ctl::Light | Ctl::Sat | Ctl::Object | Ctl::Opacity) {
                        assert_eq!(value(&st, c, 0), def, "{c:?}");
                    }
                }
            }
        }
    }
}
