// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Smith in Lemon Studio: compiled in like Citrus, it plays every Ogun channel of a project (started from the state the
//! file holds) and every one added from the picker, the browser or the channel rack; saving writes the Ogun state it
//! plays back into the channel byte for byte, so the file keeps its own plugin. FL's Ogun factory presets (`.fst`),
//! when FL is installed, are listed with Smith's own presets.
//!
// spec: plugins/smith.md (app integration, presets)

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
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
        #[cfg(feature = "stock-smith")]
        {
            let h = self.hosted.get(&row)?;
            let p = h.lock_for_state()?;
            if p.desc().name != SMITH || !crate::project_plugins::is_builtin(p.desc()) {
                return None;
            }
            smith::fl_from_plugin_state(&p.save_state()?)
        }
        #[cfg(not(feature = "stock-smith"))]
        {
            let _ = (self, row);
            None
        }
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
fn factory_dir(st: &crate::settings_file::SettingsStore) -> PathBuf {
    let fl = crate::factory_folder::dir(st).unwrap_or_else(os_paths::default_fl_dir);
    fl.join("Data").join("Patches").join("Plugin presets").join("Generators").join("Ogun")
}

/// Smith's presets beyond its own folder: FL's factory ones.
pub fn more_presets(st: &crate::settings_file::SettingsStore, plugin: &str) -> Vec<crate::presets::Preset> {
    if plugin == SMITH { crate::citrus::fst_presets_in(&factory_dir(st)) } else { Vec::new() }
}

/// The plugin state of an FL preset file for Smith; `None` when `plugin` is not Smith or `path` is not an `.fst` file,
/// an error text when it holds no Ogun state.
pub fn fst_state(path: &Path, plugin: &str) -> Option<Result<Vec<u8>, String>> {
    #[cfg(feature = "stock-smith")]
    {
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
    #[cfg(not(feature = "stock-smith"))]
    {
        let _ = (path, plugin);
        None
    }
}

/// Hosts Smith on every Ogun channel without an instance, started from the channel's saved state (the default patch
/// when the channel has none). A row waiting for the picker's own instance is left to it; a row whose instance
/// failed is not tried again.
pub fn sync_smith(s: &mut crate::session::Session) {
    #[cfg(feature = "stock-smith")]
    {
        // The installed Ogun plays these channels instead (`host_native`).
        if crate::project_plugins::native_first() {
            return;
        }
        let Some(index) = s.plugins.index_of(SMITH) else { return };
        let pending = s.pending_plugin.map(|(r, _)| r);
        let rows: Vec<usize> = (0..s.project.channels.len())
            .filter(|&r| is_ogun(&s.project.channels[r]))
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
            let st = smith::plugin_state_from_fl(&fl);
            match s.plugins.create_with(row, index, Some(&st)) {
                Ok(_) => {
                    s.hosted_rows.push(row);
                    added = true;
                }
                Err(e) => {
                    s.plugins.wrap_failed.push(row);
                    s.touched.views.push(crate::session::View::Message(format!("{SMITH}: {e}")));
                }
            }
        }
        if added {
            s.transport.set_instruments(s.plugins.instruments());
            s.dirty = true;
        }
        s.touched.repaint = true;
    }
    #[cfg(not(feature = "stock-smith"))]
    {
        let _ = s;
    }
}

#[cfg(all(test, feature = "stock-smith", feature = "stock-citrus"))]
mod tests {
    use super::*;
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
        pl.set_rate(48000);
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
