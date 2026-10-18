// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Settings ▸ Theme ▸ Adjustments: five knobs and eight colour circles that edit the applied theme live.
//!
//! They read and rewrite the applied `.flstheme` (`State::fl_text`; with no theme, the skin's own colours as
//! a theme, `os_theme::design::default_text`), so they colour either skin like any theme. A knob is dragged up / down or stepped by the wheel; a middle click
//! restores the value of the theme as it was chosen (`State::theme_base`). A colour circle opens a colour picker under
//! it: a saturation / brightness square, a hue strip, the old and new colours and Revert. Steps, Meters and Waves hold
//! several colours, picked one at a time from a row of chips. The file in the user folder (`current.flstheme`) is
//! written when a drag ends, on each wheel notch and on each picker change.
// spec: ui/theme-presets.md §C

use crate::app::{App, Drag, State};
use os_theme::Design;
use os_theme::fl;
use os_theme::{Color, FlTheme};
use os_ui::Rect;

/// The applied theme's text: with no theme, skin `d`'s own colours as a theme.
pub fn applied_text(s: &State, d: Design) -> String {
    s.fl_text.clone().unwrap_or_else(|| os_theme::design::default_text(d))
}

/// One adjustment knob: its key, caption, file range, wheel step and the name its hint gives it.
pub struct Knob {
    pub key: &'static str,
    pub label: &'static str,
    pub min: f32,
    pub max: f32,
    pub notch: f32,
    pub name: &'static str,
}

/// Hue, Saturation, Brightness (the `Lightness` key), Contrast and Text (browser text brightness), with the ranges of
/// ui/flstheme.md §3.
pub const KNOBS: [Knob; 5] = [
    Knob { key: "Hue", label: "Hue", min: -180.0, max: 180.0, notch: 5.0, name: "Hue" },
    Knob { key: "Saturation", label: "Saturation", min: -256.0, max: 256.0, notch: 8.0, name: "Saturation" },
    Knob { key: "Lightness", label: "Brightness", min: -255.0, max: 255.0, notch: 8.0, name: "Brightness" },
    Knob { key: "Contrast", label: "Contrast", min: -100.0, max: 100.0, notch: 4.0, name: "Contrast" },
    Knob { key: "Text", label: "Text", min: -255.0, max: 255.0, notch: 8.0, name: "Browser text" },
];

/// Knob travel per pixel of vertical drag: 300 px sweep the whole range.
pub const KNOB_PER_PX: f32 = 1.0 / 300.0;

