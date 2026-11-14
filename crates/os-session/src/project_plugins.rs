// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
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

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use os_flp::wrapper::{self, WRAPPER};
use os_host::{Descriptor, Format};
use os_model::{Channel, ChannelKind, PluginRef, Project, SlotPlugin};

use crate::plugins::{format_name, fx_of, Available, Plugins};

/// A plugin compiled into Lemon Studio (its library is this program).
pub fn is_builtin(d: &Descriptor) -> bool {
    std::env::current_exe().is_ok_and(|exe| exe == d.path)
}

#[cfg(any(test, feature = "test-util"))]
thread_local! {
    /// Tests: the built-in Lemon plugins stand in for third-party plugins.
    pub static WRAP_BUILTINS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(any(test, feature = "test-util"))]
thread_local! {
    /// Tests: the "real native plugins instead of Lemon recreations" preference for this thread, without touching the
    /// process environment other tests read.
    pub static NATIVE_FIRST: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Whether an opened project's native plugins are played by the installed plugins even where a built-in Open
/// recreation would take them (the hidden settings toggle; spec: plugins/fruity-abi.md "Opened projects").
pub fn native_first() -> bool {
    #[cfg(any(test, feature = "test-util"))]
    if let Some(v) = NATIVE_FIRST.with(|n| n.get()) {
        return v;
    }
    os_native::prefer_native()
}

/// Whether a channel or slot holding file id `id` is one a built-in Lemon recreation plays (3x Osc, Sytrus, Ogun, Fruity Slicer, the
/// parametric EQ, Edison, Love Philter, Maximus, FPC, and the effects of `plugins::FL_RECREATIONS`).
pub fn has_recreation(id: &str) -> bool {
    [crate::osc3::FILE_NAME, crate::citrus::FILE_NAME, crate::smith::FILE_NAME, crate::slicer::FILE_NAME, crate::plugins::EQ_FILE_NAME, crate::plugins::EDISON_FILE_NAME, crate::plugins::LOVE_PHILTER_FILE_NAME, crate::plugins::MAXIMUS_FILE_NAME, crate::drums::FILE_NAME]
        .iter()
        .any(|n| id.eq_ignore_ascii_case(n))
        || crate::plugins::recreation_of(id).is_some()
}

/// Whether a hosted plugin is saved inside the wrapper: CLAP, VST3 and VST2 plugins from plugin files, and the built-in
/// effects that have no file plugin of their own to be saved as (`plugins::LINKED_EFFECTS`).
pub fn is_wrapped(d: &Descriptor) -> bool {
    #[cfg(any(test, feature = "test-util"))]
    if WRAP_BUILTINS.with(|w| w.get()) {
        return matches!(d.format, Format::Clap | Format::Vst3 | Format::Vst2);
    }
    matches!(d.format, Format::Clap | Format::Vst3 | Format::Vst2) && (!is_builtin(d) || crate::plugins::LINKED_EFFECTS.contains(&d.name.as_str()) || d.name == crate::plugins::ZEST_NAME)
}

/// The project-file identity of a hosted plugin with its saved state. A built-in effect with no file plugin of its own
/// (`plugins::LINKED_EFFECTS`) names its CLAP bundle in the per-user CLAP folder, where `bundle-plugins --install` puts
/// it: the wrapper takes the format from the file's extension, and another host can find the bundle there.
pub fn plugin_ref(d: &Descriptor, state: Vec<u8>) -> PluginRef {
    let path = if is_builtin(d) && (crate::plugins::LINKED_EFFECTS.contains(&d.name.as_str()) || d.name == crate::plugins::ZEST_NAME) {
        let file = format!("{}.clap", d.name);
        os_clap::standard_dirs().into_iter().next().map_or_else(|| file.clone().into(), |dir| dir.join(&file))
    } else {
        d.path.clone()
    };
    PluginRef {
        format: d.format.as_str().into(),
        // A CLAP id has no place in the wrapper state (spec §4): such plugins are found by file and name.
        id: if matches!(d.format, Format::Vst3 | Format::Vst2) { d.id.clone() } else { String::new() },
        name: d.name.clone(),
        vendor: d.vendor.clone(),
        path: path.to_string_lossy().into_owned(),
        state,
    }
}

/// Writes every hosted third-party plugin into `proj` (the copy a save writes): its channel or slot becomes a wrapped
/// plugin holding the plugin's identity and current state. A channel or slot the file already held as a wrapped
/// plugin keeps the wrapper records Lemon Studio does not edit. A copy in another format that stands in for the plugin
/// the file names, and that the user has not edited, leaves the file's wrapper as it is ([`untouched_stand_in`]).
pub fn store_hosted(proj: &mut Project, plugins: &Plugins) {
    // Containers keep their effects' states in their own slot state.
    crate::container::store_members(proj, plugins);
    for key in plugins.hosted.range(..crate::plugins::MEMBER_KEY).map(|(&k, _)| k).collect::<Vec<_>>() {
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
        if !is_wrapped(&d) || plays_fl_slot(proj, key, &d) {
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

/// Whether the built-in effect `d` hosted under `key` plays a slot that `proj` stores as the FL effect it recreates
/// (`plugins::FL_RECREATIONS`): the slot stays that effect, with the state `App::store_builtin_states` gave it.
fn plays_fl_slot(proj: &Project, key: usize, d: &Descriptor) -> bool {
    let Some((t, k)) = fx_of(key) else { return false };
    let slot = proj.mixer.iter().find(|m| m.index == t).and_then(|m| m.slots[usize::from(k)].as_ref());
    is_builtin(d) && slot.and_then(|s| crate::plugins::recreation_of(&s.name)) == Some(d.name.as_str())
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
pub fn id_of_key(project: &Project, key: usize) -> Option<String> {
    match fx_of(key) {
        None => project.channels.get(key).filter(|c| c.kind == ChannelKind::Plugin).and_then(|c| c.plugin.clone()),
        Some((t, k)) => project.mixer.iter().find(|m| m.index == t)?.slots[usize::from(k)].as_ref().map(|s| s.name.clone()),
    }
}

/// A hosted native plugin's state into its channel (a plugin channel) or mixer slot.
pub fn store_native(proj: &mut Project, key: usize, state: Vec<u8>) {
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

/// Sets the sidechain port (the first auxiliary input bus) of the hosted plugin in mixer slot `slot` of track `track`:
/// the number of the track's input it listens to, 0 for none. A slot that does not hold a wrapped plugin yet becomes
/// one, as a save makes it. `false` when the slot holds no wrapped plugin (spec: audio/sidechain.md "Hosted plugins'
/// input buses").
pub fn set_slot_sidechain(proj: &mut Project, plugins: &Plugins, track: u16, slot: u8, port: usize) -> bool {
    let key = crate::plugins::fx_key(track, slot);
    let Some(s) = proj.mixer.iter_mut().find(|m| m.index == track).and_then(|m| m.slots.get_mut(usize::from(slot))?.as_mut()) else {
        return false;
    };
    if s.name != WRAPPER {
        let Some(d) = plugins.descriptor(key).filter(is_wrapped) else { return false };
        s.state = wrapper::encode(None, &plugin_ref(&d, plugins.state(key).unwrap_or_default()));
        s.name = WRAPPER.into();
    }
    let Some(state) = wrapper::set_input_port(&s.state, 1, i32::try_from(port).unwrap_or(0)) else { return false };
    s.state = state;
    true
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

/// `App::host_native` in the session; `scanning`: a plugin scan is running.
pub fn host_native(ses: &mut crate::session::Session, scanning: bool) {
    use crate::plugins::fx_key;
    let pending = ses.pending_plugin.map(|(r, _)| r);
    let mut added = (false, false);
    let (mut hosted, mut kept, mut missing, mut failed) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let rows: Vec<(usize, String, Option<Vec<u8>>)> = (0..ses.project.channels.len())
        .filter(|&r| !ses.hosted_rows.contains(&r) && !ses.plugins.wrap_failed.contains(&r) && !ses.plugins.native_failed.contains(&r) && Some(r) != pending)
        .filter_map(|r| {
            let ch = &ses.project.channels[r];
            // Lemon MIDI Out plays MIDI Out channels (`lemon_midi_out`), a container's chain a container's
            // (`container`).
            if crate::lemon_midi_out::is_midi_out(ch) || crate::container::is_container_channel(ch) {
                return None;
            }
            let name = ch.plugin.as_deref().filter(|n| ch.kind == ChannelKind::Plugin && !n.is_empty() && *n != WRAPPER)?;
            Some((r, name.to_string(), ch.plugin_state.clone()))
        })
        .collect();
    for (row, name, state) in rows {
        if ses.plugins.past_deadline() {
            break;
        }
        let Some(i) = find_native(&ses.plugins.available, &name) else {
            if !scanning {
                ses.plugins.native_failed.push(row);
                missing.push(name);
            }
            continue;
        };
        if !ses.plugins.instrument_ready(i) {
            continue;
        }
        match ses.plugins.create_with(row, i, state.as_deref().filter(|s| !s.is_empty())) {
            Ok(_) => {
                ses.hosted_rows.push(row);
                added.0 = true;
                if !native_round_trips(ses, row, state.as_deref()) {
                    kept.push(name.clone());
                }
                hosted.push(name);
            }
            Err(e) => {
                ses.plugins.native_failed.push(row);
                failed.push(format!("{name} ({e})"));
            }
        }
    }
    let slots: Vec<(u16, u8, String, Vec<u8>)> = ses.project
        .mixer
        .iter()
        .flat_map(|m| m.slots.iter().enumerate().filter_map(move |(k, s)| s.as_ref().map(|s| (m.index, k as u8, s))))
        // Before the states are copied: this runs every tick, and an Edison slot's can be tens of megabytes.
        .filter(|&(t, k, s)| {
            let key = fx_key(t, k);
            !s.name.is_empty() && s.name != WRAPPER && s.name != crate::container::CONTAINER && !ses.plugins.hosted.contains_key(&key) && !ses.plugins.native_failed.contains(&key)
        })
        .map(|(t, k, s)| (t, k, s.name.clone(), s.state.clone()))
        .collect();
    for (t, k, name, state) in slots {
        if ses.plugins.past_deadline() {
            break;
        }
        let key = fx_key(t, k);
        let Some(i) = find_native(&ses.plugins.effects, &name) else {
            if !scanning {
                ses.plugins.native_failed.push(key);
                missing.push(name);
            }
            continue;
        };
        if !ses.plugins.effect_ready(i) {
            continue;
        }
        match ses.plugins.create_effect(t, k, i, (!state.is_empty()).then_some(state.as_slice())) {
            Ok(_) => {
                added.1 = true;
                if !native_round_trips(ses, key, Some(&state)) {
                    kept.push(name.clone());
                }
                hosted.push(name);
            }
            Err(e) => {
                ses.plugins.native_failed.push(key);
                failed.push(format!("{name} ({e})"));
            }
        }
    }
    if added.0 {
        ses.transport.set_instruments(ses.plugins.instruments());
    }
    if added.1 {
        ses.transport.set_effects(ses.plugins.effects());
    }
    let mut parts = Vec::new();
    if !hosted.is_empty() {
        parts.push(format!("Hosted {} of the project's native plugins: {}", hosted.len(), name_list(hosted, 6)));
    }
    if !kept.is_empty() {
        parts.push(format!("saving keeps the file's settings for {}", name_list(kept, 6)));
    }
    ses.plugins.add_not_found(&missing);
    ses.plugins.add_not_found(&failed);
    if !missing.is_empty() {
        parts.push(format!("native plugins not installed (kept in the project): {}", name_list(missing, 6)));
    }
    if !failed.is_empty() {
        parts.push(format!("native plugins that did not start (kept in the project): {}", name_list(failed, 4)));
    }
    if !parts.is_empty() {
        ses.touched.views.push(crate::session::View::Message(parts.join("; ")));
        ses.dirty = true;
        ses.touched.repaint = true;
    }
}

/// Hosts the wrapped plugins of an opened project (spec: flp/events/plugins.md §4): each wrapped channel and slot
/// whose plugin is found starts from the state the file holds, or from its defaults when only a copy in another
/// format is found (the hint bar names those; a save keeps the file's wrapper for them until they are changed,
/// `Hosted::stand_in`). A plugin that is not found keeps its saved data (written back unchanged) and is not looked
/// for again.
pub fn sync_wrapped(ses: &mut crate::session::Session) {
    use crate::plugins::fx_key;
    let mut added = (false, false);
    let (mut missing, mut switched) = (Vec::new(), Vec::new());
    for row in 0..ses.project.channels.len() {
        if ses.hosted_rows.contains(&row) || ses.plugins.wrap_failed.contains(&row) {
            continue;
        }
        let Some(r) = wrapped_channel(&ses.project.channels[row]) else { continue };
        if ses.plugins.past_deadline() {
            break;
        }
        let made = find(&ses.plugins.available, &r).map(|i| {
            let other = switched_format(&ses.plugins.available[i].desc, &r);
            let state = other.is_none().then_some(r.state.as_slice());
            (other, ses.plugins.create_with(row, i, state))
        });
        match made {
            Some((other, Ok(_))) => {
                ses.hosted_rows.push(row);
                added.0 = true;
                // Glides reach a plugin without note tuning as pitch bend over the wrapper's range.
                if let Some(n) = ses.project.channels[row].plugin_state.as_deref().and_then(wrapper::bend_range) {
                    ses.plugins.set_bend_range(row, n);
                }
                if other.is_some() {
                    ses.plugins.mark_stand_in(row);
                }
                switched.extend(other);
            }
            Some((_, Err(e))) => {
                ses.plugins.wrap_failed.push(row);
                missing.push(format!("{} ({e})", r.name));
            }
            None => {
                ses.plugins.wrap_failed.push(row);
                missing.push(r.name);
            }
        }
    }
    let slots: Vec<(u16, u8, PluginRef)> = ses.project
        .mixer
        .iter()
        .flat_map(|m| m.slots.iter().enumerate().filter_map(move |(k, s)| Some((m.index, k as u8, s.as_ref()?))))
        // Hosted and failed slots first: decoding a slot copies its whole saved state, and this runs every tick.
        .filter(|&(t, k, _)| !ses.plugins.hosted.contains_key(&fx_key(t, k)) && !ses.plugins.wrap_failed.contains(&fx_key(t, k)))
        .filter_map(|(t, k, s)| Some((t, k, wrapped_slot(s)?)))
        .collect();
    for (t, k, r) in slots {
        if ses.plugins.past_deadline() {
            break;
        }
        let made = find(&ses.plugins.effects, &r).map(|i| {
            let other = switched_format(&ses.plugins.effects[i].desc, &r);
            let state = other.is_none().then_some(r.state.as_slice());
            (other, ses.plugins.create_effect(t, k, i, state))
        });
        match made {
            Some((other, Ok(_))) => {
                added.1 = true;
                if other.is_some() {
                    ses.plugins.mark_stand_in(fx_key(t, k));
                }
                switched.extend(other);
            }
            Some((_, Err(e))) => {
                ses.plugins.wrap_failed.push(fx_key(t, k));
                missing.push(format!("{} ({e})", r.name));
            }
            None => {
                ses.plugins.wrap_failed.push(fx_key(t, k));
                missing.push(r.name);
            }
        }
    }
    if added.0 {
        ses.transport.set_instruments(ses.plugins.instruments());
    }
    if added.1 {
        ses.transport.set_effects(ses.plugins.effects());
    }
    let mut parts = Vec::new();
    if !missing.is_empty() {
        ses.plugins.add_not_found(&missing);
        parts.push(format!("Plugins not found (kept in the project): {}", name_list(ses.plugins.not_found.clone(), 6)));
    }
    if !switched.is_empty() {
        let names = name_list(switched, 4);
        parts.push(format!("Played by a copy in another format, from its default settings (saved as the original until edited): {names}"));
    }
    if !parts.is_empty() {
        ses.touched.views.push(crate::session::View::Message(parts.join("; ")));
    }
    if added.0 || added.1 || !missing.is_empty() {
        ses.dirty = true;
        ses.touched.repaint = true;
    }
}

/// Mixer slots that hold the project file's parametric EQ are played and edited by Lemon Parametric EQ, started
/// from the slot's saved state (spec: plugins/lemon-parametric-eq-2.md §8). Without the plugin the engine's own EQ
/// plays them. Edison's slots are opened as Tesla the same way, with their sample, loop, markers and recording
/// settings (spec: plugins/tesla/README.md "Opening Edison slots"), Love Philter's as Pulp Filter and Maximus's as
/// Press with their state (spec: plugins/lemon-pulp-filter.md §6, plugins/lemon-press.md §6), and the FL effects
/// another Lemon effect recreates (`plugins::FL_RECREATIONS`: Reeverb 2, Balance, Chorus, ...) by that effect.
pub fn sync_effects(ses: &mut crate::session::Session) {
    use crate::plugins::{fx_key, EDISON_FILE_NAME, EQ_FILE_NAME, LEMON_EQ, LOVE_PHILTER_FILE_NAME, MAXIMUS_FILE_NAME, PRESS, PULP_FILTER, TESLA};
    // The installed EQ plays these slots instead (`host_native`).
    if crate::project_plugins::native_first() {
        return;
    }
    // Runs every tick: the slots already hosted are passed over before their states are read, as an Edison slot's
    // can hold tens of megabytes of audio.
    let todo: Vec<(u16, u8, String)> = ses.project
        .mixer
        .iter()
        .flat_map(|m| m.slots.iter().enumerate().map(move |(k, s)| (m.index, k as u8, s)))
        .filter(|&(t, k, _)| !ses.plugins.hosted.contains_key(&fx_key(t, k)) && !ses.plugins.fx_failed.contains(&fx_key(t, k)))
        .filter_map(|(t, k, s)| {
            s.as_ref()
                .filter(|s| {
                    [EQ_FILE_NAME, EDISON_FILE_NAME, LOVE_PHILTER_FILE_NAME, MAXIMUS_FILE_NAME].contains(&s.name.as_str())
                        || crate::plugins::recreation_of(&s.name).is_some()
                })
                .map(|s| (t, k, s.name.clone()))
        })
        .collect();
    let mut made = false;
    for (t, k, file) in todo {
        if ses.plugins.past_deadline() {
            break;
        }
        let recreation = crate::plugins::recreation_of(&file);
        let name = match file.as_str() {
            EQ_FILE_NAME => LEMON_EQ,
            LOVE_PHILTER_FILE_NAME => PULP_FILTER,
            MAXIMUS_FILE_NAME => PRESS,
            _ => recreation.unwrap_or(TESLA),
        };
        let Some(index) = ses.plugins.effect_index(name) else { continue };
        let track = ses.project.mixer.iter().find(|m| m.index == t);
        let Some(fl) = track.and_then(|m| m.slots.get(usize::from(k))?.as_ref()).map(|s| s.state.as_slice()) else { continue };
        let st = if file == EQ_FILE_NAME {
            Some(lemon_parametric_eq_2::params::plugin_state_from_fl(fl))
        } else if file == LOVE_PHILTER_FILE_NAME {
            // Bytes that are not a Love Philter state open Pulp Filter's starting state.
            Some(pulp_filter::plugin_state_from_fl(fl))
        } else if file == MAXIMUS_FILE_NAME {
            // Bytes that are not a Maximus state open Press's starting state.
            Some(press::plugin_state_from_fl(fl))
        } else if let Some(lemon) = recreation {
            // A state shorter than the original's, or of a later version, opens the defaults (spec:
            // plugins/lemon-small-effects.md §5).
            crate::plugins::recreation_state_from_fl(lemon, fl)
        } else {
            // Bytes that are not an Edison state open an empty Tesla.
            tesla::plugin_state_from_edison(fl)
        };
        made = true;
        if let Err(e) = ses.plugins.create_effect(t, k, index, st.as_deref()) {
            ses.plugins.fx_failed.push(fx_key(t, k));
            ses.touched.views.push(crate::session::View::Message(format!("{name}: {e}")));
        }
    }
    if !made {
        return;
    }
    ses.transport.set_effects(ses.plugins.effects());
    ses.dirty = true;
}

/// `App::remap_rows` in the session: the instances follow rows that moved (`m` gives each row's new row), after the
/// notes held from the keyboard ended.
pub fn remap_rows_in(s: &mut crate::session::Session, m: &dyn Fn(usize) -> Option<usize>) {
    s.plugins.remap(m);
    // The rows hosted are the instances' rows.
    s.hosted_rows = s.plugins.hosted.range(..crate::plugins::FX_KEY).map(|(&r, _)| r).collect();
    s.plugins.sampler_failed = s.plugins.sampler_failed.iter().filter_map(|&r| m(r)).collect();
    s.plugins.wrap_failed = s.plugins.wrap_failed.iter().filter_map(|&k| if k >= crate::plugins::FX_KEY { Some(k) } else { m(k) }).collect();
    s.plugins.native_failed = s.plugins.native_failed.iter().filter_map(|&k| if k >= crate::plugins::FX_KEY { Some(k) } else { m(k) }).collect();
    s.transport.set_instruments(s.plugins.instruments());
    s.dirty = true;
}

/// `App::plugin_paste` in the session: row `row` hosts available plugin `index` from saved state `blob`.
pub fn plugin_paste_in(s: &mut crate::session::Session, row: usize, index: usize, blob: &[u8]) {
    if s.plugins.create_with(row, index, Some(blob)).is_ok() {
        if !s.hosted_rows.contains(&row) {
            s.hosted_rows.push(row);
        }
        s.transport.set_instruments(s.plugins.instruments());
    }
}

#[cfg(test)]
mod tests {
    use super::*;


    /// Every Lemon effect is compiled in: `FILE_MAPPED`, then `LINKED_EFFECTS` in that order, each an effect from this program.
    #[test]
    fn every_lemon_effect_is_built_in() {
        let fx = crate::plugins::builtin_effects();
        let names: Vec<&str> = fx.iter().map(|a| a.desc.name.as_str()).collect();
        let want: Vec<&str> = crate::plugins::FILE_MAPPED.into_iter().chain(crate::plugins::LINKED_EFFECTS).collect();
        assert_eq!(names, want);
        for a in &fx {
            assert!(is_builtin(&a.desc), "{}", a.desc.name);
            assert_eq!(a.desc.kind, os_host::Kind::Effect, "{}", a.desc.name);
        }
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


    /// Zest needs no bank in the repository: it comes up with none compiled in.
    #[test]
    fn zest_is_built_in() {
        assert!(crate::plugins::builtin().into_iter().any(|a| a.desc.name == "Zest"));
    }
}

/// Whether the native instance under `key`, just started from `file_state`, gives those bytes back as its own
/// state. If not, the instance is marked so a save keeps the file's bytes (spec: plugins/fruity-abi.md "Saved
/// state": the instance would lose the file's data otherwise). No file state = the plugin starts at its defaults
/// and its own state is what the file gets.
pub fn native_round_trips(s: &mut crate::session::Session, key: usize, file_state: Option<&[u8]>) -> bool {
    let Some(file) = file_state.filter(|s| !s.is_empty()) else { return true };
    let same = s.plugins.state(key).is_some_and(|s| s == file);
    if !same {
        if let Some(h) = s.plugins.hosted.get_mut(&key) {
            h.keep_file_state = true;
        }
    }
    same
}
