// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The workspace picture of a `.flstheme` (`BackPicFilename` while `BackMode` shows a picture, `Palette::wallpaper`):
//! read from disk when a theme names one and drawn over the workspace colour, behind the windows.
// spec: ui/flstheme.md §1, §3
//
// Lemon Studio choices (the spec gives the keys, not the drawing): the picture covers the workspace right of the
// browser, scaled to fill it with its proportions kept and centred, the overflow cut off; the logo watermark and the
// version line are left out over it. PNG and JPEG files are read; another format, or a file that is missing or does not
// decode, leaves the plain colour. A relative name is the theme file's folder's: a theme taken in from a file (Load
// theme..., a drop, the user's `themes` folder) has its names made absolute against that file's folder, since the
// applied theme is kept as `current.flstheme` in the user folder; a name still relative there is that folder's.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use os_ui::Rect;

use crate::app::Ui;

/// The theme keys that name files (spec §1).
const FILE_KEYS: [&str; 2] = ["BackPicFilename", "BackHTMLFileName"];

/// Pictures larger than this on either side are not read.
const MAX_SIDE: usize = 8192;

/// A decoded picture: width, height and `0x00RRGGBB` pixels.
type Image = (usize, usize, Vec<u32>);

/// A theme's file name `name` as a path: Windows separators read as separators everywhere, a relative name joined to
/// `dir` (the theme file's folder).
pub fn resolve(name: &str, dir: Option<&Path>) -> PathBuf {
    let name = name.trim();
    let p = if cfg!(windows) { PathBuf::from(name) } else { PathBuf::from(name.replace('\\', "/")) };
    match dir {
        Some(d) if p.is_relative() => d.join(p),
        _ => p,
    }
}

/// `text` (a `.flstheme` read from a file in `dir`) with its relative file names made absolute against `dir`; the rest of
/// the text as it was.
pub fn absolute_names(text: &str, dir: &Path) -> String {
    let mut out = text.to_string();
    for key in FILE_KEYS {
        let Some(name) = os_theme::fl::get_key(text, key).filter(|n| !n.is_empty()) else { continue };
        if resolve(name, None).is_relative() {
            out = os_theme::fl::set_key(&out, key, &resolve(name, Some(dir)).to_string_lossy());
        }
    }
    out
}

/// A PNG or JPEG file's picture; `None` for another format, a file that is missing or does not decode, or one larger
/// than `MAX_SIDE`.
pub fn read(path: &Path) -> Option<Image> {
    decode(&std::fs::read(path).ok()?)
}

/// `read` on the file's bytes.
pub fn decode(bytes: &[u8]) -> Option<Image> {
    let fits = |w: usize, h: usize| w > 0 && h > 0 && w <= MAX_SIDE && h <= MAX_SIDE;
    if bytes.len() > 24 && bytes.starts_with(b"\x89PNG") {
        // The IHDR chunk's width and height come first, big-endian.
        let side = |i: usize| u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]) as usize;
        if !fits(side(16), side(20)) {
            return None;
        }
        return os_ui::Canvas::decode_png(bytes);
    }
    if !bytes.starts_with(&[0xff, 0xd8]) {
        return None;
    }
    let mut d = jpeg_decoder::Decoder::new(bytes);
    d.read_info().ok()?;
    let info = d.info()?;
    let (w, h) = (usize::from(info.width), usize::from(info.height));
    if !fits(w, h) {
        return None;
    }
    let data = d.decode().ok()?;
    let rgb = |r: u8, g: u8, b: u8| u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b);
    use jpeg_decoder::PixelFormat;
    let px: Vec<u32> = match info.pixel_format {
        PixelFormat::RGB24 => data.as_chunks::<3>().0.iter().map(|p| rgb(p[0], p[1], p[2])).collect(),
        PixelFormat::L8 => data.iter().map(|&l| rgb(l, l, l)).collect(),
        // Big-endian: the high byte first.
        PixelFormat::L16 => data.as_chunks::<2>().0.iter().map(|p| rgb(p[0], p[0], p[0])).collect(),
        PixelFormat::CMYK32 => data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| {
                let ink = |c: u8| ((255 - u32::from(c)) * (255 - u32::from(p[3])) / 255) as u8;
                rgb(ink(p[0]), ink(p[1]), ink(p[2]))
            })
            .collect(),
    };
    (px.len() == w * h).then_some((w, h, px))
}

/// `px` (`w` x `h`) cut to the proportions of `dw` x `dh` around its centre, then resampled to that size.
pub fn cover(w: usize, h: usize, px: &[u32], dw: usize, dh: usize) -> Vec<u32> {
    let (cw, ch) = if w * dh > h * dw { ((h * dw / dh).clamp(1, w), h) } else { (w, (w * dh / dw).clamp(1, h)) };
    let (x0, y0) = ((w - cw) / 2, (h - ch) / 2);
    let cut: Vec<u32> = (y0..y0 + ch).flat_map(|y| px[y * w + x0..y * w + x0 + cw].iter().copied()).collect();
    os_ui::resample(cw, ch, &cut, dw, dh)
}

