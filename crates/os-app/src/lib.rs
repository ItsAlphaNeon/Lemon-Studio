// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Studio: the application. The binary (`main.rs`) only calls [`run`].

// Plugin windows exist on Windows, Linux and macOS (`plugin_window`): elsewhere what only they reach (the Misc tab, the
// window and preset menus, their requests) is built but unused. Those builds still report real dead code.
#![cfg_attr(not(plugin_windows), allow(dead_code))]
// The interface changes the project through commands only (control-plane part 2, steps 3 and 14): a call of
// `locked::Locked::get_mut` or `write_if` here fails to build. Tests set their projects up directly.
#![cfg_attr(not(test), deny(deprecated))]
#![cfg_attr(test, allow(deprecated))]

pub mod anim;
pub mod app;
pub mod arrange;
pub mod audio_clipboard;
pub mod audio_log_ui;
pub mod help_links;
pub mod diagnostics;
pub use os_session::audio_cfg;
pub mod auto_close;
pub use os_session::autoscroll;
pub use os_session::backup;
pub mod backup_ui;
#[cfg(test)]
pub mod audio_bench;
#[cfg(target_os = "linux")]
pub use os_session::dropout_log;
pub mod editor_host;
pub use os_session::effect_ports;
pub use os_session::time;
pub use os_session::projects;
pub use os_session::user_dir;
#[cfg(test)]
pub mod checks;
#[cfg(test)]
pub mod equiv;
#[cfg(test)]
pub mod open_bench;
pub mod browser;
pub mod browser_index;
pub mod burn_midi;
pub mod cable;
pub use os_session::capture;
pub use os_session::panic_log;
#[cfg(target_os = "linux")]
pub use os_session::rt;
pub use os_session::one_click;
pub mod one_click_ui;
pub use os_session::track_rec;
pub mod track_rec_ui;
pub use os_session::audio_log;
pub use os_session::score_log;
pub mod score_log_ui;
pub use os_session::clip_colors;
pub mod clip_colors_ui;
pub mod clock;
pub mod control;
pub mod control_ui;
pub mod cpu;
pub use os_session::data_folder;
pub mod data_folder_ui;
pub use os_session::device;
pub mod dialog;
pub mod floating;
pub mod filedlg;
pub use os_session::enable_groups;
pub use os_session::export;
pub mod export_form;
pub use os_session::factory_folder;
pub mod factory_folder_ui;
pub mod high_visibility;
pub use os_session::history;
pub mod history_ui;
pub mod hold;
pub use os_session::keep_on_disk;
pub mod keep_on_disk_ui;
pub use os_session::icon;
pub mod icons;
pub use os_session::init;
pub mod init_ui;
pub use os_session::knob_id;
pub mod keys;
pub mod knobs;
pub mod layout;
pub use os_session::lemon_state;
pub mod lemon_state_ui;
pub use os_session::loading;
pub mod loading_ui;
pub use os_session::missing;
pub mod missing_ui;
pub use os_session::mix_threads;
pub mod mix_threads_ui;
pub use os_session::zipped;
pub mod zipped_ui;
pub use os_session::logo;
pub mod messages;
pub use os_session::meters;
pub use os_session::metronome;
pub use os_session::midi_cfg;
pub mod midi_file;
pub use os_session::midi_in;
pub use os_session::midi_out;
pub use os_session::lemon_midi_out;
pub mod lemon_midi_out_ui;
pub use os_session::track_links;
pub mod track_links_ui;
pub mod midi_play;
pub use os_session::container;
pub mod container_ui;
pub mod mixer;
pub use os_session::mixer_state;
pub use os_session::osc3;
pub mod osc3_ui;
pub use os_session::citrus;
pub mod citrus_ui;
pub use os_session::smith;
pub mod smith_ui;
pub mod chord_tool;
pub use os_session::slicer;
pub mod slicer_ui;
pub use os_session::drums;
pub mod drums_ui;
pub use os_session::monitor;
pub mod motion;
pub use os_session::params;
pub mod params_ui;
pub mod piano;
pub mod piano_snap;
pub use os_session::piano_split;
pub mod piano_tools;
pub mod rack;
pub use os_session::remote;
pub mod remote_ui;
pub mod render;
pub use os_session::pattern_render;
pub mod pattern_render_ui;
pub mod patterns;
pub use os_session::perf;
pub use os_session::perform;
pub mod perform_ui;
pub mod playlist;
pub use os_session::plugin_db;
pub mod plugin_manager;
pub mod plugin_menu;
pub mod plugin_palette;
pub mod plugin_popup;
pub use os_session::plugin_scan;
pub mod plugin_scan_ui;
pub mod plugin_window;
pub mod plugins;
pub mod plugins_ui;
pub mod ppi;
pub mod prefs;
pub mod settings_window;
pub use os_session::locked;
pub use os_session::session;
pub use os_session::project_io;
pub use os_session::edit;
pub mod follow;
pub use os_session::preview_track;
pub mod preview_track_ui;
pub use os_session::project_files;
pub mod project_files_ui;
pub mod project_forms;
pub use os_session::project_info;
pub mod project_info_ui;
pub mod project_picker;
pub use os_session::project_plugins;
pub mod project_plugins_ui;
pub use os_session::presets;
pub mod presets_ui;
pub use os_session::misc;
pub mod misc_ui;
pub mod misc_page;
pub use os_session::note_names;
pub mod note_names_ui;
pub mod save_form;
pub mod scroll_speed;
pub mod search_folders;
pub mod settings_file;
pub mod update;
pub use os_session::version;
pub mod shortcuts;
pub mod smooth;
pub mod snap;
pub use os_session::song_pos;
pub mod song_pos_ui;
pub mod toolbar;
pub use os_session::stall;
pub use os_session::step_edit;
pub mod step_edit_ui;
pub mod strum;
pub use os_session::flam;
pub use os_session::flip;
pub mod levels;
pub mod channel_states;
pub use os_session::chords;
pub use os_session::claw;
pub mod limit;
pub mod lfo;
pub mod quantize;
pub mod randomize;
pub mod chop;
pub mod piano_dlg;
pub mod arpeggiate;
pub mod tapper;
pub mod touch;
pub mod events;
pub use os_session::record;
pub mod record_ui;
pub mod recent;
pub mod theme_adjust;
pub mod theme_library;
pub mod templates;
pub mod timesig;
pub use os_session::transport;
pub use os_session::typein;
pub mod typein_ui;
pub mod views;
pub mod wallpaper;
pub mod waves;
pub mod win_clamp;
#[cfg(windows)]
pub mod file_assoc;

/// The UI's scale (device pixels per logical one) on a monitor whose own is `system`: Settings > General > Display >
/// Main GUI scaling keeps it (System) or replaces it with a percentage (spec: ui/ui-scaling.md §1).
pub fn gui_scale(system: f64) -> f64 {
    let pct = prefs::choice(&prefs::MAIN_GUI_SCALING).strip_suffix('%').and_then(|p| p.parse::<f64>().ok());
    pct.map_or(system, |p| p / 100.0)
}

/// The program's way to wake its event loop from any thread (`set_waker`); nothing until it sets one.
static WAKER: std::sync::OnceLock<fn()> = std::sync::OnceLock::new();

/// The program gives the interface its wake: hosted plugins and the updater call it to have the next tick come now.
pub fn set_waker(wake: fn()) {
    let _ = WAKER.set(wake);
}

/// Wakes the program's event loop (`set_waker`).
pub fn wake_ui() {
    if let Some(w) = WAKER.get() {
        w();
    }
}
