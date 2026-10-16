// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Program settings of the settings window, declared once below: checks and combos kept together in the user folder
//! (`settings.txt`, `key=value` lines; lines it does not know are kept, a missing or unreadable value is the default),
//! never in the project. The settings page draws a declared control live: a check toggles on a click; a combo shows its
//! choice and opens the list of its choices, the current one radio-marked. A change is kept at once. Undeclared
//! controls (without a module of their own) stay greyed out.
//!
//! Making a greyed-out check or combo of the settings window work:
//! 1. Declare it: a `pub const` [`Check`] or [`Combo`] whose `label` is the control's label in
//!    `views::settings::page` (the page finds it by it) and whose `key` names it in the file (stable: a later change of
//!    label keeps what users chose); then list it in [`CHECKS`] or [`COMBOS`].
//!    ```ignore
//!    /// General ▸ Miscellaneous (spec: ui/...): what it does.
//!    pub const AUTO_NAME_CHANNELS: Check = Check { key: "auto_name_channels", label: "Auto name channels", default: true };
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

use std::sync::{Mutex, OnceLock}; // This code is a part of L e m o n S t u d i o. Please don't upload it to Art Craft!

use crate::app::App;

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
/// Read by `crate::gui_scale` at every event and paint, so it applies at once (FL needs a restart). Spec:
/// ui/ui-scaling.md §1.
pub const MAIN_GUI_SCALING: Combo = Combo {
    key: "main_gui_scaling",
    label: "Main GUI scaling",
    choices: &["System", "100%", "125%", "150%", "175%", "200%", "250%", "300%", "350%", "400%"],
    default: 0,
};

/// General ▸ Display: the scale of popup menus and combo lists. Main = the main GUI's; a percentage replaces it. Read
/// by `views::popup_scale` at every paint, so it applies at once (FL needs a restart). Spec: ui/ui-scaling.md §1–2.
pub const POPUPS_SCALING: Combo = Combo {
    key: "popups_scaling",
    label: "Pop-ups scaling",
    choices: &["Main", "100%", "125%", "150%", "175%", "200%", "250%", "300%", "350%", "400%"],
    default: 0,
};

/// General ▸ Display: sample waveforms coloured by what they are made of: Off, Color map (the Waves gradient by
/// spectral centroid) or Multiband (three colours by band power); `crate::waves`. It takes effect at once. Spec:
/// ui/settings-choices.md "Display: Colorful waveforms (2026-10-16)".
pub const COLORFUL_WAVEFORMS: Combo =
    Combo { key: "colorful_waveforms", label: "Colorful waveforms", choices: &["Off", "Color map", "Multiband"], default: 0 };

/// General ▸ Language: how note names read (`crate::note_names`; FL asks for a restart, Lemon Studio renames at once).
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

/// File ▸ Backup: where backups go. Both write to the global folder until Lemon Studio has project data folders
/// (`crate::backup`). Spec: ui/settings-choices.md "Backup files".
pub const BACKUP_LOCATION: Combo = Combo {
    key: "backup_location",
    label: "Location",
    choices: &["Always save to the global folder", "Save to the project data folder if there is one"],
    default: 1,
};

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
pub const AUTO_ZIP_EMPTY: Check = Check { key: "auto_zip_empty_channels", label: "Auto zip empty channels", default: false };

/// General ▸ Miscellaneous: a painted note deletes the notes of other keys under it (`crate::piano::paint_mono`).
/// Spec: ui/settings-choices.md "Miscellaneous: seven more checks, Default template and Startup project".
pub const MONO_PAINTING: Check = Check { key: "monophonic_painting", label: "Monophonic painting in piano roll", default: false };

/// Project ▸ Playlist: a new project's "Automatic crossfades" (event 44) takes this value, whatever FL's template file
/// holds (`app::default_project`); opening a file keeps the file's. A program setting, on by default. Spec:
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
pub const AUTO_SELECT_LINKED: Check = Check { key: "auto_select_linked_modules", label: "Auto select linked modules", default: true };

/// General ▸ Miscellaneous: stored and shown only. FL showed no change on native plugin windows; it has no effect until
/// hosted plugin windows get a toolbar (spec: ui/settings-choices.md, as above).
pub const HIDE_PLUGIN_TOOLBAR: Check =
    Check { key: "hide_plugin_window_toolbar", label: "Hide plugin window toolbar by default", default: true };

/// General ▸ Miscellaneous: a sample or channel preset loaded into an existing channel renames the channel after the
/// file (`rack::auto_name`); off, the channel keeps its name. Spec: ui/settings-choices.md "Miscellaneous: naming,
/// detached plugins and startup".
pub const AUTO_NAME_CHANNELS: Check = Check { key: "auto_name_channels", label: "Auto name channels", default: true };

/// General ▸ Miscellaneous: a preset loaded into an effect renames its mixer slot after the preset
/// (`presets::auto_name`); off, the slot keeps its name. Spec: ui/settings-choices.md, as above.
pub const AUTO_NAME_EFFECT_SLOTS: Check =
    Check { key: "auto_name_effect_slots", label: "Auto name effect slots", default: false };

/// General ▸ Miscellaneous: a plugin added from now on (a new channel's, a new effect in a slot) opens its window
/// detached (`Plugins::added`); plugins that exist keep their own mode. Spec: ui/settings-choices.md "Miscellaneous:
/// naming, detached plugins and startup".
pub const DETACH_NEW: Check = Check { key: "detach_new_plugins", label: "Detach all new plugins", default: false };

