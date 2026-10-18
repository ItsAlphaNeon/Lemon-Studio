// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The system's look on Linux and the other Unixes (spec: ui/etched-design.md §2). There is no one place for it:
//!
//! - Dark or light and the accent: the desktop portal (`org.freedesktop.appearance`), which GNOME, KDE, Cinnamon,
//!   COSMIC and others answer; else the desktop's own settings.
//! - The desktop picture: each desktop keeps its own. The current one's reader goes first (`XDG_CURRENT_DESKTOP`),
//!   then the others: what is running (swww, hyprpaper, swaybg) before what is only written down (config files, which
//!   may be left from another desktop), GSettings last (it answers with its defaults even where GNOME is not used).
//!
//! The parsers take text and are tested on every platform; the readers around them run the tools and read the files.

use super::{triple, Desktop, Fit};
use os_theme::Color;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// What a desktop says it shows: the picture (none: a plain colour), how it is laid out, the colour around it.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub path: Option<PathBuf>,
    pub fit: Fit,
    pub colour: Option<Color>,
}

impl Found {
    fn picture(path: PathBuf, fit: Fit) -> Found {
        Found { path: Some(path), fit, colour: None }
    }

    /// Something to show: a picture or a colour.
    fn any(self) -> Option<Found> {
        (self.path.is_some() || self.colour.is_some()).then_some(self)
    }
}

// ---------------------------------------------------------------- the readers

/// A desktop picture reader (`dark`: the dark variant where there is one).
type Reader = fn(bool) -> Option<Found>;

/// Every reader, in the order tried when the current desktop's own found nothing.
const ALL: [Reader; 14] = [swww, hyprpaper, swaybg, xfce, kde, cosmic, lxqt, lxde, sway, feh, nitrogen, cinnamon, mate, gnome];

/// The readers of the desktops named in `XDG_CURRENT_DESKTOP` (a `:`-separated list, as "ubuntu:GNOME").
fn current(names: &str) -> Vec<Reader> {
    let mut v: Vec<Reader> = Vec::new();
    for name in names.split(':').map(|n| n.trim().to_ascii_lowercase()) {
        let r: &[Reader] = match name.as_str() {
            "gnome" | "ubuntu" | "budgie" | "budgie-desktop" | "pantheon" | "unity" | "gnome-flashback" | "gnome-classic" | "pop" | "deepin" => &[gnome],
            "x-cinnamon" | "cinnamon" => &[cinnamon],
            "mate" => &[mate],
            "xfce" => &[xfce],
            "kde" | "plasma" => &[kde],
            "lxqt" => &[lxqt],
            "lxde" => &[lxde],
            "cosmic" => &[cosmic],
            "hyprland" => &[hyprpaper, swww, swaybg],
            "sway" | "river" | "niri" | "wayfire" | "labwc" | "wlroots" => &[swww, swaybg, sway],
            "i3" | "bspwm" | "openbox" | "awesome" | "qtile" | "dwm" | "herbstluftwm" | "fluxbox" | "icewm" => &[feh, nitrogen],
            _ => &[],
        };
        for f in r {
            if !v.iter().any(|g| std::ptr::fn_addr_eq(*g, *f)) {
                v.push(*f);
            }
        }
    }
    v
}

pub fn desktop(dark: bool, _picture: Option<PathBuf>) -> Desktop {
    let names = std::env::var("XDG_CURRENT_DESKTOP").or_else(|_| std::env::var("DESKTOP_SESSION")).unwrap_or_default();
    let mut readers = current(&names);
    let rest: Vec<Reader> = ALL.into_iter().filter(|f| !readers.iter().any(|g| std::ptr::fn_addr_eq(*g, *f))).collect();
    readers.extend(rest);
    let Some(found) = readers.into_iter().find_map(|r| r(dark)) else {
        return Desktop { picture: None, fit: Fit::Fill, colour: None };
    };
    let picture = found.path.and_then(|p| resolve(&p, dark)).and_then(|p| super::load(&p));
    Desktop { picture, fit: found.fit, colour: found.colour }
}

/// The picture file a desktop's setting leads to: itself, the right picture of a KDE wallpaper package (a folder), or
/// the first picture of a GNOME slideshow (an XML file).
fn resolve(p: &Path, dark: bool) -> Option<PathBuf> {
    if p.is_dir() {
        return package_picture(p, dark);
    }
    if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("xml")) {
        return slideshow_first(&std::fs::read_to_string(p).ok()?).map(PathBuf::from).filter(|f| f.is_file());
    }
    p.is_file().then(|| p.to_path_buf())
}

