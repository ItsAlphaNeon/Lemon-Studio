// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! TOOLS ▸ Save audio clipboard to browser (Lemon Studio's design after the original's entry, the counterpart of Save
//! MIDI clipboard to browser): the audio on the system clipboard (what Tesla's Copy puts there, or any program's copied
//! WAV) goes to "Audio clipboard <n>.wav" in the user folder's "Clipboard files", the next free number; the folder
//! joins the browser and opens there. Greyed while the clipboard holds no audio, as the original's is drawn then.
//! Windows has the clipboard's wave format, macOS the pasteboard's WAV type (the ones Tesla writes); Linux has no
//! audio clipboard Lemon Studio can read yet, so the entry stays greyed there.
// spec: ui/menus.md "TOOLS"

use crate::app::App;

/// Whether `b` is a whole WAV file (RIFF ... WAVE).
fn is_wav(b: &[u8]) -> bool {
    b.len() > 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WAVE"
}

#[cfg(test)]
thread_local! {
    /// The clipboard as the tests set it (`set_for_test`).
    static TEST_CLIP: std::cell::RefCell<Option<Vec<u8>>> = const { std::cell::RefCell::new(None) };
}

/// Tests: what the clipboard holds.
#[cfg(test)]
pub fn set_for_test(bytes: Option<Vec<u8>>) {
    TEST_CLIP.with(|c| *c.borrow_mut() = bytes);
}

/// Whether the clipboard holds audio (cheap enough for every frame: the menu greys the entry by it).
pub fn has_audio() -> bool {
    #[cfg(test)]
    let has = TEST_CLIP.with(|c| c.borrow().as_deref().is_some_and(is_wav));
    #[cfg(all(not(test), windows))]
    let has = win::has_wave();
    #[cfg(all(not(test), target_os = "macos"))]
    let has = mac::has_wav();
    #[cfg(all(not(test), not(windows), not(target_os = "macos")))]
    let has = false;
    has
}

/// The clipboard's audio as a WAV file, if it holds one.
fn wav() -> Option<Vec<u8>> {
    #[cfg(test)]
    let b = TEST_CLIP.with(|c| c.borrow().clone());
    #[cfg(all(not(test), windows))]
    let b = win::wave();
    #[cfg(all(not(test), target_os = "macos"))]
    let b = mac::wav();
    #[cfg(all(not(test), not(windows), not(target_os = "macos")))]
    let b: Option<Vec<u8>> = None;
    b.filter(|b| is_wav(b))
}

impl App {
    /// TOOLS ▸ Save audio clipboard to browser.
    pub(crate) fn save_audio_clipboard(&mut self) {
        let Some(bytes) = wav() else {
            self.state.message = Some("The clipboard holds no audio".into());
            return;
        };
        let Some(dir) = crate::midi_file::clipboard_dir() else { return };
        let path = (1..).map(|n| dir.join(format!("Audio clipboard {n}.wav"))).find(|p| !p.exists()).expect("a free name");
        match std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, bytes)) {
            Ok(()) => {
                self.add_browser_folder(dir);
                self.state.message = Some(format!("Saved {}", path.display()));
            }
            Err(e) => self.state.message = Some(format!("Could not write {}: {e}", path.display())),
        }
        self.invalidate();
    }
}