impl Knob {
    /// `v` (file units) as a 0..1 knob position.
    pub fn pos(&self, v: f32) -> f32 {
        ((v - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
    }

    /// The whole file value at knob position `pos`.
    pub fn at(&self, pos: f32) -> f32 {
        (self.min + pos.clamp(0.0, 1.0) * (self.max - self.min)).round()
    }

    /// The hint while it turns, e.g. `Hue: -20°`, `Brightness: -153`.
    pub fn text(&self, v: f32) -> String {
        let unit = if self.key == "Hue" { "°" } else { "" };
        format!("{}: {v:.0}{unit}", self.name)
    }
}

/// Knob `i`'s value in theme `th`.
pub fn knob_value(th: &FlTheme, i: usize) -> f32 {
    match i {
        0 => th.surface.hue,
        1 => th.surface.saturation,
        2 => th.surface.lightness,
        3 => th.surface.contrast,
        _ => th.text,
    }
}

/// The colour circles' captions.
pub const SWATCHES: [&str; 8] = ["Selection", "Highlight", "Mute", "Option", "Steps", "Meters", "Waves", "Text"];
/// Circle index of Steps, Meters and Waves.
pub const STEPS: usize = 4;
pub const METERS: usize = 5;
pub const WAVES: usize = 6;

/// The stock Dark theme's meter gradient (spec: ui/theme-presets.md "Dark"), shown and edited when a theme has none.
const STOCK_METERS: [i64; 6] = [-11608904, -12526432, -12534787, -11614738, -13600515, -13600515];

/// One colour a circle edits: its key, its chip label and its colour now.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub key: String,
    pub label: String,
    pub colour: Color,
}

fn num(text: &str, key: &str) -> Option<i64> {
    fl::get_key(text, key)?.parse().ok()
}

/// The colours circle `i` edits in theme `text`, in chip order. A key the theme lacks shows the stock Dark value
/// (Meters: its gradient; Waves: the stock wave gradient, `os_theme::waves::STOCK`).
pub fn parts(text: &str, i: usize) -> Vec<Part> {
    let dark = fl::DARK_TEXT;
    let one = |key: &str, label: &str, fallback: Option<i64>| {
        let v = num(text, key).or_else(|| num(dark, key)).or(fallback).unwrap_or(0);
        Part { key: key.to_string(), label: label.to_string(), colour: fl::colour_of(key, v) }
    };
    match i {
        0 => vec![one("Selected", "", None)],
        1 => vec![one("Highlight", "", None)],
        2 => vec![one("Mute", "", None)],
        3 => vec![one("Option", "", None)],
        STEPS => vec![one("StepEven", "Even", None), one("StepOdd", "Odd", None)],
        METERS => (0..6).map(|k| one(&format!("Meter{k}"), &(k + 1).to_string(), Some(STOCK_METERS[k]))).collect(),
        WAVES => {
            let own: Vec<usize> = (0..6).filter(|k| num(text, &format!("WaveClr{k}")).is_some() && fl::get_key(text, &format!("WaveSpc{k}")).is_some()).collect();
            if own.is_empty() {
                os_theme::waves::STOCK
                    .iter()
                    .enumerate()
                    .map(|(k, (_, c))| Part { key: format!("WaveClr{k}"), label: (k + 1).to_string(), colour: *c })
                    .collect()
            } else {
                own.iter().enumerate().map(|(n, &k)| one(&format!("WaveClr{k}"), &(n + 1).to_string(), None)).collect()
            }
        }
        _ => vec![one("TextColor", "", None)],
    }
}

/// `text` with part `key` of circle `i` set to `c`. A Meters or Waves colour in a theme without its own gradient first
/// writes the stock gradient (Waves: positions and colours), so the one colour changed shows as part of it.
pub fn set_part(text: &str, i: usize, key: &str, c: Color) -> String {
    let mut out = text.to_string();
    if i == METERS && num(text, "Meter0").is_none() {
        for (k, v) in STOCK_METERS.iter().enumerate() {
            out = fl::set_key(&out, &format!("Meter{k}"), &v.to_string());
        }
    }
    if i == WAVES && parts(text, WAVES).iter().all(|p| num(text, &p.key).is_none()) {
        for (k, (at, col)) in os_theme::waves::STOCK.iter().enumerate() {
            out = fl::set_key(&out, &format!("WaveSpc{k}"), &at.to_string());
            out = fl::set_key(&out, &format!("WaveClr{k}"), &fl::colour_value("WaveClr", *col).to_string());
        }
    }
    fl::set_key(&out, key, &fl::colour_value(key, c).to_string())
}

/// HSV (h in degrees, s and v 0..1) from a colour.
pub fn hsv(c: Color) -> [f32; 3] {
    let (r, g, b) = (f32::from(c.r) / 255.0, f32::from(c.g) / 255.0, f32::from(c.b) / 255.0);
    let max = r.max(g).max(b);
    let d = max - r.min(g).min(b);
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    [h, if max == 0.0 { 0.0 } else { d / max }, max]
}

/// The colour of HSV `[h, s, v]`.
pub fn from_hsv([h, s, v]: [f32; 3]) -> Color {
    let f = |n: f32| {
        let k = (n + h.rem_euclid(360.0) / 60.0).rem_euclid(6.0);
        let x = v - v * s * k.min(4.0 - k).clamp(0.0, 1.0);
        (x * 255.0).round().clamp(0.0, 255.0) as u8
    };
    Color { r: f(5.0), g: f(3.0), b: f(1.0), a: 255 }
}

/// The open colour picker (`SettingsUi::picker`).
#[derive(Debug, Clone, PartialEq)]
pub struct Picker {
    /// Which circle and which of its colours (`parts`).
    pub swatch: usize,
    pub part: usize,
    /// The colour being picked, kept as HSV so hue and saturation survive black and greys.
    pub hsv: [f32; 3],
    /// The applied theme when the picker opened (`State::fl_text`), for Revert and the old colour.
    pub before: Option<String>,
    /// The circle's centre and radius: the panel opens under it.
    pub at: (f32, f32, f32),
}

/// The picker panel's parts, from its circle (`Picker::at`), its number of colours and the window it must stay in.
pub struct PickerRects {
    pub panel: Rect,
    pub close: Rect,
    pub chips: Vec<Rect>,
    pub sv: Rect,
    pub hue: Rect,
    /// New colour over old colour.
    pub new: Rect,
    pub old: Rect,
    pub revert: Rect,
}

pub const PICKER_W: f32 = 268.0;

/// Where the picker's parts are: under its circle (over it when there is no room below), inside `win`.
pub fn picker_rects(p: &Picker, n_parts: usize, win: Rect) -> PickerRects {
    let chips_h = if n_parts > 1 { 26.0 } else { 0.0 };
    let h = 34.0 + chips_h + 110.0 + 10.0;
    let (cx, cy, r) = p.at;
    let x = (cx - PICKER_W / 2.0).clamp(win.x + 6.0, (win.right() - PICKER_W - 6.0).max(win.x + 6.0)).round();
    let below = cy + r + 6.0;
    let y = if below + h <= win.bottom() - 6.0 { below } else { (cy - r - 6.0 - h).max(win.y + 6.0) }.round();
    let panel = Rect::new(x, y, PICKER_W, h);
    let chips = (0..if n_parts > 1 { n_parts } else { 0 }).map(|k| Rect::new(x + 10.0 + k as f32 * 40.0, y + 32.0, 36.0, 20.0)).collect();
    let by = y + 34.0 + chips_h;
    PickerRects {
        panel,
        close: Rect::new(panel.right() - 26.0, y + 6.0, 20.0, 20.0),
        chips,
        sv: Rect::new(x + 10.0, by, 150.0, 110.0),
        hue: Rect::new(x + 168.0, by, 14.0, 110.0),
        new: Rect::new(x + 192.0, by, 66.0, 30.0),
        old: Rect::new(x + 192.0, by + 30.0, 66.0, 22.0),
        revert: Rect::new(x + 192.0, by + 88.0, 66.0, 22.0),
    }
}

impl App {
    /// The applied theme's text (`applied_text`) to edit. The Lemon theme's key goes: edited, it is a theme like any
    /// other (on Lemon still Lemon's colours, `os_theme::design::default_text`); the Citrus theme keeps its key, so on
    /// Lemon it stays laid over Lemon even where it is stock Dark.
    pub(crate) fn theme_text(&self) -> String {
        let text = applied_text(&self.state, self.design());
        if FlTheme::parse(&text).colors == Some(Design::Lemon) { fl::remove_key(&text, fl::COLORS_KEY) } else { text }
    }

