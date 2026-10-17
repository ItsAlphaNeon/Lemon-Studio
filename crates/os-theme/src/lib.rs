// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Studio theme: named color tokens plus a global hue / saturation / lightness adjustment.
//!
//! A theme file is plain text, one `key = value` per line (`#` starts a comment):
//!
//! ```text
//! hue = 20          # degrees, rotates every token
//! saturation = 0.1  # -1 … 1, scales chroma
//! lightness = -0.05 # -1 … 1, shifts lightness
//! accent = #a8e44a  # override any token by name
//! ```
//!
// spec: ui/main-window.md (default token values are the measured colors)

use std::fmt;

pub mod design;
pub mod fl;
pub mod presets;
pub mod waves;
pub use design::{Design, Style};
pub use fl::FlTheme;

/// An sRGB color with alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgb(hex: u32) -> Self {
        Color { r: (hex >> 16) as u8, g: (hex >> 8) as u8, b: hex as u8, a: 255 }
    }

    pub const fn with_alpha(self, a: u8) -> Self {
        Color { a, ..self }
    }

    /// Parse `#rrggbb` or `#rrggbbaa`.
    pub fn parse(s: &str) -> Option<Self> {
        let h = s.trim().strip_prefix('#')?;
        let v = u32::from_str_radix(h, 16).ok()?;
        match h.len() {
            6 => Some(Color::rgb(v)),
            8 => Some(Color::rgb(v >> 8).with_alpha(v as u8)),
            _ => None,
        }
    }

    /// Linear mix toward `other` by `t` (0 = self, 1 = other).
    pub fn mix(self, other: Color, t: f32) -> Color {
        let l = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round().clamp(0.0, 255.0) as u8;
        Color { r: l(self.r, other.r), g: l(self.g, other.g), b: l(self.b, other.b), a: l(self.a, other.a) }
    }

    /// Lighten (positive) or darken (negative) by mixing with white or black.
    pub fn shade(self, amount: f32) -> Color {
        if amount >= 0.0 {
            self.mix(Color::rgb(0xffffff).with_alpha(self.a), amount)
        } else {
            self.mix(Color::rgb(0x000000).with_alpha(self.a), -amount)
        }
    }

    fn to_hsl(self) -> (f32, f32, f32) {
        let (r, g, b) = (f32::from(self.r) / 255.0, f32::from(self.g) / 255.0, f32::from(self.b) / 255.0);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let l = (max + min) / 2.0;
        if max == min {
            return (0.0, 0.0, l);
        }
        let d = max - min;
        let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
        let h = if max == r {
            (g - b) / d + if g < b { 6.0 } else { 0.0 }
        } else if max == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
        (h * 60.0, s, l)
    }

    fn from_hsl(h: f32, s: f32, l: f32, a: u8) -> Color {
        let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
        let hp = h.rem_euclid(360.0) / 60.0;
        let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
        let (r, g, b) = match hp as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = l - c / 2.0;
        let q = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
        Color { r: q(r), g: q(g), b: q(b), a }
    }

    /// Hue (degrees), saturation and lightness (0..1).
    pub fn hsl(self) -> (f32, f32, f32) {
        self.to_hsl()
    }

    pub fn from_hsl_pub(h: f32, s: f32, l: f32, a: u8) -> Color {
        Color::from_hsl(h, s, l, a)
    }

    /// Apply the theme's global adjustment.
    pub fn adjust(self, adj: Adjust) -> Color {
        if adj == Adjust::default() {
            return self;
        }
        let (h, s, l) = self.to_hsl();
        let s = (s * (1.0 + adj.saturation)).clamp(0.0, 1.0);
        let l = (l + adj.lightness).clamp(0.0, 1.0);
        Color::from_hsl(h + adj.hue, s, l, self.a)
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)?;
        if self.a != 255 {
            write!(f, "{:02x}", self.a)?;
        }
        Ok(())
    }
}

