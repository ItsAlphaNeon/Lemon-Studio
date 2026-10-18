// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! What the Etched skin takes from the system (spec: ui/etched-design.md §2): dark or light, the accent colour and the
//! desktop picture. They are read on a background thread at start and whenever the main window comes to the front,
//! then picked up by the app (`App::poll_system`).
//!
//! The backdrop is the desktop picture as it is laid out on the monitor, shrunk, blurred and tinted (`frosted`); the
//! canvas enlarges it under the frame, placed where the monitor is relative to the window, so the window seems to let
//! the desktop through.

use os_theme::etched::Frost;
use os_theme::Color;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A picture: `w` x `h` `0x00RRGGBB` pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u32>,
}

/// How the desktop picture is laid on the monitor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    /// Covers the monitor, cropped.
    Fill,
    /// Whole, on the desktop colour.
    Fit,
    /// Stretched to the monitor.
    Stretch,
}

/// The desktop as the system shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Desktop {
    /// The picture (shrunk to at most `MAX_SIDE`), or none: a plain colour or a picture that could not be read.
    pub picture: Option<Picture>,
    pub fit: Fit,
    /// The desktop colour (around a fitted picture, or alone).
    pub colour: Option<Color>,
}

/// What was read from the system.
#[derive(Clone, Debug, PartialEq)]
pub struct System {
    pub dark: bool,
    pub accent: Option<Color>,
    pub desktop: Desktop,
}

/// Larger pictures are shrunk to this many px on their long side as they are read: the backdrop needs much less.
const MAX_SIDE: usize = 640;

/// The latest reading, not picked up yet.
static RESULT: Mutex<Option<System>> = Mutex::new(None);
/// A reading is under way.
static BUSY: Mutex<bool> = Mutex::new(false);

/// Reads the system on a background thread; `wake` runs when it is done (the reading is then in `take`). `picture`: the
/// desktop picture's file when it was found on the main thread (macOS, where only it may ask).
pub fn read_async(wake: fn(), picture: Option<PathBuf>) {
    {
        let Ok(mut busy) = BUSY.lock() else { return };
        if *busy {
            return;
        }
        *busy = true;
    }
    let spawned = std::thread::Builder::new().name("system look".into()).spawn(move || {
        let s = read(picture);
        if let Ok(mut r) = RESULT.lock() {
            *r = Some(s);
        }
        if let Ok(mut b) = BUSY.lock() {
            *b = false;
        }
        wake();
    });
    if spawned.is_err() {
        if let Ok(mut b) = BUSY.lock() {
            *b = false;
        }
    }
}

/// The reading `read_async` made, once.
pub fn take() -> Option<System> {
    RESULT.lock().ok()?.take()
}

/// Reads the system now (`picture` as for `read_async`).
pub fn read(picture: Option<PathBuf>) -> System {
    let dark = sys::dark().unwrap_or(true);
    System { dark, accent: sys::accent(), desktop: sys::desktop(dark, picture) }
}

/// What the app keeps: the last reading and the backdrop made from it.
#[derive(Default)]
pub struct State {
    pub system: Option<System>,
    /// When the system was last asked (reads on focus are spaced out).
    pub asked: Option<Instant>,
    /// The frosted backdrop, by what it was made for: (monitor px w, h, dark).
    made: Option<((u32, u32, bool), Rc<Vec<u32>>, usize, usize)>,
}

impl State {
    /// Whether to ask the system again (at start, or coming to the front a few seconds after the last time).
    pub fn due(&mut self, now: Instant) -> bool {
        let due = self.asked.is_none_or(|t| now.duration_since(t) > Duration::from_secs(2));
        if due {
            self.asked = Some(now);
        }
        due
    }

    /// Takes a new reading; true when it differs from the last.
    pub fn set(&mut self, s: System) -> bool {
        if self.system.as_ref() == Some(&s) {
            return false;
        }
        self.system = Some(s);
        self.made = None;
        true
    }