/// A KDE wallpaper package's picture for a screen (its dark ones in dark mode, where it has them).
fn package_picture(dir: &Path, dark: bool) -> Option<PathBuf> {
    let images = dir.join("contents");
    let dark_dir = images.join("images_dark");
    let at = if dark && dark_dir.is_dir() { dark_dir } else { images.join("images") };
    let names: Vec<PathBuf> = std::fs::read_dir(at).ok()?.filter_map(|e| e.ok().map(|e| e.path())).collect();
    screen_sized(&names)
}

fn gnome(dark: bool) -> Option<Found> {
    let s = "org.gnome.desktop.background";
    let uri = if dark { gsettings(s, "picture-uri-dark").filter(|u| !u.is_empty()) } else { None }.or_else(|| gsettings(s, "picture-uri"));
    gsettings_found(s, uri.map(|u| from_uri(&u)))
}

fn cinnamon(_dark: bool) -> Option<Found> {
    let s = "org.cinnamon.desktop.background";
    gsettings_found(s, gsettings(s, "picture-uri").map(|u| from_uri(&u)))
}

fn mate(_dark: bool) -> Option<Found> {
    let s = "org.mate.background";
    gsettings_found(s, gsettings(s, "picture-filename").map(PathBuf::from))
}

/// A GNOME-like desktop's picture, how it is laid and the colour, from schema `s`.
fn gsettings_found(s: &str, path: Option<PathBuf>) -> Option<Found> {
    let options = gsettings(s, "picture-options").unwrap_or_default();
    let path = path.filter(|p| !p.as_os_str().is_empty() && options != "none");
    let colour = gsettings(s, "primary-color").and_then(|c| hex(&c));
    Found { path, fit: gnome_fit(&options), colour }.any()
}

fn xfce(_dark: bool) -> Option<Found> {
    xfce_found(&run("xfconf-query", &["-c", "xfce4-desktop", "-lv"])?)
}

fn kde(dark: bool) -> Option<Found> {
    let _ = dark;
    kde_found(&config("plasma-org.kde.plasma.desktop-appletsrc")?, &kde_default())
}

/// The wallpaper package Plasma shows while none is set: the one its desktop theme names (`[Wallpaper]
/// defaultWallpaperTheme` in the theme's `plasmarc`), else Next.
fn kde_default() -> PathBuf {
    let rc = |name: &str| config(name).and_then(|t| ini(&t, "Theme", "name"));
    let theme = rc("plasmarc").or_else(|| rc("kdedefaults/plasmarc")).unwrap_or_else(|| "default".into());
    let named = data_dirs().into_iter().find_map(|d| {
        let t = std::fs::read_to_string(d.join("plasma/desktoptheme").join(&theme).join("plasmarc")).ok()?;
        ini(&t, "Wallpaper", "defaultWallpaperTheme")
    });
    let name = named.unwrap_or_else(|| "Next".into());
    data_dirs().iter().map(|d| d.join("wallpapers").join(&name)).find(|p| p.is_dir()).unwrap_or_else(|| PathBuf::from("/usr/share/wallpapers").join(name))
}

fn cosmic(_dark: bool) -> Option<Found> {
    cosmic_found(&config("cosmic/com.system76.CosmicBackground/v1/all")?)
}

fn lxqt(_dark: bool) -> Option<Found> {
    lxqt_found(&config("pcmanfm-qt/lxqt/settings.conf")?)
}

fn lxde(_dark: bool) -> Option<Found> {
    let text = config("pcmanfm/LXDE/desktop-items-0.conf").or_else(|| config("pcmanfm/default/desktop-items-0.conf"))?;
    lxde_found(&text)
}

fn swww(_dark: bool) -> Option<Found> {
    swww_found(&run("swww", &["query"])?)
}

fn hyprpaper(_dark: bool) -> Option<Found> {
    run("hyprctl", &["hyprpaper", "listactive"])
        .and_then(|t| hyprpaper_found(&t))
        .or_else(|| hyprpaper_found(&config("hypr/hyprpaper.conf")?))
}

/// swaybg as it runs (Sway and other wlroots compositors start it with the picture on its command line).
fn swaybg(_dark: bool) -> Option<Found> {
    let procs = std::fs::read_dir("/proc").ok()?;
    procs.flatten().find_map(|e| {
        let dir = e.path();
        let comm = std::fs::read_to_string(dir.join("comm")).ok()?;
        if comm.trim() != "swaybg" {
            return None;
        }
        let cmd = std::fs::read(dir.join("cmdline")).ok()?;
        let args: Vec<String> = cmd.split(|&b| b == 0).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
        swaybg_found(&args)
    })
}

