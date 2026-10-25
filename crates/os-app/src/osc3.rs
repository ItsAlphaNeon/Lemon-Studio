// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Trio in Lemon Studio: compiled in like Lemon Sampler, it plays every 3x Osc channel of a project (started from
//! the state the file holds) and every one added from the picker, the browser or the channel rack; saving writes its
//! settings back as the file's 3x Osc state, so the file keeps its own plugin. The channel's sample is the custom
//! shape's: the plugin starts with it, and one the plugin loads becomes the channel's.
//!
// spec: plugins/lemon-3x-osc.md §6 (app integration), plugins/3x-osc.md §1 (saved state, custom shape)

use std::time::{Duration, Instant};

use os_model::{Channel, ChannelKind, Project};

use crate::plugins::{Plugins, FX_KEY};

/// The built-in plugin's name (picker, window title).
pub const LEMON_TRIO: &str = "Lemon Trio";
/// The plugin id project files store for it (event 201).
pub const FILE_NAME: &str = "3x Osc";

/// Whether a channel holds the file's 3x Osc.
pub fn is_osc3(ch: &Channel) -> bool {
    ch.kind == ChannelKind::Plugin && ch.plugin.as_deref() == Some(FILE_NAME)
}

/// The plugin id a new channel of picker plugin `name` stores (the built-in recreations store the original's id).
pub fn file_name(name: &str) -> &str {
    match name {
        LEMON_TRIO => FILE_NAME,
        crate::citrus::CITRUS => crate::citrus::FILE_NAME,
        _ => name,
    }
}

/// What the UI calls a channel's plugin `name` (the file id of a built-in recreation shows as the built-in plugin).
pub fn label(name: &str) -> &str {
    match name {
        FILE_NAME => LEMON_TRIO,
        crate::citrus::FILE_NAME => crate::citrus::CITRUS,
        _ => name,
    }
}

impl Plugins {
    /// The project-file state of the Lemon Trio on rack row `row` (its plugin state converted back), `None` when the
    /// row hosts something else.
    pub fn osc3_file_state(&self, row: usize) -> Option<Vec<u8>> {
        let h = self.hosted.get(&row)?;
        let p = h.plugin.lock().ok()?;
        if p.desc().name != LEMON_TRIO || !crate::project_plugins::is_builtin(p.desc()) {
            return None;
        }
        lemon_3x_osc::params::fl_from_plugin_state(&p.save_state()?)
    }

    /// The custom shape's sample file of the Lemon Trio on rack row `row` ("" = none); `None` when the row hosts
    /// something else, or the instance is busy and `wait` is false.
    pub fn osc3_sample(&self, row: usize, wait: bool) -> Option<String> {
        let h = self.hosted.get(&row)?;
        let p = if wait { h.plugin.lock().ok()? } else { h.plugin.try_lock().ok()? };
        if p.desc().name != LEMON_TRIO || !crate::project_plugins::is_builtin(p.desc()) {
            return None;
        }
        crate::plugins::sample_path_of(&p.save_state()?)
    }
}

/// Before a save: every hosted Lemon Trio writes its settings into its channel as the file's 3x Osc, and its custom
/// shape's sample as the channel's.
pub fn store(proj: &mut Project, plugins: &Plugins) {
    for row in plugins.hosted.range(..FX_KEY).map(|(&k, _)| k).collect::<Vec<_>>() {
        let Some(st) = plugins.osc3_file_state(row) else { continue };
        if let Some(ch) = proj.channels.get_mut(row).filter(|c| c.kind == ChannelKind::Plugin) {
            ch.plugin = Some(FILE_NAME.into());
            ch.plugin_state = Some(st);
        }
        follow_sample(proj, plugins, row, true);
    }
}

/// How often the app looks for a sample a Lemon Trio loaded by itself.
const LOOK_EVERY: Duration = Duration::from_millis(500);

/// The custom shape's sample of the Lemon Trio on 3x Osc channel `row` becomes the channel's sample (event 196) and,
/// while Auto name channels is on, its name, unless the channel names that file already (spec: plugins/3x-osc.md §1).
/// Whether the channel changed. `wait`: lock an instance the audio thread is rendering (a save) or leave it.
fn follow_sample(proj: &mut Project, plugins: &Plugins, row: usize, wait: bool) -> bool {
    if !proj.channels.get(row).is_some_and(is_osc3) {
        return false;
    }
    let Some(p) = plugins.osc3_sample(row, wait).filter(|p| !p.is_empty()) else { return false };
    let ch = &mut proj.channels[row];
    // A placeholder path (`%FLStudioFactoryData%`...) that still names the same file is kept as the project wrote it.
    if ch.sample_path.as_deref().is_some_and(|old| crate::plugins::resolve_sample(old) == p) {
        return false;
    }
    crate::rack::auto_name(ch, std::path::Path::new(&p));
    ch.sample_path = Some(p);
    true
}

