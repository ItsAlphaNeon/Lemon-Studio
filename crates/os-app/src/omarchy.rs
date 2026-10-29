// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Follow Omarchy: on an Omarchy desktop (omarchy.org) the theme follows the system's. Omarchy keeps its current theme
//! in `~/.local/state/omarchy/current/`: `theme.name` and the theme's folder, whose `colors.toml` names its colours by
//! role. They become a theme of the skin in use (`theme`): its four backgrounds the surfaces outright
//! (`os_theme::fl::SURFACES_KEY`), its accent the selection, highlight and switches, its foreground the text. The app
//! looks at the files once a second (`App::omarchy_tick`) and switches with them.
//!
//! Theme ▸ Options ▸ "Follow Omarchy's theme" shows only on Omarchy and is on by default there; choosing another theme
//! turns it off.

use std::path::PathBuf;
use std::time::SystemTime;

use os_theme::{fl, Color};

/// Settings key of the option.
pub const KEY: &str = "follow_omarchy";

/// Omarchy's current theme folder's parent (`theme.name`, `theme/`); `None` off Omarchy. Tests: this thread's folder.
fn dir() -> Option<PathBuf> {
    #[cfg(test)]
    return TEST_DIR.with(|d| d.borrow().clone());
    #[cfg(not(test))]
    {
        let d = PathBuf::from(std::env::var_os("HOME")?).join(".local/state/omarchy/current");
        d.join("theme/colors.toml").is_file().then_some(d)
    }
}

#[cfg(test)]
thread_local! {
    static TEST_DIR: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Tests: where `dir` is (`None`: not Omarchy).
#[cfg(test)]
pub fn set_dir(d: Option<PathBuf>) {
    TEST_DIR.with(|t| *t.borrow_mut() = d);
}

/// This is an Omarchy desktop.
pub fn available() -> bool {
    dir().is_some()
}

/// Whether the theme follows Omarchy's: the option, on Omarchy.
pub fn following() -> bool {
    available() && crate::settings_file::flag(KEY, true)
}

/// What says the system theme changed: the theme's name and when its colours were written.
pub fn stamp() -> Option<(String, Option<SystemTime>)> {
    let d = dir()?;
    let name = std::fs::read_to_string(d.join("theme.name")).unwrap_or_default().trim().to_string();
    Some((name, std::fs::metadata(d.join("theme/colors.toml")).and_then(|m| m.modified()).ok()))
}

/// Omarchy's current theme: its name as Omarchy shows it ("tokyo-night" is "Tokyo Night") and its colours as theme
/// text.
pub fn theme() -> Option<(String, String)> {
    let d = dir()?;
    let toml = std::fs::read_to_string(d.join("theme/colors.toml")).ok()?;
    let name = std::fs::read_to_string(d.join("theme.name")).unwrap_or_default();
    Some((title(name.trim()), theme_text(&toml)?))
}

/// "tokyo-night" as "Tokyo Night".
fn title(name: &str) -> String {
    let words: Vec<String> = name
        .split(['-', '_', ' '])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next().map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
        })
        .collect();
    if words.is_empty() { "Omarchy".into() } else { words.join(" ") }
}

