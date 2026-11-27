// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! A sample dropped over a plugin window, from the system's file manager or the browser, goes into that plugin: Lemon
//! Sampler and Lemon Slicer load it, and nothing else in the app gets it (spec: ui/plugin-window-sample-drop.md).

use std::cell::Cell;
use std::path::PathBuf;

use crate::app::App;
use crate::views::browser::Item;

thread_local! {
    /// The row whose plugin window the tests put under the mouse pointer (`App::plugin_window_under_cursor`).
    pub static PLUGIN_UNDER_CURSOR: Cell<Option<usize>> = const { Cell::new(None) };
}

/// The test project with Lemon Sampler on its sampler channels and Lemon Slicer on a channel of its own (loaded with
/// `first`); returns the app and the Slicer's row.
fn app(first: &str) -> (App, usize) {
    let mut a = App::new(None);
    a.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
    a.state.session.plugins = crate::plugins::Plugins::default();
    a.state.session.plugins.available = crate::plugins::builtin();
    a.plugin_tick();
    assert!(a.state.session.hosted_rows.contains(&0), "Lemon Sampler on channel 1");
    a.sample_from_plugin(first.to_string(), os_hostlink::AudioTo::Slicer);
    let slicer = a.state.session.project.channels.len() - 1;
    assert!(a.state.session.plugins.slicer_file_state(slicer).is_some(), "Lemon Slicer on the last channel");
    a.state.message = None;
    (a, slicer)
}

fn wav(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("os-app-plugin-drop-{}-{name}.wav", std::process::id()));
    let ramp: Vec<f32> = (0..22050).flat_map(|k| [(k % 100) as f32 / 100.0 - 0.5; 2]).collect();
    os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&p).unwrap(), 44100, &ramp).unwrap();
    p
}

fn slicer_path(a: &App, row: usize) -> String {
    let st = a.state.session.plugins.slicer_file_state(row).expect("Lemon Slicer");
    lemon_slicer::state::State::parse(&st).expect("its state").path().to_string()
}

fn under(row: Option<usize>) {
    PLUGIN_UNDER_CURSOR.with(|c| c.set(row));
}

/// A file from the system's file manager dropped over Lemon Sampler's window replaces its sample (and the channel's);
/// over Lemon Slicer's it replaces the loop. No channel is added, whatever lies under the window.
#[test]
fn system_drop_over_a_plugin_window_loads_into_it() {
    let first = wav("first");
    let (mut a, slicer) = app(&first.to_string_lossy());
    let channels = a.state.session.project.channels.len();

    let kick = wav("kick");
    under(Some(0));
    a.drop_file(kick.clone());
    a.host_commands();
    assert_eq!(a.state.session.plugins.sampler_path(0).as_deref(), Some(&*kick.to_string_lossy()));
    assert_eq!(a.state.session.project.channels[0].sample_path.as_deref(), Some(&*kick.to_string_lossy()));
    assert_eq!(a.state.session.project.channels.len(), channels, "no new channel");

    let looped = wav("loop");
    under(Some(slicer));
    a.drop_file(looped.clone());
    a.host_commands();
    assert_eq!(slicer_path(&a, slicer), looped.to_string_lossy());
    assert_eq!(a.state.session.project.channels.len(), channels, "no new channel");
    assert!(a.state.message.as_deref().is_some_and(|m| m.contains("loaded in Lemon Slicer")), "{:?}", a.state.message);

    // Over a plugin that takes no samples (a channel of a plugin not hosted here): it stays out, with a message.
    let other = crate::projects::add_plugin_channel(a.state.session.project.get_mut(), "Some Synth");
    under(Some(other));
    a.drop_file(kick.clone());
    assert_eq!(a.state.session.project.channels.len(), channels + 1, "no new channel");
    assert!(a.state.message.as_deref().is_some_and(|m| m.contains("does not take")), "{:?}", a.state.message);
    under(None);
    for p in [first, kick, looped] {
        let _ = std::fs::remove_file(p);
    }
}

/// A sample dragged from the browser and released over Lemon Sampler's or Lemon Slicer's window goes into it.
#[test]
fn browser_drop_over_a_plugin_window_loads_into_it() {
    let first = wav("bfirst");
    let (mut a, slicer) = app(&first.to_string_lossy());
    let channels = a.state.session.project.channels.len();

    let kick = wav("bkick");
    under(Some(0));
    a.browser_drop(Item::Sample(kick.to_string_lossy().into_owned()), true);
    a.host_commands();
    assert_eq!(a.state.session.plugins.sampler_path(0).as_deref(), Some(&*kick.to_string_lossy()));

    let looped = wav("bloop");
    under(Some(slicer));
    a.browser_drop(Item::Sample(looped.to_string_lossy().into_owned()), true);
    a.host_commands();
    assert_eq!(slicer_path(&a, slicer), looped.to_string_lossy());
    assert_eq!(a.state.session.project.channels.len(), channels, "no new channel");
    under(None);
    for p in [first, kick, looped] {
        let _ = std::fs::remove_file(p);
    }
}
