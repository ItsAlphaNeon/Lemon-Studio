// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! General ▸ Input ▸ Manage shortcuts: which F8 key runs the browser and the pickers, which modifier held while a
//! MIDI file is imported shows its import window, and which modifier held while double-clicking an audio clip opens
//! its properties or its channel's settings. Program settings (`crate::prefs`, `settings.txt`, not on a settings
//! page: `prefs::OFF_PAGE_COMBOS`). The window (`views::dialog`) edits a copy of them (`Form`) that Apply keeps; the
//! settings window closes while it is open.
// spec: ui/settings-choices.md "Input: Manage shortcuts (2026-10-16)"

use crate::app::{Action, App, Win};
use crate::dialog::{Body, Modal};
use crate::prefs::{self, Combo};

/// The window's caption.
pub const TITLE: &str = "Manage shortcuts";

/// The keys of the Browser and pickers rows.
pub const F8_KEYS: &[&str] = &["F8", "Shift+F8", "Ctrl+F8", "Alt+F8"];
/// The modifiers of the MIDI import window rows.
pub const MIDI_MODS: &[&str] = &["(none)", "Alt", "Shift"];
/// The modifiers of the Double-click on audio clips rows; "(disabled)" turns that action off.
pub const CLIP_MODS: &[&str] = &["(disabled)", "(none)", "Alt"];

/// Browser and pickers: shows or hides the browser.
pub const TOGGLE_BROWSER: Combo = Combo { key: "shortcut_toggle_browser", label: "Toggle browser", choices: F8_KEYS, default: 3 };
/// Opens or closes the plugin menu (`App::toggle_plugin_menu`).
pub const TOGGLE_PLUGIN_PICKER: Combo =
    Combo { key: "shortcut_toggle_plugin_picker", label: "Toggle plugin picker", choices: F8_KEYS, default: 0 };
/// Opens or closes the project picker (`App::toggle_project_picker`).
pub const TOGGLE_PROJECT_PICKER: Combo =
    Combo { key: "shortcut_toggle_project_picker", label: "Toggle project picker", choices: F8_KEYS, default: 2 };
/// Shows the plugin database: the browser on its Plugin database tab (`App::show_plugin_database`).
pub const SHOW_PLUGIN_DATABASE: Combo =
    Combo { key: "shortcut_show_plugin_database", label: "Show plugin database", choices: F8_KEYS, default: 1 };
/// MIDI import window: the modifier held while a MIDI file is chosen or dropped picks the row whose behaviour applies
/// (`midi_file::show_for`; spec: ui/midi-files.md §10.5).
pub const MIDI_WHEN_NECESSARY: Combo =
    Combo { key: "midi_import_window_when_necessary", label: "Show when necessary", choices: MIDI_MODS, default: 0 };
pub const MIDI_EVERY_TIME: Combo =
    Combo { key: "midi_import_window_every_time", label: "Show every time", choices: MIDI_MODS, default: 1 };
pub const MIDI_NEVER: Combo = Combo { key: "midi_import_window_never", label: "Don't show", choices: MIDI_MODS, default: 2 };
/// Double-click on audio clips: the clip's properties. Lemon Studio has no clip properties window yet, so a
/// double-click with this row's modifier does nothing.
pub const CLIP_PROPERTIES: Combo =
    Combo { key: "audio_clip_double_click_properties", label: "Show clip properties", choices: CLIP_MODS, default: 2 };
/// The clip's channel settings: its channel selected in the rack and its sampler window opened.
pub const CLIP_CHANNEL: Combo =
    Combo { key: "audio_clip_double_click_channel", label: "Show channel settings", choices: CLIP_MODS, default: 1 };

/// The window's rows, top to bottom; `GROUPS` splits them.
pub const ROWS: [&Combo; 9] = [
    &TOGGLE_BROWSER,
    &TOGGLE_PLUGIN_PICKER,
    &TOGGLE_PROJECT_PICKER,
    &SHOW_PLUGIN_DATABASE,
    &MIDI_WHEN_NECESSARY,
    &MIDI_EVERY_TIME,
    &MIDI_NEVER,
    &CLIP_PROPERTIES,
    &CLIP_CHANNEL,
];

/// The window's groups: title and how many of `ROWS` it takes, in order.
pub const GROUPS: [(&str, usize); 3] = [("Browser and pickers", 4), ("MIDI import window", 3), ("Double-click on audio clips", 2)];