    /// The frosted backdrop for a monitor of `mw` x `mh` px: (pixels, w, h).
    pub fn backdrop(&mut self, mw: u32, mh: u32, dark: bool) -> (Rc<Vec<u32>>, usize, usize) {
        let key = (mw.max(1), mh.max(1), dark);
        if let Some((k, px, w, h)) = &self.made {
            if *k == key {
                return (px.clone(), *w, *h);
            }
        }
        let accent = self.system.as_ref().and_then(|s| s.accent).unwrap_or(os_theme::etched::DEFAULT_ACCENT);
        let desk = self.system.as_ref().map(|s| &s.desktop);
        let p = frosted(desk, accent, key.0, key.1, dark);
        let px = Rc::new(p.px);
        self.made = Some((key, px.clone(), p.w, p.h));
        (px, p.w, p.h)
    }
}

/// The backdrop for a `mw` x `mh` monitor: the desktop laid out on it, at 1/8 size, blurred heavily and tinted.
pub fn frosted(desk: Option<&Desktop>, accent: Color, mw: u32, mh: u32, dark: bool) -> Picture {
    let (w, h) = (((mw as usize) / 8).clamp(8, 480), ((mh as usize) / 8).clamp(8, 480));
    let mut p = match desk {
        Some(d) => laid_out(d, w, h).unwrap_or_else(|| generated(accent, w, h, dark)),
        None => generated(accent, w, h, dark),
    };
    // Heavy: about 1/12 of the monitor's height.
    let r = (h / 24).max(2);
    os_ui::blur(&mut p.px, w, h, r, 3);
    let f: Frost = os_theme::etched::backdrop_frost(dark);
    for v in &mut p.px {
        *v = os_ui::frost_pixel(*v, &f);
    }
    p
}

/// The desktop as it covers a `w` x `h` monitor (none when it has neither a picture nor a colour).
fn laid_out(d: &Desktop, w: usize, h: usize) -> Option<Picture> {
    let bg = d.colour.map_or(0, |c| u32::from(c.r) << 16 | u32::from(c.g) << 8 | u32::from(c.b));
    let Some(pic) = &d.picture else {
        return d.colour.map(|_| Picture { w, h, px: vec![bg; w * h] });
    };
    if pic.w == 0 || pic.h == 0 {
        return None;
    }
    // The part of the picture shown and where it goes, as (x, y, w, h) fractions.
    let (pa, ma) = (pic.w as f32 / pic.h as f32, w as f32 / h as f32);
    let (src, dst) = match d.fit {
        Fit::Stretch => ((0.0, 0.0, 1.0, 1.0), (0.0, 0.0, 1.0, 1.0)),
        Fit::Fill if pa > ma => {
            let f = ma / pa;
            (((1.0 - f) / 2.0, 0.0, f, 1.0), (0.0, 0.0, 1.0, 1.0))
        }
        Fit::Fill => {
            let f = pa / ma;
            ((0.0, (1.0 - f) / 2.0, 1.0, f), (0.0, 0.0, 1.0, 1.0))
        }
        Fit::Fit if pa > ma => {
            let f = ma / pa;
            ((0.0, 0.0, 1.0, 1.0), (0.0, (1.0 - f) / 2.0, 1.0, f))
        }
        Fit::Fit => {
            let f = pa / ma;
            ((0.0, 0.0, 1.0, 1.0), ((1.0 - f) / 2.0, 0.0, f, 1.0))
        }
    };
    // Crop, then shrink into the destination.
    let (cx, cy) = ((src.0 * pic.w as f32) as usize, (src.1 * pic.h as f32) as usize);
    let (cw, ch) = (((src.2 * pic.w as f32) as usize).max(1).min(pic.w - cx), ((src.3 * pic.h as f32) as usize).max(1).min(pic.h - cy));
    let mut crop = Vec::with_capacity(cw * ch);
    for y in cy..cy + ch {
        crop.extend_from_slice(&pic.px[y * pic.w + cx..y * pic.w + cx + cw]);
    }
    let (dx, dy) = ((dst.0 * w as f32).round() as usize, (dst.1 * h as f32).round() as usize);
    let (dw, dh) = (((dst.2 * w as f32).round() as usize).clamp(1, w - dx), ((dst.3 * h as f32).round() as usize).clamp(1, h - dy));
    let small = os_ui::shrink(&crop, cw, ch, dw, dh);
    let mut px = vec![bg; w * h];
    for y in 0..dh {
        px[(dy + y) * w + dx..(dy + y) * w + dx + dw].copy_from_slice(&small[y * dw..(y + 1) * dw]);
    }
    Some(Picture { w, h, px })
}

