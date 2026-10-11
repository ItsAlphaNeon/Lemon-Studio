//! Hosted plugins in project files: what a save writes for plugin channels and mixer slots played by third-party
//! plugins (CLAP, VST3), and which plugin an opened project's wrapped channels and slots get.
//!
//! A third-party plugin is stored the way the project format stores every such plugin: as the wrapper plugin, whose
//! state names the plugin (format, class id, name, vendor, file) and holds the plugin's own saved state. Plugins
//! compiled into OpenStudio keep their own mapping (Open Sampler = sampler channels, Open Parametric EQ 2 = the
//! file's parametric EQ). Native plugins keep their own state format: a hosted native instance writes its state into
//! its channel or slot as it is, and native plugins that are not hosted keep the bytes the file had.
//!
// spec: flp/events/plugins.md (§4 the wrapper state, §6 what OpenStudio writes)

use os_flp::wrapper::{self, WRAPPER};
use os_host::{Descriptor, Format};
use os_model::{Channel, ChannelKind, PluginRef, Project, SlotPlugin};

use crate::plugins::{fx_of, Available, Plugins};

/// A plugin compiled into OpenStudio (its library is this program).
pub fn is_builtin(d: &Descriptor) -> bool {
    std::env::current_exe().is_ok_and(|exe| exe == d.path)
}

#[cfg(test)]
thread_local! {
    /// Tests: the built-in Open plugins stand in for third-party plugins.
    pub static WRAP_BUILTINS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether a hosted plugin is saved inside the wrapper: CLAP and VST3 plugins from plugin files.
pub fn is_wrapped(d: &Descriptor) -> bool {
    #[cfg(test)]
    if WRAP_BUILTINS.with(|w| w.get()) {
        return matches!(d.format, Format::Clap | Format::Vst3);
    }
    matches!(d.format, Format::Clap | Format::Vst3) && !is_builtin(d)
}

/// The project-file identity of a hosted plugin with its saved state.
pub fn plugin_ref(d: &Descriptor, state: Vec<u8>) -> PluginRef {
    PluginRef {
        format: d.format.as_str().into(),
        // A CLAP id has no place in the wrapper state (spec §4): such plugins are found by file and name.
        id: if d.format == Format::Vst3 { d.id.clone() } else { String::new() },
        name: d.name.clone(),
        vendor: d.vendor.clone(),
        path: d.path.to_string_lossy().into_owned(),
        state,
    }
}

/// Writes every hosted third-party plugin into `proj` (the copy a save writes): its channel or slot becomes a wrapped
/// plugin holding the plugin's identity and current state. A channel or slot the file already held as a wrapped
/// plugin keeps the wrapper records OpenStudio does not edit.
pub fn store_hosted(proj: &mut Project, plugins: &Plugins) {
    for key in plugins.hosted.keys().copied().collect::<Vec<_>>() {
        let Some(d) = plugins.descriptor(key) else { continue };
        if d.format == Format::Native {
            // The plugin's own state stream is what the file stores for it (spec §1, event 213).
            if let Some(state) = plugins.state(key) {
                store_native(proj, key, state);
            }
            continue;
        }
        if !is_wrapped(&d) {
            continue;
        }
        let state = plugins.state(key).unwrap_or_default();
        let r = plugin_ref(&d, state);
        match fx_of(key) {
            None => {
                let Some(ch) = proj.channels.get_mut(key).filter(|c| c.kind == ChannelKind::Plugin) else { continue };
                let base = ch.plugin_state.as_deref().filter(|_| ch.plugin.as_deref() == Some(WRAPPER));
                ch.plugin_state = Some(wrapper::encode(base, &r));
                ch.plugin = Some(WRAPPER.into());
            }
            Some((t, k)) => {
                let Some(slot) = proj.mixer.iter_mut().find(|m| m.index == t).and_then(|m| m.slots[usize::from(k)].as_mut()) else { continue };
                let base = (slot.name == WRAPPER).then_some(slot.state.as_slice());
                slot.state = wrapper::encode(base, &r);
                slot.name = WRAPPER.into();
            }
        }
    }
}

/// A hosted native plugin's state into its channel (a plugin channel) or mixer slot.
fn store_native(proj: &mut Project, key: usize, state: Vec<u8>) {
    match fx_of(key) {
        None => {
            if let Some(ch) = proj.channels.get_mut(key).filter(|c| c.kind == ChannelKind::Plugin) {
                ch.plugin_state = Some(state);
            }
        }
        Some((t, k)) => {
            if let Some(slot) = proj.mixer.iter_mut().find(|m| m.index == t).and_then(|m| m.slots[usize::from(k)].as_mut()) {
                slot.state = state;
            }
        }
    }
}

/// After a save: the model takes the plugin names and states the file now holds (from the saved copy `saved`).
pub fn adopt_saved(model: &mut Project, saved: &Project) {
    for (c, s) in model.channels.iter_mut().zip(&saved.channels) {
        c.plugin.clone_from(&s.plugin);
        c.plugin_state.clone_from(&s.plugin_state);
    }
    for (m, s) in model.mixer.iter_mut().zip(&saved.mixer) {
        m.slots.clone_from(&s.slots);
    }
}

/// The wrapped plugin a channel holds (`None` for other channels).
pub fn wrapped_channel(ch: &Channel) -> Option<PluginRef> {
    if ch.kind != ChannelKind::Plugin || ch.plugin.as_deref() != Some(WRAPPER) {
        return None;
    }
    wrapper::decode(ch.plugin_state.as_deref()?)
}

/// The wrapped plugin a mixer slot holds.
pub fn wrapped_slot(s: &SlotPlugin) -> Option<PluginRef> {
    (s.name == WRAPPER).then(|| wrapper::decode(&s.state)).flatten()
}

fn same_file(a: &std::path::Path, b: &str) -> bool {
    let b = b.rsplit(['\\', '/']).next().unwrap_or(b);
    a.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(b))
}

