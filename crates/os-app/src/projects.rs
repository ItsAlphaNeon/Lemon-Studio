// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! New projects and their channels: the templates (a new project, an empty one, a sample preview's), the rack's
//! added channels, what counts as an unsaved change, and changing the timebase (control-plane step 9: the session's,
//! moved out of `app`, which re-exports them).

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use os_model::{Channel, ChannelKind, MixerTrack, Note, Pattern, Project};

/// An empty sampler channel at rack position `i`.
pub(crate) fn sampler_channel(i: u16, name: &str, mixer_track: u16) -> Channel {
    Channel {
        id: os_model::Id::NONE,
        index: i,
        kind: ChannelKind::Sampler,
        name: Some(name.into()),
        plugin: None,
        plugin_state: None,
        color: None,
        enabled: true,
        volume: 10000.0 / 12800.0,
        pan: 0.0,
        pitch_cents: 0,
        mixer_track,
        sample_path: None,
        layer_children: Vec::new(),
        envelopes: Default::default(),
        rack: Default::default(),
        misc: Default::default(), sound: Default::default(),
        lemon_icon: None,
    }
}

/// `retime` in the session: the project's and the history's ticks; the views follow (`retime_views`).
pub(crate) fn retime_in(s: &mut crate::session::Session, from: u16, to: u16) {
    if from == to {
        return;
    }
    s.project.get_mut().set_ppq(to);
    crate::history::rescale(s, from, to);
    s.touched.views.push(crate::session::View::Retimed(from, to));
    s.dirty = true;
}

/// A generator-plugin channel played by a hosted instrument, routed like `add_channel`. Returns its rack row.
pub(crate) fn add_plugin_channel(p: &mut Project, name: &str) -> usize {
    let row = add_channel(p);
    let ch = &mut p.channels[row];
    ch.kind = ChannelKind::Plugin;
    // The built-in Lemon Trio is stored as the file's own 3x Osc, Citrus as its Sytrus, Smith as its Ogun.
    ch.plugin = Some(crate::osc3::file_name(name).to_string());
    ch.name = Some(name.to_string());
    row
}

/// Add-channel button (spec: ui/channel-rack.md §5): an empty sampler routed to the next insert
/// after the highest one in use. Returns its rack row.
pub(crate) fn add_channel(p: &mut Project) -> usize {
    let index = p.channels.iter().map(|c| c.index + 1).max().unwrap_or(0);
    let track = (p.channels.iter().map(|c| c.mixer_track).max().unwrap_or(0) + 1).min(crate::mixer::max_insert(p));
    p.channels.push(sampler_channel(index, crate::plugins::LEMON_SAMPLER, track));
    p.channels.len() - 1
}

/// A new project (owner's choice, 2026-10-10): one empty sampler channel, played and edited by Lemon Sampler, on
/// insert 1, and pattern 1.
/// `now` is the project as `saved` but for what FL does not count as an unsaved change though the file keeps it
/// (`ui/undo-steps.md`): the channels' locks, the song mode, the filter groups and filter shown, and the patterns' loops.
pub(crate) fn same_as_saved(saved: &Project, now: &Project) -> bool {
    if saved == now {
        return true;
    }
    let mut now = now.clone();
    (now.song_mode, now.groups, now.current_group) = (saved.song_mode, saved.groups.clone(), saved.current_group);
    if saved.channels.len() == now.channels.len() {
        for (x, y) in saved.channels.iter().zip(&mut now.channels) {
            (y.rack.locked, y.rack.group) = (x.rack.locked, x.rack.group);
        }
    }
    // A pattern made only to hold loop settings (loop mode switched on in an empty one) is no unsaved change either.
    let loops_only = |q: &os_model::Pattern| {
        (!q.loops.is_empty() || q.loop_mode.is_some())
            && q.notes.is_empty()
            && q.controls.is_empty()
            && q.name.is_none()
            && q.color.is_none()
            && q.length.is_none()
            && q.markers.is_empty()
    };
    now.patterns.retain(|q| saved.pattern(q.number).is_some() || !loops_only(q));
    // Loop mode, type and points: steps of their own in FL, but no unsaved change.
    for q in &mut now.patterns {
        let (loops, mode) = saved.pattern(q.number).map_or((Vec::new(), None), |x| (x.loops.clone(), x.loop_mode));
        (q.loops, q.loop_mode) = (loops, mode);
    }
    *saved == now
}

pub fn default_project() -> Project {
    template_project(&[crate::plugins::LEMON_SAMPLER])
}

/// A one-channel project that plays `path` once from the start (browser sample preview, spec: ui/browser.md §7).
pub(crate) fn preview_project(path: &str) -> Project {
    let mut p = template_project(&["Preview"]);
    p.channels[0].sample_path = Some(path.to_string());
    p.patterns = vec![Pattern { number: 1, notes: vec![Note { length: u32::from(p.ppq.max(4)), ..Note::step(0, 0) }], ..Pattern::default() }];
    p.arrangements.clear();
    p
}

/// A project with no channels: the mixer and pattern 1 of a new one (FILE ▸ Import ▸ MIDI file adds its channels).
pub(crate) fn empty_project() -> Project {
    template_project(&[])
}

/// The old five-channel template (tests that need several channels).
#[cfg(test)]
pub fn test_project() -> Project {
    template_project(&["Kick", "Clap", "Hat", "Snare", "Pluck"])
}

/// Sampler channels named `names` (channel i on insert i + 1), 18 mixer tracks (master, Insert 1-16 and the hidden
/// last one, as FL's default template stores them; spec: ui/mixer.md §1) routed to the master, pattern 1.
fn template_project(names: &[&str]) -> Project {
    let channel = |i: u16, name: &str| sampler_channel(i, name, i + 1);
    let mut mixer: Vec<MixerTrack> = (0..18u16).map(MixerTrack::new).collect();
    // The hidden last track is routed nowhere, as FL stores it (spec: flp/events/mixer.md).
    mixer[17].routes.clear();
    Project {
        ppq: 96,
        tempo: Some(130.0),
        current_pattern: Some(1),
        channels: names.iter().enumerate().map(|(i, n)| channel(i as u16, n)).collect(),
        patterns: vec![Pattern { number: 1, ..Pattern::default() }],
        mixer,
        // FL's default template holds event 30 = 1: "Play truncated notes in clips" is on in a new project (spec:
        // flp/engine-events.md "Project", "Event 30").
        play_truncated: true,
        // Automatic crossfades (event 44) as the program setting says, whatever the template holds (spec:
        // ui/settings-choices.md "Project").
        auto_crossfades: Some(crate::prefs::check(&crate::prefs::AUTO_CROSSFADES_NEW)),
        ..Project::default()
    }
}
