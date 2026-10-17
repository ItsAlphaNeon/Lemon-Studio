// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Hosted plugins in project files: what a save writes for plugin channels and mixer slots played by third-party
//! plugins (CLAP, VST3, VST2), and which plugin an opened project's wrapped channels and slots get.
//!
//! A third-party plugin is stored the way the project format stores every such plugin: as the wrapper plugin, whose
//! state names the plugin (format, class id, name, vendor, file) and holds the plugin's own saved state. Plugins
//! compiled into Lemon Studio keep their own mapping (Lemon Sampler = sampler channels, Lemon Parametric EQ = the
//! file's parametric EQ). Native plugins keep their own state format: a hosted native instance writes its state into
//! its channel or slot as it is, and native plugins that are not hosted keep the bytes the file had.
//!
// spec: flp/events/plugins.md (§4 the wrapper state, §6 what Lemon Studio writes)

use os_flp::wrapper::{self, WRAPPER};
use os_host::{Descriptor, Format};
use os_model::{Channel, ChannelKind, PluginRef, Project, SlotPlugin};

use crate::plugins::{format_name, fx_of, Available, Plugins};

/// A plugin compiled into Lemon Studio (its library is this program).
pub fn is_builtin(d: &Descriptor) -> bool {
    std::env::current_exe().is_ok_and(|exe| exe == d.path)
}