/// The plugin of `list` a wrapped plugin names: the same format and class id, else the same file and name, else the
/// same format and name, else the same name.
pub fn find(list: &[Available], r: &PluginRef) -> Option<usize> {
    let format = Format::parse(&r.format);
    let tests: [&dyn Fn(&Descriptor) -> bool; 4] = [
        &|d| !r.id.is_empty() && Some(d.format) == format && d.id.eq_ignore_ascii_case(&r.id),
        &|d| !r.path.is_empty() && same_file(&d.path, &r.path) && d.name == r.name,
        &|d| Some(d.format) == format && d.name == r.name,
        &|d| d.name == r.name,
    ];
    tests.iter().find_map(|t| list.iter().position(|a| t(&a.desc)))
}

/// What a channel's plugin is called on screen: a wrapped plugin's own name, else the plugin name.
pub fn plugin_title(ch: &Channel) -> Option<String> {
    wrapped_channel(ch).map(|r| r.name).filter(|n| !n.is_empty()).or_else(|| ch.plugin.as_deref().map(|n| crate::osc3::label(n).to_string()))
}

/// What a mixer slot shows: a wrapped plugin's own name, else the slot's label (spec: ui/mixer.md §7).
pub fn slot_title(s: &SlotPlugin) -> String {
    match wrapped_slot(s).map(|r| r.name).filter(|n| !n.is_empty()) {
        Some(n) => n,
        None => crate::plugins::slot_label(&s.name).to_string(),
    }
}

