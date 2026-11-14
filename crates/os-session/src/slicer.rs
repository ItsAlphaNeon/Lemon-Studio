// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Slicer in Lemon Studio: compiled in like Citrus, it plays every Fruity Slicer channel of a project (started
//! from the state the file holds, its loop opened from the stored path) and every one added from the picker; saving
//! writes the state it plays back into the channel, so the file keeps its own plugin with its loop and slices. The
//! dump to the piano roll arrives through the host link (`Notes`) and replaces the channel's notes in the current
//! pattern.
//!
// spec: plugins/lemon-slicer.md §4 (dump), §6 (app integration); plugins/fruity-slicer.md §1

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use os_model::{Channel, ChannelKind, Project};

use crate::plugins::{Plugins, FX_KEY};

/// The built-in plugin's name (picker, window title).
pub const LEMON_SLICER: &str = "Lemon Slicer";
/// The plugin id project files store for it (event 201); FL's own demos also store "Fruity slicer".
pub const FILE_NAME: &str = "Fruity Slicer";

/// Whether file id `id` names the original's slicer (any case).
pub fn is_file_name(id: &str) -> bool {
    id.eq_ignore_ascii_case(FILE_NAME)
}

/// Whether a channel holds the file's Fruity Slicer.
pub fn is_slicer(ch: &Channel) -> bool {
    ch.kind == ChannelKind::Plugin && ch.plugin.as_deref().is_some_and(is_file_name)
}

impl Plugins {
    /// The Fruity Slicer state the Lemon Slicer on rack row `row` plays, `None` when the row hosts something else.
    pub fn slicer_file_state(&self, row: usize) -> Option<Vec<u8>> {
        let h = self.hosted.get(&row)?;
        let p = h.lock_for_state()?;
        if p.desc().name != LEMON_SLICER || !crate::project_plugins::is_builtin(p.desc()) {
            return None;
        }
        lemon_slicer::params::fl_from_plugin_state(&p.save_state()?)
    }

    /// Loads loop file `path` into the Lemon Slicer on rack row `row` as its own Load does (a file dropped on its
    /// window): the file's beats, tempo and slices, the settings kept. The number of slices, or why the file was not
    /// taken; `None` when the row hosts no Lemon Slicer.
    pub fn load_slicer_loop(&mut self, row: usize, path: &str) -> Option<Result<usize, String>> {
        let st = lemon_slicer::state::State::parse(&self.slicer_file_state(row)?)?;
        let st = match lemon_slicer::file_state(path, st) {
            Ok((st, _)) => st,
            Err(e) => return Some(Err(e)),
        };
        let state = lemon_slicer::params::plugin_state_from_fl(&st.to_bytes());
        let state = crate::plugins::with_sample_path(&state, path).unwrap_or(state);
        let loaded = self.hosted.get(&row).and_then(|h| h.plugin.lock().ok()).is_some_and(|mut p| p.load_state(&state));
        Some(if loaded { Ok(st.slices.len()) } else { Err(format!("{path}: the plugin did not take it")) })
    }
}

/// Before a save: every hosted Lemon Slicer writes the state it plays into its channel as the file's Fruity Slicer
/// (a channel keeps its own spelling of the id).
pub fn store(proj: &mut Project, plugins: &Plugins) {
    for row in plugins.hosted.range(..FX_KEY).map(|(&k, _)| k).collect::<Vec<_>>() {
        let Some(st) = plugins.slicer_file_state(row) else { continue };
        if let Some(ch) = proj.channels.get_mut(row).filter(|c| c.kind == ChannelKind::Plugin) {
            if !ch.plugin.as_deref().is_some_and(is_file_name) {
                ch.plugin = Some(FILE_NAME.into());
            }
            ch.plugin_state = Some(st);
        }
    }
}

/// The plugin state for a channel's Fruity Slicer state: its loop's stored path resolved for the plugin to open
/// (FL's placeholders made real), the stored path kept in the state.
pub fn plugin_state(settings: &crate::settings_file::SettingsStore, fl: &[u8]) -> Vec<u8> {
    let st = lemon_slicer::params::plugin_state_from_fl(fl);
    let stored = lemon_slicer::state::State::parse(fl).map(|s| s.path()).unwrap_or_default();
    let sample = crate::plugins::resolve_sample(settings, &stored);
    if sample.is_empty() {
        return st;
    }
    crate::plugins::with_sample_path(&st, &sample).unwrap_or(st)
}

