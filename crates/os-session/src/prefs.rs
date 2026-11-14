// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Program settings of the settings window, declared once below: checks and combos kept in the program's settings file
//! (`crate::settings_file`: `settings.txt` in the user folder, a missing or unreadable value is the default), never in
//! the project. The settings page draws a declared control live: a check toggles on a click; a combo shows its
//! choice and opens the list of its choices, the current one radio-marked. A change is kept at once. Undeclared
//! controls (without a module of their own) stay greyed out.
//!
//! Making a greyed-out check or combo of the settings window work:
//! 1. Declare it: a `pub const` [`Check`] or [`Combo`] whose `label` is the control's label in
//!    `views::settings::page` (the page finds it by it) and whose `key` names it in the file (stable: a later change of
//!    label keeps what users chose); then list it in [`CHECKS`] or [`COMBOS`].
//!    ```ignore
//!    /// General ▸ Miscellaneous (spec: ui/...): what it does.
//!    pub const AUTO_NAME_CHANNELS: Check = Check { key: "auto_name_channels", label: "Name channels after their sample or plugin", default: true };
//!    ```
//! 2. Read it where its behaviour lives: `prefs::check(&prefs::AUTO_NAME_CHANNELS)` is the check's state;
//!    `prefs::choice(&prefs::MAIN_GUI_SCALING)` is the chosen entry's text (`prefs::index` its position).
//!
//! That is all: a value is read when it is needed, so a change counts from the next read (a setting that must also act
//! the moment it changes adds that to `App::pref_toggle` / `App::pref_choose`). Settings with a module and file of
//! their own (audio, auto scrolling, metronome, snap, enable groups, animations, the plugin manager's) and combos whose
//! choices are found at run time (output devices) are not declared here.
//!
//! Sliders ([`Slider`], listed in [`SLIDERS`]) are declared the same way and read with `prefs::slider`.
// spec: ui/settings.md, ui/options-coverage.md

use crate::settings_file::{read, write, Store}; // This code is a part of L e m o n S t u d i o.


/// A check (LED) on a settings page.
#[derive(Debug, PartialEq)]
pub struct Check {
    /// Its name in `settings.txt`.
    pub key: &'static str,
    /// The label its page shows; unique among the declared settings.
    pub label: &'static str,
    pub default: bool,
}

/// A combo on a settings page: the box shows the current choice, its menu lists them all.
#[derive(Debug, PartialEq)]
pub struct Combo {
    /// Its name in `settings.txt`.
    pub key: &'static str,
    /// The label its page shows; unique among the declared settings.
    pub label: &'static str,
    /// As the box and the menu show them; the file keeps the chosen one's text.
    pub choices: &'static [&'static str],
    /// Index in `choices`.
    pub default: usize,
}

/// A value box on a settings page (the red box with a number): the mouse wheel changes it, one per notch, within
/// `min..=max` (`App::settings_number`).
#[derive(Debug, PartialEq)]
pub struct Number {
    /// Its name in `settings.txt`.
    pub key: &'static str,
    /// The label its page shows; unique among the declared settings.
    pub label: &'static str,
    pub min: u16,
    pub max: u16,
    pub default: u16,
}

/// A slider on a settings page: a whole number from 0 to `max`, its handle as far along the track as the value is. A
/// click or a drag sets it from where the mouse is along the track, a wheel notch over it moves it by `step`; it shows
/// no value (`App::pref_slider_press`, `App::pref_slider_wheel`).
#[derive(Debug, PartialEq)]
pub struct Slider {
    /// Its name in `settings.txt`.
    pub key: &'static str,
    /// The label its page shows; unique among the declared settings.
    pub label: &'static str,
    pub max: u16,
    pub default: u16,
    /// What one wheel notch over it adds or takes.
    pub step: u16,
}

/// General ▸ Display: the UI's scale. System = the monitor's (the operating system's); a percentage replaces it.
/// Read by `crate::gui_scale` at every event and paint, so it applies at once (a deliberate difference: the reference
/// needs a restart). Spec: ui/ui-scaling.md §1.
pub const MAIN_GUI_SCALING: Combo = Combo {
    key: "main_gui_scaling",
    label: "Main GUI scaling",
    choices: &["System", "100%", "125%", "150%", "175%", "200%", "250%", "300%", "350%", "400%"],
    default: 0,
};

/// General ▸ Display: the scale of popup menus and combo lists. Main = the main GUI's; a percentage replaces it. Read
/// by `views::popup_scale` at every paint, so it applies at once (a deliberate difference: the reference needs a
/// restart). Spec: ui/ui-scaling.md §1–2.
pub const POPUPS_SCALING: Combo = Combo {
    key: "popups_scaling",
    label: "Pop-ups scaling",
    choices: &["Main", "100%", "125%", "150%", "175%", "200%", "250%", "300%", "350%", "400%"],
    default: 0,
};

/// General ▸ Display: the scale of the main window's top band (window buttons, hint panel, transport, menu bar,
/// toolbar). Main = the main GUI's; a percentage replaces it. Everything below the band keeps the main scale; menus
/// opened from the band keep Pop-ups scaling. Read by `views::toolbar_scale` at every event and paint, so it applies at
/// once, with no restart. Spec: ui/ui-scaling.md §1 "Toolbars scaling".
pub const TOOLBARS_SCALING: Combo = Combo {
    key: "toolbars_scaling",
    label: "Toolbars scaling",
    choices: &["Main", "100%", "125%", "150%", "175%", "200%", "250%", "300%", "350%", "400%"],
    default: 0,
};

/// General ▸ Display: sample waveforms coloured by what they are made of: Off, Color map (the Waves gradient by
/// spectral centroid) or Multiband (three colours by band power); `crate::waves`. It takes effect at once. Spec:
/// ui/settings-choices.md "Display: Colorful waveforms (2026-10-16)".
pub const COLORFUL_WAVEFORMS: Combo =
    Combo { key: "colorful_waveforms", label: "Colorful waveforms", choices: &["Off", "Color map", "Multiband"], default: 0 };

/// General ▸ Language: how note names read (`crate::note_names`; names change at once, a deliberate difference: the
/// reference asks for a restart).
/// Spec: ui/settings-choices.md "Note names and Black notes".
pub const NOTE_NAMES: Combo = Combo { key: "note_names", label: "Note names", choices: &["English", "Germanic", "Solfege (do, re, mi, ...)"], default: 0 };

/// General ▸ Language: black keys as sharps of the key below or flats of the key above (`crate::note_names`).
pub const BLACK_NOTES: Combo = Combo { key: "black_notes", label: "Black notes", choices: &["Sharps - #", "Flats - \u{266d}"], default: 0 };