impl crate::app::App {
    /// Hosts the wrapped plugins of an opened project (spec: flp/events/plugins.md §4): each wrapped channel and slot
    /// whose plugin is found starts from the state the file holds. A plugin that is not found keeps its saved data
    /// (written back unchanged) and is not looked for again.
    pub(crate) fn sync_wrapped(&mut self) {
        use crate::plugins::fx_key;
        let mut added = (false, false);
        let mut missing = Vec::new();
        for row in 0..self.state.project.channels.len() {
            if self.state.hosted_rows.contains(&row) || self.wrap_failed.contains(&row) {
                continue;
            }
            let Some(r) = wrapped_channel(&self.state.project.channels[row]) else { continue };
            let made = find(&self.plugins.available, &r).map(|i| self.plugins.create_with(row, i, Some(&r.state)));
            match made {
                Some(Ok(_)) => {
                    self.state.hosted_rows.push(row);
                    added.0 = true;
                }
                Some(Err(e)) => {
                    self.wrap_failed.push(row);
                    missing.push(format!("{} ({e})", r.name));
                }
                None => {
                    self.wrap_failed.push(row);
                    missing.push(r.name);
                }
            }
        }
        let slots: Vec<(u16, u8, PluginRef)> = self
            .state
            .project
            .mixer
            .iter()
            .flat_map(|m| m.slots.iter().enumerate().filter_map(move |(k, s)| Some((m.index, k as u8, wrapped_slot(s.as_ref()?)?))))
            .filter(|&(t, k, _)| !self.plugins.hosted.contains_key(&fx_key(t, k)) && !self.wrap_failed.contains(&fx_key(t, k)))
            .collect();
        for (t, k, r) in slots {
            match find(&self.plugins.effects, &r).map(|i| self.plugins.create_effect(t, k, i, Some(&r.state))) {
                Some(Ok(_)) => added.1 = true,
                Some(Err(e)) => {
                    self.wrap_failed.push(fx_key(t, k));
                    missing.push(format!("{} ({e})", r.name));
                }
                None => {
                    self.wrap_failed.push(fx_key(t, k));
                    missing.push(r.name);
                }
            }
        }
        if added.0 {
            self.transport.set_instruments(self.plugins.instruments());
        }
        if added.1 {
            self.transport.set_effects(self.plugins.effects());
        }
        if !missing.is_empty() {
            missing.dedup();
            self.state.message = Some(format!("Plugins not found (kept in the project): {}", missing.join(", ")));
        }
        if added.0 || added.1 || !missing.is_empty() {
            self.state.piano.dirty = true;
            self.invalidate();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::plugins::{fx_key, OPEN_EQ, OPEN_SAMPLER};

    /// A hosted plugin channel and a mixer slot plugin are saved as wrapped plugins holding their identity and state,
    /// and the reopened project hosts them again with that state; a second save keeps working on the saved file
    /// (spec: flp/events/plugins.md). The built-in Open plugins stand in for third-party plugins.
    #[test]
    fn hosted_plugins_save_and_reopen() {
        WRAP_BUILTINS.with(|w| w.set(true));
        let path = std::env::temp_dir().join(format!("openstudio-wrapped-{}.flp", std::process::id()));
        let mut a = App::new(None);
        a.state.project = crate::app::test_project();
        a.state.file = None;
        a.state.path = None;
        a.plugins.available = crate::plugins::builtin();
        a.plugins.effects = crate::plugins::builtin_effects();
        let row = crate::app::add_plugin_channel(&mut a.state.project, OPEN_SAMPLER);
        a.plugins.create(row, a.plugins.index_of(OPEN_SAMPLER).unwrap()).unwrap();
        a.state.hosted_rows.push(row);
        assert!(a.plugins.load_sample(row, "C:/samples/one.wav"));
        let mut fl = open_parametric_eq_2::params::default_state();
        open_parametric_eq_2::params::set_field(&mut fl, 1, -600);
        let eq = open_parametric_eq_2::params::plugin_state_from_fl(&fl);
        a.state.project.mixer[3].slots[2] = Some(SlotPlugin { name: OPEN_EQ.into(), state: Vec::new() });
        a.plugins.create_effect(3, 2, a.plugins.effect_index(OPEN_EQ).unwrap(), Some(&eq)).unwrap();
        a.save_to(path.clone());
        assert!(a.state.message.as_deref().is_some_and(|m| m.starts_with("Saved") && !m.contains("not stored")), "{:?}", a.state.message);

        // The file: a wrapped plugin channel and a wrapped slot.
        let file = os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap();
        let saved = os_flp::project::load(&file).unwrap();
        let ch = &saved.channels[row];
        assert_eq!((ch.kind, ch.plugin.as_deref()), (ChannelKind::Plugin, Some(WRAPPER)));
        let r = wrapped_channel(ch).unwrap();
        // (A plugin file's extension gives the format: `.clap` / `.vst3`; the stand-in's file is this program.)
        assert_eq!(r.name, OPEN_SAMPLER);
        assert_eq!(std::path::PathBuf::from(&r.path), std::env::current_exe().unwrap());
        assert_eq!(Some(r.state.clone()), a.plugins.state(row));
        let s = wrapped_slot(saved.mixer[3].slots[2].as_ref().unwrap()).unwrap();
        assert_eq!(s.name, OPEN_EQ);
        assert_eq!(slot_title(saved.mixer[3].slots[2].as_ref().unwrap()), OPEN_EQ);
        // The model now holds what the file holds.
        assert_eq!(a.state.project.channels[row].plugin.as_deref(), Some(WRAPPER));

        // Reopened: both plugins are hosted again with their state.
        let mut b = App::new(None);
        b.plugins.available = crate::plugins::builtin();
        b.plugins.effects = crate::plugins::builtin_effects();
        b.open(path.clone());
        b.sync_wrapped();
        assert!(b.state.hosted_rows.contains(&row), "{:?}", b.state.message);
        assert_eq!(b.plugins.sampler_path(row).as_deref(), Some("C:/samples/one.wav"));
        assert_eq!(b.plugins.eq_file_state(fx_key(3, 2)), Some(fl));
        assert_eq!(crate::mixer::slot_name(&b.state, 3, 2).as_deref(), Some(OPEN_EQ));

        // Saved again (onto the opened file): the new state is written into the same wrapper.
        assert!(b.plugins.load_sample(row, "C:/samples/two.wav"));
        b.save_to(path.clone());
        assert!(b.state.message.as_deref().is_some_and(|m| m.starts_with("Saved")), "{:?}", b.state.message);
        let again = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        let r2 = wrapped_channel(&again.channels[row]).unwrap();
        assert_eq!(Some(r2.state), b.plugins.state(row));
        assert_eq!(again.mixer[3].slots[2], saved.mixer[3].slots[2]);
    }

    /// Mixer tracks added in the app are saved (before the hidden last track) and come back on reopening; deleting a
    /// track keeps the hidden track last (spec: flp/events/mixer.md "Adding tracks").
    #[test]
    fn added_mixer_tracks_save_and_reopen() {
        let path = std::env::temp_dir().join(format!("openstudio-tracks-{}.flp", std::process::id()));
        let mut a = App::new(None);
        a.state.project = crate::app::test_project();
        a.state.file = None;
        a.state.path = None;
        a.save_to(path.clone());
        let n = a.state.project.mixer.len();
        let hidden = a.state.project.mixer[n - 1].clone();
        assert!(hidden.routes.is_empty());
        let t = crate::mixer::add_track(&mut a.state, None);
        assert_eq!(usize::from(t), n - 1);
        crate::mixer::add_track(&mut a.state, Some(2));
        a.state.project.mixer[usize::from(t) + 1].name = Some("Added".into());
        a.state.project.mixer[usize::from(t) + 1].slot_mix[4] = 0.5;
        assert_eq!(a.state.project.mixer.len(), n + 2);
        assert_eq!(a.state.project.mixer[n + 1], os_model::MixerTrack { index: (n + 1) as u16, ..hidden.clone() });
        // Deleting an insert keeps the hidden track last.
        crate::mixer::run(&mut a, crate::mixer::MCmd::Delete(1));
        assert_eq!(a.state.project.mixer[n + 1], os_model::MixerTrack { index: (n + 1) as u16, ..hidden.clone() });
        a.save_to(path.clone());
        assert!(a.state.message.as_deref().is_some_and(|m| m.starts_with("Saved") && !m.contains("not stored")), "{:?}", a.state.message);
        let mut b = App::new(None);
        b.open(path.clone());
        let _ = std::fs::remove_file(&path);
        assert_eq!(b.state.project.mixer, a.state.project.mixer);
        assert_eq!(crate::mixer::inserts(&b.state), crate::mixer::inserts(&a.state));
    }

    /// A stand-in native plugin: its state is four bytes.
    struct NativeStandIn(Descriptor, Vec<u8>);
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
            true
        }
    }

