// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The last look of each plugin editor drawn in the app's window (Linux, `plugin_window::EmbedFrame`), by plugin id:
//! kept when its window closes, and in the user folder for the next start (`editor-looks/<id>.png`). A window opening
//! shows it, dimmed, until the editor draws its first frame, so it looks open at once (owner, 2026-10-27: the
//! "skeleton" of a page that loads, with the real picture instead of grey shapes).
//!
//! Files are read and written on threads of their own: closing a window or starting the app waits for neither.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use os_plugkit::embed::Pixels;

fn looks() -> &'static Mutex<HashMap<String, Pixels>> {
    static LOOKS: OnceLock<Mutex<HashMap<String, Pixels>>> = OnceLock::new();
    LOOKS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The folder of the kept looks; none in tests (they keep theirs in memory only).
fn dir() -> Option<PathBuf> {
    if cfg!(test) { None } else { os_paths::user_file("editor-looks") }
}

/// A plugin id as a file name: what is not a letter, a digit, `.`, `-` or `_` becomes `_`.
fn file_name(id: &str) -> String {
    let stem: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' }).collect();
    format!("{stem}.png")
}

/// Reads the kept looks from the user folder (at startup), on a thread of its own; one kept since wins.
pub fn load_all() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    let Some(dir) = dir() else { return };
    let mut first = false;
    ONCE.call_once(|| first = true);
    if !first {
        return;
    }
    std::thread::spawn(move || {
        let Ok(entries) = std::fs::read_dir(&dir) else { return };
        for e in entries.flatten() {
            let path = e.path();
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()).map(str::to_string) else { continue };
            let Ok(img) = tiny_skia::Pixmap::load_png(&path) else { continue };
            let px: Vec<u32> = img.pixels().iter().map(|p| (u32::from(p.red()) << 16) | (u32::from(p.green()) << 8) | u32::from(p.blue())).collect();
            let look = Pixels { px: Arc::new(px), w: img.width(), h: img.height() };
            if let Ok(mut m) = looks().lock() {
                m.entry(stem).or_insert(look);
            }
        }
    });
}

/// The look kept for plugin `id`.
pub fn get(id: &str) -> Option<Pixels> {
    let key = file_name(id).trim_end_matches(".png").to_string();
    looks().lock().ok()?.get(&key).cloned()
}

/// Keeps `look` as plugin `id`'s, and writes it to the user folder on a thread of its own.
pub fn put(id: &str, look: Pixels) {
    let name = file_name(id);
    if let Ok(mut m) = looks().lock() {
        m.insert(name.trim_end_matches(".png").to_string(), look.clone());
    }
    let Some(dir) = dir() else { return };
    std::thread::spawn(move || {
        let Some(mut img) = tiny_skia::Pixmap::new(look.w, look.h) else { return };
        for (o, &p) in img.pixels_mut().iter_mut().zip(look.px.iter()) {
            if let Some(c) = tiny_skia::PremultipliedColorU8::from_rgba((p >> 16) as u8, (p >> 8) as u8, p as u8, 255) {
                *o = c;
            }
        }
        if std::fs::create_dir_all(&dir).is_ok() {
            let tmp = dir.join(format!("{name}.part"));
            if img.save_png(&tmp).is_ok() {
                let _ = std::fs::rename(&tmp, dir.join(&name));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kept_by_plugin_id() {
        let look = Pixels { px: Arc::new(vec![0x123456; 6]), w: 3, h: 2 };
        put("com.lemon.citrus/test:1", look);
        let got = get("com.lemon.citrus/test:1").expect("kept");
        assert_eq!((got.w, got.h, got.px[0]), (3, 2, 0x123456));
        assert!(get("another").is_none());
        assert_eq!(file_name("a b/c"), "a_b_c.png");
    }
}
