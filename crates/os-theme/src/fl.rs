// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

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

/// No adjustment (every parameter 0): surfaces stay in FL's base palette (spec §4, §5).
pub const BASE: Surface = Surface { hue: 0.0, saturation: 0.0, lightness: 0.0, contrast: 0.0 };

/// The stock Dark theme's file text (its key colours; what Lemon Studio's default palette was measured with). A theme's
/// `Lightmode` key is ignored: every theme is drawn as FL draws it with Light mode on (spec: ui/settings-choices.md
/// "Light mode and clip note colours", removed 2026-10-18).
pub const DARK_TEXT: &str = "Hue=0\nSaturation=0\nLightness=-153\nContrast=-8\nText=0\nSelected=11068490\nHighlight=-12283141\n\
     Mute=577656\nOption=-14135064\nStepEven=-13357016\nStepOdd=-12764317\nTextColor=-3551277\nBackMode=2\n";

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
    pub override_clips: bool,
    pub meters: Vec<Color>,
    /// The Waves gradient, (`WaveSpc`i, `WaveClr`i) for each i in 0..6 that has both, sorted by position (§3).
    pub waves: Vec<(f32, Color)>,
    pub note_colors: Vec<Color>,
    /// (colour, custom flag, contrast %) for piano roll, playlist, event editor.
    pub pr_grid: (Color, bool, f32),
    pub pl_grid: (Color, bool, f32),
    pub ee_grid: (Color, bool, f32),
    pub back_mode: i32,
    pub back_color: Option<Color>,
    pub back_picture: String,
    /// Lemon Studio's own key (`COLORS_KEY`, ignored by FL): the theme is a skin's own colours, the Lemon or Citrus
    /// theme (spec: ui/theme-presets.md §B).
    pub colors: Option<crate::Design>,
}

/// The key that makes a theme file a skin's own colours: `LemonStudioColors=Lemon` or `=Citrus`. FL ignores it and
/// reads the rest of the file (an approximation of those colours).
pub const COLORS_KEY: &str = "LemonStudioColors";

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
        let mut waves: Vec<(f32, Color)> =
            (0..6).filter_map(|i| Some((num(&format!("wavespc{i}"))? as f32, col(&format!("waveclr{i}"))?))).collect();
        waves.sort_by(|a, b| a.0.total_cmp(&b.0));
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
            override_clips: num("overrideclips").unwrap_or(0.0) != 0.0,
            meters: list("meter", 6),
            waves,
            note_colors: list("notecolor", 16),
            pr_grid: grid("pr"),
            pl_grid: grid("pl"),
            ee_grid: grid("ee"),
            back_mode: num("backmode").unwrap_or(2.0) as i32,
            back_color: col("backcolor"),
            back_picture: kv.get("backpicfilename").cloned().unwrap_or_default(),
            colors: kv.get(&COLORS_KEY.to_ascii_lowercase()).and_then(|v| crate::Design::parse(v)),
        }
    }

    /// The stock Dark theme's key colours (what Lemon Studio's default palette was measured with).
    pub fn dark() -> FlTheme {
        FlTheme::parse(DARK_TEXT)
    }

    /// Whether this is the stock Dark theme, whatever its clip note colours option: the colours a skin has of its own
    /// (no theme).
    pub fn is_stock(&self) -> bool {
        self.same_colours(&FlTheme::dark())
    }

    /// Whether the two themes differ at most in their clip note colours option.
    pub fn same_colours(&self, other: &FlTheme) -> bool {
        let plain = |t: &FlTheme| FlTheme { override_clips: false, ..t.clone() };
        plain(self) == plain(other)
    }
}

/// `text` (a `.flstheme` file) with `key` set to `value`: its line rewritten in place, or added at the end; the rest of
/// the file is kept as it was (toggling a theme option rewrites only its own key, spec: ui/settings-choices.md "Light
/// mode and clip note colours").
pub fn set_key(text: &str, key: &str, value: &str) -> String {
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = text.split(nl).map(String::from).collect();
    let mut found = false;
    for line in &mut lines {
        let bom = if line.starts_with('\u{feff}') { "\u{feff}" } else { "" };
        if line.trim_start_matches('\u{feff}').split_once('=').is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case(key)) {
            *line = format!("{bom}{key}={value}");
            found = true;
        }
    }
    if !found {
        // Before the final line break, if the file ends with one.
        let at = lines.len() - usize::from(lines.last().is_some_and(String::is_empty));
        lines.insert(at, format!("{key}={value}"));
    }
    lines.join(nl)
}

/// `text` without its `key` lines.
pub fn remove_key(text: &str, key: &str) -> String {
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    text.split(nl)
        .filter(|l| !l.trim_start_matches('\u{feff}').split_once('=').is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case(key)))
        .collect::<Vec<_>>()
        .join(nl)
}

