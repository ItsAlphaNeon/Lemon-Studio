// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The Etched skin's colours: a frosted-glass look. The window's base is the desktop wallpaper, blurred and tinted
//! (the backdrop, drawn by the app); the toolbar, the browser and the workspace are see-through to it; menus, the
//! settings window and floating title bars are frosted (they blur what is drawn behind them). Controls and editors
//! are solid, in neutral greys with one accent colour, which follows the system's. Dark or light follows the system
//! too (`Appearance`).
// spec: ui/etched-design.md

use crate::design::DesignMap;
use crate::Color;
use std::sync::RwLock;

/// The system's look, which Etched follows: dark or light, and the accent colour (the system's own, unshaded).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Appearance {
    pub dark: bool,
    pub accent: Color,
}

/// The accent when the system has none to give.
pub const DEFAULT_ACCENT: Color = Color::rgb(0x0078d4);

impl Default for Appearance {
    fn default() -> Self {
        Appearance { dark: true, accent: DEFAULT_ACCENT }
    }
}

static APPEARANCE: RwLock<Option<Appearance>> = RwLock::new(None);

/// The system look Etched draws with (set by the app as it reads it; the default until then).
pub fn appearance() -> Appearance {
    APPEARANCE.read().ok().and_then(|g| *g).unwrap_or_default()
}

pub fn set_appearance(a: Appearance) {
    if let Ok(mut g) = APPEARANCE.write() {
        *g = Some(a);
    }
}

/// The accent's shades: the system colour at a set lightness (hue and saturation kept). On dark surfaces controls use a
/// light shade, on light ones a dark shade, so the accent reads on either. A grey accent (black, white, graphite)
/// stays grey; a dull colour is brought up to some colour.
pub fn accent_shade(accent: Color, l: f32) -> Color {
    let chroma = f32::from(accent.r.max(accent.g).max(accent.b) - accent.r.min(accent.g).min(accent.b)) / 255.0;
    let (h, s, _) = accent.hsl();
    Color::from_hsl_pub(h, if chroma < 0.06 { 0.0 } else { s.max(0.35) }, l, 255)
}

/// How the backdrop is tinted: the blurred wallpaper's lightness is pulled to `tint`'s by `luminosity` (its colour
/// kept by `chroma`), then `tint` is laid over it at `tint_opacity`, then the result lightened (`brightness` > 0) or
/// darkened (< 0).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frost {
    pub tint: Color,
    pub tint_opacity: f32,
    pub luminosity: f32,
    /// How much of the wallpaper's colour survives (0 = grey; above 1 more vivid).
    pub chroma: f32,
    /// Grain (0..1 of a level step; 0 = none).
    pub noise: f32,
    /// -1..1: toward black / white, after the tint.
    pub brightness: f32,
}

/// A tunable value of the look (the Etched debug panel, views/etcheddebug.rs): its range and its default in dark and
/// in light.
pub struct Param {
    pub name: &'static str,
    pub dark: f32,
    pub light: f32,
    pub min: f32,
    pub max: f32,
    pub decimals: usize,
    pub unit: &'static str,
    pub hint: &'static str,
}

#[allow(clippy::too_many_arguments)]
const fn p(name: &'static str, dark: f32, light: f32, min: f32, max: f32, decimals: usize, unit: &'static str, hint: &'static str) -> Param {
    Param { name, dark, light, min, max, decimals, unit, hint }
}

/// Indices into `PARAMS`.
pub mod k {
    pub const BACK_BRIGHTNESS: usize = 0;
    pub const BACK_FROST: usize = 1;
    pub const BACK_OPACITY: usize = 2;
    pub const BACK_LUMINOSITY: usize = 3;
    pub const BACK_COLOUR: usize = 4;
    pub const BACK_GRAIN: usize = 5;
    pub const PANEL_BRIGHTNESS: usize = 6;
    pub const PANEL_FROST: usize = 7;
    pub const PANEL_OPACITY: usize = 8;
    pub const PANEL_LUMINOSITY: usize = 9;
    pub const PANEL_COLOUR: usize = 10;
    pub const PANEL_GRAIN: usize = 11;
    pub const CONTROLS: usize = 12;
    pub const EDITORS: usize = 13;
    pub const SURFACES: usize = 14;
}

