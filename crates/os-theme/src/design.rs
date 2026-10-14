// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Designs: complete looks (colours and shapes) over the same layout.
//!
//! `Classic` is the measured default look. `Lemon` is Lemon Studio's own look: neutral greys, one lemon-yellow brand
//! colour and rounded-square controls (knobs stay round). It re-colours every token and every role-mapped literal
//! (`Palette::s/a/h/tx/led`) through a `DesignMap` (a neutral lightness ramp plus key colours), sets the tokens the
//! map can't get right outright, and tells the widget kit which shape language to draw (`Style`).
// spec: ui/lemon-design.md

// PLEASE. DO NOT USE THE LEMON THEME IN ANOTHER PROJECT. IT IS LEMON STUDIO'S OWN THEME

// This code is a part of L e m o n S t u d i o. Please don't upload it to Art Craft!

use crate::fl::{self, FlTheme};
use crate::{Color, Palette};

/// A complete look.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Design {
    /// The measured default look.
    #[default]
    Classic,
    /// Lemon Studio's own look: lemon on neutral greys, rounded squares.
    Lemon,
}

impl Design {
    pub const ALL: [Design; 2] = [Design::Classic, Design::Lemon];

    pub fn name(self) -> &'static str {
        match self {
            Design::Classic => "Classic",
            Design::Lemon => "Lemon",
        }
    }

    pub fn parse(s: &str) -> Option<Design> {
        Design::ALL.into_iter().find(|d| d.name().eq_ignore_ascii_case(s.trim()))
    }

    pub fn next(self) -> Design {
        let i = Design::ALL.iter().position(|d| *d == self).unwrap_or(0);
        Design::ALL[(i + 1) % Design::ALL.len()]
    }

    pub fn style(self) -> Style {
        match self {
            Design::Classic => Style::CLASSIC,
            Design::Lemon => Style::LEMON,
        }
    }
}

/// Shape language of a design. Sizes that affect layout are not here: the layout never changes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    pub design: Design,
    /// Corner radius of buttons, boxes and combos.
    pub radius: f32,
    /// Corner radius of small parts (steps, slots, LEDs, notes).
    pub radius_small: f32,
    /// Corner radius of panels, popups and floating windows.
    pub radius_panel: f32,
}

impl Style {
    pub const CLASSIC: Style = Style { design: Design::Classic, radius: 3.0, radius_small: 2.5, radius_panel: 4.0 };
    pub const LEMON: Style = Style { design: Design::Lemon, radius: 6.0, radius_small: 4.0, radius_panel: 10.0 };

    pub fn is(&self, d: Design) -> bool {
        self.design == d
    }

    pub fn lemon(&self) -> bool {
        self.design == Design::Lemon
    }
}

impl Default for Style {
    fn default() -> Self {
        Style::CLASSIC
    }
}

/// How a design re-colours the measured (Classic) colours, by role.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DesignMap {
    /// Surface lightness: (classic lightness, design lightness) points, ascending input.
    pub ramp: &'static [(f32, f32)],
    /// Hue (degrees) and saturation of the design's neutrals.
    pub tint: (f32, f32),
    /// Classic key colours -> design key colours.
    pub accent: Color,
    pub highlight: Color,
    pub mute: Color,
    /// Text: the design's text colour and the surface it is read on (dimmer classic text moves toward it).
    pub text: Color,
    pub text_bg: Color,
}

/// Classic text and its toolbar background (the reference for `DesignMap::text_of`).
const CLASSIC_TEXT_L: f32 = 0.85;
const CLASSIC_BG_L: f32 = 0.19;

impl DesignMap {
    fn ramp_l(&self, l: f32) -> f32 {
        let r = self.ramp;
        if l <= r[0].0 {
            return r[0].1;
        }
        for w in r.windows(2) {
            let ((a, fa), (b, fb)) = (w[0], w[1]);
            if l <= b {
                let t = if b > a { (l - a) / (b - a) } else { 1.0 };
                return fa + (fb - fa) * t;
            }
        }
        r[r.len() - 1].1
    }

    /// A classic surface colour in this design: lightness through the ramp, neutrals take the design's tint;
    /// clearly coloured surfaces keep their hue with reduced saturation.
    pub fn surface(&self, c: Color) -> Color {
        let (h, s, l) = c.hsl();
        let l2 = self.ramp_l(l).clamp(0.0, 1.0);
        let (th, ts) = self.tint;
        // Coloured = visible chroma (HSL saturation overrates the stock look's blue-tinted dark greys).
        let chroma = f32::from(c.r.max(c.g).max(c.b) - c.r.min(c.g).min(c.b)) / 255.0;
        let (h2, s2) = if chroma > 0.09 { (h, (s * 0.6).max(ts)) } else { (th, ts) };
        Color::from_hsl_pub(h2, s2.clamp(0.0, 1.0), l2, c.a)
    }

