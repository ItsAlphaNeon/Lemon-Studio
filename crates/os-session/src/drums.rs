// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Drum Sampler in Lemon Studio: compiled in like Citrus, it plays every FPC channel of a project (started from
//! the state the file holds, its layers' samples opened from the stored paths) and every one added from the picker;
//! saving writes the state it plays back into the channel byte for byte, so the file keeps its own plugin with its
//! pads and samples.
//!
// spec: plugins/lemon-drum-sampler.md §4 (app integration); plugins/fpc.md §1

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use os_model::{Channel, ChannelKind, Project};

use crate::plugins::{Plugins, FX_KEY};

/// The built-in plugin's name (picker, window title).
pub const LEMON_DRUMS: &str = "Lemon Drum Sampler";
/// The plugin id project files store for it (event 201).
pub const FILE_NAME: &str = "FPC";

/// Whether file id `id` names the original's drum sampler (any case).
pub fn is_file_name(id: &str) -> bool {
    id.eq_ignore_ascii_case(FILE_NAME)
}

/// Whether a channel holds the file's FPC.
pub fn is_fpc(ch: &Channel) -> bool {
    ch.kind == ChannelKind::Plugin && ch.plugin.as_deref().is_some_and(is_file_name)
}

impl Plugins {
    /// The FPC state the Lemon Drum Sampler on rack row `row` plays, `None` when the row hosts something else.
    pub fn drums_file_state(&self, row: usize) -> Option<Vec<u8>> {
        let h = self.hosted.get(&row)?;
        let p = h.lock_for_state()?;
        if p.desc().name != LEMON_DRUMS || !crate::project_plugins::is_builtin(p.desc()) {
            return None;
        }
        lemon_drum_sampler::params::fl_from_plugin_state(&p.save_state()?)
    }
}

/// Before a save: every hosted Lemon Drum Sampler writes the state it plays into its channel as the file's FPC (a
/// channel keeps its own spelling of the id).
pub fn store(proj: &mut Project, plugins: &Plugins) {
    for row in plugins.hosted.range(..FX_KEY).map(|(&k, _)| k).collect::<Vec<_>>() {
        let Some(st) = plugins.drums_file_state(row) else { continue };
        if let Some(ch) = proj.channels.get_mut(row).filter(|c| c.kind == ChannelKind::Plugin) {
            if !ch.plugin.as_deref().is_some_and(is_file_name) {
                ch.plugin = Some(FILE_NAME.into());
            }
            ch.plugin_state = Some(st);
        }
    }
}

/// The plugin state for a channel's FPC state: every layer's stored sample path resolved for the plugin to open (FL's
/// placeholders made real), the stored paths kept in the state.
pub fn plugin_state(fl: &[u8]) -> Vec<u8> {
    let paths: Vec<String> = lemon_drum_sampler::params::stored_paths(fl).iter().map(|p| crate::plugins::resolve_sample(p)).collect();
    lemon_drum_sampler::params::plugin_state_from_fl(fl, &paths)
}

/// Hosts Lemon Drum Sampler on every FPC channel without an instance, started from the channel's saved state (a new
/// instance's when it has none). A row waiting for the picker's own instance is left to it; a row whose instance
/// failed is not tried again.
pub fn sync_drums(s: &mut crate::session::Session) {
    if crate::project_plugins::native_first() {
        return;
    }
    let Some(index) = s.plugins.index_of(LEMON_DRUMS) else { return };
    let pending = s.pending_plugin.map(|(r, _)| r);
    let rows: Vec<usize> = (0..s.project.channels.len())
        .filter(|&r| is_fpc(&s.project.channels[r]))
        .filter(|&r| !s.hosted_rows.contains(&r) && !s.plugins.wrap_failed.contains(&r) && Some(r) != pending)
        .collect();
    if rows.is_empty() {
        return;
    }
    let mut added = false;
    for row in rows {
        if s.plugins.past_deadline() {
            break;
        }
        let fl = s.project.channels[row].plugin_state.clone().unwrap_or_default();
        match s.plugins.create_with(row, index, Some(&plugin_state(&fl))) {
            Ok(_) => {
                s.hosted_rows.push(row);
                added = true;
            }
            Err(e) => {
                s.plugins.wrap_failed.push(row);
                s.touched.views.push(crate::session::View::Message(format!("{LEMON_DRUMS}: {e}")));
            }
        }
    }
    if added {
        s.transport.set_instruments(s.plugins.instruments());
        s.dirty = true;
    }
    s.touched.repaint = true;
}