    /// Applies an edited theme `text` to the skin in use, live: the window, the skin tiles' previews (the theme tiles
    /// keep showing the themes as shipped) and the chosen tile, which stays the theme it was chosen from.
    pub(crate) fn theme_edit(&mut self, text: String) {
        if self.state.fl_text.as_deref() == Some(text.as_str()) {
            return;
        }
        let th = FlTheme::parse(&text);
        let palette = os_theme::design::palette(self.design(), Some(&th));
        self.theme = os_theme::Theme::new(&self.theme.name.clone(), palette, self.theme.adjust);
        os_theme::set_current(&self.theme.colors);
        self.state.fl_text = Some(text);
        if self.state.settings.open {
            self.state.settings.skin_previews = os_theme::design::Design::ALL.iter().map(|d| os_theme::design::palette(*d, Some(&th))).collect();
        }
        self.check_readable();
        self.invalidate();
    }

    /// Keeps the applied theme for the next start (`current.flstheme` in the user folder).
    pub(crate) fn theme_store(&self) {
        if let (Some(d), Some(text)) = (crate::app::theme_dir(), &self.state.fl_text) {
            let _ = std::fs::create_dir_all(&d);
            let _ = std::fs::write(d.join("current.flstheme"), text);
        }
    }

    fn knob_set(&mut self, i: usize, v: f32) {
        let k = &KNOBS[i];
        let v = v.clamp(k.min, k.max).round();
        let text = fl::set_key(&self.theme_text(), k.key, &format!("{v:.0}"));
        self.theme_edit(text);
        self.hint = std::borrow::Cow::Owned(k.text(v));
    }

    /// Knob `i` was pressed: a drag from here turns it (`Drag::ThemeKnob`).
    pub(crate) fn theme_knob_press(&mut self, i: usize) {
        self.theme_mark(crate::theme_library::Step::Other);
        let th = FlTheme::parse(&self.theme_text());
        let k = &KNOBS[i];
        self.drag = Some(Drag::ThemeKnob { i, y0: self.mouse.1, p0: k.pos(knob_value(&th, i)) });
        self.hint = std::borrow::Cow::Owned(k.text(knob_value(&th, i)));
    }