/// The colours of a `colors.toml` (`key = "#rrggbb"` lines) as theme text; `None` without its backgrounds and
/// foreground.
pub fn theme_text(toml: &str) -> Option<String> {
    let get = |k: &str| {
        toml.lines().find_map(|l| {
            let (key, v) = l.split_once('=')?;
            (key.trim() == k).then(|| Color::parse(v.trim().trim_matches('"')))?
        })
    };
    let surfaces = fl::Surfaces([get("darker_background")?, get("dark_background")?, get("background")?, get("lighter_background")?]);
    let text = get("foreground")?;
    let accent = get("accent").or_else(|| get("blue"))?;
    let v = |key: &str, c: Color| format!("{key}={}\n", fl::colour_value(key, c));
    // The rest is for FL, which ignores Lemon Studio's key: neutral surfaces, as dark or light as the theme.
    let lightness = if surfaces.light() { 200 } else { -153 };
    Some(format!(
        "Hue=0\nSaturation=-256\nLightness={lightness}\nContrast=-8\nText=0\n{}{}{}{}BackMode=2\n{}={}\n",
        v("Selected", accent),
        v("Highlight", accent),
        v("Mute", accent),
        v("TextColor", text),
        fl::SURFACES_KEY,
        surfaces.text()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVERFOREST: &str = "mode = \"dark\"\n\naccent = \"#7fbbb3\"\nselection = \"#3d484d\"\n\nbackground = \"#2d353b\"\n\
        dark_background = \"#21272c\"\ndarker_background = \"#181d20\"\nlighter_background = \"#343f44\"\n\nforeground = \"#d3c6aa\"\n";

    /// On Omarchy the theme follows the system's from the start and switches with it; turned off, the theme kept from
    /// before comes back; off Omarchy there is no option. A temp folder, never the real one.
    #[test]
    fn the_theme_follows_omarchy() {
        let dir = std::env::temp_dir().join(format!("lemon-omarchy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("theme")).unwrap();
        let set = |name: &str, toml: &str| {
            std::fs::write(dir.join("theme.name"), name).unwrap();
            std::fs::write(dir.join("theme/colors.toml"), toml).unwrap();
        };
        set("everforest", EVERFOREST);
        set_dir(Some(dir.clone()));
        let mut app = crate::app::App::new(None);
        assert!(following());
        assert_eq!(app.theme.name, "Everforest");
        assert!(app.state.fl_text.as_deref().is_some_and(|t| t.contains(fl::SURFACES_KEY)));
        let later = |app: &crate::app::App| app.omarchy.1 + std::time::Duration::from_millis(10);
        set("tokyo-night", &EVERFOREST.replace("#2d353b", "#1a1b26"));
        let t = later(&app);
        app.omarchy_tick(t);
        assert_eq!(app.theme.name, "Tokyo Night");
        app.toggle_follow_omarchy();
        assert!(!following());
        assert_ne!(app.theme.name, "Tokyo Night", "the theme from before");
        assert_eq!(app.state.fl_text, None);
        let t = later(&app);
        app.omarchy_tick(t);
        assert_ne!(app.theme.name, "Tokyo Night", "not followed while off");
        app.toggle_follow_omarchy();
        assert_eq!(app.theme.name, "Tokyo Night");
        set_dir(None);
        assert!(!available() && !following());
        crate::settings_file::write(KEY, "");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_as_omarchy_shows_them() {
        assert_eq!(title("tokyo-night"), "Tokyo Night");
        assert_eq!(title("everforest"), "Everforest");
        assert_eq!(title(""), "Omarchy");
    }

    /// Omarchy's backgrounds become the surfaces outright, on either skin: the Lemon skin's toolbar is the background,
    /// its workspace the dark background, its browser list the darker one; the accent selects, the foreground is text.
    #[test]
    fn colours_become_a_theme() {
        let text = theme_text(EVERFOREST).expect("a theme");
        let th = os_theme::FlTheme::parse(&text);
        assert_eq!(th.surfaces.map(|s| s.0[2]), Color::parse("#2d353b"));
        assert_eq!(th.selected, Color::parse("#7fbbb3"));
        assert_eq!(th.text_color, Color::parse("#d3c6aa"));
        let p = os_theme::design::palette(os_theme::Design::Lemon, Some(&th));
        let near = |a: Color, hex: &str| {
            let b = Color::parse(hex).unwrap();
            (i16::from(a.r) - i16::from(b.r)).abs() <= 2 && (i16::from(a.g) - i16::from(b.g)).abs() <= 2 && (i16::from(a.b) - i16::from(b.b)).abs() <= 2
        };
        assert!(near(p.toolbar, "#2d353b"), "{:?}", p.toolbar);
        assert!(near(p.workspace, "#21272c"), "{:?}", p.workspace);
        assert!(near(p.browser_list, "#181d20"), "{:?}", p.browser_list);
        assert!(near(p.button, "#343f44"), "{:?}", p.button);
        assert_eq!(theme_text("background = \"#000000\"\n"), None, "not a whole theme");
    }
}