/// File ▸ Backup: how often the project is backed up (`crate::backup`). Spec: ui/settings-choices.md "Backup files".
pub const AUTOSAVE: Combo = Combo {
    key: "autosave",
    label: "Autosave",
    choices: &[
        "Never (just remind every 10 minutes)",
        "Rarely (every 15 minutes)",
        "Occasionally (every 10 minutes)",
        "Regularly (every 5 minutes)",
        "Frequently (every 5 minutes & before risky operations)",
        "Very frequently (every minute & before risky operations)",
    ],
    default: 2,
};

/// File ▸ Backup: where backups go. "Save to the project data folder if there is one" writes a project with a data
/// folder to `<data folder>/Backup` (`crate::data_folder::backup_folder`). Spec: ui/settings-choices.md "Backup files",
/// ui/data-folder.md §3.
pub const BACKUP_LOCATION: Combo = Combo {
    key: "backup_location",
    label: "Location",
    choices: &["Always save to the global folder", "Save to the project data folder if there is one"],
    default: 1,
};

/// File ▸ Backup: the most autosaves kept per project name; after a backup the oldest beyond it are deleted
/// (`crate::backup::prune`). Lowering it deletes nothing until the next backup. The range, 1 to 999, is a guess:
/// the reference's range is unverified. Spec: ui/settings-choices.md "Backup files".
pub const BACKUP_MAXIMUM: Number = Number { key: "backup_maximum", label: "Maximum", min: 1, max: 999, default: 100 };

/// General ▸ Miscellaneous: unsolo puts back the mute states from before the first solo (channel rack, playlist,
/// mixer); off, unsolo turns everything on. Spec: ui/settings-choices.md "Miscellaneous: what four checks do".
pub const RESTORE_AFTER_SOLO: Check = Check { key: "restore_after_solo", label: "Restore previous state after solo", default: false };

/// General ▸ Miscellaneous: switching pattern selects the first channel with notes in it (`App::pattern_tick`).
pub const SELECT_FIRST_CHANNEL: Check =
    Check { key: "select_first_channel", label: "Select first channel with notes when the pattern changes", default: false };

/// General ▸ Miscellaneous: opening the piano roll zooms on the pattern and the channel's notes (`crate::piano::auto_zoom`).
pub const AUTO_ZOOM_PIANO: Check = Check { key: "auto_zoom_piano", label: "Auto zoom in piano roll", default: false };

/// General ▸ Miscellaneous: thin editor scroll bars without arrow buttons (`crate::views::small_scrollbars`).
pub const SMALL_SCROLLBARS: Check = Check { key: "small_scrollbars", label: "Small scrollbars in editors", default: false };

/// General ▸ Miscellaneous: a pattern switch zips the channels with nothing in the new pattern (`App::pattern_tick`).
/// Spec: ui/settings-choices.md "Miscellaneous: seven more checks, Default template and Startup project".
pub const AUTO_ZIP_EMPTY: Check = Check { key: "auto_zip_empty_channels", label: "Auto-collapse empty channels", default: false };

/// General ▸ Miscellaneous: a painted note deletes the notes of other keys under it (`crate::piano::paint_mono`).
/// Spec: ui/settings-choices.md "Miscellaneous: seven more checks, Default template and Startup project".
pub const MONO_PAINTING: Check = Check { key: "monophonic_painting", label: "Monophonic painting in piano roll", default: false };

/// Project ▸ Playlist: a new project's "Automatic crossfades" (event 44) takes this value, whatever the default
/// template holds (`app::default_project`); opening a file keeps the file's. A program setting, on by default. Spec:
/// ui/settings-choices.md "Project" (the storage table).
pub const AUTO_CROSSFADES_NEW: Check =
    Check { key: "auto_crossfades_new_projects", label: "Enable automatic crossfades in new projects", default: true };

/// Audio ▸ Mixer: Play, and a jump of the position while playing, start the notes that are under way at the new
/// place, afresh and for what is left of them; off, those notes are skipped. Read at each start (`transport::Starts`).
/// Spec: ui/settings-choices.md "Transport: Play truncated notes and Reset plugins".
pub const PLAY_TRUNCATED_ON_TRANSPORT: Check =
    Check { key: "play_truncated_on_transport", label: "Play truncated notes on transport", default: true };

/// Audio ▸ Mixer: Play, and a jump of the position while playing, reset every hosted plugin, the effects included
/// (their tails stop); off, the hosted effects ring on through the start. Stop resets them either way
/// (audio/transport.md). Read at each start (`transport::Starts`). Spec: ui/settings-choices.md "Transport: Play
/// truncated notes and Reset plugins".
pub const RESET_PLUGINS_ON_TRANSPORT: Check =
    Check { key: "reset_plugins_on_transport", label: "Reset plugins on transport", default: false };

/// General ▸ Miscellaneous: every meter that uses the mixer's level law, and the "C" scale with its hover read-out,
/// go by a second dB scale with more room near 0 dB: marks 0 … 30, no +3 (`meters::alternate_db`). Spec:
/// ui/settings-choices.md "Miscellaneous: seven more checks, Default template and Startup project".
pub const ALTERNATE_METER_SCALE: Check = Check { key: "alternate_meter_scale", label: "Alternate meter scale", default: false };

/// General ▸ Miscellaneous: without a time selection, "Create automation clip" starts the clip at the song position;
/// off, at bar 1 (`playlist::automation_span`). Spec: ui/settings-choices.md "Miscellaneous: seven more checks,
/// Default template and Startup project".
pub const AUTOMATION_AT_PLAYBACK_POSITION: Check =
    Check { key: "automation_clips_at_playback_position", label: "Create automation clips at the playback position", default: true };

/// General ▸ Miscellaneous: opening or focusing a plugin window selects its mixer track
/// (`App::select_window_track`). Spec: ui/settings-choices.md "Miscellaneous: seven more checks, Default template and
/// Startup project".
pub const AUTO_SELECT_LINKED: Check = Check { key: "auto_select_linked_modules", label: "Select linked controls automatically", default: true };

/// General ▸ Miscellaneous: stored and shown only. The reference shows no change on native plugin windows; it has no
/// effect until hosted plugin windows get a toolbar (spec: ui/settings-choices.md, as above).
pub const HIDE_PLUGIN_TOOLBAR: Check =
    Check { key: "hide_plugin_window_toolbar", label: "Hide plugin window strip by default", default: true };

/// General ▸ Miscellaneous: a sample or channel preset loaded into an existing channel renames the channel after the
/// file (`rack::auto_name`); off, the channel keeps its name. Spec: ui/settings-choices.md "Miscellaneous: naming,
/// detached plugins and startup".
pub const AUTO_NAME_CHANNELS: Check = Check { key: "auto_name_channels", label: "Name channels after their sample or plugin", default: true };

