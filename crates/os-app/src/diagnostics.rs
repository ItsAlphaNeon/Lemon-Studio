// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! HELP ▸ Diagnostics (Lemon Studio's design: the original's entry was not examined, and its support tools are the
//! maker's own): a plain-text report of this build, the system, the audio settings, the plugins found and the open
//! project, written to "Diagnostics.txt" in the user folder and opened in the system's text viewer, ready to paste
//! into a bug report. It reads only what the program already knows: no device is opened and no plugin loaded for it.
// spec: ui/menus.md "HELP"

use std::path::PathBuf;

use crate::app::App;

/// Where the report goes: the user folder's "Diagnostics.txt" (tests: a temporary file of the test's own thread).
fn report_path() -> Option<PathBuf> {
    if cfg!(test) {
        let thread = format!("{:?}", std::thread::current().id()).chars().filter(char::is_ascii_digit).collect::<String>();
        return Some(std::env::temp_dir().join(format!("lemon-diagnostics-{}-{thread}.txt", std::process::id())));
    }
    os_paths::user_file("Diagnostics.txt")
}

impl App {
    /// The report's text.
    pub(crate) fn diagnostics(&self) -> String {
        let s = &self.state;
        let p = &s.project;
        let cfg = crate::audio_cfg::AudioCfg::current();
        let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
        let out = [
            format!("Lemon Studio {} (pre-alpha)", env!("CARGO_PKG_VERSION")),
            format!("System: {} {}, {cpus} logical processors", std::env::consts::OS, std::env::consts::ARCH),
            format!("User folder: {}", os_paths::user_dir().map_or_else(|| "none".into(), |d| d.display().to_string())),
            String::new(),
            "Audio settings:".into(),
            format!("  Audio system: {}", crate::audio_cfg::host_name(&cfg)),
            format!("  Output: {}", cfg.device.as_deref().unwrap_or("the system's default")),
            format!("  Input: {}", cfg.input.as_deref().unwrap_or("the system's default")),
            format!("  Sample rate: {}", cfg.rate.map_or_else(|| "the device's own".into(), |r| format!("{r} Hz"))),
            format!("  Buffer: {}", cfg.buffer.map_or_else(|| "the device's own".into(), |b| if b == crate::audio_cfg::SMALLEST { "smallest".into() } else { format!("{b} frames") })),
            format!("  Exclusive mode: {}", if cfg.exclusive { "on" } else { "off" }),
            String::new(),
            format!("Plugins found: {} instruments, {} effects", self.plugins.available.len(), self.plugins.effects.len()),
            format!("Plugins hosted now: {}", self.plugins.hosted.len()),
            String::new(),
            format!("Project: {}", s.path.as_ref().map_or_else(|| "not saved".into(), |f| f.display().to_string())),
            format!("  Channels: {}, patterns: {}, mixer tracks: {}, arranger clips: {}", p.channels.len(), p.patterns.len(), p.mixer.len(), p.arrangements.iter().map(|a| a.clips.len()).sum::<usize>()),
            format!("  Tempo: {} BPM, {} PPQ", s.tempo, p.ppq),
            String::new(),
        ];
        out.join("\n")
    }

    /// HELP ▸ Diagnostics: the report written and opened.
    pub(crate) fn save_diagnostics(&mut self) {
        let Some(path) = report_path() else { return };
        let text = self.diagnostics();
        let written = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&path, text));
        self.state.message = Some(match written {
            Ok(()) => {
                crate::help_links::open(&path.to_string_lossy());
                format!("Saved {}", path.display())
            }
            Err(e) => format!("Could not write {}: {e}", path.display()),
        });
        self.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crate::views::menus::{item_at, works, HELP_MENU};

    /// HELP ▸ Diagnostics works: it writes the report, naming this build and the open project, and says where.
    #[test]
    fn diagnostics_writes_the_report() {
        let mut app = App::new(None);
        app.state.project = crate::app::test_project();
        let at = (0..20).find(|&i| item_at(HELP_MENU, i).is_some_and(|it| it.label == "Diagnostics")).unwrap();
        assert!(works(HELP_MENU, item_at(HELP_MENU, at).unwrap()));
        app.menu_item(HELP_MENU, at);
        let path = super::report_path().unwrap();
        assert_eq!(app.state.message, Some(format!("Saved {}", path.display())));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with(&format!("Lemon Studio {} (pre-alpha)\n", env!("CARGO_PKG_VERSION"))), "{text}");
        assert!(text.contains(&format!("System: {} {}", std::env::consts::OS, std::env::consts::ARCH)), "{text}");
        assert!(text.contains("Project: not saved\n  Channels: 5, patterns: 1, mixer tracks: 18"), "{text}");
        let _ = std::fs::remove_file(path);
    }
}
