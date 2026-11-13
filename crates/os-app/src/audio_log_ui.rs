// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The audio logger's menu and commands (TOOLS > Retro audio capture); the log itself is `crate::audio_log`.
// spec: ui/audio-logger.md

use crate::app::{App, State};
use crate::audio_log::{enabled, minutes, LENGTH, MINUTES, ON};
use crate::playlist::{Cmd, Mark, PItem};
use crate::record::RecCmd;

/// TOOLS > Retro audio capture > Settings ▸.
pub fn items(_: &State) -> Vec<PItem> {
    let now = minutes();
    let mut items = vec![PItem::head("Keep the last")];
    items.extend(MINUTES.iter().map(|&m| {
        PItem::new(format!("{m} minute{}", if m == 1 { "" } else { "s" }), Cmd::Rec(RecCmd::AudioLog(m))).mark(Mark::Radio(m == now))
    }));
    items
}

impl App {
    /// TOOLS > Retro audio capture > Enabled.
    pub(crate) fn toggle_audio_log(&mut self) {
        let on = !enabled();
        crate::settings_file::set_flag(ON, on);
        self.audio_log.tick();
        self.state.message = Some(if on { format!("Retro audio capture on: keeps the last {} minute(s) of what plays", minutes()) } else { "Retro audio capture off".into() });
    }

    pub(crate) fn set_audio_log_minutes(&mut self, m: u32) -> String {
        crate::settings_file::write(LENGTH, &m.to_string());
        format!("Retro audio capture: keeps the last {m} minute(s)")
    }

    /// TOOLS > Retro audio capture > Open capture in audio editor: the log (silence at either end left out) into a WAV in the
    /// `Recorded` folder, and into Tesla on the master track as One-click recording's takes go.
    pub(crate) fn dump_audio_log(&mut self) -> String {
        if !enabled() {
            return "Retro audio capture is off: TOOLS > Retro audio capture > Enabled".into();
        }
        let (l, r) = self.audio_log.audio();
        if l.is_empty() {
            return "The audio logger has heard nothing yet".into();
        }
        let sample = tesla::sample::Sample::from_channels(self.audio_log.rate.max(1), vec![l, r], tesla::sample::Format::Float32);
        let dir = crate::track_rec::folder(&self.state.session);
        let name = format!("Audio log_{}.wav", chrono::Local::now().format("%Y-%m-%d %H-%M-%S"));
        let path = dir.join(name);
        let wrote = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, tesla::wav::write(&sample, tesla::wav::Depth::Own)));
        match wrote {
            Ok(()) => self.take_to_editor(&path),
            Err(e) => format!("{} could not be written: {e}", path.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Open capture in audio editor: off, a message; on, the log into a WAV in `Recorded` and into Tesla on the master track.
    #[test]
    fn a_dump_goes_into_the_audio_editor() {
        use crate::mixer::FxReq;
        let mut app = App::new(None);
        app.state.session.project = crate::locked::Locked::new(crate::app::test_project());
        app.state.session.plugins.effects = crate::plugins::builtin_effects();
        crate::settings_file::set_flag(ON, false);
        assert!(app.dump_audio_log().contains("off"));
        crate::settings_file::set_flag(ON, true);
        assert!(app.dump_audio_log().contains("nothing"));
        let tone: Vec<f32> = (0..9600).map(|i| if i % 2 == 0 { (i / 2) as f32 / 4800.0 } else { 0.5 }).collect();
        app.audio_log.keep(&tone, 48000, 5);
        let msg = app.dump_audio_log();
        assert!(msg.starts_with("Recorded Audio log_") && msg.ends_with(" into the audio editor"), "{msg}");
        let Some(FxReq::Create { track: 0, state: Some(st), .. }) = app.state.session.fx_requests.last().cloned() else {
            panic!("{:?}", app.state.session.fx_requests)
        };
        let saved = tesla::state::from_plugin_state(&st).unwrap();
        assert_eq!((saved.sample.frames(), saved.sample.rate), (4800, 48000));
        let path = std::path::PathBuf::from(&saved.path);
        assert!(path.exists() && path.parent().unwrap().ends_with("Recorded"), "{path:?}");
        let _ = std::fs::remove_file(path);
    }
}
