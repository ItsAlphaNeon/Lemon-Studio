// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Citrus in Lemon Studio: compiled in like Lemon Trio, it plays every Sytrus channel of a project (started from the
//! state the file holds) and every one added from the picker, the browser or the channel rack; saving writes the
//! Sytrus state it plays back into the channel byte for byte, so the file keeps its own plugin. FL's Sytrus factory
//! presets (`.fst`), when FL is installed, are listed with Citrus's own presets.
//!
// spec: plugins/citrus.md (app integration, presets)

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use std::path::{Path, PathBuf};

use os_model::{Channel, ChannelKind, Project};

use crate::plugins::{Plugins, FX_KEY};

/// The built-in plugin's name (picker, window title).
pub const CITRUS: &str = "Citrus";

/// A Sytrus channel's automation clips target its Modulation X and Y as parameters 18 and 19; they are Citrus's parameters 0
/// and 1 (spec: plugins/plugin-parameters.md section 3).
pub const SYTRUS_PARAMS: &[(u32, u32)] = &[(18, 0), (19, 1)];

/// The plugin id project files store for it (event 201).
pub const FILE_NAME: &str = "Sytrus";

/// Whether a channel holds the file's Sytrus.
pub fn is_sytrus(ch: &Channel) -> bool {
    ch.kind == ChannelKind::Plugin && ch.plugin.as_deref() == Some(FILE_NAME)
}

impl Plugins {
    /// The Sytrus state the Citrus on rack row `row` plays (empty while it plays its default patch), `None` when the
    /// row hosts something else.
    pub fn sytrus_file_state(&self, row: usize) -> Option<Vec<u8>> {
        #[cfg(feature = "stock-citrus")]
        {
            let h = self.hosted.get(&row)?;
            let p = h.lock_for_state()?;
            if p.desc().name != CITRUS || !crate::project_plugins::is_builtin(p.desc()) {
                return None;
            }
            citrus::fl_from_plugin_state(&p.save_state()?)
        }
        #[cfg(not(feature = "stock-citrus"))]
        {
            let _ = (self, row);
            None
        }
    }
}

/// Before a save: every hosted Citrus writes the state it plays into its channel as the file's Sytrus. One that plays
/// its default patch stores none (FL starts such a channel with its own default patch).
pub fn store(proj: &mut Project, plugins: &Plugins) {
    for row in plugins.hosted.range(..FX_KEY).map(|(&k, _)| k).collect::<Vec<_>>() {
        let Some(st) = plugins.sytrus_file_state(row) else { continue };
        if let Some(ch) = proj.channels.get_mut(row).filter(|c| c.kind == ChannelKind::Plugin) {
            ch.plugin = Some(FILE_NAME.into());
            ch.plugin_state = (!st.is_empty()).then_some(st);
        }
    }
}

/// FL's Sytrus factory presets, when FL is installed.
fn factory_dir(st: &crate::settings_file::SettingsStore) -> PathBuf {
    let fl = crate::factory_folder::dir(st).unwrap_or_else(os_paths::default_fl_dir);
    fl.join("Data").join("Patches").join("Plugin presets").join("Generators").join("Sytrus")
}

/// The `.fst` presets under `dir` (any depth), shown as "Folder - Sub/Name" (one submenu per folder path; the ones at
/// the top by their name alone) and sorted by that name. No source is named.
pub fn fst_presets_in(dir: &Path) -> Vec<crate::presets::Preset> {
    fn walk(dir: &Path, rel: &mut Vec<String>, out: &mut Vec<crate::presets::Preset>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                rel.push(p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
                walk(&p, rel, out);
                rel.pop();
            } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("fst")) {
                let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let name = if rel.is_empty() { stem } else { format!("{}/{stem}", rel.join(" - ")) };
                out.push(crate::presets::Preset { name, path: p, factory: false });
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut Vec::new(), &mut out);
    out.sort_by_key(|p| p.name.to_lowercase());
    out
}

/// Citrus's presets beyond its own folder: FL's factory ones.
pub fn more_presets(st: &crate::settings_file::SettingsStore, plugin: &str) -> Vec<crate::presets::Preset> {
    if plugin == CITRUS { fst_presets_in(&factory_dir(st)) } else { Vec::new() }
}

/// The plugin state of an FL preset file for Citrus; `None` when `path` is not an `.fst` file, an error text when it
/// holds no Sytrus state.
pub fn fst_state(path: &Path) -> Option<Result<Vec<u8>, String>> {
    #[cfg(feature = "stock-citrus")]
    {
        if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("fst")) {
            return None;
        }
        Some(
            std::fs::read(path)
                .map_err(|e| format!("{}: {e}", path.display()))
                .and_then(|b| citrus::preset::state_from_fst(&b).ok_or_else(|| format!("{} holds no patch Citrus can read", path.display())))
                .map(|fl| citrus::plugin_state_from_fl(&fl)),
        )
    }
    #[cfg(not(feature = "stock-citrus"))]
    {
        let _ = path;
        None
    }
}

