// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! `.flstheme` import: parse the file and turn it into a Lemon Studio palette.
// spec: ui/flstheme.md
//
// Lemon Studio's default palette is the measured look of the stock "Dark" theme (spec §5). Importing a theme maps
// every surface token back to the base look (inverse of the Dark adjustment) and then forward through the
// theme's own adjustment (§4). Tokens that the file colours directly (accent, highlight, text, LEDs, steps,
// grids) are re-keyed from the default key colour to the theme's (same hue / saturation / lightness offset).

use crate::{Color, Palette};

/// The four surface adjustment parameters of a theme file (raw file units).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Surface {
    pub hue: f32,
    pub saturation: f32,
    pub lightness: f32,
    pub contrast: f32,
}

/// The stock "Dark" theme: Lemon Studio's default palette was measured in it.
pub const DARK: Surface = Surface { hue: 0.0, saturation: 0.0, lightness: -153.0, contrast: -8.0 };

impl Surface {
    fn tc(&self) -> (f32, f32) {
        ((self.lightness / 255.0).clamp(-1.0, 1.0), (self.contrast / 100.0).clamp(-1.0, 1.0))
    }

    /// Lightness map (§4 steps 1–2).
    pub fn map_l(&self, l: f32) -> f32 {
        let (t, c) = self.tc();
        let a = t.abs();
        let l1 = if t < 0.0 { l * (1.0 - 0.403 * a) - 0.186 * a } else { l + (1.0 - l) * 0.888 * t + 0.099 * t * (1.0 - t) };
        let k = if c >= 0.0 { (0.941 * c).exp() } else { (1.52 * c.max(-0.5)).exp() };
        let pivot = 0.490 - 0.134 * t;
        ((l1 - pivot) * k + pivot).clamp(0.0, 1.0)
    }

    /// Saturation gain (§4 step 3).
    pub fn sat_gain(&self) -> f32 {
        let (t, c) = self.tc();
        let mut g = if c >= 0.0 { (2.03 * c).exp() } else { (4.565 * c.max(-0.5)).exp() };
        if t > 0.0 {
            g /= 1.0 + 9.53 * t;
        }
        let sg = (self.saturation / 256.0).clamp(-1.0, 1.0);
        g * if sg >= 0.0 { 1.0 + 0.925 * sg } else { 1.0 + sg }
    }

    /// Map a base-look colour through this adjustment.
    pub fn apply(&self, c: Color) -> Color {
        let (h, s, l) = c.hsl();
        Color::from_hsl_pub(h - self.hue, (s * self.sat_gain()).clamp(0.0, 1.0), self.map_l(l), c.a)
    }

    /// Inverse: the base-look colour that this adjustment maps to `c` (where clipped, the edge of the clipped range).
    pub fn invert(&self, c: Color) -> Color {
        let (h, s, l) = c.hsl();
        let target = l.clamp(0.5 / 255.0, 1.0 - 0.5 / 255.0);
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..32 {
            let mid = (lo + hi) / 2.0;
            if self.map_l(mid) < target { lo = mid } else { hi = mid }
        }
        let g = self.sat_gain().max(1e-3);
        Color::from_hsl_pub(h + self.hue, (s / g).clamp(0.0, 1.0), (lo + hi) / 2.0, c.a)
    }
}

/// A parsed `.flstheme` file (spec §3). Colours are already decoded from COLORREF (§2).
#[derive(Debug, Clone, PartialEq)]
pub struct FlTheme {
    pub surface: Surface,
    pub text: f32,
    pub text_color: Option<Color>,
    pub selected: Option<Color>,
    pub highlight: Option<Color>,
    pub mute: Option<Color>,
    pub option: Option<Color>,
    pub step_even: Option<Color>,
    pub step_odd: Option<Color>,
    pub light_mode: bool,
    pub override_clips: bool,
    pub meters: Vec<Color>,
    pub note_colors: Vec<Color>,
    /// (colour, custom flag, contrast %) for piano roll, playlist, event editor.
    pub pr_grid: (Color, bool, f32),
    pub pl_grid: (Color, bool, f32),
    pub ee_grid: (Color, bool, f32),
    pub back_mode: i32,
    pub back_color: Option<Color>,
    pub back_picture: String,
}