/// What the debug panel tunes (spec: ui/etched-design.md §9). The first six make the backdrop, the next six the
/// frosted panels; the last two set how see-through controls and the editors' surfaces are.
pub const PARAMS: [Param; 15] = [
    p("Backdrop brightness", 0.0, 0.0, -0.5, 0.5, 2, "", "Lightens (right) or darkens (left) the backdrop after its tint"),
    p("Backdrop frost", 36.0, 36.0, 0.0, 160.0, 0, "px", "How far the desktop picture is blurred (on the screen)"),
    p("Backdrop opacity", 0.5, 0.4, 0.0, 1.0, 2, "", "How much the base colour covers the desktop (0 = the picture shows fully)"),
    p("Backdrop luminosity", 0.7, 0.65, 0.0, 1.0, 2, "", "How far the picture's lightness is pulled to the base colour's"),
    p("Backdrop colour", 1.1, 1.0, 0.0, 2.0, 2, "x", "How much of the picture's colour is kept"),
    p("Backdrop grain", 0.02, 0.02, 0.0, 0.12, 3, "", "Noise over the backdrop"),
    p("Panel brightness", 0.0, 0.0, -0.5, 0.5, 2, "", "Lightens or darkens menus and popups"),
    p("Panel frost", 30.0, 30.0, 0.0, 60.0, 0, "px", "How far what is behind a menu is blurred"),
    p("Panel opacity", 0.45, 0.35, 0.0, 1.0, 2, "", "How much the panel colour covers what is behind a menu"),
    p("Panel luminosity", 0.9, 0.8, 0.0, 1.0, 2, "", "How far what is behind a menu is pulled to the panel colour's lightness"),
    p("Panel colour", 1.0, 1.0, 0.0, 2.0, 2, "x", "How much colour from behind a menu is kept"),
    p("Panel grain", 0.03, 0.03, 0.0, 0.12, 3, "", "Noise over menus and popups"),
    p("Control fill", 1.0, 1.0, 0.0, 3.0, 2, "x", "Strength of the see-through fills of buttons, fields and rows"),
    p("Editor opacity", 0.55, 0.6, 0.0, 1.0, 2, "", "How solid an editor window's body is over the backdrop (1 = opaque)"),
    p("Surface tint", 0.5, 0.5, 0.0, 1.0, 2, "", "How solid the surfaces inside an editor are over its body (grids, track headers, strips)"),
];

/// The editors' surfaces (`overrides`): see-through, so the backdrop shows through the whole window. A window's
/// body (`window_body`) takes the tuned editor opacity, the surfaces inside it the surface tint. Lines and small parts
/// stay solid.
pub const EDITOR_SURFACES: [&str; 18] = [
    "grid",
    "grid_alt",
    "grid_gap",
    "ruler",
    "track_header",
    "window_body",
    "mixer_bg",
    "mixer_strip",
    "mixer_strip_dark",
    "pr_row",
    "pr_row_black",
    "pr_row_sel",
    "pr_row_black_sel",
    "rack_inset",
    "rack_row_box",
    "scrollbar",
    "splitter",
    "picker",
];

/// The look's tunable values, one set for dark and one for light (`PARAMS`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tuning {
    pub dark: [f32; PARAMS.len()],
    pub light: [f32; PARAMS.len()],
}

impl Default for Tuning {
    fn default() -> Self {
        Tuning { dark: PARAMS.map(|p| p.dark), light: PARAMS.map(|p| p.light) }
    }
}

