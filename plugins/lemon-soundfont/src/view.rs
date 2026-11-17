// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon SoundFont Player editor: painting and hit regions (platform independent), in the Lemon "Pith" plugin look
//! on a 6 px grid. Top to bottom: the font (open button, file name; patch, bank and preset name), the reverb and
//! chorus cards, the volume envelope's sliders beside the LFO and misc knobs, and the strip with the playback mode
//! and the wordmark.
//!
// spec: plugins/lemon-soundfont.md §4, ui/lemon-design.md §11.1

use os_theme::design::lemon as brand;
use os_theme::Palette;
use os_ui::kit::lemon::{self, plug};
use os_ui::kit::{inst, State as Kst};
use os_ui::{Canvas, Color, Font, Rect};

use crate::profile::Mode;
use crate::state::{Control, State};

/// Editor size in logical pixels.
pub const W: u32 = 462;
pub const H: u32 = 474;

const GAP: f32 = 6.0;
const fn edges(x0: f32, x1: f32, y0: f32, y1: f32) -> Rect {
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}
const RIGHT: f32 = W as f32 - GAP;
const BOTTOM: f32 = H as f32 - GAP;
const MID: f32 = 228.0;

/// The font card: open button and file name; patch, bank and preset name.
pub const FONT_CARD: Rect = edges(GAP, RIGHT, GAP, 84.0);
pub const OPEN: Rect = edges(12.0, 36.0, 12.0, 36.0);
pub const FILE: Rect = edges(42.0, RIGHT - 6.0, 12.0, 36.0);
pub const PATCH: Rect = edges(12.0, 60.0, 54.0, 78.0);
pub const BANK: Rect = edges(66.0, 114.0, 54.0, 78.0);
pub const NAME: Rect = edges(120.0, 396.0, 54.0, 78.0);
pub const PREV: Rect = edges(402.0, 426.0, 54.0, 78.0);
pub const NEXT: Rect = edges(426.0, 450.0, 54.0, 78.0);
/// The effect cards.
pub const REVERB: Rect = edges(GAP, MID, 90.0, 168.0);
pub const CHORUS: Rect = edges(MID + GAP, RIGHT, 90.0, 168.0);
/// The envelope's sliders, the LFO and misc cards.
pub const ENVELOPE: Rect = edges(GAP, MID, 174.0, 414.0);
pub const LFO: Rect = edges(MID + GAP, RIGHT, 174.0, 288.0);
pub const MISC: Rect = edges(MID + GAP, RIGHT, 294.0, 414.0);
/// The bottom strip: the playback mode, the wordmark.
pub const STRIP: Rect = edges(GAP, RIGHT, 420.0, BOTTOM);
const STRIP_Y: f32 = (420.0 + BOTTOM) / 2.0;
pub const CLASSIC: Rect = edges(66.0, 144.0, STRIP_Y - 12.0, STRIP_Y + 12.0);
pub const MODERN: Rect = edges(150.0, 228.0, STRIP_Y - 12.0, STRIP_Y + 12.0);
/// The wordmark; an unhosted editor shows its hint there instead.
pub const LOGO: Rect = edges(240.0, RIGHT - 6.0, 426.0, BOTTOM - 6.0);
pub const HINT: Rect = LOGO;

const BAND: f32 = 18.0;
const KNOB_R: f32 = 17.0;
/// The envelope sliders' columns and travel.
const SLIDER_X: [f32; 4] = [51.0, 95.0, 139.0, 183.0];
const SLIDER_TOP: f32 = 204.0;
const SLIDER_BOTTOM: f32 = 384.0;

const SMALL: Font = Font::new(10);
const LABEL: Font = Font::new(11);
const PILL: Font = Font::new(10).bold();

