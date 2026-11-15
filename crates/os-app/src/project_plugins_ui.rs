// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::project_plugins` (step 9: the session's side stays there).

#![allow(unused_imports)]
#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)

use os_flp::wrapper::{self, WRAPPER};
use os_host::{Descriptor, Format};
use os_model::{Channel, ChannelKind, PluginRef, Project, SlotPlugin};
use crate::plugins::{format_name, fx_of, Available, Plugins};
#[allow(unused_imports)]
use crate::project_plugins::*;

impl crate::app::App {
    /// Hosts the wrapped plugins of an opened project (spec: flp/events/plugins.md §4): each wrapped channel and slot
    /// whose plugin is found starts from the state the file holds, or from its defaults when only a copy in another
    /// format is found (the hint bar names those; a save keeps the file's wrapper for them until they are changed,
    /// `Hosted::stand_in`). A plugin that is not found keeps its saved data (written back unchanged) and is not looked
    /// for again.
    pub fn sync_wrapped(&mut self) {
        self.host(sync_wrapped);
    }

    /// With the experimental native host on (Settings > Plugins), the native plugins an opened project names are
    /// hosted by the installed plugins themselves (spec: plugins/fruity-abi.md "Opened projects"): see `host_native`.
    pub fn sync_native(&mut self) {
        if os_session::native::enabled() {
            self.host_native();
        }
    }

    /// Hosts every plugin channel and filled mixer slot that names a native plugin (its file id, event 201) and has
    /// no instance yet, started from the state the file holds (event 213). Channels and slots the built-in Open
    /// plugins play (3x Osc, the parametric EQ, wrapped plugins) are theirs already, so they are left alone. A plugin
    /// still waiting for its first-load check is tried again on a later tick; one that is not installed is reported
    /// once the scan is over and kept in the project; one that does not start is reported and kept too. An instance
    /// that does not give the file's state back byte for byte keeps the file's bytes on a save (`Hosted::keep_file_state`).
    pub fn host_native(&mut self) {
        let scanning = self.sweep.is_some();
        self.host(|s| host_native(s, scanning));
    }