impl crate::app::App {
    /// Hosts Lemon Trio on every 3x Osc channel without an instance, started from the channel's saved state (a new
    /// instance's state when the channel has none). A row waiting for the picker's own instance is left to it; a row
    /// whose instance failed is not tried again.
    pub(crate) fn sync_osc3(&mut self) {
        // The installed 3x Osc plays these channels instead (`host_native`).
        if crate::project_plugins::native_first() {
            return;
        }
        let Some(index) = self.plugins.index_of(LEMON_TRIO) else { return };
        let pending = self.state.pending_plugin.map(|(r, _)| r);
        let rows: Vec<usize> = (0..self.state.project.channels.len())
            .filter(|&r| is_osc3(&self.state.project.channels[r]))
            .filter(|&r| !self.state.hosted_rows.contains(&r) && !self.wrap_failed.contains(&r) && Some(r) != pending)
            .collect();
        if rows.is_empty() {
            return;
        }
        let mut added = false;
        for row in rows {
            let ch = &self.state.project.channels[row];
            let fl = ch.plugin_state.clone().unwrap_or_else(lemon_3x_osc::params::default_state);
            let mut st = lemon_3x_osc::params::plugin_state_from_fl(&fl);
            // The channel's sample is the custom shape's (spec: plugins/3x-osc.md §1); the plugin opens the file
            // directly, so factory placeholders become real paths.
            let sample = crate::plugins::resolve_sample(ch.sample_path.as_deref().unwrap_or(""));
            if !sample.is_empty()
                && let Some(b) = crate::plugins::with_sample_path(&st, &sample)
            {
                st = b;
            }
            match self.plugins.create_with(row, index, Some(&st)) {
                Ok(_) => {
                    self.state.hosted_rows.push(row);
                    added = true;
                }
                Err(e) => {
                    self.wrap_failed.push(row);
                    self.state.message = Some(format!("{LEMON_TRIO}: {e}"));
                }
            }
        }
        if added {
            self.transport.set_instruments(self.plugins.instruments());
            self.state.piano.dirty = true;
        }
        self.invalidate();
    }

