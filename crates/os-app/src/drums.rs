// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Drum Sampler in Lemon Studio: compiled in like Citrus, it plays every FPC channel of a project (started from
//! the state the file holds, its layers' samples opened from the stored paths) and every one added from the picker;
//! saving writes the state it plays back into the channel byte for byte, so the file keeps its own plugin with its
//! pads and samples.
//!
// spec: plugins/lemon-drum-sampler.md §4 (app integration); plugins/fpc.md §1

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

impl crate::app::App {
    /// Hosts Lemon Drum Sampler on every FPC channel without an instance, started from the channel's saved state (a new
    /// instance's when it has none). A row waiting for the picker's own instance is left to it; a row whose instance
    /// failed is not tried again.
    pub(crate) fn sync_drums(&mut self) {
        if crate::project_plugins::native_first() {
            return;
        }
        let Some(index) = self.plugins.index_of(LEMON_DRUMS) else { return };
        let pending = self.state.pending_plugin.map(|(r, _)| r);
        let rows: Vec<usize> = (0..self.state.project.channels.len())
            .filter(|&r| is_fpc(&self.state.project.channels[r]))
            .filter(|&r| !self.state.hosted_rows.contains(&r) && !self.wrap_failed.contains(&r) && Some(r) != pending)
            .collect();
        if rows.is_empty() {
            return;
        }
        let mut added = false;
        for row in rows {
            if self.past_deadline() {
                break;
            }
            let fl = self.state.project.channels[row].plugin_state.clone().unwrap_or_default();
            match self.plugins.create_with(row, index, Some(&plugin_state(&fl))) {
                Ok(_) => {
                    self.state.hosted_rows.push(row);
                    added = true;
                }
                Err(e) => {
                    self.wrap_failed.push(row);
                    self.state.message = Some(format!("{LEMON_DRUMS}: {e}"));
                }
            }
        }
        if added {
            self.transport.set_instruments(self.plugins.instruments());
            self.state.piano.dirty = true;
        }
        self.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use lemon_drum_sampler::state::State;
    use os_engine::SharedInstrument;

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

    /// An FPC state with `path` on pad 2 (key 36).
    fn state_for(path: &std::path::Path) -> Vec<u8> {
        let mut s = State::default();
        s.set_sample(1, &path.to_string_lossy());
        s.to_bytes()
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

    /// An FPC channel added in the app saves a new instance's state; one with a sample saves its bytes, and the
    /// reopened project hosts it again from them.
    #[test]
    fn fpc_channels_save_and_reopen() {
        let file = std::env::temp_dir().join(format!("openstudio-drums-{}.flp", std::process::id()));
        let path = wav("reopen");
        let mut a = App::new(None);
        a.state.project = crate::app::test_project();
        a.state.file = None;
        a.state.path = None;
        a.plugins.available = crate::plugins::builtin();
        let row = crate::app::add_plugin_channel(&mut a.state.project, LEMON_DRUMS);
        let ch = &a.state.project.channels[row];
        assert_eq!((ch.kind, ch.plugin.as_deref()), (ChannelKind::Plugin, Some(FILE_NAME)));
        assert_eq!(crate::project_plugins::plugin_title(ch).as_deref(), Some(LEMON_DRUMS));
        a.sync_drums();
        assert!(a.state.hosted_rows.contains(&row));
        a.save_to(file.clone());
        let saved = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&file).unwrap()).unwrap()).unwrap();
        assert_eq!(saved.channels[row].plugin_state.as_deref(), Some(lemon_drum_sampler::params::default_state().as_slice()));

        let fl = state_for(&path);
        assert!(a.plugins.set_state(row, &plugin_state(&fl)));
        a.save_to(file.clone());
        let saved = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&file).unwrap()).unwrap()).unwrap();
        assert_eq!(saved.channels[row].plugin_state.as_deref(), Some(fl.as_slice()));
        let mut b = App::new(None);
        b.plugins.available = crate::plugins::builtin();
        b.open(file.clone());
        b.sync_drums();
        assert!(b.state.hosted_rows.contains(&row), "{:?}", b.state.message);
        assert_eq!(b.plugins.drums_file_state(row), Some(fl.clone()));
        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_file(path);
    }

    /// FL's own Breakbeat template (when FL 2026 is installed): its two FPC channels open with their pads, every
    /// sample they name opens, a pad plays, and a save writes their bytes back unchanged.
    #[test]
    fn the_breakbeat_template_plays_its_kits() {
        let path = std::path::PathBuf::from(r"C:\Program Files\Image-Line\FL Studio 2026\Data\Templates\Advanced\Electronic\Breakbeat.flp");
        let Some(project) = std::fs::read(&path).ok().and_then(|b| os_flp::FlpFile::parse(&b).ok()).and_then(|f| os_flp::project::load(&f).ok()) else {
            return;
        };
        let rows: Vec<usize> = (0..project.channels.len()).filter(|&r| is_fpc(&project.channels[r])).collect();
        assert_eq!(rows.len(), 2, "two FPC channels");
        let mut a = App::new(None);
        a.plugins.available = crate::plugins::builtin();
        a.open(path.clone());
        a.sync_drums();
        for &r in &rows {
            let fl = project.channels[r].plugin_state.clone().expect("an FPC state");
            assert_eq!(a.plugins.drums_file_state(r).as_ref(), Some(&fl), "channel {r}");
            let st = State::parse(&fl).expect("a readable state");
            let pads: Vec<_> = st.pads().into_iter().filter(|p| p.sounding().next().is_some()).collect();
            assert!(pads.len() >= 8, "channel {r}: {} pads with samples", pads.len());
            for p in &pads {
                for (_, l) in p.sounding() {
                    let file = crate::plugins::resolve_sample(&l.path);
                    assert!(std::path::Path::new(&file).is_file(), "channel {r}: {} -> {file}", l.path);
                }
            }
            // The first pad whose samples open (Lemon Studio reads none of a WavPack file whose first block is empty,
            // like the Breakbeat kick).
            let decodes = |p: &&lemon_drum_sampler::state::Pad| {
                p.sounding().all(|(_, l)| os_engine::sample::load_shared(std::path::Path::new(&crate::plugins::resolve_sample(&l.path))).is_ok())
            };
            let pad = pads.iter().find(decodes).unwrap_or_else(|| panic!("channel {r}: a pad whose samples open"));
            let inst = a.plugins.hosted[&r].plugin.clone() as SharedInstrument;
            let mut i = inst.lock().unwrap();
            let key = u8::try_from(pad.key).unwrap();
            i.note_on(0, key, 100.0 / 128.0);
            assert!(peak(&mut *i) > 0.01, "channel {r}: pad {:?} on key {key} sounds", pad.name);
        }
        let out = std::env::temp_dir().join(format!("openstudio-drums-breakbeat-{}.flp", std::process::id()));
        let before = a.state.project.clone();
        a.save_to(out.clone());
        let back = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&out).unwrap()).unwrap()).unwrap();
        let _ = std::fs::remove_file(&out);
        for &r in &rows {
            assert_eq!(back.channels[r], before.channels[r], "channel {r}");
        }
    }
}