/// General ▸ Miscellaneous: a detached window opening for the first time goes on the main window's monitor; off, on
/// the primary monitor (`plugin_window::first_position`, Windows only). A reopened one goes where it was either way.
/// Spec: as above.
pub const DETACHED_WITH_MAIN: Check =
    Check { key: "detached_with_main_window", label: "Open detached plugins with the main window", default: false };

/// General ▸ Miscellaneous: stored and shown only (native plugins have no typing keyboard option). Spec: as above.
pub const TYPING_PRIORITY_OFF: Check =
    Check { key: "disable_typing_priority", label: "Disable typing keyboard priority by default", default: false };

/// General ▸ Miscellaneous: at most `plugins::MAX_EDITORS` plugin editor windows open; opening another first closes the
/// one opened earliest (`Plugins::open_window`). Spec: as above.
pub const LIMIT_EDITORS: Check = Check { key: "limit_plugin_editors", label: "Limit the number of open plugin editors", default: true };

/// General ▸ Miscellaneous: at start, a borderless window centred on the primary monitor shows the logo mark until the
/// main window shows (`crate::splash`). A program setting, read at start. Spec: ui/settings-choices.md "Miscellaneous:
/// naming, detached plugins and startup".
pub const SHOW_SPLASH: Check = Check { key: "show_startup_splash", label: "Show startup splash screen", default: true };

/// General ▸ Miscellaneous: stored and shown only. Lemon Studio has no startup sound (and does not use FL's); if it ever
/// gets one, this check silences it. Spec: ui/settings-choices.md "Miscellaneous: naming, detached plugins and startup".
pub const SILENT_STARTUP: Check = Check { key: "silent_startup", label: "Silent startup", default: false };

/// General ▸ Input: off, a left or a right click on a step of the step sequencer toggles it; on, the left button only
/// sets steps and the right one only clears them. A drag sets (left) or clears (right) the steps it crosses either way
/// (`App::rack_step`). Spec: ui/settings-choices.md "Input: Click-and-hold and both mouse buttons (2026-10-16)".
pub const BOTH_MOUSE_BUTTONS: Check =
    Check { key: "both_mouse_buttons_step_sequencer", label: "Use both mouse buttons in step sequencer", default: false };

/// General ▸ Miscellaneous: the project Lemon Studio starts with when no project file is passed to it
/// (`App::startup_project`). Spec: ui/settings-choices.md, as above.
pub const STARTUP_PROJECT: Combo =
    Combo { key: "startup_project", label: "Startup project", choices: &["Empty project", "Default template", "Last used project"], default: 1 };

/// General ▸ Input: a held click that doesn't move does more: in the piano roll the Draw tool's new note cycles plain,
/// slide, porta, and a note held on is cloned; in the playlist a clip held on is cloned (`crate::hold`). Spec:
/// ui/settings-choices.md "Input: Click-and-hold and both mouse buttons (2026-10-16)".
pub const CLICK_AND_HOLD: Check = Check { key: "click_and_hold_gestures", label: "Click-and-hold & special gesture functions", default: false };

/// General ▸ Undo history: the most steps the project's undo history keeps; past it the oldest step goes
/// (`crate::history`). FL's range was not measured: Lemon Studio's is 1 to 999. Spec: ui/settings-choices.md "Undo
/// history: the four checks (2026-10-16)".
pub const MAX_UNDO_LEVELS: Number = Number { key: "max_undo_levels", label: "Maximum undo levels", min: 1, max: 999, default: 100 };

/// General ▸ Undo history: knob turns are undo steps; off, they make none (`history::knob`). Spec: as above.
pub const UNDO_KNOB_TWEAKS: Check = Check { key: "undo_knob_tweaks", label: "Undo knob tweaks", default: true };

/// General ▸ Undo history: on (classic), Ctrl+Z steps back once more per press and Ctrl+Alt+Z redoes; off, Ctrl+Z
/// toggles the last move and Ctrl+Alt+Z steps back further (`history::ctrl_z`). EDIT follows. Spec: as above.
pub const ALTERNATE_UNDO: Check = Check { key: "alternate_undo_mode", label: "Alternate undo mode", default: true };

/// General ▸ Undo history: stored and shown only. Lemon Studio does not record audio yet, so no undone recording can
/// go to the recycle bin; recording will read it. Spec: as above (not measured: it needs recording).
pub const UNDONE_RECORDINGS_TO_BIN: Check =
    Check { key: "undone_recordings_to_recycle_bin", label: "Put undone recordings in the recycle bin", default: true };

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
    &SILENT_STARTUP,
    &BOTH_MOUSE_BUTTONS,
    &CLICK_AND_HOLD,
    &UNDO_KNOB_TWEAKS,
    &ALTERNATE_UNDO,
    &UNDONE_RECORDINGS_TO_BIN,
    &UNDO_HISTORY_IN_BROWSER,
    &RECENT_CHANGES_TOP,
    &READ_ROOT_NOTE,
    &READ_TEMPO,
];
/// The declared combos.
pub const COMBOS: &[&Combo] = &[&MAIN_GUI_SCALING, &POPUPS_SCALING, &NOTE_NAMES, &BLACK_NOTES, &AUTOSAVE, &BACKUP_LOCATION, &STARTUP_PROJECT, &COLORFUL_WAVEFORMS];