/// The value of `key` in `text` (a `.flstheme` file), trimmed; the last line wins, as in `FlTheme::parse`.
pub fn get_key<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.trim_start_matches('\u{feff}')
        .lines()
        .filter_map(|l| l.split_once('='))
        .filter(|(k, _)| k.trim().eq_ignore_ascii_case(key))
        .map(|(_, v)| v.trim())
        .next_back()
}

/// Whether `key` holds an RGB-ordered colour (§2: Selected and TextColor); the other colour keys are COLORREF.
pub fn rgb_key(key: &str) -> bool {
    key.eq_ignore_ascii_case("Selected") || key.eq_ignore_ascii_case("TextColor")
}

/// `key`'s colour from its file value (§2).
pub fn colour_of(key: &str, v: i64) -> Color {
    if rgb_key(key) { rgb_value(v) } else { colorref(v) }
}

/// `c` as `key`'s file value (§2), high byte 0.
pub fn colour_value(key: &str, c: Color) -> u32 {
    let (r, g, b) = (u32::from(c.r), u32::from(c.g), u32::from(c.b));
    if rgb_key(key) { r << 16 | g << 8 | b } else { b << 16 | g << 8 | r }
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
    /// A theme laid over the design's colours (after `design`).
    pub retint: Option<crate::design::Retint>,
}