    /// The knob drag moved to height `y`: up for more.
    pub(crate) fn theme_knob_drag(&mut self, i: usize, y0: f32, p0: f32, y: f32) {
        self.knob_set(i, KNOBS[i].at(p0 + (y0 - y) * KNOB_PER_PX));
    }

    /// Wheel notches over knob `i`: one step each, up for more.
    pub(crate) fn theme_knob_wheel(&mut self, i: usize, notches: i32) {
        self.theme_mark(crate::theme_library::Step::Wheel(i));
        let v = knob_value(&FlTheme::parse(&self.theme_text()), i);
        self.knob_set(i, v + notches as f32 * KNOBS[i].notch);
        self.theme_store();
    }

    /// A middle click: knob `i` goes back to the value of the theme as it was chosen.
    pub(crate) fn theme_knob_reset(&mut self, i: usize) {
        self.theme_mark(crate::theme_library::Step::Other);
        let base = self.state.theme_base.clone().unwrap_or_else(|| os_theme::design::default_text(self.design()));
        self.knob_set(i, knob_value(&FlTheme::parse(&base), i));
        self.theme_store();
    }

    /// Colour circle `i` (its colour `part`) was clicked: the picker opens under it, or moves to that colour.
    pub(crate) fn picker_open(&mut self, i: usize, part: usize, at: (f32, f32, f32)) {
        let parts = parts(&self.theme_text(), i);
        let part = part.min(parts.len() - 1);
        let before = match &self.state.settings.picker {
            Some(p) if p.swatch == i => p.before.clone(),
            _ => self.state.fl_text.clone(),
        };
        let hint = format!("{}: pick a colour", SWATCHES[i]);
        self.state.settings.picker = Some(Picker { swatch: i, part, hsv: hsv(parts[part].colour), before, at });
        self.state.message = Some(hint);
        self.invalidate();
    }

    /// The picker shows another of its circle's colours.
    pub(crate) fn picker_part(&mut self, part: usize) {
        let text = self.theme_text();
        if let Some(p) = &mut self.state.settings.picker {
            if let Some(c) = parts(&text, p.swatch).get(part) {
                p.part = part;
                p.hsv = hsv(c.colour);
            }
        }
        self.invalidate();
    }

    /// Sets the picked colour from the picker's HSV and applies it.
    fn picker_apply(&mut self) {
        let Some(p) = self.state.settings.picker.clone() else { return };
        let text = self.theme_text();
        let Some(part) = parts(&text, p.swatch).get(p.part).cloned() else { return };
        let c = from_hsv(p.hsv);
        self.theme_edit(set_part(&text, p.swatch, &part.key, c));
        self.hint = std::borrow::Cow::Owned(format!("{}: #{:02x}{:02x}{:02x}", SWATCHES[p.swatch], c.r, c.g, c.b));
    }

    /// The saturation / brightness square `r` was pressed or dragged to (`x`, `y`): saturation left to right,
    /// brightness bottom to top.
    pub(crate) fn picker_sv(&mut self, r: Rect, x: f32, y: f32) {
        if let Some(p) = &mut self.state.settings.picker {
            p.hsv[1] = ((x - r.x) / r.w).clamp(0.0, 1.0);
            p.hsv[2] = (1.0 - (y - r.y) / r.h).clamp(0.0, 1.0);
        }
        self.picker_apply();
    }

    /// The hue strip `r` was pressed or dragged to height `y`: 0° at the top round to 360° at the bottom.
    pub(crate) fn picker_hue(&mut self, r: Rect, y: f32) {
        if let Some(p) = &mut self.state.settings.picker {
            p.hsv[0] = ((y - r.y) / r.h).clamp(0.0, 1.0) * 360.0;
        }
        self.picker_apply();
    }