impl Tuning {
    pub fn get(&self, dark: bool, i: usize) -> f32 {
        if dark { self.dark[i] } else { self.light[i] }
    }
    pub fn default_of(dark: bool, i: usize) -> f32 {
        if dark { PARAMS[i].dark } else { PARAMS[i].light }
    }
    pub fn set(&mut self, dark: bool, i: usize, x: f32) {
        let p = &PARAMS[i];
        // Dividing by a power of ten gives back exactly the literal a value was written as (defaults, etched.txt).
        let scale = 10f32.powi(p.decimals as i32);
        let v = ((x.clamp(p.min, p.max) * scale).round() / scale).clamp(p.min, p.max);
        if dark { self.dark[i] = v } else { self.light[i] = v }
    }
    pub fn norm(&self, dark: bool, i: usize) -> f32 {
        (self.get(dark, i) - PARAMS[i].min) / (PARAMS[i].max - PARAMS[i].min)
    }
    pub fn text(&self, dark: bool, i: usize) -> String {
        let p = &PARAMS[i];
        format!("{:.*} {}", p.decimals, self.get(dark, i), p.unit).trim_end().to_string()
    }
    /// `dark.<name> = value` and `light.<name> = value` lines (what the panel's Save writes, `load` reads).
    pub fn dump(&self) -> String {
        let mut out = String::new();
        for (mode, v) in [("dark", &self.dark), ("light", &self.light)] {
            for (p, x) in PARAMS.iter().zip(v) {
                out += &format!("{mode}.{} = {x:.*}\n", p.name, p.decimals);
            }
        }
        out
    }
    pub fn load(&mut self, text: &str) {
        for line in text.lines() {
            let Some((name, val)) = line.split_once('=') else { continue };
            let Some((mode, name)) = name.trim().split_once('.') else { continue };
            if let (Some(i), Ok(x)) = (PARAMS.iter().position(|p| p.name == name.trim()), val.trim().parse::<f32>()) {
                self.set(mode != "light", i, x);
            }
        }
    }
}

static TUNING: RwLock<Option<Tuning>> = RwLock::new(None);

/// The look's values now (the defaults until the app loads `etched.txt` or the debug panel changes one).
pub fn tuning() -> Tuning {
    TUNING.read().ok().and_then(|g| *g).unwrap_or_default()
}

pub fn set_tuning(t: Tuning) {
    if let Ok(mut g) = TUNING.write() {
        *g = Some(t);
    }
}

/// Value `i` of the current tuning in dark or light.
pub fn tuned(dark: bool, i: usize) -> f32 {
    tuning().get(dark, i)
}

/// The window backdrop (the wallpaper behind the window, blurred).
pub fn backdrop_frost(dark: bool) -> Frost {
    let v = |i| tuned(dark, i);
    Frost {
        tint: base(dark),
        tint_opacity: v(k::BACK_OPACITY),
        luminosity: v(k::BACK_LUMINOSITY),
        chroma: v(k::BACK_COLOUR),
        noise: v(k::BACK_GRAIN),
        brightness: v(k::BACK_BRIGHTNESS),
    }
}

/// How far the backdrop is blurred, in screen px.
pub fn backdrop_blur(dark: bool) -> f32 {
    tuned(dark, k::BACK_FROST)
}

/// Frosted panels (menus and popups) over the app's own content.
pub fn panel_frost(dark: bool) -> Frost {
    let v = |i| tuned(dark, i);
    Frost {
        tint: if dark { Color::rgb(0x2c2c2c) } else { Color::rgb(0xfcfcfc) },
        tint_opacity: v(k::PANEL_OPACITY),
        luminosity: v(k::PANEL_LUMINOSITY),
        chroma: v(k::PANEL_COLOUR),
        noise: v(k::PANEL_GRAIN),
        brightness: v(k::PANEL_BRIGHTNESS),
    }
}

/// Blur radius of the frosted panels, in logical px.
pub fn panel_blur(dark: bool) -> f32 {
    tuned(dark, k::PANEL_FROST)
}

/// The strength of the controls' see-through fills (1 = as designed).
pub fn control_fill(dark: bool) -> f32 {
    tuned(dark, k::CONTROLS)
}