/// Decode an RGB-ordered value (§2: Selected, TextColor): blue in the low byte, high byte ignored.
pub fn rgb_value(v: i64) -> Color {
    Color::rgb(v as u32 & 0xff_ffff)
}

/// Decode a COLORREF value (§2): red in the low byte, high byte ignored.
pub fn colorref(v: i64) -> Color {
    let u = v as u32;
    Color { r: u as u8, g: (u >> 8) as u8, b: (u >> 16) as u8, a: 255 }
}

impl FlTheme {
    /// Parse a `.flstheme` file. Unknown keys are ignored; malformed numbers are skipped.
    pub fn parse(text: &str) -> FlTheme {
        let mut kv = std::collections::HashMap::new();
        for line in text.trim_start_matches('\u{feff}').lines() {
            if let Some((k, v)) = line.split_once('=') {
                kv.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
            }
        }
        let num = |k: &str| kv.get(k).and_then(|v| v.parse::<f64>().ok());
        let col = |k: &str| kv.get(k).and_then(|v| v.parse::<i64>().ok()).map(colorref);
        let rgbc = |k: &str| kv.get(k).and_then(|v| v.parse::<i64>().ok()).map(rgb_value);
        let grid = |p: &str| {
            (
                col(&format!("{p}gridback")).unwrap_or(colorref(5129268)),
                num(&format!("{p}gridcustom")).unwrap_or(0.0) != 0.0,
                num(&format!("{p}gridcontrast")).unwrap_or(100.0) as f32,
            )
        };
        let list = |p: &str, n: usize| (0..n).map_while(|i| col(&format!("{p}{i}"))).collect::<Vec<_>>();
        FlTheme {
            surface: Surface {
                hue: num("hue").unwrap_or(0.0) as f32,
                saturation: num("saturation").unwrap_or(0.0) as f32,
                lightness: num("lightness").unwrap_or(0.0) as f32,
                contrast: num("contrast").unwrap_or(0.0) as f32,
            },
            text: num("text").unwrap_or(0.0) as f32,
            text_color: rgbc("textcolor"),
            selected: rgbc("selected"),
            highlight: col("highlight"),
            mute: col("mute"),
            option: col("option"),
            step_even: col("stepeven"),
            step_odd: col("stepodd"),
            light_mode: num("lightmode").unwrap_or(0.0) != 0.0,
            override_clips: num("overrideclips").unwrap_or(0.0) != 0.0,
            meters: list("meter", 6),
            note_colors: list("notecolor", 16),
            pr_grid: grid("pr"),
            pl_grid: grid("pl"),
            ee_grid: grid("ee"),
            back_mode: num("backmode").unwrap_or(2.0) as i32,
            back_color: col("backcolor"),
            back_picture: kv.get("backpicfilename").cloned().unwrap_or_default(),
        }
    }

    /// The stock Dark theme's key colours (what Lemon Studio's default palette was measured with).
    pub fn dark() -> FlTheme {
        FlTheme::parse(
            "Hue=0\nSaturation=0\nLightness=-153\nContrast=-8\nText=0\nSelected=11068490\nHighlight=-12283141\n\
             Mute=577656\nOption=-14135064\nStepEven=-13357016\nStepOdd=-12764317\nTextColor=-3551277\nBackMode=2\n",
        )
    }
}

/// What a token is coloured by (spec §3).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Role {
    Surface,
    Accent,
    Highlight,
    Text,
    Mute,
    StepEven,
    StepOdd,
    /// Signal colours (record, meters, clip, categories): kept.
    Fixed,
}