/// General ▸ Miscellaneous: a preset loaded into an effect renames its mixer slot after the preset
/// (`presets::auto_name`); off, the slot keeps its name. Spec: ui/settings-choices.md, as above.
pub const AUTO_NAME_EFFECT_SLOTS: Check =
    Check { key: "auto_name_effect_slots", label: "Name effect slots after their plugin", default: false };

/// General ▸ Miscellaneous: a plugin added from now on (a new channel's, a new effect in a slot) opens its window
/// detached (`Plugins::added`); plugins that exist keep their own mode. Spec: ui/settings-choices.md "Miscellaneous:
/// naming, detached plugins and startup".
pub const DETACH_NEW: Check = Check { key: "detach_new_plugins", label: "Open new plugin windows floating", default: false };

/// General ▸ Miscellaneous: a detached window opening for the first time goes on the main window's monitor; off, on
/// the primary monitor (`plugin_window::first_position`, Windows only). A reopened one goes where it was either way.
/// Spec: as above.
pub const DETACHED_WITH_MAIN: Check =
    Check { key: "detached_with_main_window", label: "Open detached plugins with the main window", default: false };

/// General ▸ Miscellaneous: stored and shown only (native plugins have no typing keyboard option). Spec: as above.
pub const TYPING_PRIORITY_OFF: Check =
    Check { key: "disable_typing_priority", label: "Disable computer keyboard priority by default", default: false };

/// General ▸ Miscellaneous: at most `plugins::MAX_EDITORS` plugin editor windows open; opening another first closes the
/// one opened earliest (`Plugins::open_window`). Spec: as above.
pub const LIMIT_EDITORS: Check = Check { key: "limit_plugin_editors", label: "Limit the number of open plugin editors", default: true };

/// General ▸ Miscellaneous: at start, a borderless window centred on the primary monitor shows the logo mark until the
/// main window shows (`crate::splash`). A program setting, read at start. Spec: ui/settings-choices.md "Miscellaneous:
/// naming, detached plugins and startup".
/// General ▸ Miscellaneous: windows of one plugin, drawn in the app's, are tabs of one window (`plugin_tabs`).
pub const PLUGIN_TABS: Check = Check { key: "plugin_window_tabs", label: "Show windows of one plugin as tabs", default: true };

/// General ▸ Miscellaneous: those tabs lie partly over each other instead of side by side.
pub const PLUGIN_TABS_STACKED: Check = Check { key: "plugin_window_tabs_stacked", label: "Stack plugin tabs", default: false };

pub const SHOW_SPLASH: Check = Check { key: "show_startup_splash", label: "Show startup splash screen", default: true };

/// General ▸ Miscellaneous: stored and shown only. Lemon Studio has no startup sound (and does not use the
/// reference's); if it ever gets one, this check silences it. Spec: ui/settings-choices.md "Miscellaneous: naming,
/// detached plugins and startup".
pub const SILENT_STARTUP: Check = Check { key: "silent_startup", label: "Silent startup", default: false };

/// General ▸ Updates: at start, a release build checks its channel (nightly or stable) for a newer build
/// (`crate::update`). A build from source never checks.
pub const AUTO_CHECK_UPDATES: Check = Check { key: "auto_check_updates", label: "Automatically check for updates", default: true };

/// General ▸ Updates: a newer build found is downloaded right away, so Install and restart has nothing to wait
/// for; off, it is downloaded once Install is chosen.
pub const AUTO_DOWNLOAD_UPDATES: Check =
    Check { key: "auto_download_updates", label: "Automatically download program updates", default: true };

/// General ▸ Input: off, a left or a right click on a step of the step sequencer toggles it; on, the left button only
/// sets steps and the right one only clears them. A drag sets (left) or clears (right) the steps it crosses either way
/// (`App::rack_step`). Spec: ui/settings-choices.md "Input: Click-and-hold and both mouse buttons (2026-10-16)".
pub const BOTH_MOUSE_BUTTONS: Check =
    Check { key: "both_mouse_buttons_step_sequencer", label: "Right button clears steps", default: false };

/// General ▸ Miscellaneous: the project Lemon Studio starts with when no project file is passed to it
/// (`App::startup_project`). Spec: ui/settings-choices.md, as above. Cutting-edge: "Last used project" by default, and
/// it puts the windows back too (`crate::session_restore`): closing costs nothing.
pub const STARTUP_PROJECT: Combo =
    Combo { key: "startup_project", label: "Startup project", choices: &["Empty project", "Default template", "Last used project"], default: 2 };

/// General ▸ Input: a held click that doesn't move does more: in the piano roll the Draw tool's new note cycles plain,
/// slide, porta, and a note held on is cloned; in the playlist a clip held on is cloned (`crate::hold`). Spec:
/// ui/settings-choices.md "Input: Click-and-hold and both mouse buttons (2026-10-16)".
pub const CLICK_AND_HOLD: Check = Check { key: "click_and_hold_gestures", label: "Click-and-hold gestures", default: false };

/// General ▸ Undo history: the most steps the project's undo history keeps; past it the oldest step goes
/// (`crate::history`). The range, 1 to 999, is a guess: the reference's range is unverified. Spec:
/// ui/settings-choices.md "Undo history: the four checks (2026-10-16)".
pub const MAX_UNDO_LEVELS: Number = Number { key: "max_undo_levels", label: "Maximum undo levels", min: 1, max: 999, default: 100 };

/// General ▸ Undo history: knob turns are undo steps; off, they make none (`history::knob`). Spec: as above.
pub const UNDO_KNOB_TWEAKS: Check = Check { key: "undo_knob_tweaks", label: "Undo knob tweaks", default: true };

/// General ▸ Undo history: on (classic), Ctrl+Z steps back once more per press and Ctrl+Alt+Z redoes; off, Ctrl+Z
/// toggles the last move and Ctrl+Alt+Z steps back further (`history::ctrl_z`). EDIT follows. Spec: as above.
pub const ALTERNATE_UNDO: Check = Check { key: "alternate_undo_mode", label: "Alternate undo mode", default: true };

/// General ▸ Undo history: stored and shown only. Undoing a recording ("Recorded items") keeps its files in the
/// Recorded folder for now, since a redo puts its clips back; nothing moves them to the trash yet. Spec: as above (not
/// measured).
pub const UNDONE_RECORDINGS_TO_BIN: Check =
    Check { key: "undone_recordings_to_recycle_bin", label: "Move undone recordings to the trash", default: true };

/// General ▸ Undo history: Ctrl+Z also shows the browser's Current project ▸ History, opened
/// (`browser::show_history`). Spec: as above.
pub const UNDO_HISTORY_IN_BROWSER: Check =
    Check { key: "show_undo_history_in_browser", label: "Show undo history in the browser when undoing changes", default: false };

/// General ▸ Undo history: the browser's History lists the newest step first, "Last reset" last (`History::entries`).
/// Spec: as above.
pub const RECENT_CHANGES_TOP: Check = Check { key: "undo_history_newest_first", label: "Show recent changes at the top", default: false };