    /// Revert (or a click on the old colour): the theme goes back to how it was when the picker opened.
    pub(crate) fn picker_revert(&mut self) {
        let Some(p) = self.state.settings.picker.clone() else { return };
        self.theme_mark(crate::theme_library::Step::Other);
        match p.before.clone() {
            Some(text) => self.theme_edit(text),
            None => {
                self.theme = crate::app::design_theme(self.design(), false);
                os_theme::set_current(&self.theme.colors);
                self.state.fl_text = None;
                self.sync_preset();
            }
        }
        match &self.state.fl_text {
            Some(_) => self.theme_store(),
            None => {
                if let Some(d) = crate::app::theme_dir() {
                    let _ = std::fs::remove_file(d.join("current.flstheme"));
                }
            }
        }
        let text = self.theme_text();
        if let Some(q) = &mut self.state.settings.picker {
            if let Some(c) = parts(&text, q.swatch).get(q.part) {
                q.hsv = hsv(c.colour);
            }
        }
        self.state.message = Some(format!("{}: back to how it was", SWATCHES[p.swatch]));
        self.invalidate();
    }

    /// Closes the colour picker, if open.
    pub(crate) fn picker_close(&mut self) {
        if self.state.settings.picker.take().is_some() {
            self.invalidate();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knob_ranges_and_texts() {
        let hue = &KNOBS[0];
        assert_eq!((hue.pos(-180.0), hue.pos(0.0), hue.pos(180.0), hue.pos(999.0)), (0.0, 0.5, 1.0, 1.0));
        assert_eq!((hue.at(0.5), hue.at(0.25), hue.at(-1.0)), (0.0, -90.0, -180.0));
        assert_eq!(hue.text(-20.0), "Hue: -20°");
        assert_eq!(KNOBS[2].text(-153.0), "Brightness: -153");
        let th = FlTheme::dark();
        assert_eq!((0..5).map(|i| knob_value(&th, i)).collect::<Vec<_>>(), vec![0.0, 0.0, -153.0, -8.0, 0.0]);
    }

    #[test]
    fn hsv_round_trips() {
        for c in [0xa8e44a, 0xfb9244, 0x000000, 0xffffff, 0x808080, 0x123456, 0xff0000, 0x00ff00, 0x0000ff, 0xc0ffee] {
            let c = Color::rgb(c);
            assert_eq!(from_hsv(hsv(c)), c);
        }
        assert_eq!(from_hsv([120.0, 1.0, 1.0]), Color::rgb(0x00ff00));
        assert_eq!(from_hsv([360.0, 1.0, 1.0]), Color::rgb(0xff0000));
    }

    /// Each circle's colours, from the theme or the stock Dark theme; picking writes the key in its byte order.
    #[test]
    fn circles_read_and_write_their_keys() {
        let dark = fl::DARK_TEXT;
        assert_eq!(parts(dark, 0)[0].colour, Color::rgb(0xa8e44a));
        assert_eq!(parts(dark, 1)[0].colour, Color::rgb(0xfb9244));
        let steps = parts(dark, STEPS);
        assert_eq!(steps.iter().map(|p| p.label.as_str()).collect::<Vec<_>>(), vec!["Even", "Odd"]);
        assert_eq!(parts(dark, METERS).len(), 6, "the stock meters when the theme has none");
        let t = set_part(dark, 0, "Selected", Color::rgb(0x2d7cd0));
        assert_eq!(FlTheme::parse(&t).selected, Some(Color::rgb(0x2d7cd0)));
        let t = set_part(dark, METERS, "Meter2", Color::rgb(0x010203));
        let meters = FlTheme::parse(&t).meters;
        assert_eq!((meters.len(), meters[2]), (6, Color::rgb(0x010203)), "the stock meters, with the one colour changed");
        // Waves: a theme without them gets the stock gradient with the one colour changed.
        let waves = parts(dark, WAVES);
        assert_eq!(waves.len(), 6);
        let t = set_part(dark, WAVES, &waves[3].key, Color::rgb(0x00ff00));
        let th = FlTheme::parse(&t);
        assert_eq!(th.waves.len(), 6);
        assert_eq!(th.waves[3].1, Color::rgb(0x00ff00));
        assert_eq!(th.waves[0], os_theme::waves::STOCK[0]);
        // A theme with its own Waves edits those.
        let moss = os_theme::presets::PRESETS.iter().find(|(n, _)| *n == "Moss").unwrap().1;
        let own = parts(moss, WAVES);
        assert!(!own.is_empty() && own.len() <= 6);
        let t = set_part(moss, WAVES, &own[0].key, Color::rgb(0x112233));
        assert_eq!(parts(&t, WAVES)[0].colour, Color::rgb(0x112233));
        assert_eq!(parts(&t, WAVES).len(), own.len());
    }
}