/// Global adjustment applied to every token (the "easy theming" knobs).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Adjust {
    /// Degrees.
    pub hue: f32,
    /// −1 … 1 (relative chroma change).
    pub saturation: f32,
    /// −1 … 1 (absolute lightness shift).
    pub lightness: f32,
}

macro_rules! tokens {
    ($($name:ident = $hex:literal, $doc:literal;)*) => {
        /// All theme colors, by role.
        #[derive(Debug, Clone, PartialEq)]
        pub struct Palette {
            $(#[doc = $doc] pub $name: Color,)*
            /// How measured default colours map to this theme (see `Palette::s`, `a`, `h`, `tx`, `led`).
            pub xf: fl::Xform,
            /// Shape language of the design this palette belongs to (corner radii; spec: ui/lemon-design.md).
            pub style: Style,
            /// The theme's Waves gradient, (position, colour) sorted by position: a `.flstheme`'s `WaveClr0..5` at
            /// `WaveSpc0..5` (spec: ui/flstheme.md §3). Empty without one; colourful waveforms then use
            /// `waves::STOCK`.
            pub waves: Vec<(f32, Color)>,
        }

        impl Default for Palette {
            fn default() -> Self {
                Palette { $($name: Color::rgb($hex),)* xf: fl::Xform::default(), style: Style::CLASSIC, waves: Vec::new() }
            }
        }

        impl Palette {
            /// Token names, in declaration order.
            pub const NAMES: &'static [&'static str] = &[$(stringify!($name),)*];

            pub fn get_mut(&mut self, name: &str) -> Option<&mut Color> {
                match name {
                    $(stringify!($name) => Some(&mut self.$name),)*
                    _ => None,
                }
            }

            pub fn get(&self, name: &str) -> Option<Color> {
                match name {
                    $(stringify!($name) => Some(self.$name),)*
                    _ => None,
                }
            }

            fn map(&self, f: impl Fn(Color) -> Color) -> Palette {
                Palette { $($name: f(self.$name),)* xf: self.xf.clone(), style: self.style, waves: self.waves.clone() }
            }
        }
    };
}