/// General ▸ Advanced: a WAV loaded into a sampler channel sets its root note to the file's `smpl` unity note, C5 (60)
/// without one; off, the root note stays (`rack::read_tags`). Spec: ui/settings-choices.md "Advanced: Read sample root
/// note and Read sample tempo information (2026-10-16)".
pub const READ_ROOT_NOTE: Check = Check { key: "read_sample_root_note", label: "Read sample root note", default: true };

/// General ▸ Advanced: a WAV with an `acid` beat count loaded into a sampler channel is fitted to those beats (stretch
/// method Auto, TIME the beat count); without one, or off, the time stretching stays (`rack::read_tags`). Spec: as
/// above.
pub const READ_TEMPO: Check = Check { key: "read_sample_tempo", label: "Read sample tempo information", default: true };

/// General ▸ Display: the most frames a second while something animates (`smooth::frame_cap`): the display's own rate
/// (Ultrasmooth), 60 (Smooth) or 40 (Less smooth). Spec: ui/settings-choices.md "Display: Smoothing, Animation refresh
/// rate, Transparent menus and windows (2026-10-16)".
pub const ANIMATION_REFRESH_RATE: Combo =
    Combo { key: "animation_refresh_rate", label: "Animation refresh rate", choices: &["Less smooth", "Smooth", "Ultrasmooth"], default: 2 };

/// General ▸ Advanced: about detached plugin windows (top-level windows of their own). Off, one dragged by its title
/// bar keeps the title bar on the screens; on, it goes anywhere (`win_clamp::bar_on_screens`, Windows only: on Linux
/// and macOS detached windows are not limited either way yet). Windows inside the main window keep their part in the
/// workspace whatever this says (`crate::win_clamp`). Spec: ui/settings-choices.md "Advanced: Don't limit windows to
/// screen (2026-10-16)".
pub const DONT_LIMIT_WINDOWS: Check = Check { key: "dont_limit_windows_to_screen", label: "Don't limit windows to screen", default: false };

/// General ▸ Advanced: "Create automation clip" also puts the control in the project's initial values with its value at
/// that moment, replacing an earlier one, in the same undo step (`playlist::create_automation`, `crate::init`). Spec:
/// ui/settings-choices.md "Advanced: Initialize controls automatically (2026-10-16)".
pub const INIT_CONTROLS: Check = Check { key: "init_controls_automatically", label: "Set start values automatically", default: true };

/// General ▸ Advanced: stored and shown only. Lemon Studio's browser preview already starts as soon as the start of
/// the file is decoded (ui/browser.md §7), and the reference shows no difference either. Spec: ui/settings-choices.md
/// "Advanced: Fast sample preview, Auto keep long audio on disk, Threaded project sample loading (2026-10-16)".
pub const FAST_SAMPLE_PREVIEW: Check = Check { key: "fast_sample_preview", label: "Fast sample preview", default: true };

/// General ▸ Advanced: a file of 30 s or more loaded into a sampler channel turns its Keep on disk on; nothing turns it
/// off (`crate::keep_on_disk`). Spec: as above.
pub const AUTO_KEEP_ON_DISK: Check = Check { key: "auto_keep_long_audio_on_disk", label: "Auto keep long audio on disk", default: true };

/// General ▸ Advanced: an opened project's samples are decoded on up to 4 worker threads; off, one file at a time on
/// one (`loading::workers`, read when a project starts loading). Spec: as above.
pub const THREADED_SAMPLE_LOADING: Check =
    Check { key: "threaded_project_sample_loading", label: "Threaded project sample loading", default: false };

/// General ▸ Advanced: stored and shown only (the choices of Main GUI scaling). Lemon Studio draws no cursors of its
/// own yet: the system's follow the system's size. Spec: ui/settings-choices.md "Advanced: Cursors scaling and Legacy
/// scaling (2026-10-16)".
pub const CURSORS_SCALING: Combo = Combo {
    key: "cursors_scaling",
    label: "Cursors scaling",
    choices: &["System", "100%", "125%", "150%", "175%", "200%", "250%", "300%", "350%", "400%"],
    default: 0,
};

/// General ▸ Advanced: stored and shown only; meant for hosted plugin windows that don't follow the display scale,
/// which keep the size their plugin gives them until how the reference scales them is known. Spec: as above.
pub const LEGACY_SCALING: Combo =
    Combo { key: "legacy_scaling", label: "Legacy scaling", choices: &["Main", "100%", "200%", "300%", "400%"], default: 0 };

/// General ▸ Display: some controls drawn brighter, in both looks, at once (a deliberate difference: the reference
/// asks for a restart): the record dot, the rack's names, track numbers and overview toggle, every scroll bar and the
/// playlist's horizontal one (`crate::high_visibility`). Spec: ui/settings-choices.md "Display: PPI ruler, Force
/// refreshes, Thick lines, High visibility (2026-10-16)".
pub const HIGH_VISIBILITY: Check = Check { key: "high_visibility", label: "High visibility", default: false };

/// General ▸ Display: stored and shown only. Lemon Studio redraws its whole window itself each frame, so there is
/// nothing to force. Spec: ui/settings-choices.md "Display: PPI ruler, Force refreshes, Thick lines, High visibility
/// (2026-10-16)".
pub const FORCE_REFRESHES: Check = Check { key: "force_refreshes", label: "Force refreshes", default: true };

/// General ▸ Display: stored and shown, greyed out while the effective UI scale is under 200 %
/// (`views::settings::thick_lines_live`). Lemon Studio already draws its lines at the scale's width, so it changes
/// nothing yet. Spec: as above.
pub const THICK_LINES: Check = Check { key: "thick_lines", label: "Thick lines", default: false };

/// General ▸ Display: a menu goes see-through behind its open submenu (`Motion::menu_behind`). Spec: ui/animations.md
/// §4.2.
pub const TRANSPARENT_MENUS: Check = Check { key: "transparent_menus", label: "Transparent menus", default: true };

/// General ▸ Display: a window that isn't the active one shows what is behind it (`Motion::window_opacity`). Spec:
/// ui/animations.md §4.9.
pub const TRANSPARENT_WINDOWS: Combo = Combo { key: "transparent_windows", label: "Transparent windows", choices: &["Off", "On"], default: 0 };

/// General ▸ Input: stored and shown only. Lemon Studio has no pen or touch input yet; when it does, these choices are
/// where it starts. Spec: ui/settings-choices.md "Input: Optimize for (2026-10-16)".
pub const OPTIMIZE_FOR: Combo =
    Combo { key: "optimize_for", label: "Optimize for", choices: &["Multi-button mouse", "Pencil", "Multi-touch monitor"], default: 0 };