    pub fn accent_of(&self, c: Color) -> Color {
        fl::rekey(c, classic_keys().selected, self.accent)
    }
    pub fn highlight_of(&self, c: Color) -> Color {
        fl::rekey(c, classic_keys().highlight, self.highlight)
    }
    pub fn mute_of(&self, c: Color) -> Color {
        fl::rekey(c, classic_keys().mute, self.mute)
    }

    /// A classic text colour: its position between the classic background and text is kept between the
    /// design's text background and text colour (so dim text stays dim).
    pub fn text_of(&self, c: Color) -> Color {
        let (_, s, l) = c.hsl();
        let t = ((l - CLASSIC_BG_L) / (CLASSIC_TEXT_L - CLASSIC_BG_L)).clamp(-0.2, 1.25);
        let (th, ts, tl) = self.text.hsl();
        let bl = self.text_bg.hsl().2;
        let l2 = (bl + (tl - bl) * t).clamp(0.0, 1.0);
        Color::from_hsl_pub(th, if s > 0.25 { s * 0.6 } else { ts }, l2, c.a)
    }
}

/// Key colours of the stock look (Classic was measured with them).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Keys {
    pub selected: Color,
    pub highlight: Color,
    pub text: Color,
    pub mute: Color,
}

pub fn classic_keys() -> Keys {
    let d = FlTheme::dark();
    Keys {
        selected: d.selected.unwrap_or(Color::rgb(0xa8e44a)),
        highlight: d.highlight.unwrap_or(Color::rgb(0xfb9244)),
        text: d.text_color.unwrap_or(Color::rgb(0xc9d0d3)),
        mute: d.mute.unwrap_or(Color::rgb(0x78d008)),
    }
}

/// The Lemon brand palette (owner's base colours, 2026-10-12) and the in-between shades derived from it.
pub mod lemon {
    use crate::Color;
    /// Main colour (pale yellow): "on" controls, selection, the playhead, lit steps.
    pub const MAIN: Color = Color::rgb(0xffe066);
    /// Highlight: the bright top of lit controls, hover on lit controls, selected notes.
    pub const HIGHLIGHT: Color = Color::rgb(0xfff1b8);
    /// Dark: the main surface (toolbar).
    pub const DARK: Color = Color::rgb(0x3b3b3b);
    /// Dark highlight: raised faces on hover, hairlines, knob tracks.
    pub const DARK_HIGHLIGHT: Color = Color::rgb(0x666666);
    /// Coloured dark: glyphs and text on lemon, unlit LEDs, hovered menu rows.
    pub const COLORED_DARK: Color = Color::rgb(0x473900);
    /// In-between: a darker lemon for edges of lit controls and meter bodies.
    pub const DEEP: Color = Color::rgb(0xc9a92c);
    /// Text on the greys.
    pub const TEXT: Color = Color::rgb(0xececec);
}

/// Lemon: Classic lightness -> Lemon lightness. Classic's toolbar (0.19) lands on DARK (#3b3b3b, 0.231) and the
/// 0.33 raised greys on DARK_HIGHLIGHT (#666666, 0.40); the near-black lists and rulers lift to about #151515.
const LEMON_RAMP: &[(f32, f32)] = &[
    (0.0, 0.07),
    (0.03, 0.085),
    (0.06, 0.10),
    (0.10, 0.125),
    (0.13, 0.15),
    (0.155, 0.18),
    (0.19, 0.231),
    (0.23, 0.285),
    (0.27, 0.33),
    (0.33, 0.40),
    (0.5, 0.56),
    (0.8, 0.84),
    (1.0, 1.0),
];

fn map_of(d: Design) -> Option<DesignMap> {// This code is a part of L e m o n S t u d i o. Please don't upload it to Art Craft!
    match d {
        Design::Classic => None,
        Design::Lemon => Some(DesignMap {
            ramp: LEMON_RAMP,
            tint: (48.0, 0.0),
            accent: lemon::MAIN,
            highlight: lemon::MAIN,
            mute: lemon::MAIN,
            text: lemon::TEXT,
            text_bg: lemon::DARK,
        }),
    }
}