/// Hosts Citrus on every Sytrus channel without an instance, started from the channel's saved state (the default
/// patch when the channel has none). A row waiting for the picker's own instance is left to it; a row whose
/// instance failed is not tried again.
pub fn sync_citrus(s: &mut crate::session::Session) {
    #[cfg(feature = "stock-citrus")]
    {
        // The installed Sytrus plays these channels instead (`host_native`).
        if crate::project_plugins::native_first() {
            return;
        }
        let Some(index) = s.plugins.index_of(CITRUS) else { return };
        let pending = s.pending_plugin.map(|(r, _)| r);
        let rows: Vec<usize> = (0..s.project.channels.len())
            .filter(|&r| is_sytrus(&s.project.channels[r]))
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
            let st = citrus::plugin_state_from_fl(&fl);
            match s.plugins.create_with(row, index, Some(&st)) {
                Ok(_) => {
                    s.hosted_rows.push(row);
                    added = true;
                }
                Err(e) => {
                    s.plugins.wrap_failed.push(row);
                    s.touched.views.push(crate::session::View::Message(format!("{CITRUS}: {e}")));
                }
            }
        }
        if added {
            s.transport.set_instruments(s.plugins.instruments());
            s.dirty = true;
        }
        s.touched.repaint = true;
    }
    #[cfg(not(feature = "stock-citrus"))]
    {
        let _ = s;
    }
}

#[cfg(all(test, feature = "stock-citrus"))]
mod tests {
    use super::*;
    use os_engine::SharedInstrument;

    /// A Sytrus state: operator 1 as a saw (harmonic levels `1 / h`) to the output, made from Citrus's default patch.
    fn saw_state() -> Vec<u8> {
        citrus::preset::test_state(&(1..=24).map(|h| 1.0 / h as f32).collect::<Vec<_>>())
    }

    /// The built-in Citrus starts from a project file's Sytrus state, plays it, and gives the same bytes back.
    #[test]
    fn builtin_citrus_takes_and_gives_the_file_state() {
        let mut pl = Plugins::default();
        pl.available = crate::plugins::builtin();
        pl.set_rate(48000);
        let index = pl.index_of(CITRUS).expect("built-in Citrus");
        assert!(crate::project_plugins::is_builtin(&pl.available[index].desc));
        let fl = saw_state();
        pl.create_with(2, index, Some(&citrus::plugin_state_from_fl(&fl))).unwrap();
        assert_eq!(pl.sytrus_file_state(2), Some(fl.clone()));
        // It sounds: key 69 at 440 Hz (key 60 plays C5 at 523.25 Hz in Sytrus's pitch, so 69 is 880 Hz), counted at
        // 48 kHz.
        let inst = pl.hosted[&2].plugin.clone() as SharedInstrument;
        let mut i = inst.lock().unwrap();
        i.note_on(0, 69, 100.0 / 128.0);
        let (mut l, mut r) = (vec![0.0f32; 4800], vec![0.0f32; 4800]);
        i.process(48000, &mut l, &mut r);
        let peak = l.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.05 && peak < 1.5, "peak {peak}");
        let crossings = l[480..].windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        assert!((35..=45).contains(&crossings) || (75..=85).contains(&crossings), "{crossings} rising crossings in 4320 samples");
        i.note_off(0, 69);
        drop(i);
        assert_eq!(pl.sytrus_file_state(99), None);
    }

    /// A state Citrus cannot read is played as the default patch and still saved as it came.
    #[test]
    fn unreadable_sytrus_states_are_kept() {
        let mut pl = Plugins::default();
        pl.available = crate::plugins::builtin();
        let index = pl.index_of(CITRUS).unwrap();
        pl.create_with(0, index, Some(&citrus::plugin_state_from_fl(&[4, 5, 6]))).unwrap();
        assert_eq!(pl.sytrus_file_state(0), Some(vec![4, 5, 6]));
    }

    /// `.fst` files are listed at any depth with their folder path, and one is read into a Citrus plugin state.
    #[test]
    fn fst_presets_are_listed_and_read() {
        let dir = std::env::temp_dir().join(format!("openstudio-citrus-fst-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Bass").join("Deep")).unwrap();
        let fl = saw_state();
        let fst = citrus::preset::test_fst(&fl);
        std::fs::write(dir.join("Default.fst"), &fst).unwrap();
        std::fs::write(dir.join("Bass").join("Deep").join("Sub.fst"), &fst).unwrap();
        std::fs::write(dir.join("Bass").join("notes.txt"), b"x").unwrap();
        let l = fst_presets_in(&dir);
        let names: Vec<&str> = l.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["Bass - Deep/Sub", "Default"]);
        let st = fst_state(&l[0].path).unwrap().unwrap();
        assert_eq!(citrus::fl_from_plugin_state(&st), Some(fl));
        assert!(fst_state(&dir.join("Bass").join("notes.txt")).is_none());
        std::fs::write(dir.join("Bad.fst"), b"nope").unwrap();
        assert!(fst_state(&dir.join("Bad.fst")).unwrap().is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