fn sway(_dark: bool) -> Option<Found> {
    sway_found(&config("sway/config")?)
}

fn feh(_dark: bool) -> Option<Found> {
    feh_found(&std::fs::read_to_string(home()?.join(".fehbg")).ok()?)
}

fn nitrogen(_dark: bool) -> Option<Found> {
    nitrogen_found(&config("nitrogen/bg-saved.cfg")?)
}

// ---------------------------------------------------------------- dark or light, the accent

pub fn dark() -> Option<bool> {
    // 1 = prefers dark, 2 = prefers light, 0 = no preference (the desktop's own setting decides).
    match portal("color-scheme").and_then(|t| numbers(&t).first().copied()) {
        Some(v) if v == 1.0 => return Some(true),
        Some(v) if v == 2.0 => return Some(false),
        _ => {}
    }
    if kde_session() {
        let g = config("kdeglobals")?;
        if let Some(bg) = ini(&g, "Colors:Window", "BackgroundNormal").and_then(|s| triple(&s)) {
            return Some(bg.hsl().2 < 0.5);
        }
        return ini(&g, "General", "ColorScheme").map(|s| s.to_ascii_lowercase().contains("dark"));
    }
    if let Some(s) = gsettings("org.gnome.desktop.interface", "color-scheme") {
        if s != "default" {
            return Some(s == "prefer-dark");
        }
    }
    gsettings("org.gnome.desktop.interface", "gtk-theme").map(|t| t.to_ascii_lowercase().contains("dark"))
}

pub fn accent() -> Option<Color> {
    if let Some(c) = portal("accent-color").and_then(|t| portal_colour(&t)) {
        return Some(c);
    }
    if kde_session() {
        if let Some(c) = config("kdeglobals").and_then(|g| ini(&g, "General", "AccentColor")).and_then(|s| triple(&s)) {
            return Some(c);
        }
    }
    gsettings("org.gnome.desktop.interface", "accent-color").map(|n| gnome_accent(&n))
}

/// A value of the desktop portal's `org.freedesktop.appearance` settings, as the tool printed it (gdbus, else
/// busctl).
fn portal(key: &str) -> Option<String> {
    let dest = "org.freedesktop.portal.Desktop";
    let path = "/org/freedesktop/portal/desktop";
    let ns = "org.freedesktop.appearance";
    for method in ["ReadOne", "Read"] {
        let m = format!("org.freedesktop.portal.Settings.{method}");
        if let Some(t) = run("gdbus", &["call", "--session", "--timeout", "2", "--dest", dest, "--object-path", path, "--method", &m, ns, key]) {
            return Some(t);
        }
        if let Some(t) = run("busctl", &["--user", "--timeout=2", "call", dest, path, "org.freedesktop.portal.Settings", method, "ss", ns, key]) {
            return Some(t);
        }
    }
    None
}

/// The portal's accent (three channels 0..1; outside that: none set).
fn portal_colour(text: &str) -> Option<Color> {
    let v = numbers(text);
    let c = v.get(v.len().checked_sub(3)?..)?;
    if c.iter().any(|x| !(0.0..=1.0).contains(x)) {
        return None;
    }
    let q = |x: f64| (x * 255.0).round() as u32;
    Some(Color::rgb(q(c[0]) << 16 | q(c[1]) << 8 | q(c[2])))
}

/// The numbers in a tool's printout of a D-Bus value ("(<uint32 1>,)", "v (ddd) 0.2 0.5 0.9"): its words that are
/// numbers whole.
fn numbers(text: &str) -> Vec<f64> {
    text.split(|c: char| c.is_whitespace() || "()<>[],".contains(c)).filter_map(|w| w.parse().ok()).collect()
}

fn gnome_accent(name: &str) -> Color {
    Color::rgb(match name {
        "teal" => 0x2190a4,
        "green" => 0x3a944a,
        "yellow" => 0xc88800,
        "orange" => 0xed5b00,
        "red" => 0xe62d42,
        "pink" => 0xd56199,
        "purple" => 0x9141ac,
        "slate" => 0x6f8396,
        _ => 0x3584e4,
    })
}

// ---------------------------------------------------------------- the parsers

/// GNOME's (and Cinnamon's, MATE's) `picture-options`.
fn gnome_fit(o: &str) -> Fit {
    match o {
        "stretched" => Fit::Stretch,
        "scaled" | "centered" => Fit::Fit,
        _ => Fit::Fill,
    }
}