/// Tokens a design sets outright (the ones the role map can't get right: signal colours, notes, keys, steps).
fn overrides(d: Design) -> Vec<(&'static str, Color)> {
    let c = Color::rgb;
    use lemon::*;
    match d {
        Design::Classic => vec![],
        Design::Lemon => vec![
            ("toolbar", DARK),
            ("toolbar_line", c(0x2a2a2a)),
            ("toolbar_edge", c(0x4a4a4a)),
            ("sunken", c(0x2b2b2b)),
            ("sunken_dark", c(0x232323)),
            ("time_panel", c(0x232323)),
            ("menu_panel", c(0x333333)),
            ("menu_text", TEXT),
            ("news_panel", c(0x333333)),
            ("button", c(0x474747)),
            ("button_hi", c(0x555555)),
            ("glyph", c(0xe6e6e6)),
            ("glyph_dim", c(0x8c8c8c)),
            ("text", TEXT),
            ("text_dim", c(0xa3a3a3)),
            ("text_faint", c(0x7f7f7f)),
            ("on_top", HIGHLIGHT),
            ("on_bottom", MAIN),
            ("on_glyph", COLORED_DARK),
            ("pill", MAIN),
            ("pill_text", COLORED_DARK),
            ("record", c(0xff6159)),
            ("tempo_box", c(0x2b2b2b)),
            ("accent", MAIN),
            ("accent_dim", COLORED_DARK),
            ("workspace", c(0x262626)),
            ("watermark", c(0x2e2e2e)),
            ("version_text", c(0x5a5a5a)),
            ("browser_title", c(0x333333)),
            ("browser_tabs", c(0x2b2b2b)),
            ("browser_tab_outline", MAIN),
            ("browser_list", c(0x1f1f1f)),
            ("browser_row_hi", c(0x333333)),
            ("browser_folder", c(0xd9d9d9)),
            ("browser_footer", c(0x2b2b2b)),
            ("browser_search", MAIN),
            ("cat_project", MAIN),
            ("cat_recent", c(0xc8e3a6)),
            ("cat_plugin_db", c(0xa9cbe6)),
            ("cat_plugin_presets", c(0xe6b3cf)),
            ("cat_channel_presets", c(0xd4bce8)),
            ("cat_mixer_presets", c(0xbcc2ec)),
            ("splitter", c(0x262626)),
            ("splitter_dark", c(0x1c1c1c)),
            ("window_title", c(0x333333)),
            ("window_title_float", c(0x333333)),
            ("window_title_hi", c(0x3d3d3d)),
            ("window_body", c(0x2e2e2e)),
            ("window_border", c(0x1a1a1a)),
            ("picker", c(0x2b2b2b)),
            ("track_header", c(0x383838)),
            ("overview", c(0x555555)),
            ("ruler", c(0x232323)),
            ("ruler_text", c(0x9e9e9e)),
            ("grid", c(0x282828)),
            ("grid_alt", c(0x2d2d2d)),
            ("grid_gap", c(0x202020)),
            ("grid_bar", c(0x161616)),
            ("scrollbar", c(0x2b2b2b)),
            ("playhead", MAIN),
            ("mixer_bg", c(0x262626)),
            ("mixer_strip", c(0x303030)),
            ("mixer_strip_dark", c(0x242424)),
            ("mixer_panel", c(0x333333)),
            ("slot", c(0x2b2b2b)),
            ("slot_text", c(0x707070)),
            ("slot_text_used", c(0xdedede)),
            ("fader_handle", c(0x5c5c5c)),
            ("meter_low", DEEP),
            ("meter_high", HIGHLIGHT),
            ("meter_clip", c(0xff6159)),
            ("rack_row_box", c(0x3a3a3a)),
            ("name_button", c(0x474747)),
            ("name_button_sel", c(0x5e5e5e)),
            ("name_text", c(0xe2e2e2)),
            ("rack_inset", c(0x333333)),
            ("rack_inset_border", c(0x262626)),
            ("rack_control_border", c(0x222222)),
            ("step_a", c(0x4d4d4d)),
            ("step_b", c(0x5a4d1f)),
            ("step_dim_a", c(0x3a3a3a)),
            ("step_dim_b", c(0x3d3827)),
            ("step_dim_border", c(0x2e2e2e)),
            ("step_on", MAIN),
            ("add_hover", MAIN),
            ("menu_drop", c(0x333333)),
            ("menu_drop_hi", c(0x4d4d4d)),
            ("menu_drop_text", TEXT),
            ("menu_drop_disabled", c(0x7f7f7f)),
            ("menu_pop", c(0x333333)),
            ("menu_pop_border", c(0x1f1f1f)),
            ("menu_pop_hi", COLORED_DARK),
            ("menu_pop_text", TEXT),
            ("menu_pop_shortcut", c(0xa3a3a3)),
            ("menu_pop_dim", c(0x7f7f7f)),
            ("menu_pop_head", c(0x2b2b2b)),
            ("menu_pop_head_text", c(0x8c8c8c)),
            ("clip", c(0x8a7a3a)),
            ("pr_row", c(0x2b2b2b)),
            ("pr_row_black", c(0x242424)),
            ("pr_row_sel", c(0x2f2c22)),
            ("pr_row_black_sel", c(0x29261c)),
            ("pr_bar", c(0x141414)),
            ("note_top", MAIN),
            ("note_bottom", c(0xf2d052)),
            ("note_rim", c(0x8a7420)),
            ("note_text", COLORED_DARK),
            ("note_sel_top", c(0xffffff)),
            ("note_sel_bottom", HIGHLIGHT),
            ("note_sel_rim", COLORED_DARK),
            ("note_sel_text", COLORED_DARK),
            ("key_white", c(0xededed)),
            ("key_white_left", c(0xd2d2d2)),
            ("key_black", c(0x3b3b3b)),
            ("key_black_left", c(0x262626)),
            ("key_c", c(0xdddddd)),
            ("key_label", c(0x8a7420)),
            ("key_lit", HIGHLIGHT),
            ("key_lit_left", MAIN),
            ("key_lit_edge", DEEP),
            ("key_black_lit", MAIN),
            ("key_black_lit_left", c(0xa88f25)),
            ("velocity", MAIN),
            ("velocity_dot", COLORED_DARK),
        ],
    }
}