#[cfg(windows)]
mod win {
    use windows_sys::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard};
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};

    /// The clipboard's standard wave format (`CF_WAVE`).
    const CF_WAVE: u32 = 12;

    #[cfg_attr(test, allow(dead_code))]
    pub fn has_wave() -> bool {
        unsafe { IsClipboardFormatAvailable(CF_WAVE) != 0 }
    }

    #[cfg_attr(test, allow(dead_code))]
    pub fn wave() -> Option<Vec<u8>> {
        unsafe {
            if OpenClipboard(std::ptr::null_mut()) == 0 {
                return None;
            }
            let h = GetClipboardData(CF_WAVE);
            let out = if h.is_null() {
                None
            } else {
                let size = GlobalSize(h);
                let p = GlobalLock(h) as *const u8;
                let v = (!p.is_null()).then(|| std::slice::from_raw_parts(p, size).to_vec());
                GlobalUnlock(h);
                v
            };
            CloseClipboard();
            out
        }
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use objc2_app_kit::NSPasteboard;
    use objc2_foundation::NSString;
    use std::cell::Cell;

    /// The pasteboard type Tesla writes its audio as.
    const WAV_TYPE: &str = "com.microsoft.waveform-audio";

    thread_local! {
        /// The pasteboard's change count when last looked at, and whether it held audio then: the menu bar asks every
        /// frame, the pasteboard is read again only after it changed.
        static SEEN: Cell<Option<(isize, bool)>> = const { Cell::new(None) };
    }

    #[cfg_attr(test, allow(dead_code))]
    pub fn has_wav() -> bool {
        let pb = NSPasteboard::generalPasteboard();
        let count = pb.changeCount();
        if let Some((_, has)) = SEEN.with(Cell::get).filter(|(c, _)| *c == count) {
            return has;
        }
        let has = wav().is_some_and(|b| super::is_wav(&b));
        SEEN.with(|s| s.set(Some((count, has))));
        has
    }

    #[cfg_attr(test, allow(dead_code))]
    pub fn wav() -> Option<Vec<u8>> {
        NSPasteboard::generalPasteboard().dataForType(&NSString::from_str(WAV_TYPE)).map(|d| d.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crate::views::menus::{active, item_at, works, TOOLS_MENU};

    /// A WAV file of `n` silent mono 16-bit frames at 44.1 kHz.
    fn wav(n: u32) -> Vec<u8> {
        let data = n * 2;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&[1, 0, 1, 0]);
        b.extend_from_slice(&44100u32.to_le_bytes());
        b.extend_from_slice(&88200u32.to_le_bytes());
        b.extend_from_slice(&[2, 0, 16, 0]);
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data.to_le_bytes());
        b.resize(b.len() + data as usize, 0);
        b
    }

    /// Save audio clipboard to browser is greyed while the clipboard holds no audio, then writes it as is into a
    /// numbered file of the clipboard folder, which the browser then shows.
    #[test]
    fn save_audio_clipboard_to_browser() {
        let mut app = App::new(None);
        let at = (0..60).find(|&i| item_at(TOOLS_MENU, i).is_some_and(|it| it.label == crate::views::menus::SAVE_AUDIO_CLIP_LABEL)).unwrap();
        let entry = item_at(TOOLS_MENU, at).unwrap();
        assert!(works(TOOLS_MENU, entry));
        super::set_for_test(None);
        assert!(!active(&app.state, TOOLS_MENU, entry), "nothing copied");
        super::set_for_test(Some(b"plain text".to_vec()));
        assert!(!active(&app.state, TOOLS_MENU, entry), "not audio");
        let audio = wav(100);
        super::set_for_test(Some(audio.clone()));
        assert!(active(&app.state, TOOLS_MENU, entry));
        let dir = crate::midi_file::clipboard_dir().unwrap();
        let _ = std::fs::remove_file(dir.join("Audio clipboard 1.wav"));
        let _ = std::fs::remove_file(dir.join("Audio clipboard 2.wav"));
        app.menu_item(TOOLS_MENU, at);
        app.menu_item(TOOLS_MENU, at);
        assert_eq!(std::fs::read(dir.join("Audio clipboard 2.wav")).unwrap(), audio);
        assert!(dir.join("Audio clipboard 1.wav").exists() && app.state.browser_folders.contains(&dir));
        let _ = std::fs::remove_file(dir.join("Audio clipboard 1.wav"));
        let _ = std::fs::remove_file(dir.join("Audio clipboard 2.wav"));
        super::set_for_test(None);
    }
}