/// The first picture of a GNOME slideshow (`<static><file>…</file>`, or the largest `<size>` in it).
fn slideshow_first(xml: &str) -> Option<String> {
    let body = between(xml, "<file>", "</file>")?;
    if !body.contains("<size") {
        return Some(body.trim().to_string());
    }
    // `<size width="1920" height="1080">path</size>` …: the widest.
    body.split("<size").skip(1).filter_map(|s| {
        let w = between(s, "width=\"", "\"")?.parse::<u32>().ok()?;
        let path = between(s, ">", "</size>")?.trim().to_string();
        Some((w, path))
    })
    .max_by_key(|(w, _)| *w)
    .map(|(_, p)| p)
}

/// XFCE's desktop settings (`xfconf-query -c xfce4-desktop -lv`: a property and its value a line): workspace 0 of the
/// first monitor that has a picture.
fn xfce_found(text: &str) -> Option<Found> {
    let props: Vec<(&str, &str)> = text.lines().filter_map(|l| l.split_once(char::is_whitespace)).map(|(k, v)| (k.trim(), v.trim())).collect();
    let image = props
        .iter()
        .filter(|(k, v)| k.ends_with("/last-image") && !v.is_empty())
        .min_by_key(|(k, _)| !k.contains("/workspace0/"))?;
    let base = image.0.trim_end_matches("/last-image");
    let style = props.iter().find(|(k, _)| k.strip_prefix(base) == Some("/image-style")).and_then(|(_, v)| v.parse::<u32>().ok());
    // 0 none, 1 centred, 2 tiled, 3 stretched, 4 scaled, 5 zoomed.
    let fit = match style {
        Some(3) => Fit::Stretch,
        Some(1) | Some(4) => Fit::Fit,
        _ => Fit::Fill,
    };
    (style != Some(0)).then(|| Found::picture(PathBuf::from(image.1), fit))
}

/// Plasma's desktop: the containment on the first screen that shows a picture (`org.kde.image`), its picture and fill.
fn kde_found(text: &str, default: &Path) -> Option<Found> {
    let sections = sections(text);
    // Desktop containments with the picture plugin, the first screen's first.
    let mut desks: Vec<(&str, i64)> = sections
        .iter()
        .filter(|(name, keys)| name.starts_with("Containments][") && name.matches("][").count() == 1 && get(keys, "wallpaperplugin") == Some("org.kde.image"))
        .map(|(name, keys)| (*name, get(keys, "lastScreen").and_then(|s| s.parse().ok()).unwrap_or(i64::MAX)))
        .collect();
    desks.sort_by_key(|d| d.1);
    // A wallpaper never changed from the default has no section or no Image: Plasma shows its default package.
    desks.into_iter().next().map(|(name, _)| {
        let keys = sections.iter().find(|(n, _)| *n == format!("{name}][Wallpaper][org.kde.image][General")).map(|s| s.1.clone()).unwrap_or_default();
        let path = get(&keys, "Image").filter(|i| !i.is_empty()).map_or_else(|| default.to_path_buf(), from_uri);
        // Qt's fill modes: 0 stretch, 1 fit, 2 crop (the default), 6 pad.
        let fit = match get(&keys, "FillMode").and_then(|v| v.parse::<u32>().ok()) {
            Some(0) => Fit::Stretch,
            Some(1) | Some(6) => Fit::Fit,
            _ => Fit::Fill,
        };
        let colour = get(&keys, "Color").and_then(triple);
        Found { path: Some(path), fit, colour }
    })
}

/// COSMIC's background (RON): `source: Path("…")` and `scaling_mode`.
fn cosmic_found(text: &str) -> Option<Found> {
    let path = between(text, "Path(\"", "\")")?;
    let fit = if text.contains("Stretch") {
        Fit::Stretch
    } else if text.contains("scaling_mode: Fit") {
        Fit::Fit
    } else {
        Fit::Fill
    };
    Some(Found::picture(PathBuf::from(path), fit))
}

/// LXQt's desktop (pcmanfm-qt): `[Desktop]` `Wallpaper`, `WallpaperMode`, `BgColor`.
fn lxqt_found(text: &str) -> Option<Found> {
    let mode = ini(text, "Desktop", "WallpaperMode").unwrap_or_default();
    let path = ini(text, "Desktop", "Wallpaper").filter(|p| !p.is_empty() && mode != "none").map(|p| expand(&p));
    let fit = match mode.as_str() {
        "stretch" => Fit::Stretch,
        "fit" | "center" => Fit::Fit,
        _ => Fit::Fill,
    };
    Found { path, fit, colour: ini(text, "Desktop", "BgColor").and_then(|c| hex(&c)) }.any()
}