/// The declared value boxes.
pub const NUMBERS: &[&Number] = &[&MAX_UNDO_LEVELS, &BROWSER_PREVIEW_TRACK];

/// General ▸ Input: the editors scroll by the vertical wheel's steps times value / 256 (`crate::scroll_speed`);
/// Shift+wheel keeps its own step. Spec: ui/settings-choices.md "Input: Vertical and Horizontal scrolling speed".
pub const VERTICAL_SCROLLING_SPEED: Slider =
    Slider { key: "vertical_scrolling_speed", label: "Vertical scrolling speed", max: 512, default: 256, step: 32 };

/// General ▸ Input: the same for the sideways wheel (a tilt wheel, a touchpad's sideways swipe).
pub const HORIZONTAL_SCROLLING_SPEED: Slider =
    Slider { key: "horizontal_scrolling_speed", label: "Horizontal scrolling speed", max: 512, default: 256, step: 32 };

/// The declared sliders.
pub const SLIDERS: &[&Slider] = &[&VERTICAL_SCROLLING_SPEED, &HORIZONTAL_SCROLLING_SPEED];

/// Audio ▸ Mixer: the insert track the browser's previews enter the mixer at; 0, shown "---", plays them straight into
/// the master (`crate::preview_track`). The wheel offers the open project's insert tracks (`App::settings_number`), up
/// to 125, the most a project file shows (spec: ui/mixer.md §1). Spec: ui/settings-choices.md "Mixer: Browser preview
/// track".
pub const BROWSER_PREVIEW_TRACK: Number = Number { key: "browser_preview_track", label: "Browser preview track", min: 0, max: 125, default: 0 };

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

/// Tests only: a check that exercises the settings page. An FL service Lemon Studio leaves out (ui/options-coverage.md
/// §4), so it is never declared for real.
#[cfg(test)]
pub const TEST_CHECK: Check = Check { key: "test_check", label: "Automatically check for updates", default: true };

fn checks() -> impl Iterator<Item = &'static Check> {
    #[cfg(test)]
    let more: &[&Check] = &[&TEST_CHECK];
    #[cfg(not(test))]
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

fn check_in(s: &Store, c: &Check) -> bool {
    match s.get(c.key) {
        Some("1") => true,
        Some("0") => false,
        _ => c.default,
    }
}

fn number_in(s: &Store, n: &Number) -> u16 {
    s.get(n.key).and_then(|v| v.parse().ok()).filter(|v| (n.min..=n.max).contains(v)).unwrap_or(n.default)
}

fn index_in(s: &Store, c: &Combo) -> usize {
    s.get(c.key).and_then(|v| c.choices.iter().position(|ch| *ch == v)).unwrap_or(c.default)
}

/// The file's lines as read: lines of keys this version does not know are written back as they were.
#[derive(Debug, Clone, Default, PartialEq)]
struct Store {
    lines: Vec<String>,
}

impl Store {
    fn parse(text: &str) -> Store {
        Store { lines: text.lines().map(String::from).collect() }
    }

    fn text(&self) -> String {
        self.lines.iter().map(|l| format!("{l}\n")).collect()
    }

    /// The value of `key`: its last line's.
    fn get(&self, key: &str) -> Option<&str> {
        self.lines.iter().rev().filter_map(|l| l.split_once('=')).find(|(k, _)| k.trim() == key).map(|(_, v)| v.trim())
    }

    /// Sets `key` to `value`: its first line is replaced (and any later one dropped), or a line is added at the end.
    fn set(&mut self, key: &str, value: &str) {
        let line = format!("{key}={value}");
        let mut found = false;
        self.lines.retain_mut(|l| {
            if !l.split_once('=').is_some_and(|(k, _)| k.trim() == key) {
                return true;
            }
            if found {
                return false;
            }
            found = true;
            l.clone_from(&line);
            true
        });
        if !found {
            self.lines.push(line);
        }
    }
}

#[cfg_attr(test, allow(dead_code))]
static STORE: OnceLock<Mutex<Store>> = OnceLock::new();

#[cfg(test)]
thread_local! {
    /// Tests run in parallel threads: each has its own settings, and none reads or writes the user folder.
    static TEST: std::cell::RefCell<Store> = std::cell::RefCell::new(Store::default());
}

/// Runs `f` on the settings: read from `settings.txt` the first time.
fn read<R>(f: impl FnOnce(&Store) -> R) -> R {
    #[cfg(test)]
    return TEST.with(|s| f(&s.borrow()));
    #[cfg(not(test))]
    f(&STORE.get_or_init(|| Mutex::new(load())).lock().unwrap_or_else(|e| e.into_inner()))
}

/// Sets `key` to `value` and keeps the file when that changed it.
fn write(key: &str, value: &str) {
    #[cfg(test)]
    TEST.with(|s| s.borrow_mut().set(key, value));
    #[cfg(not(test))]
    {
        let mut s = STORE.get_or_init(|| Mutex::new(load())).lock().unwrap_or_else(|e| e.into_inner());
        if s.get(key) != Some(value) {
            s.set(key, value);
            save(&s);
        }
    }
}