/// General ▸ Input: stored and shown only. The windowing system hands Lemon Studio a pen's tip and barrel button as the
/// mouse's left and right buttons, so there is nothing to swap yet; when Lemon Studio reads pen input, on swaps what the
/// tip and the barrel button do. Spec: ui/settings-choices.md "Input: Optimize for (2026-10-16)" (Invert pencil
/// buttons).
pub const INVERT_PENCIL_BUTTONS: Check = Check { key: "invert_pencil_buttons", label: "Swap mouse buttons for drawing", default: false };

/// Audio ▸ Input / output: while another program is in front, the audio output closes and a playing transport pauses;
/// both come back with the focus (`crate::auto_close`). Spec: ui/settings-choices.md "Input / output: Auto close; CPU
/// and Mixer: Smart disable, Align tick lengths, Resampling quality (2026-10-16)".
pub const AUTO_CLOSE: Check = Check { key: "auto_close", label: "Auto close", default: false };

/// Audio ▸ CPU: Auto-bypass when silent for every effect slot whose own switch is on (`os_engine::engine::auto_bypass`;
/// spec: audio/auto-bypass.md).
pub const SMART_DISABLE: Check = Check { key: "smart_disable", label: "Auto-bypass when silent", default: true };

/// Audio ▸ CPU: stored and shown only. Lemon Studio's ticks follow audio/timing.md whatever it says. Spec: as above.
pub const ALIGN_TICK_LENGTHS: Check = Check { key: "align_tick_lengths", label: "Align tick lengths", default: false };

/// Audio ▸ CPU: the mixer's tracks, and the generator channels with the sampler's voices, render on several cores
/// (`crate::mix_threads`). Spec: audio/multicore.md "Settings".
pub const MULTITHREADED_MIXER: Check = Check { key: "multithreaded_mixer", label: "Multithreaded mixer processing", default: true };
pub const MULTITHREADED_GENERATORS: Check =
    Check { key: "multithreaded_generators", label: "Multithreaded instrument processing", default: true };
/// Audio ▸ CPU: live, the two above take the extra cores only when one thread would not keep up comfortably. Spec:
/// audio/multicore.md "Only when needed".
pub const MULTITHREADED_AUTO: Check = Check { key: "multithreaded_auto", label: "Extra cores only when needed", default: true };

/// Audio ▸ Mixer: stored and shown only. The sampler keeps the one kernel audio/sampler.md gives, whatever is chosen;
/// the live kernels are unknown, and once they are known the choice will pick among them. Its menu groups the
/// choices under grey headings (`headings`). Spec: as above.
pub const RESAMPLING_QUALITY: Combo = Combo {
    key: "resampling_quality",
    label: "Resampling quality",
    choices: &[
        "2-point linear",
        "6-point Hermite",
        "16-point sinc",
        "24-point sinc",
        "32-point sinc",
        "64-point sinc",
        "128-point sinc",
        "256-point sinc",
        "512-point sinc",
    ],
    default: 3,
};

/// General ▸ Hints and warnings: after a save writes a project file over this size (MB = 1024 × 1024 bytes), an
/// information message says so (`crate::messages`); "Don't warn me" never warns. Spec:
/// ui/settings-choices.md "Hints and warnings: Manage messages and Project data size warning (2026-10-16)".
pub const PROJECT_DATA_SIZE_WARNING: Combo = Combo {
    key: "project_data_size_warning",
    label: "Project data size warning",
    choices: &["50MB", "100MB (default)", "250MB", "500MB", "1GB", "1.5GB", "Don't warn me"],
    default: 1,
};

/// Project ▸ Data folder: when Lemon Studio's Save as and New project forms come up (`crate::save_form`): saving a
/// project never saved, FILE ▸ New, the program's start. A program setting, not the project's. Spec:
/// ui/data-folder.md §1, §7.
pub const ASK_DATA_FOLDER: Combo = Combo {
    key: "ask_for_data_folder",
    label: "Ask for data folder",
    choices: &[
        "On startup, when starting a new project and when saving unsaved projects",
        "When starting a new project and when saving unsaved projects",
        "When starting a new project only",
        "When saving unsaved projects only",
        "Don't ask",
    ],
    default: 3,
};

/// Project ▸ Data folder: the samples a project uses are copied into `<data folder>/Samples` (`crate::data_folder`):
/// all of them when it is ticked (after a question), then each sample loaded while it is ticked. A program setting,
/// not the project's. Spec: ui/data-folder.md §4.
pub const COPY_USED_SAMPLES: Check = Check { key: "copy_used_samples", label: "Copy used samples to the project folder", default: false };

/// MIDI ▸ Input: a linked controller does nothing after its knob changed another way until its value reaches or
/// crosses the knob's (`crate::remote`). Spec: ui/remote-control.md §5.
pub const PICKUP: Check = Check { key: "midi_pickup", label: "Soft takeover", default: false };

/// MIDI ▸ Input: the Remote control settings box accepts the first controller it detects and closes (`crate::remote`).
/// Spec: ui/remote-control.md §2.
pub const AUTO_ACCEPT: Check = Check { key: "midi_auto_accept", label: "Auto accept detected controller", default: true };

/// MIDI ▸ Input: controller 64 holds released keys until the pedal comes up; off, it goes to the instrument
/// (`crate::midi_play`). Spec: ui/midi-settings.md §3.
pub const FOOT_PEDAL: Check = Check { key: "midi_foot_pedal", label: "Sustain pedal releases notes", default: true };

/// MIDI ▸ Input: the generator muting keys mute while held (`crate::midi_play`). Spec: ui/midi-settings.md §3.
pub const TOGGLE_ON_RELEASE: Check = Check { key: "midi_toggle_on_release", label: "Toggle on release", default: false };

/// MIDI ▸ Input: what a note's velocity becomes (`crate::midi_play`). Spec: ui/midi-settings.md §3.
pub const VELOCITY_LINK: Combo =
    Combo { key: "midi_velocity_link", label: "Link note on velocity to", choices: &["(none)", "Velocity", "Mod A", "Mod B"], default: 1 };

/// MIDI ▸ Input: what a note's release velocity becomes. Spec: as above.
pub const RELEASE_LINK: Combo = Combo { key: "midi_release_link", label: "Link release velocity to", choices: &["(none)", "Release"], default: 1 };

/// MIDI ▸ Input: Lemon Studio runs no device scripts, so every device is a generic controller. Spec:
/// ui/midi-settings.md §1.
pub const CONTROLLER_TYPE: Combo = Combo { key: "midi_controller_type", label: "Controller type", choices: &["(generic controller)"], default: 0 };