/// The picture last asked for: the theme's name for it, the picture (`None`: not readable) and its copy scaled to the
/// size it was last drawn at.
struct Held {
    name: String,
    image: Option<Image>,
    scaled: Option<Image>,
}

thread_local! {
    static HELD: RefCell<Option<Held>> = const { RefCell::new(None) };
}

/// Drops the picture read last, so the next paint reads its file again (a theme file taken in anew).
pub fn forget() {
    HELD.with(|h| *h.borrow_mut() = None);
}

/// Draws the applied theme's picture over `work` (the workspace right of the browser), covering it. False when the
/// theme has none or it cannot be read: the plain colour shows.
pub fn paint(ui: &mut Ui, work: Rect) -> bool {
    let Some(name) = ui.t.wallpaper.as_deref() else { return false };
    let k = ui.c.scale;
    let (dw, dh) = ((work.w * k).round().max(1.0) as usize, (work.h * k).round().max(1.0) as usize);
    HELD.with(|held| {
        let mut held = held.borrow_mut();
        if held.as_ref().is_none_or(|h| h.name != name) {
            // Read once per name (a name still relative is the user folder's, where `current.flstheme` is).
            let image = read(&resolve(name, crate::app::theme_dir().as_deref()));
            *held = Some(Held { name: name.to_string(), image, scaled: None });
        }
        let h = held.as_mut().expect("just set");
        let Some((w, ht, px)) = &h.image else { return false };
        if h.scaled.as_ref().is_none_or(|s| (s.0, s.1) != (dw, dh)) {
            h.scaled = Some((dw, dh, cover(*w, *ht, px, dw, dh)));
        }
        let (sw, sh, spx) = h.scaled.as_ref().expect("just set");
        ui.c.image(work, *sw, *sh, spx);
        true
    })
}

/// VIEW ▸ Background ▸ (spec: ui/menu-submenus.md §3): Set image wallpaper... picks a PNG or JPEG for the applied
/// theme's workspace picture. The HTML document and the background colour stay greyed (Lemon Studio draws no HTML;
/// the colour has no picker outside Theme ▸ Adjustments yet).
pub fn items(_: &crate::app::State) -> Vec<crate::playlist::PItem> {
    use crate::playlist::{Cmd, PItem};
    vec![
        PItem::new("Set image wallpaper...", Cmd::Wallpaper),
        PItem::new("Set dynamic wallpaper / HTML document...", Cmd::Nothing).off(),
        PItem::new("Set background color...", Cmd::Nothing).off(),
    ]
}

impl crate::app::App {
    /// Set image wallpaper...: a file dialog for the picture.
    pub(crate) fn wallpaper_dialog(&mut self) {
        let dlg = crate::filedlg::FileDlg::new(crate::filedlg::Mode::Open, "Set image wallpaper", "wallpaper", crate::dialog::Then::Wallpaper)
            .filter("Picture", &["png", "jpg", "jpeg"])
            .start(None);
        self.file_dialog(dlg);
    }