pub(crate) fn role(name: &str) -> Role {
    match name {
        "accent" | "accent_dim" | "playhead" | "velocity" | "velocity_dot" => Role::Accent,
        "on_top" | "on_bottom" | "on_glyph" | "pill" | "pill_text" | "add_hover" | "browser_tab_outline" | "browser_search"
        | "key_lit" | "key_lit_left" | "key_lit_edge" | "key_black_lit" | "key_black_lit_left" => Role::Highlight,
        "menu_text" | "glyph" | "glyph_dim" | "text" | "text_dim" | "text_faint" | "ruler_text" | "slot_text"
        | "slot_text_used" | "name_text" | "menu_drop_text" | "menu_drop_disabled" | "menu_pop_text" | "menu_pop_shortcut"
        | "menu_pop_dim" | "menu_pop_head_text" | "browser_folder" | "version_text" => Role::Text,
        "step_a" | "step_dim_a" => Role::StepEven,
        "step_b" | "step_dim_b" => Role::StepOdd,
        "record" | "meter_low" | "meter_high" | "meter_clip" | "cat_project" | "cat_recent" | "cat_plugin_db"
        | "cat_plugin_presets" | "cat_channel_presets" | "cat_mixer_presets" | "clip" | "note_top" | "note_bottom" | "note_rim"
        | "note_text" | "note_sel_top" | "note_sel_bottom" | "note_sel_rim" | "note_sel_text" | "key_white" | "key_white_left"
        | "key_black" | "key_black_left" | "key_c" | "key_label" | "step_on" => Role::Fixed,
        _ => Role::Surface,
    }
}

/// Move `c` by the hue / saturation / lightness difference between `from` and `to`.
pub fn rekey(c: Color, from: Color, to: Color) -> Color {
    let (h, s, l) = c.hsl();
    let (hf, sf, lf) = from.hsl();
    let (ht, st, lt) = to.hsl();
    let s2 = if sf > 0.02 { s * st / sf } else { st };
    // Lightness: keep the token's relative position between black / the key / white.
    let l2 = if l <= lf { if lf > 0.0 { l / lf * lt } else { lt } } else if lf < 1.0 { lt + (l - lf) / (1.0 - lf) * (1.0 - lt) } else { lt };
    let h2 = if sf > 0.02 { h + (ht - hf) } else { ht };
    Color::from_hsl_pub(h2, s2.clamp(0.0, 1.0), l2.clamp(0.0, 1.0), c.a)
}

/// Like `rekey` but moves lightness by the difference (keeps a dark face dark when the key is a dark colour).
pub fn shift(c: Color, from: Color, to: Color) -> Color {
    let (h, s, l) = c.hsl();
    let (hf, sf, lf) = from.hsl();
    let (ht, st, lt) = to.hsl();
    let s2 = if sf > 0.02 { s * st / sf } else { st.max(s * 0.5) };
    let h2 = if sf > 0.02 { h + (ht - hf) } else { ht };
    Color::from_hsl_pub(h2, s2.clamp(0.0, 1.0), (l + lt - lf).clamp(0.0, 1.0), c.a)
}

/// Mapping from Lemon Studio's measured default colours to a theme, per role. Default = identity.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Xform {
    /// Surface adjustment of the theme (`None` = the stock Dark look).
    pub surf: Option<Surface>,
    /// (default key, theme key) pairs.
    pub accent: Option<(Color, Color)>,
    pub highlight: Option<(Color, Color)>,
    pub text: Option<(Color, Color)>,
    pub mute: Option<(Color, Color)>,
    /// A design's re-colouring (`design::palette`), applied instead of the surface / key pairs above.
    pub design: Option<crate::design::DesignMap>,
}

impl Xform {
    pub fn surface(&self, c: Color) -> Color {
        if let Some(d) = &self.design {
            return d.surface(c);
        }
        match self.surf {
            Some(s) => s.apply(DARK.invert(c)),
            None => c,
        }
    }
    fn key(k: Option<(Color, Color)>, c: Color) -> Color {
        match k {
            Some((f, t)) => rekey(c, f, t),
            None => c,
        }
    }
    pub fn accent(&self, c: Color) -> Color {
        if let Some(d) = &self.design {
            return d.accent_of(c);
        }
        Self::key(self.accent, c)
    }
    pub fn highlight(&self, c: Color) -> Color {
        if let Some(d) = &self.design {
            return d.highlight_of(c);
        }
        Self::key(self.highlight, c)
    }
    pub fn text(&self, c: Color) -> Color {
        if let Some(d) = &self.design {
            return d.text_of(c);
        }
        Self::key(self.text, c)
    }
    pub fn mute(&self, c: Color) -> Color {
        if let Some(d) = &self.design {
            return d.mute_of(c);
        }
        Self::key(self.mute, c)
    }
}