/// The palette of `design`, with `theme` (a `.flstheme`) applied when the design is Classic. Lemon has its own
/// colours: a theme file does not apply to it.
pub fn palette(design: Design, theme: Option<&FlTheme>) -> Palette {
    let Some(map) = map_of(design) else {
        return match theme {
            Some(th) => fl::palette(th),
            None => Palette::default(),
        };
    };
    let classic = Palette::default();
    let mut p = classic.clone();
    for name in Palette::NAMES {
        let c = classic.get(name).expect("token");
        let v = match fl::role(name) {
            fl::Role::Surface | fl::Role::StepEven | fl::Role::StepOdd | fl::Role::Fixed => map.surface(c),
            fl::Role::Accent => map.accent_of(c),
            fl::Role::Highlight => map.highlight_of(c),
            fl::Role::Text => map.text_of(c),
            fl::Role::Mute => map.mute_of(c),
        };
        *p.get_mut(name).expect("token") = v;
    }
    for (name, c) in overrides(design) {
        if let Some(slot) = p.get_mut(name) {
            *slot = c;
        }
    }
    p.xf = fl::Xform { design: Some(map), ..fl::Xform::default() };
    p.style = design.style();
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contrast(a: Color, b: Color) -> f32 {
        (a.hsl().2 - b.hsl().2).abs()
    }

    #[test]
    fn classic_is_unchanged() {
        assert_eq!(palette(Design::Classic, None), Palette::default());
        let th = FlTheme::parse("Hue=120\nSaturation=60\nLightness=-90\nContrast=0\nSelected=16744576\n");
        assert_eq!(palette(Design::Classic, Some(&th)), fl::palette(&th));
    }

    #[test]
    fn lemon_uses_the_brand_colours() {
        let p = palette(Design::Lemon, None);
        assert!(p.style.lemon());
        assert_eq!(p.toolbar, lemon::DARK);
        assert_eq!(p.pill, lemon::MAIN);
        assert_eq!(p.on_glyph, lemon::COLORED_DARK);
        // Role-mapped literals follow the design: Classic's raised grey lands on the dark highlight,
        // orange highlights on lemon.
        let g = p.s(0x505355);
        assert!(g.r.abs_diff(0x66) <= 6 && g.r == g.g && g.g == g.b, "{g}");
        let (h, s, _) = p.h(0xfb9244).hsl();
        assert!((h - lemon::MAIN.hsl().0).abs() < 4.0 && s > 0.8, "{}", p.h(0xfb9244));
        assert_eq!(p.a(0xa8e44a), lemon::MAIN);
    }

    #[test]
    fn lemon_text_is_readable() {
        let p = palette(Design::Lemon, None);
        for (fg, bg) in [
            (p.text, p.window_title),
            (p.text_dim, p.window_title),
            (p.glyph, p.toolbar),
            (p.menu_text, p.menu_panel),
            (p.browser_folder, p.browser_list),
            (p.pill_text, p.pill),
            (p.on_glyph, p.on_bottom),
            (p.menu_pop_text, p.menu_pop_hi),
            (p.name_text, p.name_button),
            (p.ruler_text, p.ruler),
            (p.slot_text_used, p.slot),
            (p.note_text, p.note_top),
        ] {
            assert!(contrast(fg, bg) >= 0.38, "{fg} on {bg}");
        }
    }

    #[test]
    fn names_round_trip() {
        for d in Design::ALL {
            assert_eq!(Design::parse(d.name()), Some(d));
        }
        assert_eq!(Design::Lemon.next(), Design::Classic);
    }
}
