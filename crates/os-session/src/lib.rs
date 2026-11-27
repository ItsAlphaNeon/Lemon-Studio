// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Studio's session: the project and everything that plays, records, loads and saves it, with no
//! window (control-plane part 2, step 9). `os-app` re-exports these modules under the names they had.

#![deny(deprecated)]
// Test helpers go unused in one of the two test builds (this crate's, or `test-util` for the interface's).
#![cfg_attr(any(test, feature = "test-util"), allow(dead_code))]

pub mod audio_cfg;
pub mod audio_log;
pub mod autoscroll;
pub mod backup;
pub mod capture;
pub mod chords;
pub mod citrus;
pub mod claw;
pub mod clip_colors;
pub mod container;
pub mod control;
pub mod data_folder;
pub mod device;
#[cfg(target_os = "linux")]
pub mod dropout_log;
pub mod drums;
pub mod soundfont;
pub mod edit;
pub mod effect_ports;
pub mod enable_groups;
pub mod export;
pub mod factory_folder;
pub mod flam;
pub mod flip;
pub mod history;
pub mod icon;
pub mod init;
pub mod keep_on_disk;
pub mod knob_id;
pub mod lemon_midi_out;
pub mod lemon_state;
pub mod loading;
pub mod locked;
pub mod logo;
pub mod meters;
pub mod metronome;
pub mod midi_cfg;
pub mod midi_in;
pub mod midi_notes;
pub mod midi_ports;
pub mod midi_out;
pub mod misc;
pub mod missing;
pub mod mix_threads;
pub mod native;
pub mod mixer_state;
pub mod monitor;
pub mod note_names;
pub mod one_click;
pub mod osc3;
pub mod panic_log;
pub mod params;
pub mod pattern_render;
pub mod perf;
pub mod perform;
pub mod piano_split;
pub mod plugin_db;
pub mod plugin_scan;
pub mod plugins;
pub mod prefs;
pub mod presets;
pub mod preview_track;
pub mod project_files;
pub mod project_info;
pub mod project_io;
pub mod project_plugins;
pub mod projects;
pub mod record;
pub mod remote;
pub mod replace_file;
#[cfg(target_os = "linux")]
pub mod rt;
pub mod score_log;
pub mod session;
pub mod settings_file;
pub mod slicer;
pub mod smith;
pub mod song_pos;
pub mod stall;
pub mod step_edit;
pub mod time;
pub mod track_links;
pub mod track_rec;
pub mod loop_takes;
pub mod transport;
pub mod typein;
pub mod user_dir;
pub mod version;
pub mod zipped;
