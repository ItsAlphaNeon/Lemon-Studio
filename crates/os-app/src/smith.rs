// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Smith in Lemon Studio: compiled in like Citrus, it plays every Ogun channel of a project (started from the state the
//! file holds) and every one added from the picker, the browser or the channel rack; saving writes the Ogun state it
//! plays back into the channel byte for byte, so the file keeps its own plugin. FL's Ogun factory presets (`.fst`),
//! when FL is installed, are listed with Smith's own presets.
//!
// spec: plugins/smith.md (app integration, presets)

use std::path::{Path, PathBuf};

use os_model::{Channel, ChannelKind, Project};

use crate::plugins::{Plugins, FX_KEY};

/// The built-in plugin's name (picker, window title).
pub const SMITH: &str = "Smith";
/// The plugin id project files store for it (event 201).
pub const FILE_NAME: &str = "Ogun";

/// Whether a channel holds the file's Ogun.
pub fn is_ogun(ch: &Channel) -> bool {
    ch.kind == ChannelKind::Plugin && ch.plugin.as_deref().is_some_and(|p| p.eq_ignore_ascii_case(FILE_NAME))
}

impl Plugins {
    /// The Ogun state the Smith on rack row `row` plays (empty while it plays its default patch), `None` when the row
    /// hosts something else.
    pub fn ogun_file_state(&self, row: usize) -> Option<Vec<u8>> {
        let h = self.hosted.get(&row)?;
        let p = h.lock_for_state()?;
        if p.desc().name != SMITH || !crate::project_plugins::is_builtin(p.desc()) {
            return None;
        }
        smith::fl_from_plugin_state(&p.save_state()?)
    }
}

/// Before a save: every hosted Smith writes the state it plays into its channel as the file's Ogun. One that plays its
/// default patch stores none (FL starts such a channel with its own default patch).
pub fn store(proj: &mut Project, plugins: &Plugins) {
    for row in plugins.hosted.range(..FX_KEY).map(|(&k, _)| k).collect::<Vec<_>>() {
        let Some(st) = plugins.ogun_file_state(row) else { continue };
        if let Some(ch) = proj.channels.get_mut(row).filter(|c| c.kind == ChannelKind::Plugin) {
            ch.plugin = Some(FILE_NAME.into());
            ch.plugin_state = (!st.is_empty()).then_some(st);
        }
    }
}

/// FL's Ogun factory presets, when FL is installed.
fn factory_dir() -> PathBuf {
    let fl = crate::factory_folder::dir().unwrap_or_else(os_native::default_fl_dir);
    fl.join("Data").join("Patches").join("Plugin presets").join("Generators").join("Ogun")
}

/// Smith's presets beyond its own folder: FL's factory ones.
pub fn more_presets(plugin: &str) -> Vec<crate::presets::Preset> {
    if plugin == SMITH { crate::citrus::fst_presets_in(&factory_dir()) } else { Vec::new() }
}

/// The plugin state of an FL preset file for Smith; `None` when `plugin` is not Smith or `path` is not an `.fst` file,
/// an error text when it holds no Ogun state.
pub fn fst_state(path: &Path, plugin: &str) -> Option<Result<Vec<u8>, String>> {
    if plugin != SMITH || !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("fst")) {
        return None;
    }
    Some(
        std::fs::read(path)
            .map_err(|e| format!("{}: {e}", path.display()))
            .and_then(|b| {
                smith::state_from_fst(&b)
                    .filter(|fl| smith::state::State::from_bytes(fl).is_some())
                    .ok_or_else(|| format!("{} holds no patch Smith can read", path.display()))
            })
            .map(|fl| smith::plugin_state_from_fl(&fl)),
    )
}

