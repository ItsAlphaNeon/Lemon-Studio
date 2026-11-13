// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::drums` (step 9: the session's side stays there).

#![allow(unused_imports)]
#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use os_model::{Channel, ChannelKind, Project};
use crate::plugins::{Plugins, FX_KEY};
#[allow(unused_imports)]
use crate::drums::*;

impl crate::app::App {
    /// Hosts Lemon Drum Sampler on every FPC channel without an instance, started from the channel's saved state (a new
    /// instance's when it has none). A row waiting for the picker's own instance is left to it; a row whose instance
    /// failed is not tried again.
    pub(crate) fn sync_drums(&mut self) {
        self.host(sync_drums);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use lemon_drum_sampler::state::State;
    use os_engine::SharedInstrument;

    fn peak(i: &mut dyn os_engine::Instrument) -> f32 {
        let (mut l, mut r) = (vec![0.0f32; 2400], vec![0.0f32; 2400]);
        i.process(48000, &mut l, &mut r);
        l.iter().chain(&r).fold(0.0f32, |m, v| m.max(v.abs()))
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

    /// An FPC state with `path` on pad 2 (key 36).
    fn state_for(path: &std::path::Path) -> Vec<u8> {
        let mut s = State::default();
        s.set_sample(1, &path.to_string_lossy());
        s.to_bytes()
    }

    /// An FPC channel added in the app saves a new instance's state; one with a sample saves its bytes, and the
    /// reopened project hosts it again from them.
    #[test]
    fn fpc_channels_save_and_reopen() {
        let file = std::env::temp_dir().join(format!("openstudio-drums-{}.flp", std::process::id()));
        let path = wav("reopen");
        let mut a = App::new(None);
        a.state.session.project = crate::locked::Locked::new(crate::app::test_project());
        a.state.session.file = None;
        a.state.session.path = None;
        a.state.session.plugins.available = crate::plugins::builtin();
        let row = crate::app::add_plugin_channel(a.state.session.project.get_mut(), LEMON_DRUMS);
        let ch = &a.state.session.project.channels[row];
        assert_eq!((ch.kind, ch.plugin.as_deref()), (ChannelKind::Plugin, Some(FILE_NAME)));
        assert_eq!(crate::project_plugins::plugin_title(ch).as_deref(), Some(LEMON_DRUMS));
        a.sync_drums();
        assert!(a.state.session.hosted_rows.contains(&row));
        a.save_to(file.clone());
        let saved = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&file).unwrap()).unwrap()).unwrap();
        assert_eq!(saved.channels[row].plugin_state.as_deref(), Some(lemon_drum_sampler::params::default_state().as_slice()));

        let fl = state_for(&path);
        assert!(a.state.session.plugins.set_state(row, &plugin_state(&fl)));
        a.save_to(file.clone());
        let saved = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&file).unwrap()).unwrap()).unwrap();
        assert_eq!(saved.channels[row].plugin_state.as_deref(), Some(fl.as_slice()));
        let mut b = App::new(None);
        b.state.session.plugins.available = crate::plugins::builtin();
        b.open(file.clone());
        b.sync_drums();
        assert!(b.state.session.hosted_rows.contains(&row), "{:?}", b.state.message);
        assert_eq!(b.state.session.plugins.drums_file_state(row), Some(fl.clone()));
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
        a.state.session.plugins.available = crate::plugins::builtin();
        a.open(path.clone());
        a.sync_drums();
        for &r in &rows {
            let fl = project.channels[r].plugin_state.clone().expect("an FPC state");
            assert_eq!(a.state.session.plugins.drums_file_state(r).as_ref(), Some(&fl), "channel {r}");
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
            let inst = a.state.session.plugins.hosted[&r].plugin.clone() as SharedInstrument;
            let mut i = inst.lock().unwrap();
            let key = u8::try_from(pad.key).unwrap();
            i.note_on(0, key, 100.0 / 128.0);
            assert!(peak(&mut *i) > 0.01, "channel {r}: pad {:?} on key {key} sounds", pad.name);
        }
        let out = std::env::temp_dir().join(format!("openstudio-drums-breakbeat-{}.flp", std::process::id()));
        let before = a.state.session.project.clone();
        a.save_to(out.clone());
        let back = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&out).unwrap()).unwrap()).unwrap();
        let _ = std::fs::remove_file(&out);
        for &r in &rows {
            assert_eq!(back.channels[r], before.channels[r], "channel {r}");
        }
    }
}