/// `c` moved in lightness (hue and saturation kept) until it differs from `bg` by at least `gap`.
pub fn readable(c: Color, bg: Color, gap: f32) -> Color {
    let (h, s, l) = c.hsl();
    let (_, _, lb) = bg.hsl();
    if (l - lb).abs() >= gap {
        return c;
    }
    let up = (lb + gap).min(1.0);
    let down = (lb - gap).max(0.0);
    // Go the way that has room (light background -> darker text).
    let l2 = if lb > 0.5 { down } else { up };
    Color::from_hsl_pub(h, s, l2, c.a)
}

/// The default palette re-coloured by `th`. `Palette::default()` is returned for the stock Dark theme.
pub fn palette(th: &FlTheme) -> Palette {
    let dark = FlTheme::dark();
    let base = Palette::default();
    let mut out = base.clone();
    let pair = |from: Option<Color>, to: Option<Color>| match (from, to) {
        (Some(f), Some(t)) if f != t => Some((f, t)),
        _ => None,
    };
    out.xf = Xform {
        design: None,
        surf: (th.surface != DARK).then_some(th.surface),
        accent: pair(dark.selected, th.selected),
        highlight: pair(dark.highlight, th.highlight),
        text: pair(dark.text_color, th.text_color),
        mute: pair(dark.mute, th.mute),
    };
    let surf = |c: Color| th.surface.apply(DARK.invert(c));
    let identity = th.surface == DARK;
    let key = |c: Color, from: Option<Color>, to: Option<Color>| match (from, to) {
        (Some(f), Some(t)) if f != t => rekey(c, f, t),
        _ => c,
    };
    // Step faces: the step key colour blended 40 % toward the rack panel surface (§3; fits Dark and NightGold).
    let panel_dark = base.rack_inset;
    let panel_th = if identity { panel_dark } else { surf(panel_dark) };
    let step = |c: Color, from: Option<Color>, to: Option<Color>| match (from, to) {
        (Some(f), Some(t)) => shift(c, f.mix(panel_dark, 0.4), t.mix(panel_th, 0.4)),
        _ => if identity { c } else { surf(c) },
    };
    for name in Palette::NAMES {
        let c = base.get(name).expect("token");
        let v = match role(name) {
            Role::Surface => if identity { c } else { surf(c) },
            Role::Accent => key(c, dark.selected, th.selected),
            Role::Highlight => key(c, dark.highlight, th.highlight),
            Role::Text => key(c, dark.text_color, th.text_color),
            Role::Mute => key(c, dark.mute, th.mute),
            Role::StepEven => step(c, dark.step_even, th.step_even),
            Role::StepOdd => step(c, dark.step_odd, th.step_odd),
            Role::Fixed => c,
        };
        *out.get_mut(name).expect("token") = v;
    }
    // Browser text brightness (§3 Text).
    // Text: browser text brightness, -255 darker .. +255 brighter (spec §3). On light themes darker = more contrast.
    if th.text != 0.0 {
        let k = (th.text / 255.0).clamp(-1.0, 1.0);
        let target = if k < 0.0 { Color::rgb(0x000000) } else { Color::rgb(0xffffff) };
        out.browser_folder = out.browser_folder.mix(target, k.abs() * 0.6);
    }
    // Readability guard: text-like tokens keep a minimum lightness gap from the surface they sit on
    // (signal colours like category names were measured on a dark list and vanish on light themes).
    let pairs: [(&str, Color); 14] = [
        ("cat_project", out.browser_list), ("cat_recent", out.browser_list), ("cat_plugin_db", out.browser_list),
        ("cat_plugin_presets", out.browser_list), ("cat_channel_presets", out.browser_list),
        ("cat_mixer_presets", out.browser_list), ("browser_folder", out.browser_list), ("text", out.window_title),
        ("text_dim", out.window_title), ("glyph", out.toolbar), ("menu_text", out.menu_panel),
        ("name_text", out.name_button), ("ruler_text", out.ruler), ("slot_text_used", out.slot),
    ];
    for (name, bg) in pairs.into_iter().filter(|_| !identity) {
        let slot = out.get_mut(name).expect("token");
        *slot = readable(*slot, bg, 0.38);
    }
    // Custom grid backs are used as given (§3).
    if th.pl_grid.1 {
        let g = th.pl_grid.0;
        out.grid = g;
        out.grid_alt = g.shade(0.04);
        out.grid_gap = g.shade(-0.25);
        out.grid_bar = g.shade(-0.6);
    }
    if th.pr_grid.1 {
        let g = th.pr_grid.0;
        out.pr_row = g;
        out.pr_row_black = g.shade(-0.18);
        out.pr_row_sel = g.mix(Color::rgb(0x3a2a30), 0.3);
        out.pr_row_black_sel = out.pr_row_black.mix(Color::rgb(0x3a2a30), 0.3);
        out.pr_bar = g.shade(-0.85);
    }
    if let (1, Some(bc)) = (th.back_mode, th.back_color) {
        out.workspace = bc;
        out.watermark = bc.shade(if bc.hsl().2 > 0.5 { -0.06 } else { 0.06 });
    }
    if let Some(n) = th.note_colors.first() {
        let from = base.note_top;
        for slot in [&mut out.note_top, &mut out.note_bottom, &mut out.note_rim, &mut out.note_text, &mut out.velocity, &mut out.velocity_dot] {
            *slot = rekey(*slot, from, *n);
        }
    }
    if th.meters.len() >= 4 {
        out.meter_low = th.meters[0];
        out.meter_high = th.meters[2];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colorref_is_bgr() {
        assert_eq!(colorref(255), Color::rgb(0xff0000));
        assert_eq!(colorref(16711680), Color::rgb(0x0000ff));
        assert_eq!(colorref(-13884384), Color::rgb(0x20242c));
    }

    #[test]
    fn dark_theme_is_the_default_palette() {
        assert_eq!(palette(&FlTheme::dark()), Palette::default());
    }

    #[test]
    fn invert_round_trips() {
        for c in [0x2e3234u32, 0x22272b, 0x313436, 0x5d6366] {
            let c = Color::rgb(c);
            let back = DARK.apply(DARK.invert(c));
            for (a, b) in [(c.r, back.r), (c.g, back.g), (c.b, back.b)] {
                assert!(a.abs_diff(b) <= 3, "{c} -> {back}");
            }
        }
    }

    #[test]
    fn fitted_model_matches_fl_base_to_dark() {
        // FL default-look grid #34444e becomes #22272b under Dark (spec §5), within a few levels.
        let p = DARK.apply(Color::rgb(0x34444e));
        let want = Color::rgb(0x22272b);
        assert!(p.r.abs_diff(want.r) <= 6 && p.g.abs_diff(want.g) <= 6 && p.b.abs_diff(want.b) <= 6, "{p}");
    }

    #[test]
    fn night_gold_imports() {
        let t = FlTheme::parse("\u{feff}Hue=-18\r\nSaturation=6\r\nLightness=-57\r\nContrast=40\r\nSelected=-4546441\r\nPLGridCustom=1\r\nPLGridback=-13884384\r\nUnknownKey=1\r\n");
        assert_eq!(t.selected, Some(Color::rgb(0xbaa077)));
        let p = palette(&t);
        assert_eq!(p.grid, Color::rgb(0x20242c));
        assert_eq!(p.accent, rekey(Palette::default().accent, Color::rgb(0xa8e44a), Color::rgb(0xbaa077)));
    }
}