/// MIDI ▸ Output: what master sync sends (`crate::midi_out::SyncKind::of`, by index). Spec: ui/midi-settings.md §7.
pub const SYNC_TYPE: Combo = Combo {
    key: "midi_sync_type",
    label: "Synchronization type",
    choices: &[
        "MIDI clock",
        "MTC 24 fps",
        "MTC 25 fps",
        "MTC 30 fps (drop)",
        "MTC 30 fps (non-drop)",
        "MTC 29.97 fps (drop)",
        "MTC 29.97 fps (non-drop)",
    ],
    default: 0,
};

/// MIDI ▸ Input: the MIDI channels (1..16; 0, "---", none) whose white keys preview rack channels, jump to the song's
/// markers and mute rack channels (`crate::midi_play`). Spec: ui/midi-settings.md §3.
pub const OMNI_CHANNEL: Number = Number { key: "midi_omni_channel", label: "Omni preview MIDI channel", min: 0, max: 16, default: 0 };
pub const MARKER_CHANNEL: Number = Number { key: "midi_marker_channel", label: "Song marker jump MIDI channel", min: 0, max: 16, default: 0 };
pub const MUTING_CHANNEL: Number = Number { key: "midi_muting_channel", label: "Instrument muting MIDI channel", min: 0, max: 16, default: 0 };
/// Its notes launch the arranger's performance zone (spec: ui/performance-mode.md §3).
pub const PERFORMANCE_CHANNEL: Number = Number { key: "midi_performance_channel", label: "Performance mode MIDI channel", min: 0, max: 16, default: 0 };

// Recording (Lemon Studio's own page, spec: ui/settings.md "Recording"): the same settings the Record button's
// right-click menu, the toolbar's option buttons and the mixer's Disk recording menu set (`crate::record::Opt` reads
// them through these), so the page and those menus always agree.

/// Recording ▸ Recording filter: notes played while recording are written into the pattern. Spec: ui/recording.md §2.
pub const REC_NOTES: Check = Check { key: "rec_notes", label: "Notes", default: true };
/// Recording ▸ Recording filter: control moves are written into the pattern as its control points.
pub const REC_AUTOMATION: Check = Check { key: "rec_automation", label: "Automation", default: true };
/// Recording ▸ Recording filter: the armed mixer tracks record (spec: audio/track-io.md §2.5).
pub const REC_AUDIO: Check = Check { key: "rec_audio", label: "Audio (armed mixer tracks)", default: false };
/// Recording ▸ Options: a click or R on the disarmed Record button asks what to record.
pub const REC_ASK: Check = Check { key: "rec_ask", label: "Ask what to record", default: true };
/// Recording ▸ Options: arming while stopped also starts playback.
pub const REC_STARTS_PLAYBACK: Check = Check { key: "rec_starts_play", label: "Recording starts playback", default: false };
/// Recording ▸ Options: Stop disarms Record.
pub const REC_DISARM_ON_STOP: Check = Check { key: "rec_disarm_stop", label: "Disarm on stop", default: false };
/// Recording ▸ Options: the toolbar's Loop record (spec: ui/recording.md §4); off, playback runs on past the end while
/// recording (`record::run_on`, kept up to date by `App::record_tick`).
pub const REC_LOOP: Check = Check { key: "rec_loop", label: "Loop record", default: false };
/// Recording ▸ Options: the toolbar's Blend (spec: ui/recording.md §3): a take keeps the pattern's earlier notes.
pub const REC_BLEND: Check = Check { key: "rec_blend", label: "Blend recorded notes", default: false };
/// Recording ▸ Quantize: recorded note starts and ends snap to the main snap.
pub const REC_QUANTIZE_START: Check = Check { key: "rec_q_start", label: "Note start time", default: true };
pub const REC_QUANTIZE_END: Check = Check { key: "rec_q_end", label: "Note end time", default: true };
/// Recording ▸ Start on input: the toolbar's Start on input and its right-click checks (spec: ui/recording.md §3).
/// Turning it off ends a wait for input (`App::pref_toggle`).
pub const REC_WAIT: Check = Check { key: "rec_wait", label: "Start on input", default: false };
pub const REC_WAIT_FILTER: Check = Check { key: "rec_wait_filter", label: "Respect recording filter", default: true };
pub const REC_WAIT_ARMED: Check = Check { key: "rec_wait_armed", label: "Only when recording", default: false };
/// Recording ▸ Count-in: the toolbar's Count-in and its length (spec: ui/recording.md §3).
pub const REC_COUNT_IN: Check = Check { key: "rec_precount", label: "Count-in", default: false };
pub const COUNT_IN_LENGTH: Combo = Combo { key: "rec_precount_bars", label: "Count-in length", choices: &["1 bar", "2 bars"], default: 0 };
/// Recording ▸ Audio recording: a take recorded into mixer tracks becomes audio clips (`crate::track_rec`; the mixer
/// menu's Disk recording ▸ Auto-create audio clips). Spec: audio/track-io.md §2.5.
pub const AUTO_AUDIO_CLIPS: Check = Check { key: "rec_auto_clips", label: "Auto-create audio clips", default: true };
/// Recording ▸ Audio recording: the answer to the mixer's External input question (`App::ask_external`), which its
/// Remember my choice sets; Ask asks again. Spec: audio/track-io.md §1.4, §2.3.
pub const EXTERNAL_INPUT: Combo = Combo {
    key: "ext_input_answer",
    label: "External input question",
    choices: &["Ask", "Switch to External input only", "Keep the record source"],
    default: 0,
};

