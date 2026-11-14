// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Trio in Lemon Studio: compiled in like Lemon Sampler, it plays every 3x Osc channel of a project (started from
//! the state the file holds) and every one added from the picker, the browser or the channel rack; saving writes its
//! settings back as the file's 3x Osc state, so the file keeps its own plugin. The channel's sample is the custom
//! shape's: the plugin starts with it, and one the plugin loads becomes the channel's.
//!
// spec: plugins/lemon-3x-osc.md §6 (app integration), plugins/3x-osc.md §1 (saved state, custom shape)

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use std::time::Duration;

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

/// The state Lemon Trio starts from for channel `ch` holding 3x Osc state `fl`: an older version's state, without
/// envelope records or filter of its own, takes the channel's (spec: plugins/lemon-3x-osc.md §6.1). A volume record
/// exactly as the model fills a record the file does not have keeps the plugin's default one, as `ChannelVoicing::of`
/// does for the channel.
pub fn channel_voicing(ch: &Channel, fl: &[u8]) -> Vec<u8> {
    let records = std::array::from_fn(|k| {
        let e = &ch.envelopes[k];
        (k != os_model::Envelope::VOLUME || *e != os_model::Envelope::default()).then(|| e.fields())
    });
    let s = &ch.sound;
    lemon_3x_osc::params::with_channel_voicing(fl, &records, [s.cutoff, s.resonance, s.filter_type])
}

/// The plugin id a new channel of picker plugin `name` stores (the built-in recreations store the original's id).
pub fn file_name(name: &str) -> &str {
    match name {
        LEMON_TRIO => FILE_NAME,
        crate::citrus::CITRUS => crate::citrus::FILE_NAME,
        crate::smith::SMITH => crate::smith::FILE_NAME,
        crate::slicer::LEMON_SLICER => crate::slicer::FILE_NAME,
        crate::drums::LEMON_DRUMS => crate::drums::FILE_NAME,
        _ => name,
    }
}

/// What the UI calls a channel's plugin `name` (the file id of a built-in recreation shows as the built-in plugin).
pub fn label(name: &str) -> &str {
    match name {
        FILE_NAME => LEMON_TRIO,
        crate::citrus::FILE_NAME => crate::citrus::CITRUS,
        crate::smith::FILE_NAME => crate::smith::SMITH,
        n if crate::slicer::is_file_name(n) => crate::slicer::LEMON_SLICER,
        crate::drums::FILE_NAME => crate::drums::LEMON_DRUMS,
        n if n.eq_ignore_ascii_case(crate::lemon_midi_out::FILE_NAME) => "Lemon MIDI Out",
        _ => name,
    }
}

impl Plugins {
    /// The project-file state of the Lemon Trio on rack row `row` (its plugin state converted back), `None` when the
    /// row hosts something else.
    pub fn osc3_file_state(&self, row: usize) -> Option<Vec<u8>> {
        let h = self.hosted.get(&row)?;
        let p = h.lock_for_state()?;
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
pub fn store(settings: &crate::settings_file::SettingsStore, proj: &mut Project, plugins: &Plugins) {
    for row in plugins.hosted.range(..FX_KEY).map(|(&k, _)| k).collect::<Vec<_>>() {
        let Some(st) = plugins.osc3_file_state(row) else { continue };
        if let Some(ch) = proj.channels.get_mut(row).filter(|c| c.kind == ChannelKind::Plugin) {
            ch.plugin = Some(FILE_NAME.into());
            ch.plugin_state = Some(st);
        }
        follow_sample(settings, proj, plugins, row, true);
    }
}

/// How often the app looks for a sample a Lemon Trio loaded by itself.
pub const LOOK_EVERY: Duration = Duration::from_millis(500);

/// The custom shape's sample of the Lemon Trio on 3x Osc channel `row` becomes the channel's sample (event 196) and,
/// while Auto name channels is on, its name, unless the channel names that file already (spec: plugins/3x-osc.md §1).
/// Whether the channel changed. `wait`: lock an instance the audio thread is rendering (a save) or leave it.
pub fn follow_sample(st: &crate::settings_file::SettingsStore, proj: &mut Project, plugins: &Plugins, row: usize, wait: bool) -> bool {
    if !proj.channels.get(row).is_some_and(is_osc3) {
        return false;
    }
    let Some(p) = plugins.osc3_sample(row, wait).filter(|p| !p.is_empty()) else { return false };
    let ch = &mut proj.channels[row];
    // A placeholder path (`%FLStudioFactoryData%`...) that still names the same file is kept as the project wrote it.
    if ch.sample_path.as_deref().is_some_and(|old| crate::plugins::resolve_sample(st, old) == p) {
        return false;
    }
    crate::edit::rack::auto_name(st, ch, std::path::Path::new(&p));
    ch.sample_path = Some(p);
    true
}

/// Hosts Lemon Trio on every 3x Osc channel without an instance, started from the channel's saved state (a new
/// instance's state when the channel has none). A row waiting for the picker's own instance is left to it; a row
/// whose instance failed is not tried again.
pub fn sync_osc3(s: &mut crate::session::Session) {
    // The installed 3x Osc plays these channels instead (`host_native`).
    if crate::project_plugins::native_first() {
        return;
    }
    let Some(index) = s.plugins.index_of(LEMON_TRIO) else { return };
    let pending = s.pending_plugin.map(|(r, _)| r);
    let rows: Vec<usize> = (0..s.project.channels.len())
        .filter(|&r| is_osc3(&s.project.channels[r]))
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
        let ch = &s.project.channels[row];
        let fl = ch.plugin_state.clone().unwrap_or_else(lemon_3x_osc::params::default_state);
        let mut st = lemon_3x_osc::params::plugin_state_from_fl(&channel_voicing(ch, &fl));
        // The channel's sample is the custom shape's (spec: plugins/3x-osc.md §1); the plugin opens the file
        // directly, so factory placeholders become real paths.
        let sample = crate::plugins::resolve_sample(&s.settings, ch.sample_path.as_deref().unwrap_or(""));
        if !sample.is_empty()
            && let Some(b) = crate::plugins::with_sample_path(&st, &sample)
        {
            st = b;
        }
        match s.plugins.create_with(row, index, Some(&st)) {
            Ok(_) => {
                s.hosted_rows.push(row);
                added = true;
            }
            Err(e) => {
                s.plugins.wrap_failed.push(row);
                s.touched.views.push(crate::session::View::Message(format!("{LEMON_TRIO}: {e}")));
            }
        }
    }
    if added {
        s.transport.set_instruments(s.plugins.instruments());
        s.dirty = true;
    }
    s.touched.repaint = true;
}

#[cfg(test)]
mod tests {
    use super::*;
    use lemon_3x_osc::params::{self as op, default_state, set_field};
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
        pl.set_rate(48000);
        let index = pl.index_of(LEMON_TRIO).expect("built-in Lemon Trio");
        assert!(crate::project_plugins::is_builtin(&pl.available[index].desc));
        let mut fl = default_state();
        set_field(&mut fl, op::osc(0) + op::SHAPE + 1, 3); // osc 1: saw
        set_field(&mut fl, op::MIX2 + 1, 0); // osc 2 and 3 silent
        set_field(&mut fl, op::MIX3 + 1, 0);
        set_field(&mut fl, 109, 77); // a field nothing uses: kept
        pl.create_with(2, index, Some(&op::plugin_state_from_fl(&fl))).unwrap();
        assert_eq!(pl.osc3_file_state(2), Some(fl.clone()));
        // It sounds: a saw at 880 Hz (pitch formula of 3x-osc.md: key 69 with coarse 0), counted at 48 kHz.
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

}