/// A desktop when there is no picture to read: two soft glows of the accent on the base colour.
fn generated(accent: Color, w: usize, h: usize, dark: bool) -> Picture {
    let base = if dark { Color::rgb(0x101418) } else { Color::rgb(0xdfe6ee) };
    let (ah, as_, _) = accent.hsl();
    let glow1 = Color::from_hsl_pub(ah, as_.min(0.9), if dark { 0.35 } else { 0.7 }, 255);
    let glow2 = Color::from_hsl_pub((ah + 0.08) % 1.0, (as_ * 0.8).min(0.8), if dark { 0.22 } else { 0.82 }, 255);
    let mut px = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
            let g = |cx: f32, cy: f32, r: f32| (1.0 - (((u - cx) * 1.6).powi(2) + (v - cy).powi(2)).sqrt() / r).clamp(0.0, 1.0).powi(2);
            let (a, b) = (g(0.72, 0.62, 0.75), g(0.2, 0.15, 0.6));
            let mix = |base: u8, c1: u8, c2: u8| {
                let v = f32::from(base) + (f32::from(c1) - f32::from(base)) * a;
                (v + (f32::from(c2) - v) * b * 0.8).round().clamp(0.0, 255.0) as u32
            };
            px.push(mix(base.r, glow1.r, glow2.r) << 16 | mix(base.g, glow1.g, glow2.g) << 8 | mix(base.b, glow1.b, glow2.b));
        }
    }
    Picture { w, h, px }
}

/// Reads a picture file (PNG or JPEG), shrunk to at most `MAX_SIDE` px on its long side.
pub fn load(path: &Path) -> Option<Picture> {
    let data = std::fs::read(path).ok()?;
    let p = decode(&data)?;
    let k = (p.w.max(p.h) as f32 / MAX_SIDE as f32).max(1.0);
    if k <= 1.0 {
        return Some(p);
    }
    let (w, h) = (((p.w as f32 / k).round() as usize).max(1), ((p.h as f32 / k).round() as usize).max(1));
    Some(Picture { w, h, px: os_ui::shrink(&p.px, p.w, p.h, w, h) })
}

fn decode(data: &[u8]) -> Option<Picture> {
    if data.starts_with(b"\x89PNG") {
        let pm = tiny_skia::Pixmap::decode_png(data).ok()?;
        let px = pm
            .pixels()
            .iter()
            .map(|p| {
                let c = p.demultiply();
                u32::from(c.red()) << 16 | u32::from(c.green()) << 8 | u32::from(c.blue())
            })
            .collect();
        return Some(Picture { w: pm.width() as usize, h: pm.height() as usize, px });
    }
    if data.starts_with(&[0xff, 0xd8]) {
        use zune_jpeg::zune_core::colorspace::ColorSpace;
        use zune_jpeg::zune_core::options::DecoderOptions;
        let opts = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
        let mut d = zune_jpeg::JpegDecoder::new_with_options(std::io::Cursor::new(data), opts);
        let rgb = d.decode().ok()?;
        let (w, h) = d.dimensions()?;
        if rgb.len() < w * h * 3 {
            return None;
        }
        let px = rgb.chunks_exact(3).map(|c| u32::from(c[0]) << 16 | u32::from(c[1]) << 8 | u32::from(c[2])).collect();
        return Some(Picture { w, h, px });
    }
    None
}