#[cfg_attr(test, allow(dead_code))]
fn load() -> Store {
    os_paths::user_file("settings.txt").and_then(|f| std::fs::read_to_string(f).ok()).map(|t| Store::parse(&t)).unwrap_or_default()
}

#[cfg_attr(test, allow(dead_code))]
fn save(s: &Store) {
    if let Some(dir) = os_paths::user_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("settings.txt"), s.text());
    }
}

/// The settings window's side: what a declared control does when clicked (`views::settings` draws them).
impl App {
    /// A declared check was clicked (`SAct::Check`): toggles and keeps it.
    pub(crate) fn pref_toggle(&mut self, label: &str) {
        let Some(c) = find_check(label) else { return };
        let on = toggle(c);
        self.state.message = Some(format!("{}: {}", c.label, if on { "on" } else { "off" }));
        self.invalidate();
    }

    /// A declared combo was clicked (`SAct::Combo`): the menu of its choices under its box (`x`, `y`), the current one
    /// marked.
    pub(crate) fn pref_menu(&mut self, label: &str, x: f32, y: f32) {
        use crate::playlist::{Cmd, Mark, PItem, Popup};
        let Some(c) = find_combo(label) else { return };
        let now = index(c);
        let items = c.choices.iter().enumerate().map(|(i, &ch)| PItem::new(ch, Cmd::Pref(c, i)).mark(Mark::Radio(i == now))).collect();
        self.state.playlist.popup = Some(Popup { x, y: y + 1.0, items, sub: None });
        self.invalidate();
    }

    /// Entry `i` of a declared combo's menu was chosen (`Cmd::Pref`): kept; returns the message.
    pub(crate) fn pref_choose(&mut self, c: &'static Combo, i: usize) -> Option<String> {
        set_choice(c, i);
        self.invalidate();
        Some(format!("{}: {}", c.label, choice(c)))
    }
}

/// The settings window's side of the declared sliders.
impl App {
    /// A declared slider was pressed (`SAct::Slider`, its track `w` wide from `x`): set from the mouse, then dragged
    /// (`Drag::PrefSlider`).
    pub(crate) fn pref_slider_press(&mut self, label: &'static str, x: f32, w: f32) {
        self.pref_slider_drag(label, x, w);
        self.drag = Some(crate::app::Drag::PrefSlider { label, x, w });
    }

    /// Sets the declared slider labelled `label` from the mouse along its track (`w` wide from `x`).
    pub(crate) fn pref_slider_drag(&mut self, label: &str, x: f32, w: f32) {
        let Some(s) = find_slider(label) else { return };
        let at = ((self.mouse.0 - x) / w.max(1.0)).clamp(0.0, 1.0);
        set_slider(s, (at * f32::from(s.max)).round() as u16);
        self.invalidate();
    }