/// LXDE's desktop (pcmanfm): `[*]` `wallpaper`, `wallpaper_mode`, `desktop_bg`.
fn lxde_found(text: &str) -> Option<Found> {
    let mode = ini(text, "*", "wallpaper_mode").unwrap_or_default();
    let path = ini(text, "*", "wallpaper").filter(|p| !p.is_empty() && mode != "color").map(|p| expand(&p));
    let fit = match mode.as_str() {
        "stretch" => Fit::Stretch,
        "fit" | "center" => Fit::Fit,
        _ => Fit::Fill,
    };
    Found { path, fit, colour: ini(text, "*", "desktop_bg").and_then(|c| hex(&c)) }.any()
}

/// `swww query`: a line an output, "…, currently displaying: image: /path".
fn swww_found(text: &str) -> Option<Found> {
    text.lines().find_map(|l| l.split_once("image: ")).map(|(_, p)| Found::picture(PathBuf::from(p.trim()), Fit::Fill))
}

/// hyprpaper: `hyprctl hyprpaper listactive` ("DP-1 = /path"), or its config (`wallpaper = DP-1,/path`, or a
/// `wallpaper { path = … }` block).
fn hyprpaper_found(text: &str) -> Option<Found> {
    text.lines().map(|l| l.split('#').next().unwrap_or("").trim()).find_map(|l| {
        let (key, value) = l.split_once('=')?;
        let (key, value) = (key.trim(), value.trim());
        let path = match key {
            "wallpaper" => value.rsplit_once(',').map_or(value, |(_, p)| p).trim(),
            "path" => value,
            // `listactive`: the output's name.
            k if !k.is_empty() && !k.contains(' ') && value.starts_with('/') => value,
            _ => return None,
        };
        let path = path.strip_prefix("contain:").unwrap_or(path);
        (!path.is_empty() && path != "{").then(|| Found::picture(expand(path), Fit::Fill))
    })
}

/// swaybg's command line: `-i picture`, `-m mode`, `-c colour`.
fn swaybg_found(args: &[String]) -> Option<Found> {
    let arg = |short: &str, long: &str| args.windows(2).find(|w| w[0] == short || w[0] == long).map(|w| w[1].as_str());
    let fit = match arg("-m", "--mode") {
        Some("stretch") => Fit::Stretch,
        Some("fit") | Some("center") => Fit::Fit,
        _ => Fit::Fill,
    };
    Found { path: arg("-i", "--image").map(expand), fit, colour: arg("-c", "--color").and_then(hex) }.any()
}

/// Sway's config: `output <name> bg <path> <mode> [colour]` (also spelled `background`).
fn sway_found(text: &str) -> Option<Found> {
    text.lines().find_map(|l| {
        // Comments are whole lines (a `#` later on is a colour).
        let words = shell_words(l);
        if words.first().map(String::as_str) != Some("output") {
            return None;
        }
        let at = words.iter().position(|w| w == "bg" || w == "background")?;
        let path = words.get(at + 1)?;
        let mode = words.get(at + 2).map(String::as_str);
        if mode == Some("solid_color") {
            return Found { path: None, fit: Fit::Fill, colour: hex(path) }.any();
        }
        let fit = match mode {
            Some("stretch") => Fit::Stretch,
            Some("fit") | Some("center") => Fit::Fit,
            _ => Fit::Fill,
        };
        Some(Found::picture(expand(path), fit))
    })
}

/// `~/.fehbg`: `feh --no-fehbg --bg-fill '/path'`.
fn feh_found(text: &str) -> Option<Found> {
    text.lines().find_map(|l| {
        let words = shell_words(l);
        let at = words.iter().position(|w| w.starts_with("--bg-"))?;
        let fit = match words[at].as_str() {
            "--bg-scale" => Fit::Stretch,
            "--bg-max" | "--bg-center" => Fit::Fit,
            _ => Fit::Fill,
        };
        let path = words[at + 1..].iter().find(|w| !w.starts_with('-'))?;
        Some(Found::picture(expand(path), fit))
    })
}

/// nitrogen's `bg-saved.cfg`: the first screen's `file`, `mode`, `bgcolor`.
fn nitrogen_found(text: &str) -> Option<Found> {
    let s = sections(text);
    let (_, keys) = s.iter().find(|(_, k)| get(k, "file").is_some())?;
    // 1 scaled, 2 centred, 4 zoomed (fit), 5 zoomed fill.
    let fit = match get(keys, "mode").and_then(|m| m.parse::<u32>().ok()) {
        Some(1) => Fit::Stretch,
        Some(2) | Some(4) => Fit::Fit,
        _ => Fit::Fill,
    };
    Some(Found { path: get(keys, "file").map(expand), fit, colour: get(keys, "bgcolor").and_then(hex) })
}