    /// Every half second: a sample a hosted Lemon Trio loaded for its custom shape (its own file dialog) becomes its
    /// channel's (`follow_sample`). The instances the audio thread is rendering are looked at next time.
    pub(crate) fn follow_osc3_samples(&mut self) {
        let now = Instant::now();
        if self.osc3_look.is_some_and(|t| now.duration_since(t) < LOOK_EVERY) {
            return;
        }
        self.osc3_look = Some(now);
        let mut changed = false;
        for row in self.state.hosted_rows.clone() {
            changed |= follow_sample(&mut self.state.project, &self.plugins, row, false);
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

    /// The built-in Lemon Trio starts from a project file's 3x Osc state, plays it, and gives the same bytes back
    /// (spec: plugins/lemon-3x-osc.md §6).
    #[test]
    fn builtin_lemon_3x_osc_takes_and_gives_the_file_state() {
        let mut pl = Plugins::default();
        pl.available = crate::plugins::builtin();
        let index = pl.index_of(LEMON_TRIO).expect("built-in Lemon Trio");
        assert!(crate::project_plugins::is_builtin(&pl.available[index].desc));
        let mut fl = default_state();
        set_field(&mut fl, op::osc(0) + op::SHAPE + 1, 3); // osc 1: saw
        set_field(&mut fl, op::MIX2 + 1, 0); // osc 2 and 3 silent
        set_field(&mut fl, op::MIX3 + 1, 0);
        set_field(&mut fl, 109, 77); // a field nothing uses: kept
        pl.create_with(2, index, Some(&op::plugin_state_from_fl(&fl))).unwrap();
        assert_eq!(pl.osc3_file_state(2), Some(fl.clone()));
        // It sounds: a saw at 880 Hz (pitch formula of 3x-osc.md: key 69 with coarse 0), counted at the device rate
        // the instance runs at (44.1 or 48 kHz).
        let inst = instance(&pl, 2);
        let mut i = inst.lock().unwrap();
        i.note_on(0, 69, 100.0 / 127.0);
        let (mut l, mut r) = (vec![0.0f32; 4800], vec![0.0f32; 4800]);
        i.process(48000, &mut l, &mut r);
        let peak = l.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.3 && peak < 1.0, "peak {peak}");
        let crossings = l[480..].windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        assert!((77..=89).contains(&crossings), "{crossings} rising crossings in 4320 samples");
        i.note_off(0, 69);
        drop(i);
        // An instance whose plugin is not the built-in gives no file state.
        assert_eq!(pl.osc3_file_state(99), None);
    }

    /// A 3x Osc channel added in the app is saved as the file's 3x Osc with its settings; the reopened project hosts it
    /// again, and a file's 3x Osc channel is hosted from its own state and written back unchanged.
    #[test]
    fn osc3_channels_save_and_reopen() {
        let path = std::env::temp_dir().join(format!("openstudio-osc3-{}.flp", std::process::id()));
        let mut a = App::new(None);
        a.state.project = crate::app::test_project();
        a.state.file = None;
        a.state.path = None;
        a.plugins.available = crate::plugins::builtin();
        // Added as the picker adds it: a plugin channel storing the file id, shown with the built-in name.
        let row = crate::app::add_plugin_channel(&mut a.state.project, LEMON_TRIO);
        let ch = &a.state.project.channels[row];
        assert_eq!((ch.kind, ch.plugin.as_deref()), (ChannelKind::Plugin, Some(FILE_NAME)));
        assert_eq!(crate::project_plugins::plugin_title(ch).as_deref(), Some(LEMON_TRIO));
        // Hosted with a new instance's settings, then one is edited.
        a.sync_osc3();
        assert!(a.state.hosted_rows.contains(&row));
        let mut fl = a.plugins.osc3_file_state(row).unwrap();
        assert_eq!(fl, default_state());
        set_field(&mut fl, op::osc(1) + op::COARSE + 1, 7);
        assert!(a.plugins.set_state(row, &op::plugin_state_from_fl(&fl)));
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
        b.plugins.available = crate::plugins::builtin();
        b.open(path.clone());
        b.sync_osc3();
        assert!(b.state.hosted_rows.contains(&row), "{:?}", b.state.message);
        assert_eq!(b.plugins.osc3_file_state(row), Some(fl.clone()));
        // Saved again unedited: the channel's state bytes are unchanged.
        b.save_to(path.clone());
        let again = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(again.channels[row], saved.channels[row]);
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
        a.state.project = crate::app::test_project();
        a.state.file = None;
        a.state.path = None;
        a.plugins.available = crate::plugins::builtin();
        let row = crate::app::add_plugin_channel(&mut a.state.project, LEMON_TRIO);
        let mut fl = default_state();
        set_field(&mut fl, op::osc(0) + op::SHAPE + 1, 6); // osc 1: custom
        set_field(&mut fl, op::MIX2 + 1, 0); // osc 2 and 3 silent
        set_field(&mut fl, op::MIX3 + 1, 0);
        let ch = &mut a.state.project.channels[row];
        (ch.plugin_state, ch.sample_path) = (Some(fl.clone()), Some(square.clone()));
        a.sync_osc3();
        assert!(a.state.hosted_rows.contains(&row), "{:?}", a.state.message);
        // The plugin opens the file itself: placeholders resolved.
        assert_eq!(a.plugins.osc3_sample(row, true), Some(crate::plugins::resolve_sample(&square)));
        assert_eq!(a.plugins.osc3_file_state(row), Some(fl.clone()));
        // It sounds: the square at 880 Hz (key 69), one period per cycle.
        let inst = instance(&a.plugins, row);
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
        assert_eq!(a.state.project.channels[row].name.as_deref(), Some(LEMON_TRIO));
        // A sample the plugin loads itself (its file dialog): the channel takes its path and its name.
        assert!(a.plugins.load_sample(row, &saw));
        a.osc3_look = None;
        a.follow_osc3_samples();
        let ch = &a.state.project.channels[row];
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
        b.plugins.available = crate::plugins::builtin();
        b.open(path.clone());
        b.sync_osc3();
        assert_eq!(b.plugins.osc3_sample(row, true), Some(crate::plugins::resolve_sample(&saw)));
        drop(b);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The rack's Add one ▸ and Insert ▸ menus add a Lemon Trio channel and ask for the built-in plugin (the
    /// instance itself is made on the next tick, which also opens its window: done by hand here).
    #[test]
    fn plugin_menus_add_lemon_3x_osc_channels() {
        use crate::plugin_menu::Target;
        let mut a = App::new(None);
        a.state.project = crate::app::test_project();
        a.plugins.available = crate::plugins::builtin();
        a.state.plugdb.rebuild(&a.plugins);
        let index = a.plugins.index_of(LEMON_TRIO).unwrap();
        let pick = |a: &App, target| crate::plugin_menu::items(&a.state, target, true).into_iter().find(|it| it.label == LEMON_TRIO).map(|it| it.cmd);
        let n = a.state.project.channels.len();
        assert!(crate::rack::add_one_items(&a.state).iter().any(|it| it.label == LEMON_TRIO), "Add one lists Lemon Trio");
        let cmd = pick(&a, Target::Gen(None)).expect("the generator menu lists Lemon Trio");
        crate::playlist::run(&mut a, cmd);
        assert_eq!(a.state.project.channels.len(), n + 1);
        assert_eq!(a.state.pending_plugin, Some((n, index)));
        assert!(is_osc3(&a.state.project.channels[n]));
        assert_eq!(a.state.project.channels[n].name.as_deref(), Some(LEMON_TRIO));
        assert_eq!(a.plugins.available[index].desc.name, LEMON_TRIO);
        a.state.pending_plugin = None;
        a.sync_osc3();
        assert!(a.state.hosted_rows.contains(&n));
        assert_eq!(a.plugins.osc3_file_state(n), Some(default_state()));
        // Insert ▸ on the first row: right below it.
        let cmd = pick(&a, Target::Gen(Some(0))).unwrap();
        crate::playlist::run(&mut a, cmd);
        assert_eq!(a.state.pending_plugin, Some((1, index)));
        assert!(is_osc3(&a.state.project.channels[1]));
        assert_eq!(a.state.selected_channel, 1);
        assert!(a.state.hosted_rows.contains(&(n + 1)), "the channel hosted before moved down one row");
    }

    /// Replace ▸ (channel menu): the channel keeps its row, notes, mixer track and levels, and takes the new generator.
    /// A sample the channel had goes: it would be the new Lemon Trio's custom shape sample.
    #[test]
    fn replace_puts_another_generator_on_a_channel() {
        use crate::plugin_menu::Target;
        let mut a = App::new(None);
        a.state.project = crate::app::test_project();
        a.state.project.channels[2].sample_path = Some("C:/Samples/Kick.wav".into());
        a.plugins.available = crate::plugins::builtin();
        a.state.plugdb.rebuild(&a.plugins);
        let index = a.plugins.index_of(LEMON_TRIO).unwrap();
        let (n, before) = (a.state.project.channels.len(), a.state.project.channels[2].clone());
        let cmd = crate::plugin_menu::items(&a.state, Target::Replace(2), true).into_iter().find(|it| it.label == LEMON_TRIO).map(|it| it.cmd).unwrap();
        crate::playlist::run(&mut a, cmd);
        let after = &a.state.project.channels[2];
        assert_eq!(a.state.project.channels.len(), n, "no channel added");
        assert!(is_osc3(after));
        assert_eq!(after.name.as_deref(), Some(LEMON_TRIO));
        assert_eq!((after.index, after.mixer_track, after.volume, after.pan), (before.index, before.mixer_track, before.volume, before.pan));
        assert_eq!(after.sample_path, None, "the new instance starts without a sample");
        assert_eq!(a.state.pending_plugin, Some((2, index)));
        assert!(!a.state.hosted_rows.contains(&2), "the old instance is gone");
    }

    /// Corpus check (OPENSTUDIO_FLP_CORPUS): every 3x Osc channel of every file is hosted by Lemon Trio from its
    /// saved state, gives the same bytes back, and an unedited save leaves the whole project as it was.
    #[test]
    fn corpus_osc3_channels_survive_hosting_and_saving() {
        let Some(list) = std::env::var_os("OPENSTUDIO_FLP_CORPUS") else { return };
        let text = std::fs::read_to_string(list).unwrap();
        let (mut files, mut channels) = (0, 0);
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
            a.plugins.available = crate::plugins::builtin();
            a.open(std::path::PathBuf::from(path));
            a.sync_osc3();
            for &r in &rows {
                let st = project.channels[r].plugin_state.clone().expect("3x Osc state");
                assert_eq!(st.len(), op::STATE_LEN, "{path}: channel {r}");
                assert_eq!(a.plugins.osc3_file_state(r).as_ref(), Some(&st), "{path}: channel {r}");
                channels += 1;
            }
            let before = a.state.project.clone();
            a.save_to(out.clone());
            assert!(a.state.message.as_deref().is_some_and(|m| m.starts_with("Saved")), "{path}: {:?}", a.state.message);
            let back = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&out).unwrap()).unwrap()).unwrap();
            for &r in &rows {
                assert_eq!(back.channels[r], before.channels[r], "{path}: channel {r}");
            }
            files += 1;
        }
        let _ = std::fs::remove_file(&out);
        eprintln!("3x Osc: {channels} channels in {files} files");
        assert!(files > 0, "no corpus file with a 3x Osc channel");
    }
}
