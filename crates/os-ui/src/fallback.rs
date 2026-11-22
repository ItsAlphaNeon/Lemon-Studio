// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! System fonts for characters the bundled faces lack (Chinese, Japanese, Korean, ...), for the outline engine.
//!
//! On Linux, fontconfig is asked (`fc-match`) for a sans-serif font with the character; failing that, and on macOS, a
//! list of the fonts systems usually ship is tried in order. A font file is read once per process and kept for its
//! lifetime (`'static`), and a character no font has is drawn as the face's missing glyph.
// spec: ui/main-window.md §10 (typography)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// A font file's bytes and the face inside it (collections hold several).
pub type Source = (&'static [u8], u32);

/// Font files larger than this are not read (the largest CJK collections are about 30 MB).
const MAX_BYTES: u64 = 64 << 20;

/// Fonts usually installed, in the order they are tried: broad sans-serif faces, then CJK faces, then symbols.
#[cfg(all(unix, not(target_os = "macos")))]
const KNOWN: &[&str] = &[
    "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/google-noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/google-noto-sans-cjk-vf-fonts/NotoSansCJK-VF.ttc",
    "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
    "/usr/share/fonts/google-droid/DroidSansFallbackFull.ttf",
    "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
    "/usr/share/fonts/wenquanyi/wqy-microhei/wqy-microhei.ttc",
    "/usr/share/fonts/truetype/unifont/unifont.ttf",
];
#[cfg(target_os = "macos")]
const KNOWN: &[&str] = &[
    "/System/Library/Fonts/Helvetica.ttc",
    "/System/Library/Fonts/PingFang.ttc",
    "/System/Library/Fonts/Hiragino Sans GB.ttc",
    "/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc",
    "/System/Library/Fonts/AppleSDGothicNeo.ttc",
    "/System/Library/Fonts/STHeiti Light.ttc",
    "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
    "/Library/Fonts/Arial Unicode.ttf",
    "/System/Library/Fonts/Apple Symbols.ttf",
];
/// Windows draws with GDI, which falls back by itself; the outline engine runs there only in tests.
#[cfg(windows)]
const KNOWN: &[&str] = &["C:\\Windows\\Fonts\\segoeui.ttf", "C:\\Windows\\Fonts\\msyh.ttc", "C:\\Windows\\Fonts\\malgun.ttf", "C:\\Windows\\Fonts\\seguisym.ttf"];

/// Files read so far (`None`: unreadable), shared by every engine in the process.
type Files = HashMap<PathBuf, Option<&'static [u8]>>;
static FILES: Mutex<Option<Files>> = Mutex::new(None);

/// The bytes of the font file at `path`, read once.
fn load(path: &Path) -> Option<&'static [u8]> {
    let mut files = FILES.lock().unwrap_or_else(|e| e.into_inner());
    let files = files.get_or_insert_with(HashMap::new);
    *files.entry(path.to_path_buf()).or_insert_with(|| {
        let len = std::fs::metadata(path).ok()?.len();
        if len > MAX_BYTES {
            return None;
        }
        let data = std::fs::read(path).ok()?;
        Some(Box::leak(data.into_boxed_slice()))
    })
}

/// Fonts that may have `c`, best first; the caller checks each one's character map.
pub fn candidates(c: char) -> Vec<(PathBuf, u32)> {
    let mut out = Vec::new();
    #[cfg(all(unix, not(target_os = "macos")))]
    if let Some(found) = fc_match(c) {
        out.push(found);
    }
    let _ = c;
    out.extend(KNOWN.iter().map(|p| (PathBuf::from(p), 0)));
    out
}

/// fontconfig's sans-serif font for `c`: its file and face index.
#[cfg(all(unix, not(target_os = "macos")))]
fn fc_match(c: char) -> Option<(PathBuf, u32)> {
    let out = std::process::Command::new("fc-match")
        .arg("--format=%{file}\n%{index}")
        .arg(format!("sans-serif:charset={:x}", u32::from(c)))
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let mut lines = text.lines();
    let file = lines.next().filter(|f| !f.is_empty())?;
    let index = lines.next().and_then(|i| i.trim().parse().ok()).unwrap_or(0);
    Some((PathBuf::from(file), index))
}

/// The first font of `candidates` whose character map has `c` (by `has`), read.
pub fn find(c: char, has: impl Fn(Source) -> bool) -> Option<Source> {
    candidates(c).into_iter().find_map(|(path, index)| {
        let src = (load(&path)?, index);
        has(src).then_some(src)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_read_once() {
        let missing = Path::new("/no/such/font.ttf");
        assert!(load(missing).is_none());
        if let Some((path, _)) = candidates('a').into_iter().find(|(p, _)| p.exists()) {
            let (a, b) = (load(&path).unwrap(), load(&path).unwrap());
            assert!(std::ptr::eq(a, b));
        }
    }
}