    /// The applied theme shows picture `path` over the workspace (`BackMode` 2, `BackPicFilename` its absolute name),
    /// live and kept for the next start, as Theme ▸ Adjustments keep their changes.
    pub(crate) fn set_wallpaper(&mut self, path: &Path) {
        let name = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let text = os_theme::fl::set_key(&self.theme_text(), "BackMode", "2");
        let text = os_theme::fl::set_key(&text, "BackPicFilename", &name.to_string_lossy());
        self.theme_edit(text);
        self.theme_store();
        forget();
        self.state.message = Some(format!("Wallpaper: {}", name.display()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Set image wallpaper... puts the picture's absolute name in the applied theme with a picture BackMode, so the
    /// palette shows it.
    #[test]
    fn set_image_wallpaper() {
        let mut app = crate::app::App::new(None);
        let items = items(&app.state);
        assert_eq!(items.iter().map(|i| (i.label.to_string(), i.enabled)).collect::<Vec<_>>(), [("Set image wallpaper...".to_string(), true), ("Set dynamic wallpaper / HTML document...".to_string(), false), ("Set background color...".to_string(), false)]);
        let pic = std::env::temp_dir().join("lemon-wall.png");
        app.set_wallpaper(&pic);
        let text = app.state.fl_text.clone().unwrap();
        assert_eq!(os_theme::fl::get_key(&text, "BackMode").as_deref(), Some("2"));
        assert_eq!(app.theme.colors.wallpaper.as_deref().map(PathBuf::from), Some(pic));
    }

    /// A relative name is the theme file's folder's, with `\` read as a separator; absolute names and the rest of the
    /// text stay as they were.
    #[test]
    fn relative_names_are_made_absolute() {
        let dir = std::env::temp_dir().join("lemon-themes");
        let text = "\u{feff}Hue=4\r\nBackPicFilename=..\\Pictures\\wall.jpg\r\nBackHTMLFileName=\r\nBackMode=2\r\n";
        let out = absolute_names(text, &dir);
        let want = dir.join("..").join("Pictures").join("wall.jpg");
        assert_eq!(os_theme::fl::get_key(&out, "BackPicFilename").map(PathBuf::from), Some(want.clone()));
        assert_eq!(os_theme::fl::remove_key(&out, "BackPicFilename"), os_theme::fl::remove_key(text, "BackPicFilename"));
        assert_eq!(resolve("..\\Pictures\\wall.jpg", Some(&dir)), want);
        let abs = std::env::temp_dir().join("wall.png");
        let text = format!("BackPicFilename={}\n", abs.display());
        assert_eq!(absolute_names(&text, &dir), text, "an absolute name is kept");
        assert_eq!(resolve(&abs.to_string_lossy(), Some(&dir)), abs);
    }

    /// PNG and JPEG pictures decode (a JPEG's colours within its compression); other bytes do not.
    #[test]
    fn png_and_jpeg_pictures_decode() {
        let mut pm = tiny_skia::Pixmap::new(3, 2).unwrap();
        pm.fill(tiny_skia::Color::from_rgba8(0x10, 0x80, 0xf0, 0xff));
        let (w, h, px) = decode(&pm.encode_png().unwrap()).expect("png");
        assert_eq!((w, h), (3, 2));
        assert!(px.iter().all(|&p| p == 0x1080f0));
        let (w, h, px) = decode(JPEG_RED_8X8).expect("jpeg");
        assert_eq!((w, h, px.len()), (8, 8, 64));
        let near = |p: u32| ((p >> 16) & 0xff) > 0xe0 && ((p >> 8) & 0xff) < 0x20 && (p & 0xff) < 0x20;
        assert!(px.iter().all(|&p| near(p)), "{:06x}", px[0]);
        assert!(decode(b"GIF89a....").is_none());
        assert!(decode(&JPEG_RED_8X8[..40]).is_none(), "a cut file");
        assert!(read(Path::new("/no/such/picture.png")).is_none());
    }

    /// Cover keeps the picture's proportions: a wide picture loses its sides, a tall one its top and bottom.
    #[test]
    fn cover_cuts_to_the_target_proportions() {
        // 4 x 1: columns 0 1 2 3.
        let px = [0u32, 1, 2, 3];
        assert_eq!(cover(4, 1, &px, 2, 1), vec![1, 2], "the middle two columns");
        let px = [0u32, 1, 2, 3];
        assert_eq!(cover(1, 4, &px, 1, 2), vec![1, 2], "the middle two rows");
        assert_eq!(cover(2, 2, &[5, 5, 5, 5], 4, 4), vec![5; 16]);
    }

    /// An 8 x 8 baseline JPEG, pure red (made for this test).
    const JPEG_RED_8X8: &[u8] = &[
        0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0xff, 0xdb, 0x00, 0x43,
        0x00, 0x03, 0x02, 0x02, 0x03, 0x02, 0x02, 0x03, 0x03, 0x03, 0x03, 0x04, 0x03, 0x03, 0x04, 0x05, 0x08, 0x05, 0x05, 0x04, 0x04, 0x05, 0x0a, 0x07,
        0x07, 0x06, 0x08, 0x0c, 0x0a, 0x0c, 0x0c, 0x0b, 0x0a, 0x0b, 0x0b, 0x0d, 0x0e, 0x12, 0x10, 0x0d, 0x0e, 0x11, 0x0e, 0x0b, 0x0b, 0x10, 0x16, 0x10,
        0x11, 0x13, 0x14, 0x15, 0x15, 0x15, 0x0c, 0x0f, 0x17, 0x18, 0x16, 0x14, 0x18, 0x12, 0x14, 0x15, 0x14, 0xff, 0xdb, 0x00, 0x43, 0x01, 0x03, 0x04,
        0x04, 0x05, 0x04, 0x05, 0x09, 0x05, 0x05, 0x09, 0x14, 0x0d, 0x0b, 0x0d, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14,
        0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14,
        0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0xff, 0xc0, 0x00, 0x11, 0x08, 0x00, 0x08, 0x00, 0x08, 0x03,
        0x01, 0x11, 0x00, 0x02, 0x11, 0x01, 0x03, 0x11, 0x01, 0xff, 0xc4, 0x00, 0x14, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08, 0xff, 0xc4, 0x00, 0x14, 0x10, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xc4, 0x00, 0x15, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x07, 0x09, 0xff, 0xc4, 0x00, 0x14, 0x11, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0xff, 0xda, 0x00, 0x0c, 0x03, 0x01, 0x00, 0x02, 0x11, 0x03, 0x11, 0x00, 0x3f, 0x00, 0x3a, 0x03, 0x15, 0x4d, 0xff, 0xd9,
    ];
}