#[cfg(test)]
mod tests {
    use super::*;
    use lemon_drum_sampler::state::State;
    use os_engine::SharedInstrument;

    /// An FPC state with `path` on pad 2 (key 36).
    fn state_for(path: &std::path::Path) -> Vec<u8> {
        let mut s = State::default();
        s.set_sample(1, &path.to_string_lossy());
        s.to_bytes()
    }

    /// A 0.1 s stereo 16-bit WAV at 44.1 kHz: a tone.
    fn wav(name: &str) -> std::path::PathBuf {
        let frames = 4410usize;
        let mut data = Vec::new();
        for i in 0..frames {
            let v = ((i as f32 * 0.05).sin() * 16000.0) as i16;
            data.extend(v.to_le_bytes());
            data.extend(v.to_le_bytes());
        }
        let mut fmt = Vec::new();
        for v in [1u16, 2] {
            fmt.extend(v.to_le_bytes());
        }
        fmt.extend(44100u32.to_le_bytes());
        fmt.extend((44100u32 * 4).to_le_bytes());
        fmt.extend(4u16.to_le_bytes());
        fmt.extend(16u16.to_le_bytes());
        let mut body = b"WAVE".to_vec();
        for (id, b) in [(b"fmt ", &fmt), (b"data", &data)] {
            body.extend(id);
            body.extend((b.len() as u32).to_le_bytes());
            body.extend(b);
        }
        let mut f = b"RIFF".to_vec();
        f.extend((body.len() as u32).to_le_bytes());
        f.extend(body);
        let p = std::env::temp_dir().join(format!("openstudio-drums-{}-{name}.wav", std::process::id()));
        std::fs::write(&p, f).unwrap();
        p
    }

    fn peak(i: &mut dyn os_engine::Instrument) -> f32 {
        let (mut l, mut r) = (vec![0.0f32; 2400], vec![0.0f32; 2400]);
        i.process(48000, &mut l, &mut r);
        l.iter().chain(&r).fold(0.0f32, |m, v| m.max(v.abs()))
    }

    /// The built-in Lemon Drum Sampler starts from a channel's state, opens its pad's sample, plays it on the pad's key
    /// (and nothing on another key) and gives the same bytes back.
    #[test]
    fn builtin_drums_take_and_give_the_file_state() {
        let path = wav("plays");
        let mut pl = Plugins::default();
        pl.available = crate::plugins::builtin();
        pl.set_rate(48000);
        let index = pl.index_of(LEMON_DRUMS).expect("built-in Lemon Drum Sampler");
        assert!(crate::project_plugins::is_builtin(&pl.available[index].desc));
        assert_eq!(crate::plugin_db::category(&pl.available[index].desc), "Sampler");
        let fl = state_for(&path);
        pl.create_with(1, index, Some(&plugin_state(&fl))).unwrap();
        assert_eq!(pl.drums_file_state(1), Some(fl.clone()));
        let inst = pl.hosted[&1].plugin.clone() as SharedInstrument;
        let mut i = inst.lock().unwrap();
        i.note_on(0, 60, 100.0 / 128.0);
        assert!(peak(&mut *i) == 0.0, "no pad on C5");
        i.note_on(0, 36, 100.0 / 128.0);
        let p = peak(&mut *i);
        assert!(p > 0.2 && p < 1.0, "the pad sounds: {p}");
        drop(i);
        assert_eq!(pl.drums_file_state(99), None);
        let _ = std::fs::remove_file(path);
    }

}