tokens! {
    toolbar = 0x2e3234, "Toolbar rows and their panels.";
    toolbar_line = 0x222628, "Separator lines between toolbar rows and groups.";
    toolbar_edge = 0x3a3f41, "Light top edge of toolbar groups.";
    sunken = 0x25282a, "Sunken panels: hint bar, monitors.";
    sunken_dark = 0x1c1e20, "CPU / memory panel.";
    time_panel = 0x2c3334, "Song position display.";
    menu_panel = 0x313838, "Menu bar background.";
    menu_text = 0xbcd0dc, "Menu bar items.";
    news_panel = 0x2a2e30, "News panel.";
    button = 0x292d2f, "Toolbar button face.";
    button_hi = 0x363b3d, "Toolbar button top highlight / hover.";
    glyph = 0xd3dade, "Icon glyphs and bright text.";
    glyph_dim = 0x6c7173, "Inactive glyphs and labels.";
    text = 0xd4dade, "Primary text.";
    text_dim = 0x7e8386, "Secondary text.";
    text_faint = 0x6d7275, "Faint labels (track names, numbers).";
    on_top = 0xffe193, "'On' button gradient, top.";
    on_bottom = 0xf59a3c, "'On' button gradient, bottom.";
    on_glyph = 0xb14000, "Glyph on an 'on' button.";
    pill = 0xf7a94c, "Active PAT/SONG pill.";
    pill_text = 0x3b1d00, "Text on the active pill.";
    record = 0xfb655e, "Record dot.";
    tempo_box = 0x3b3f41, "Tempo field.";
    accent = 0xa8e44a, "Green accent: LEDs, selection.";
    accent_dim = 0x4d6b22, "Dim accent: unlit LEDs.";
    workspace = 0x333438, "Workspace background.";
    watermark = 0x3a3c40, "Workspace logo watermark.";
    version_text = 0x555a5e, "Version string under the watermark.";
    browser_title = 0x292d2f, "Browser title bar.";
    browser_tabs = 0x1a1e1f, "Browser tab strip.";
    browser_tab_outline = 0xd0a080, "Active browser tab outline.";
    browser_list = 0x040809, "Browser list background.";
    browser_row_hi = 0x1e2224, "Highlighted browser row.";
    browser_folder = 0xa9bcc8, "Browser folder entries.";
    browser_footer = 0x16191b, "Browser footer (tags / search).";
    browser_search = 0xf08a30, "Search glyph.";
    cat_project = 0xc09080, "Browser category: current project.";
    cat_recent = 0x9bc58a, "Browser category: recent files.";
    cat_plugin_db = 0x93b4d6, "Browser category: plugin database.";
    cat_plugin_presets = 0xb98aa6, "Browser category: plugin presets.";
    cat_channel_presets = 0xb08ea8, "Browser category: channel presets.";
    cat_mixer_presets = 0xa890b8, "Browser category: mixer presets.";
    splitter = 0x272a2c, "Splitter between browser and windows.";
    splitter_dark = 0x1b1e20, "Splitter shadow, window borders.";
    window_title = 0x25292b, "Window title bars.";
    window_title_float = 0x25292b, "Floating window title bars.";
    window_title_hi = 0x2a2e30, "Top highlight row of floating title bars.";
    window_body = 0x25292b, "Floating window body / footer.";
    window_border = 0x191d1f, "Floating window outer border.";
    picker = 0x222627, "Playlist picker panel.";
    track_header = 0x292c2e, "Playlist track headers.";
    overview = 0x323537, "Playlist overview / scroll bars.";
    ruler = 0x0c1012, "Time rulers.";
    ruler_text = 0x607a7a, "Ruler numbers.";
    grid = 0x22272b, "Playlist grid rows.";
    grid_alt = 0x262b2f, "Alternate bar groups.";
    grid_gap = 0x1a1e1f, "Gap between playlist tracks / beat lines.";
    grid_bar = 0x0b0e11, "Bar lines.";
    scrollbar = 0x232729, "Scroll bar track.";
    playhead = 0x9fe05a, "Playback position line.";
    mixer_bg = 0x232527, "Mixer strip area.";
    mixer_strip = 0x25282b, "Mixer strip body.";
    mixer_strip_dark = 0x1d1f22, "Fader well / strip lower body.";
    mixer_panel = 0x313436, "Mixer right panel.";
    slot = 0x25282a, "Effect slot.";
    slot_text = 0x5d6366, "Empty slot label.";
    slot_text_used = 0xc9d0d4, "Occupied slot label.";
    fader_handle = 0x4b4c4f, "Fader handle.";
    meter_low = 0x6cc04a, "Meter, low level.";
    meter_high = 0xe8d040, "Meter, high level.";
    meter_clip = 0xf05040, "Meter, clipping.";
    rack_row_box = 0x2a2d2e, "Channel rack mixer-track box.";
    name_button = 0x2e3132, "Channel name button.";
    name_button_sel = 0x485156, "Selected channel name button.";
    name_text = 0xaab4bb, "Channel name text.";
    rack_inset = 0x313436, "Channel rack row panel.";
    rack_inset_border = 0x1e2224, "Channel rack row panel border and steps strip.";
    rack_control_border = 0x16191a, "Channel rack box / step borders.";
    step_a = 0x323a3e, "Step buttons, even groups of 4.";
    step_b = 0x724a4c, "Step buttons, odd groups of 4.";
    step_dim_a = 0x34383b, "Steps past the pattern length, even groups.";
    step_dim_b = 0x4d4042, "Steps past the pattern length, odd groups.";
    step_dim_border = 0x25282a, "Border of steps past the pattern length.";
    step_on = 0xd8dde0, "Lit step.";
    add_hover = 0xffbc40, "Hovered add-channel button outline.";
    menu_drop = 0x2b3032, "Dropdown menu background.";
    menu_drop_hi = 0x47555c, "Dropdown menu hovered row.";
    menu_drop_text = 0xd4dade, "Dropdown menu text.";
    menu_drop_disabled = 0x6d7275, "Dropdown menu, not yet available.";
    menu_pop = 0x505355, "Menu bar dropdown: body (spec: ui/menus.md).";
    menu_pop_border = 0x000000, "Menu bar dropdown: 2 px border.";
    menu_pop_hi = 0x03080c, "Menu bar dropdown: hovered row.";
    menu_pop_text = 0xc1cdd0, "Menu bar dropdown: item text.";
    menu_pop_shortcut = 0x9ea4a3, "Menu bar dropdown: shortcut text.";
    menu_pop_dim = 0x838584, "Menu bar dropdown: disabled / not available item text.";
    menu_pop_head = 0x2f3336, "Menu bar dropdown: section header and separator.";
    menu_pop_head_text = 0x6c7678, "Menu bar dropdown: section header text.";
    clip = 0x4c6a7c, "Default clip color.";
    pr_row = 0x22272b, "Piano roll: white-key rows.";
    pr_row_black = 0x1c2125, "Piano roll: black-key rows.";
    pr_row_sel = 0x302c30, "Piano roll: white-key rows while notes are selected.";
    pr_row_black_sel = 0x2a262a, "Piano roll: black-key rows while notes are selected.";
    pr_bar = 0x000307, "Piano roll: bar lines.";
    note_top = 0xb0e3b7, "Note body, top of the gradient.";
    note_bottom = 0x8ec195, "Note body, bottom of the gradient.";
    note_rim = 0x4b6652, "Note rim.";
    note_text = 0x3c5a41, "Note label.";
    note_sel_top = 0xef8f8f, "Selected note body, top.";
    note_sel_bottom = 0xd37373, "Selected note body, bottom.";
    note_sel_rim = 0x714243, "Selected note rim.";
    note_sel_text = 0xfbe9e9, "Selected note label.";
    key_white = 0xe5e9f0, "Piano keyboard: white keys (right end of the gradient).";
    key_white_left = 0xbfc3ca, "Piano keyboard: white keys (left end).";
    key_black = 0x4d4e50, "Piano keyboard: black keys (right end).";
    key_black_left = 0x2a2b2d, "Piano keyboard: black keys (left end).";
    key_c = 0xc6cad1, "Piano keyboard: C keys.";
    key_label = 0x5082b5, "Piano keyboard: octave labels.";
    key_lit = 0xffd094, "Piano keyboard: sounding white key (right end of the gradient).";
    key_lit_left = 0xdea367, "Piano keyboard: sounding white key (left end).";
    key_lit_edge = 0xa76c30, "Piano keyboard: sounding white key, right edge.";
    key_black_lit = 0xc98a4a, "Piano keyboard: sounding black key (right end).";
    key_black_lit_left = 0x7a4f24, "Piano keyboard: sounding black key (left end).";
    velocity = 0xacdfb3, "Velocity lollipops.";
    velocity_dot = 0x477a4e, "Velocity lollipop disc.";
}