    /// Wheel notches over the declared slider labelled `label`: up (`notches` > 0) moves it by its step toward its
    /// `max`.
    pub(crate) fn pref_slider_wheel(&mut self, label: &str, notches: i32) {
        let Some(s) = find_slider(label) else { return };
        let v = i32::from(slider(s)) + notches * i32::from(s.step);
        set_slider(s, v.clamp(0, i32::from(s.max)) as u16);
        self.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Action;
    use crate::views::settings::{self, Row, SAct};
    use os_ui::{Canvas, Rect};

    #[test]
    fn file_keeps_unknown_lines_and_defaults_what_is_missing() {
        let mut s = Store::parse("theme=dark\njunk\nmain_gui_scaling=150%\n");
        assert_eq!(index_in(&s, &MAIN_GUI_SCALING), 3);
        assert!(check_in(&s, &TEST_CHECK), "missing: the default");
        s.set("test_check", "0");
        s.set("main_gui_scaling", "System");
        assert_eq!(s.text(), "theme=dark\njunk\nmain_gui_scaling=System\ntest_check=0\n");
        assert!(!check_in(&s, &TEST_CHECK));
        // Values it cannot read are the defaults; a key set twice keeps one line.
        let mut s = Store::parse("test_check=yes\nmain_gui_scaling=90%\nmain_gui_scaling = 200% \n");
        assert!(check_in(&s, &TEST_CHECK));
        assert_eq!(index_in(&s, &MAIN_GUI_SCALING), 5, "the last line counts");
        s.set("main_gui_scaling", "125%");
        assert_eq!(s.text(), "test_check=yes\nmain_gui_scaling=125%\n");
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
        assert_eq!(find_check("Automatically check for updates"), Some(&TEST_CHECK));
        assert_eq!((find_check("Main GUI scaling"), find_combo("Toolbars scaling"), find_check("")), (None, None, None));
    }

    /// Every declaration has its own key and label, a default among its choices, and a control of its kind with that
    /// label on a settings page (checks: an enabled check; combos: a combo; value boxes: a value box).
    #[test]
    fn declarations_match_the_settings_pages() {
        let app = App::new(None);
        let mut checks_shown = Vec::new();
        let mut combos_shown = Vec::new();
        let mut numbers_shown = Vec::new();
        fn walk(r: &Row, checks: &mut Vec<&'static str>, combos: &mut Vec<&'static str>, numbers: &mut Vec<&'static str>) {
            match r {
                Row::Check(k, _) => checks.push(*k),
                Row::Check2(a, _, b, _) => checks.extend([*a, *b]),
                Row::Combo(_, _, l) => combos.push(*l),
                Row::Value(_, l) => numbers.push(*l),
                Row::Pair(a, b) => {
                    walk(a, checks, combos, numbers);
                    walk(b, checks, combos, numbers);
                }
                _ => {}
            }
        }
        for tab in 0..settings::TABS.len() {
            for g in settings::page(tab, &app.state) {
                for r in &g.rows {
                    walk(r, &mut checks_shown, &mut combos_shown, &mut numbers_shown);
                }
            }
        }
        let mut keys = Vec::new();
        let mut labels = Vec::new();
        for c in checks() {
            assert!(checks_shown.contains(&c.label), "{} is a check on a settings page", c.label);
            keys.push(c.key);
            labels.push(c.label);
        }
        for c in COMBOS {
            assert!(combos_shown.contains(&c.label), "{} is a combo on a settings page", c.label);
            assert!(c.default < c.choices.len(), "{}: default among its choices", c.label);
            keys.push(c.key);
            labels.push(c.label);
        }
        for n in NUMBERS {
            assert!(numbers_shown.contains(&n.label), "{} is a value box on a settings page", n.label);
            assert!((n.min..=n.max).contains(&n.default), "{}: default in its range", n.label);
            keys.push(n.key);
            labels.push(n.label);
        }
        for (i, k) in keys.iter().enumerate() {
            assert!(!k.is_empty() && !k.contains('=') && k.trim() == *k, "key {k:?}");
            assert!(!keys[..i].contains(k), "key {k} declared twice");
            assert!(!labels[..i].contains(&labels[i]), "label {} declared twice (TEST_CHECK may need another label)", labels[i]);
        }
    }

    fn hit_rect(app: &App, a: &Action) -> Rect {
        app.hits.iter().rev().find(|h| &h.action == a).map(|h| h.rect).unwrap_or_else(|| panic!("no hit for {a:?}"))
    }

    fn click(app: &mut App, c: &mut Canvas, r: Rect) {
        app.mouse_move(r.cx(), r.cy());
        app.mouse_down();
        app.mouse_up();
        app.paint(c, None);
    }

    /// The page draws declared controls live: a check toggles and is kept, a combo lists its choices and shows the
    /// chosen one; undeclared controls stay greyed out.
    #[test]
    fn declared_controls_work_on_the_settings_page() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.open_settings(settings::GENERAL);
        app.paint(&mut c, None);
        // Main GUI scaling opens its list under its box, System marked; Toolbars scaling is not available yet.
        let combo = |app: &App| {
            app.hits.iter().rev().find(|h| matches!(&h.action, Action::Settings(SAct::Combo(l, ..)) if *l == MAIN_GUI_SCALING.label)).map(|h| h.rect).expect("the combo works")
        };
        assert!(app.hits.iter().any(|h| h.action == Action::Placeholder("Toolbars scaling")));
        let r = combo(&app);
        click(&mut app, &mut c, r);
        let p = app.state.playlist.popup.as_ref().expect("its menu");
        let labels: Vec<&str> = p.items.iter().map(|it| it.label.as_ref()).collect();
        assert_eq!(labels, MAIN_GUI_SCALING.choices);
        assert_eq!(p.items[0].mark, crate::playlist::Mark::Radio(true));
        assert!(p.items[1..].iter().all(|it| it.mark == crate::playlist::Mark::Radio(false)));
        assert!((p.y - r.bottom()).abs() < 2.0, "under its box");
        let item = hit_rect(&app, &Action::PlPopup(2));
        click(&mut app, &mut c, item);
        assert!(app.state.playlist.popup.is_none());
        assert_eq!(choice(&MAIN_GUI_SCALING), "125%");
        assert_eq!(app.state.message.as_deref(), Some("Main GUI scaling: 125%"));
        // The menu marks the new choice.
        let r = combo(&app);
        click(&mut app, &mut c, r);
        assert_eq!(app.state.playlist.popup.as_ref().unwrap().items[2].mark, crate::playlist::Mark::Radio(true));
        app.state.playlist.popup = None;
        // The test check (Updates and privacy, further down the page: the groups above it folded) toggles.
        app.state.settings.folded.extend(["Display", "Language", "Input", "Undo history"]);
        app.paint(&mut c, None);
        let key = Action::Settings(SAct::Check(TEST_CHECK.label));
        let r = hit_rect(&app, &key);
        click(&mut app, &mut c, r);
        assert!(!check(&TEST_CHECK));
        assert_eq!(app.state.message.as_deref(), Some("Automatically check for updates: off"));
        let r = hit_rect(&app, &key);
        click(&mut app, &mut c, r);
        assert!(check(&TEST_CHECK));
        // Its neighbours are not declared: greyed out.
        assert!(app.hits.iter().any(|h| h.action == Action::Placeholder("Send usage data")));
        assert!(!app.hits.iter().any(|h| h.action == Action::Settings(SAct::Check("Send usage data"))));
    }