/// Etched's base colour: the backdrop's tint, what see-through tokens are mixed with when there is no backdrop.
pub fn base(dark: bool) -> Color {
    if dark { Color::rgb(0x202020) } else { Color::rgb(0xf3f3f3) }
}

/// Lightness ramps for the role-mapped literals (`Palette::s`): classic lightness -> Etched lightness. Dark: the
/// classic toolbar (0.19) lands on the base (#202020), raised greys (0.33) on a control hover; light: wells and lists
/// go near-white, the base on #f3f3f3, raised controls white, and the light classic greys (glyph-like) turn dark.
const DARK_RAMP: &[(f32, f32)] = &[
    (0.0, 0.065),
    (0.06, 0.085),
    (0.10, 0.1),
    (0.155, 0.11),
    (0.19, 0.125),
    (0.23, 0.155),
    (0.27, 0.18),
    (0.33, 0.22),
    (0.5, 0.42),
    (0.8, 0.8),
    (1.0, 1.0),
];
const LIGHT_RAMP: &[(f32, f32)] = &[
    (0.0, 0.92),
    (0.06, 0.95),
    (0.10, 0.965),
    (0.155, 0.955),
    (0.19, 0.953),
    (0.23, 0.975),
    (0.33, 0.99),
    (0.5, 0.62),
    (0.8, 0.28),
    (1.0, 0.1),
];

/// The role map of the current appearance (`design::palette`).
pub fn map(a: Appearance) -> DesignMap {
    let k = accent(a);
    if a.dark {
        DesignMap { ramp: DARK_RAMP, tint: (0.0, 0.0), accent: k, highlight: k, mute: k, text: Color::rgb(0xffffff), text_bg: base(true) }
    } else {
        DesignMap { ramp: LIGHT_RAMP, tint: (0.0, 0.0), accent: k, highlight: k, mute: k, text: Color::rgb(0x1a1a1a), text_bg: base(false) }
    }
}

/// Lemon's lightness -> Etched's, for Lemon's own greys (`brand`): Lemon's toolbar (#3b3b3b) on the base, its
/// dark highlight (#666666) on a control hover; on light, Lemon's dark wells go near-white and its light greys dark.
const BRAND_DARK: &[(f32, f32)] = &[
    (0.0, 0.0),
    (0.07, 0.065),
    (0.10, 0.085),
    (0.15, 0.11),
    (0.231, 0.125),
    (0.285, 0.165),
    (0.33, 0.19),
    (0.40, 0.22),
    (0.56, 0.42),
    (0.84, 0.84),
    (1.0, 1.0),
];
const BRAND_LIGHT: &[(f32, f32)] = &[
    (0.0, 0.85),
    (0.07, 0.93),
    (0.10, 0.95),
    (0.15, 0.96),
    (0.231, 0.953),
    (0.285, 0.975),
    (0.33, 0.985),
    (0.40, 1.0),
    (0.56, 0.62),
    (0.84, 0.28),
    (1.0, 0.1),
];

/// Lemon's own colours (the constants the widget kit draws with) in Etched's: its yellows take the accent, its greys
/// Etched's, its text Etched's text.
pub fn brand(a: Appearance) -> crate::design::Retint {
    let m = map(a);
    let lemon = crate::design::lemon::TEXT;
    crate::design::Retint {
        surface: crate::fl::DARK,
        key: crate::design::lemon::MAIN,
        accent: Some(m.accent),
        highlight: Some(m.accent),
        mute: Some(m.accent),
        text_from: (lemon, crate::design::lemon::DARK),
        text_to: (m.text, m.text_bg),
        ramp: Some(if a.dark { BRAND_DARK } else { BRAND_LIGHT }),
    }
}

/// The accent fill controls use: a light shade on dark, a dark one on light.
pub fn accent(a: Appearance) -> Color {
    accent_shade(a.accent, if a.dark { 0.65 } else { 0.38 })
}

/// Text and glyphs on the accent fill.
pub fn on_accent(a: Appearance) -> Color {
    if a.dark { Color::rgb(0x000000) } else { Color::rgb(0xffffff) }
}