/// The declared checks.
pub const CHECKS: &[&Check] = &[
    &RESTORE_AFTER_SOLO,
    &SELECT_FIRST_CHANNEL,
    &AUTO_ZOOM_PIANO,
    &SMALL_SCROLLBARS,
    &AUTO_ZIP_EMPTY,
    &MONO_PAINTING,
    &AUTO_CROSSFADES_NEW,
    &PLAY_TRUNCATED_ON_TRANSPORT,
    &RESET_PLUGINS_ON_TRANSPORT,
    &ALTERNATE_METER_SCALE,
    &AUTOMATION_AT_PLAYBACK_POSITION,
    &AUTO_SELECT_LINKED,
    &HIDE_PLUGIN_TOOLBAR,
    &AUTO_NAME_CHANNELS,
    &AUTO_NAME_EFFECT_SLOTS,
    &DETACH_NEW,
    &DETACHED_WITH_MAIN,
    &TYPING_PRIORITY_OFF,
    &LIMIT_EDITORS,
    &SHOW_SPLASH,
    &PLUGIN_TABS,
    &PLUGIN_TABS_STACKED,
    &SILENT_STARTUP,
    &AUTO_CHECK_UPDATES,
    &AUTO_DOWNLOAD_UPDATES,
    &BOTH_MOUSE_BUTTONS,
    &CLICK_AND_HOLD,
    &UNDO_KNOB_TWEAKS,
    &ALTERNATE_UNDO,
    &UNDONE_RECORDINGS_TO_BIN,
    &UNDO_HISTORY_IN_BROWSER,
    &RECENT_CHANGES_TOP,
    &READ_ROOT_NOTE,
    &READ_TEMPO,
    &DONT_LIMIT_WINDOWS,
    &INIT_CONTROLS,
    &FAST_SAMPLE_PREVIEW,
    &AUTO_KEEP_ON_DISK,
    &THREADED_SAMPLE_LOADING,
    &HIGH_VISIBILITY,
    &FORCE_REFRESHES,
    &THICK_LINES,
    &TRANSPARENT_MENUS,
    &AUTO_CLOSE,
    &SMART_DISABLE,
    &ALIGN_TICK_LENGTHS,
    &MULTITHREADED_MIXER,
    &MULTITHREADED_GENERATORS,
    &MULTITHREADED_AUTO,
    &INVERT_PENCIL_BUTTONS,
    &COPY_USED_SAMPLES,
    &PICKUP,
    &AUTO_ACCEPT,
    &FOOT_PEDAL,
    &TOGGLE_ON_RELEASE,
    &REC_NOTES,
    &REC_AUTOMATION,
    &REC_AUDIO,
    &REC_ASK,
    &REC_STARTS_PLAYBACK,
    &REC_DISARM_ON_STOP,
    &REC_LOOP,
    &REC_BLEND,
    &REC_QUANTIZE_START,
    &REC_QUANTIZE_END,
    &REC_WAIT,
    &REC_WAIT_FILTER,
    &REC_WAIT_ARMED,
    &REC_COUNT_IN,
    &AUTO_AUDIO_CLIPS,
];
/// The declared combos.
pub const COMBOS: &[&Combo] = &[&MAIN_GUI_SCALING, &POPUPS_SCALING, &NOTE_NAMES, &BLACK_NOTES, &AUTOSAVE, &BACKUP_LOCATION, &STARTUP_PROJECT, &COLORFUL_WAVEFORMS, &ANIMATION_REFRESH_RATE, &TRANSPARENT_WINDOWS, &CURSORS_SCALING, &LEGACY_SCALING, &OPTIMIZE_FOR, &RESAMPLING_QUALITY, &PROJECT_DATA_SIZE_WARNING, &TOOLBARS_SCALING, &ASK_DATA_FOLDER, &VELOCITY_LINK, &RELEASE_LINK, &CONTROLLER_TYPE, &SYNC_TYPE, &COUNT_IN_LENGTH, &EXTERNAL_INPUT];
/// The declared value boxes.
pub const NUMBERS: &[&Number] = &[&MAX_UNDO_LEVELS, &BACKUP_MAXIMUM, &BROWSER_PREVIEW_TRACK, &METRONOME_TRACK, &OMNI_CHANNEL, &MARKER_CHANNEL, &MUTING_CHANNEL, &PERFORMANCE_CHANNEL];

/// General ▸ Input: the editors scroll by the vertical wheel's steps times value / 256 (`crate::scroll_speed`);
/// Shift+wheel keeps its own step. Spec: ui/settings-choices.md "Input: Vertical and Horizontal scrolling speed".
pub const VERTICAL_SCROLLING_SPEED: Slider =
    Slider { key: "vertical_scrolling_speed", label: "Vertical scrolling speed", max: 512, default: 256, step: 32 };

/// General ▸ Input: the same for the sideways wheel (a tilt wheel, a touchpad's sideways swipe).
pub const HORIZONTAL_SCROLLING_SPEED: Slider =
    Slider { key: "horizontal_scrolling_speed", label: "Horizontal scrolling speed", max: 512, default: 256, step: 32 };

/// General ▸ Display: how slowly the editors' scrolls ease to where the wheel or a scroll arrow sends them, 0 (they
/// jump) to 512 (`crate::smooth`). Spec: ui/settings-choices.md "Display: Smoothing, Animation refresh rate,
/// Transparent menus and windows (2026-10-16)".
pub const SMOOTHING: Slider = Slider { key: "smoothing", label: "Smoothing", max: 512, default: 128, step: 16 };

/// The declared sliders.
pub const SLIDERS: &[&Slider] = &[&SMOOTHING, &VERTICAL_SCROLLING_SPEED, &HORIZONTAL_SCROLLING_SPEED];

/// Audio ▸ Mixer: the insert track the browser's previews enter the mixer at; 0, shown "---", plays them straight into
/// the master (`crate::preview_track`). The wheel offers the open project's insert tracks (`App::settings_number`), up
/// to 125, the most a project file shows (spec: ui/mixer.md §1). Spec: ui/settings-choices.md "Mixer: Browser preview
/// track".
pub const BROWSER_PREVIEW_TRACK: Number = Number { key: "browser_preview_track", label: "Browser preview track", min: 0, max: 125, default: 0 };

/// Audio ▸ Mixer: the insert track the metronome's clicks (and the count-in's and the Tempo tapper's) enter; 0, shown
/// "---", the master (`crate::metronome::switch`). The wheel offers the open project's insert tracks, as for Browser
/// preview track; one the playing project does not have is the master. Spec: ui/metronome.md §4.
pub const METRONOME_TRACK: Number = Number { key: "metronome_track", label: "Metronome track", min: 0, max: 125, default: 0 };

/// The declared slider its page labels `label`.
pub fn find_slider(label: &str) -> Option<&'static Slider> {
    SLIDERS.iter().copied().find(|s| s.label == label)
}

/// Slider `s`'s value, 0 to its `max`.
pub fn slider(s: &Slider) -> u16 {
    read(|st| slider_in(st, s))
}

/// Sets slider `s` (to its `max` at most) and keeps it.
pub fn set_slider(s: &Slider, v: u16) {
    write(s.key, &v.min(s.max).to_string());
}

/// Value box `n` as its box shows it: its number, or "---" for 0 in a box whose range starts at 0 (none).
pub fn number_text(n: &Number) -> String {
    match number(n) {
        0 if n.min == 0 => "---".into(),
        v => v.to_string(),
    }
}

fn slider_in(st: &Store, s: &Slider) -> u16 {
    st.get(s.key).and_then(|v| v.parse::<u16>().ok()).map_or(s.default, |v| v.min(s.max))
}

/// Tests only: a check that exercises the settings page, on a control that stays greyed for real (General ▸ Advanced;
/// ui/options-coverage.md §4).
#[cfg(any(test, feature = "test-util"))]
pub const TEST_CHECK: Check = Check { key: "test_check", label: "Use the high-performance power plan", default: true };

pub fn checks() -> impl Iterator<Item = &'static Check> {
    #[cfg(any(test, feature = "test-util"))]
    let more: &[&Check] = &[&TEST_CHECK];
    #[cfg(not(any(test, feature = "test-util")))]
    let more: &[&Check] = &[];
    CHECKS.iter().chain(more).copied()
}

