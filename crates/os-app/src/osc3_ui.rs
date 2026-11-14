// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::osc3` (step 9: the session's side stays there).

#![allow(unused_imports)]
#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use std::time::{Duration, Instant};
use os_model::{Channel, ChannelKind, Project};
use crate::plugins::{Plugins, FX_KEY};
#[allow(unused_imports)]
use crate::osc3::*;

impl crate::app::App {
    /// Hosts Lemon Trio on every 3x Osc channel without an instance, started from the channel's saved state (a new
    /// instance's state when the channel has none). A row waiting for the picker's own instance is left to it; a row
    /// whose instance failed is not tried again.
    pub fn sync_osc3(&mut self) {
        self.host(sync_osc3);
    }

    /// Every half second: a sample a hosted Lemon Trio loaded for its custom shape (its own file dialog) becomes its
    /// channel's (`follow_sample`). The instances the audio thread is rendering are looked at next time.
    pub fn follow_osc3_samples(&mut self) {
        let now = Instant::now();
        if self.osc3_look.is_some_and(|t| now.duration_since(t) < LOOK_EVERY) {
            return;
        }
        self.osc3_look = Some(now);
        let mut changed = false;
        for row in self.state.session.hosted_rows.clone() {
            let plugins = &self.state.session.plugins;
            changed |= self.state.session.project.write_if(|p| follow_sample(&self.state.session.settings, p, plugins, row, false));
        }
        if changed {
            self.invalidate();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use lemon_3x_osc::params::{self as op, default_state, field, set_field};
    use os_engine::SharedInstrument;

    fn instance(pl: &Plugins, row: usize) -> SharedInstrument {
        pl.hosted[&row].plugin.clone() as SharedInstrument
    }

    /// A 3x Osc channel added in the app is saved as the file's 3x Osc with its settings; the reopened project hosts it
    /// again, and a file's 3x Osc channel is hosted from its own state and written back unchanged.
    #[test]
    fn osc3_channels_save_and_reopen() {
        let path = std::env::temp_dir().join(format!("openstudio-osc3-{}.flp", std::process::id()));
        let mut a = App::new(None);
        a.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        a.state.session.file = None;
        a.state.session.path = None;
        a.state.session.plugins.available = crate::plugins::builtin();
        // Added as the picker adds it: a plugin channel storing the file id, shown with the built-in name.
        let row = crate::projects::add_plugin_channel(a.state.session.project.get_mut(), LEMON_TRIO);
        let ch = &a.state.session.project.channels[row];
        assert_eq!((ch.kind, ch.plugin.as_deref()), (ChannelKind::Plugin, Some(FILE_NAME)));
        assert_eq!(crate::project_plugins::plugin_title(ch).as_deref(), Some(LEMON_TRIO));
        // Hosted with a new instance's settings, then one is edited.
        a.sync_osc3();
        assert!(a.state.session.hosted_rows.contains(&row));
        let mut fl = a.state.session.plugins.osc3_file_state(row).unwrap();
        assert_eq!(fl, default_state());
        set_field(&mut fl, op::osc(1) + op::COARSE + 1, 7);
        assert!(a.state.session.plugins.set_state(row, &op::plugin_state_from_fl(&fl)));
        a.save_to(path.clone());
        assert!(a.state.message.as_deref().is_some_and(|m| m.starts_with("Saved") && !m.contains("not stored")), "{:?}", a.state.message);

        let file = os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap();
        let saved = os_flp::project::load(&file).unwrap();
        let ch = &saved.channels[row];
        assert_eq!((ch.kind, ch.plugin.as_deref()), (ChannelKind::Plugin, Some(FILE_NAME)));
        assert_eq!(ch.plugin_state.as_deref(), Some(fl.as_slice()));
        assert_eq!(field(ch.plugin_state.as_ref().unwrap(), op::osc(1) + op::COARSE + 1), 7);

        // Reopened: hosted again from the file's state.
        let mut b = App::new(None);
        b.state.session.plugins.available = crate::plugins::builtin();
        b.open(path.clone());
        b.sync_osc3();
        assert!(b.state.session.hosted_rows.contains(&row), "{:?}", b.state.message);
        assert_eq!(b.state.session.plugins.osc3_file_state(row), Some(fl.clone()));
        // Saved again unedited: the channel's state bytes are unchanged.
        b.save_to(path.clone());
        let again = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(again.channels[row], saved.channels[row]);
    }

    /// A channel with an older version's 3x Osc state (no records or filter of its own) is hosted with the channel's
    /// envelope records and filter, as FL plays it; a new channel's volume record keeps the plugin's default (spec:
    /// plugins/lemon-3x-osc.md §6.1).
    #[test]
    fn an_older_osc3_state_takes_the_channels_voicing() {
        let mut a = App::new(None);
        a.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        a.state.session.plugins.available = crate::plugins::builtin();
        let row = crate::projects::add_plugin_channel(a.state.session.project.get_mut(), LEMON_TRIO);
        let mut old = default_state()[..92].to_vec();
        set_field(&mut old, 0, 12);
        set_field(&mut old, op::osc(0) + op::COARSE + 1, 5);
        let ch = &mut a.state.session.project.get_mut().channels[row];
        ch.plugin_state = Some(old.clone());
        ch.envelopes[os_model::Envelope::PITCH].enabled = true;
        ch.envelopes[os_model::Envelope::PITCH].amount = 1200;
        (ch.sound.cutoff, ch.sound.resonance, ch.sound.filter_type) = (64, 32, 3);
        a.sync_osc3();
        let st = a.state.session.plugins.osc3_file_state(row).expect("hosted");
        assert_eq!(st[4..92], old[4..]);
        let pitch = op::rec(op::T_PITCH) + 1;
        assert_eq!((field(&st, pitch + 1), field(&st, pitch + 8)), (-1, 1200));
        let vol = op::rec(op::T_VOL) + 1;
        assert_eq!(st[4 * vol..4 * (vol + 17)], default_state()[4 * vol..4 * (vol + 17)]);
        assert_eq!([op::CUTOFF, op::RESONANCE, op::FILTER].map(|k| field(&st, k + 1)), [64, 32, 3]);
    }

    /// A 3x Osc channel's sample is the custom shape's: Lemon Trio starts with it and plays it as one period; a sample
    /// the plugin loads itself becomes the channel's (path and name) and is saved as the channel's sample, the 3x Osc
    /// state staying 460 bytes (spec: plugins/3x-osc.md §1, plugins/lemon-3x-osc.md §6).
    #[test]
    fn osc3_custom_shape_plays_the_channel_sample() {
        let dir = std::env::temp_dir().join(format!("openstudio-osc3-custom-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // One period of a square, and of a saw.
        let wav = |name: &str, pts: &[f32]| {
            let p = dir.join(format!("{name}.wav"));
            let both: Vec<f32> = pts.iter().flat_map(|&v| [v, v]).collect();
            os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&p).unwrap(), 44100, &both).unwrap();
            p.to_string_lossy().into_owned()
        };
        let square = wav("Square cycle", &[0.9, 0.9, 0.9, 0.9, -0.9, -0.9, -0.9, -0.9]);
        let saw = wav("Saw cycle", &[-0.75, -0.5, -0.25, 0.0, 0.25, 0.5, 0.75]);
        let mut a = App::new(None);
        a.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        a.state.session.file = None;
        a.state.session.path = None;
        a.state.session.plugins.available = crate::plugins::builtin();
        a.state.session.plugins.set_rate(48000);
        let row = crate::projects::add_plugin_channel(a.state.session.project.get_mut(), LEMON_TRIO);
        let mut fl = default_state();
        set_field(&mut fl, op::osc(0) + op::SHAPE + 1, 6); // osc 1: custom
        set_field(&mut fl, op::MIX2 + 1, 0); // osc 2 and 3 silent
        set_field(&mut fl, op::MIX3 + 1, 0);
        let ch = &mut a.state.session.project.get_mut().channels[row];
        (ch.plugin_state, ch.sample_path) = (Some(fl.clone()), Some(square.clone()));
        a.sync_osc3();
        assert!(a.state.session.hosted_rows.contains(&row), "{:?}", a.state.message);
        // The plugin opens the file itself: placeholders resolved.
        assert_eq!(a.state.session.plugins.osc3_sample(row, true), Some(crate::plugins::resolve_sample(&crate::settings_file::store(), &square)));
        assert_eq!(a.state.session.plugins.osc3_file_state(row), Some(fl.clone()));
        // It sounds: the square at 880 Hz (key 69), one period per cycle.
        let inst = instance(&a.state.session.plugins, row);
        let mut i = inst.lock().unwrap();
        i.note_on(0, 69, 100.0 / 127.0);
        let (mut l, mut r) = (vec![0.0f32; 4800], vec![0.0f32; 4800]);
        i.process(48000, &mut l, &mut r);
        let peak = l.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.5 && peak < 0.9, "peak {peak}");
        let crossings = l[480..].windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        assert!((77..=89).contains(&crossings), "{crossings} rising crossings in 4320 samples");
        i.note_off(0, 69);
        drop(i);
        // Nothing to follow while the channel names the plugin's sample.
        a.follow_osc3_samples();
        assert_eq!(a.state.session.project.channels[row].name.as_deref(), Some(LEMON_TRIO));
        // A sample the plugin loads itself (its file dialog): the channel takes its path and its name.
        assert!(a.state.session.plugins.load_sample(row, &saw));
        a.osc3_look = None;
        a.follow_osc3_samples();
        let ch = &a.state.session.project.channels[row];
        assert_eq!((ch.sample_path.as_deref(), ch.name.as_deref()), (Some(saw.as_str()), Some("Saw cycle")));
        // Saved as the channel's sample; reopened, the plugin starts with it.
        let path = dir.join("custom.flp");
        a.save_to(path.clone());
        assert!(a.state.message.as_deref().is_some_and(|m| m.starts_with("Saved")), "{:?}", a.state.message);
        let saved = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        let ch = &saved.channels[row];
        assert_eq!((ch.plugin.as_deref(), ch.sample_path.as_deref()), (Some(FILE_NAME), Some(saw.as_str())));
        assert_eq!(ch.plugin_state.as_ref().map(Vec::len), Some(op::STATE_LEN));
        let mut b = App::new(None);
        b.state.session.plugins.available = crate::plugins::builtin();
        b.open(path.clone());
        b.sync_osc3();
        assert_eq!(b.state.session.plugins.osc3_sample(row, true), Some(crate::plugins::resolve_sample(&crate::settings_file::store(), &saw)));
        drop(b);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The rack's Add one ▸ and Insert ▸ menus add a Lemon Trio channel and ask for the built-in plugin (the
    /// instance itself is made on the next tick, which also opens its window: done by hand here).
    #[test]
    fn plugin_menus_add_lemon_3x_osc_channels() {
        use crate::plugin_menu::Target;
        let mut a = App::new(None);
        a.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        a.state.session.plugins.available = crate::plugins::builtin();
        a.state.plugdb.rebuild(&a.state.session.plugins);
        let index = a.state.session.plugins.index_of(LEMON_TRIO).unwrap();
        let pick = |a: &App, target| crate::plugin_menu::items(&a.state, target, true).into_iter().find(|it| it.label == LEMON_TRIO).map(|it| it.cmd);
        let n = a.state.session.project.channels.len();
        assert!(crate::rack::add_one_items(&a.state).iter().any(|it| it.label == LEMON_TRIO), "Add one lists Lemon Trio");
        let cmd = pick(&a, Target::Gen(None)).expect("the generator menu lists Lemon Trio");
        crate::playlist::run(&mut a, cmd);
        assert_eq!(a.state.session.project.channels.len(), n + 1);
        assert_eq!(a.state.session.pending_plugin, Some((n, index)));
        assert!(is_osc3(&a.state.session.project.channels[n]));
        assert_eq!(a.state.session.project.channels[n].name.as_deref(), Some(LEMON_TRIO));
        assert_eq!(a.state.session.plugins.available[index].desc.name, LEMON_TRIO);
        a.state.session.pending_plugin = None;
        a.sync_osc3();
        assert!(a.state.session.hosted_rows.contains(&n));
        assert_eq!(a.state.session.plugins.osc3_file_state(n), Some(default_state()));
        // Insert ▸ on the first row: right below it.
        let cmd = pick(&a, Target::Gen(Some(0))).unwrap();
        crate::playlist::run(&mut a, cmd);
        assert_eq!(a.state.session.pending_plugin, Some((1, index)));
        assert!(is_osc3(&a.state.session.project.channels[1]));
        assert_eq!(a.state.selected_channel, 1);
        assert!(a.state.session.hosted_rows.contains(&(n + 1)), "the channel hosted before moved down one row");
    }

    /// Replace ▸ (channel menu): the channel keeps its row, notes, mixer track and levels, and takes the new generator.
    /// A sample the channel had goes: it would be the new Lemon Trio's custom shape sample.
    #[test]
    fn replace_puts_another_generator_on_a_channel() {
        use crate::plugin_menu::Target;
        let mut a = App::new(None);
        a.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        a.state.session.project.get_mut().channels[2].sample_path = Some("C:/Samples/Kick.wav".into());
        a.state.session.plugins.available = crate::plugins::builtin();
        a.state.plugdb.rebuild(&a.state.session.plugins);
        let index = a.state.session.plugins.index_of(LEMON_TRIO).unwrap();
        let (n, before) = (a.state.session.project.channels.len(), a.state.session.project.channels[2].clone());
        let cmd = crate::plugin_menu::items(&a.state, Target::Replace(2), true).into_iter().find(|it| it.label == LEMON_TRIO).map(|it| it.cmd).unwrap();
        crate::playlist::run(&mut a, cmd);
        let after = &a.state.session.project.channels[2];
        assert_eq!(a.state.session.project.channels.len(), n, "no channel added");
        assert!(is_osc3(after));
        assert_eq!(after.name.as_deref(), Some(LEMON_TRIO));
        assert_eq!((after.index, after.mixer_track, after.volume, after.pan), (before.index, before.mixer_track, before.volume, before.pan));
        assert_eq!(after.sample_path, None, "the new instance starts without a sample");
        assert_eq!(a.state.session.pending_plugin, Some((2, index)));
        assert!(!a.state.session.hosted_rows.contains(&2), "the old instance is gone");
    }

    /// Corpus check (OPENSTUDIO_FLP_CORPUS): every 3x Osc channel of every file is hosted by Lemon Trio from its
    /// saved state, gives the same bytes back, and an unedited save leaves the whole project as it was.
    #[test]
    fn corpus_osc3_channels_survive_hosting_and_saving() {
        let Some(list) = std::env::var_os("OPENSTUDIO_FLP_CORPUS") else { return };
        let text = std::fs::read_to_string(list).unwrap();
        let (mut files, mut channels, mut older) = (0, 0, 0);
        let out = std::env::temp_dir().join(format!("openstudio-osc3-corpus-{}.flp", std::process::id()));
        for path in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let Some(project) = std::fs::read(path).ok().and_then(|b| os_flp::FlpFile::parse(&b).ok()).and_then(|f| os_flp::project::load(&f).ok())
            else {
                continue;
            };
            let rows: Vec<usize> = (0..project.channels.len()).filter(|&r| is_osc3(&project.channels[r])).collect();
            if rows.is_empty() {
                continue;
            }
            let mut a = App::new(None);
            a.state.session.plugins.available = crate::plugins::builtin();
            a.open(std::path::PathBuf::from(path));
            a.sync_osc3();
            for &r in &rows {
                let st = project.channels[r].plugin_state.clone().expect("3x Osc state");
                let hosted = a.state.session.plugins.osc3_file_state(r).expect("a Lemon Trio");
                if st.len() < op::STATE_LEN {
                    // An older version's state (projects older than FL 21): its fields kept, the channel's records
                    // and filter added, the rest the defaults, at the current version.
                    assert_eq!((hosted.len(), op::field(&hosted, 0)), (op::STATE_LEN, op::STATE_VERSION), "{path}: channel {r}");
                    assert_eq!(hosted[4..st.len()], st[4..], "{path}: channel {r}");
                    assert_eq!(hosted, channel_voicing(&project.channels[r], &st), "{path}: channel {r}");
                    older += 1;
                } else {
                    assert_eq!(st.len(), op::STATE_LEN, "{path}: channel {r}");
                    assert_eq!(hosted, st, "{path}: channel {r}");
                }
                channels += 1;
            }
            let before = a.state.session.project.clone();
            a.save_to(out.clone());
            assert!(a.state.message.as_deref().is_some_and(|m| m.starts_with("Saved")), "{path}: {:?}", a.state.message);
            let back = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&out).unwrap()).unwrap()).unwrap();
            for &r in &rows {
                let (mut x, mut y) = (back.channels[r].clone(), before.channels[r].clone());
                if y.plugin_state.as_ref().is_some_and(|s| s.len() < op::STATE_LEN) {
                    (x.plugin_state, y.plugin_state) = (None, None);
                }
                assert_eq!(x, y, "{path}: channel {r}");
            }
            files += 1;
        }
        let _ = std::fs::remove_file(&out);
        eprintln!("3x Osc: {channels} channels ({older} with an older version's state) in {files} files");
        assert!(files > 0, "no corpus file with a 3x Osc channel");
    }
}