/// What a region does when clicked or dragged.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// Opens a font (the button, the file name).
    Open,
    /// The preset: drag or wheel steps through the font's presets.
    Patch,
    /// The bank: wheel steps to the next bank's first preset.
    Bank,
    /// The preset name: a menu of the font's presets.
    Name,
    Prev,
    Next,
    /// A control of the player's own.
    Value(Control),
    /// An effect's send track (0 reverb, 1 chorus).
    Send(usize),
    /// An effect's built-in switch.
    BuiltIn(usize),
    Mode(Mode),
    Logo,
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub r: Rect,
    pub t: Target,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewState {
    pub mouse: Option<(f32, f32)>,
    pub pressed: Option<Target>,
}

/// What a paint shows besides the state.
pub struct Look<'a> {
    pub state: &'a State,
    pub mode: Mode,
    /// The font's name and the chosen preset's (bank, program, name), when a font opened.
    pub font: Option<String>,
    pub preset: Option<(u16, u16, String)>,
    /// Why the font did not open ("" when it did or none is set).
    pub error: &'a str,
}

/// A control's stored value as one number from "off" upward (the stored off value is below the range).
pub fn raw(st: &State, c: Control) -> i32 {
    st.control(c).unwrap_or_else(|| c.off())
}

/// A control's value text.
pub fn text(c: Control, v: Option<i32>, mode: Mode) -> String {
    let Some(v) = v else { return "Off (the font's)".into() };
    match c {
        Control::ReverbLevel | Control::ChorusLevel => format!("{}%", (f64::from(v) * 100.0 / 128.0).round()),
        Control::Attack => format!("{v} ms"),
        Control::Decay | Control::Release | Control::LfoDelay => format!("{} ms", v * 4),
        Control::LfoSpeed => format!("{:.2} Hz", mode.lfo_rate(v)),
        Control::LfoAmount => format!("{:.0} cents", mode.lfo_depth(v)),
        Control::Sustain | Control::Cutoff | Control::Mod => v.to_string(),
    }
}

/// A control's name for the hint.
pub fn name(c: Control) -> &'static str {
    match c {
        Control::ReverbLevel => "Reverb level",
        Control::ChorusLevel => "Chorus level",
        Control::Mod => "Modulation",
        Control::Attack => "Envelope attack",
        Control::Decay => "Envelope decay",
        Control::Sustain => "Envelope sustain",
        Control::Release => "Envelope release",
        Control::LfoDelay => "LFO delay",
        Control::LfoAmount => "LFO amount",
        Control::LfoSpeed => "LFO speed",
        Control::Cutoff => "Filter cutoff",
    }
}

/// Where a control sits from 0 (off, or its lowest) to 1.
fn norm(st: &State, c: Control) -> f32 {
    let (lo, hi) = c.range();
    // The levels' knobs travel to 200 % (higher values can be typed).
    let hi = if matches!(c, Control::ReverbLevel | Control::ChorusLevel) { 256 } else { hi };
    let lo = if c.can_be_off() { c.off() } else { lo };
    ((raw(st, c) - lo) as f32 / (hi - lo) as f32).clamp(0.0, 1.0)
}

struct Ui<'a> {
    c: &'a mut Canvas,
    t: &'a Palette,
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

/// `s` as it fits in `max_w`: whole, or cut at its end with an ellipsis.
pub fn elide(c: &mut Canvas, f: Font, s: &str, max_w: f32) -> String {
    if c.text_width(f, s) <= max_w {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let cut = |k: usize| format!("{}…", chars[..k].iter().collect::<String>().trim_end());
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if c.text_width(f, &cut(mid)) <= max_w { lo = mid } else { hi = mid - 1 }
    }
    cut(lo)
}

/// The file name of a stored path.
pub fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

const MISSING: Color = Color::rgb(0xd06050);