/// Tokens Etched sets outright (all of them), for the current appearance. See-through tokens have alpha 0 (the
/// backdrop shows) or a low alpha (a veil over it); their colour is what they mix toward without one. The editors'
/// surfaces take the tuned editor opacity.
pub fn overrides(a: Appearance) -> Vec<(&'static str, Color)> {
    let alpha = |i: usize| (tuned(a.dark, i) * 255.0).round().clamp(0.0, 255.0) as u8;
    let (body, inner) = (alpha(k::EDITORS), alpha(k::SURFACES));
    let mut v = own(a);
    for (name, c) in &mut v {
        if *name == "window_body" {
            *c = c.with_alpha(body);
        } else if EDITOR_SURFACES.contains(name) {
            *c = c.with_alpha(inner);
        }
    }
    v
}

fn own(a: Appearance) -> Vec<(&'static str, Color)> {
    let c = Color::rgb;
    let k = accent(a);
    let on = on_accent(a);
    let kc = |l: f32| accent_shade(a.accent, l);
    // A colour with alpha (a veil): white on dark, black on light.
    let veil = |alpha: u8| if a.dark { c(0xffffff).with_alpha(alpha) } else { c(0x000000).with_alpha(alpha) };
    let clear = base(a.dark).with_alpha(0);
    if a.dark {
        let hi = k.mix(c(0xffffff), 0.12);
        let soft = |g: u32, t: f32| c(g).mix(k, t);
        vec![
            ("toolbar", clear),
            ("toolbar_line", veil(0x15)),
            ("toolbar_edge", veil(0x12)),
            ("sunken", c(0x1c1c1c)),
            ("sunken_dark", c(0x171717)),
            ("time_panel", c(0x1c1c1c)),
            ("menu_panel", c(0x2c2c2c)),
            ("menu_text", c(0xffffff)),
            ("news_panel", c(0x2b2b2b)),
            ("button", c(0x2d2d2d)),
            ("button_hi", c(0x383838)),
            ("glyph", c(0xffffff)),
            ("glyph_dim", c(0x9d9d9d)),
            ("text", c(0xffffff)),
            ("text_dim", c(0xababab)),
            ("text_faint", c(0x787878)),
            ("on_top", hi),
            ("on_bottom", k),
            ("on_glyph", on),
            ("pill", k),
            ("pill_text", on),
            ("record", c(0xff6b74)),
            ("tempo_box", c(0x1c1c1c)),
            ("accent", k),
            ("accent_dim", soft(0x2b2b2b, 0.3)),
            ("workspace", clear),
            ("watermark", veil(0x0e)),
            ("version_text", veil(0x50)),
            ("browser_title", clear),
            ("browser_tabs", clear),
            ("browser_tab_outline", k),
            ("browser_list", clear),
            ("browser_row_hi", veil(0x0f)),
            ("browser_folder", c(0xffffff)),
            ("browser_footer", clear),
            ("browser_search", k),
            ("cat_project", k),
            ("cat_recent", c(0xb7dfa0)),
            ("cat_plugin_db", c(0xa3cdee)),
            ("cat_plugin_presets", c(0xe8b4d2)),
            ("cat_channel_presets", c(0xd2bcee)),
            ("cat_mixer_presets", c(0xbcc4f2)),
            ("splitter", c(0x1c1c1c)),
            ("splitter_dark", c(0x141414)),
            ("window_title", c(0x2b2b2b)),
            ("window_title_float", c(0x2b2b2b)),
            ("window_title_hi", c(0x323232)),
            ("window_body", c(0x272727)),
            ("window_border", c(0x3d3d3d)),
            ("picker", c(0x2b2b2b)),
            ("track_header", c(0x2b2b2b)),
            ("overview", c(0x3a3a3a)),
            ("ruler", c(0x1c1c1c)),
            ("ruler_text", c(0x9d9d9d)),
            ("grid", c(0x232323)),
            ("grid_alt", c(0x272727)),
            ("grid_gap", c(0x1c1c1c)),
            ("grid_bar", c(0x141414)),
            ("scrollbar", c(0x202020)),
            ("playhead", k),
            ("mixer_bg", c(0x1c1c1c)),
            ("mixer_strip", c(0x2b2b2b)),
            ("mixer_strip_dark", c(0x232323)),
            ("mixer_panel", c(0x2d2d2d)),
            ("slot", c(0x232323)),
            ("slot_text", c(0x717171)),
            ("slot_text_used", c(0xe6e6e6)),
            ("fader_handle", c(0x5a5a5a)),
            ("meter_low", kc(0.5)),
            ("meter_high", kc(0.8)),
            ("meter_clip", c(0xff6b74)),
            ("rack_row_box", c(0x2d2d2d)),
            ("name_button", c(0x383838)),
            ("name_button_sel", c(0x4a4a4a)),
            ("name_text", c(0xf0f0f0)),
            ("rack_inset", c(0x2b2b2b)),
            ("rack_inset_border", c(0x1c1c1c)),
            ("rack_control_border", c(0x1a1a1a)),
            ("step_a", c(0x3a3a3a)),
            ("step_b", soft(0x3a3a3a, 0.25)),
            ("step_dim_a", c(0x2e2e2e)),
            ("step_dim_b", soft(0x2e2e2e, 0.15)),
            ("step_dim_border", c(0x232323)),
            ("step_on", k),
            ("add_hover", k),
            ("menu_drop", c(0x2c2c2c)),
            ("menu_drop_hi", c(0x383838)),
            ("menu_drop_text", c(0xffffff)),
            ("menu_drop_disabled", c(0x717171)),
            ("menu_pop", c(0x2c2c2c)),
            ("menu_pop_border", c(0x1a1a1a)),
            ("menu_pop_hi", c(0x383838)),
            ("menu_pop_text", c(0xffffff)),
            ("menu_pop_shortcut", c(0x9d9d9d)),
            ("menu_pop_dim", c(0x717171)),
            ("menu_pop_head", c(0x262626)),
            ("menu_pop_head_text", c(0x9d9d9d)),
            ("clip", k.mix(c(0x202020), 0.5)),
            ("pr_row", c(0x262626)),
            ("pr_row_black", c(0x202020)),
            ("pr_row_sel", soft(0x262626, 0.06)),
            ("pr_row_black_sel", soft(0x202020, 0.06)),
            ("pr_bar", c(0x141414)),
            ("note_top", k),
            ("note_bottom", k.shade(-0.08)),
            ("note_rim", k.mix(c(0x000000), 0.45)),
            ("note_text", on),
            ("note_sel_top", c(0xffffff)),
            ("note_sel_bottom", kc(0.85)),
            ("note_sel_rim", k.mix(c(0x000000), 0.6)),
            ("note_sel_text", c(0x000000)),
            ("key_white", c(0xededed)),
            ("key_white_left", c(0xd2d2d2)),
            ("key_black", c(0x2d2d2d)),
            ("key_black_left", c(0x1c1c1c)),
            ("key_c", c(0xdddddd)),
            ("key_label", k.mix(c(0x000000), 0.45)),
            ("key_lit", kc(0.85)),
            ("key_lit_left", k),
            ("key_lit_edge", kc(0.45)),
            ("key_black_lit", k),
            ("key_black_lit_left", k.mix(c(0x000000), 0.35)),
            ("velocity", k),
            ("velocity_dot", on),
        ]
    } else {
        let hi = k.mix(c(0xffffff), 0.12);
        let soft = |g: u32, t: f32| c(g).mix(k, t);
        vec![
            ("toolbar", clear),
            ("toolbar_line", veil(0x0f)),
            ("toolbar_edge", veil(0x0f)),
            ("sunken", c(0xfbfbfb)),
            ("sunken_dark", c(0xececec)),
            ("time_panel", c(0xffffff)),
            ("menu_panel", c(0xf9f9f9)),
            ("menu_text", c(0x1a1a1a)),
            ("news_panel", c(0xf9f9f9)),
            ("button", c(0xfbfbfb)),
            ("button_hi", c(0xf3f3f3)),
            ("glyph", c(0x1a1a1a)),
            ("glyph_dim", c(0x6e6e6e)),
            ("text", c(0x1a1a1a)),
            ("text_dim", c(0x5f5f5f)),
            ("text_faint", c(0x8f8f8f)),
            ("on_top", hi),
            ("on_bottom", k),
            ("on_glyph", on),
            ("pill", k),
            ("pill_text", on),
            ("record", c(0xc42b1c)),
            ("tempo_box", c(0xffffff)),
            ("accent", k),
            ("accent_dim", soft(0xf3f3f3, 0.3)),
            ("workspace", clear),
            ("watermark", veil(0x0c)),
            ("version_text", veil(0x60)),
            ("browser_title", clear),
            ("browser_tabs", clear),
            ("browser_tab_outline", k),
            ("browser_list", clear),
            ("browser_row_hi", veil(0x09)),
            ("browser_folder", c(0x1a1a1a)),
            ("browser_footer", clear),
            ("browser_search", k),
            ("cat_project", k),
            ("cat_recent", c(0x3f7d2b)),
            ("cat_plugin_db", c(0x2f6a9c)),
            ("cat_plugin_presets", c(0x9c4a78)),
            ("cat_channel_presets", c(0x7552a0)),
            ("cat_mixer_presets", c(0x4f58a6)),
            ("splitter", c(0xe5e5e5)),
            ("splitter_dark", c(0xd6d6d6)),
            ("window_title", c(0xf9f9f9)),
            ("window_title_float", c(0xf9f9f9)),
            ("window_title_hi", c(0xffffff)),
            ("window_body", c(0xf9f9f9)),
            ("window_border", c(0xcfcfcf)),
            ("picker", c(0xffffff)),
            ("track_header", c(0xf9f9f9)),
            ("overview", c(0xe0e0e0)),
            ("ruler", c(0xfbfbfb)),
            ("ruler_text", c(0x616161)),
            ("grid", c(0xfafafa)),
            ("grid_alt", c(0xf3f3f3)),
            ("grid_gap", c(0xe8e8e8)),
            ("grid_bar", c(0xc8c8c8)),
            ("scrollbar", c(0xededed)),
            ("playhead", k),
            ("mixer_bg", c(0xebebeb)),
            ("mixer_strip", c(0xf9f9f9)),
            ("mixer_strip_dark", c(0xf0f0f0)),
            ("mixer_panel", c(0xfbfbfb)),
            ("slot", c(0xffffff)),
            ("slot_text", c(0x9a9a9a)),
            ("slot_text_used", c(0x1a1a1a)),
            ("fader_handle", c(0x9a9a9a)),
            ("meter_low", kc(0.45)),
            ("meter_high", kc(0.7)),
            ("meter_clip", c(0xc42b1c)),
            ("rack_row_box", c(0xf9f9f9)),
            ("name_button", c(0xffffff)),
            ("name_button_sel", c(0xe0e0e0)),
            ("name_text", c(0x1a1a1a)),
            ("rack_inset", c(0xfbfbfb)),
            ("rack_inset_border", c(0xe0e0e0)),
            ("rack_control_border", c(0xd0d0d0)),
            ("step_a", c(0xe6e6e6)),
            ("step_b", soft(0xe6e6e6, 0.25)),
            ("step_dim_a", c(0xededed)),
            ("step_dim_b", soft(0xededed, 0.15)),
            ("step_dim_border", c(0xdadada)),
            ("step_on", k),
            ("add_hover", k),
            ("menu_drop", c(0xf9f9f9)),
            ("menu_drop_hi", c(0xebebeb)),
            ("menu_drop_text", c(0x1a1a1a)),
            ("menu_drop_disabled", c(0x9a9a9a)),
            ("menu_pop", c(0xf9f9f9)),
            ("menu_pop_border", c(0xd6d6d6)),
            ("menu_pop_hi", c(0xebebeb)),
            ("menu_pop_text", c(0x1a1a1a)),
            ("menu_pop_shortcut", c(0x616161)),
            ("menu_pop_dim", c(0x9a9a9a)),
            ("menu_pop_head", c(0xf0f0f0)),
            ("menu_pop_head_text", c(0x616161)),
            ("clip", k.mix(c(0xffffff), 0.55)),
            ("pr_row", c(0xfafafa)),
            ("pr_row_black", c(0xf0f0f0)),
            ("pr_row_sel", soft(0xfafafa, 0.06)),
            ("pr_row_black_sel", soft(0xf0f0f0, 0.06)),
            ("pr_bar", c(0xc8c8c8)),
            ("note_top", k),
            ("note_bottom", k.shade(-0.08)),
            ("note_rim", k.mix(c(0x000000), 0.35)),
            ("note_text", on),
            ("note_sel_top", k.mix(c(0x000000), 0.35)),
            ("note_sel_bottom", k.mix(c(0x000000), 0.45)),
            ("note_sel_rim", k.mix(c(0x000000), 0.7)),
            ("note_sel_text", c(0xffffff)),
            ("key_white", c(0xffffff)),
            ("key_white_left", c(0xe6e6e6)),
            ("key_black", c(0x3a3a3a)),
            ("key_black_left", c(0x202020)),
            ("key_c", c(0xf0f0f0)),
            ("key_label", k),
            ("key_lit", kc(0.8)),
            ("key_lit_left", kc(0.6)),
            ("key_lit_edge", k),
            ("key_black_lit", k),
            ("key_black_lit_left", k.mix(c(0x000000), 0.35)),
            ("velocity", k),
            ("velocity_dot", on),
        ]
    }
}