// ---------------------------------------------------------------- helpers

/// An ini-like file's sections: (name without the outer brackets, its key = value lines).
fn sections(text: &str) -> Vec<(&str, Vec<(&str, &str)>)> {
    let mut out: Vec<(&str, Vec<(&str, &str)>)> = Vec::new();
    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            out.push((name, Vec::new()));
        } else if let (Some((k, v)), Some(last)) = (line.split_once('='), out.last_mut()) {
            last.1.push((k.trim(), v.trim()));
        }
    }
    out
}

fn get<'a>(keys: &[(&str, &'a str)], key: &str) -> Option<&'a str> {
    keys.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// `key=` in `[section]` of an ini-like file.
fn ini(text: &str, section: &str, key: &str) -> Option<String> {
    sections(text).into_iter().find(|(n, _)| *n == section).and_then(|(_, keys)| get(&keys, key).map(str::to_string))
}

/// The text between the first `a` and the `b` after it.
fn between<'a>(s: &'a str, a: &str, b: &str) -> Option<&'a str> {
    let from = s.find(a)? + a.len();
    let len = s[from..].find(b)?;
    Some(&s[from..from + len])
}

/// A line split into words as a shell would (single and double quotes, no expansion).
fn shell_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let (mut word, mut quote, mut any) = (String::new(), None, false);
    for c in line.chars() {
        match (quote, c) {
            (None, '\'' | '"') => {
                quote = Some(c);
                any = true;
            }
            (Some(q), _) if c == q => quote = None,
            (None, c) if c.is_whitespace() => {
                if any {
                    words.push(std::mem::take(&mut word));
                    any = false;
                }
            }
            _ => {
                word.push(c);
                any = true;
            }
        }
    }
    if any {
        words.push(word);
    }
    words
}

/// "#rrggbb" (or "rrggbb", or GNOME's "#rrrrggggbbbb").
fn hex(s: &str) -> Option<Color> {
    let h = s.trim().trim_matches('\'').trim_start_matches('#');
    let v = match h.len() {
        6 => u32::from_str_radix(h, 16).ok()?,
        12 => {
            let c = |i: usize| u32::from_str_radix(&h[i..i + 2], 16).ok();
            c(0)? << 16 | c(4)? << 8 | c(8)?
        }
        _ => return None,
    };
    Some(Color::rgb(v))
}

/// A `file://` URI's path (%20 and the like decoded), or a path as it is.
fn from_uri(s: &str) -> PathBuf {
    let s = s.strip_prefix("file://").unwrap_or(s);
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    PathBuf::from(String::from_utf8_lossy(&out).into_owned())
}

/// `~/…` and `$HOME/…` from the home folder.
fn expand(p: &str) -> PathBuf {
    let rest = p.strip_prefix("~/").or_else(|| p.strip_prefix("$HOME/"));
    match (rest, home()) {
        (Some(r), Some(h)) => h.join(r),
        _ => PathBuf::from(p),
    }
}

/// Of the pictures named `<w>x<h>.<ext>` (KDE's packages), the one shaped most like a 16:9 screen, the largest of
/// those (a package also holds phone and 32:9 ones, which a window crops badly); else the first.
fn screen_sized(paths: &[PathBuf]) -> Option<PathBuf> {
    let key = |p: &PathBuf| {
        let (w, h) = p.file_stem()?.to_str()?.split_once('x')?;
        let (w, h) = (w.parse::<u64>().ok()?, h.parse::<u64>().ok()?);
        let off = ((w as f64 / h.max(1) as f64).ln() - (16.0f64 / 9.0).ln()).abs();
        Some((std::cmp::Reverse((off * 100.0).round() as u64), w * h))
    };
    let pictures = paths.iter().filter(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg" | "webp" | "jxl" | "avif" | "svg")));
    pictures.max_by_key(|p| key(p).unwrap_or((std::cmp::Reverse(u64::MAX), 0))).cloned()
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).filter(|h| h.is_absolute())
}

/// The folders data is looked up in: the user's first, then the system's (`XDG_DATA_HOME`, `XDG_DATA_DIRS`).
fn data_dirs() -> Vec<PathBuf> {
    let user = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|d| d.is_absolute()).or_else(|| home().map(|h| h.join(".local/share")));
    let system = std::env::var("XDG_DATA_DIRS").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    user.into_iter().chain(system.split(':').map(PathBuf::from).filter(|d| d.is_absolute())).collect()
}

/// A file under the user's config folder.
fn config(name: &str) -> Option<String> {
    let dir = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|d| d.is_absolute()).or_else(|| home().map(|h| h.join(".config")))?;
    std::fs::read_to_string(dir.join(name)).ok()
}