    /// The hidden preference changed (or the native host with it): the channels and slots a built-in recreation can
    /// play (3x Osc, the parametric EQ) move to the side that should play them now. Each one hosted by the other side
    /// writes its settings into the project as the file's own state, its instance is closed, and the next tick hosts
    /// it again from that state (`sync_osc3` / `sync_effects`, or `host_native`). A row the plugin menus are just
    /// creating is left alone. Returns how many channels and slots were switched.
    pub fn swap_builtin_overrides(&mut self) -> usize {
        let native = native_first();
        let pending = self.state.session.pending_plugin.map(|(r, _)| r);
        let swap: Vec<usize> = self
            .state.session.plugins
            .hosted
            .keys()
            .copied()
            .filter(|&key| Some(key) != pending && id_of_key(&self.state.session.project, key).is_some_and(|id| has_recreation(&id)))
            .filter(|&key| {
                let Some(d) = self.state.session.plugins.descriptor(key) else { return false };
                // Moving to the real plugins: the recreations go; moving back: the real plugins go.
                if native { is_builtin(&d) } else { d.format == Format::Native }
            })
            .collect();
        // Their settings, as the file stores them, stay in the project.
        self.store_builtin_states();
        for &key in &swap {
            if self.state.session.plugins.descriptor(key).is_some_and(|d| d.format == Format::Native) && !self.state.session.plugins.hosted[&key].keep_file_state {
                if let Some(state) = self.state.session.plugins.state(key) {
                    store_native(self.state.session.project.get_mut(), key, state);
                }
            }
        }
        for &key in &swap {
            self.state.session.plugins.drop_key(key);
            self.state.session.hosted_rows.retain(|&r| r != key);
        }
        // Either side may try the moved channels and slots again (a recreation that failed before, or a missing
        // native plugin); so may any other channel or slot a recreation plays.
        let all: Vec<usize> = self.state.session.plugins.wrap_failed.iter().chain(&self.state.session.plugins.fx_failed).chain(&self.state.session.plugins.native_failed).copied().filter(|&k| id_of_key(&self.state.session.project, k).is_some_and(|id| has_recreation(&id))).collect();
        for list in [&mut self.state.session.plugins.wrap_failed, &mut self.state.session.plugins.fx_failed, &mut self.state.session.plugins.native_failed] {
            list.retain(|k| !all.contains(k));
        }
        if !swap.is_empty() {
            self.state.session.transport.set_instruments(self.state.session.plugins.instruments());
            self.state.session.transport.set_effects(self.state.session.plugins.effects());
            self.state.session.dirty = true;
        }
        self.invalidate();
        swap.len()
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::plugins::{fx_key, LEMON_EQ, LEMON_SAMPLER};

    /// A stand-in native plugin: its state is a few bytes; `exact` false = it gives a loaded state back with one byte
    /// appended (a plugin whose state does not round-trip, spec plugins/fruity-abi.md "Saved state").
    struct NativeStandIn(Descriptor, Vec<u8>, bool);

    /// A library of one native stand-in whose descriptor id is its folder name (as `os_native` lists them).
    struct NativeStandInLib(Descriptor, bool);
    impl os_host::Library for NativeStandInLib {
        fn path(&self) -> &std::path::Path {
            &self.0.path
        }
        fn plugins(&self) -> &[Descriptor] {
            std::slice::from_ref(&self.0)
        }
        fn create(self: std::sync::Arc<Self>, _: &str) -> Result<os_host::SharedPlugin, String> {
            Ok(std::sync::Arc::new(std::sync::Mutex::new(NativeStandIn(self.0.clone(), vec![0, 0], self.1))))
        }
    }

    fn native_stand_in(name: &str, kind: os_host::Kind, exact: bool) -> crate::plugins::Available {
        let folder = if kind == os_host::Kind::Effect { "Effects" } else { "Generators" };
        let desc = Descriptor {
            format: Format::Native,
            id: name.into(),
            name: name.into(),
            vendor: String::new(),
            version: String::new(),
            features: vec![],
            kind,
            path: format!("C:/FL/Plugins/Fruity/{folder}/{name}/{name}_x64.dll").into(),
        };
        crate::plugins::Available { lib: std::sync::Arc::new(NativeStandInLib(desc.clone(), exact)), desc }
    }
    impl os_engine::Instrument for NativeStandIn {
        fn note_on(&mut self, _: u32, _: u8, _: f32) {}
        fn note_off(&mut self, _: u32, _: u8) {}
        fn process(&mut self, _: u32, _: &mut [f32], _: &mut [f32]) {}
    }
    impl os_engine::HostedEffect for NativeStandIn {
        fn process_fx(&mut self, _: u32, _: &mut [f32], _: &mut [f32]) {}
    }
    impl os_host::Plugin for NativeStandIn {
        fn desc(&self) -> &Descriptor {
            &self.0
        }
        fn activate(&mut self, _: u32) -> bool {
            true
        }
        fn idle(&mut self) {}
        fn has_editor(&self) -> bool {
            false
        }
        fn editor_open(&self) -> bool {
            false
        }
        fn open_editor(&mut self, _: *mut std::ffi::c_void, _: f32) -> Result<(u32, u32), String> {
            Err("none".into())
        }
        fn editor_size(&self) -> Option<(u32, u32)> {
            None
        }
        fn close_editor(&mut self) {}
        fn save_state(&self) -> Option<Vec<u8>> {
            Some(self.1.clone())
        }
        fn load_state(&mut self, d: &[u8]) -> bool {
            self.1 = d.to_vec();
            if !self.2 {
                self.1.push(0xEE);
            }
            true
        }
    }

    /// A hosted plugin channel and a mixer slot plugin are saved as wrapped plugins holding their identity and state,
    /// and the reopened project hosts them again with that state; a second save keeps working on the saved file
    /// (spec: flp/events/plugins.md). The built-in Lemon plugins stand in for third-party plugins.
    #[test]
    #[cfg(feature = "stock-parametric-eq-2")]
    fn hosted_plugins_save_and_reopen() {
        WRAP_BUILTINS.with(|w| w.set(true));
        let path = std::env::temp_dir().join(format!("openstudio-wrapped-{}.flp", std::process::id()));
        let mut a = App::new(None);
        a.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        a.state.session.file = None;
        a.state.session.path = None;
        a.state.session.plugins.available = crate::plugins::builtin();
        a.state.session.plugins.effects = crate::plugins::builtin_effects();
        let row = crate::projects::add_plugin_channel(a.state.session.project.get_mut(), LEMON_SAMPLER);
        a.state.session.plugins.create(row, a.state.session.plugins.index_of(LEMON_SAMPLER).unwrap()).unwrap();
        a.state.session.hosted_rows.push(row);
        assert!(a.state.session.plugins.load_sample(row, "C:/samples/one.wav"));
        let mut fl = lemon_parametric_eq_2::params::default_state();
        lemon_parametric_eq_2::params::set_field(&mut fl, 1, -600);
        let eq = lemon_parametric_eq_2::params::plugin_state_from_fl(&fl);
        a.state.session.project.get_mut().mixer[3].slots[2] = Some(SlotPlugin { id: os_model::Id::NONE, name: LEMON_EQ.into(), state: Vec::new() });
        a.state.session.plugins.create_effect(3, 2, a.state.session.plugins.effect_index(LEMON_EQ).unwrap(), Some(&eq)).unwrap();
        a.save_to(path.clone());
        assert!(a.state.message.as_deref().is_some_and(|m| m.starts_with("Saved") && !m.contains("not stored")), "{:?}", a.state.message);

        // The file: a wrapped plugin channel and a wrapped slot.
        let file = os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap();
        let saved = os_flp::project::load(&file).unwrap();
        let ch = &saved.channels[row];
        assert_eq!((ch.kind, ch.plugin.as_deref()), (ChannelKind::Plugin, Some(WRAPPER)));
        let r = wrapped_channel(ch).unwrap();
        // (A plugin file's extension gives the format: `.clap` / `.vst3`; the stand-in's file is this program.)
        assert_eq!(r.name, LEMON_SAMPLER);
        assert_eq!(std::path::PathBuf::from(&r.path), std::env::current_exe().unwrap());
        assert_eq!(Some(r.state.clone()), a.state.session.plugins.state(row));
        let s = wrapped_slot(saved.mixer[3].slots[2].as_ref().unwrap()).unwrap();
        assert_eq!(s.name, LEMON_EQ);
        assert_eq!(slot_title(saved.mixer[3].slots[2].as_ref().unwrap()), LEMON_EQ);
        // The model now holds what the file holds.
        assert_eq!(a.state.session.project.channels[row].plugin.as_deref(), Some(WRAPPER));

        // Reopened: both plugins are hosted again with their state.
        let mut b = App::new(None);
        b.state.session.plugins.available = crate::plugins::builtin();
        b.state.session.plugins.effects = crate::plugins::builtin_effects();
        b.open(path.clone());
        b.sync_wrapped();
        assert!(b.state.session.hosted_rows.contains(&row), "{:?}", b.state.message);
        assert_eq!(b.state.session.plugins.sampler_path(row).as_deref(), Some("C:/samples/one.wav"));
        assert_eq!(b.state.session.plugins.eq_file_state(fx_key(3, 2)), Some(fl));
        assert_eq!(crate::mixer::slot_name(&b.state.session, 3, 2).as_deref(), Some(LEMON_EQ));

        // Saved again (onto the opened file): the new state is written into the same wrapper.
        assert!(b.state.session.plugins.load_sample(row, "C:/samples/two.wav"));
        b.save_to(path.clone());
        assert!(b.state.message.as_deref().is_some_and(|m| m.starts_with("Saved")), "{:?}", b.state.message);
        let again = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        let r2 = wrapped_channel(&again.channels[row]).unwrap();
        assert_eq!(Some(r2.state), b.state.session.plugins.state(row));
        assert_eq!(again.mixer[3].slots[2], saved.mixer[3].slots[2]);
    }

    /// Lemon Studio's other effects are compiled in like the EQ but have no file plugin of their own to be saved as: a
    /// slot hosting one is saved wrapped with its state, and the reopened project hosts the built-in copy again with
    /// that state (Tesla's is its sample: its parameters are only its transport; Lemon Drop's is its display state).
    #[test]
    #[cfg(feature = "stock-drop")]
    fn linked_effects_save_wrapped_and_reopen() {
        use crate::plugins::LINKED_EFFECTS;
        let path = std::env::temp_dir().join(format!("openstudio-linked-fx-{}.flp", std::process::id()));
        let mut a = App::new(None);
        a.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        a.state.session.file = None;
        a.state.session.path = None;
        a.state.session.plugins.effects = crate::plugins::builtin_effects();
        // Ten slots a track: track 3, then 4.
        let at = |k: usize| (3 + (k / 10) as u16, (k % 10) as u8);
        let key = |k: usize| fx_key(at(k).0, at(k).1);
        let tesla_frames = 4410;
        let tesla_state = tesla::state::plugin_state(&tesla::state::Saved {
            sample: tesla::sample::Sample::from_channels(44100, vec![vec![0.5; tesla_frames]], tesla::sample::Format::Int16),
            ..Default::default()
        });
        let tesla_frames_of = |st: Option<Vec<u8>>| st.and_then(|b| tesla::state::from_plugin_state(&b)).map(|s| s.sample.frames());
        // Lemon Drop's settings are its saved state (its one parameter is the display's hold switch).
        let drop_state = lemon_drop::plugin_state_from_fl(&lemon_drop::state::State { gain: 42, ..Default::default() }.to_bytes());
        let drop_gain_of = |st: Option<Vec<u8>>| st.and_then(|b| lemon_drop::fl_from_plugin_state(&b)).map(|f| lemon_drop::state_of(&f).gain);
        let mut set = Vec::new();
        for (k, name) in LINKED_EFFECTS.iter().enumerate() {
            let (t, slot) = at(k);
            a.state.session.project.get_mut().mixer[usize::from(t)].slots[usize::from(slot)] = Some(crate::plugins::new_slot(name));
            let tesla = *name == crate::plugins::TESLA;
            let drop = *name == crate::plugins::LEMON_DROP;
            let start = if tesla { Some(tesla_state.as_slice()) } else if drop { Some(drop_state.as_slice()) } else { None };
            a.state.session.plugins.create_effect(t, slot, a.state.session.plugins.effect_index(name).unwrap(), start).unwrap();
            if tesla {
                assert_eq!(tesla_frames_of(a.state.session.plugins.state(key(k))), Some(tesla_frames));
                set.push((u32::MAX, 0.0));
                continue;
            }
            if drop {
                assert_eq!(drop_gain_of(a.state.session.plugins.state(key(k))), Some(42));
                set.push((u32::MAX, 0.0));
                continue;
            }
            // One setting away from its default.
            let p = a.state.session.plugins.param_infos(key(k)).into_iter().find(|p| !p.read_only && !p.bypass).unwrap();
            let n = if (p.default - 0.3).abs() > 0.1 { 0.3 } else { 0.8 };
            assert!(a.state.session.plugins.set_param_named(key(k), &p.name, p.min + n * (p.max - p.min)), "{name}");
            // The edit reaches the plugin at its next idle (the app idles its plugins every tick).
            a.state.session.plugins.hosted[&key(k)].plugin.lock().unwrap().idle();
            let v = a.state.session.plugins.param_value(key(k), p.id).unwrap();
            assert_ne!(v, p.default, "{name}: {}", p.name);
            set.push((p.id, v));
        }
        a.save_to(path.clone());
        assert!(a.state.message.as_deref().is_some_and(|m| m.starts_with("Saved") && !m.contains("not stored")), "{:?}", a.state.message);

        let saved = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        for (k, name) in LINKED_EFFECTS.iter().enumerate() {
            let (t, slot) = at(k);
            let r = wrapped_slot(saved.mixer[usize::from(t)].slots[usize::from(slot)].as_ref().unwrap()).unwrap_or_else(|| panic!("{name} wrapped"));
            assert_eq!((Format::parse(&r.format), r.name.as_str()), (Some(Format::Clap), *name));
            assert!(r.path.ends_with(&format!("{name}.clap")), "{}", r.path);
        }

        let mut b = App::new(None);
        b.state.session.plugins.available = crate::plugins::builtin();
        b.state.session.plugins.effects = crate::plugins::builtin_effects();
        b.open(path.clone());
        b.sync_wrapped();
        let _ = std::fs::remove_file(&path);
        for (k, name) in LINKED_EFFECTS.iter().enumerate() {
            assert_eq!(crate::mixer::slot_name(&b.state.session, at(k).0, at(k).1).as_deref(), Some(*name));
            assert!(b.state.session.plugins.descriptor(key(k)).is_some_and(|d| is_builtin(&d)), "{name} hosted, built in");
            if *name == crate::plugins::TESLA {
                assert_eq!(tesla_frames_of(b.state.session.plugins.state(key(k))), Some(tesla_frames), "Tesla kept its sample");
                continue;
            }
            if *name == crate::plugins::LEMON_DROP {
                assert_eq!(drop_gain_of(b.state.session.plugins.state(key(k))), Some(42), "Lemon Drop kept its state");
                continue;
            }
            assert_eq!(b.state.session.plugins.param_value(key(k), set[k].0), Some(set[k].1), "{name} kept its setting");
        }
    }

    /// Mixer tracks added in the app are saved (before the hidden last track) and come back on reopening; deleting a
    /// track keeps the hidden track last (spec: flp/events/mixer.md "Adding tracks").
    #[test]
    fn added_mixer_tracks_save_and_reopen() {
        let path = std::env::temp_dir().join(format!("openstudio-tracks-{}.flp", std::process::id()));
        let mut a = App::new(None);
        a.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        a.state.session.file = None;
        a.state.session.path = None;
        a.save_to(path.clone());
        let n = a.state.session.project.mixer.len();
        let hidden = a.state.session.project.mixer[n - 1].clone();
        assert!(hidden.routes.is_empty());
        let t = crate::mixer::add_track(&mut a.state, None);
        assert_eq!(usize::from(t), n - 1);
        crate::mixer::add_track(&mut a.state, Some(2));
        a.state.session.project.get_mut().mixer[usize::from(t) + 1].name = Some("Added".into());
        a.state.session.project.get_mut().mixer[usize::from(t) + 1].slot_mix[4] = 0.5;
        assert_eq!(a.state.session.project.mixer.len(), n + 2);
        assert_eq!(a.state.session.project.mixer[n + 1], os_model::MixerTrack { index: (n + 1) as u16, ..hidden.clone() });
        // Deleting an insert keeps the hidden track last.
        crate::mixer::run(&mut a, crate::mixer::MCmd::Delete(1));
        assert_eq!(a.state.session.project.mixer[n + 1], os_model::MixerTrack { index: (n + 1) as u16, ..hidden.clone() });
        a.save_to(path.clone());
        assert!(a.state.message.as_deref().is_some_and(|m| m.starts_with("Saved") && !m.contains("not stored")), "{:?}", a.state.message);
        let mut b = App::new(None);
        b.open(path.clone());
        let _ = std::fs::remove_file(&path);
        assert_eq!(b.state.session.project.mixer, a.state.session.project.mixer);
        assert_eq!(crate::mixer::inserts(&b.state.session), crate::mixer::inserts(&a.state.session));
    }

    /// Trim unused tracks leaves fewer tracks than the file has; the save drops the rest (spec:
    /// ui/mixer-track-commands.md §4, flp/events/mixer.md "Removing tracks").
    #[test]
    fn trimmed_mixer_saves_and_reopens() {
        let path = std::env::temp_dir().join(format!("openstudio-trim-{}.flp", std::process::id()));
        let mut a = App::new(None);
        a.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        a.state.session.file = None;
        a.state.session.path = None;
        a.save_to(path.clone());
        a.state.session.project.get_mut().channels[4].mixer_track = 9;
        a.state.session.project.get_mut().mixer[9].name = Some("Nine".into());
        crate::mixer::run(&mut a, crate::mixer::MCmd::Trim { non_empty: false });
        assert_eq!(crate::mixer::inserts(&a.state.session), 5);
        a.save_to(path.clone());
        assert!(a.state.message.as_deref().is_some_and(|m| m.starts_with("Saved") && !m.contains("not stored")), "{:?}", a.state.message);
        let mut b = App::new(None);
        b.open(path.clone());
        let _ = std::fs::remove_file(&path);
        assert_eq!(b.state.session.project.mixer, a.state.session.project.mixer);
        assert_eq!(b.state.session.project.channels[4].mixer_track, 5);
    }

    /// A hosted native effect writes its own state into its slot (not wrapped); the file reads it back
    /// (spec: flp/events/plugins.md §1).
    #[test]
    fn hosted_native_effects_save_their_state() {
        let path = std::env::temp_dir().join(format!("openstudio-native-{}.flp", std::process::id()));
        let mut a = App::new(None);
        a.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        a.state.session.file = None;
        a.state.session.path = None;
        let desc = Descriptor {
            format: Format::Native,
            id: "native:Fruity Balance".into(),
            name: "Fruity Balance".into(),
            vendor: String::new(),
            version: String::new(),
            features: vec![],
            kind: os_host::Kind::Effect,
            path: "C:/FL/Plugins/Fruity/Effects/Balance/Fruity Balance.dll".into(),
        };
        let plugin: os_host::SharedPlugin = std::sync::Arc::new(std::sync::Mutex::new(NativeStandIn(desc, vec![9, 8, 7, 6], true)));
        a.state.session.project.get_mut().mixer[2].slots[1] = Some(crate::plugins::new_slot("Fruity Balance"));
        a.state.session.plugins.hosted.insert(
            fx_key(2, 1),
            crate::plugins::Hosted::new(plugin),
        );
        a.save_to(path.clone());
        assert!(a.state.message.as_deref().is_some_and(|m| m.starts_with("Saved")), "{:?}", a.state.message);
        let saved = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(saved.mixer[2].slots[1], Some(SlotPlugin { id: os_model::Id::NONE, name: "Fruity Balance".into(), state: vec![9, 8, 7, 6] }));
        assert_eq!(a.state.session.project.mixer[2].slots[1], saved.mixer[2].slots[1]);
    }

    /// An opened project's plugin channels and mixer slots that name installed native plugins are hosted by them,
    /// started from the file's state; a plugin that is not installed is reported and kept; the states of hosted
    /// instances are saved, except for one that did not give the file's state back, whose file bytes are kept
    /// (spec: plugins/fruity-abi.md "Opened projects", "Saved state").
    #[test]
    fn opened_projects_host_their_native_plugins() {
        let path = std::env::temp_dir().join(format!("openstudio-native-open-{}.flp", std::process::id()));
        let mut a = App::new(None);
        a.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        a.state.session.file = None;
        a.state.session.path = None;
        a.state.session.plugins.available = vec![native_stand_in("Harmor", os_host::Kind::Instrument, true)];
        a.state.session.plugins.effects = vec![native_stand_in("Fruity Balance", os_host::Kind::Effect, true), native_stand_in("Gross Beat", os_host::Kind::Effect, false)];
        let harmor = crate::projects::add_plugin_channel(a.state.session.project.get_mut(), "Harmor");
        a.state.session.project.get_mut().channels[harmor].plugin_state = Some(vec![1, 2, 3]);
        let sytrus = crate::projects::add_plugin_channel(a.state.session.project.get_mut(), "Sytrus");
        a.state.session.project.get_mut().channels[sytrus].plugin_state = Some(vec![4]);
        a.state.session.project.get_mut().mixer[2].slots[1] = Some(SlotPlugin { id: os_model::Id::NONE, name: "Fruity Balance".into(), state: vec![5, 6] });
        a.state.session.project.get_mut().mixer[3].slots[0] = Some(SlotPlugin { id: os_model::Id::NONE, name: "Gross Beat".into(), state: vec![7, 8, 9] });
        // A file id in another case than the folder name (files store `Fruity slicer`).
        a.state.session.project.get_mut().mixer[4].slots[0] = Some(SlotPlugin { id: os_model::Id::NONE, name: "fruity balance".into(), state: vec![] });
        let (balance, gross, lower) = (fx_key(2, 1), fx_key(3, 0), fx_key(4, 0));

        a.host_native();
        assert!(a.state.session.hosted_rows.contains(&harmor));
        assert_eq!(a.state.session.plugins.state(harmor), Some(vec![1, 2, 3]));
        assert_eq!(a.state.session.plugins.state(balance), Some(vec![5, 6]));
        assert!(a.state.session.plugins.hosted.contains_key(&lower) && !a.state.session.plugins.hosted[&lower].keep_file_state);
        assert_eq!(a.state.session.plugins.state(gross), Some(vec![7, 8, 9, 0xEE]));
        assert!(!a.state.session.plugins.hosted[&harmor].keep_file_state && !a.state.session.plugins.hosted[&balance].keep_file_state);
        assert!(a.state.session.plugins.hosted[&gross].keep_file_state);
        assert!(!a.state.session.hosted_rows.contains(&sytrus) && a.state.session.plugins.native_failed.contains(&sytrus));
        let msg = a.state.message.clone().unwrap_or_default();
        assert!(msg.contains("Harmor") && msg.contains("Gross Beat") && msg.contains("not installed") && msg.contains("Sytrus"), "{msg}");
        // Another tick changes nothing.
        let n = a.state.session.plugins.hosted.len();
        a.host_native();
        assert_eq!((a.state.session.plugins.hosted.len(), a.state.session.hosted_rows.len()), (n, 1));

        // Saved: hosted instances write their state; the one that did not round-trip keeps the file's bytes.
        assert!(a.state.session.plugins.set_state(harmor, &[11, 12]) && a.state.session.plugins.set_state(balance, &[13]) && a.state.session.plugins.set_state(gross, &[14]));
        a.save_to(path.clone());
        assert!(a.state.message.as_deref().is_some_and(|m| m.starts_with("Saved")), "{:?}", a.state.message);
        let saved = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!((saved.channels[harmor].plugin.as_deref(), saved.channels[harmor].plugin_state.as_deref()), (Some("Harmor"), Some(&[11, 12][..])));
        assert_eq!((saved.channels[sytrus].plugin.as_deref(), saved.channels[sytrus].plugin_state.as_deref()), (Some("Sytrus"), Some(&[4][..])));
        assert_eq!(saved.mixer[2].slots[1], Some(SlotPlugin { id: os_model::Id::NONE, name: "Fruity Balance".into(), state: vec![13] }));
        assert_eq!(saved.mixer[3].slots[0], Some(SlotPlugin { id: os_model::Id::NONE, name: "Gross Beat".into(), state: vec![7, 8, 9] }));
        // The file's own spelling of the id is kept; the instance's state (its defaults) is written.
        assert_eq!(saved.mixer[4].slots[0], Some(SlotPlugin { id: os_model::Id::NONE, name: "fruity balance".into(), state: vec![0, 0] }));
    }

    /// The hidden "real native plugins" toggle: a 3x Osc channel and a parametric EQ slot played by the Open
    /// recreations move to the installed plugins, started from the file's state, and back again; the settings stay
    /// in the project across each move (spec: plugins/fruity-abi.md "Opened projects").
    #[test]
    #[cfg(all(feature = "stock-parametric-eq-2", feature = "stock-trio"))]
    fn real_native_plugins_replace_the_open_recreations_and_back() {
        use crate::osc3::{FILE_NAME as OSC3_FILE, LEMON_TRIO};
        use crate::plugins::EQ_FILE_NAME;
        NATIVE_FIRST.with(|n| n.set(Some(false)));
        let mut a = App::new(None);
        a.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        a.state.session.plugins.available = crate::plugins::builtin();
        a.state.session.plugins.available.push(native_stand_in(OSC3_FILE, os_host::Kind::Instrument, true));
        a.state.session.plugins.effects = crate::plugins::builtin_effects();
        a.state.session.plugins.effects.push(native_stand_in(EQ_FILE_NAME, os_host::Kind::Effect, true));
        let row = crate::projects::add_plugin_channel(a.state.session.project.get_mut(), LEMON_TRIO);
        let osc_file = lemon_3x_osc::params::default_state();
        a.state.session.project.get_mut().channels[row].plugin_state = Some(osc_file.clone());
        let mut eq_file = lemon_parametric_eq_2::params::default_state();
        lemon_parametric_eq_2::params::set_field(&mut eq_file, 1, -600);
        a.state.session.project.get_mut().mixer[2].slots[1] = Some(SlotPlugin { id: os_model::Id::NONE, name: EQ_FILE_NAME.into(), state: eq_file.clone() });
        let slot = fx_key(2, 1);
        let builtin_at = |a: &App, key| a.state.session.plugins.descriptor(key).is_some_and(|d| is_builtin(&d));
        let native_at = |a: &App, key| a.state.session.plugins.descriptor(key).is_some_and(|d| d.format == Format::Native);

        // Off: the recreations play them, and the native pass leaves them alone.
        a.sync_osc3();
        a.sync_effects();
        a.host_native();
        assert!(builtin_at(&a, row) && builtin_at(&a, slot));

        // On: switched to the installed plugins, started from the file's bytes.
        NATIVE_FIRST.with(|n| n.set(Some(true)));
        assert_eq!(a.swap_builtin_overrides(), 2);
        assert!(!a.state.session.plugins.hosted.contains_key(&row) && !a.state.session.plugins.hosted.contains_key(&slot) && !a.state.session.hosted_rows.contains(&row));
        a.sync_osc3();
        a.sync_effects();
        assert!(!a.state.session.plugins.hosted.contains_key(&row), "the recreation stays out");
        a.host_native();
        assert!(native_at(&a, row) && native_at(&a, slot) && a.state.session.hosted_rows.contains(&row));
        assert_eq!(a.state.session.plugins.state(row).as_deref(), a.state.session.project.channels[row].plugin_state.as_deref());
        assert_eq!(a.state.session.plugins.state(slot), Some(eq_file.clone()));

        // Off again: back to the recreations, the settings still in the project.
        NATIVE_FIRST.with(|n| n.set(Some(false)));
        assert_eq!(a.swap_builtin_overrides(), 2);
        assert_eq!(a.state.session.project.mixer[2].slots[1].as_ref().map(|s| s.state.clone()), Some(eq_file.clone()));
        a.sync_osc3();
        a.sync_effects();
        assert!(builtin_at(&a, row) && builtin_at(&a, slot));
        assert_eq!(a.state.session.plugins.eq_file_state(slot), Some(eq_file));
        assert_eq!(a.state.session.project.channels[row].plugin.as_deref(), Some(OSC3_FILE));
        let _ = osc_file;
        NATIVE_FIRST.with(|n| n.set(None));
    }

    /// While a plugin scan runs, a native plugin that is not listed yet is neither hosted nor given up on; after the
    /// scan it is looked for again.
    #[test]
    fn native_plugins_wait_for_the_scan() {
        let mut a = App::new(None);
        a.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        a.state.session.plugins.available = Vec::new();
        let row = crate::projects::add_plugin_channel(a.state.session.project.get_mut(), "Harmor");
        a.sweep = Some(crate::plugin_scan::Sweep::idle());
        a.host_native();
        assert!(!a.state.session.hosted_rows.contains(&row) && a.state.session.plugins.native_failed.is_empty());
        a.sweep = None;
        a.state.session.plugins.available = vec![native_stand_in("Harmor", os_host::Kind::Instrument, true)];
        a.host_native();
        assert!(a.state.session.hosted_rows.contains(&row));
    }

    /// A wrapped plugin found only as its copy in another format starts from its defaults (the saved state is the other
    /// format's) and the hint bar says so, next to the plugins that are not found; one found in its own format gets the
    /// saved state. Until the user edits it, the copy leaves the file's wrapper byte for byte, also when something else
    /// (playback, an automation clip) changed its state; once edited, it is saved as what it is, and from then on as any
    /// hosted plugin. The built-in Lemon Parametric EQ (a CLAP plugin) stands in.
    #[test]
    #[cfg(feature = "stock-parametric-eq-2")]
    fn copies_in_another_format_start_from_their_defaults() {
        WRAP_BUILTINS.with(|w| w.set(true));
        let mut a = App::new(None);
        a.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        a.state.session.plugins.available = crate::plugins::builtin();
        a.state.session.plugins.effects = crate::plugins::builtin_effects();
        let mut fl = lemon_parametric_eq_2::params::default_state();
        lemon_parametric_eq_2::params::set_field(&mut fl, 1, -600);
        let eq = lemon_parametric_eq_2::params::plugin_state_from_fl(&fl);
        let saved_as = |format: &str, name: &str, file: &str| PluginRef {
            format: format.into(),
            id: if format == "vst2" { "4C455132".into() } else { String::new() },
            name: name.into(),
            vendor: String::new(),
            path: format!("C:/Plugins/{file}"),
            state: eq.clone(),
        };
        let slot = |r: &PluginRef| Some(SlotPlugin { id: os_model::Id::NONE, name: WRAPPER.into(), state: wrapper::encode(None, r) });
        a.state.session.project.get_mut().mixer[2].slots[0] = slot(&saved_as("vst2", LEMON_EQ, "Lemon EQ.dll"));
        a.state.session.project.get_mut().mixer[3].slots[0] = slot(&saved_as("clap", LEMON_EQ, "Lemon EQ.clap"));
        let row = crate::projects::add_plugin_channel(a.state.session.project.get_mut(), WRAPPER);
        a.state.session.project.get_mut().channels[row].plugin_state = Some(wrapper::encode(None, &saved_as("vst2", "Nowhere", "Nowhere.dll")));
        a.sync_wrapped();

        let fresh = fx_key(4, 0);
        a.state.session.plugins.create_effect(4, 0, a.state.session.plugins.effect_index(LEMON_EQ).unwrap(), None).unwrap();
        let defaults = a.state.session.plugins.eq_file_state(fresh);
        assert_ne!(defaults, Some(fl.clone()));
        assert_eq!(a.state.session.plugins.eq_file_state(fx_key(2, 0)), defaults, "the VST2 copy's state is not given to the CLAP plugin");
        assert_eq!(a.state.session.plugins.eq_file_state(fx_key(3, 0)), Some(fl), "the CLAP state is");
        assert!(a.state.session.plugins.wrap_failed.contains(&row));
        let told = format!(
            "Plugins not found (kept in the project): Nowhere; Played by a copy in another format, from its default settings \
             (saved as the original until edited): {LEMON_EQ} (VST2 as CLAP)"
        );
        assert_eq!(a.state.message.as_deref(), Some(told.as_str()));
        a.state.session.plugins.drop_key(fresh);
        a.state.session.project.get_mut().mixer[4].slots[0] = None;

        // Saved untouched: the VST2 plugin's wrapper is written as the file had it; the CLAP plugin as itself.
        let path = std::env::temp_dir().join(format!("openstudio-stand-in-{}.flp", std::process::id()));
        let reload = |path: &std::path::Path| os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(path).unwrap()).unwrap()).unwrap();
        let named = a.state.session.project.mixer[2].slots[0].clone();
        a.save_to(path.clone());
        assert!(a.state.message.as_deref().is_some_and(|m| m.starts_with("Saved")), "{:?}", a.state.message);
        let saved = reload(&path);
        assert_eq!(saved.mixer[2].slots[0], named, "byte for byte");
        assert_eq!(wrapped_slot(saved.mixer[3].slots[0].as_ref().unwrap()).unwrap().state, a.state.session.plugins.state(fx_key(3, 0)).unwrap());
        // Saved again, still untouched: the same.
        a.save_to(path.clone());
        assert_eq!(reload(&path).mixer[2].slots[0], named);