    /// A hosted native effect writes its own state into its slot (not wrapped); the file reads it back
    /// (spec: flp/events/plugins.md §1).
    #[test]
    fn hosted_native_effects_save_their_state() {
        let path = std::env::temp_dir().join(format!("openstudio-native-{}.flp", std::process::id()));
        let mut a = App::new(None);
        a.state.project = crate::app::test_project();
        a.state.file = None;
        a.state.path = None;
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
        let plugin: os_host::SharedPlugin = std::sync::Arc::new(std::sync::Mutex::new(NativeStandIn(desc, vec![9, 8, 7, 6])));
        a.state.project.mixer[2].slots[1] = Some(crate::plugins::new_slot("Fruity Balance"));
        a.plugins.hosted.insert(
            fx_key(2, 1),
            crate::plugins::Hosted { plugin, #[cfg(windows)] window: None, #[cfg(windows)] link: None },
        );
        a.save_to(path.clone());
        assert!(a.state.message.as_deref().is_some_and(|m| m.starts_with("Saved")), "{:?}", a.state.message);
        let saved = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(saved.mixer[2].slots[1], Some(SlotPlugin { name: "Fruity Balance".into(), state: vec![9, 8, 7, 6] }));
        assert_eq!(a.state.project.mixer[2].slots[1], saved.mixer[2].slots[1]);
    }

    /// A wrapped plugin is found by class id, else by file and name, else by name.
    #[test]
    fn wrapped_plugins_are_found_by_identity() {
        let list = crate::plugins::builtin_effects();
        let mut r = plugin_ref(&list[0].desc, vec![]);
        assert_eq!(find(&list, &r), Some(0));
        r.path = "D:/elsewhere/Other.clap".into();
        assert_eq!(find(&list, &r), Some(0), "same name, moved file");
        r.name = "Something else".into();
        assert_eq!(find(&list, &r), None);
    }
}