/// The declared check its page labels `label`.
pub fn find_check(label: &str) -> Option<&'static Check> {
    checks().find(|c| c.label == label)
}

/// The declared combo its page labels `label`.
pub fn find_combo(label: &str) -> Option<&'static Combo> {
    COMBOS.iter().copied().find(|c| c.label == label)
}

/// The declared value box its page labels `label`.
pub fn find_number(label: &str) -> Option<&'static Number> {
    NUMBERS.iter().copied().find(|n| n.label == label)
}

/// Value box `n`'s value.
pub fn number(n: &Number) -> u16 {
    read(|s| number_in(s, n))
}

/// Sets value box `n` (kept within its range) and keeps it.
pub fn set_number(n: &Number, v: u16) {
    write(n.key, &v.clamp(n.min, n.max).to_string());
}

/// Whether check `c` is on.
pub fn check(c: &Check) -> bool {
    read(|s| check_in(s, c))
}

/// Turns check `c` on or off and keeps it.
pub fn set_check(c: &Check, on: bool) {
    write(c.key, if on { "1" } else { "0" });
}

/// Toggles check `c` and keeps it; returns the new state.
pub fn toggle(c: &Check) -> bool {
    let on = !check(c);
    set_check(c, on);
    on
}

/// The position of combo `c`'s choice in its `choices`.
pub fn index(c: &Combo) -> usize {
    read(|s| index_in(s, c))
}

/// The text of combo `c`'s choice.
pub fn choice(c: &Combo) -> &'static str {
    c.choices[index(c)]
}

/// Chooses entry `i` of combo `c` and keeps it (an index past its choices changes nothing).
pub fn set_choice(c: &Combo, i: usize) {
    if let Some(v) = c.choices.get(i) {
        write(c.key, v);
    }
}

/// A setting kept as free text under `key`, for values found at run time (a template, a file path; `crate::templates`):
/// `None` when it is not set or empty.
pub fn text(key: &str) -> Option<String> {
    read(|s| s.get(key).filter(|v| !v.is_empty()).map(String::from))
}

/// Keeps `value` (one line: line breaks become spaces) as `key`'s free text; "" clears it.
pub fn set_text(key: &str, value: &str) {
    write(key, &value.replace(['\r', '\n'], " "));
}

pub fn check_in(s: &Store, c: &Check) -> bool {
    match s.get(c.key) {
        Some("1") => true,
        Some("0") => false,
        _ => c.default,
    }
}

fn number_in(s: &Store, n: &Number) -> u16 {
    s.get(n.key).and_then(|v| v.parse().ok()).filter(|v| (n.min..=n.max).contains(v)).unwrap_or(n.default)
}

pub fn index_in(s: &Store, c: &Combo) -> usize {
    s.get(c.key).and_then(|v| c.choices.iter().position(|ch| *ch == v)).unwrap_or(c.default)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Values it cannot read are the defaults; the last line of a key counts (the file's own rules:
    /// `settings_file`).
    #[test]
    fn values_and_defaults() {
        let s = Store::parse("theme=dark
junk
main_gui_scaling=150%
");
        assert_eq!(index_in(&s, &MAIN_GUI_SCALING), 3);
        assert!(check_in(&s, &TEST_CHECK), "missing: the default");
        assert!(!check_in(&Store::parse("test_check=0"), &TEST_CHECK));
        let s = Store::parse("test_check=yes
main_gui_scaling=90%
main_gui_scaling = 200% 
");
        assert!(check_in(&s, &TEST_CHECK));
        assert_eq!(index_in(&s, &MAIN_GUI_SCALING), 5, "the last line counts");
        assert_eq!(index_in(&Store::parse("main_gui_scaling=90%"), &MAIN_GUI_SCALING), 0);
    }

    #[test]
    fn reading_and_changing_settings() {
        assert_eq!((check(&TEST_CHECK), choice(&MAIN_GUI_SCALING)), (true, "System"));
        assert!(!toggle(&TEST_CHECK) && !check(&TEST_CHECK));
        set_check(&TEST_CHECK, true);
        assert!(check(&TEST_CHECK));
        set_choice(&MAIN_GUI_SCALING, 2);
        assert_eq!((index(&MAIN_GUI_SCALING), choice(&MAIN_GUI_SCALING)), (2, "125%"));
        set_choice(&MAIN_GUI_SCALING, 99);
        assert_eq!(choice(&MAIN_GUI_SCALING), "125%");
        assert_eq!(find_combo("Main GUI scaling"), Some(&MAIN_GUI_SCALING));
        assert_eq!(find_check("Use the high-performance power plan"), Some(&TEST_CHECK));
        assert_eq!((find_check("Main GUI scaling"), find_combo("Language"), find_check("")), (None, None, None));
    }

    /// A slider is stored as a number: unreadable is the default, past the range its end. A value box whose range
    /// starts at 0 shows 0 as "---".
    #[test]
    fn slider_values_and_the_preview_track() {
        let s = Store::parse("vertical_scrolling_speed=96\nhorizontal_scrolling_speed=fast\n");
        assert_eq!((slider_in(&s, &VERTICAL_SCROLLING_SPEED), slider_in(&s, &HORIZONTAL_SCROLLING_SPEED)), (96, 256));
        assert_eq!(slider_in(&Store::parse("vertical_scrolling_speed=9000"), &VERTICAL_SCROLLING_SPEED), 512);
        assert_eq!(slider(&VERTICAL_SCROLLING_SPEED), 256, "unset: the middle");
        set_slider(&VERTICAL_SCROLLING_SPEED, 600);
        assert_eq!(slider(&VERTICAL_SCROLLING_SPEED), 512);
        set_slider(&VERTICAL_SCROLLING_SPEED, 0);
        assert_eq!(slider(&VERTICAL_SCROLLING_SPEED), 0);
        set_slider(&VERTICAL_SCROLLING_SPEED, 256);
        assert_eq!(find_slider("Vertical scrolling speed"), Some(&VERTICAL_SCROLLING_SPEED));
        assert_eq!((find_slider(""), find_slider("Smoothing")), (None, Some(&SMOOTHING)));
        assert_eq!((number(&BROWSER_PREVIEW_TRACK), number_text(&BROWSER_PREVIEW_TRACK).as_str()), (0, "---"));
        set_number(&BROWSER_PREVIEW_TRACK, 5);
        assert_eq!((number(&BROWSER_PREVIEW_TRACK), number_text(&BROWSER_PREVIEW_TRACK).as_str()), (5, "5"));
        set_number(&BROWSER_PREVIEW_TRACK, 0);
        assert_eq!(number_text(&MAX_UNDO_LEVELS), "100", "a box from 1 shows its number");
        assert_eq!(find_number("Browser preview track"), Some(&BROWSER_PREVIEW_TRACK));
    }
}