/// Role-mapped literals: widgets that need a measured shade with no token of its own write it as
/// `t.s(0x313436)` (surface), `t.a(..)` (accent green), `t.h(..)` (orange highlight), `t.tx(..)` (text),
/// `t.led(..)` (enable LEDs). Under the default theme each returns the literal unchanged; under an imported theme
/// it follows the same mapping as the named tokens of that role (spec: ui/flstheme.md §4).
impl Palette {
    pub fn s(&self, hex: u32) -> Color {
        self.xf.surface(Color::rgb(hex))
    }
    pub fn a(&self, hex: u32) -> Color {
        self.xf.accent(Color::rgb(hex))
    }
    pub fn h(&self, hex: u32) -> Color {
        self.xf.highlight(Color::rgb(hex))
    }
    pub fn tx(&self, hex: u32) -> Color {
        self.xf.text(Color::rgb(hex))
    }
    pub fn led(&self, hex: u32) -> Color {
        self.xf.mute(Color::rgb(hex))
    }
}

static CURRENT: std::sync::RwLock<Option<std::sync::Arc<Palette>>> = std::sync::RwLock::new(None);

/// The palette in use by this process (set by the host when a theme loads; default palette until then).
/// Lets separately painted windows (plugin frames, plugin editors in-process) follow the app theme.
pub fn current() -> std::sync::Arc<Palette> {
    if let Some(p) = CURRENT.read().ok().and_then(|g| g.clone()) {
        return p;
    }
    std::sync::Arc::new(Palette::default())
}