/// A "#rrggbb"-free colour from "r g b" or "r,g,b" (0..255 each).
fn triple(s: &str) -> Option<Color> {
    let v: Vec<u8> = s.split(|c: char| c == ',' || c.is_whitespace()).filter(|t| !t.is_empty()).map(|t| t.trim().parse().ok()).collect::<Option<_>>()?;
    (v.len() >= 3).then(|| Color::rgb(u32::from(v[0]) << 16 | u32::from(v[1]) << 8 | u32::from(v[2])))
}

#[cfg(windows)]
mod sys {
    use super::{load, triple, Desktop, Fit};
    use os_theme::Color;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RRF_RT_REG_SZ};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETDESKWALLPAPER};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn dword(key: &str, name: &str) -> Option<u32> {
        let (k, n) = (wide(key), wide(name));
        let mut v = 0u32;
        let mut bytes = 4u32;
        // SAFETY: NUL-terminated strings; `v` holds the 4 bytes asked for.
        let r = unsafe { RegGetValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr(), RRF_RT_REG_DWORD, null_mut(), (&raw mut v).cast(), &mut bytes) };
        (r == ERROR_SUCCESS).then_some(v)
    }

    fn string(key: &str, name: &str) -> Option<String> {
        let (k, n) = (wide(key), wide(name));
        let mut buf = vec![0u16; 1024];
        let mut bytes = (buf.len() * 2) as u32;
        // SAFETY: `buf` holds `bytes` bytes.
        let r = unsafe { RegGetValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr(), RRF_RT_REG_SZ, null_mut(), buf.as_mut_ptr().cast(), &mut bytes) };
        if r != ERROR_SUCCESS {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    }

    pub fn dark() -> Option<bool> {
        dword(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "AppsUseLightTheme").map(|v| v == 0)
    }

    pub fn accent() -> Option<Color> {
        // 0xAABBGGRR.
        dword(r"Software\Microsoft\Windows\DWM", "AccentColor").map(|v| Color::rgb((v & 0xff) << 16 | (v & 0xff00) | (v >> 16) & 0xff))
    }

    pub fn desktop(_dark: bool, _picture: Option<std::path::PathBuf>) -> Desktop {
        let mut buf = vec![0u16; 1024];
        // SAFETY: `buf` holds 1024 wide chars, the size passed.
        let ok = unsafe { SystemParametersInfoW(SPI_GETDESKWALLPAPER, buf.len() as u32, buf.as_mut_ptr().cast(), 0) } != 0;
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let path = if ok { String::from_utf16_lossy(&buf[..end]) } else { String::new() };
        let mut picture = (!path.is_empty()).then(|| load(path.as_ref())).flatten();
        // A picture the system copied (and converted) for the desktop, where the original cannot be read.
        if picture.is_none() && !path.is_empty() {
            if let Some(app) = std::env::var_os("APPDATA") {
                picture = load(&std::path::Path::new(&app).join(r"Microsoft\Windows\Themes\TranscodedWallpaper"));
            }
        }
        let style = string(r"Control Panel\Desktop", "WallpaperStyle").and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(10);
        let fit = match style {
            2 => Fit::Stretch,
            6 => Fit::Fit,
            _ => Fit::Fill,
        };
        let colour = string(r"Control Panel\Colors", "Background").and_then(|s| triple(&s));
        Desktop { picture, fit, colour }
    }
}

#[cfg(target_os = "macos")]
mod sys {
    use super::{load, Desktop, Fit};
    use os_theme::Color;
    use std::process::Command;