pub fn paint(c: &mut Canvas, t: &Palette, st: &ViewState, look: &Look, hint: &str) -> Vec<Hit> {
    let mut ui = Ui { c, t, st, hits: Vec::new() };
    ui.c.begin_frame(None);
    ui.c.clear(plug::BG);
    font_card(&mut ui, look);
    effect_card(&mut ui, REVERB, "REVERB", 0, Control::ReverbLevel, look);
    effect_card(&mut ui, CHORUS, "CHORUS", 1, Control::ChorusLevel, look);
    envelope(&mut ui, look);
    titled(&mut ui, LFO, "LFO");
    let y = LFO.y + 54.0;
    for (i, (ctl, cap)) in [(Control::LfoDelay, "DEL"), (Control::LfoAmount, "AMT"), (Control::LfoSpeed, "SPD")].into_iter().enumerate() {
        knob(&mut ui, LFO.x + 45.0 + 66.0 * i as f32, y, ctl, cap, look);
    }
    titled(&mut ui, MISC, "MISC");
    let y = MISC.y + 60.0;
    knob(&mut ui, MISC.x + 111.0, y, Control::Cutoff, "CUT", look);
    knob(&mut ui, MISC.x + 177.0, y, Control::Mod, "MOD", look);
    strip(&mut ui, look, hint);
    ui.hits
}

/// A card with its title in a band and a line under the band.
fn titled(ui: &mut Ui, r: Rect, title: &str) {
    lemon::plug_panel(ui.c, r);
    ui.c.fill_rect(Rect::new(r.x + 1.0, r.y + BAND, r.w - 2.0, 1.0), plug::PANEL_LINE);
    ui.c.text(SMALL.bold(), r.x + 8.0, r.y + 3.0, plug::DIM, title);
}

fn font_card(ui: &mut Ui, look: &Look) {
    lemon::plug_panel(ui.c, FONT_CARD);
    // The open button: a folder.
    let kst = ui.state(OPEN, &Target::Open);
    let ink = lemon::plug_button(ui.c, OPEN, 5.0, kst);
    let (x, y) = (OPEN.x + 6.0, OPEN.y + 7.0);
    ui.c.stroke_rrect(Rect::new(x, y + 2.0, 12.0, 9.0), 1.0, 1.4, ink);
    ui.c.fill_rect(Rect::new(x, y, 5.0, 3.0), ink);
    ui.hit(OPEN, Target::Open);
    let kst = ui.state(FILE, &Target::Open);
    lemon::field(ui.c, FILE, kst);
    let file = look.state.path();
    let (s, col) = if file.is_empty() {
        ("No SoundFont: click to open one".to_string(), plug::FAINT)
    } else if !look.error.is_empty() {
        (format!("{} (not found)", file_name(&file)), MISSING)
    } else {
        (look.font.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| file_name(&file).to_string()), brand::TEXT)
    };
    let s = elide(ui.c, LABEL.bold(), &s, FILE.w - 16.0);
    ui.c.text(LABEL.bold(), FILE.x + 8.0, FILE.cy() - 7.0, col, &s);
    ui.hit(FILE, Target::Open);
    for (r, cap) in [(PATCH, "PATCH"), (BANK, "BANK"), (NAME, "NAME")] {
        ui.c.text(Font::new(8).bold(), r.x + 1.0, r.y - 11.0, plug::FAINT, cap);
    }
    let p = look.preset.as_ref();
    for (r, t, v) in [(PATCH, Target::Patch, p.map(|p| p.1 + 1)), (BANK, Target::Bank, p.map(|p| p.0))] {
        let mut kst = ui.state(r, &t);
        kst.disabled = v.is_none();
        lemon::field(ui.c, r, kst);
        if let Some(v) = v {
            let col = if kst.hover || kst.pressed { brand::TEXT } else { brand::MAIN };
            ui.c.text_center(LABEL.bold(), r, col, &v.to_string());
        }
        ui.hit(r, t);
    }
    let kst = Kst { disabled: p.is_none(), ..ui.state(NAME, &Target::Name) };
    lemon::field(ui.c, NAME, kst);
    if let Some((_, _, n)) = p {
        let s = elide(ui.c, LABEL.bold(), n, NAME.w - 16.0);
        ui.c.text(LABEL.bold(), NAME.x + 8.0, NAME.cy() - 7.0, brand::MAIN, &s);
    }
    ui.hit(NAME, Target::Name);
    for (r, t, left) in [(PREV, Target::Prev, true), (NEXT, Target::Next, false)] {
        let kst = Kst { disabled: p.is_none(), ..ui.state(r, &t) };
        let ink = lemon::plug_button(ui.c, r, 5.0, kst);
        let (x, y) = (r.cx(), r.cy());
        let d = if left { -1.0 } else { 1.0 };
        ui.c.fill_polygon(&[(x - 3.0 * d, y - 4.0), (x - 3.0 * d, y + 4.0), (x + 3.0 * d, y)], ink);
        ui.hit(r, t);
    }
}