/// Hosts Lemon Slicer on every Fruity Slicer channel without an instance, started from the channel's saved state
/// (a new instance's when it has none). A row waiting for the picker's own instance is left to it; a row whose
/// instance failed is not tried again.
pub fn sync_slicer(s: &mut crate::session::Session) {
    if crate::project_plugins::native_first() {
        return;
    }
    let Some(index) = s.plugins.index_of(LEMON_SLICER) else { return };
    let pending = s.pending_plugin.map(|(r, _)| r);
    let rows: Vec<usize> = (0..s.project.channels.len())
        .filter(|&r| is_slicer(&s.project.channels[r]))
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
        match s.plugins.create_with(row, index, Some(&plugin_state(&s.settings, &fl))) {
            Ok(_) => {
                s.hosted_rows.push(row);
                added = true;
            }
            Err(e) => {
                s.plugins.wrap_failed.push(row);
                s.touched.views.push(crate::session::View::Message(format!("{LEMON_SLICER}: {e}")));
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
    use lemon_slicer::state::{Slice, State, H_BEATS, H_BPM};
    use os_engine::SharedInstrument;

    /// A 1 s stereo 16-bit WAV at 44.1 kHz: a tone in its second half.
    fn loop_file(name: &str) -> std::path::PathBuf {
        let frames = 44100usize;
        let mut data = Vec::new();
        for i in 0..frames {
            let v = if i >= frames / 2 { ((i as f32 * 0.05).sin() * 16000.0) as i16 } else { 0 };
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
        let p = std::env::temp_dir().join(format!("openstudio-slicer-{}-{name}.wav", std::process::id()));
        std::fs::write(&p, f).unwrap();
        p
    }

    /// A Fruity Slicer state for `path`: two slices (silence, then the tone), 2 beats at 120 BPM.
    fn state_for(path: &std::path::Path) -> Vec<u8> {
        let mut s = State::default();
        s.set_f32(H_BEATS, 2.0);
        s.set_f32(H_BPM, 120.0);
        s.set_path(&path.to_string_lossy());
        s.slices = vec![Slice { name: b"Rest".to_vec(), ..Slice::at(0) }, Slice::at(22050)];
        s.to_bytes()
    }

    /// The built-in Lemon Slicer starts from a channel's state, opens its loop, plays slice 2 on C#5 and gives the
    /// same bytes back.
    #[test]
    fn builtin_slicer_takes_and_gives_the_file_state() {
        let st = &crate::settings_file::SettingsStore::memory();
        let path = loop_file("plays");
        let mut pl = Plugins::default();
        pl.available = crate::plugins::builtin();
        pl.set_rate(48000);
        let index = pl.index_of(LEMON_SLICER).expect("built-in Lemon Slicer");
        assert!(crate::project_plugins::is_builtin(&pl.available[index].desc));
        let fl = state_for(&path);
        pl.create_with(1, index, Some(&plugin_state(st, &fl))).unwrap();
        assert_eq!(pl.slicer_file_state(1), Some(fl.clone()));
        let inst = pl.hosted[&1].plugin.clone() as SharedInstrument;
        let mut i = inst.lock().unwrap();
        let peak = |i: &mut dyn os_engine::Instrument| {
            let (mut l, mut r) = (vec![0.0f32; 2400], vec![0.0f32; 2400]);
            i.process(48000, &mut l, &mut r);
            l.iter().fold(0.0f32, |m, v| m.max(v.abs()))
        };
        i.note_on(0, 60, 100.0 / 128.0);
        assert!(peak(&mut *i) < 1e-4, "slice 1 is silence");
        i.note_off(0, 60);
        i.note_on(0, 61, 100.0 / 128.0);
        let p = peak(&mut *i);
        assert!(p > 0.2 && p < 1.0, "slice 2 sounds: {p}");
        i.note_off(0, 61);
        drop(i);
        assert_eq!(pl.slicer_file_state(99), None);
        let _ = std::fs::remove_file(path);
    }

    /// `OS_SLICER_FLP=<a.flp>;<b.flp> cargo test -p os-app corpus_slicer -- --ignored`: every Fruity Slicer channel of
    /// the projects comes back byte for byte through Lemon Slicer, and its loop is found.
    #[test]
    #[ignore]
    fn corpus_slicer_states_round_trip() {
        let st = &crate::settings_file::SettingsStore::memory();
        let Ok(list) = std::env::var("OS_SLICER_FLP") else { return };
        let mut seen = 0;
        for f in list.split(';').filter(|f| !f.is_empty()) {
            let Ok(p) = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(f).unwrap()).unwrap()) else { eprintln!("{f}: not read"); continue };
            for ch in p.channels.iter().filter(|c| is_slicer(c)) {
                let fl = ch.plugin_state.clone().unwrap_or_default();
                let st = State::parse(&fl).unwrap_or_else(|| panic!("{f}: {:?} does not parse", ch.name));
                assert_eq!(st.to_bytes(), fl, "{f}: {:?}", ch.name);
                assert_eq!(lemon_slicer::params::fl_from_plugin_state(&plugin_state(&crate::settings_file::SettingsStore::memory(), &fl)), Some(fl.clone()));
                let sample = crate::plugins::resolve_sample(&crate::settings_file::SettingsStore::memory(), &st.path());
                eprintln!("{f}: {:?} ({}), {} slices, {:.3} beats at {:.3} BPM, {sample} found: {}", ch.name, ch.plugin.as_deref().unwrap_or(""), st.slices.len(), st.beats(), st.bpm(), std::path::Path::new(&sample).exists());
                seen += 1;
            }
        }
        assert!(seen > 0);
    }
}
