// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 Samrat Dutta
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Settings window (OPTIONS > MIDI / Audio / General / File / Theme settings, Project settings, Project info,
//! Debugging log; F10 / F11): eleven tabs of grouped controls. The controls that work today are listed in `LIVE` and
//! handled in `App::settings_act`, or are program settings declared in `crate::prefs`; every other control is drawn
//! greyed out and only says it is not available yet.
//! The Plugins tab is the plugin manager (search folders, scanning, the plugin table).
// spec: ui/settings.md, ui/plugin-manager.md

use std::collections::HashSet;

use os_ui::kit::lemon as lk;
use os_ui::kit::{self, Dir};
use os_ui::{Color, Font, Rect};

use crate::app::{Action, State, Ui};
use crate::plugin_db::{self as db, Col, Prefer};

/// What a click in the settings window does (`Action::Settings`).
#[derive(Debug, Clone, PartialEq)]
pub enum SAct {
    /// The window body (swallows clicks, wheel scrolls the page).
    Body,
    Close,
    Tab(usize),
    /// A working check (radio LED) by its label (`LIVE`).
    Check(&'static str),
    /// A collapsible group header by its title.
    Group(&'static str),
    /// A working button by its label (`LIVE`).
    Button(&'static str),
    /// Skins tile: `os_theme::Design::ALL[i]`.
    Skin(usize),
    /// Themes tile `SettingsUi::themes[i]`: the Lemon and Citrus themes (the skins' own colours), then the rest.
    Preset(usize),
    /// Theme ▸ Adjustments knob `i` (`crate::theme_adjust::KNOBS`): a drag or the wheel turns it, a middle click restores
    /// the chosen theme's value.
    ThemeKnob(u8),
    /// Colour circle `.0` (`crate::theme_adjust::SWATCHES`), its colour `.1` (Steps: the left half is 0, the right 1):
    /// opens the colour picker under it.
    Swatch(u8, u8),
    /// The colour picker (`crate::theme_adjust::Picker`): its panel (swallows clicks), close button, colour chips,
    /// saturation / brightness square, hue strip, and Revert (also a click on the old colour).
    PickBody,
    PickClose,
    PickPart(u8),
    PickSv,
    PickHue,
    PickRevert,
    /// The name panel (`crate::theme_library::NameEdit`): its body (swallows clicks), OK and Cancel.
    NameBody,
    NameOk,
    NameCancel,
    /// The plugin manager's controls (spec: ui/plugin-manager.md §2).
    Plug(Plug),
    /// A working combo (`LIVE_COMBOS`) by its label, with its box's bottom left (where its menu opens).
    Combo(&'static str, f32, f32),
    /// An Info page text box: click to edit (`App::info_act`).
    Info(InfoField),
    /// The Info page's genre button, with its bottom left (where the genre menu opens; `App::genre_menu`).
    Genres(f32, f32),
    /// A working value box (`LIVE_NUMBERS`) by its label: the mouse wheel changes it (`App::settings_number`).
    Number(&'static str),
    /// A slider declared in `crate::prefs` by its label, with its track's left and width: a press sets it and drags it
    /// (`App::pref_slider_press`), the wheel steps it (`App::pref_slider_wheel`).
    Slider(&'static str, f32, f32),
    /// General ▸ Display's PPI ruler: a press starts a drag of its end (`App::ppi_press`), the wheel steps it
    /// (`App::ppi_wheel`).
    Ppi,
    /// Settings > MIDI's own controls (`super::midi_page`, `App::midi_page_act`).
    Midi(super::midi_page::MAct),
    /// File ▸ Browser extra search folders: row i's folder button (`App::search_folder_pick`).
    FolderPick(usize),
    /// Row i's Folder or Browser name cell: a click selects it for typing (`App::search_folder_cell`).
    FolderCell(usize, crate::search_folders::Col),
}

/// The Info page's text boxes: the project's credits (spec: ui/settings.md "Info", flp/events/project-info.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfoField {
    Title,
    Author,
    Genre,
    /// Several lines: Enter starts a new one.
    Comments,
    Url,
}

impl InfoField {
    pub fn multiline(self) -> bool {
        self == InfoField::Comments
    }

    pub fn get(self, i: &os_model::ProjectInfo) -> &Option<String> {
        match self {
            InfoField::Title => &i.title,
            InfoField::Author => &i.author,
            InfoField::Genre => &i.genre,
            InfoField::Comments => &i.comments,
            InfoField::Url => &i.url,
        }
    }

    pub fn get_mut(self, i: &mut os_model::ProjectInfo) -> &mut Option<String> {
        match self {
            InfoField::Title => &mut i.title,
            InfoField::Author => &mut i.author,
            InfoField::Genre => &mut i.genre,
            InfoField::Comments => &mut i.comments,
            InfoField::Url => &mut i.url,
        }
    }

    fn hint(self) -> &'static str {
        match self {
            InfoField::Title => "Project title: the window caption shows it instead of the file name",
            InfoField::Author => "Project author",
            InfoField::Genre => "Project genre",
            InfoField::Comments => "Project comments",
            InfoField::Url => "Project web link",
        }
    }
}

/// The Info box that has the keyboard: its field and the text being typed (the file dialog's text field: caret,
/// selection). Enter, a click elsewhere or another tab writes it into the project; Esc drops it.
#[derive(Debug, Clone, PartialEq)]
pub struct InfoEdit {
    pub field: InfoField,
    pub text: crate::dialog::Field,
}

/// Plugin manager controls. Rows count in `plugin_db::search_dirs` order (folders) or in `Db::rows` (table).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Plug {
    /// Search folder i: searched or not.
    Folder(usize),
    /// Remove added search folder i.
    Remove(usize),
    Prefer(Prefer),
    /// Column title: sort by it (again: reverse).
    Sort(Col),
    /// The table body (the wheel scrolls it).
    Table,
    /// Table row i (its hint: the path, why a file failed).
    Row(usize),
    Star(usize),
    Show(usize),
    /// The Find box: typing filters the table.
    Find,
}

/// Session state of the window.
#[derive(Debug, Default)]
pub struct SettingsUi {
    pub open: bool,
    pub tab: usize,
    /// Page scroll in pixels.
    pub scroll: f32,
    /// Groups whose open state differs from their default.
    pub folded: HashSet<&'static str>,
    /// Filled when the window opens: output devices and ASIO drivers, theme files (name, path), selected preset.
    pub devices: Vec<String>,
    pub asio_drivers: Vec<String>,
    pub device: String,
    /// The input devices, and the Input device box's text.
    pub inputs: Vec<String>,
    pub input: String,
    /// Audio page: the audio settings, the rate streams open at, and the status lines (kept up to date while shown).
    pub audio: crate::audio_cfg::AudioCfg,
    pub rate: Option<u32>,
    /// What the chosen Windows device offers for short buffers (`device::periods`; `None` under an ASIO driver, off
    /// Windows, or when Windows does not say).
    pub periods: Option<crate::device::Periods>,
    /// The chosen ASIO driver as it last opened in this session (its rate and buffer length, the rates and lengths it
    /// takes), whether an ASIO driver is open now (Show ASIO panel works), and the device playing in place of the
    /// driver chosen (`DeviceInfo::instead`).
    pub driver: Option<crate::device::DeviceInfo>,
    pub asio_open: bool,
    pub instead: Option<String>,
    pub status: [String; 2],
    /// Theme presets (name, `.flstheme` text): built-ins (spec ui/theme-presets.md), then the user's theme folder.
    pub themes: Vec<(String, String)>,
    /// The chosen Themes tile (`SAct::Preset`; `usize::MAX`: none) and Skins tile (`SAct::Skin`).
    pub preset: usize,
    pub skin: usize,
    /// The tiles' previews (`App::sync_preset`): each skin under the applied theme, and the skin in use under each of
    /// `themes`.
    pub skin_previews: Vec<os_theme::Palette>,
    pub theme_previews: Vec<os_theme::Palette>,
    /// Theme ▸ Adjustments' colour picker, while open.
    pub picker: Option<crate::theme_adjust::Picker>,
    /// The Themes tiles from here on are the user's own (`crate::theme_library::user_dir`).
    pub user_from: usize,
    /// The name panel (Save preset..., a tile's Rename... / Delete...), while open.
    pub name_edit: Option<crate::theme_library::NameEdit>,
    /// Theme undo / redo.
    pub history: crate::theme_library::History,
    /// The applied theme's text is hard to read: its readability (`crate::theme_library::readability`).
    pub unreadable: Option<f32>,
    /// The Info page's box being edited.
    pub info_edit: Option<InfoEdit>,
    /// The File page's Browser extra search folders cell being typed into.
    pub folder_edit: Option<crate::search_folders::CellEdit>,
    /// The Plugins page shows the experimental native host toggles: `os_native::settings_shown()` at startup.
    pub native: bool,
    /// Project ▸ Time settings: the last bar (beats per bar, steps per beat) of each "Set as" mode, indexed by
    /// `signature as usize` (0 = time division), kept while the program runs; `None` = 4/4 (spec:
    /// ui/settings-choices.md "Set as time division").
    pub bar_memory: [Option<(u16, u16)>; 2],
    /// General ▸ System: `.flp` files open with this program, as Windows said when the page last opened or was
    /// clicked (`crate::file_assoc`); always false on macOS and Linux, where the check is greyed.
    pub assoc: bool,
}

/// Tabs: label, page title, group (0 system / 1 project / 2 info) (spec §2). Plugins takes the place of FL's Account
/// tab (spec: ui/plugin-manager.md §2); tabs are laid out from their text widths (`tab_rects`).
pub const TABS: [(&str, &str, u8); 11] = [
    ("MIDI", "MIDI input / output devices", 0),
    ("Audio", "Audio devices / mixer", 0),
    ("Recording", "Recording options", 0),
    ("General", "System / miscellaneous", 0),
    ("File", "Files & folders", 0),
    ("Plugins", "Plugin manager", 0),
    ("Theme", "Look and feel", 0),
    ("Project", "Current project", 1),
    ("Info", "Project credits", 1),
    ("Debug", "Debugging log", 2),
    ("About", "About", 2),
];
pub const MIDI: usize = 0;
pub const AUDIO: usize = 1;
/// Lemon Studio's own page (FL has none): the recording settings, spec ui/settings.md "Recording".
pub const RECORDING: usize = 2;
pub const GENERAL: usize = 3;
pub const FILE: usize = 4;
pub const PLUGINS: usize = 5;

/// Opt in to the experimental FL native plugin host (spec plugins/fruity-abi.md), on the Plugins page only with
/// `OPENSTUDIO_NATIVE_SETTINGS` (`SettingsUi::native`). Labels are the keys for `SAct::Check`; `App::settings_act`
/// acts on them.
pub const NATIVE_HOST_CHECK: &str = "Experimental: host installed native plugins";
/// Shown only while the native host is on: an opened project's native plugins are played by the installed plugins,
/// never by the built-in Lemon recreations (spec plugins/fruity-abi.md "Opened projects").
pub const NATIVE_FIRST_CHECK: &str = "Experimental: real native plugins instead of Lemon recreations";
/// Plugin manager checks (spec: ui/plugin-manager.md §2).
pub const STARTUP_CHECK: &str = "Scan for new plugins at startup";
pub const ERRORS_CHECK: &str = "Rescan plugins with errors";
pub const COMBINE_CHECK: &str = "Combine formats of the same plugin";

/// Whether `key` is one of the plugin manager's own checks (handled by `App::plugin_check`).
pub fn plugin_check(key: &str) -> bool {
    [STARTUP_CHECK, ERRORS_CHECK, COMBINE_CHECK].contains(&key)
}

/// General ▸ System: whether `.flp` files open with this program. Not a stored setting: on Windows the check shows what
/// the registry says (`SettingsUi::assoc`, `crate::file_assoc`); on macOS and Linux it is greyed. Spec:
/// ui/settings-choices.md "System: Associate project files with application (2026-10-16)".
pub const ASSOCIATE_CHECK: &str = "Associate project files with application";
/// Its hint where the system's own "Open with" settings decide (spec).
#[cfg(target_os = "macos")]
const ASSOCIATE_ELSEWHERE: &str = "The app bundle declares .flp files; the system's Open with settings decide";
#[cfg(all(not(windows), not(target_os = "macos")))]
const ASSOCIATE_ELSEWHERE: &str = "The system's Open with settings decide which program opens .flp files";

/// The checks and buttons that work (by label, handled in `App::settings_act`). Apart from these, `LIVE_COMBOS` and the
/// program settings declared in `crate::prefs`, every other check, combo, value box, slider and button is not available
/// yet: drawn greyed out, its hint and click say so.
pub const LIVE: &[&str] = &[
    NATIVE_HOST_CHECK,
    NATIVE_FIRST_CHECK,
    STARTUP_CHECK,
    ERRORS_CHECK,
    COMBINE_CHECK,
    TRUNCATED_CHECK,
    CROSSFADES_CHECK,
    SHOW_INFO_CHECK,
    NOTE_COLORS_CHECK,
    SAVE_PRESET,
    THEME_UNDO,
    THEME_REDO,
    FIX_TEXT,
    RESET_WORK_TIME,
    MANAGE_SHORTCUTS,
    "Manage plugins",
    "Manage messages",
    REFRESH_MIDI,
    "Add folder...",
    "Scan for new plugins",
    "Rescan all",
    "Load theme...",
    "Reset",
    ASSOCIATE_CHECK,
    SHOW_ASIO_PANEL,
];

/// MIDI: lists the devices again (`App::midi_refresh`); Update MIDI scripts stays greyed with its reason.
pub const REFRESH_MIDI: &str = "Refresh device list";
pub const UPDATE_SCRIPTS: &str = "Update MIDI scripts";
const NO_SCRIPTS: &str = "Lemon Studio has no MIDI scripts";
/// MIDI: Performance mode MIDI channel stays greyed with its reason.

/// `LIVE`, and the checks declared in `crate::prefs`.
fn live(key: &str) -> bool {
    LIVE.contains(&key) || crate::prefs::find_check(key).is_some()
}

/// The combos that work (they open a menu of choices, `App::audio_menu`).
pub const LIVE_COMBOS: [&str; 6] = [OUTPUT_DEVICE, INPUT_DEVICE, MODE, SAMPLE_RATE, BUFFER_LENGTH, ANIMATIONS];
/// General ▸ Display: "Playful" simulates the mixer's cables, "Off" hangs them still (crate::cable).
pub const ANIMATIONS: &str = "Animations";
pub const OUTPUT_DEVICE: &str = "Output device";
/// The device the mixer tracks' inputs and one-click recording take in from (not under an ASIO driver, whose inputs
/// are its own).
pub const INPUT_DEVICE: &str = "Input device";
pub const SAMPLE_RATE: &str = "Sample rate (Hz)";
pub const BUFFER_LENGTH: &str = "Buffer length";
/// Windows devices: shared mode, or exclusive mode with the device to Lemon Studio alone (spec:
/// audio/wasapi-low-latency.md 4.2); its menu's two choices; its hint under an ASIO driver, which has no such mode.
/// Linux: shared mode plays through the sound server (PulseAudio or PipeWire), exclusive mode opens an ALSA device
/// itself (`audio_cfg::host`).
pub const MODE: &str = "Mode";
pub const MODE_SHARED: &str = "Shared (other programs play too)";
#[cfg(not(target_os = "linux"))]
pub const MODE_EXCLUSIVE: &str = "Exclusive (Lemon Studio alone, smaller buffers)";
#[cfg(target_os = "linux")]
pub const MODE_EXCLUSIVE: &str = "Exclusive (ALSA directly, past the sound server)";
const MODE_ASIO: &str = "Mode: for Windows devices (an ASIO driver plays on its own terms)";
/// With an ASIO driver: its own settings window, where its buffer length is set.
pub const SHOW_ASIO_PANEL: &str = "Show ASIO panel";
/// Show ASIO panel's hint while no ASIO driver is open.
const PANEL_WAITS: &str = "Show ASIO panel: the ASIO driver's own settings, while one is open";
/// The Audio page's line under Show ASIO panel on Windows while no ASIO driver is installed (the page is filled again
/// each time Settings opens, so a driver installed meanwhile is listed then).
pub const NO_ASIO_DRIVER: &str = "No ASIO driver is installed. For lower latency, install one (ASIO4ALL is free), then reopen Settings.";

/// Project ▸ Time settings (spec: ui/settings-choices.md "Project"): the timebase, the "Set as" mode (event 35; the
/// combo has no label beside it), and the bar (events 17, 18) as a time signature (Numerator / Denominator, FL's
/// choices) or as a time division (Beats per bar / Steps per beat value boxes).
pub const TIMEBASE: &str = "Timebase (PPQ)";
pub const TIMEBASES: &[u16] = &[24, 48, 72, 96, 120, 144, 168, 192, 384, 768, 960];
pub const SET_AS: &str = "Set as";
pub const SET_AS_SIGNATURE: &str = "Set as time signature";
pub const SET_AS_DIVISION: &str = "Set as time division";
pub const NUMERATOR: &str = "Numerator";
pub const DENOMINATOR: &str = "Denominator";
pub const NUMERATORS: &[u16] = &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
pub const DENOMINATORS: &[u16] = &[2, 4, 8, 16];
pub const BEATS_PER_BAR: &str = "Beats per bar";
pub const STEPS_PER_BEAT: &str = "Steps per beat";
/// Project ▸ Advanced ▸ Panning law (event 23; spec: ui/settings-choices.md "Project", audio/gain-laws.md "Panning
/// law: Triangular"): FL's choices and their values.
pub const PANNING_LAW: &str = "Panning law";
pub const PANNING_LAWS: [(&str, u8); 2] = [("Circular", 0), ("Triangular", 2)];
/// The Project page's combos that work (their menus: `App::audio_menu`).
pub const PROJECT_COMBOS: [&str; 5] = [TIMEBASE, SET_AS, NUMERATOR, DENOMINATOR, PANNING_LAW];
/// The value boxes that work, with their ranges (the mouse wheel changes them: `App::settings_number`).
pub const LIVE_NUMBERS: [(&str, u16, u16); 2] = [(BEATS_PER_BAR, 1, 32), (STEPS_PER_BEAT, 1, 16)];

/// The range of the working value box labelled `label` (`LIVE_NUMBERS`, and the value boxes declared in
/// `crate::prefs`).
pub fn number_range(label: &str) -> Option<(u16, u16)> {
    LIVE_NUMBERS.iter().find(|n| n.0 == label).map(|n| (n.1, n.2)).or_else(|| crate::prefs::find_number(label).map(|n| (n.min, n.max)))
}

/// The Denominator box's text for `steps` per beat: 16 / steps (spec: flp/engine-events.md "Project"); steps that make
/// no signature (3) show the quotient to two decimals.
pub fn denominator_text(steps: u16) -> String {
    let steps = steps.max(1);
    if 16 % steps == 0 { (16 / steps).to_string() } else { format!("{:.2}", 16.0 / f64::from(steps)) }
}
/// Project ▸ Advanced: "play truncated notes" (event 30; spec: audio/song-structure.md "Play truncated notes").
pub const TRUNCATED_CHECK: &str = "Play truncated notes in clips";
/// Project ▸ Playlist: "Automatic crossfades" (event 44; spec: ui/settings-choices.md "Project"). Its neighbour,
/// "Enable automatic crossfades in new projects", is a program setting (`prefs::AUTO_CROSSFADES_NEW`).
pub const CROSSFADES_CHECK: &str = "Automatic crossfades";
/// Info: "Show info on opening" (event 10) and the button that zeroes the time spent (event 237; spec:
/// ui/settings-choices.md "Info", flp/events/project-info.md).
pub const SHOW_INFO_CHECK: &str = "Show info on opening";
/// Theme ▸ Options: the applied theme's `OverrideClips` (`App::toggle_theme_option`; spec: ui/settings-choices.md
/// "Light mode and clip note colours"). There is no Light mode option (removed 2026-10-18; themes draw as with it on).
pub const NOTE_COLORS_CHECK: &str = "Audio and automation clips use note colors";
/// Theme: save the applied theme under a name, undo / redo theme changes, and the readability warning's fix
/// (`crate::theme_library`).
pub const SAVE_PRESET: &str = "Save preset...";
pub const THEME_UNDO: &str = "Undo";
pub const THEME_REDO: &str = "Redo";
pub const FIX_TEXT: &str = "Fix text";
pub const RESET_WORK_TIME: &str = "Reset time spent on project";
/// General ▸ Input: opens the Manage shortcuts window (`App::open_shortcuts`).
pub const MANAGE_SHORTCUTS: &str = "Manage shortcuts";

/// General ▸ Miscellaneous: FILE ▸ New's template; its tree is found at run time (`crate::templates`).
pub const DEFAULT_TEMPLATE: &str = "Default template";

/// Whether the combo labelled `label` works (opens a menu of choices).
fn live_combo(label: &str) -> bool {
    LIVE_COMBOS.contains(&label) || PROJECT_COMBOS.contains(&label) || label == DEFAULT_TEMPLATE
}

/// A buffer length as the Audio page shows it: frames and milliseconds at `rate`.
pub fn buffer_text(frames: u32, rate: u32) -> String {
    format!("{frames} frames ({:.1} ms)", f64::from(frames) * 1000.0 / f64::from(rate.max(1)))
}

pub const THEME: usize = 6;
pub const PROJECT: usize = 7;
pub const INFO: usize = 8;
pub const DEBUG: usize = 9;

/// Plugin table rows shown at once (spec: ui/plugin-manager.md §2).
pub const TABLE_ROWS: usize = 18;
pub const TABLE_ROW_H: f32 = 17.0;
/// Table columns: x from the table's left edge, width.
const COLS: [(f32, f32); 8] = [(0.0, 20.0), (20.0, 38.0), (58.0, 152.0), (210.0, 50.0), (260.0, 68.0), (328.0, 112.0), (440.0, 66.0), (506.0, 80.0)];

pub const W: f32 = 668.0;
const LABEL: Font = Font::new(13);
const TABS_B: f32 = 64.0;
const GX: f32 = 30.0;
const GW: f32 = 608.0;

/// The window rect: 668 x 900 at (626, 70) on a 1920x1080 screen, centred and shrunk to fit smaller screens.
pub fn rect(s: &State) -> Rect {
    let (sw, sh) = s.screen;
    let h = 900.0_f32.min(sh - 20.0).max(200.0);
    let y = 70.0_f32.min(sh - h - 10.0).max(0.0);
    Rect::new(((sw - W) / 2.0).max(0.0).round(), y, W, h)
}

/// One control row. Labels double as keys (unique per page).
pub enum Row {
    /// Check: label, default.
    Check(&'static str, bool),
    /// Two checks side by side (second at x 340).
    Check2(&'static str, bool, &'static str, bool),
    /// Disabled check (drawn dim). No page has one at the moment.
    #[allow(dead_code)]
    Off(&'static str, bool),
    /// Combo box: value, box width, label to its right.
    Combo(String, f32, &'static str),
    /// Red value box: value, label to its right.
    Value(String, &'static str),
    /// Label + red value box on the right side of the panel.
    ValueR(&'static str, String),
    /// Buttons in a row: (label, width).
    Buttons(Vec<(&'static str, f32)>),
    /// Slider: label, 0..1.
    Slider(&'static str, f32),
    /// List: column (title, x), rows of cells, visible rows.
    List(Vec<(&'static str, f32)>, Vec<Vec<String>>, usize),
    /// Plain text line (dim = small grey).
    Text(String, bool),
    /// Large text (About page).
    Big(String),
    /// Settings > Theme tiles with rendered previews (`theme_preview`): which list, the names, the chosen one.
    Tiles(TileList, Vec<String>, usize),
    /// Theme adjustment knobs and colour circles.
    ThemeKnobs,
    /// Theme: the applied theme's text is hard to read; a Fix text button (`crate::theme_library`).
    ThemeWarn,
    Gap(f32),
    /// Two controls on one line: left one, right one (right-hand column at x 304).
    Pair(Box<Row>, Box<Row>),
    /// A display that belongs to a feature not available yet (name for the hint): drawn greyed out.
    Dim(&'static str, Box<Row>),
    /// Plugin search folders: (path, tag, searched, added by the user).
    Folders(Vec<(String, &'static str, bool, bool)>),
    /// File ▸ Browser extra search folders (reads `State::browser_folders`, `crate::search_folders`): the rows shown.
    SearchFolders(usize),
    /// The preferred-format switch.
    Prefer(Prefer),
    /// The plugin table with its count and Find box (reads `State::plugdb`).
    Table,
    /// An Info text box: field, width (0 = to the panel's right edge). The Genre box has a ▸ after it.
    Edit(InfoField, f32),
    /// The Info page's footer: small grey lines at the left, the Reset working time button at the bottom right.
    InfoFooter(Vec<String>),
    /// General ▸ Display's PPI ruler (`crate::ppi`, `ppi_ruler`).
    Ppi,
    /// Project ▸ Data folder's Rename button, folder button and path (`super::data_folder`).
    DataFolder,
    /// File ▸ FL Studio factory content folder's Reset button, folder button and path (`super::factory_folder`).
    FactoryFolder,
    /// A row of Settings > MIDI drawn by `super::midi_page`.
    Midi(super::midi_page::MRow),
    /// The ASIO Compatible Logo with Steinberg's trademark notice to its right (`asio_logo`), on Windows, whose build
    /// includes the ASIO SDK: the SDK's usage guidelines ask for it in the About box and where ASIO is switched on.
    AsioLogo,
}

/// The two tile lists of Settings > Theme (spec: ui/theme-presets.md "Settings").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileList {
    /// The skins (`os_theme::Design::ALL`), each previewed under the applied theme.
    Skins,
    /// The themes (the Lemon and Citrus themes first), each previewed in the skin in use.
    Themes,
}

/// Theme tiles, five to a row: size, pitch, the preview's height (the miniature's shape) and the name strip under it.
const TILE_W: f32 = 110.0;
const TILE_PREVIEW_H: f32 = ((TILE_W - 4.0) * super::theme_preview::H / super::theme_preview::W) as i32 as f32;
const TILE_NAME_H: f32 = 18.0;
const TILE_H: f32 = TILE_PREVIEW_H + TILE_NAME_H + 4.0;
const TILE_PITCH_X: f32 = TILE_W + 8.0;
const TILE_PITCH_Y: f32 = TILE_H + 6.0;
const TILES_PER_ROW: usize = 5;

pub struct Group {
    pub title: &'static str,
    pub note: &'static str,
    /// Has a fold chevron; `closed` = folded by default.
    pub fold: bool,
    pub closed: bool,
    pub rows: Vec<Row>,
}

fn pair(a: Row, b: Row) -> Row {
    Row::Pair(Box::new(a), Box::new(b))
}
fn g(title: &'static str, rows: Vec<Row>) -> Group {
    Group { title, note: "", fold: false, closed: false, rows }
}
fn gf(title: &'static str, closed: bool, rows: Vec<Row>) -> Group {
    Group { title, note: "", fold: true, closed, rows }
}
fn combo(v: &str, w: f32, label: &'static str) -> Row {
    Row::Combo(v.into(), w, label)
}
/// The panning law's name for event 23 (`PANNING_LAWS`; none is Circular); a value outside those shows as a number.
pub fn panning_text(law: Option<u8>) -> String {
    let v = law.unwrap_or(0);
    PANNING_LAWS.iter().find(|l| l.1 == v).map_or_else(|| v.to_string(), |l| l.0.to_string())
}
fn txt(v: impl Into<String>) -> Row {
    Row::Text(v.into(), false)
}
fn dim(name: &'static str, r: Row) -> Row {
    Row::Dim(name, Box::new(r))
}

/// The groups of page `tab` (spec §4). FL-only groups (cloud, remote app, licence) are left out.
pub fn page(tab: usize, s: &State) -> Vec<Group> {
    use Row::*;
    let st = &s.settings;
    match tab {
        // spec: ui/midi-settings.md §1. The device rows are `super::midi_page`'s; the checks, link combos, sync type and
        // channel boxes are `crate::prefs` declarations.
        MIDI => {
            use super::midi_page::MRow;
            use crate::midi_cfg::{CurveOf, Offset};
            use crate::prefs::{self as p, number_text};
            vec![
                g("Output", vec![
                    Midi(MRow::Outputs),
                    Midi(MRow::OutOptions),
                    pair(Midi(MRow::Offset(Offset::MasterSync)), combo(p::choice(&p::SYNC_TYPE), 168.0, p::SYNC_TYPE.label)),
                ]),
                g("Input", vec![
                    Midi(MRow::Inputs),
                    Midi(MRow::InOptions),
                    combo(p::choice(&p::CONTROLLER_TYPE), 250.0, p::CONTROLLER_TYPE.label),
                    pair(Midi(MRow::Link(CurveOf::Velocity)), ValueR(p::OMNI_CHANNEL.label, number_text(&p::OMNI_CHANNEL))),
                    pair(Midi(MRow::Link(CurveOf::Release)), ValueR(p::MARKER_CHANNEL.label, number_text(&p::MARKER_CHANNEL))),
                    pair(Check(p::PICKUP.label, false), ValueR(p::PERFORMANCE_CHANNEL.label, number_text(&p::PERFORMANCE_CHANNEL))),
                    pair(Check(p::AUTO_ACCEPT.label, true), ValueR(p::MUTING_CHANNEL.label, number_text(&p::MUTING_CHANNEL))),
                    pair(Check(p::FOOT_PEDAL.label, true), Check(p::TOGGLE_ON_RELEASE.label, false)),
                ]),
                g("External sync", vec![pair(Midi(MRow::ExtSync), Midi(MRow::Offset(Offset::External))), Midi(MRow::Status)]),
                g("", vec![Buttons(vec![(REFRESH_MIDI, 136.0), (UPDATE_SCRIPTS, 136.0)])]),
            ]
        }
        AUDIO => {
            // Device, rate and buffer length work (spec: ui/settings.md "Audio"); a choice of "the device's own" shows
            // what that is.
            // Under an ASIO driver its own rate and buffer length are the "Driver" ones (spec: audio/devices.md).
            let rate = st.rate.unwrap_or(44100);
            let asio = st.audio.asio_driver().is_some();
            let rate_text = match st.audio.rate {
                Some(r) => r.to_string(),
                None if asio => format!("Driver ({rate})"),
                None => format!("Device ({rate})"),
            };
            // A Windows device's own period and its smallest, as the device says (spec: audio/wasapi-low-latency.md 4.3).
            let exclusive = st.audio.exclusive();
            let periods = st.periods.as_ref().filter(|_| !asio);
            let buffer = match st.audio.buffer {
                Some(crate::audio_cfg::SMALLEST) => match periods {
                    Some(p) => format!("Smallest ({})", buffer_text(p.exclusive_frames(rate).1, rate)),
                    None => "Smallest".into(),
                },
                Some(b) => buffer_text(b, rate),
                None if asio => match st.driver.as_ref().and_then(|d| d.fixed.map(|b| buffer_text(b, d.rate))) {
                    Some(b) => format!("Driver ({b})"),
                    None => "Driver".into(),
                },
                None => match periods {
                    Some(p) => format!("Device ({})", buffer_text(p.own(exclusive, rate), rate)),
                    None => "Device default".into(),
                },
            };
            let listed = st.devices.iter().cloned().chain(st.asio_drivers.iter().map(|d| format!("{d} (ASIO)")));
            vec![
                g("Input / output", [
                    vec![combo(&st.device, 360.0, OUTPUT_DEVICE)],
                    if asio { Vec::new() } else { vec![combo(&st.input, 360.0, INPUT_DEVICE)] },
                    // Windows devices play shared or exclusive (greyed under an ASIO driver, `MODE_ASIO`); so do Linux
                    // ones, through the sound server or ALSA.
                    if cfg!(windows) || cfg!(target_os = "linux") {
                        let mode = combo(if st.audio.exclusive { "Exclusive" } else { "Shared" }, 120.0, MODE);
                        let device = st.audio.device.as_deref().unwrap_or("the default device");
                        let alone = exclusive.then(|| {
                            let line = if cfg!(windows) {
                                format!("Other programs can't play on {device} while Lemon Studio has it open.")
                            } else {
                                format!("Other programs can't play on {device} while Lemon Studio has it open through ALSA (it lets go once nothing plays).")
                            };
                            Text(line, true)
                        });
                        std::iter::once(mode).chain(alone).collect()
                    } else {
                        Vec::new()
                    },
                    vec![
                        combo(&rate_text, 120.0, SAMPLE_RATE),
                        combo(&buffer, 190.0, BUFFER_LENGTH),
                        Check("Auto close", false),
                    ],
                    st.instead.iter().map(|i| Text(i.clone(), true)).collect(),
                    vec![Text(st.status[0].clone(), true), Text(st.status[1].clone(), true)],
                    // Below Status, as FL has it; there are ASIO drivers on Windows only. The ASIO logo goes with them
                    // (the SDK's usage guidelines: in every dialog that switches ASIO on). With none installed, a line
                    // says to install one.
                    if cfg!(windows) {
                        let none = st.asio_drivers.is_empty().then(|| Text(NO_ASIO_DRIVER.into(), true));
                        [pair(Buttons(vec![(SHOW_ASIO_PANEL, 136.0)]), Row::AsioLogo)].into_iter().chain(none).collect()
                    } else {
                        Vec::new()
                    },
                    vec![
                        List(vec![("Output devices", 0.0)], listed.map(|d| vec![d]).collect(), 4),
                        Check2("Mix at buffer boundaries", false, "Triple buffer", false),
                        combo("Highest", 96.0, "Priority"),
                        Check("Avoid dropouts under load", true),
                        combo("Mixer", 96.0, "Playback tracking"),
                    ],
                ]
                .into_iter()
                .flatten()
                .collect()),
                g("CPU", vec![
                    Check2("Multithreaded instrument processing", true, "Auto-bypass when silent", true),
                    Check2("Multithreaded mixer processing", true, "Align tick lengths", false),
                ]),
                g("Mixer", vec![
                    combo("24-point sinc", 122.0, "Resampling quality"),
                    Value(crate::prefs::number_text(&crate::prefs::BROWSER_PREVIEW_TRACK), "Browser preview track"),
                    Value(crate::prefs::number_text(&crate::prefs::METRONOME_TRACK), "Metronome track"),
                    Check("Play truncated notes on transport", true),
                    Check("Reset plugins on transport", false),
                ]),
            ]
        }
        // Lemon Studio's own page (spec: ui/settings.md "Recording"): every control is a `crate::prefs` declaration that
        // the Record button's menus, the toolbar's option buttons and the mixer's Disk recording menu share.
        RECORDING => {
            use crate::prefs as p;
            // Where takes go now: the project's data folder's Recorded folder, else the user folder's.
            let folder = crate::track_rec::folder(s).display().to_string();
            vec![
                g("Recording filter", vec![Check2(p::REC_NOTES.label, true, p::REC_AUTOMATION.label, true), Check(p::REC_AUDIO.label, false)]),
                g("Options", vec![
                    Check2(p::REC_ASK.label, true, p::REC_LOOP.label, false),
                    Check2(p::REC_STARTS_PLAYBACK.label, false, p::REC_BLEND.label, false),
                    Check(p::REC_DISARM_ON_STOP.label, false),
                ]),
                g("Quantize", vec![Check2(p::REC_QUANTIZE_START.label, true, p::REC_QUANTIZE_END.label, true)]),
                g("Start on input", vec![Check(p::REC_WAIT.label, false), Check2(p::REC_WAIT_FILTER.label, true, p::REC_WAIT_ARMED.label, false)]),
                g("Count-in", vec![Check(p::REC_COUNT_IN.label, false), combo(p::choice(&p::COUNT_IN_LENGTH), 80.0, p::COUNT_IN_LENGTH.label)]),
                g("Audio recording", vec![
                    Check(p::AUTO_AUDIO_CLIPS.label, true),
                    combo(p::choice(&p::EXTERNAL_INPUT), 230.0, p::EXTERNAL_INPUT.label),
                    Text("Recordings go to the project's data folder's Recorded folder, else the user folder's:".into(), true),
                    Text(folder, true),
                    Text("The input device is chosen on the Audio page.".into(), true),
                ]),
            ]
        }
        GENERAL => vec![
            // Display and Language apply at once, so they leave off FL's "(requires a restart to take effect)" (spec:
            // ui/settings.md "General").
            gf("Display", false, vec![
                combo("System", 64.0, "Main GUI scaling"),
                // spec: ui/settings-choices.md "Display: PPI ruler, Force refreshes, Thick lines, High visibility
                // (2026-10-16)".
                txt("Define 1 inch / 2.54 cm or set display's PPI"),
                Text(crate::ppi::label(crate::ppi::ppi(s.display_scale), s.display_scale), true),
                Ppi,
                combo("Main", 64.0, "Pop-ups scaling"),
                combo("Main", 64.0, "Toolbars scaling"),
                combo("Off", 64.0, "Transparent windows"),
                combo(if s.cables.borrow().animations() { "Playful" } else { "Off" }, 126.0, ANIMATIONS),
                Slider("Smoothing", 0.2),
                combo("Ultrasmooth", 126.0, "Animation refresh rate"),
                Check2("Force refreshes", true, "Transparent menus", true),
                Check("Thick lines", false),
                Check("High visibility", false),
                combo("Off", 146.0, "Colorful waveforms"),
            ]),
            gf("Language", false, vec![
                combo("English (US) (en)", 176.0, "Language"),
                combo("English", 176.0, "Note names"),
                combo("Sharps - #", 96.0, "Black notes"),
                Buttons(vec![("Update languages", 138.0)]),
            ]),
            gf("Input", false, vec![
                combo("Multi-button mouse", 136.0, "Optimize for"),
                Buttons(vec![(MANAGE_SHORTCUTS, 212.0)]),
                Check("Click-and-hold gestures", false),
                Check("Right button clears steps", false),
                Slider("Vertical scrolling speed", 0.5),
                Slider("Horizontal scrolling speed", 0.5),
                Check("Swap mouse buttons for drawing", false),
            ]),
            // spec: ui/settings-choices.md "Undo history: the four checks (2026-10-16)" (`crate::history`).
            gf("Undo history", false, vec![
                Value(crate::prefs::number(&crate::prefs::MAX_UNDO_LEVELS).to_string(), "Maximum undo levels"),
                Check("Undo knob tweaks", true),
                Check("Alternate undo mode", true),
                Check("Move undone recordings to the trash", true),
                Check("Show undo history in the browser when undoing changes", false),
                Check("Show recent changes at the top", false),
            ]),
            gf("Hints and warnings", false, vec![
                Buttons(vec![("Manage messages", 138.0)]),
                combo("100MB (default)", 110.0, "Project data size warning"),
            ]),
            gf("System", false, vec![Check(ASSOCIATE_CHECK, s.settings.assoc)]),
            // FL's "Updates and privacy" without Send usage data and its More information button: Lemon Studio collects
            // nothing. The update checks stay greyed until Lemon Studio can update itself (spec: ui/settings.md
            // "General").
            gf("Updates", false, vec![
                Check("Automatically check for updates", true),
                Check("Automatically download program updates", true),
            ]),
            gf("Miscellaneous", false, vec![
                Check("Name channels after their sample or plugin", true),
                Check("Name effect slots after their plugin", false),
                Check("Auto-collapse empty channels", false),
                Check("Select linked controls automatically", true),
                Check("Auto zoom in piano roll", false),
                Check("Small scrollbars in editors", false),
                Check2("Open new plugin windows floating", false, "Open detached plugins with the main window", false),
                Check("Disable computer keyboard priority by default", false),
                Check("Alternate meter scale", false),
                Check("Show startup splash screen", true),
                Check("Restore previous state after solo", false),
                Check("Hide plugin window strip by default", true),
                Check("Silent startup", false),
                Check("Monophonic painting in piano roll", false),
                Check("Create automation clips at the playback position", true),
                Check("Select first channel with notes when the pattern changes", false),
                Check("Limit the number of open plugin editors", true),
                combo(&crate::templates::name(), 160.0, DEFAULT_TEMPLATE),
                combo("Default template", 160.0, "Startup project"),
            ]),
            gf("Advanced", false, vec![
                Check("Don't limit windows to screen", false),
                Check("Fast sample preview", true),
                Check("Auto keep long audio on disk", true),
                Check2("Read sample tempo information", true, "Read sample root note", true),
                Check("Set start values automatically", true),
                Check("Use the high-performance power plan", true),
                Check("Threaded project sample loading", false),
                combo("System", 64.0, "Cursors scaling"),
                combo("Main", 64.0, "Legacy scaling"),
                combo("Show help in a web browser", 160.0, ""),
            ]),
        ],
        FILE => {
            let user = os_paths::user_dir().map(|d| format!("{}{}", d.display(), std::path::MAIN_SEPARATOR)).unwrap_or_default();
            vec![
                g("Backup", vec![
                    pair(combo("Occasionally (every 10 minutes)", 336.0, "Autosave"), ValueR(crate::prefs::BACKUP_MAXIMUM.label, crate::prefs::number_text(&crate::prefs::BACKUP_MAXIMUM))),
                    combo("Save to the project data folder if there is one", 336.0, "Location"),
                ]),
                g("Plugins", vec![
                    Buttons(vec![("Manage plugins", 124.0)]),
                    Text(format!("{} instruments, {} effects", s.plugdb.lists.gens.len(), s.plugdb.lists.fx.len()), true),
                ]),
                g("User data folder", vec![txt(user)]),
                g(crate::factory_folder::GROUP, vec![FactoryFolder]),
                g("Browser extra search folders", vec![SearchFolders(crate::search_folders::shown_rows(s))]),
                g("External tools", vec![
                    dim("External tools", List(vec![("File", 30.0), ("Name", 320.0)], vec![], 4)),
                    dim("External tools", txt("Script editor   C:\\Windows\\System32\\Notepad.exe")),
                ]),
                gf("Troubleshooting", true, vec![Buttons(vec![("Repair optional features", 124.0), ("Reset browser cache", 124.0)])]),
            ]
        }
        PLUGINS => {
            let d = &s.plugdb;
            let p = &d.prefs;
            let folders = db::search_dirs(p).into_iter().map(|f| (f.path.display().to_string(), f.tag(), f.on, f.kind == db::DirKind::Added)).collect();
            let mut scanning = vec![
                Check(STARTUP_CHECK, p.startup_scan),
                Check(ERRORS_CHECK, p.rescan_errors),
                Check(COMBINE_CHECK, p.combine),
                Prefer(p.prefer),
            ];
            // The native host toggle appears only with `OPENSTUDIO_NATIVE_SETTINGS`, the second one only while the
            // native host is on too.
            if st.native {
                scanning.push(Check(NATIVE_HOST_CHECK, os_native::enabled()));
                if os_native::enabled() {
                    scanning.push(Check(NATIVE_FIRST_CHECK, os_native::prefer_native()));
                }
            }
            scanning.push(Buttons(vec![("Scan for new plugins", 130.0), ("Rescan all", 100.0)]));
            scanning.push(Text(d.status_line(), true));
            vec![
                g("Search folders", vec![Folders(folders), Buttons(vec![("Add folder...", 110.0)])]),
                g("Scanning", scanning),
                g("Plugins", vec![Table]),
            ]
        }
        THEME => {
            // Skins (shapes and base colours) and themes (`.flstheme` colours for either skin), spec:
            // ui/theme-presets.md "Settings".
            let skins = os_theme::Design::ALL.iter().map(|d| d.name().to_string()).collect();
            let themes = st.themes.iter().map(|(n, _)| n.clone()).collect();
            vec![
                g("Skins", vec![Tiles(TileList::Skins, skins, st.skin)]),
                g("Themes", vec![Tiles(TileList::Themes, themes, st.preset)]),
            ]
            .into_iter()
            .chain(st.unreadable.map(|_| g("", vec![ThemeWarn])))
            .chain([
                gf("Adjustments", false, vec![ThemeKnobs]),
                g("Options", vec![Check(NOTE_COLORS_CHECK, s.fl_theme().override_clips)]),
                g(
                    "",
                    vec![Buttons(vec![("Load theme...", 110.0), ("Reset", 64.0), (SAVE_PRESET, 100.0), (THEME_UNDO, 64.0), (THEME_REDO, 64.0)])],
                ),
            ])
            .collect()
        }
        PROJECT => vec![
            // The project's data folder (events 167 and 202) and two program settings (spec: ui/data-folder.md §1);
            // drawn by `super::data_folder`.
            g(super::data_folder::GROUP, vec![
                combo(crate::data_folder::choice_text(&s.project), 306.0, crate::data_folder::COMBO),
                DataFolder,
                combo(crate::prefs::choice(&crate::prefs::ASK_DATA_FOLDER), 444.0, crate::prefs::ASK_DATA_FOLDER.label),
                Check(crate::prefs::COPY_USED_SAMPLES.label, false),
            ]),
            g("Time settings", {
                // The bar as the project holds it (4/4 when the file has none), in the mode of event 35: a time
                // signature's two combos or a time division's two value boxes (spec: ui/settings-choices.md "Project").
                let (beats, steps) = s.project.bar();
                let sig = s.project.bar_is_signature();
                let mut rows = vec![
                    combo(&s.project.ppq.to_string(), 44.0, TIMEBASE),
                    combo(if sig { SET_AS_SIGNATURE } else { SET_AS_DIVISION }, 196.0, SET_AS),
                ];
                if sig {
                    rows.push(combo(&beats.to_string(), 44.0, NUMERATOR));
                    rows.push(combo(&denominator_text(steps), 44.0, DENOMINATOR));
                } else {
                    rows.push(Value(beats.to_string(), BEATS_PER_BAR));
                    rows.push(Value(steps.to_string(), STEPS_PER_BEAT));
                }
                rows
            }),
            // The panning law (event 23) and the check show what the engine plays. Fast declick shows the project's
            // value, greyed: its sound is not played yet (event 40; spec: flp/engine-events.md "Project").
            gf("Advanced", true, vec![
                combo(&panning_text(s.project.panning_law), 82.0, PANNING_LAW),
                Check(TRUNCATED_CHECK, s.project.play_truncated),
                Check("Fast fade for choke groups", s.project.fast_declick.unwrap_or(false)),
            ]),
            // The project's automatic crossfades (event 44; off when the file has none) and the program setting new
            // projects take it from (`prefs::AUTO_CROSSFADES_NEW`, which the check shows).
            gf("Arranger", false, vec![Check2(
                CROSSFADES_CHECK,
                s.project.auto_crossfades.unwrap_or(false),
                crate::prefs::AUTO_CROSSFADES_NEW.label,
                true,
            )]),
        ],
        // The credits are edited in place (spec: ui/settings.md "Info"; events 194 / 207 / 206 / 195 / 197,
        // flp/events/project-info.md); the genre button opens FL's genre list (spec: ui/settings-choices.md "Info").
        // "Show info on opening" is event 10; the footer and Reset working time are `crate::project_info`.
        INFO => {
            vec![g("Project credits", vec![
                Text("Title".into(), true),
                pair(Edit(InfoField::Title, 288.0), Check(SHOW_INFO_CHECK, s.project.show_info.unwrap_or(false))),
                Text("Author".into(), true),
                Edit(InfoField::Author, 0.0),
                Text("Genre".into(), true),
                Edit(InfoField::Genre, 288.0),
                Text("Comments".into(), true),
                Edit(InfoField::Comments, 0.0),
                Text("Web link".into(), true),
                Edit(InfoField::Url, 0.0),
                Gap(8.0),
                // FL's footer (spec: ui/settings-choices.md "Info").
                InfoFooter(crate::project_info::footer(s, std::time::Instant::now())),
            ])]
        }
        DEBUG => {
            let mut lines = vec![
                format!("Lemon Studio {}", env!("CARGO_PKG_VERSION")),
                format!("Project: {}", s.path.as_ref().map_or("(new project)".into(), |p| p.display().to_string())),
                format!("Saved by: {}", s.project.saved_by),
                format!("PPQ {}, tempo {:.3}", s.project.ppq, s.tempo),
                format!("Channels {}, patterns {}, mixer tracks {}", s.project.channels.len(), s.project.patterns.len(), s.project.mixer.len()),
                format!("Output device: {}", st.device),
                format!("Device rate: {:?}", st.rate),
                format!("Instruments listed: {}, effects listed: {}", s.plugdb.lists.gens.len(), s.plugdb.lists.fx.len()),
            ];
            lines.extend(s.plugdb.lists.gens.iter().map(|e| format!("  {} ({})", e.label, db::format_label(e.format))));
            if let Some(m) = &s.message {
                lines.push(format!("Last message: {m}"));
            }
            vec![g("", vec![List(vec![("", 0.0)], lines.into_iter().map(|l| vec![l]).collect(), 40)])]
        }
        // About also carries what FL's Account tab had (spec: ui/plugin-manager.md §2).
        // The Windows build includes the ASIO SDK, which is under the GPL version 3 only, and names ASIO, so it carries
        // the ASIO logo and Steinberg's notice (THIRD-PARTY-NOTICES.txt §9).
        _ => vec![
            g("", [
                vec![
                    Gap(20.0),
                    Big(format!("Lemon Studio {}", env!("CARGO_PKG_VERSION"))),
                    txt("A digital audio workstation written in Rust."),
                    txt("Copyright (C) 2026 ItsAlphaNeon and the Lemon Studio contributors."),
                    Gap(10.0),
                ],
                if cfg!(windows) {
                    vec![txt("Licence: GNU General Public License, version 3 (the ASIO SDK it includes is version 3 only).")]
                } else {
                    vec![txt("Licence: GNU General Public License, version 3 or later.")]
                },
                vec![txt("Fonts: Cuprum (SIL Open Font License). Icons: Tabler Icons (MIT).")],
                if cfg!(windows) {
                    vec![txt("ASIO® output: the ASIO SDK by Steinberg Media Technologies GmbH (GPL version 3)."), Row::AsioLogo]
                } else {
                    Vec::new()
                },
            ]
            .into_iter()
            .flatten()
            .collect()),
            g("Licence", vec![
                Big("Lemon Studio needs no account.".into()),
                txt("It is free software: you can redistribute it and/or modify it under the terms"),
                txt("of the GNU General Public License, version 3 or (at your option) any later version."),
                txt("It comes with ABSOLUTELY NO WARRANTY. Licence: https://www.gnu.org/licenses/"),
                txt("No registration, no online services, no usage data."),
            ]),
        ],
    }
}

fn row_h(r: &Row) -> f32 {
    match r {
        Row::Check(..) | Row::Check2(..) | Row::Off(..) => 21.0,
        Row::Combo(..) | Row::Value(..) | Row::ValueR(..) => 27.0,
        Row::Buttons(..) => 36.0,
        Row::Slider(..) | Row::Ppi => 28.0,
        Row::List(c, _, n) => (if c.iter().any(|c| !c.0.is_empty()) { 18.0 } else { 0.0 }) + 17.0 * *n as f32 + 10.0,
        Row::Text(_, dim) => if *dim { 16.0 } else { 20.0 },
        Row::Big(_) => 28.0,
        Row::Tiles(_, t, _) => t.len().div_ceil(TILES_PER_ROW) as f32 * TILE_PITCH_Y + 4.0,
        Row::ThemeKnobs => 140.0,
        Row::ThemeWarn => 36.0,
        Row::Gap(h) => *h,
        Row::Pair(a, b) => row_h(a).max(row_h(b)),
        Row::Dim(_, r) => row_h(r),
        Row::Folders(f) => 21.0 * f.len().max(1) as f32 + 10.0,
        Row::SearchFolders(n) => 18.0 + SF_ROW_H * *n as f32 + 10.0,
        Row::Prefer(_) => 27.0,
        Row::Table => 4.0 + 18.0 + TABLE_ROW_H * TABLE_ROWS as f32 + 4.0 + 34.0,
        Row::Edit(f, _) => if f.multiline() { COMMENT_LINE_H * COMMENT_LINES as f32 + 12.0 } else { 28.0 },
        Row::InfoFooter(lines) => (lines.len() as f32 * FOOTER_LINE_H + 8.0).max(36.0),
        Row::DataFolder => super::data_folder::FOLDER_ROW_H,
        Row::FactoryFolder => super::factory_folder::ROW_H,
        Row::AsioLogo => ASIO_LOGO_H + 4.0,
        Row::Midi(m) => m.height(),
    }
}

fn group_h(gr: &Group, open: bool) -> f32 {
    let head = if gr.title.is_empty() { 8.0 } else { 30.0 };
    if !open {
        return head;
    }
    head + gr.rows.iter().map(row_h).sum::<f32>() + 8.0
}

fn group_open(st: &SettingsUi, gr: &Group) -> bool {
    !gr.fold || (!gr.closed ^ st.folded.contains(gr.title))
}

/// Total content height of the current page (for scrolling).
pub fn content_h(s: &State) -> f32 {
    page(s.settings.tab, s).iter().map(|gr| group_h(gr, group_open(&s.settings, gr)) + 6.0).sum::<f32>() + 16.0
}

/// Visible page height.
pub fn page_h(s: &State) -> f32 {
    rect(s).h - TABS_B - 4.0
}

pub fn paint(ui: &mut Ui, s: &State) {
    let st = &s.settings;
    let r = rect(s);
    let t = ui.t;
    ui.hit(r, Action::Settings(SAct::Body), "");
    // Shadow, frame, title bar.
    let lemon = super::lemon(ui);
    if lemon {
        // Lemon (spec: ui/lemon-design.md §10): rounded window, pill tabs, rounded groups and square checks.
        ui.c.fill_rrect(r.outset(4.0), lk::r_panel(t) + 4.0, Color::rgb(0).with_alpha(50));
        lk::outlined(ui.c, r, lk::r_panel(t), t.s(0x1c1e21), t.s(0x2b2d30));
        ui.c.fill_rrect(Rect::new(r.x + 1.0, r.y + 1.0, r.w - 2.0, TABS_B - 1.0), lk::r_panel(t) - 1.0, t.s(0x25272a));
        ui.c.fill_rect(Rect::new(r.x + 1.0, r.y + TABS_B - lk::r_panel(t), r.w - 2.0, lk::r_panel(t)), t.s(0x25272a));
    } else {
        ui.c.fill_rrect(r.outset(4.0), 6.0, Color::rgb(0).with_alpha(50));
        ui.c.fill_rect(r, t.s(0x1c1e21));
        ui.c.fill_rect(r.inset(1.0), t.s(0x2b2d30));
        ui.c.fill_rect(Rect::new(r.x + 1.0, r.y + 1.0, r.w - 2.0, TABS_B - 1.0), t.s(0x25272a));
    }
    ui.c.icon(&os_ui::tabler::SETTINGS, Rect::new(r.x + 7.0, r.y + 7.0, 16.0, 16.0), t.text_dim);
    let (_, title, _) = TABS[st.tab.min(TABS.len() - 1)];
    ui.c.text(Font::new(13), r.x + 28.0, r.y + 7.0, t.text, &format!("Settings - {title}"));
    let x = Rect::new(r.right() - 26.0, r.y + 5.0, 20.0, 20.0);
    let hov = ui.hit(x, Action::Settings(SAct::Close), "Close");
    ui.c.icon(&os_ui::tabler::X, x.inset(3.0), if hov { t.glyph } else { t.text });
    // Tabs (spec §2): active text coloured by group.
    let font = Font::new(13);
    let rects = tab_rects(ui, r);
    for (i, &(label, _, grp)) in TABS.iter().enumerate() {
        let tr = rects[i];
        let hov = ui.hit(tr, Action::Settings(SAct::Tab(i)), TABS[i].1);
        let on = i == st.tab;
        let body = if on { t.s(0x1d1e1f) } else { t.s(0x222425) };
        if lemon {
            let pill = Rect::new(tr.x, tr.y + 2.0, tr.w, tr.h - 6.0);
            if on {
                lk::lit_in(ui.c, ui.t, pill, lk::r_btn(t), hov);
            } else {
                ui.c.fill_rrect(pill, lk::r_btn(t), if hov { t.button_hi } else { body });
            }
        } else {
            ui.c.fill_rect(tr, body);
            for k in 0..7 {
                // slanted right edge
                let k = k as f32;
                ui.c.fill_rect(Rect::new(tr.right() + k, tr.y + 2.0 + k * 4.0, 1.0, tr.h - 2.0 - k * 4.0), body);
            }
        }
        let col = if on && lemon {
            lk::ink_on_lit(ui.t)
        } else if on {
            match grp {
                0 => t.a(0x2da2ff),
                1 => t.h(0xfdc13d),
                _ => t.a(0xbeff3c),
            }
        } else if hov {
            t.glyph
        } else {
            t.text
        };
        let lh = ui.c.line_height(font);
        let ty = if lemon { tr.cy() - 2.0 } else { tr.cy() };
        ui.c.text(font, tr.x + 10.0, (ty - lh / 2.0).round(), col, label);
    }
    ui.c.fill_rect(Rect::new(r.x + 1.0, r.y + TABS_B, r.w - 2.0, 1.0), t.s(0x1c1e21));
    // Page.
    let area = Rect::new(r.x + 1.0, r.y + TABS_B + 1.0, r.w - 2.0, r.h - TABS_B - 3.0);
    let total = content_h(s);
    let scroll = st.scroll.min((total - area.h).max(0.0)).max(0.0);
    ui.c.push_clip(area);
    let mut y = area.y + 7.0 - scroll;
    for gr in page(st.tab, s) {
        let open = group_open(st, &gr);
        let h = group_h(&gr, open);
        if y + h > area.y && y < area.bottom() {
            paint_group(ui, s, &gr, Rect::new(r.x + GX, y, GW, h), open, area);
        }
        y += h + 6.0;
    }
    ui.c.pop_clip();
    if total > area.h {
        let track = Rect::new(area.right() - 10.0, area.y + 4.0, 7.0, area.h - 8.0);
        super::scrollbar(ui, track, true, scroll, area.h, total, crate::app::Scroll::Settings);
    }
    if let Some(p) = st.picker.as_ref().filter(|_| st.tab == THEME) {
        picker(ui, s, p, r);
    }
    if let Some(e) = st.name_edit.as_ref().filter(|_| st.tab == THEME) {
        name_panel(ui, s, e, r);
    }
    if let Some(e) = s.midi.curve.as_ref().filter(|_| st.tab == MIDI) {
        super::midi_page::paint_curve(ui, e, r);
    }
}

/// The name panel over the Theme page (`crate::theme_library`): a title, the name box (not for Delete), OK and Cancel.
fn name_panel(ui: &mut Ui, s: &State, e: &crate::theme_library::NameEdit, win: Rect) {
    let t = ui.t;
    let (panel, field, ok, cancel) = crate::theme_library::name_rects(e, win);
    let lemon = super::lemon(ui);
    let rad = if lemon { lk::r_panel(t) } else { 0.0 };
    ui.hit(panel, Action::Settings(SAct::NameBody), "");
    ui.c.fill_rrect(panel.outset(3.0), rad + 3.0, Color::rgb(0).with_alpha(70));
    ui.c.fill_rrect(panel, rad, t.s(0x1c1e21));
    ui.c.fill_rrect(panel.inset(1.0), (rad - 1.0).max(0.0), t.s(0x2b2d30));
    ui.c.text_clip(Font::new(13).bold(), panel.x + 10.0, panel.y + 9.0, panel.w - 20.0, t.text, &e.target.title(s));
    if e.target.typed() {
        kit::sunken(ui.c, t, field, t.s(0x141516));
        let f = LABEL;
        let lh = ui.c.line_height(f);
        let text = e.text.text.as_str();
        let inner = Rect::new(field.x + 6.0, field.y + 2.0, field.w - 12.0, field.h - 4.0);
        let caret_x = ui.c.text_width(f, &text[..byte_at(text, e.text.caret)]);
        let x = inner.x - (caret_x - (inner.w - 2.0)).max(0.0);
        let y = (field.cy() - lh / 2.0).round();
        ui.c.push_clip(inner);
        if e.text.all && !text.is_empty() {
            let sel = if lemon { t.brand_sel(os_theme::design::lemon::COLORED_DARK) } else { t.a(0x2d5a8c) };
            let tw = ui.c.text_width(f, text);
            ui.c.fill_rect(Rect::new(x - 1.0, y, tw + 2.0, lh), sel);
        } else {
            ui.c.fill_rect(Rect::new((x + caret_x).round(), y, 1.0, lh), t.text);
        }
        ui.c.text(f, x, y, t.text, text);
        ui.c.pop_clip();
    } else {
        ui.c.text_clip(Font::new(13), field.x, field.y + 4.0, field.w, t.text_dim, "The file goes from your themes folder.");
    }
    for (b, act, label, hint) in [
        (ok, SAct::NameOk, e.target.ok(), "Enter"),
        (cancel, SAct::NameCancel, "Cancel", "Esc"),
    ] {
        let hov = ui.hit(b, Action::Settings(act), hint);
        if lemon {
            lk::outlined(ui.c, b, lk::r_btn(t), t.s(0x161819), lk::face(t.s(0x2a2d2f), kit::State::hover(hov)));
        } else {
            kit::bordered(ui.c, b, 2.0, t.s(0x161819), kit::face(t.s(0x212425), kit::State::hover(hov)));
        }
        ui.c.text_center(Font::new(13), b, if hov { t.glyph } else { t.text }, label);
    }
}

/// A circle of radius `r` split into equal slices of `colours`, the first from the bottom round by the left.
fn pie(c: &mut os_ui::Canvas, cx: f32, cy: f32, r: f32, colours: &[Color]) {
    let n = colours.len();
    if n <= 1 {
        return c.fill_circle(cx, cy, r, colours.first().copied().unwrap_or(Color::rgb(0)));
    }
    let tau = std::f32::consts::TAU;
    for (k, col) in colours.iter().enumerate() {
        let a0 = tau / 4.0 + k as f32 * tau / n as f32;
        let steps = (48 / n).max(4);
        let mut pts = vec![(cx, cy)];
        pts.extend((0..=steps).map(|j| {
            let a = a0 + j as f32 / steps as f32 * tau / n as f32;
            (cx + r * a.cos(), cy + r * a.sin())
        }));
        c.fill_polygon(&pts, *col);
    }
}

/// Theme ▸ Adjustments' colour picker (`crate::theme_adjust`), over the page: a saturation / brightness square, a hue
/// strip, the new colour over the old one with its hex value, Revert, and chips for circles with several colours.
fn picker(ui: &mut Ui, s: &State, p: &crate::theme_adjust::Picker, win: Rect) {
    use crate::theme_adjust::{self as adj, from_hsv, SWATCHES};
    let t = ui.t;
    let text = adj::applied_text(s, t.style.design);
    let parts = adj::parts(&text, p.swatch);
    let rr = adj::picker_rects(p, parts.len(), win);
    let lemon = super::lemon(ui);
    let rad = if lemon { lk::r_panel(t) } else { 0.0 };
    ui.hit(rr.panel, Action::Settings(SAct::PickBody), "");
    ui.c.fill_rrect(rr.panel.outset(3.0), rad + 3.0, Color::rgb(0).with_alpha(70));
    ui.c.fill_rrect(rr.panel, rad, t.s(0x1c1e21));
    ui.c.fill_rrect(rr.panel.inset(1.0), (rad - 1.0).max(0.0), t.s(0x2b2d30));
    let title = match parts.get(p.part) {
        Some(part) if parts.len() > 1 => format!("{} colour {}", SWATCHES[p.swatch], part.label.to_lowercase()),
        _ => format!("{} colour", SWATCHES[p.swatch]),
    };
    ui.c.text(Font::new(13).bold(), rr.panel.x + 10.0, rr.panel.y + 9.0, t.text, &title);
    let hov = ui.hit(rr.close, Action::Settings(SAct::PickClose), "Close (Esc)");
    ui.c.icon(&os_ui::tabler::X, rr.close.inset(3.0), if hov { t.glyph } else { t.text });
    for (k, (b, part)) in rr.chips.iter().zip(&parts).enumerate() {
        let hov = ui.hit(*b, Action::Settings(SAct::PickPart(k as u8)), "Pick this colour");
        let frame = if k == p.part {
            if lemon { t.brand_sel(os_theme::design::lemon::MAIN) } else { t.a(0x2d7cd0) }
        } else if hov {
            t.s(0x8a8c8e)
        } else {
            t.s(0x161819)
        };
        ui.c.fill_rrect(*b, 4.0, frame);
        ui.c.fill_rrect(b.inset(2.0), 3.0, part.colour);
        let ink = if part.colour.hsl().2 > 0.55 { Color::rgb(0x111111) } else { Color::rgb(0xf2f2f2) };
        ui.c.text_center(Font::new(12), *b, ink, &part.label);
    }
    // Saturation left to right, brightness bottom to top, at the picked hue; a ring marks the colour.
    let (w, h) = (48usize, 36usize);
    let sv: Vec<u32> = (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .map(|(x, y)| {
            let c = from_hsv([p.hsv[0], x as f32 / (w - 1) as f32, 1.0 - y as f32 / (h - 1) as f32]);
            u32::from(c.r) << 16 | u32::from(c.g) << 8 | u32::from(c.b)
        })
        .collect();
    ui.hit(rr.sv, Action::Settings(SAct::PickSv), "Saturation (left to right) and brightness (bottom to top)");
    ui.c.image(rr.sv, w, h, &sv);
    let (mx, my) = (rr.sv.x + p.hsv[1] * rr.sv.w, rr.sv.y + (1.0 - p.hsv[2]) * rr.sv.h);
    ui.c.stroke_circle(mx, my, 5.0, 1.5, Color::rgb(0));
    ui.c.stroke_circle(mx, my, 4.0, 1.5, Color::rgb(0xffffff));
    let hue: Vec<u32> = (0..64)
        .map(|y| {
            let c = from_hsv([y as f32 / 63.0 * 360.0, 1.0, 1.0]);
            u32::from(c.r) << 16 | u32::from(c.g) << 8 | u32::from(c.b)
        })
        .collect();
    ui.hit(rr.hue, Action::Settings(SAct::PickHue), "Hue");
    ui.c.image(rr.hue, 1, 64, &hue);
    let hy = (rr.hue.y + p.hsv[0] / 360.0 * rr.hue.h).round();
    ui.c.fill_rect(Rect::new(rr.hue.x - 2.0, hy - 2.0, rr.hue.w + 4.0, 4.0), Color::rgb(0));
    ui.c.fill_rect(Rect::new(rr.hue.x - 1.0, hy - 1.0, rr.hue.w + 2.0, 2.0), Color::rgb(0xffffff));
    // New colour over the old one (a click goes back to it), the hex value, Revert.
    let new = from_hsv(p.hsv);
    let before = p.before.clone().unwrap_or_else(|| os_theme::design::default_text(t.style.design));
    let old = adj::parts(&before, p.swatch).get(p.part).map_or(new, |q| q.colour);
    ui.c.fill_rect(rr.new, new);
    ui.hit(rr.old, Action::Settings(SAct::PickRevert), "The colour before: click to go back to it");
    ui.c.fill_rect(rr.old, old);
    ui.c.stroke_rrect(Rect::new(rr.new.x, rr.new.y, rr.new.w, rr.old.bottom() - rr.new.y), 0.0, 1.0, t.s(0x161819));
    let hex = format!("#{:02x}{:02x}{:02x}", new.r, new.g, new.b);
    ui.c.text_center(Font::new(13), Rect::new(rr.new.x, rr.old.bottom() + 4.0, rr.new.w, 20.0), t.text, &hex);
    let b = rr.revert;
    let hov = ui.hit(b, Action::Settings(SAct::PickRevert), "Back to the colours from before the picker opened");
    if lemon {
        lk::outlined(ui.c, b, lk::r_btn(t), t.s(0x161819), lk::face(t.s(0x2a2d2f), kit::State::hover(hov)));
    } else {
        kit::bordered(ui.c, b, 2.0, t.s(0x161819), kit::face(t.s(0x212425), kit::State::hover(hov)));
    }
    ui.c.text_center(Font::new(13), b, if hov { t.glyph } else { t.text }, "Revert");
}

/// Tab rects in window `r`: text width + 10 px each side, 11 px apart, 15 px more between groups. When that is wider
/// than the window (eleven tabs since Recording), the sides narrow first, to 4 px, then the gaps, to 8 px (Citrus's
/// slanted edges need 7), so About stays inside.
fn tab_rects(ui: &mut Ui, r: Rect) -> Vec<Rect> {
    let font = Font::new(13);
    let texts: Vec<f32> = TABS.iter().map(|t| ui.c.text_width(font, t.0)).collect();
    let n = TABS.len() as f32;
    let groups = TABS.windows(2).filter(|w| w[0].2 != w[1].2).count() as f32;
    // 3 px margins, and room for the last tab's slanted edge.
    let room = r.w - 13.0 - texts.iter().sum::<f32>() - 15.0 * groups;
    let (side, gap) = {
        let over = (2.0 * 10.0 * n + 11.0 * (n - 1.0) - room).max(0.0);
        let side = (10.0 - over / (2.0 * n)).max(4.0);
        let over = (2.0 * side * n + 11.0 * (n - 1.0) - room).max(0.0);
        (side, (11.0 - over / (n - 1.0)).max(8.0))
    };
    let mut x = r.x + 3.0;
    let mut v = Vec::new();
    for (i, &(_, _, grp)) in TABS.iter().enumerate() {
        if i > 0 && TABS[i - 1].2 != grp {
            x += 15.0;
        }
        let w = texts[i] + 2.0 * side;
        v.push(Rect::new(x.round(), r.y + 34.0, w.round(), TABS_B - 34.0));
        x += w + gap;
    }
    v
}

fn paint_group(ui: &mut Ui, s: &State, gr: &Group, r: Rect, open: bool, clip: Rect) {
    let t = ui.t;
    let lemon = super::lemon(ui);
    if lemon {
        lk::outlined(ui.c, r, lk::r_panel(t) - 2.0, t.s(0x1c1e21), t.s(0x25272a));
    } else {
        ui.c.fill_rect(r, t.s(0x1c1e21));
        ui.c.fill_rect(r.inset(1.0), t.s(0x25272a));
    }
    let mut y = r.y + 8.0;
    if !gr.title.is_empty() {
        let f = Font::new(13).bold();
        let tw = ui.c.text_width(f, gr.title);
        let hdr = Rect::new(r.x + 1.0, r.y + 1.0, tw + 14.0, 24.0);
        if !lemon {
            ui.c.fill_rect(hdr, t.s(0x222427));
            for k in 0..6 {
                let k = k as f32;
                ui.c.fill_rect(Rect::new(hdr.right() + k, hdr.y, 1.0, hdr.h - k * 4.0), t.s(0x222427));
            }
        }
        ui.c.text(f, r.x + if lemon { 8.0 } else { 4.0 }, r.y + 4.0, if lemon { t.accent } else { t.text }, gr.title);
        if !gr.note.is_empty() {
            ui.c.text(Font::new(13), r.x + if lemon { 8.0 } else { 4.0 } + tw, r.y + 4.0, t.text_dim, gr.note);
        }
        if gr.fold {
            let bar = Rect::new(r.x, r.y, r.w, 26.0);
            if clip.contains(bar.cx(), bar.cy()) {
                ui.hit(bar, Action::Settings(SAct::Group(gr.title)), if open { "Fold" } else { "Unfold" });
            }
            kit::chevron(ui.c, r.right() - 10.0, r.y + 13.0, 4.0, if open { Dir::Up } else { Dir::Down }, t.text_dim);
        }
        y = r.y + 30.0;
    }
    if !open {
        return;
    }
    for row in &gr.rows {
        let h = row_h(row);
        if y + h > clip.y && y < clip.bottom() {
            paint_row(ui, s, gr.title, row, r, y, h);
        }
        y += h;
    }
}

/// A check showing `on`; one that is not `enabled` (not in `LIVE`) is greyed out.
fn check(ui: &mut Ui, key: &'static str, on: bool, x: f32, cy: f32, enabled: bool) {
    // A check declared in `crate::prefs` shows its state (the row's is its default).
    let on = crate::prefs::find_check(key).map_or(on, crate::prefs::check);
    let t = ui.t;
    let tw = ui.c.text_width(Font::new(13), key);
    let hr = Rect::new(x - 8.0, cy - 9.0, tw + 26.0, 18.0);
    let hov = enabled && ui.hit(hr, Action::Settings(SAct::Check(key)), "");
    if !enabled {
        ui.off(hr, key);
    }
    led(ui, x, cy, on, hov);
    let col = if hov { t.glyph } else { t.text };
    let lh = ui.c.line_height(Font::new(13));
    ui.c.text(Font::new(13), x + 11.0, (cy - lh / 2.0).round(), col, key);
    if !enabled {
        grey(ui, hr);
    }
}

/// A declared check that can't be changed now, for a reason other than "not available yet": it shows its state
/// greyed out, and its hit only shows `hint` (a click and the wheel act as on the page's body).
fn check_greyed(ui: &mut Ui, key: &'static str, x: f32, cy: f32, hint: &'static str) {
    let on = crate::prefs::find_check(key).is_some_and(crate::prefs::check);
    let t = ui.t;
    let tw = ui.c.text_width(Font::new(13), key);
    let hr = Rect::new(x - 8.0, cy - 9.0, tw + 26.0, 18.0);
    ui.hit(hr, Action::Settings(SAct::Body), hint);
    led(ui, x, cy, on, false);
    let lh = ui.c.line_height(Font::new(13));
    ui.c.text(Font::new(13), x + 11.0, (cy - lh / 2.0).round(), t.text, key);
    grey(ui, hr);
}

/// Whether General ▸ Display's Thick lines (`crate::prefs::THICK_LINES`) can be changed on a canvas at `scale` (the
/// effective UI scale): from 200 % (spec: ui/settings-choices.md "Display: PPI ruler, Force refreshes, Thick lines,
/// High visibility (2026-10-16)").
pub fn thick_lines_live(scale: f32) -> bool {
    (scale * 100.0).round() >= 200.0
}

/// The check mark itself at (x, cy).
pub(super) fn led(ui: &mut Ui, x: f32, cy: f32, on: bool, hov: bool) {
    let t = ui.t;
    // Round LED (spec §3): orange when on. Lemon: a rounded square check, lit with a tick when on.
    let lemon = super::lemon(ui);
    if !lemon {
        ui.c.fill_circle(x, cy, 6.5, t.s(0x141516));
    }
    if lemon {
        let b = Rect::new(x - 6.5, cy - 6.5, 13.0, 13.0);
        if on {
            lk::lit_in(ui.c, ui.t, b, lk::r_small(t), hov);
            ui.c.polyline(&[(x - 3.5, cy), (x - 1.0, cy + 2.5), (x + 3.5, cy - 2.5)], 1.6, lk::ink_on_lit(ui.t));
        } else {
            lk::outlined(ui.c, b, lk::r_small(t), t.s(0x141516), if hov { t.s(0x4a4c4e) } else { t.s(0x36393a) });
        }
    } else if on {
        ui.c.fill_circle(x, cy, 5.5, t.h(0x952b15));
        ui.c.fill_circle(x, cy, 4.2, t.h(0xf09672));
        ui.c.fill_circle(x - 1.0, cy - 1.2, 1.6, t.h(0xffd2bc));
    } else {
        ui.c.fill_circle(x, cy, 5.5, if hov { t.s(0x6a6c6e) } else { t.s(0x58595a) });
        ui.c.fill_circle(x, cy, 4.2, t.s(0x36393a));
    }
}

/// Greys out what was just drawn in `r` on a group body.
pub(super) fn grey(ui: &mut Ui, r: Rect) {
    let bg = ui.t.s(0x25272a);
    ui.veil(r, bg);
}

/// The hint name of a control: its label, else its group's title.
fn name_of(label: &'static str, group: &'static str) -> &'static str {
    if !label.is_empty() {
        label
    } else if !group.is_empty() {
        group
    } else {
        "This option"
    }
}

fn paint_row(ui: &mut Ui, s: &State, group: &'static str, row: &Row, g: Rect, y: f32, h: f32) {
    paint_row_at(ui, s, group, row, g, y, h, 0.0)
}

#[allow(clippy::too_many_arguments)]
fn paint_row_at(ui: &mut Ui, s: &State, group: &'static str, row: &Row, g: Rect, y: f32, h: f32, dx: f32) {
    let t = ui.t;
    let f = LABEL;
    let x0 = g.x + 12.0 + dx;
    let cy = y + h / 2.0;
    let lh = ui.c.line_height(f);
    let ty = (cy - lh / 2.0).round();
    match row {
        // Project ▸ Data folder's rows.
        _ if super::data_folder::shows(group) => super::data_folder::paint_row(ui, s, row, g, x0, y, h),
        // General ▸ Display's Thick lines works from 200 % scaling: below, greyed out, it says so.
        Row::Check(k, _) if *k == crate::prefs::THICK_LINES.label && !thick_lines_live(ui.c.scale) => {
            check_greyed(ui, k, x0 + 10.0, cy, "Thick lines: from 200 % scaling")
        }
        // General ▸ System's association: the system's own "Open with" settings decide on macOS and Linux (spec:
        // ui/settings-choices.md "System: Associate project files with application").
        #[cfg(not(windows))]
        Row::Check(k, _) if *k == ASSOCIATE_CHECK => check_greyed(ui, k, x0 + 10.0, cy, ASSOCIATE_ELSEWHERE),
        // A check that is not available yet shows its default, greyed out.
        Row::Check(k, d) => check(ui, k, *d, x0 + 10.0, cy, live(k)),
        Row::Off(k, d) => check(ui, k, *d, x0 + 10.0, cy, false),
        Row::Check2(a, da, b, db) => {
            check(ui, a, *da, g.x + 22.0, cy, live(a));
            check(ui, b, *db, g.x + 324.0, cy, live(b));
        }
        // The Audio page's device, rate and buffer combos and General's Animations open their menus.
        // An ASIO driver has no Windows mode: the Mode combo shows the one kept, greyed, and its hint says why.
        Row::Combo(v, w, label) if *label == MODE && s.settings.audio.asio_driver().is_some() => {
            let b = Rect::new(x0, y + 3.0, *w, 21.0);
            ui.hit(b, Action::Settings(SAct::Body), MODE_ASIO);
            kit::combo(ui.c, t, b, v, f, kit::State::NORMAL);
            let lw = ui.c.text(f, b.right() + 8.0, ty, t.text, label);
            grey(ui, Rect::new(b.x, b.y, b.w + 8.0 + lw, b.h));
        }
        Row::Combo(v, w, label) if live_combo(label) || crate::prefs::find_combo(label).is_some() => {
            // A combo declared in `crate::prefs` shows its choice.
            let v = match crate::prefs::find_combo(label) {
                Some(c) => crate::prefs::choice(c),
                None => v,
            };
            let b = Rect::new(x0, y + 3.0, *w, 21.0);
            let hov = ui.hit(b, Action::Settings(SAct::Combo(label, b.x, b.bottom())), "");
            kit::combo(ui.c, t, b, v, f, kit::State::hover(hov));
            // Project's "Set as" combo has no label beside it.
            if *label != SET_AS {
                ui.c.text(f, b.right() + 8.0, ty, if hov { t.glyph } else { t.text }, label);
            }
        }
        // Other combos and value boxes do not work yet. A combo keeps showing its value, greyed out.
        Row::Combo(v, w, label) => {
            let b = Rect::new(x0, y + 3.0, *w, 21.0);
            ui.off(b, name_of(label, group));
            kit::combo(ui.c, t, b, v, f, kit::State::NORMAL);
            let lw = ui.c.text(f, b.right() + 8.0, ty, t.text, label);
            grey(ui, Rect::new(b.x, b.y, b.w + 8.0 + lw, b.h));
        }
        // Project ▸ Time settings as a time division, General ▸ Undo history ▸ Maximum undo levels: the wheel changes
        // the value (`App::settings_number`).
        Row::Value(v, label) if number_range(label).is_some() => {
            let b = Rect::new(x0, y + 3.0, 38.0, 21.0);
            let hov = ui.hit(b, Action::Settings(SAct::Number(label)), "");
            value_red(ui, b, v);
            ui.c.text(f, b.right() + 8.0, ty, if hov { t.glyph } else { t.text }, label);
        }
        Row::Value(v, label) => {
            let b = Rect::new(x0, y + 3.0, 38.0, 21.0);
            ui.off(b, name_of(label, group));
            value_red(ui, b, v);
            let lw = ui.c.text(f, b.right() + 8.0, ty, t.text, label);
            grey(ui, Rect::new(b.x, b.y, b.w + 8.0 + lw, b.h));
        }
        Row::ValueR(label, v) if number_range(label).is_some() => {
            let b = Rect::new(g.right() - 52.0, y + 3.0, 42.0, 21.0);
            let hov = ui.hit(b, Action::Settings(SAct::Number(label)), "");
            value_red(ui, b, v);
            ui.c.text_right(f, b.x - 8.0, ty, if hov { t.glyph } else { t.text }, label);
        }
        Row::ValueR(label, v) => {
            let b = Rect::new(g.right() - 52.0, y + 3.0, 42.0, 21.0);
            ui.off(b, name_of(label, group));
            value_red(ui, b, v);
            ui.c.text_right(f, b.x - 8.0, ty, t.text, label);
            let lw = ui.c.text_width(f, label);
            grey(ui, Rect::span(b.x - 8.0 - lw, b.right(), b.y, b.bottom()));
        }
        Row::Buttons(bs) => {
            let mut x = x0;
            for &(label, w) in bs {
                let b = Rect::new(x, y + 4.0, w, 28.0);
                // Show ASIO panel works while an ASIO driver is open.
                let waits = label == SHOW_ASIO_PANEL && !s.settings.asio_open;
                let on = live(label) && !waits;
                let hov = on && ui.hit(b, Action::Settings(SAct::Button(label)), "");
                if waits {
                    ui.hit(b, Action::Settings(SAct::Body), PANEL_WAITS);
                } else if label == UPDATE_SCRIPTS {
                    ui.hit(b, Action::Settings(SAct::Body), NO_SCRIPTS);
                } else if !on {
                    ui.off(b, label);
                }
                if super::lemon(ui) {
                    lk::outlined(ui.c, b, lk::r_btn(t), t.s(0x161819), lk::face(t.s(0x2a2d2f), kit::State::hover(hov)));
                } else {
                    kit::bordered(ui.c, b, 2.0, t.s(0x161819), kit::face(t.s(0x212425), kit::State::hover(hov)));
                }
                ui.c.text_center(Font::new(13), b, if hov { t.glyph } else { t.text }, label);
                if !on {
                    grey(ui, b);
                }
                x += w + 8.0;
            }
        }
        // A slider declared in `crate::prefs` (General ▸ Display's Smoothing, Input's scrolling speeds): its handle is
        // at its value (no number shows); a click or a drag sets it (`App::pref_slider_press`), the wheel steps it.
        Row::Slider(label, _) if crate::prefs::find_slider(label).is_some() => {
            let Some(sl) = crate::prefs::find_slider(label) else { return };
            let v = f32::from(crate::prefs::slider(sl)) / f32::from(sl.max.max(1));
            let tr = Rect::new(x0, cy - 2.0, 150.0, 4.0);
            let hov = ui.hit(Rect::span(tr.x - 9.0, tr.right() + 9.0, cy - 9.0, cy + 9.0), Action::Settings(SAct::Slider(label, tr.x, tr.w)), "");
            slider_track(ui, tr, cy, v);
            ui.c.text(f, tr.right() + 12.0, ty, if hov { t.glyph } else { t.text }, label);
        }
        Row::Slider(label, v) => {
            let tr = Rect::new(x0, cy - 2.0, 150.0, 4.0);
            slider_track(ui, tr, cy, *v);
            let lw = ui.c.text(f, tr.right() + 12.0, ty, t.text, label);
            let r = Rect::span(tr.x - 9.0, tr.right() + 14.0 + lw, cy - 9.0, cy + 9.0);
            ui.off(r, name_of(label, group));
            grey(ui, r);
        }
        Row::List(cols, rows, n) => {
            let b = Rect::new(x0 - 4.0, y + 2.0, g.w - 16.0, h - 6.0);
            if super::lemon(ui) {
                lk::outlined(ui.c, b, lk::r_btn(t), t.s(0x161819), t.s(0x222527));
            } else {
                ui.c.fill_rect(b, t.s(0x161819));
                ui.c.fill_rect(b.inset(1.0), t.s(0x222527));
            }
            let mut ry = b.y + 2.0;
            if cols.iter().any(|c| !c.0.is_empty()) {
                for (c, cx) in cols {
                    ui.c.text(Font::new(13), b.x + 6.0 + cx, ry, t.text_dim, c);
                }
                ry += 18.0;
            }
            for (i, cells) in rows.iter().take(*n).enumerate() {
                let rr = Rect::new(b.x + 2.0, ry + i as f32 * 17.0, b.w - 4.0, 17.0);
                if i % 2 == 1 {
                    ui.c.fill_rect(rr, t.s(0x26292b));
                }
                for (c, (_, cx)) in cells.iter().zip(cols.iter()) {
                    ui.c.text_clip(Font::new(13), rr.x + 4.0 + cx, rr.y + 1.0, rr.w - cx - 8.0, t.text, c);
                }
            }
        }
        Row::Text(v, dim) => {
            ui.c.text(if *dim { Font::new(13) } else { f }, x0, ty, if *dim { t.text_dim } else { t.text }, v);
        }
        Row::Big(v) => {
            let b = Rect::new(x0 - 4.0, y + 2.0, g.w - 16.0, 24.0);
            ui.c.fill_rect(b, t.s(0x1d1f21));
            let lh = ui.c.line_height(Font::new(14));
            ui.c.text_clip(Font::new(14), b.x + 6.0, (b.cy() - lh / 2.0).round(), b.w - 12.0, t.text, v);
        }
        Row::Tiles(kind, names, sel) => {
            let previews = match kind {
                TileList::Skins => &s.settings.skin_previews,
                TileList::Themes => &s.settings.theme_previews,
            };
            for (i, name) in names.iter().enumerate() {
                let (col, rowi) = ((i % TILES_PER_ROW) as f32, (i / TILES_PER_ROW) as f32);
                let b = Rect::new(x0 + col * TILE_PITCH_X, y + 4.0 + rowi * TILE_PITCH_Y, TILE_W, TILE_H);
                let (act, hint) = match kind {
                    TileList::Skins => (SAct::Skin(i), "Use this skin: its shapes and own colours"),
                    TileList::Themes if i < os_theme::Design::ALL.len() => {
                        (SAct::Preset(i), "Use this skin's own colours, on either skin (right-click to duplicate)")
                    }
                    TileList::Themes if i >= s.settings.user_from => {
                        (SAct::Preset(i), "Use your theme (right-click to rename, duplicate, delete or show it in its folder)")
                    }
                    TileList::Themes => (SAct::Preset(i), "Use this theme's colours (right-click to duplicate)"),
                };
                let hov = ui.hit(b, Action::Settings(act), hint);
                let lemon = super::lemon(ui);
                let rad = if lemon { lk::r_btn(t) } else { 0.0 };
                let frame = if i == *sel {
                    if lemon { t.brand_sel(os_theme::design::lemon::MAIN) } else { t.a(0x2d7cd0) }
                } else if hov {
                    if lemon { t.brand_grey(os_theme::design::lemon::DARK_HIGHLIGHT) } else { t.s(0x5a5c5e) }
                } else {
                    t.s(0x161819)
                };
                ui.c.fill_rrect(b, rad + 2.0, frame);
                let inner = b.inset(2.0);
                let pr = Rect::new(inner.x, inner.y, inner.w, TILE_PREVIEW_H);
                match previews.get(i) {
                    Some(p) => super::theme_preview::paint(ui.c, p, pr),
                    None => ui.c.fill_rect(pr, t.s(0x1d1e1f)),
                }
                let nr = Rect::new(inner.x, pr.bottom(), inner.w, inner.h - pr.h);
                let name_bg = t.s(0x1d1e1f);
                ui.c.fill_rrect(nr, rad, name_bg);
                ui.c.fill_rect(Rect::new(nr.x, nr.y, nr.w, rad), name_bg);
                let f = ui.c.fit_font(Font::new(13).bold(), name, nr.w - 6.0);
                ui.c.text_center(f, nr, t.text, name);
            }
        }
        // The applied theme's text is hard to read on some of its surfaces (`crate::theme_library::readability`).
        Row::ThemeWarn => {
            let b = Rect::new(x0, y + 4.0, 80.0, 28.0);
            let hov = ui.hit(b, Action::Settings(SAct::Button(FIX_TEXT)), "Change the text colour so it reads on every surface");
            if super::lemon(ui) {
                lk::outlined(ui.c, b, lk::r_btn(t), t.s(0x161819), lk::face(t.s(0x2a2d2f), kit::State::hover(hov)));
            } else {
                kit::bordered(ui.c, b, 2.0, t.s(0x161819), kit::face(t.s(0x212425), kit::State::hover(hov)));
            }
            ui.c.text_center(Font::new(13), b, if hov { t.glyph } else { t.text }, FIX_TEXT);
            let warn = Color::rgb(0xffb020);
            let (cx, cy) = (b.right() + 20.0, cy);
            ui.c.fill_circle(cx, cy, 8.0, warn);
            ui.c.text_center(Font::new(13).bold(), Rect::new(cx - 8.0, cy - 8.0, 16.0, 16.0), Color::rgb(0x1a1a1a), "!");
            ui.c.text_clip(f, cx + 14.0, ty, g.right() - 12.0 - (cx + 14.0), t.text, "Text is hard to read on some surfaces with these colours.");
        }
        // The applied theme's values and colours, edited live (spec: ui/theme-presets.md §C, `crate::theme_adjust`).
        Row::ThemeKnobs => {
            use crate::theme_adjust::{self as adj, KNOBS, SWATCHES};
            let text = adj::applied_text(s, t.style.design);
            let th = os_theme::FlTheme::parse(&text);
            for (i, k) in KNOBS.iter().enumerate() {
                let cx = x0 + 28.0 + i as f32 * 61.0;
                let v = adj::knob_value(&th, i);
                let act = Action::Settings(SAct::ThemeKnob(i as u8));
                let held = ui.held(&act);
                let hint = format!("{} (drag or scroll; middle click: the theme's own value)", k.text(v));
                let hov = ui.hit(Rect::new(cx - 28.0, y + 4.0, 56.0, 58.0), act, hint);
                kit::knob(ui.c, t, cx, y + 24.0, 14.0, k.pos(v), true, kit::KnobLook::Ring, kit::State::hover(hov || held));
                ui.c.text_center(Font::new(13), Rect::new(cx - 30.0, y + 42.0, 60.0, 18.0), t.text, k.label);
            }
            let open = s.settings.picker.as_ref().map(|p| p.swatch);
            for (i, k) in SWATCHES.iter().enumerate() {
                let (cx, cy) = (x0 + 28.0 + i as f32 * 61.0, y + 92.0);
                let parts = adj::parts(&text, i);
                let cr = Rect::new(cx - 16.0, cy - 16.0, 32.0, 32.0);
                let hint = format!("{k} colour: click to pick");
                let hov = if i == adj::STEPS {
                    let l = ui.hit(Rect::new(cr.x, cr.y, 16.0, 32.0), Action::Settings(SAct::Swatch(i as u8, 0)), format!("{k} colour (even groups): click to pick"));
                    let r = ui.hit(Rect::new(cx, cr.y, 16.0, 32.0), Action::Settings(SAct::Swatch(i as u8, 1)), format!("{k} colour (odd groups): click to pick"));
                    l || r
                } else {
                    ui.hit(cr, Action::Settings(SAct::Swatch(i as u8, 0)), hint)
                };
                let ring = if open == Some(i) {
                    if super::lemon(ui) { t.brand_sel(os_theme::design::lemon::MAIN) } else { t.a(0x2d7cd0) }
                } else if hov {
                    t.s(0x8a8c8e)
                } else {
                    t.s(0x111213)
                };
                ui.c.fill_circle(cx, cy, 16.0, ring);
                pie(ui.c, cx, cy, 14.5, &parts.iter().map(|p| p.colour).collect::<Vec<_>>());
                ui.c.text_center(Font::new(13), Rect::new(cx - 30.0, y + 110.0, 60.0, 18.0), t.text, k);
            }
        }
        Row::Gap(_) => {}
        Row::Pair(a, b) => {
            paint_row_at(ui, s, group, a, g, y, h, 0.0);
            paint_row_at(ui, s, group, b, g, y, h, 304.0);
        }
        Row::Folders(list) => folders(ui, list, Rect::new(x0 - 4.0, y + 2.0, g.w - 16.0, h - 6.0)),
        Row::Ppi => ppi_ruler(ui, s, x0, cy),
        Row::SearchFolders(_) => search_folders(ui, s, Rect::new(x0 - 4.0, y + 2.0, g.w - 16.0, h - 6.0)),
        Row::Prefer(cur) => prefer(ui, *cur, x0, y, ty),
        Row::Table => table(ui, s, Rect::new(x0 - 4.0, y + 2.0, g.w - 16.0, h - 4.0)),
        Row::Dim(name, r) => {
            paint_row_at(ui, s, group, r, g, y, h, dx);
            let b = Rect::span(x0 - 4.0, g.right() - 8.0, y, y + h);
            ui.off(b, name);
            grey(ui, b);
        }
        Row::Edit(field, w) => {
            let w = if *w > 0.0 { *w } else { g.right() - 8.0 - (x0 - 4.0) };
            let b = Rect::new(x0 - 4.0, y + 2.0, w, h - 4.0);
            info_box(ui, s, *field, b);
            // The genre button (▸ at the box's right end) opens the genre menu (spec: ui/settings-choices.md "Info").
            if *field == InfoField::Genre {
                let a = Rect::new(b.right() + 4.0, b.y, 22.0, b.h);
                let hov = ui.hit(a, Action::Settings(SAct::Genres(a.x, a.bottom())), "MP3 music genres");
                kit::combo(ui.c, t, a, "", f, kit::State::hover(hov));
            }
        }
        // Four lines of small grey text at the bottom left, beside the Reset working time button (spec:
        // ui/settings-choices.md "Info"), which sits at the bottom right and is drawn as a `Row::Buttons` there.
        Row::InfoFooter(lines) => {
            let bx = g.right() - 8.0 - 150.0;
            let by = y + h - 36.0;
            for (i, line) in lines.iter().enumerate() {
                let ly = y + 4.0 + i as f32 * FOOTER_LINE_H;
                let right = if ly + FOOTER_LINE_H > by + 4.0 { bx - 12.0 } else { g.right() - 8.0 };
                ui.c.text_clip(Font::new(13), x0, ly, right - x0, t.text_dim, line);
            }
            paint_row_at(ui, s, group, &Row::Buttons(vec![(RESET_WORK_TIME, 150.0)]), g, by, 36.0, bx - g.x - 12.0);
        }
        Row::DataFolder => super::data_folder::paint_row(ui, s, row, g, x0, y, h),
        Row::FactoryFolder => super::factory_folder::paint_row(ui, g, x0, y, h),
        Row::Midi(m) => super::midi_page::paint_row(ui, s, *m, g, x0, y, h),
        // As Steinberg provides it, with the notice beside it in the page's own font (its usage guidelines §14).
        Row::AsioLogo => {
            let Some((iw, ih, px)) = asio_logo() else { return };
            let r = Rect::new(x0, y + 2.0, (ASIO_LOGO_H * *iw as f32 / *ih as f32).round(), ASIO_LOGO_H);
            ui.c.image(r, *iw, *ih, px);
            let small = Font::new(13);
            let lh = ui.c.line_height(small);
            let ty = (r.cy() - lh).round();
            ui.c.text(small, r.right() + 8.0, ty, t.text_dim, "ASIO is a registered trademark of");
            ui.c.text(small, r.right() + 8.0, ty + lh, t.text_dim, "Steinberg Media Technologies GmbH.");
        }
    }
}

/// The Info footer's line pitch.
const FOOTER_LINE_H: f32 = 16.0;

/// The ASIO Compatible Logo's height (`Row::AsioLogo`).
const ASIO_LOGO_H: f32 = 40.0;

/// The ASIO Compatible Logo from the ASIO SDK (`asiosdk/logo`, unchanged), decoded once.
#[cfg(windows)]
pub(crate) fn asio_logo() -> Option<&'static (usize, usize, Vec<u32>)> {
    static LOGO: std::sync::OnceLock<Option<(usize, usize, Vec<u32>)>> = std::sync::OnceLock::new();
    LOGO.get_or_init(|| os_ui::Canvas::decode_png(include_bytes!("../../asiosdk/logo/ASIO-compatible-logo-Steinberg-TM-BW.png")))
        .as_ref()
}

/// Builds without ASIO show no logo.
#[cfg(not(windows))]
pub(crate) fn asio_logo() -> Option<&'static (usize, usize, Vec<u32>)> {
    None
}

/// Lines the Comments box shows, and their pitch.
const COMMENT_LINES: usize = 24;
const COMMENT_LINE_H: f32 = 17.0;

/// An Info text box: the project's text, or the text being typed with its caret or selection. A one-line box scrolls
/// sideways to keep the caret in view; Comments wraps its lines at the box's width and, while edited, scrolls to the
/// caret's line.
fn info_box(ui: &mut Ui, s: &State, field: InfoField, b: Rect) {
    let t = ui.t;
    let f = LABEL;
    let lh = ui.c.line_height(f);
    let edit = s.settings.info_edit.as_ref().filter(|e| e.field == field).map(|e| &e.text);
    let hint = match (edit.is_some(), field.multiline()) {
        (false, _) => field.hint(),
        (true, false) => "Enter accepts, Esc cancels",
        (true, true) => "Enter starts a new line; a click elsewhere accepts, Esc cancels",
    };
    let hov = ui.hit(b, Action::Settings(SAct::Info(field)), hint);
    let well = if edit.is_some() { t.s(0x141516) } else if hov { t.s(0x1d1f21) } else { t.s(0x191b1c) };
    kit::sunken(ui.c, t, b, well);
    let shown = field.get(&s.project.info).as_deref().unwrap_or("");
    let (text, caret, all) = match edit {
        Some(e) => (e.text.as_str(), Some(e.caret), e.all && !e.text.is_empty()),
        None => (shown, None, false),
    };
    let sel = if super::lemon(ui) { t.brand_sel(os_theme::design::lemon::COLORED_DARK) } else { t.a(0x2d5a8c) };
    let inner = Rect::new(b.x + 6.0, b.y + 3.0, b.w - 12.0, b.h - 6.0);
    ui.c.push_clip(inner);
    if !field.multiline() {
        let caret_x = caret.map_or(0.0, |c| ui.c.text_width(f, &text[..byte_at(text, c)]));
        let x = inner.x - (caret_x - (inner.w - 2.0)).max(0.0);
        let y = (b.cy() - lh / 2.0).round();
        if all {
            let w = ui.c.text_width(f, text);
            ui.c.fill_rect(Rect::new(x - 1.0, y, w + 2.0, lh), sel);
        }
        ui.c.text(f, x, y, t.text, text);
        if caret.is_some() && !all {
            ui.c.fill_rect(Rect::new((x + caret_x).round(), y, 1.0, lh), t.text);
        }
    } else {
        let lines = wrap(ui, f, text, inner.w - 2.0);
        // The caret's line: the last one starting at or before it.
        let at = caret.map(|c| lines.iter().rposition(|&(start, _)| start <= c).unwrap_or(0));
        let top = at.map_or(0, |l| (l + 1).saturating_sub(COMMENT_LINES));
        for (i, &(start, n)) in lines.iter().enumerate().skip(top).take(COMMENT_LINES) {
            let y = inner.y + 1.0 + (i - top) as f32 * COMMENT_LINE_H + ((COMMENT_LINE_H - lh) / 2.0).round();
            let line = &text[byte_at(text, start)..byte_at(text, start + n)];
            if all {
                let w = ui.c.text_width(f, line);
                ui.c.fill_rect(Rect::new(inner.x - 1.0, y, w + 2.0, lh), sel);
            }
            ui.c.text(f, inner.x, y, t.text, line);
            if let (Some(c), Some(l)) = (caret, at) {
                if l == i && !all {
                    let cx = ui.c.text_width(f, &text[byte_at(text, start)..byte_at(text, c)]);
                    ui.c.fill_rect(Rect::new((inner.x + cx).round(), y, 1.0, lh), t.text);
                }
            }
        }
    }
    ui.c.pop_clip();
}

/// Byte offset of character `i` in `s` (its length past the end).
fn byte_at(s: &str, i: usize) -> usize {
    s.char_indices().nth(i).map_or(s.len(), |(b, _)| b)
}

/// The lines `text` shows in `w` px: (first character, characters) of each, in characters. A line longer than `w`
/// breaks after its last space that fits, else where it is full; the line break itself belongs to no line.
pub(crate) fn wrap(ui: &mut Ui, f: Font, text: &str, w: f32) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut base = 0;
    for para in text.split('\n') {
        let chars: Vec<char> = para.chars().collect();
        let mut start = 0;
        loop {
            let (mut x, mut end, mut space) = (0.0, start, None);
            while end < chars.len() {
                let cw = ui.c.text_width(f, chars[end].encode_utf8(&mut [0; 4]));
                if x + cw > w && end > start {
                    break;
                }
                x += cw;
                end += 1;
                if chars[end - 1] == ' ' {
                    space = Some(end);
                }
            }
            if end < chars.len() {
                end = space.unwrap_or(end);
            }
            out.push((base + start, end - start));
            if end >= chars.len() {
                break;
            }
            start = end;
        }
        base += chars.len() + 1;
    }
    out
}

/// A list frame (lists, the folder list, the plugin table).
fn list_frame(ui: &mut Ui, b: Rect) {
    let t = ui.t;
    if super::lemon(ui) {
        lk::outlined(ui.c, b, lk::r_btn(t), t.s(0x161819), t.s(0x222527));
    } else {
        ui.c.fill_rect(b, t.s(0x161819));
        ui.c.fill_rect(b.inset(1.0), t.s(0x222527));
    }
}

/// Browser extra search folders: a row's height, where its Folder and Browser name columns start (from the list's
/// left).
const SF_ROW_H: f32 = 17.0;
const SF_FOLDER_X: f32 = 30.0;
const SF_NAME_X: f32 = 440.0;

/// File ▸ Browser extra search folders (`crate::search_folders`; spec: ui/settings-choices.md "Browser extra search
/// folders"): the Folder and Browser name columns with a folder button at the left of each row, the filled rows first,
/// then empty ones. The cell being typed into shows its text with its caret or selection.
fn search_folders(ui: &mut Ui, s: &State, b: Rect) {
    use crate::search_folders::{rows, shown_rows, Col};
    let t = ui.t;
    list_frame(ui, b);
    let f = Font::new(13);
    for (c, cx) in [("Folder", SF_FOLDER_X), ("Browser name", SF_NAME_X)] {
        ui.c.text(f, b.x + 6.0 + cx, b.y + 2.0, t.text_dim, c);
    }
    let filled = rows(s);
    let edit = s.settings.folder_edit.as_ref();
    for i in 0..shown_rows(s) {
        let rr = Rect::new(b.x + 2.0, b.y + 20.0 + i as f32 * SF_ROW_H, b.w - 4.0, SF_ROW_H);
        if i % 2 == 1 {
            ui.c.fill_rect(rr, t.s(0x26292b));
        }
        let pick = Rect::new(rr.x + 4.0, rr.y, 22.0, rr.h);
        let hint = if i < filled.len() { "Choose another folder for this row" } else { "Add a folder to the browser" };
        let hov = ui.hit(pick, Action::Settings(SAct::FolderPick(i)), hint);
        let icon = Rect::new(pick.x + 4.0, pick.y + 2.0, 13.0, 13.0);
        crate::icons::draw(ui.c, crate::icons::Icon::Folder, icon, if hov { t.glyph } else { t.text_dim });
        let Some((path, name)) = filled.get(i) else { continue };
        let cells = [
            (Col::Folder, Rect::span(rr.x + SF_FOLDER_X, rr.x + SF_NAME_X - 6.0, rr.y, rr.bottom()), path, "The folder: click, type another one or empty it (removes it from the browser), Enter"),
            (Col::Name, Rect::span(rr.x + SF_NAME_X, rr.right(), rr.y, rr.bottom()), name, "Its name in the browser (empty: the folder's name): click, type, Enter"),
        ];
        for (col, cr, text, hint) in cells {
            let typing = edit.filter(|e| e.row == i && e.col == col).map(|e| &e.text);
            let hov = ui.hit(cr, Action::Settings(SAct::FolderCell(i, col)), if typing.is_some() { "Enter accepts, Esc cancels" } else { hint });
            cell_text(ui, cr, text, typing, hov);
        }
    }
}

/// A list cell's text, or the text being typed into it: a darker well, the text scrolled sideways to keep the caret in
/// view, the caret or the selection.
fn cell_text(ui: &mut Ui, r: Rect, text: &str, typing: Option<&crate::dialog::Field>, hov: bool) {
    let t = ui.t;
    let f = Font::new(13);
    let lh = ui.c.line_height(f);
    let y = (r.cy() - lh / 2.0).round();
    let inner = Rect::new(r.x + 4.0, r.y, r.w - 8.0, r.h);
    let Some(e) = typing else {
        ui.c.text_clip(f, inner.x, y, inner.w, if hov { t.glyph } else { t.text }, text);
        return;
    };
    ui.c.fill_rect(r, t.s(0x141516));
    let sel = if super::lemon(ui) { t.brand_sel(os_theme::design::lemon::COLORED_DARK) } else { t.a(0x2d5a8c) };
    ui.c.push_clip(inner);
    let caret_x = ui.c.text_width(f, e.before_caret());
    let x = inner.x - (caret_x - (inner.w - 2.0)).max(0.0);
    let all = e.all && !e.text.is_empty();
    if all {
        let w = ui.c.text_width(f, &e.text);
        ui.c.fill_rect(Rect::new(x - 1.0, y, w + 2.0, lh), sel);
    }
    ui.c.text(f, x, y, t.text, &e.text);
    if !all {
        ui.c.fill_rect(Rect::new((x + caret_x).round(), y, 1.0, lh), t.text);
    }
    ui.c.pop_clip();
}

/// Search folders (spec: ui/plugin-manager.md §2): a check, the path, the tag; added folders end with a remove button.
fn folders(ui: &mut Ui, list: &[(String, &'static str, bool, bool)], b: Rect) {
    let t = ui.t;
    list_frame(ui, b);
    let f = Font::new(13);
    let lh = ui.c.line_height(f);
    if list.is_empty() {
        ui.c.text(f, b.x + 8.0, (b.y + 2.0 + 10.5 - lh / 2.0).round(), t.text_dim, "(no folders)");
    }
    for (i, (path, tag, on, added)) in list.iter().enumerate() {
        let rr = Rect::new(b.x + 2.0, b.y + 2.0 + i as f32 * 21.0, b.w - 4.0, 21.0);
        if i % 2 == 1 {
            ui.c.fill_rect(rr, t.s(0x26292b));
        }
        let ty = (rr.cy() - lh / 2.0).round();
        let right = if *added { rr.right() - 26.0 } else { rr.right() - 8.0 };
        let tag_w = ui.c.text_width(f, tag);
        let hr = Rect::span(rr.x, right - tag_w - 8.0, rr.y, rr.bottom());
        let hov = ui.hit(hr, Action::Settings(SAct::Plug(Plug::Folder(i))), if *on { "Searched: click to skip this folder" } else { "Skipped: click to search this folder" });
        led(ui, rr.x + 12.0, rr.cy(), *on, hov);
        let col = if !*on { t.text_dim } else if hov { t.glyph } else { t.text };
        ui.c.text_clip(f, rr.x + 26.0, ty, hr.right() - rr.x - 30.0, col, path);
        ui.c.text_right(f, right, ty, t.text_dim, tag);
        if *added {
            let xr = Rect::new(rr.right() - 21.0, rr.y + 2.0, 17.0, 17.0);
            let hov = ui.hit(xr, Action::Settings(SAct::Plug(Plug::Remove(i))), "Remove this folder");
            ui.c.icon(&os_ui::tabler::X, xr.inset(2.0), if hov { t.glyph } else { t.text_dim });
        }
    }
}

/// The preferred format switch (spec: ui/plugin-manager.md §3): VST3, CLAP, VST2.
fn prefer(ui: &mut Ui, cur: Prefer, x0: f32, y: f32, ty: f32) {
    let t = ui.t;
    let f = LABEL;
    let lw = ui.c.text(f, x0, ty, t.text, "Preferred format");
    let lemon = super::lemon(ui);
    let mut x = x0 + lw + 12.0;
    for p in [Prefer::Vst3, Prefer::Clap, Prefer::Vst2] {
        let b = Rect::new(x, y + 3.0, 54.0, 21.0);
        let on = p == cur;
        let hov = ui.hit(b, Action::Settings(SAct::Plug(Plug::Prefer(p))), "Listed when a plugin is found in several formats");
        let ink = if lemon {
            if on {
                lk::lit_in(ui.c, ui.t, b, lk::r_btn(t), hov);
                lk::ink_on_lit(ui.t)
            } else {
                lk::outlined(ui.c, b, lk::r_btn(t), t.s(0x161819), lk::face(t.s(0x2a2d2f), kit::State::hover(hov)));
                if hov { t.glyph } else { t.text }
            }
        } else if on {
            kit::bordered(ui.c, b, 2.0, t.s(0x161819), t.h(0xf09672));
            t.s(0x1c1e21)
        } else {
            kit::bordered(ui.c, b, 2.0, t.s(0x161819), kit::face(t.s(0x212425), kit::State::hover(hov)));
            if hov { t.glyph } else { t.text }
        };
        ui.c.text_center(Font::new(13), b, ink, p.label());
        x += 58.0;
    }
}

/// A filled star (favourite).
const STAR_ON: os_ui::tabler::TablerIcon = os_ui::tabler::TablerIcon { filled: true, d: os_ui::tabler::STAR.d };

/// The plugin table (spec: ui/plugin-manager.md §2): sortable column titles, ★ / Show toggles, the count and the Find box.
fn table(ui: &mut Ui, s: &State, area: Rect) {
    let t = ui.t;
    let d = &s.plugdb;
    let f = Font::new(13);
    let lh = ui.c.line_height(f);
    let b = Rect::new(area.x, area.y, area.w, 4.0 + 18.0 + TABLE_ROW_H * TABLE_ROWS as f32 + 2.0);
    list_frame(ui, b);
    ui.hit(b, Action::Settings(SAct::Plug(Plug::Table)), "");
    let x0 = b.x + 2.0;
    // Column titles.
    let hy = b.y + 2.0;
    for (col, &(cx, cw)) in Col::ALL.iter().zip(&COLS) {
        let hr = Rect::new(x0 + cx, hy, cw, 18.0);
        let hov = ui.hit(hr, Action::Settings(SAct::Plug(Plug::Sort(*col))), "Sort by this column");
        let tw = ui.c.text(f, hr.x + 4.0, (hr.cy() - lh / 2.0).round(), if hov { t.glyph } else { t.text_dim }, col.title());
        if d.sort.0 == *col {
            kit::chevron(ui.c, hr.x + 4.0 + tw + 6.0, hr.cy(), 3.0, if d.sort.1 { Dir::Up } else { Dir::Down }, t.text_dim);
        }
    }
    ui.c.fill_rect(Rect::new(b.x + 1.0, hy + 18.0, b.w - 2.0, 1.0), t.s(0x161819));
    // Rows.
    let ry0 = hy + 19.0;
    let n = d.rows.len();
    let scroll = n > TABLE_ROWS;
    let rw = b.w - 4.0 - if scroll { 10.0 } else { 0.0 };
    if n == 0 {
        let msg = if d.find.is_empty() { "(no plugins)" } else { "(no plugin matches)" };
        ui.c.text(f, x0 + 8.0, ry0 + 1.0, t.text_dim, msg);
    }
    for (vi, row) in d.rows.iter().enumerate().skip(d.top).take(TABLE_ROWS) {
        let rr = Rect::new(x0, ry0 + (vi - d.top) as f32 * TABLE_ROW_H, rw, TABLE_ROW_H);
        if (vi - d.top) % 2 == 1 {
            ui.c.fill_rect(rr, t.s(0x26292b));
        }
        let hint = match (row.path.is_empty(), row.note.is_empty()) {
            (false, false) => format!("{}: {}", row.path, row.note),
            (false, true) => row.path.clone(),
            (true, false) => row.note.clone(),
            (true, true) => String::new(),
        };
        ui.hit(rr, Action::Settings(SAct::Plug(Plug::Row(vi))), hint);
        let ty = (rr.cy() - lh / 2.0).round();
        if row.toggles {
            let sr = Rect::new(x0 + COLS[0].0, rr.y, COLS[0].1, rr.h);
            let hov = ui.hit(sr, Action::Settings(SAct::Plug(Plug::Star(vi))), if row.favorite { "Favourite: listed first in the plugin menus" } else { "Star: list it first in the plugin menus" });
            let ir = Rect::new(sr.cx() - 6.5, sr.cy() - 6.5, 13.0, 13.0);
            if row.favorite {
                ui.c.icon(&STAR_ON, ir, if super::lemon(ui) { t.brand_lit(os_theme::design::lemon::MAIN) } else { t.h(0xfdc13d) });
            } else {
                ui.c.icon(&os_ui::tabler::STAR, ir, if hov { t.glyph } else { t.s(0x58595a) });
            }
            let shr = Rect::new(x0 + COLS[1].0, rr.y, COLS[1].1, rr.h);
            let hov = ui.hit(shr, Action::Settings(SAct::Plug(Plug::Show(vi))), if row.shown { "Listed in the plugin menus: click to hide" } else { "Hidden from the plugin menus: click to list" });
            led(ui, shr.cx(), shr.cy(), row.shown, hov);
        }
        let col = if row.dim || !row.shown { t.text_dim } else { t.text };
        // A plugin this program cannot load is listed dimmed, not as an error.
        let failed = !row.toggles && !row.status.is_empty() && row.status != "ok" && row.status != db::Status::Unsupported.text();
        let cells: [(&str, Color); 6] = [
            (&row.name, col),
            (row.format, col),
            (row.kind, col),
            (&row.vendor, col),
            (&row.status, if failed { t.h(0xe8502a) } else { col }),
            (&row.file, col),
        ];
        for (k, (text, c)) in cells.iter().enumerate() {
            let (cx, cw) = COLS[k + 2];
            let w = (cw - 8.0).min(rr.right() - (x0 + cx) - 6.0);
            ui.c.text_clip(f, x0 + cx + 4.0, ty, w, *c, text);
        }
    }
    if scroll {
        let track = Rect::new(b.right() - 10.0, ry0 + 2.0, 7.0, TABLE_ROW_H * TABLE_ROWS as f32 - 4.0);
        super::scrollbar(ui, track, true, d.top as f32 * TABLE_ROW_H, TABLE_ROW_H * TABLE_ROWS as f32, n as f32 * TABLE_ROW_H, crate::app::Scroll::PlugTable);
    }
    // Count and Find box.
    let fy = b.bottom() + 6.0;
    let all = d.all_rows.len();
    let count = if n == all { format!("{n} plugins") } else { format!("{n} of {all} plugins") };
    ui.c.text(f, b.x + 4.0, (fy + 11.0 - lh / 2.0).round(), t.text_dim, &count);
    let fr = Rect::new(b.right() - 220.0, fy, 220.0, 22.0);
    let hov = ui.hit(fr, Action::Settings(SAct::Plug(Plug::Find)), "Find: type to keep the plugins whose name, vendor or file contain the text (Esc clears)");
    let well = if d.finding { t.s(0x141516) } else if hov { t.s(0x1d1f21) } else { t.s(0x191b1c) };
    kit::sunken(ui.c, t, fr, well);
    ui.c.icon(&os_ui::tabler::SEARCH, Rect::new(fr.x + 5.0, fr.cy() - 6.5, 13.0, 13.0), t.text_dim);
    let tx = fr.x + 23.0;
    let ty = (fr.cy() - lh / 2.0).round();
    if d.find.is_empty() && !d.finding {
        ui.c.text(f, tx, ty, t.text_dim, "Find");
    } else {
        ui.c.text_clip(f, tx, ty, fr.w - 30.0, t.text, &d.find);
        if d.finding {
            let cx = (tx + ui.c.text_width(f, &d.find)).min(fr.right() - 6.0);
            ui.c.fill_rect(Rect::new(cx.round() + 1.0, fr.y + 4.0, 1.0, fr.h - 8.0), t.text);
        }
    }
}

/// General ▸ Display's PPI ruler from `x` (the page's left margin), centred a little above `cy` (spec:
/// ui/settings-choices.md "Display: PPI ruler, Force refreshes, Thick lines, High visibility (2026-10-16)"): a dark
/// track as long as 400 device pixels with ticks under it every 100, filled from its left end N device pixels long
/// (+1 for the rounded end), grey at rest and orange (Lemon: the accent) while the mouse is over the bar or drags it.
fn ppi_ruler(ui: &mut Ui, s: &State, x: f32, cy: f32) {
    let t = ui.t;
    // Logical px per device px.
    let px = 1.0 / ui.c.scale.max(0.01);
    let n = crate::ppi::ppi(s.display_scale);
    let track = Rect::new(x, cy - 6.0, f32::from(crate::ppi::MAX) * px, 6.0);
    let a = Action::Settings(SAct::Ppi);
    let hit = Rect::new(track.x - 4.0, track.y - 4.0, track.w + 8.0, 18.0);
    let hov = ui.hit(hit, a.clone(), "Display PPI: drag, or turn the mouse wheel");
    let active = hov || ui.held(&a);
    ui.c.fill_rrect(track, 3.0, t.s(0x161819));
    for i in 1..=4 {
        let tx = x + 100.0 * i as f32 * px;
        ui.c.fill_rect(Rect::new(tx - px, track.bottom() + 2.0, px, 4.0), t.text_dim);
    }
    let col = match (active, super::lemon(ui)) {
        (false, _) => t.tx(0xb9bfc3),
        (true, false) => t.h(0xffbc40),
        (true, true) => t.accent,
    };
    ui.c.fill_rrect(Rect::new(x, track.y, f32::from(n + 1) * px, track.h), 3.0, col);
}

/// A slider's track `tr` filled up to `v` (0..=1) and its handle there, centred on `cy`.
fn slider_track(ui: &mut Ui, tr: Rect, cy: f32, v: f32) {
    let t = ui.t;
    if super::lemon(ui) {
        ui.c.fill_rrect(tr, 2.0, t.s(0x161819));
        ui.c.fill_rrect(Rect::new(tr.x, tr.y, tr.w * v, tr.h), 2.0, t.accent);
    } else {
        ui.c.fill_rect(tr, t.s(0x161819));
        ui.c.fill_rect(Rect::new(tr.x, tr.y, tr.w * v, tr.h), t.h(0xc8312e));
    }
    let hx = tr.x + tr.w * v;
    ui.c.fill_circle(hx, cy, 8.0, t.s(0x111213));
    ui.c.fill_circle(hx, cy, 7.0, t.s(0x4a4d50));
    ui.c.fill_rect(Rect::new(hx - 0.5, cy - 6.0, 1.0, 5.0), t.s(0x1a1b1c));
}

pub(super) fn value_red(ui: &mut Ui, b: Rect, v: &str) {
    let t = ui.t;
    if super::lemon(ui) {
        // Lemon: number fields are the coloured dark with lemon digits.
        use os_theme::design::lemon as brand;
        let well = t.brand_lit(brand::COLORED_DARK);
        lk::outlined(ui.c, b, lk::r_small(t), well.shade(-0.25), well);
        ui.c.text_center(Font::new(15), b, t.brand_lit(brand::HIGHLIGHT), v);
        return;
    }
    kit::bordered(ui.c, b, 3.0, t.h(0x5a2229), t.h(0x863c44));
    ui.c.text_center(Font::new(15), b, t.tx(0xf2e4e6), v);
}