/// `p` with its see-through tokens laid over Etched's base (what a tile preview or a plugin window shows, having no
/// backdrop).
pub fn flatten(p: &mut crate::Palette, dark: bool) {
    let b = base(dark);
    for name in crate::Palette::NAMES {
        let slot = p.get_mut(name).expect("token");
        if slot.a < 255 {
            *slot = over(*slot, b);
        }
    }
}

/// `c` (with alpha) over the opaque `bg`.
pub fn over(c: Color, bg: Color) -> Color {
    bg.mix(c.with_alpha(255), f32::from(c.a) / 255.0).with_alpha(255)
}

/// Whether the palette has see-through surfaces (the app then draws the backdrop under the frame).
pub fn see_through(p: &crate::Palette) -> bool {
    p.workspace.a < 255
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grey_accent_stays_grey_and_a_colour_keeps_its_hue() {
        for grey in [0x000000, 0x6b6b6b, 0xffffff] {
            let c = accent_shade(Color::rgb(grey), 0.65);
            assert!(c.r == c.g && c.g == c.b, "{grey:06x}: {c:?}");
        }
        let blue = accent_shade(DEFAULT_ACCENT, 0.65);
        assert!(blue.b > blue.g && blue.g > blue.r, "{blue:?}");
    }

    #[test]
    fn tuning_round_trips_through_its_text() {
        let mut t = Tuning::default();
        t.set(true, k::BACK_OPACITY, 0.25);
        t.set(false, k::PANEL_FROST, 12.0);
        let mut u = Tuning::default();
        u.load(&t.dump());
        assert_eq!(t, u);
        assert_eq!(u.get(true, k::BACK_OPACITY), 0.25);
        assert_eq!(u.get(false, k::BACK_OPACITY), PARAMS[k::BACK_OPACITY].light);
    }

    #[test]
    fn flattened_colours_are_opaque() {
        let mut p = crate::Palette::default();
        p.workspace = p.workspace.with_alpha(0);
        p.toolbar = Color::rgb(0xffffff).with_alpha(0x20);
        assert!(see_through(&p));
        flatten(&mut p, true);
        assert!(!see_through(&p) && p.toolbar.a == 255);
        assert_eq!(p.workspace, base(true), "a clear token shows the base");
    }
}