    /// Pop-ups scaling: menus and combo lists are painted at its scale, rows as ui-scaling.md §2 has them, from where they
    /// were opened; clicks find their entries. Main follows the window.
    #[test]
    fn pop_ups_scaling_scales_menus() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.open_settings(settings::GENERAL);
        app.paint(&mut c, None);
        let combo = |app: &App| {
            app.hits.iter().rev().find(|h| matches!(&h.action, Action::Settings(SAct::Combo(l, ..)) if *l == POPUPS_SCALING.label)).map(|h| h.rect).expect("the combo works")
        };
        let r = combo(&app);
        click(&mut app, &mut c, r);
        let main = hit_rect(&app, &Action::PlPopup(1));
        assert_eq!(main.h, 19.0, "Main at 100 %");
        let item = hit_rect(&app, &Action::PlPopup(5));
        click(&mut app, &mut c, item);
        assert_eq!((choice(&POPUPS_SCALING), app.state.message.as_deref()), ("200%", Some("Pop-ups scaling: 200%")));
        // At 200 % on a 100 % window: item rows 12k + 7 = 31 px, the panel's border 2 px, text twice as wide.
        let r = combo(&app);
        click(&mut app, &mut c, r);
        let p = app.state.playlist.popup.as_ref().expect("its menu");
        let (px, py) = (p.x, p.y);
        let (first, big) = (hit_rect(&app, &Action::PlPopup(0)), hit_rect(&app, &Action::PlPopup(1)));
        assert_eq!((big.h, first.y - py, first.x - px), (31.0, 2.0, 2.0));
        assert!((1.8..2.2).contains(&(big.w / main.w)), "{} -> {}", main.w, big.w);
        click(&mut app, &mut c, first);
        assert_eq!(choice(&POPUPS_SCALING), "Main", "the click finds the scaled entry");
        // Menu bar dropdowns too.
        set_choice(&POPUPS_SCALING, 3);
        app.state.settings.open = false;
        app.state.menu_open = Some(0);
        app.paint(&mut c, None);
        let save = app.hits.iter().rev().find(|h| h.action == crate::views::menus::pick(0, crate::views::menus::FILE_SAVE)).map(|h| h.rect).expect("FILE > Save");
        assert_eq!(save.h, 25.0, "150 %: 12k + 7");
    }

    /// "Small scrollbars in editors" (spec: ui/settings-choices.md "Miscellaneous"): the piano roll's and the
    /// playlist's scroll bars get about half as thick (the playlist's as FL measured: 9 px across, 10 px down) and
    /// lose their arrow buttons; the time rulers grow by the room freed above them, the grids by the room at the side.
    #[test]
    fn small_scrollbars_in_editors() {
        use crate::app::{Scroll, Win};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut seen = Vec::new();
        for on in [false, true] {
            set_check(&SMALL_SCROLLBARS, on);
            let mut app = App::new(None);
            let mut measure = |app: &mut App, win: Win| {
                app.state.z = vec![win];
                app.paint(&mut c, None);
                let bar = |t: Scroll| app.bars.iter().find(|b| b.target == t).map(|b| b.track).unwrap_or_else(|| panic!("{t:?} bar"));
                let arrows = app.hits.iter().filter(|h| matches!(h.action, Action::ScrollStep(..) | Action::PitchStep(_))).count();
                let ruler = |a: Action| hit_rect(app, &a);
                match win {
                    Win::PianoRoll => (bar(Scroll::PianoX).h, bar(Scroll::PianoY).w, arrows, ruler(Action::PianoRuler), app.piano_grid.expect("grid")),
                    _ => (bar(Scroll::PlaylistX).h, bar(Scroll::PlaylistY).w, arrows, ruler(Action::PlRuler), app.pl_grid().expect("grid")),
                }
            };
            seen.push((measure(&mut app, Win::PianoRoll), measure(&mut app, Win::Playlist)));
        }
        let ((piano, playlist), (piano_small, playlist_small)) = (seen[0], seen[1]);
        assert_eq!((piano.0, piano.1, playlist.0, playlist.1), (22.0, 20.0, 18.0, 18.0), "off: as before");
        assert_eq!((piano_small.0, piano_small.1, playlist_small.0, playlist_small.1), (11.0, 10.0, 9.0, 10.0), "on: thin");
        assert_eq!((piano.2 > 0, playlist.2), (true, 0), "arrow buttons: the piano roll's");
        assert_eq!((piano_small.2, playlist_small.2), (0, 0), "on: no arrow buttons");
        // The rulers end where they did and start higher; the grids keep their top and widen.
        for ((r0, g0), (r1, g1), freed_h, freed_w) in [((piano.3, piano.4), (piano_small.3, piano_small.4), 11.0, 10.0), ((playlist.3, playlist.4), (playlist_small.3, playlist_small.4), 9.0, 8.0)] {
            assert_eq!((r1.bottom(), r1.h), (r0.bottom(), r0.h + freed_h), "ruler");
            assert_eq!((g1.x, g1.y, g1.h, g1.w), (g0.x, g0.y, g0.h, g0.w + freed_w), "grid");
        }
        set_check(&SMALL_SCROLLBARS, false);
    }

    // spec: ui/settings-choices.md "Display: Colorful waveforms (2026-10-16)"
    /// "Colorful waveforms" is Off by default and found by its label; the General page's combo (Display) lists Off,
    /// Color map and Multiband and switches it.
    #[test]
    fn colorful_waveforms_setting() {
        assert_eq!((index(&COLORFUL_WAVEFORMS), choice(&COLORFUL_WAVEFORMS)), (0, "Off"));
        assert_eq!(find_combo("Colorful waveforms"), Some(&COLORFUL_WAVEFORMS));
        assert_eq!(COLORFUL_WAVEFORMS.choices, ["Off", "Color map", "Multiband"]);
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.open_settings(settings::GENERAL);
        app.paint(&mut c, None);
        let combo = |app: &App| {
            app.hits.iter().rev().find(|h| matches!(&h.action, Action::Settings(SAct::Combo(l, ..)) if *l == COLORFUL_WAVEFORMS.label)).map(|h| h.rect).expect("the combo works")
        };
        let r = combo(&app);
        click(&mut app, &mut c, r);
        let p = app.state.playlist.popup.as_ref().expect("its menu");
        let labels: Vec<&str> = p.items.iter().map(|it| it.label.as_ref()).collect();
        assert_eq!(labels, COLORFUL_WAVEFORMS.choices);
        assert_eq!(p.items[0].mark, crate::playlist::Mark::Radio(true), "Off marked");
        let item = hit_rect(&app, &Action::PlPopup(2));
        click(&mut app, &mut c, item);
        assert_eq!(choice(&COLORFUL_WAVEFORMS), "Multiband");
        assert_eq!(app.state.message.as_deref(), Some("Colorful waveforms: Multiband"));
        set_choice(&COLORFUL_WAVEFORMS, 0);
    }

    /// Every declared slider is a slider on a settings page, with a key and a label of its own among all the
    /// declarations, a default in range and a step.
    #[test]
    fn sliders_match_the_settings_pages() {
        let app = App::new(None);
        let mut shown = Vec::new();
        fn walk(r: &Row, shown: &mut Vec<&'static str>) {
            match r {
                Row::Slider(l, _) => shown.push(*l),
                Row::Pair(a, b) => {
                    walk(a, shown);
                    walk(b, shown);
                }
                _ => {}
            }
        }
        for tab in 0..settings::TABS.len() {
            for g in settings::page(tab, &app.state) {
                for r in &g.rows {
                    walk(r, &mut shown);
                }
            }
        }
        let mut keys: Vec<&str> = checks().map(|c| c.key).chain(COMBOS.iter().map(|c| c.key)).chain(NUMBERS.iter().map(|n| n.key)).collect();
        let mut labels: Vec<&str> = checks().map(|c| c.label).chain(COMBOS.iter().map(|c| c.label)).chain(NUMBERS.iter().map(|n| n.label)).collect();
        for s in SLIDERS {
            assert!(shown.contains(&s.label), "{} is a slider on a settings page", s.label);
            assert!(s.default <= s.max && s.step > 0, "{}: default and step", s.label);
            assert!(!s.key.is_empty() && !s.key.contains('=') && s.key.trim() == s.key, "key {:?}", s.key);
            assert!(!keys.contains(&s.key), "key {} declared twice", s.key);
            assert!(!s.label.is_empty() && !labels.contains(&s.label), "label {} declared twice", s.label);
            keys.push(s.key);
            labels.push(s.label);
        }
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
        assert_eq!((find_slider(""), find_slider("Smoothing")), (None, None));
        assert_eq!((number(&BROWSER_PREVIEW_TRACK), number_text(&BROWSER_PREVIEW_TRACK).as_str()), (0, "---"));
        set_number(&BROWSER_PREVIEW_TRACK, 5);
        assert_eq!((number(&BROWSER_PREVIEW_TRACK), number_text(&BROWSER_PREVIEW_TRACK).as_str()), (5, "5"));
        set_number(&BROWSER_PREVIEW_TRACK, 0);
        assert_eq!(number_text(&MAX_UNDO_LEVELS), "100", "a box from 1 shows its number");
        assert_eq!(find_number("Browser preview track"), Some(&BROWSER_PREVIEW_TRACK));
    }

    // spec: ui/settings-choices.md "Input: Vertical and Horizontal scrolling speed (2026-10-16)"
    /// General ▸ Input's scrolling speeds are live sliders: a click sets one from where it lands on the track, a drag
    /// follows the mouse, the wheel moves it by 32 a notch; the handle shows the value.
    #[test]
    fn scrolling_speed_sliders_work() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.open_settings(settings::GENERAL);
        app.state.settings.folded.extend(["Display", "Language"]);
        app.paint(&mut c, None);
        let track = |app: &App, s: &Slider| {
            app.hits
                .iter()
                .rev()
                .find_map(|h| match h.action {
                    Action::Settings(SAct::Slider(l, x, w)) if l == s.label => Some((h.rect, x, w)),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{} works", s.label))
        };
        for s in SLIDERS {
            assert!(!app.hits.iter().any(|h| h.action == Action::Placeholder(s.label)), "{} is not greyed out", s.label);
        }
        let (r, x, w) = track(&app, &VERTICAL_SCROLLING_SPEED);
        // A click three quarters along: 384; a drag past the left end: 0.
        app.mouse_move(x + w * 0.75, r.cy());
        app.mouse_down();
        assert_eq!(slider(&VERTICAL_SCROLLING_SPEED), 384);
        app.mouse_move(x - 40.0, r.cy() + 30.0);
        assert_eq!(slider(&VERTICAL_SCROLLING_SPEED), 0);
        app.mouse_move(x + w * 0.25, r.cy() + 30.0);
        app.mouse_up();
        assert_eq!(slider(&VERTICAL_SCROLLING_SPEED), 128);
        app.mouse_move(x + w + 40.0, r.cy());
        assert_eq!(slider(&VERTICAL_SCROLLING_SPEED), 128, "released: the mouse no longer moves it");
        assert_eq!(slider(&HORIZONTAL_SCROLLING_SPEED), 256, "the other one stays");
        app.paint(&mut c, None);
        // The wheel: 32 a notch, kept in 0..=512.
        let (r, _, _) = track(&app, &HORIZONTAL_SCROLLING_SPEED);
        app.mouse_move(r.cx(), r.cy());
        let scroll = app.state.settings.scroll;
        app.wheel(0.0, 1.0, false, false);
        assert_eq!(slider(&HORIZONTAL_SCROLLING_SPEED), 288);
        app.wheel(0.0, -1.0, false, false);
        app.wheel(0.0, -1.0, false, false);
        assert_eq!(slider(&HORIZONTAL_SCROLLING_SPEED), 224);
        assert_eq!(app.state.settings.scroll, scroll, "the page does not scroll");
        set_slider(&HORIZONTAL_SCROLLING_SPEED, 500);
        app.wheel(0.0, 1.0, false, false);
        assert_eq!(slider(&HORIZONTAL_SCROLLING_SPEED), 512);
        set_slider(&HORIZONTAL_SCROLLING_SPEED, 10);
        app.wheel(0.0, -1.0, false, false);
        assert_eq!(slider(&HORIZONTAL_SCROLLING_SPEED), 0);
        // The handle is drawn where the value is: the canvas changes with it.
        set_slider(&HORIZONTAL_SCROLLING_SPEED, 0);
        app.invalidate();
        app.paint(&mut c, None);
        let at0 = shot(&c);
        set_slider(&HORIZONTAL_SCROLLING_SPEED, 512);
        app.invalidate();
        app.paint(&mut c, None);
        assert_ne!(shot(&c), at0, "the handle moved");
        set_slider(&VERTICAL_SCROLLING_SPEED, 256);
        set_slider(&HORIZONTAL_SCROLLING_SPEED, 256);
    }

    // spec: ui/settings-choices.md "Mixer: Browser preview track (2026-10-16)"
    /// Audio ▸ Mixer's Browser preview track box shows "---" or a track; the wheel steps it from "---" through the
    /// open project's insert tracks, one a notch, and a click says so.
    #[test]
    fn browser_preview_track_box() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.open_settings(settings::AUDIO);
        app.paint(&mut c, None);
        let shown = |app: &App| {
            let rows = settings::page(settings::AUDIO, &app.state).into_iter().flat_map(|g| g.rows);
            rows.into_iter().find_map(|r| match r {
                Row::Value(v, l) if l == BROWSER_PREVIEW_TRACK.label => Some(v),
                _ => None,
            })
        };
        assert_eq!(shown(&app).as_deref(), Some("---"), "the box shows --- for none");
        let key = Action::Settings(SAct::Number(BROWSER_PREVIEW_TRACK.label));
        assert!(!app.hits.iter().any(|h| h.action == Action::Placeholder(BROWSER_PREVIEW_TRACK.label)));
        let r = hit_rect(&app, &key);
        click(&mut app, &mut c, r);
        assert_eq!(app.state.message.as_deref(), Some("Browser preview track: the mouse wheel changes it"));
        app.mouse_move(r.cx(), r.cy());
        app.wheel(0.0, 1.0, false, false);
        assert_eq!(number(&BROWSER_PREVIEW_TRACK), 1);
        assert_eq!(app.state.message.as_deref(), Some("Browser preview track: 1"));
        app.wheel(0.0, 2.0, false, false);
        assert_eq!(number(&BROWSER_PREVIEW_TRACK), 2, "one a notch");
        app.wheel(0.0, -1.0, false, false);
        app.wheel(0.0, -1.0, false, false);
        app.wheel(0.0, -1.0, false, false);
        assert_eq!((number(&BROWSER_PREVIEW_TRACK), app.state.message.as_deref()), (0, Some("Browser preview track: ---")));
        // Up to the open project's last insert track.
        let inserts = crate::mixer::inserts(&app.state);
        assert!(inserts > 0);
        set_number(&BROWSER_PREVIEW_TRACK, inserts);
        app.wheel(0.0, 1.0, false, false);
        assert_eq!(number(&BROWSER_PREVIEW_TRACK), inserts);
        // A track another project had comes back in range.
        set_number(&BROWSER_PREVIEW_TRACK, inserts + 10);
        app.wheel(0.0, -1.0, false, false);
        assert_eq!(number(&BROWSER_PREVIEW_TRACK), inserts);
        // The box shows the value.
        set_number(&BROWSER_PREVIEW_TRACK, 0);
        app.invalidate();
        app.paint(&mut c, None);
        let before = shot(&c);
        set_number(&BROWSER_PREVIEW_TRACK, 7);
        app.invalidate();
        app.paint(&mut c, None);
        assert_ne!(shot(&c), before, "the box shows 7");
        assert_eq!(shown(&app).as_deref(), Some("7"));
        set_number(&BROWSER_PREVIEW_TRACK, 0);
    }

    fn shot(c: &Canvas) -> Vec<u32> {
        let mut v = vec![0u32; 1920 * 1080];
        c.copy_to_xrgb(&mut v);
        v
    }
}