        // Its state changed by something other than the user (as an automation clip does): still the file's wrapper.
        let start = a.state.session.plugins.state(fx_key(2, 0)).unwrap();
        assert!(a.state.session.plugins.set_state(fx_key(2, 0), &eq));
        a.save_to(path.clone());
        assert_eq!(reload(&path).mixer[2].slots[0], named, "not the user's edit");

        // Edited by the user (Paste value in the parameter menu): saved as the copy that plays it, with its own state.
        // (The stand-in's file is this program, so the saved path names it; a plugin file's extension would give the
        // format.)
        let exe = std::env::current_exe().unwrap();
        assert_eq!(a.browse_params(fx_key(2, 0)), None);
        a.state.value_clip = Some(0.75);
        let paste = crate::playlist::Cmd::Param(crate::params::PCmd::Paste(fx_key(2, 0), 0));
        assert_eq!(crate::playlist::run(&mut a, paste), None);
        a.state.session.plugins.hosted[&fx_key(2, 0)].plugin.lock().unwrap().idle();
        a.save_to(path.clone());
        let r = wrapped_slot(reload(&path).mixer[2].slots[0].as_ref().unwrap()).unwrap();
        assert_eq!((std::path::PathBuf::from(&r.path), r.id.as_str()), (exe.clone(), ""));
        assert_eq!(Some(r.state), a.state.session.plugins.state(fx_key(2, 0)));
        // From then on the project names the copy: back at its first state, it is saved as it is.
        assert!(a.state.session.plugins.set_state(fx_key(2, 0), &start));
        a.save_to(path.clone());
        let r = wrapped_slot(reload(&path).mixer[2].slots[0].as_ref().unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!((std::path::PathBuf::from(&r.path), r.state), (exe, start));
    }