impl Xform {
    pub fn surface(&self, c: Color) -> Color {
        if let Some(d) = &self.design {
            let c = d.surface(c);
            return self.retint.as_ref().map_or(c, |r| r.surface(c));
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
            let c = d.accent_of(c);
            return self.retint.as_ref().map_or(c, |r| r.accent(c));
        }
        Self::key(self.accent, c)
    }
    pub fn highlight(&self, c: Color) -> Color {
        if let Some(d) = &self.design {
            let c = d.highlight_of(c);
            return self.retint.as_ref().map_or(c, |r| r.highlight(c));
        }
        Self::key(self.highlight, c)
    }
    pub fn text(&self, c: Color) -> Color {
        if let Some(d) = &self.design {
            let c = d.text_of(c);
            return self.retint.as_ref().map_or(c, |r| r.text(c));
        }
        Self::key(self.text, c)
    }
    pub fn mute(&self, c: Color) -> Color {
        if let Some(d) = &self.design {
            let c = d.mute_of(c);
            return self.retint.as_ref().map_or(c, |r| r.mute(c));
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
    let surface = th.surface;
    let pair = |from: Option<Color>, to: Option<Color>| match (from, to) {
        (Some(f), Some(t)) if f != t => Some((f, t)),
        _ => None,
    };
    out.xf = Xform {
        design: None,
        retint: None,
        surf: (surface != DARK).then_some(surface),
        accent: pair(dark.selected, th.selected),
        highlight: pair(dark.highlight, th.highlight),
        text: pair(dark.text_color, th.text_color),
        mute: pair(dark.mute, th.mute),
    };
    let surf = |c: Color| surface.apply(DARK.invert(c));
    let identity = surface == DARK;
    let key = |c: Color, from: Option<Color>, to: Option<Color>| match (from, to) {
        (Some(f), Some(t)) if f != t => rekey(c, f, t),
        _ => c,
    };
    // Step faces: the step key colour blended 40 % toward the rack panel surface (§3; fits Dark and NightGold).
    let panel_dark = base.rack_inset;
    // They take the theme's own adjustment.
    let own = |c: Color| th.surface.apply(DARK.invert(c));
    let own_identity = th.surface == DARK;
    let panel_th = if own_identity { panel_dark } else { own(panel_dark) };
    let step = |c: Color, from: Option<Color>, to: Option<Color>| match (from, to) {
        (Some(f), Some(t)) => shift(c, f.mix(panel_dark, 0.4), t.mix(panel_th, 0.4)),
        _ => if own_identity { c } else { own(c) },
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
    finish(&mut out, th, base.note_top, !identity);
    out
}

/// What a theme colours directly whatever the skin (spec §3): browser text brightness, the readability guard (`guard`),
/// custom grids, the solid workspace colour, note colours (re-keyed from `note_from`, the skin's note body), meters and
/// the Waves gradient.
pub(crate) fn finish(out: &mut Palette, th: &FlTheme, note_from: Color, guard: bool) {
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
    for (name, bg) in pairs.into_iter().filter(|_| guard) {
        let slot = out.get_mut(name).expect("token");
        *slot = readable(*slot, bg, 0.38);
    }
    // Faint labels (playlist track names) may stay fainter, but not vanish on light themes.
    if guard {
        out.text_faint = readable(out.text_faint, out.track_header, 0.3);
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
        let from = note_from;
        for slot in [&mut out.note_top, &mut out.note_bottom, &mut out.note_rim, &mut out.note_text, &mut out.velocity, &mut out.velocity_dot] {
            *slot = rekey(*slot, from, *n);
        }
    }
    if th.meters.len() >= 4 {
        out.meter_low = th.meters[0];
        out.meter_high = th.meters[2];
    }
    // Colourful waveforms use the theme's Waves gradient as it is (spec: ui/settings-choices.md "Display: Colorful
    // waveforms").
    out.waves = th.waves.clone();
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

    // spec: ui/flstheme.md §3 (WaveClr / WaveSpc), ui/settings-choices.md "Display: Colorful waveforms (2026-10-16)"
    /// The Waves gradient: COLORREF colours at their positions, sorted, a stop missing either half left out; the
    /// palette carries it.
    #[test]
    fn keys_are_read_and_colours_written_in_their_byte_order() {
        let text = "Hue=0\r\nSelected=11068490\r\nHighlight=-12283141\r\nhue=-20\r\n";
        assert_eq!(get_key(text, "HUE"), Some("-20"), "the last line wins, any case");
        assert_eq!(get_key(text, "Mute"), None);
        assert_eq!(colour_of("Selected", 11068490), Color::rgb(0xa8e44a));
        assert_eq!(colour_of("Highlight", -12283141), Color::rgb(0xfb9244));
        assert_eq!(colour_value("Selected", Color::rgb(0xa8e44a)), 0xa8e44a);
        assert_eq!(colour_value("Highlight", Color::rgb(0xfb9244)), 0x4492fb);
        assert_eq!(colour_value("textcolor", Color::rgb(0x123456)), 0x123456);
        let th = FlTheme::parse(&set_key(text, "Highlight", &colour_value("Highlight", Color::rgb(0x102030)).to_string()));
        assert_eq!(th.highlight, Some(Color::rgb(0x102030)));
    }

    #[test]
    fn waves_gradient_is_read() {
        let th = FlTheme::parse("WaveClr0=4661375
WaveSpc0=0
WaveClr1=-7249815
WaveSpc1=1
WaveClr2=255
WaveSpc2=0.5
WaveClr3=65280
");
        assert_eq!(th.waves, vec![(0.0, Color::rgb(0x7f2047)), (0.5, Color::rgb(0xff0000)), (1.0, Color::rgb(0x696091))]);
        assert_eq!(palette(&th).waves, th.waves);
        assert!(FlTheme::dark().waves.is_empty());
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

    /// A theme's Lightmode key is ignored (the Light mode option was removed, owner 2026-10-18): Lightmode=0, 1 or no
    /// key draw alike, in Citrus and in Lemon. Lemon keeps its own colours under the stock Dark theme whatever its
    /// options, and follows the key colours of any other theme (spec: ui/theme-presets.md §B).
    #[test]
    fn the_light_mode_key_is_ignored() {
        let dark = Palette::default();
        assert_eq!(palette(&FlTheme::dark()), dark);
        for v in ["0", "1"] {
            assert_eq!(palette(&FlTheme::parse(&set_key(DARK_TEXT, "Lightmode", v))), dark, "Lightmode={v}");
        }
        let lemon = |t: &str| crate::design::palette(crate::Design::Lemon, Some(&FlTheme::parse(t)));
        let brand = crate::design::palette(crate::Design::Lemon, None);
        assert_eq!(lemon(&set_key(DARK_TEXT, "Lightmode", "0")), brand);
        assert_eq!(lemon(&set_key(DARK_TEXT, "OverrideClips", "1")), brand);
        let night = "Hue=-18\nSaturation=6\nLightness=-57\nContrast=40\nSelected=-4546441\n";
        assert_eq!(lemon(&set_key(night, "Lightmode", "0")), lemon(night));
        assert_ne!(lemon(night), brand);
        assert_eq!(FlTheme::parse(&set_key(night, "Lightmode", "0")), FlTheme::parse(night));
    }

    /// Without OverrideClips note colours are off (spec: ui/flstheme.md §3).
    #[test]
    fn a_missing_override_clips_key_means_off() {
        assert!(!FlTheme::parse("Hue=0\n").override_clips);
        assert!(FlTheme::parse("Lightmode=0\r\nOverrideClips=1\r\n").override_clips);
    }

    /// Setting a key rewrites only its line, keeping the BOM and the line breaks; a missing key is added at the end.
    #[test]
    fn set_key_rewrites_one_line() {
        let text = "\u{feff}Lightmode=1\r\nHue=4\r\n";
        assert_eq!(set_key(text, "Lightmode", "0"), "\u{feff}Lightmode=0\r\nHue=4\r\n");
        assert_eq!(set_key(text, "OverrideClips", "1"), "\u{feff}Lightmode=1\r\nHue=4\r\nOverrideClips=1\r\n");
        assert_eq!(set_key("Hue=4", "lightmode", "0"), "Hue=4\nlightmode=0");
        assert_eq!(set_key("", "Lightmode", "0"), "Lightmode=0\n");
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