/// A knob of a control (hit area 40 px across), its caption under it; off shows an empty ring.
fn knob(ui: &mut Ui, cx: f32, cy: f32, ctl: Control, caption: &str, look: &Look) {
    let t = Target::Value(ctl);
    let hr = Rect::new(cx - KNOB_R - 3.0, cy - KNOB_R - 3.0, 2.0 * KNOB_R + 6.0, 2.0 * KNOB_R + 6.0);
    let kst = ui.state(hr, &t);
    let v = look.state.control(ctl);
    let bipolar = ctl == Control::LfoAmount;
    let n = if bipolar { v.map_or(0.5, |v| (v + 127) as f32 / 254.0) } else { norm(look.state, ctl) };
    let arc = if v.is_none() && ctl.can_be_off() { plug::OFF } else { brand::MAIN };
    lemon::dial(ui.c, cx, cy, KNOB_R, 2.8, n, bipolar, arc, kst);
    inst::caption(ui.c, ui.t, cx, cy + KNOB_R + 8.0, caption, v.is_none() && ctl.can_be_off());
    ui.hit(hr, t);
}

fn effect_card(ui: &mut Ui, r: Rect, title: &str, fx: usize, level: Control, look: &Look) {
    titled(ui, r, title);
    // Send to: a mixer track box.
    let send = Rect::new(r.x + 66.0, r.y + 28.0, 54.0, 20.0);
    ui.c.text(Font::new(9).bold(), r.x + 10.0, send.cy() - 6.0, plug::DIM, "SEND TO");
    let t = Target::Send(fx);
    let kst = ui.state(send, &t);
    lemon::field(ui.c, send, kst);
    let tr = look.state.send(fx);
    let s = if tr < 0 { "---".to_string() } else { tr.to_string() };
    ui.c.text_center(SMALL.bold(), send, if tr < 0 { plug::FAINT } else { brand::MAIN }, &s);
    ui.hit(send, t);
    // Built-in: a switch.
    let led = Rect::new(r.x + 10.0, r.bottom() - 22.0, 76.0, 16.0);
    let t = Target::BuiltIn(fx);
    let kst = Kst { on: look.state.built_in(fx), ..ui.state(led, &t) };
    lemon::toggle(ui.c, led.x + 6.0, led.cy(), kst.on, kst);
    ui.c.text(Font::new(9).bold(), led.x + 16.0, led.cy() - 6.0, plug::DIM, "BUILT-IN");
    ui.hit(led, t);
    knob(ui, r.right() - 36.0, r.y + 42.0, level, "LEVEL", look);
}