/// The rows of row `row`'s group (indices in `ROWS`).
fn group_of(row: usize) -> std::ops::Range<usize> {
    let mut start = 0;
    for (_, n) in GROUPS {
        if row < start + n {
            return start..start + n;
        }
        start += n;
    }
    row..row + 1
}

/// The window's form: a choice per row, and the row whose list of choices is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    /// Each row's choice (an index in its `choices`), in `ROWS` order.
    pub vals: [usize; ROWS.len()],
    pub open: Option<usize>,
}

impl Form {
    /// The applied shortcuts (as the window opens).
    pub fn applied() -> Form {
        Form { vals: ROWS.map(prefs::index), open: None }
    }

    /// Row `row` takes choice `v`. Each value is used once per group, "(disabled)" included: the row of the group that
    /// held `v` takes `row`'s old value (a swap).
    pub fn pick(&mut self, row: usize, v: usize) {
        let Some(&old) = self.vals.get(row) else { return };
        if v == old || v >= ROWS[row].choices.len() {
            return;
        }
        for r in group_of(row) {
            if r != row && self.vals[r] == v {
                self.vals[r] = old;
            }
        }
        self.vals[row] = v;
    }

    /// Reset all: the defaults in the form (kept only by Apply).
    pub fn reset(&mut self) {
        self.vals = ROWS.map(|c| c.default);
    }

    /// Apply: the form's choices are kept and work at once.
    pub fn apply(&self) {
        for (c, &v) in ROWS.iter().zip(&self.vals) {
            prefs::set_choice(c, v);
        }
    }

    /// Row `row`'s choice as its box shows it.
    pub fn text(&self, row: usize) -> &'static str {
        ROWS[row].choices[self.vals[row]]
    }
}

/// What an F8 key runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum F8 {
    Browser,
    PluginPicker,
    ProjectPicker,
    PluginDatabase,
}

/// The F8 action of the modifiers held: the Browser and pickers row whose key that is. F8 with two or more modifiers
/// is none of the choices: nothing.
pub fn f8(shift: bool, ctrl: bool, alt: bool) -> Option<F8> {
    let key = match (shift, ctrl, alt) {
        (false, false, false) => "F8",
        (true, false, false) => "Shift+F8",
        (false, true, false) => "Ctrl+F8",
        (false, false, true) => "Alt+F8",
        _ => return None,
    };
    [(&TOGGLE_BROWSER, F8::Browser), (&TOGGLE_PLUGIN_PICKER, F8::PluginPicker), (&TOGGLE_PROJECT_PICKER, F8::ProjectPicker), (&SHOW_PLUGIN_DATABASE, F8::PluginDatabase)]
        .into_iter()
        .find(|(c, _)| prefs::choice(c) == key)
        .map(|(_, a)| a)
}

/// What a double-click on an audio clip opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipOpen {
    Properties,
    Channel,
}

/// What a double-click on an audio clip opens with Alt held or not: the Double-click on audio clips row with that
/// modifier ("(none)" without Alt). A row set to "(disabled)" never does; with neither row, nothing.
pub fn clip_double_click(alt: bool) -> Option<ClipOpen> {
    let held = if alt { "Alt" } else { "(none)" };
    [(&CLIP_PROPERTIES, ClipOpen::Properties), (&CLIP_CHANNEL, ClipOpen::Channel)]
        .into_iter()
        .find(|(c, _)| prefs::choice(c) == held)
        .map(|(_, a)| a)
}