pub fn set_current(p: &Palette) {
    if let Ok(mut g) = CURRENT.write() {
        *g = Some(std::sync::Arc::new(p.clone()));
    }
}

/// A loaded theme: the base palette with the global adjustment applied.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub name: String,
    pub adjust: Adjust,
    /// Unadjusted colors (defaults plus file overrides).
    pub base: Palette,
    /// What widgets paint with.
    pub colors: Palette,
}

impl Default for Theme {
    fn default() -> Self {
        Theme::new("Default", Palette::default(), Adjust::default())
    }
}

impl Theme {
    pub fn new(name: &str, base: Palette, adjust: Adjust) -> Self {
        let colors = base.map(|c| c.adjust(adjust));
        Theme { name: name.into(), adjust, base, colors }
    }

    pub fn set_adjust(&mut self, adjust: Adjust) {
        *self = Theme::new(&self.name, self.base.clone(), adjust);
    }

    /// Parse a theme file (see the module docs). Unknown keys are reported as errors.
    pub fn parse(name: &str, text: &str) -> Result<Theme, String> {
        let mut base = Palette::default();
        let mut adjust = Adjust::default();
        for (n, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (k, rest) = line.split_once('=').ok_or(format!("line {}: expected key = value", n + 1))?;
            // The value is the first word (colors start with `#` too); a trailing `# …` is a comment.
            let mut words = rest.split_whitespace();
            let (k, v) = (k.trim(), words.next().unwrap_or(""));
            if words.next().is_some_and(|w| !w.starts_with('#')) {
                return Err(format!("line {}: unexpected text after the value", n + 1));
            }
            let num = || v.parse::<f32>().map_err(|e| format!("line {}: {k}: {e}", n + 1));
            match k {
                "hue" => adjust.hue = num()?,
                "saturation" => adjust.saturation = num()?,
                "lightness" => adjust.lightness = num()?,
                _ => {
                    let slot = base.get_mut(k).ok_or(format!("line {}: unknown token {k}", n + 1))?;
                    *slot = Color::parse(v).ok_or(format!("line {}: bad color {v}", n + 1))?;
                }
            }
        }
        Ok(Theme::new(name, base, adjust))
    }

    /// Serialize as a theme file listing every token.
    pub fn to_text(&self) -> String {
        let mut s = format!(
            "# Lemon Studio theme: {}\nhue = {}\nsaturation = {}\nlightness = {}\n",
            self.name, self.adjust.hue, self.adjust.saturation, self.adjust.lightness
        );
        for name in Palette::NAMES {
            s += &format!("{name} = {}\n", self.base.get(name).expect("token"));
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsl_round_trip() {
        for c in [Color::rgb(0x2e3234), Color::rgb(0xa8e44a), Color::rgb(0xfb655e), Color::rgb(0x000000)] {
            let (h, s, l) = c.to_hsl();
            let back = Color::from_hsl(h, s, l, 255);
            for (a, b) in [(c.r, back.r), (c.g, back.g), (c.b, back.b)] {
                assert!(a.abs_diff(b) <= 1, "{c} -> {back}");
            }
        }
    }

    #[test]
    fn file_round_trip() {
        let mut t = Theme::default();
        t.set_adjust(Adjust { hue: 30.0, saturation: 0.2, lightness: -0.05 });
        let parsed = Theme::parse("Default", &t.to_text()).unwrap();
        assert_eq!(parsed, t);
    }

    #[test]
    fn override_and_comment() {
        let t = Theme::parse("x", "# comment\naccent = #112233 # trailing\nhue = 0\n").unwrap();
        assert_eq!(t.colors.accent, Color::rgb(0x112233));
        assert!(Theme::parse("x", "nope = #000000").is_err());
    }
}