#[cfg(test)]
thread_local! {
    /// Tests: the built-in Lemon plugins stand in for third-party plugins.
    pub static WRAP_BUILTINS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
thread_local! {
    /// Tests: the "real native plugins instead of Lemon recreations" preference for this thread, without touching the
    /// process environment other tests read.
    pub static NATIVE_FIRST: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Whether an opened project's native plugins are played by the installed plugins even where a built-in Open
/// recreation would take them (the hidden settings toggle; spec: plugins/fruity-abi.md "Opened projects").
pub fn native_first() -> bool {
    #[cfg(test)]
    if let Some(v) = NATIVE_FIRST.with(|n| n.get()) {
        return v;
    }
    os_native::prefer_native()
}

/// Whether a channel or slot holding file id `id` is one a built-in Lemon recreation plays (3x Osc, the parametric EQ).
pub fn has_recreation(id: &str) -> bool {
    id.eq_ignore_ascii_case(crate::osc3::FILE_NAME) || id.eq_ignore_ascii_case(crate::plugins::EQ_FILE_NAME)
}

/// Whether a hosted plugin is saved inside the wrapper: CLAP, VST3 and VST2 plugins from plugin files.
pub fn is_wrapped(d: &Descriptor) -> bool {
    #[cfg(test)]
    if WRAP_BUILTINS.with(|w| w.get()) {
        return matches!(d.format, Format::Clap | Format::Vst3 | Format::Vst2);
    }
    matches!(d.format, Format::Clap | Format::Vst3 | Format::Vst2) && !is_builtin(d)
}

/// The project-file identity of a hosted plugin with its saved state.
pub fn plugin_ref(d: &Descriptor, state: Vec<u8>) -> PluginRef {
    PluginRef {
        format: d.format.as_str().into(),
        // A CLAP id has no place in the wrapper state (spec §4): such plugins are found by file and name.
        id: if matches!(d.format, Format::Vst3 | Format::Vst2) { d.id.clone() } else { String::new() },
        name: d.name.clone(),
        vendor: d.vendor.clone(),
        path: d.path.to_string_lossy().into_owned(),
        state,
    }
}

/// Writes every hosted third-party plugin into `proj` (the copy a save writes): its channel or slot becomes a wrapped
/// plugin holding the plugin's identity and current state. A channel or slot the file already held as a wrapped
/// plugin keeps the wrapper records Lemon Studio does not edit. A copy in another format that stands in for the plugin
/// the file names, and that the user has not edited, leaves the file's wrapper as it is ([`untouched_stand_in`]).
pub fn store_hosted(proj: &mut Project, plugins: &Plugins) {
    for key in plugins.hosted.keys().copied().collect::<Vec<_>>() {
        let Some(d) = plugins.descriptor(key) else { continue };
        if d.format == Format::Native {
            // The plugin's own state stream is what the file stores for it (spec §1, event 213). An instance that
            // did not give the file's state back byte for byte leaves the file's bytes alone (`sync_native`).
            if plugins.hosted.get(&key).is_some_and(|h| h.keep_file_state) {
                continue;
            }
            if let Some(state) = plugins.state(key) {
                store_native(proj, key, state);
            }
            continue;
        }
        if !is_wrapped(&d) {
            continue;
        }
        if untouched_stand_in(proj, plugins, key, &d) {
            continue;
        }
        let r = plugin_ref(&d, plugins.state(key).unwrap_or_default());
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

/// Whether the instance under `key` (descriptor `d`) is a copy in another format standing in for the wrapped plugin its
/// channel or slot names in `proj`, not edited by the user (`Hosted::stand_in`). Its save would replace the named plugin
/// and its saved settings with the copy, so the wrapper keeps the file's bytes. Playback and automation clips moving the
/// copy's parameters do not count. Once the user edits it (in its window, with the parameter menu, by loading a preset),
/// or once a save has written it (the channel or slot then names the copy itself), it is saved as what is hosted.
fn untouched_stand_in(proj: &Project, plugins: &Plugins, key: usize, d: &Descriptor) -> bool {
    if !plugins.hosted.get(&key).is_some_and(|h| h.stand_in) {
        return false;
    }
    let named = match fx_of(key) {
        None => proj.channels.get(key).and_then(wrapped_channel),
        Some((t, k)) => proj.mixer.iter().find(|m| m.index == t).and_then(|m| m.slots[usize::from(k)].as_ref()).and_then(wrapped_slot),
    };
    named.is_some_and(|r| switched_format(d, &r).is_some())
}

/// The file id (event 201) of the plugin channel or filled mixer slot under hosting key `key`.
fn id_of_key(project: &Project, key: usize) -> Option<String> {
    match fx_of(key) {
        None => project.channels.get(key).filter(|c| c.kind == ChannelKind::Plugin).and_then(|c| c.plugin.clone()),
        Some((t, k)) => project.mixer.iter().find(|m| m.index == t)?.slots[usize::from(k)].as_ref().map(|s| s.name.clone()),
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
/// same format and name, else the same name (a copy in another format, which does not get the saved state:
/// [`switched_format`]).
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

/// When `d`, found for wrapped plugin `r`, is its copy in another format: what the hint bar says about it ("Kars (VST2
/// as CLAP)"). The saved state is the other format's, which a copy may take without reading it the same way, so the
/// copy starts from its defaults instead. The very file the project names is that plugin, whatever format its
/// extension suggested (spec: flp/events/plugins.md §4).
pub fn switched_format(d: &Descriptor, r: &PluginRef) -> Option<String> {
    let saved = Format::parse(&r.format)?;
    let same = saved == d.format || (!r.path.is_empty() && same_file(&d.path, &r.path));
    (!same).then(|| format!("{} ({} as {})", r.name, format_name(saved), format_name(d.format)))
}

/// The native plugin of `list` that a project names by its file id (event 201 = the plugin's folder name, which is
/// also the native descriptor's id; spec: plugins/fruity-abi.md "Opened projects"). Case does not count: files
/// store ids such as `Fruity slicer` for the `Fruity Slicer` folder.
pub fn find_native(list: &[Available], file_id: &str) -> Option<usize> {
    list.iter().position(|a| a.desc.format == Format::Native && a.desc.id.eq_ignore_ascii_case(file_id))
}

/// A hint-bar list of plugin names: sorted, each once, at most `max` named.
fn name_list(mut names: Vec<String>, max: usize) -> String {
    names.sort();
    names.dedup();
    let more = names.len().saturating_sub(max);
    let mut s = names.iter().take(max).cloned().collect::<Vec<_>>().join(", ");
    if more > 0 {
        s.push_str(&format!(" and {more} more"));
    }
    s
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
    /// whose plugin is found starts from the state the file holds, or from its defaults when only a copy in another
    /// format is found (the hint bar names those; a save keeps the file's wrapper for them until they are changed,
    /// `Hosted::stand_in`). A plugin that is not found keeps its saved data (written back unchanged) and is not looked
    /// for again.
    pub(crate) fn sync_wrapped(&mut self) {
        use crate::plugins::fx_key;
        let mut added = (false, false);
        let (mut missing, mut switched) = (Vec::new(), Vec::new());
        for row in 0..self.state.project.channels.len() {
            if self.state.hosted_rows.contains(&row) || self.wrap_failed.contains(&row) {
                continue;
            }
            let Some(r) = wrapped_channel(&self.state.project.channels[row]) else { continue };
            let made = find(&self.plugins.available, &r).map(|i| {
                let other = switched_format(&self.plugins.available[i].desc, &r);
                let state = other.is_none().then_some(r.state.as_slice());
                (other, self.plugins.create_with(row, i, state))
            });
            match made {
                Some((other, Ok(_))) => {
                    self.state.hosted_rows.push(row);
                    added.0 = true;
                    if other.is_some() {
                        self.plugins.mark_stand_in(row);
                    }
                    switched.extend(other);
                }
                Some((_, Err(e))) => {
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
            let made = find(&self.plugins.effects, &r).map(|i| {
                let other = switched_format(&self.plugins.effects[i].desc, &r);
                let state = other.is_none().then_some(r.state.as_slice());
                (other, self.plugins.create_effect(t, k, i, state))
            });
            match made {
                Some((other, Ok(_))) => {
                    added.1 = true;
                    if other.is_some() {
                        self.plugins.mark_stand_in(fx_key(t, k));
                    }
                    switched.extend(other);
                }
                Some((_, Err(e))) => {
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
        let mut parts = Vec::new();
        if !missing.is_empty() {
            missing.dedup();
            parts.push(format!("Plugins not found (kept in the project): {}", missing.join(", ")));
        }
        if !switched.is_empty() {
            let names = name_list(switched, 4);
            parts.push(format!("Played by a copy in another format, from its default settings (saved as the original until edited): {names}"));
        }
        if !parts.is_empty() {
            self.state.message = Some(parts.join("; "));
        }
        if added.0 || added.1 || !missing.is_empty() {
            self.state.piano.dirty = true;
            self.invalidate();
        }
    }

    /// With the experimental native host on (Settings > Plugins), the native plugins an opened project names are
    /// hosted by the installed plugins themselves (spec: plugins/fruity-abi.md "Opened projects"): see `host_native`.
    pub(crate) fn sync_native(&mut self) {
        if os_native::enabled() {
            self.host_native();
        }
    }

    /// Hosts every plugin channel and filled mixer slot that names a native plugin (its file id, event 201) and has
    /// no instance yet, started from the state the file holds (event 213). Channels and slots the built-in Open
    /// plugins play (3x Osc, the parametric EQ, wrapped plugins) are theirs already, so they are left alone. A plugin
    /// still waiting for its first-load check is tried again on a later tick; one that is not installed is reported
    /// once the scan is over and kept in the project; one that does not start is reported and kept too. An instance
    /// that does not give the file's state back byte for byte keeps the file's bytes on a save (`Hosted::keep_file_state`).
    pub(crate) fn host_native(&mut self) {
        use crate::plugins::fx_key;
        let scanning = self.sweep.is_some();
        let pending = self.state.pending_plugin.map(|(r, _)| r);
        let mut added = (false, false);
        let (mut hosted, mut kept, mut missing, mut failed) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let rows: Vec<(usize, String, Option<Vec<u8>>)> = (0..self.state.project.channels.len())
            .filter(|&r| !self.state.hosted_rows.contains(&r) && !self.wrap_failed.contains(&r) && !self.native_failed.contains(&r) && Some(r) != pending)
            .filter_map(|r| {
                let ch = &self.state.project.channels[r];
                let name = ch.plugin.as_deref().filter(|n| ch.kind == ChannelKind::Plugin && !n.is_empty() && *n != WRAPPER)?;
                Some((r, name.to_string(), ch.plugin_state.clone()))
            })
            .collect();
        for (row, name, state) in rows {
            let Some(i) = find_native(&self.plugins.available, &name) else {
                if !scanning {
                    self.native_failed.push(row);
                    missing.push(name);
                }
                continue;
            };
            if !self.plugins.instrument_ready(i) {
                continue;
            }
            match self.plugins.create_with(row, i, state.as_deref().filter(|s| !s.is_empty())) {
                Ok(_) => {
                    self.state.hosted_rows.push(row);
                    added.0 = true;
                    if !self.native_round_trips(row, state.as_deref()) {
                        kept.push(name.clone());
                    }
                    hosted.push(name);
                }
                Err(e) => {
                    self.native_failed.push(row);
                    failed.push(format!("{name} ({e})"));
                }
            }
        }
        let slots: Vec<(u16, u8, String, Vec<u8>)> = self
            .state
            .project
            .mixer
            .iter()
            .flat_map(|m| m.slots.iter().enumerate().filter_map(move |(k, s)| s.as_ref().map(|s| (m.index, k as u8, s.name.clone(), s.state.clone()))))
            .filter(|(t, k, name, _)| {
                let key = fx_key(*t, *k);
                !name.is_empty() && name != WRAPPER && !self.plugins.hosted.contains_key(&key) && !self.native_failed.contains(&key)
            })
            .collect();
        for (t, k, name, state) in slots {
            let key = fx_key(t, k);
            let Some(i) = find_native(&self.plugins.effects, &name) else {
                if !scanning {
                    self.native_failed.push(key);
                    missing.push(name);
                }
                continue;
            };
            if !self.plugins.effect_ready(i) {
                continue;
            }
            match self.plugins.create_effect(t, k, i, (!state.is_empty()).then_some(state.as_slice())) {
                Ok(_) => {
                    added.1 = true;
                    if !self.native_round_trips(key, Some(&state)) {
                        kept.push(name.clone());
                    }
                    hosted.push(name);
                }
                Err(e) => {
                    self.native_failed.push(key);
                    failed.push(format!("{name} ({e})"));
                }
            }
        }
        if added.0 {
            self.transport.set_instruments(self.plugins.instruments());
        }
        if added.1 {
            self.transport.set_effects(self.plugins.effects());
        }
        let mut parts = Vec::new();
        if !hosted.is_empty() {
            parts.push(format!("Hosted {} of the project's native plugins: {}", hosted.len(), name_list(hosted, 6)));
        }
        if !kept.is_empty() {
            parts.push(format!("saving keeps the file's settings for {}", name_list(kept, 6)));
        }
        if !missing.is_empty() {
            parts.push(format!("native plugins not installed (kept in the project): {}", name_list(missing, 6)));
        }
        if !failed.is_empty() {
            parts.push(format!("native plugins that did not start (kept in the project): {}", name_list(failed, 4)));
        }
        if !parts.is_empty() {
            self.state.message = Some(parts.join("; "));
            self.state.piano.dirty = true;
            self.invalidate();
        }
    }

    /// The hidden preference changed (or the native host with it): the channels and slots a built-in recreation can
    /// play (3x Osc, the parametric EQ) move to the side that should play them now. Each one hosted by the other side
    /// writes its settings into the project as the file's own state, its instance is closed, and the next tick hosts
    /// it again from that state (`sync_osc3` / `sync_effects`, or `host_native`). A row the plugin menus are just
    /// creating is left alone. Returns how many channels and slots were switched.
    pub(crate) fn swap_builtin_overrides(&mut self) -> usize {
        let native = native_first();
        let pending = self.state.pending_plugin.map(|(r, _)| r);
        let swap: Vec<usize> = self
            .plugins
            .hosted
            .keys()
            .copied()
            .filter(|&key| Some(key) != pending && id_of_key(&self.state.project, key).is_some_and(|id| has_recreation(&id)))
            .filter(|&key| {
                let Some(d) = self.plugins.descriptor(key) else { return false };
                // Moving to the real plugins: the recreations go; moving back: the real plugins go.
                if native { is_builtin(&d) } else { d.format == Format::Native }
            })
            .collect();
        // Their settings, as the file stores them, stay in the project.
        self.store_builtin_states();
        for &key in &swap {
            if self.plugins.descriptor(key).is_some_and(|d| d.format == Format::Native) && !self.plugins.hosted[&key].keep_file_state {
                if let Some(state) = self.plugins.state(key) {
                    store_native(&mut self.state.project, key, state);
                }
            }
        }
        for &key in &swap {
            self.plugins.drop_key(key);
            self.state.hosted_rows.retain(|&r| r != key);
        }
        // Either side may try the moved channels and slots again (a recreation that failed before, or a missing
        // native plugin); so may any other channel or slot a recreation plays.
        let all: Vec<usize> = self.wrap_failed.iter().chain(&self.fx_failed).chain(&self.native_failed).copied().filter(|&k| id_of_key(&self.state.project, k).is_some_and(|id| has_recreation(&id))).collect();
        for list in [&mut self.wrap_failed, &mut self.fx_failed, &mut self.native_failed] {
            list.retain(|k| !all.contains(k));
        }
        if !swap.is_empty() {
            self.transport.set_instruments(self.plugins.instruments());
            self.transport.set_effects(self.plugins.effects());
            self.state.piano.dirty = true;
        }
        self.invalidate();
        swap.len()
    }

    /// Whether the native instance under `key`, just started from `file_state`, gives those bytes back as its own
    /// state. If not, the instance is marked so a save keeps the file's bytes (spec: plugins/fruity-abi.md "Saved
    /// state": the instance would lose the file's data otherwise). No file state = the plugin starts at its defaults
    /// and its own state is what the file gets.
    fn native_round_trips(&mut self, key: usize, file_state: Option<&[u8]>) -> bool {
        let Some(file) = file_state.filter(|s| !s.is_empty()) else { return true };
        let same = self.plugins.state(key).is_some_and(|s| s == file);
        if !same {
            if let Some(h) = self.plugins.hosted.get_mut(&key) {
                h.keep_file_state = true;
            }
        }
        same
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::plugins::{fx_key, LEMON_EQ, LEMON_SAMPLER};

    /// A hosted plugin channel and a mixer slot plugin are saved as wrapped plugins holding their identity and state,
    /// and the reopened project hosts them again with that state; a second save keeps working on the saved file
    /// (spec: flp/events/plugins.md). The built-in Lemon plugins stand in for third-party plugins.
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
        let row = crate::app::add_plugin_channel(&mut a.state.project, LEMON_SAMPLER);
        a.plugins.create(row, a.plugins.index_of(LEMON_SAMPLER).unwrap()).unwrap();
        a.state.hosted_rows.push(row);
        assert!(a.plugins.load_sample(row, "C:/samples/one.wav"));
        let mut fl = lemon_parametric_eq_2::params::default_state();
        lemon_parametric_eq_2::params::set_field(&mut fl, 1, -600);
        let eq = lemon_parametric_eq_2::params::plugin_state_from_fl(&fl);
        a.state.project.mixer[3].slots[2] = Some(SlotPlugin { name: LEMON_EQ.into(), state: Vec::new() });
        a.plugins.create_effect(3, 2, a.plugins.effect_index(LEMON_EQ).unwrap(), Some(&eq)).unwrap();
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
        assert_eq!(Some(r.state.clone()), a.plugins.state(row));
        let s = wrapped_slot(saved.mixer[3].slots[2].as_ref().unwrap()).unwrap();
        assert_eq!(s.name, LEMON_EQ);
        assert_eq!(slot_title(saved.mixer[3].slots[2].as_ref().unwrap()), LEMON_EQ);
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
        assert_eq!(crate::mixer::slot_name(&b.state, 3, 2).as_deref(), Some(LEMON_EQ));

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

    /// Trim unused tracks leaves fewer tracks than the file has; the save drops the rest (spec:
    /// ui/mixer-track-commands.md §4, flp/events/mixer.md "Removing tracks").
    #[test]
    fn trimmed_mixer_saves_and_reopens() {
        let path = std::env::temp_dir().join(format!("openstudio-trim-{}.flp", std::process::id()));
        let mut a = App::new(None);
        a.state.project = crate::app::test_project();
        a.state.file = None;
        a.state.path = None;
        a.save_to(path.clone());
        a.state.project.channels[4].mixer_track = 9;
        a.state.project.mixer[9].name = Some("Nine".into());
        crate::mixer::run(&mut a, crate::mixer::MCmd::Trim { non_empty: false });
        assert_eq!(crate::mixer::inserts(&a.state), 5);
        a.save_to(path.clone());
        assert!(a.state.message.as_deref().is_some_and(|m| m.starts_with("Saved") && !m.contains("not stored")), "{:?}", a.state.message);
        let mut b = App::new(None);
        b.open(path.clone());
        let _ = std::fs::remove_file(&path);
        assert_eq!(b.state.project.mixer, a.state.project.mixer);
        assert_eq!(b.state.project.channels[4].mixer_track, 5);
    }

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
        let plugin: os_host::SharedPlugin = std::sync::Arc::new(std::sync::Mutex::new(NativeStandIn(desc, vec![9, 8, 7, 6], true)));
        a.state.project.mixer[2].slots[1] = Some(crate::plugins::new_slot("Fruity Balance"));
        a.plugins.hosted.insert(
            fx_key(2, 1),
            crate::plugins::Hosted::new(plugin),
        );
        a.save_to(path.clone());
        assert!(a.state.message.as_deref().is_some_and(|m| m.starts_with("Saved")), "{:?}", a.state.message);
        let saved = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(saved.mixer[2].slots[1], Some(SlotPlugin { name: "Fruity Balance".into(), state: vec![9, 8, 7, 6] }));
        assert_eq!(a.state.project.mixer[2].slots[1], saved.mixer[2].slots[1]);
    }

    /// An opened project's plugin channels and mixer slots that name installed native plugins are hosted by them,
    /// started from the file's state; a plugin that is not installed is reported and kept; the states of hosted
    /// instances are saved, except for one that did not give the file's state back, whose file bytes are kept
    /// (spec: plugins/fruity-abi.md "Opened projects", "Saved state").
    #[test]
    fn opened_projects_host_their_native_plugins() {
        let path = std::env::temp_dir().join(format!("openstudio-native-open-{}.flp", std::process::id()));
        let mut a = App::new(None);
        a.state.project = crate::app::test_project();
        a.state.file = None;
        a.state.path = None;
        a.plugins.available = vec![native_stand_in("Harmor", os_host::Kind::Instrument, true)];
        a.plugins.effects = vec![native_stand_in("Fruity Balance", os_host::Kind::Effect, true), native_stand_in("Gross Beat", os_host::Kind::Effect, false)];
        let harmor = crate::app::add_plugin_channel(&mut a.state.project, "Harmor");
        a.state.project.channels[harmor].plugin_state = Some(vec![1, 2, 3]);
        let sytrus = crate::app::add_plugin_channel(&mut a.state.project, "Sytrus");
        a.state.project.channels[sytrus].plugin_state = Some(vec![4]);
        a.state.project.mixer[2].slots[1] = Some(SlotPlugin { name: "Fruity Balance".into(), state: vec![5, 6] });
        a.state.project.mixer[3].slots[0] = Some(SlotPlugin { name: "Gross Beat".into(), state: vec![7, 8, 9] });
        // A file id in another case than the folder name (files store `Fruity slicer`).
        a.state.project.mixer[4].slots[0] = Some(SlotPlugin { name: "fruity balance".into(), state: vec![] });
        let (balance, gross, lower) = (fx_key(2, 1), fx_key(3, 0), fx_key(4, 0));

        a.host_native();
        assert!(a.state.hosted_rows.contains(&harmor));
        assert_eq!(a.plugins.state(harmor), Some(vec![1, 2, 3]));
        assert_eq!(a.plugins.state(balance), Some(vec![5, 6]));
        assert!(a.plugins.hosted.contains_key(&lower) && !a.plugins.hosted[&lower].keep_file_state);
        assert_eq!(a.plugins.state(gross), Some(vec![7, 8, 9, 0xEE]));
        assert!(!a.plugins.hosted[&harmor].keep_file_state && !a.plugins.hosted[&balance].keep_file_state);
        assert!(a.plugins.hosted[&gross].keep_file_state);
        assert!(!a.state.hosted_rows.contains(&sytrus) && a.native_failed.contains(&sytrus));
        let msg = a.state.message.clone().unwrap_or_default();
        assert!(msg.contains("Harmor") && msg.contains("Gross Beat") && msg.contains("not installed") && msg.contains("Sytrus"), "{msg}");
        // Another tick changes nothing.
        let n = a.plugins.hosted.len();
        a.host_native();
        assert_eq!((a.plugins.hosted.len(), a.state.hosted_rows.len()), (n, 1));

        // Saved: hosted instances write their state; the one that did not round-trip keeps the file's bytes.
        assert!(a.plugins.set_state(harmor, &[11, 12]) && a.plugins.set_state(balance, &[13]) && a.plugins.set_state(gross, &[14]));
        a.save_to(path.clone());
        assert!(a.state.message.as_deref().is_some_and(|m| m.starts_with("Saved")), "{:?}", a.state.message);
        let saved = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!((saved.channels[harmor].plugin.as_deref(), saved.channels[harmor].plugin_state.as_deref()), (Some("Harmor"), Some(&[11, 12][..])));
        assert_eq!((saved.channels[sytrus].plugin.as_deref(), saved.channels[sytrus].plugin_state.as_deref()), (Some("Sytrus"), Some(&[4][..])));
        assert_eq!(saved.mixer[2].slots[1], Some(SlotPlugin { name: "Fruity Balance".into(), state: vec![13] }));
        assert_eq!(saved.mixer[3].slots[0], Some(SlotPlugin { name: "Gross Beat".into(), state: vec![7, 8, 9] }));
        // The file's own spelling of the id is kept; the instance's state (its defaults) is written.
        assert_eq!(saved.mixer[4].slots[0], Some(SlotPlugin { name: "fruity balance".into(), state: vec![0, 0] }));
    }

    /// The hidden "real native plugins" toggle: a 3x Osc channel and a parametric EQ slot played by the Open
    /// recreations move to the installed plugins, started from the file's state, and back again; the settings stay
    /// in the project across each move (spec: plugins/fruity-abi.md "Opened projects").
    #[test]
    fn real_native_plugins_replace_the_open_recreations_and_back() {
        use crate::osc3::{FILE_NAME as OSC3_FILE, LEMON_TRIO};
        use crate::plugins::EQ_FILE_NAME;
        NATIVE_FIRST.with(|n| n.set(Some(false)));
        let mut a = App::new(None);
        a.state.project = crate::app::test_project();
        a.plugins.available = crate::plugins::builtin();
        a.plugins.available.push(native_stand_in(OSC3_FILE, os_host::Kind::Instrument, true));
        a.plugins.effects = crate::plugins::builtin_effects();
        a.plugins.effects.push(native_stand_in(EQ_FILE_NAME, os_host::Kind::Effect, true));
        let row = crate::app::add_plugin_channel(&mut a.state.project, LEMON_TRIO);
        let osc_file = lemon_3x_osc::params::default_state();
        a.state.project.channels[row].plugin_state = Some(osc_file.clone());
        let mut eq_file = lemon_parametric_eq_2::params::default_state();
        lemon_parametric_eq_2::params::set_field(&mut eq_file, 1, -600);
        a.state.project.mixer[2].slots[1] = Some(SlotPlugin { name: EQ_FILE_NAME.into(), state: eq_file.clone() });
        let slot = fx_key(2, 1);
        let builtin_at = |a: &App, key| a.plugins.descriptor(key).is_some_and(|d| is_builtin(&d));
        let native_at = |a: &App, key| a.plugins.descriptor(key).is_some_and(|d| d.format == Format::Native);

        // Off: the recreations play them, and the native pass leaves them alone.
        a.sync_osc3();
        a.sync_effects();
        a.host_native();
        assert!(builtin_at(&a, row) && builtin_at(&a, slot));

        // On: switched to the installed plugins, started from the file's bytes.
        NATIVE_FIRST.with(|n| n.set(Some(true)));
        assert_eq!(a.swap_builtin_overrides(), 2);
        assert!(!a.plugins.hosted.contains_key(&row) && !a.plugins.hosted.contains_key(&slot) && !a.state.hosted_rows.contains(&row));
        a.sync_osc3();
        a.sync_effects();
        assert!(!a.plugins.hosted.contains_key(&row), "the recreation stays out");
        a.host_native();
        assert!(native_at(&a, row) && native_at(&a, slot) && a.state.hosted_rows.contains(&row));
        assert_eq!(a.plugins.state(row).as_deref(), a.state.project.channels[row].plugin_state.as_deref());
        assert_eq!(a.plugins.state(slot), Some(eq_file.clone()));

        // Off again: back to the recreations, the settings still in the project.
        NATIVE_FIRST.with(|n| n.set(Some(false)));
        assert_eq!(a.swap_builtin_overrides(), 2);
        assert_eq!(a.state.project.mixer[2].slots[1].as_ref().map(|s| s.state.clone()), Some(eq_file.clone()));
        a.sync_osc3();
        a.sync_effects();
        assert!(builtin_at(&a, row) && builtin_at(&a, slot));
        assert_eq!(a.plugins.eq_file_state(slot), Some(eq_file));
        assert_eq!(a.state.project.channels[row].plugin.as_deref(), Some(OSC3_FILE));
        let _ = osc_file;
        NATIVE_FIRST.with(|n| n.set(None));
    }

    /// While a plugin scan runs, a native plugin that is not listed yet is neither hosted nor given up on; after the
    /// scan it is looked for again.
    #[test]
    fn native_plugins_wait_for_the_scan() {
        let mut a = App::new(None);
        a.state.project = crate::app::test_project();
        a.plugins.available = Vec::new();
        let row = crate::app::add_plugin_channel(&mut a.state.project, "Harmor");
        a.sweep = Some(crate::plugin_scan::Sweep::idle());
        a.host_native();
        assert!(!a.state.hosted_rows.contains(&row) && a.native_failed.is_empty());
        a.sweep = None;
        a.plugins.available = vec![native_stand_in("Harmor", os_host::Kind::Instrument, true)];
        a.host_native();
        assert!(a.state.hosted_rows.contains(&row));
    }

    /// A wrapped plugin found only as its copy in another format starts from its defaults (the saved state is the other
    /// format's) and the hint bar says so, next to the plugins that are not found; one found in its own format gets the
    /// saved state. Until the user edits it, the copy leaves the file's wrapper byte for byte, also when something else
    /// (playback, an automation clip) changed its state; once edited, it is saved as what it is, and from then on as any
    /// hosted plugin. The built-in Lemon Parametric EQ (a CLAP plugin) stands in.
    #[test]
    fn copies_in_another_format_start_from_their_defaults() {
        WRAP_BUILTINS.with(|w| w.set(true));
        let mut a = App::new(None);
        a.state.project = crate::app::test_project();
        a.plugins.available = crate::plugins::builtin();
        a.plugins.effects = crate::plugins::builtin_effects();
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
        let slot = |r: &PluginRef| Some(SlotPlugin { name: WRAPPER.into(), state: wrapper::encode(None, r) });
        a.state.project.mixer[2].slots[0] = slot(&saved_as("vst2", LEMON_EQ, "Lemon EQ.dll"));
        a.state.project.mixer[3].slots[0] = slot(&saved_as("clap", LEMON_EQ, "Lemon EQ.clap"));
        let row = crate::app::add_plugin_channel(&mut a.state.project, WRAPPER);
        a.state.project.channels[row].plugin_state = Some(wrapper::encode(None, &saved_as("vst2", "Nowhere", "Nowhere.dll")));
        a.sync_wrapped();

        let fresh = fx_key(4, 0);
        a.plugins.create_effect(4, 0, a.plugins.effect_index(LEMON_EQ).unwrap(), None).unwrap();
        let defaults = a.plugins.eq_file_state(fresh);
        assert_ne!(defaults, Some(fl.clone()));
        assert_eq!(a.plugins.eq_file_state(fx_key(2, 0)), defaults, "the VST2 copy's state is not given to the CLAP plugin");
        assert_eq!(a.plugins.eq_file_state(fx_key(3, 0)), Some(fl), "the CLAP state is");
        assert!(a.wrap_failed.contains(&row));
        let told = format!(
            "Plugins not found (kept in the project): Nowhere; Played by a copy in another format, from its default settings \
             (saved as the original until edited): {LEMON_EQ} (VST2 as CLAP)"
        );
        assert_eq!(a.state.message.as_deref(), Some(told.as_str()));
        a.plugins.drop_key(fresh);
        a.state.project.mixer[4].slots[0] = None;

        // Saved untouched: the VST2 plugin's wrapper is written as the file had it; the CLAP plugin as itself.
        let path = std::env::temp_dir().join(format!("openstudio-stand-in-{}.flp", std::process::id()));
        let reload = |path: &std::path::Path| os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(path).unwrap()).unwrap()).unwrap();
        let named = a.state.project.mixer[2].slots[0].clone();
        a.save_to(path.clone());
        assert!(a.state.message.as_deref().is_some_and(|m| m.starts_with("Saved")), "{:?}", a.state.message);
        let saved = reload(&path);
        assert_eq!(saved.mixer[2].slots[0], named, "byte for byte");
        assert_eq!(wrapped_slot(saved.mixer[3].slots[0].as_ref().unwrap()).unwrap().state, a.plugins.state(fx_key(3, 0)).unwrap());
        // Saved again, still untouched: the same.
        a.save_to(path.clone());
        assert_eq!(reload(&path).mixer[2].slots[0], named);

        // Its state changed by something other than the user (as an automation clip does): still the file's wrapper.
        let start = a.plugins.state(fx_key(2, 0)).unwrap();
        assert!(a.plugins.set_state(fx_key(2, 0), &eq));
        a.save_to(path.clone());
        assert_eq!(reload(&path).mixer[2].slots[0], named, "not the user's edit");

        // Edited by the user (Paste value in the parameter menu): saved as the copy that plays it, with its own state.
        // (The stand-in's file is this program, so the saved path names it; a plugin file's extension would give the
        // format.)
        let exe = std::env::current_exe().unwrap();
        assert_eq!(a.browse_params(fx_key(2, 0)), None);
        a.state.params.copied = Some(0.75);
        let paste = crate::playlist::Cmd::Param(crate::params::PCmd::Paste(fx_key(2, 0), 0));
        assert_eq!(crate::playlist::run(&mut a, paste), None);
        a.plugins.hosted[&fx_key(2, 0)].plugin.lock().unwrap().idle();
        a.save_to(path.clone());
        let r = wrapped_slot(reload(&path).mixer[2].slots[0].as_ref().unwrap()).unwrap();
        assert_eq!((std::path::PathBuf::from(&r.path), r.id.as_str()), (exe.clone(), ""));
        assert_eq!(Some(r.state), a.plugins.state(fx_key(2, 0)));
        // From then on the project names the copy: back at its first state, it is saved as it is.
        assert!(a.plugins.set_state(fx_key(2, 0), &start));
        a.save_to(path.clone());
        let r = wrapped_slot(reload(&path).mixer[2].slots[0].as_ref().unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!((std::path::PathBuf::from(&r.path), r.state), (exe, start));
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
