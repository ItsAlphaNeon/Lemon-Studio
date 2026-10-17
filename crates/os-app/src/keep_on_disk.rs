// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! General ▸ Advanced ▸ Auto keep long audio on disk (`prefs::AUTO_KEEP_ON_DISK`): a file of 30 seconds or more loaded
//! into a sampler channel turns on the channel's Keep on disk switch (sampler flags 0x100, Lemon Sampler's
//! "keep_disk"), by its length in time (frames / rate), whatever its rate and channels. A load never turns it off.
//! It comes with the paths that read the file into the channel (`rack::read_tags`: Load sample..., a drop on the rack
//! or on a plugin window, the browser's send), inside their "Load channel sample(s)" history step; project loading and
//! previews don't.
// spec: ui/settings-choices.md "Advanced: Fast sample preview, Auto keep long audio on disk, Threaded project sample
// loading (2026-10-16)"

use std::path::Path;

use os_model::ChannelSound;

/// The shortest file kept on disk, in seconds.
pub const LONG_SECS: u64 = 30;

/// Sample `path` went into sampler channel `ch`: while the check is on, a file of `LONG_SECS` or more turns Keep on
/// disk on (the channel's other sampler flags stay; a channel without them gets those of a new channel).
pub fn keep_long(ch: &mut os_model::Channel, path: &Path) {
    if !ch.kind.is_sampler() || !crate::prefs::check(&crate::prefs::AUTO_KEEP_ON_DISK) {
        return;
    }
    let Some((frames, rate)) = os_engine::sample::length(path) else { return };
    if rate > 0 && frames >= LONG_SECS * u64::from(rate) {
        let s = &mut ch.sound;
        s.sampler_flags = Some(s.sampler_flags.unwrap_or(ChannelSound::DEFAULT_FLAGS) | ChannelSound::KEEP_ON_DISK);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::history::{ctrl_alt_z, ctrl_z, LOAD_SAMPLE};
    use crate::prefs::{check, set_check, AUTO_KEEP_ON_DISK};
    use crate::rack::sample_chosen;
    use std::path::PathBuf;

    const KEPT: Option<u32> = Some(ChannelSound::DEFAULT_FLAGS | ChannelSound::KEEP_ON_DISK);

    /// A silent 16-bit WAV of `frames` frames at `rate` with `channels` channels, under a name of its own (tests run in
    /// parallel).
    fn wav(name: &str, rate: u32, channels: u16, frames: u64) -> PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let k = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let block = 2 * channels;
        let mut fmt = vec![1, 0];
        fmt.extend(channels.to_le_bytes());
        fmt.extend(rate.to_le_bytes());
        fmt.extend((rate * u32::from(block)).to_le_bytes());
        fmt.extend(block.to_le_bytes());
        fmt.extend(16u16.to_le_bytes());
        let data = frames as usize * usize::from(block);
        let mut b = b"RIFF".to_vec();
        b.extend(((4 + 8 + fmt.len() + 8 + data) as u32).to_le_bytes());
        b.extend(b"WAVEfmt ");
        b.extend((fmt.len() as u32).to_le_bytes());
        b.extend(fmt);
        b.extend(b"data");
        b.extend((data as u32).to_le_bytes());
        b.resize(b.len() + data, 0);
        let p = std::env::temp_dir().join(format!("os-app-keep-{name}-{}-{k}.wav", std::process::id()));
        std::fs::write(&p, b).unwrap();
        p
    }

    /// The default project's sampler row.
    fn sampler_row(app: &App) -> usize {
        (0..app.state.project.channels.len()).find(|&r| app.state.project.channels[r].kind == os_model::ChannelKind::Sampler).unwrap()
    }

    fn flags(app: &App, r: usize) -> Option<u32> {
        app.state.project.channels[r].sound.sampler_flags
    }

    // spec: ui/settings-choices.md "Advanced: Fast sample preview, Auto keep long audio on disk, Threaded project
    // sample loading (2026-10-16)"
    /// On (as found), a fresh channel each time: 29.9 s and shorter leave Keep on disk off, 30 s and longer turn it on,
    /// mono and stereo alike. The length is time: 30 s at 22.05 kHz turns it on, 20 s at 96 kHz (more frames than 30 s
    /// at 44.1 kHz) doesn't. A drop below the rack's rows (a new channel) does the same.
    #[test]
    fn files_of_30_seconds_are_kept_on_disk() {
        assert!(check(&AUTO_KEEP_ON_DISK), "on by default");
        let cases: [(&str, u32, u16, u64, bool); 7] = [
            ("29.9 s mono", 44100, 1, 1_318_590, false),
            ("30 s mono", 44100, 1, 1_323_000, true),
            ("29.9 s stereo", 44100, 2, 1_318_590, false),
            ("30 s stereo", 44100, 2, 1_323_000, true),
            ("30 s at 22.05 kHz", 22050, 1, 661_500, true),
            ("29.9 s at 22.05 kHz", 22050, 1, 659_295, false),
            ("20 s at 96 kHz", 96000, 1, 1_920_000, false),
        ];
        for (what, rate, channels, frames, kept) in cases {
            let mut app = App::new(None);
            let r = sampler_row(&app);
            assert_eq!(flags(&app, r), None, "a fresh channel");
            let p = wav("case", rate, channels, frames);
            sample_chosen(&mut app, r, p.clone());
            assert_eq!(flags(&app, r), if kept { KEPT } else { None }, "{what}");
            let _ = std::fs::remove_file(p);
        }
        let mut app = App::new(None);
        let n = app.state.project.channels.len();
        let p = wav("new-row", 44100, 2, 1_323_000);
        app.sample_to_rack(p.to_string_lossy().into_owned(), crate::views::rack::rect(&app.state).bottom() + 40.0);
        assert_eq!(app.state.project.channels.len(), n + 1);
        assert_eq!(flags(&app, n), KEPT, "a channel a drop made");
        let _ = std::fs::remove_file(p);
    }

    /// A load never turns Keep on disk off: a short file after a long one leaves it on. The channel's other flags stay.
    #[test]
    fn a_short_file_after_a_long_one_keeps_it() {
        let mut app = App::new(None);
        let r = sampler_row(&app);
        app.state.project.channels[r].sound.sampler_flags = Some(ChannelSound::RESAMPLE);
        let (long, short) = (wav("long", 44100, 1, 31 * 44100), wav("short", 44100, 1, 5 * 44100));
        sample_chosen(&mut app, r, long.clone());
        assert_eq!(flags(&app, r), Some(ChannelSound::RESAMPLE | ChannelSound::KEEP_ON_DISK));
        sample_chosen(&mut app, r, short.clone());
        assert_eq!(flags(&app, r), Some(ChannelSound::RESAMPLE | ChannelSound::KEEP_ON_DISK), "still on");
        for p in [long, short] {
            let _ = std::fs::remove_file(p);
        }
    }

    /// Off: a 60 s file leaves it off.
    #[test]
    fn off_leaves_it_off() {
        set_check(&AUTO_KEEP_ON_DISK, false);
        let mut app = App::new(None);
        let r = sampler_row(&app);
        let p = wav("off", 44100, 2, 60 * 44100);
        sample_chosen(&mut app, r, p.clone());
        assert_eq!(flags(&app, r), None);
        set_check(&AUTO_KEEP_ON_DISK, true);
        let _ = std::fs::remove_file(p);
    }

    /// The switch is part of the load's one history step: undo puts the flags back as they were, redo turns it on.
    #[test]
    fn undo_restores_the_flag() {
        let mut app = App::new(None);
        let r = sampler_row(&app);
        let steps = app.state.history.len();
        let p = wav("undo", 44100, 1, 30 * 44100);
        sample_chosen(&mut app, r, p.clone());
        assert_eq!(flags(&app, r), KEPT);
        assert_eq!(app.state.history.len(), steps + 1, "one step");
        assert_eq!(app.state.history.entries().last(), Some(&(steps + 1, LOAD_SAMPLE)));
        ctrl_z(&mut app.state);
        assert_eq!(flags(&app, r), None, "undone");
        ctrl_alt_z(&mut app.state);
        assert_eq!(flags(&app, r), KEPT, "redone");
        let _ = std::fs::remove_file(p);
    }

    /// The channel's Lemon Sampler gets the switch with the sample on the next tick, and follows undo.
    #[test]
    fn the_hosted_sampler_sees_it() {
        let mut app = App::new(None);
        app.plugins = crate::plugins::Plugins::default();
        app.plugins.available = crate::plugins::builtin();
        let r = sampler_row(&app);
        app.plugin_tick();
        assert!(app.state.hosted_rows.contains(&r), "Lemon Sampler hosts the row");
        let keep_disk = |app: &App| {
            let st = app.plugins.state(r).expect("state");
            let json = if st.first() == Some(&b'{') { &st[..] } else { &st[8..] };
            serde_json::from_slice::<serde_json::Value>(json).unwrap()["params"]["keep_disk"]["bool"].as_bool()
        };
        assert_eq!(keep_disk(&app), Some(false));
        let p = wav("hosted", 48000, 2, 30 * 48000);
        sample_chosen(&mut app, r, p.clone());
        app.plugin_tick();
        assert_eq!(app.plugins.sampler_path(r).as_deref(), Some(p.to_string_lossy().as_ref()));
        assert_eq!(keep_disk(&app), Some(true));
        ctrl_z(&mut app.state);
        app.plugin_tick();
        assert_eq!(keep_disk(&app), Some(false), "undone");
        let _ = std::fs::remove_file(p);
    }
}