fn kde_session() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.to_ascii_uppercase().contains("KDE"))
}

/// `gsettings get schema key`, its quotes taken off (none when gsettings or the schema is missing).
fn gsettings(schema: &str, key: &str) -> Option<String> {
    run("gsettings", &["get", schema, key]).map(|s| s.trim().trim_matches('\'').to_string())
}

/// A tool's output when it ran and succeeded within a few seconds (none when it is missing, fails or hangs).
fn run(tool: &str, args: &[&str]) -> Option<String> {
    let mut child = Command::new(tool).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut out = String::new();
                if status.success() {
                    use std::io::Read;
                    child.stdout.take()?.read_to_string(&mut out).ok()?;
                    return Some(out.trim().to_string()).filter(|s| !s.is_empty());
                }
                return None;
            }
            Ok(None) if start.elapsed() < Duration::from_secs(3) => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pic(p: &str, fit: Fit) -> Option<Found> {
        Some(Found::picture(PathBuf::from(p), fit))
    }

    #[test]
    fn current_desktops_go_first() {
        let r = current("ubuntu:GNOME");
        assert_eq!(r.len(), 1);
        assert!(std::ptr::fn_addr_eq(r[0], gnome as Reader));
        assert!(std::ptr::fn_addr_eq(current("X-Cinnamon")[0], cinnamon as Reader));
        assert!(std::ptr::fn_addr_eq(current("Hyprland")[0], hyprpaper as Reader));
        assert!(current("SomethingNew").is_empty());
    }

    #[test]
    fn kde_takes_the_first_screens_picture() {
        let text = "[Containments][7]\nlastScreen=1\nplugin=org.kde.plasma.folder\nwallpaperplugin=org.kde.image\n\n\
            [Containments][7][Wallpaper][org.kde.image][General]\nImage=file:///home/u/Second%20screen.jpg\n\n\
            [Containments][2]\nlastScreen=0\nwallpaperplugin=org.kde.image\n\n\
            [Containments][2][Wallpaper][org.kde.image][General]\nFillMode=1\nImage=/usr/share/wallpapers/Next/\n\n\
            [Containments][9]\nlastScreen=0\nplugin=org.kde.panel\n";
        assert_eq!(kde_found(text, Path::new("/d")), pic("/usr/share/wallpapers/Next/", Fit::Fit));
    }

    #[test]
    fn kde_never_changed_shows_the_default_package() {
        // As Plasma 6 writes it before the wallpaper is first changed: no [Wallpaper] section at all.
        let text = "[ActionPlugins][0]\nRightButton;NoModifier=org.kde.contextmenu\n\n\
            [Containments][219]\nItemGeometries-1536x864=\nactivityId=00000000-0000-0000-0000-000000000001\nformfactor=0\nimmutability=1\nlastScreen=0\nlocation=0\nplugin=org.kde.plasma.folder\nwallpaperplugin=org.kde.image\n\n\
            [Containments][220]\nactivityId=\nformfactor=2\nlastScreen=0\nlocation=4\nplugin=org.kde.panel\nwallpaperplugin=org.kde.image\n\n\
            [Containments][220][Applets][221]\nimmutability=1\nplugin=org.kde.plasma.kickoff\n";
        assert_eq!(kde_found(text, Path::new("/usr/share/wallpapers/Next")), pic("/usr/share/wallpapers/Next", Fit::Fill));
        let empty = "[Containments][1]\nlastScreen=0\nwallpaperplugin=org.kde.image\n\n[Containments][1][Wallpaper][org.kde.image][General]\nFillMode=1\nImage=\n";
        assert_eq!(kde_found(empty, Path::new("/d")), pic("/d", Fit::Fit));
        assert_eq!(kde_found("[Containments][1]\nwallpaperplugin=org.kde.color\n", Path::new("/d")), None);
    }

    #[test]
    fn packages_give_the_screen_shaped_picture() {
        let names: Vec<PathBuf> = ["1440x2960.png", "7680x2160.png", "1920x1080.png", "5120x2880.png", "metadata.json"].iter().map(PathBuf::from).collect();
        assert_eq!(screen_sized(&names), Some(PathBuf::from("5120x2880.png")));
        assert_eq!(screen_sized(&[PathBuf::from("a.jpg")]), Some(PathBuf::from("a.jpg")));
    }

    #[test]
    fn xfce_takes_workspace_zero() {
        let text = "/backdrop/screen0/monitoreDP-1/workspace1/last-image    /a.png\n\
            /backdrop/screen0/monitoreDP-1/workspace0/image-style  3\n\
            /backdrop/screen0/monitoreDP-1/workspace0/last-image    /usr/share/backgrounds/xfce/blue.svg";
        assert_eq!(xfce_found(text), pic("/usr/share/backgrounds/xfce/blue.svg", Fit::Stretch));
    }

    #[test]
    fn gnome_slideshows_give_their_first_picture() {
        assert_eq!(slideshow_first("<background><static><duration>10</duration><file>/b/one.jxl</file></static></background>").as_deref(), Some("/b/one.jxl"));
        let sized = "<static><file><size width=\"1024\" height=\"768\">/b/s.png</size><size width=\"3840\" height=\"2160\">/b/l.png</size></file></static>";
        assert_eq!(slideshow_first(sized).as_deref(), Some("/b/l.png"));
    }

    #[test]
    fn wlroots_tools_and_configs() {
        assert_eq!(swww_found("eDP-1: 1920x1080, scale: 1, currently displaying: image: /w/a.webp"), pic("/w/a.webp", Fit::Fill));
        assert_eq!(hyprpaper_found("DP-1 = /w/b.png"), pic("/w/b.png", Fit::Fill));
        assert_eq!(hyprpaper_found("preload = /w/c.png\n# wallpaper = ,/no.png\nwallpaper = DP-1,/w/c.png"), pic("/w/c.png", Fit::Fill));
        assert_eq!(hyprpaper_found("wallpaper {\n    monitor = DP-1\n    path = /w/d.png\n}"), pic("/w/d.png", Fit::Fill));
        let args: Vec<String> = ["swaybg", "-o", "*", "-i", "/w/e.jpg", "-m", "fit"].iter().map(|s| s.to_string()).collect();
        assert_eq!(swaybg_found(&args), pic("/w/e.jpg", Fit::Fit));
        assert_eq!(sway_found("set $mod Mod4\noutput * bg \"/w/my f.png\" stretch #000000"), pic("/w/my f.png", Fit::Stretch));
        assert_eq!(sway_found("output * bg #203040 solid_color").and_then(|f| f.colour), Some(Color::rgb(0x203040)));
    }

    #[test]
    fn x11_setters() {
        assert_eq!(feh_found("#!/bin/sh\nfeh --no-fehbg --bg-scale '/w/f g.jpg' "), pic("/w/f g.jpg", Fit::Stretch));
        assert_eq!(nitrogen_found("[xin_-1]\nfile=/w/n.png\nmode=4\nbgcolor=#000000"), Some(Found { path: Some("/w/n.png".into()), fit: Fit::Fit, colour: Some(Color::rgb(0)) }));
        assert_eq!(lxqt_found("[Desktop]\nBgColor=#2e3436\nWallpaper=/w/q.png\nWallpaperMode=zoom"), Some(Found { path: Some("/w/q.png".into()), fit: Fit::Fill, colour: Some(Color::rgb(0x2e3436)) }));
        assert_eq!(lxde_found("[*]\nwallpaper_mode=color\nwallpaper=/w/x.png\ndesktop_bg=#101010").map(|f| (f.path, f.colour)), Some((None, Some(Color::rgb(0x101010)))));
        assert_eq!(cosmic_found("(\n    output: \"all\",\n    source: Path(\"/usr/share/backgrounds/pop/a.jpg\"),\n    scaling_mode: Zoom,\n)"), pic("/usr/share/backgrounds/pop/a.jpg", Fit::Fill));
    }

    #[test]
    fn portal_values_read() {
        assert_eq!(numbers("(<uint32 1>,)"), vec![1.0]);
        assert_eq!(numbers("(<<uint32 2>>,)"), vec![2.0]);
        assert_eq!(numbers("v u 1"), vec![1.0]);
        assert_eq!(portal_colour("(<(0.2, 0.4, 1.0)>,)"), Some(Color::rgb(0x3366ff)));
        assert_eq!(portal_colour("v (ddd) 0.2 0.4 1"), Some(Color::rgb(0x3366ff)));
        assert_eq!(portal_colour("(<(-1.0, -1.0, -1.0)>,)"), None, "none set");
    }

    #[test]
    fn colours_and_uris() {
        assert_eq!(hex("'#023c88'"), Some(Color::rgb(0x023c88)));
        assert_eq!(hex("#02023c3c8888"), Some(Color::rgb(0x023c88)));
        assert_eq!(from_uri("file:///home/u/My%20Pictures/a.png"), PathBuf::from("/home/u/My Pictures/a.png"));
        assert_eq!(shell_words("feh --bg-fill '/a b' \"c d\" e"), ["feh", "--bg-fill", "/a b", "c d", "e"]);
    }
}