    fn default(key: &str) -> Option<String> {
        let out = Command::new("defaults").args(["read", "-g", key]).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    pub fn dark() -> Option<bool> {
        // The key is there only in dark mode.
        Some(default("AppleInterfaceStyle").is_some_and(|s| s.eq_ignore_ascii_case("dark")))
    }

    pub fn accent() -> Option<Color> {
        let c = match default("AppleAccentColor").and_then(|s| s.parse::<i32>().ok()) {
            Some(-1) => 0x8c8c8c,
            Some(0) => 0xff5257,
            Some(1) => 0xf7821b,
            Some(2) => 0xffc600,
            Some(3) => 0x62ba46,
            Some(5) => 0xa550a7,
            Some(6) => 0xf74f9e,
            _ => 0x007aff,
        };
        Some(Color::rgb(c))
    }

    pub fn desktop(_dark: bool, picture: Option<std::path::PathBuf>) -> Desktop {
        let picture = picture.and_then(|p| {
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            if matches!(ext.as_str(), "png" | "jpg" | "jpeg") {
                if let Some(pic) = load(&p) {
                    return Some(pic);
                }
            }
            // HEIC and the rest: the system converts it.
            let out = std::env::temp_dir().join(format!("lemonstudio-desktop-{}.png", std::process::id()));
            let ok = Command::new("sips")
                .args(["-s", "format", "png", "-Z", "640"])
                .arg(&p)
                .arg("--out")
                .arg(&out)
                .output()
                .is_ok_and(|o| o.status.success());
            let pic = ok.then(|| load(&out)).flatten();
            let _ = std::fs::remove_file(&out);
            pic
        });
        Desktop { picture, fit: Fit::Fill, colour: None }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod sys {
    use super::{load, triple, Desktop, Fit};
    use os_theme::Color;
    use std::path::PathBuf;
    use std::process::Command;

    fn gsettings(schema: &str, key: &str) -> Option<String> {
        let out = Command::new("gsettings").args(["get", schema, key]).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().trim_matches('\'').to_string())
    }

    fn config(name: &str) -> Option<String> {
        let dir = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        std::fs::read_to_string(dir.join(name)).ok()
    }

    /// `key=` in `[section]` of an ini-like file.
    fn ini(text: &str, section: &str, key: &str) -> Option<String> {
        let mut inside = false;
        for line in text.lines() {
            let l = line.trim();
            if l.starts_with('[') {
                inside = l == format!("[{section}]");
            } else if inside {
                if let Some(v) = l.strip_prefix(key).and_then(|r| r.strip_prefix('=')) {
                    return Some(v.trim().to_string());
                }
            }
        }
        None
    }

    fn kde() -> bool {
        std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.to_ascii_uppercase().contains("KDE"))
    }

    pub fn dark() -> Option<bool> {
        if kde() {
            let g = config("kdeglobals")?;
            if let Some(bg) = ini(&g, "Colors:Window", "BackgroundNormal").and_then(|s| triple(&s)) {
                return Some(bg.hsl().2 < 0.5);
            }
            return ini(&g, "General", "ColorScheme").map(|s| s.to_ascii_lowercase().contains("dark"));
        }
        if let Some(s) = gsettings("org.gnome.desktop.interface", "color-scheme") {
            return Some(s == "prefer-dark");
        }
        gsettings("org.gnome.desktop.interface", "gtk-theme").map(|t| t.to_ascii_lowercase().contains("dark"))
    }

    pub fn accent() -> Option<Color> {
        if kde() {
            if let Some(c) = config("kdeglobals").and_then(|g| ini(&g, "General", "AccentColor")).and_then(|s| triple(&s)) {
                return Some(c);
            }
        }
        let c = match gsettings("org.gnome.desktop.interface", "accent-color")?.as_str() {
            "teal" => 0x2190a4,
            "green" => 0x3a944a,
            "yellow" => 0xc88800,
            "orange" => 0xed5b00,
            "red" => 0xe62d42,
            "pink" => 0xd56199,
            "purple" => 0x9141ac,
            "slate" => 0x6f8396,
            _ => 0x3584e4,
        };
        Some(Color::rgb(c))
    }

    fn from_uri(s: &str) -> PathBuf {
        let s = s.strip_prefix("file://").unwrap_or(s);
        // %20 and the like.
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

    pub fn desktop(dark: bool, _picture: Option<PathBuf>) -> Desktop {
        if kde() {
            let picture = config("plasma-org.kde.plasma.desktop-appletsrc").and_then(|t| {
                t.lines().find_map(|l| l.trim().strip_prefix("Image=").map(|v| from_uri(v.trim())))
            });
            // A wallpaper package (a folder): its largest picture.
            let picture = picture.and_then(|p| if p.is_dir() { largest(&p.join("contents/images")) } else { Some(p) });
            return Desktop { picture: picture.and_then(|p| load(&p)), fit: Fit::Fill, colour: None };
        }
        let key = if dark { "picture-uri-dark" } else { "picture-uri" };
        let uri = gsettings("org.gnome.desktop.background", key).filter(|s| !s.is_empty()).or_else(|| gsettings("org.gnome.desktop.background", "picture-uri"));
        let picture = uri.map(|u| from_uri(&u)).and_then(|p| load(&p));
        let fit = match gsettings("org.gnome.desktop.background", "picture-options").as_deref() {
            Some("stretched") => Fit::Stretch,
            Some("scaled" | "centered") => Fit::Fit,
            _ => Fit::Fill,
        };
        let colour = gsettings("org.gnome.desktop.background", "primary-color").and_then(|s| {
            let h = s.trim_start_matches('#');
            (h.len() == 6).then(|| u32::from_str_radix(h, 16).ok()).flatten().map(Color::rgb)
        });
        Desktop { picture, fit, colour }
    }

    fn largest(dir: &std::path::Path) -> Option<PathBuf> {
        let px = |p: &PathBuf| {
            let n = p.file_stem()?.to_str()?;
            let (w, h) = n.split_once('x')?;
            Some(w.parse::<u64>().ok()? * h.parse::<u64>().ok()?)
        };
        std::fs::read_dir(dir).ok()?.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|e| e == "png" || e == "jpg" || e == "jpeg")).max_by_key(|p| px(p).unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_read_from_triples() {
        assert_eq!(triple("10 20 30"), Some(Color::rgb(0x0a141e)));
        assert_eq!(triple("61,174,233"), Some(Color::rgb(0x3daee9)));
        assert_eq!(triple("1 2"), None);
    }

    #[test]
    fn a_wide_picture_fills_by_cropping_and_fits_with_bars() {
        // Left half black, right half white, twice as wide as the monitor's shape.
        let pic = Picture { w: 32, h: 8, px: (0..256).map(|i| if i % 32 < 16 { 0 } else { 0xffffff }).collect() };
        let d = Desktop { picture: Some(pic.clone()), fit: Fit::Fill, colour: Some(Color::rgb(0xff0000)) };
        let p = laid_out(&d, 16, 16).expect("laid out");
        assert_eq!(p.px[8 * 16], 0, "left edge from the black half");
        assert_eq!(p.px[8 * 16 + 15], 0xffffff);
        let d = Desktop { fit: Fit::Fit, ..d };
        let p = laid_out(&d, 16, 16).expect("laid out");
        assert_eq!(p.px[0], 0xff0000, "bar above in the desktop colour");
        assert_eq!(p.px[8 * 16], 0);
    }

    #[test]
    fn the_backdrop_is_dark_on_dark_and_light_on_light() {
        let luma = |p: &Picture| p.px.iter().map(|v| (v >> 8) & 0xff).sum::<u32>() / p.px.len() as u32;
        let accent = Color::rgb(0x0078d4);
        assert!(luma(&frosted(None, accent, 1920, 1080, true)) < 0x40);
        assert!(luma(&frosted(None, accent, 1920, 1080, false)) > 0xc0);
    }

    #[test]
    fn pictures_decode() {
        let mut pm = tiny_skia::Pixmap::new(4, 2).expect("pixmap");
        pm.fill(tiny_skia::Color::from_rgba8(0x12, 0x34, 0x56, 255));
        let p = decode(&pm.encode_png().expect("png")).expect("decoded");
        assert_eq!((p.w, p.h, p.px[0]), (4, 2, 0x123456));
        assert!(decode(b"not a picture").is_none());
    }
}

#[cfg(test)]
mod system_probe {
    /// Reads this computer's look (run by hand: `cargo test -p os-app system_look -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn system_look() {
        let s = super::read(None);
        let pic = s.desktop.picture.as_ref().map(|p| (p.w, p.h));
        println!("dark {} accent {:?} picture {:?} fit {:?} colour {:?}", s.dark, s.accent, pic, s.desktop.fit, s.desktop.colour);
    }
}