    /// Zest saved in a project comes back built in, with a setting it was given.
    #[test]
    fn zest_saves_and_reopens() {
        use crate::app::App;
        let path = std::env::temp_dir().join(format!("lemon-zest-{}.flp", std::process::id()));
        let mut a = App::new(None);
        a.state.session.plugins.available = crate::plugins::builtin();
        a.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        let row = 0;
        a.state.session.project.get_mut().channels[row].kind = os_model::ChannelKind::Plugin;
        a.state.session.project.get_mut().channels[row].plugin = Some("Zest".into());
        a.state.session.plugins.create_with(row, a.state.session.plugins.index_of("Zest").unwrap(), None).unwrap();
        let p = a.state.session.plugins.param_infos(row).into_iter().find(|p| !p.read_only && !p.bypass && p.max > p.min).unwrap();
        let n = if (p.default - 0.3).abs() > 0.1 { 0.3 } else { 0.8 };
        assert!(a.state.session.plugins.set_param_named(row, &p.name, p.min + n * (p.max - p.min)));
        a.state.session.plugins.hosted[&row].plugin.lock().unwrap().idle();
        let v = a.state.session.plugins.param_value(row, p.id).unwrap();
        a.save_to(path.clone());
        assert!(a.state.message.as_deref().is_some_and(|m| m.starts_with("Saved") && !m.contains("not stored")), "{:?}", a.state.message);

        let mut b = App::new(None);
        b.state.session.plugins.available = crate::plugins::builtin();
        b.open(path.clone());
        b.sync_wrapped();
        let _ = std::fs::remove_file(&path);
        assert!(b.state.session.plugins.descriptor(row).is_some_and(|d| d.name == "Zest" && is_builtin(&d)), "Zest hosted, built in");
        assert_eq!(b.state.session.plugins.param_value(row, p.id), Some(v), "Zest kept its setting");
    }
}