fn envelope(ui: &mut Ui, look: &Look) {
    titled(ui, ENVELOPE, "ENVELOPE");
    for (i, (ctl, cap)) in [(Control::Attack, "A"), (Control::Decay, "D"), (Control::Sustain, "S"), (Control::Release, "R")].into_iter().enumerate() {
        let x = SLIDER_X[i];
        let t = Target::Value(ctl);
        let hr = edges(x - 14.0, x + 14.0, SLIDER_TOP - 8.0, SLIDER_BOTTOM + 8.0);
        let kst = ui.state(hr, &t);
        // Ticks either side of the track.
        for k in 0..=10 {
            let y = SLIDER_TOP + (SLIDER_BOTTOM - SLIDER_TOP) * k as f32 / 10.0;
            ui.c.fill_rect(Rect::new(x - 12.0, y, 5.0, 1.0), plug::TICK);
            ui.c.fill_rect(Rect::new(x + 7.0, y, 5.0, 1.0), plug::TICK);
        }
        ui.c.fill_rrect(Rect::new(x - 2.0, SLIDER_TOP, 4.0, SLIDER_BOTTOM - SLIDER_TOP), 2.0, plug::TRACK);
        let v = look.state.control(ctl);
        let n = norm(look.state, ctl);
        let y = SLIDER_BOTTOM - (SLIDER_BOTTOM - SLIDER_TOP) * n;
        if v.is_some() {
            ui.c.fill_rect(Rect::new(x - 1.0, y, 2.0, SLIDER_BOTTOM - y), brand::MAIN);
        }
        let handle = Rect::new(x - 10.0, y - 6.0, 20.0, 12.0);
        let ink = lemon::plug_button(ui.c, handle, 3.0, kst);
        ui.c.fill_rect(Rect::new(handle.x + 4.0, handle.cy() - 0.5, handle.w - 8.0, 1.0), if v.is_some() { brand::MAIN } else { ink });
        inst::caption(ui.c, ui.t, x, SLIDER_BOTTOM + 14.0, cap, v.is_none());
        ui.hit(hr, t);
    }
}

fn strip(ui: &mut Ui, look: &Look, hint: &str) {
    lemon::plug_panel(ui.c, STRIP);
    ui.c.text(Font::new(9).bold(), STRIP.x + 8.0, CLASSIC.cy() - 6.0, plug::DIM, "MODE");
    for (r, m, label) in [(CLASSIC, Mode::Classic, "CLASSIC"), (MODERN, Mode::Modern, "MODERN")] {
        let t = Target::Mode(m);
        let mut kst = ui.state(r, &t);
        kst.on = look.mode == m;
        let ink = lemon::plug_button(ui.c, r, 6.0, kst);
        ui.c.text_center(PILL, r, ink, label);
        ui.hit(r, t);
    }
    if hint.is_empty() {
        lemon::wordmark(ui.c, LOGO.right(), LOGO.y + 2.0, false, "SOUNDFONT", Font::condensed(18, 0).bold(), Font::new(8).bold());
    } else {
        let (left, right) = hint.split_once('\t').unwrap_or((hint, ""));
        let s = if right.is_empty() { left.to_string() } else { format!("{left}  ({right})") };
        let s = elide(ui.c, SMALL, &s, HINT.w);
        let lh = ui.c.line_height(SMALL);
        ui.c.text(SMALL, HINT.x, (HINT.cy() - lh / 2.0).round(), plug::DIM, &s);
    }
    ui.hit(LOGO, Target::Logo);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layout_keeps_its_grid() {
        assert_eq!((W % 6, H % 6), (0, 0));
        for r in [FONT_CARD, REVERB, CHORUS, ENVELOPE, LFO, MISC, STRIP] {
            assert!(r.x >= GAP && r.right() <= RIGHT && r.y >= GAP && r.bottom() <= BOTTOM, "{r:?}");
            assert_eq!((r.x % 6.0, r.y % 6.0), (0.0, 0.0), "{r:?}");
        }
        assert_eq!(CHORUS.x - REVERB.right(), GAP);
        assert_eq!(ENVELOPE.y - REVERB.bottom(), GAP);
        assert_eq!(MISC.y - LFO.bottom(), GAP);
        assert_eq!(STRIP.y - ENVELOPE.bottom(), GAP);
    }
}