/// The key a menu shows beside an entry, its `shortcut` as written: the F8 keys of the browser and the pickers follow
/// the applied shortcuts.
pub fn menu_key(label: &str, shortcut: &'static str) -> &'static str {
    let c = match label {
        "Browser" => &TOGGLE_BROWSER,
        "Plugin picker" => &TOGGLE_PLUGIN_PICKER,
        "Project picker" => &TOGGLE_PROJECT_PICKER,
        "Plugin database" => &SHOW_PLUGIN_DATABASE,
        _ => return shortcut,
    };
    if shortcut.ends_with("F8") { prefs::choice(c) } else { shortcut }
}

impl App {
    /// The Manage shortcuts button: the window, filled with the applied shortcuts. The settings window closes while it
    /// is open.
    pub(crate) fn open_shortcuts(&mut self) {
        crate::project_info::accept_info(&mut self.state);
        self.state.settings.open = false;
        self.state.dialogs.push(Modal::new(Body::Shortcuts(Form::applied())));
        self.modal_changed();
    }

    /// The window closed: by Apply (`apply`), which keeps the form, or by its × (or Esc), which drops it. The settings
    /// window comes back.
    pub(crate) fn shortcuts_closed(&mut self, form: &Form, apply: bool) {
        if apply {
            form.apply();
            self.state.message = Some("Shortcuts applied".into());
        }
        self.state.settings.open = true;
        self.invalidate();
    }

    /// F8 with the modifiers held (`ctrl`, `alt`, `App::shift`): the Browser and pickers rows.
    pub(crate) fn f8_key(&mut self, ctrl: bool, alt: bool) {
        match f8(self.shift, ctrl, alt) {
            Some(F8::Browser) => self.act(Action::ToggleWin(Win::Browser)),
            Some(F8::PluginPicker) => self.toggle_plugin_menu(),
            Some(F8::PluginDatabase) => self.show_plugin_database(),
            Some(F8::ProjectPicker) => self.toggle_project_picker(),
            None => {}
        }
    }

    /// Show plugin database: VIEW > Plugin database (spec: ui/browser-commands.md §4), the browser shown on its Plugin
    /// database tab as it was left, unless the browser's search field has the keyboard.
    pub(crate) fn show_plugin_database(&mut self) {
        if !self.state.browser_search_focus {
            self.browser_jump(crate::browser::Jump::PluginDb);
        }
        self.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Action;
    use crate::dialog::DHit;
    use crate::views::settings::{self, SAct};
    use os_ui::{Canvas, Rect};

    fn row(c: &Combo) -> usize {
        ROWS.iter().position(|r| r.key == c.key).expect("a row")
    }

    fn texts(f: &Form) -> Vec<&'static str> {
        (0..ROWS.len()).map(|i| f.text(i)).collect()
    }

    /// The rows, their groups and defaults, and the choices of each (spec: the table).
    #[test]
    fn the_rows_and_their_defaults() {
        let f = Form::applied();
        assert_eq!(texts(&f), ["Alt+F8", "F8", "Ctrl+F8", "Shift+F8", "(none)", "Alt", "Shift", "Alt", "(none)"]);
        let labels: Vec<&str> = ROWS.iter().map(|c| c.label).collect();
        assert_eq!(labels, [
            "Toggle browser",
            "Toggle plugin picker",
            "Toggle project picker",
            "Show plugin database",
            "Show when necessary",
            "Show every time",
            "Don't show",
            "Show clip properties",
            "Show channel settings"
        ]);
        assert_eq!(GROUPS.iter().map(|g| g.1).sum::<usize>(), ROWS.len());
        assert_eq!(F8_KEYS, ["F8", "Shift+F8", "Ctrl+F8", "Alt+F8"]);
        assert_eq!(MIDI_MODS, ["(none)", "Alt", "Shift"]);
        assert_eq!(CLIP_MODS, ["(disabled)", "(none)", "Alt"]);
        // They are program settings, not on a settings page.
        for c in ROWS {
            assert!(prefs::OFF_PAGE_COMBOS.iter().any(|o| o.key == c.key), "{} is declared", c.label);
        }
    }

    /// Each value is used once per group: picking a value another row holds swaps (spec: the four examples).
    #[test]
    fn picking_a_held_value_swaps() {
        let mut f = Form::applied();
        // Toggle browser Alt+F8 to F8: Toggle plugin picker becomes Alt+F8.
        f.pick(row(&TOGGLE_BROWSER), 0);
        assert_eq!((f.text(0), f.text(1), f.text(2), f.text(3)), ("F8", "Alt+F8", "Ctrl+F8", "Shift+F8"));
        // Show every time Alt to Shift: Don't show becomes Alt.
        f.pick(row(&MIDI_EVERY_TIME), 2);
        assert_eq!((f.text(4), f.text(5), f.text(6)), ("(none)", "Shift", "Alt"));
        // Show clip properties Alt to (disabled): nothing else changes.
        let before = f.clone();
        f.pick(row(&CLIP_PROPERTIES), 0);
        assert_eq!(f.text(7), "(disabled)");
        assert_eq!(f.vals[..7], before.vals[..7]);
        assert_eq!(f.text(8), "(none)");
        // Then (disabled) to (none): Show channel settings, which held (none), becomes (disabled).
        f.pick(row(&CLIP_PROPERTIES), 1);
        assert_eq!((f.text(7), f.text(8)), ("(none)", "(disabled)"));
        // Every group still uses each value once.
        let mut start = 0;
        for (_, n) in GROUPS {
            let mut v = f.vals[start..start + n].to_vec();
            v.sort_unstable();
            v.dedup();
            assert_eq!(v.len(), n);
            start += n;
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

    fn form(app: &App) -> &Form {
        match &app.state.dialogs.last().expect("the window is open").body {
            Body::Shortcuts(f) => f,
            _ => panic!("another dialog is on top"),
        }
    }

    /// Picks choice `v` of row `row` through the window: its box, then the entry in its list.
    fn pick(app: &mut App, c: &mut Canvas, row: usize, v: usize) {
        let r = hit_rect(app, &Action::Dlg(DHit::Shortcut(row)));
        click(app, c, r);
        assert_eq!(form(app).open, Some(row), "its list opens");
        let r = hit_rect(app, &Action::Dlg(DHit::ShortcutChoice(v)));
        click(app, c, r);
        assert_eq!(form(app).open, None, "the list closes");
    }

    /// The button opens the window and the settings window closes; Reset all only fills the form, the × discards it,
    /// Apply keeps it and closes, and the settings window comes back each time (spec).
    #[test]
    fn the_window_reset_all_apply_and_close() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.open_settings(settings::GENERAL);
        app.paint(&mut c, None);
        let button = hit_rect(&app, &Action::Settings(SAct::Button(settings::MANAGE_SHORTCUTS)));
        click(&mut app, &mut c, button);
        assert!(!app.state.settings.open, "the settings window closes");
        assert_eq!(form(&app), &Form::applied());
        // Toggle browser to F8 (a swap), then Apply: kept, the window closes, settings come back.
        pick(&mut app, &mut c, 0, 0);
        assert_eq!((form(&app).text(0), form(&app).text(1)), ("F8", "Alt+F8"));
        assert_eq!(prefs::choice(&TOGGLE_BROWSER), "Alt+F8", "nothing applied yet");
        let apply = hit_rect(&app, &Action::Dlg(DHit::Apply));
        click(&mut app, &mut c, apply);
        assert!(app.state.dialogs.is_empty() && app.state.settings.open);
        assert_eq!((prefs::choice(&TOGGLE_BROWSER), prefs::choice(&TOGGLE_PLUGIN_PICKER)), ("F8", "Alt+F8"));
        // Again: Reset all puts the defaults in the form only; the × drops it, the applied keys stay.
        app.paint(&mut c, None);
        let button = hit_rect(&app, &Action::Settings(SAct::Button(settings::MANAGE_SHORTCUTS)));
        click(&mut app, &mut c, button);
        assert_eq!(form(&app).text(0), "F8", "the window shows the applied keys");
        let reset = hit_rect(&app, &Action::Dlg(DHit::ResetAll));
        click(&mut app, &mut c, reset);
        assert_eq!(texts(form(&app)), texts(&Form { vals: ROWS.map(|c| c.default), open: None }));
        assert_eq!(prefs::choice(&TOGGLE_BROWSER), "F8", "Reset all applies nothing");
        let x = hit_rect(&app, &Action::Dlg(DHit::Close));
        click(&mut app, &mut c, x);
        assert!(app.state.dialogs.is_empty() && app.state.settings.open);
        assert_eq!((prefs::choice(&TOGGLE_BROWSER), prefs::choice(&TOGGLE_PLUGIN_PICKER)), ("F8", "Alt+F8"));
        // Esc closes it without applying too; Reset all and Apply puts the defaults back.
        app.open_shortcuts();
        app.paint(&mut c, None);
        pick(&mut app, &mut c, 7, 0);
        app.key(&winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape), false, false);
        assert!(app.state.dialogs.is_empty() && app.state.settings.open);
        assert_eq!(prefs::choice(&CLIP_PROPERTIES), "Alt");
        app.open_shortcuts();
        app.paint(&mut c, None);
        let reset = hit_rect(&app, &Action::Dlg(DHit::ResetAll));
        click(&mut app, &mut c, reset);
        let apply = hit_rect(&app, &Action::Dlg(DHit::Apply));
        click(&mut app, &mut c, apply);
        assert_eq!((prefs::choice(&TOGGLE_BROWSER), prefs::choice(&TOGGLE_PLUGIN_PICKER)), ("Alt+F8", "F8"));
        // MIDI import rows are kept (nothing reads them yet).
        app.open_shortcuts();
        app.paint(&mut c, None);
        pick(&mut app, &mut c, row(&MIDI_EVERY_TIME), 2);
        let apply = hit_rect(&app, &Action::Dlg(DHit::Apply));
        click(&mut app, &mut c, apply);
        assert_eq!((prefs::choice(&MIDI_EVERY_TIME), prefs::choice(&MIDI_NEVER)), ("Shift", "Alt"));
    }

    /// F8, Alt+F8 and Shift+F8 follow the applied keys at once: after the swap, F8 toggles the browser and Alt+F8 opens
    /// the plugin picker (spec). Show plugin database shows the browser's Plugin database tab; the project picker's key
    /// does nothing.
    #[test]
    fn f8_keys_follow_the_applied_shortcuts() {
        use winit::keyboard::{Key, NamedKey};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.paint(&mut c, None);
        let f8 = Key::Named(NamedKey::F8);
        let browser = |app: &App| app.state.show_browser;
        let picker = |app: &App| crate::plugin_menu::is_open(&app.state);
        let shown = browser(&app);
        // Defaults: Alt+F8 the browser, F8 the plugin picker.
        app.key(&f8, false, true);
        assert_ne!(browser(&app), shown, "Alt+F8 toggles the browser");
        app.key(&f8, false, true);
        assert_eq!(browser(&app), shown);
        app.key(&f8, false, false);
        assert!(picker(&app), "F8 opens the plugin picker");
        app.key(&f8, false, false);
        assert!(!picker(&app));
        // Toggle browser to F8 and apply: the keys swap.
        let mut f = Form::applied();
        f.pick(0, 0);
        f.apply();
        app.key(&f8, false, false);
        assert_ne!(browser(&app), shown, "F8 toggles the browser now");
        assert!(!picker(&app));
        app.key(&f8, false, false);
        assert_eq!(browser(&app), shown);
        app.key(&f8, false, true);
        assert!(picker(&app), "Alt+F8 opens the plugin picker now");
        assert_eq!(browser(&app), shown);
        app.key(&f8, false, true);
        assert!(!picker(&app));
        // Shift+F8: the browser on its Plugin database tab, as it was left (spec: ui/browser-commands.md §4).
        app.state.show_browser = false;
        app.shift = true;
        app.key(&f8, false, false);
        app.shift = false;
        assert!(browser(&app) && app.state.browser_tab == crate::views::browser::PLUGIN_TAB);
        // Ctrl+F8 (the project picker) and F8 with two modifiers do nothing.
        let before = (browser(&app), picker(&app), app.state.browser_tab);
        app.key(&f8, true, false);
        app.key(&f8, true, true);
        assert_eq!((browser(&app), picker(&app), app.state.browser_tab), before);
        assert_eq!(super::f8(true, false, true), None);
    }

    /// The menus show the applied keys beside the browser and the pickers.
    #[test]
    fn menus_show_the_applied_keys() {
        assert_eq!((menu_key("Browser", "Alt+F8"), menu_key("Plugin picker", "F8")), ("Alt+F8", "F8"));
        let mut f = Form::applied();
        f.pick(0, 0);
        f.pick(3, 2);
        f.apply();
        assert_eq!((menu_key("Browser", "Alt+F8"), menu_key("Plugin picker", "F8")), ("F8", "Alt+F8"));
        assert_eq!((menu_key("Plugin database", "Shift+F8"), menu_key("Project picker", "Ctrl+F8")), ("Ctrl+F8", "Shift+F8"));
        assert_eq!(menu_key("Browser", ""), "", "an entry without a key keeps none");
        assert_eq!(menu_key("Mixer", "F9"), "F9");
    }

    /// Double-click on audio clips: the row with the held modifier wins; "(disabled)" never fires.
    #[test]
    fn audio_clip_double_click_rows() {
        assert_eq!((clip_double_click(false), clip_double_click(true)), (Some(ClipOpen::Channel), Some(ClipOpen::Properties)));
        let mut f = Form::applied();
        f.pick(7, 0);
        f.pick(7, 1);
        f.apply();
        assert_eq!(texts(&f)[7..], ["(none)", "(disabled)"]);
        assert_eq!((clip_double_click(false), clip_double_click(true)), (Some(ClipOpen::Properties), None));
        f.pick(8, 2);
        f.apply();
        assert_eq!((clip_double_click(false), clip_double_click(true)), (Some(ClipOpen::Properties), Some(ClipOpen::Channel)));
    }
}