impl crate::app::App {
    /// Hosts Smith on every Ogun channel without an instance, started from the channel's saved state (the default patch
    /// when the channel has none). A row waiting for the picker's own instance is left to it; a row whose instance
    /// failed is not tried again.
    pub(crate) fn sync_smith(&mut self) {
        // The installed Ogun plays these channels instead (`host_native`).
        if crate::project_plugins::native_first() {
            return;
        }
        let Some(index) = self.plugins.index_of(SMITH) else { return };
        let pending = self.state.pending_plugin.map(|(r, _)| r);
        let rows: Vec<usize> = (0..self.state.project.channels.len())
            .filter(|&r| is_ogun(&self.state.project.channels[r]))
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
            let st = smith::plugin_state_from_fl(&fl);
            match self.plugins.create_with(row, index, Some(&st)) {
                Ok(_) => {
                    self.state.hosted_rows.push(row);
                    added = true;
                }
                Err(e) => {
                    self.wrap_failed.push(row);
                    self.state.message = Some(format!("{SMITH}: {e}"));
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
    use os_engine::SharedInstrument;
    use smith::state::{State, RICH};

    fn patch() -> Vec<u8> {
        let mut st = State::default();
        st.ints[RICH] = 9;
        st.to_bytes()
    }

    /// The built-in Smith starts from a project file's Ogun state, plays it, and gives the same bytes back.
    #[test]
    fn builtin_smith_takes_and_gives_the_file_state() {
        let mut pl = Plugins::default();
        pl.available = crate::plugins::builtin();
        let index = pl.index_of(SMITH).expect("built-in Smith");
        assert!(crate::project_plugins::is_builtin(&pl.available[index].desc));
        let fl = patch();
        pl.create_with(2, index, Some(&smith::plugin_state_from_fl(&fl))).unwrap();
        assert_eq!(pl.ogun_file_state(2), Some(fl.clone()));
        let inst = pl.hosted[&2].plugin.clone() as SharedInstrument;
        let mut i = inst.lock().unwrap();
        i.note_on(0, 69, 100.0 / 128.0);
        let (mut l, mut r) = (vec![0.0f32; 4800], vec![0.0f32; 4800]);
        i.process(48000, &mut l, &mut r);
        i.process(48000, &mut l, &mut r);
        let peak = l.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.005 && peak < 2.0, "peak {peak}");
        i.note_off(0, 69);
        drop(i);
        assert_eq!(pl.ogun_file_state(99), None);
    }

    /// An Ogun channel added in the app saves no state while it plays the default patch; one loaded with a patch saves
    /// that patch's bytes, and the reopened project hosts it again from them.
    #[test]
    fn ogun_channels_save_and_reopen() {
        let path = std::env::temp_dir().join(format!("openstudio-smith-{}.flp", std::process::id()));
        let mut a = App::new(None);
        a.state.project = crate::app::test_project();
        a.state.file = None;
        a.state.path = None;
        a.plugins.available = crate::plugins::builtin();
        let row = crate::app::add_plugin_channel(&mut a.state.project, SMITH);
        let ch = &a.state.project.channels[row];
        assert_eq!((ch.kind, ch.plugin.as_deref()), (ChannelKind::Plugin, Some(FILE_NAME)));
        assert_eq!(crate::project_plugins::plugin_title(ch).as_deref(), Some(SMITH));
        a.sync_smith();
        assert!(a.state.hosted_rows.contains(&row));
        assert_eq!(a.plugins.ogun_file_state(row), Some(Vec::new()));
        a.save_to(path.clone());
        let saved = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        assert_eq!((saved.channels[row].plugin.as_deref(), saved.channels[row].plugin_state.as_deref()), (Some(FILE_NAME), None));

        // A patch goes in (as a preset load does), the project is saved and reopened.
        let fl = patch();
        assert!(a.plugins.set_state(row, &smith::plugin_state_from_fl(&fl)));
        a.save_to(path.clone());
        let saved = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        assert_eq!(saved.channels[row].plugin_state.as_deref(), Some(fl.as_slice()));
        let mut b = App::new(None);
        b.plugins.available = crate::plugins::builtin();
        b.open(path.clone());
        b.sync_smith();
        assert!(b.state.hosted_rows.contains(&row), "{:?}", b.state.message);
        assert_eq!(b.plugins.ogun_file_state(row), Some(fl.clone()));
        b.save_to(path.clone());
        let again = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(again.channels[row], saved.channels[row]);
    }

    /// A state Smith cannot read is played as the default patch and still saved as it came.
    #[test]
    fn unreadable_ogun_states_are_kept() {
        let mut pl = Plugins::default();
        pl.available = crate::plugins::builtin();
        let index = pl.index_of(SMITH).unwrap();
        pl.create_with(0, index, Some(&smith::plugin_state_from_fl(&[4, 5, 6]))).unwrap();
        assert_eq!(pl.ogun_file_state(0), Some(vec![4, 5, 6]));
    }

    /// An `.fst` holding an Ogun state is read for Smith and left alone for any other plugin; one that holds something
    /// else is an error.
    #[test]
    fn fst_presets_are_read_for_smith() {
        let dir = std::env::temp_dir().join(format!("openstudio-smith-fst-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fl = patch();
        let good = dir.join("Good.fst");
        std::fs::write(&good, citrus::preset::test_fst(&fl)).unwrap();
        let st = fst_state(&good, SMITH).unwrap().unwrap();
        assert_eq!(smith::fl_from_plugin_state(&st), Some(fl));
        assert!(fst_state(&good, crate::citrus::CITRUS).is_none());
        assert!(fst_state(&dir.join("notes.txt"), SMITH).is_none());
        std::fs::write(dir.join("Bad.fst"), b"nope").unwrap();
        assert!(fst_state(&dir.join("Bad.fst"), SMITH).unwrap().is_err());
        std::fs::write(dir.join("Other.fst"), citrus::preset::test_fst(&[1, 2, 3])).unwrap();
        assert!(fst_state(&dir.join("Other.fst"), SMITH).unwrap().is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
