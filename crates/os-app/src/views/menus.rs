// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Menu bar contents and the dropdown.
//!
//! Structure, sizes and colours follow the captured menus (spec: ui/menus.md). Product names and
//! cloud / shop entries are not reproduced; entries with no behaviour yet are greyed out: dim, never
//! highlighted, and they only answer "not available yet".
// spec: ui/menus.md, ui/main-window.md §3

use os_ui::{Font, Rect};

use crate::app::{Action, State, Ui, Win};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Item,
    /// Section header row (not clickable).
    Head,
    Sep,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mark {
    None,
    Check(bool),
    Radio(bool),
}

#[derive(Clone, Copy)]
pub struct Item {
    pub kind: Kind,
    pub label: &'static str,
    pub shortcut: &'static str,
    /// Implemented in this build.
    pub works: bool,
    pub mark: Mark,
    /// Opens a submenu (contents not captured yet; drawn with an arrow).
    pub sub: bool,
}

const fn it(label: &'static str, shortcut: &'static str) -> Item {
    Item { kind: Kind::Item, label, shortcut, works: false, mark: Mark::None, sub: false }
}
const fn head(label: &'static str) -> Item {
    Item { kind: Kind::Head, label, shortcut: "", works: false, mark: Mark::None, sub: false }
}
const SEP: Item = Item { kind: Kind::Sep, label: "", shortcut: "", works: false, mark: Mark::None, sub: false };

impl Item {
    const fn on(self) -> Item {
        Item { works: true, ..self }
    }
    const fn sub(self) -> Item {
        Item { sub: true, ..self }
    }
    const fn tick(self) -> Item {
        Item { mark: Mark::Check(false), ..self }
    }
    const fn radio(self, on: bool) -> Item {
        Item { mark: Mark::Radio(on), ..self }
    }
}

/// One dropdown: its title, column widths and the items of each column.
pub struct Menu {
    pub title: &'static str,
    pub cols: &'static [(f32, &'static [Item])],
}

// Flat item indices (columns in order) used by the app; `tests::indices` checks them.
pub const FILE_NEW: usize = 0;
pub const FILE_OPEN: usize = 2;
pub const FILE_SAVE: usize = 4;
pub const FILE_SAVE_AS: usize = 5;
pub const FILE_SAVE_VERSION: usize = 6;
/// FILE > Export as WAV (Ctrl+R; the toolbar's render button).
pub const FILE_EXPORT: usize = 12;
pub const FILE_EXIT: usize = 16;
/// OPTIONS > Theme settings (flat index: header counts).
#[cfg(test)]
pub const OPT_THEME: usize = 5;
/// OPTIONS > Metronome (spec: ui/metronome.md §1).
pub const METRONOME_LABEL: &str = "Metronome";
/// OPTIONS > Auto scrolling (spec: ui/auto-scrolling.md §1).
pub const AUTOSCROLL_LABEL: &str = "Auto scrolling";
/// OPTIONS > Enable groups (spec: ui/enable-groups.md §1).
pub const GROUPS_LABEL: &str = "Enable groups";
/// Index of the OPTIONS menu.
pub const OPTIONS_MENU: usize = 5;
/// OPTIONS > Project info (the toolbar's info button).
pub const OPT_INFO: usize = 9;
/// HELP > About (the toolbar logo).
pub const HELP_MENU: usize = 7;
pub const HELP_ABOUT: usize = 6;
/// Index of the ADD and TOOLS menus.
pub const ADD_MENU: usize = 2;
pub const TOOLS_MENU: usize = 6;
/// `MenuItem(PATTERNS, PATTERN_PICK + n)` selects pattern `n` (the pattern list column).
pub const PATTERNS: usize = 3;
pub const PATTERN_PICK: usize = 1000;
pub const VIEW: usize = 4;
pub const EDIT_MENU: usize = 1;

const FILE: &[Item] = &[
    it("New", "").on(),
    it("New from template", "").sub(),
    it("Open...", "Ctrl+O").on(),
    SEP,
    it("Save", "Ctrl+S").on(),
    it("Save as...", "Shift+Ctrl+S").on(),
    it("Save new version", "Ctrl+N").on(),
    it("Save to new project folder...", ""),
    it("Save as template...", ""),
    it("Delete unused files and save project", ""),
    SEP,
    it("Import", "").sub(),
    it("Export as WAV...", "Ctrl+R").on(),
    SEP,
    it("Revert to last backup", ""),
    SEP,
    it("Exit", "").on(),
];

const EDIT: &[Item] = &[it("Undo", "Ctrl+Z")];

const ADD_PLUGINS: &[Item] = &[
    it("More plugins...", "").on(),
    SEP,
    head("Channel"),
    it("Sampler", "").on(),
    head("Misc"),
    it("Audio clip", ""),
    it("Automation clip", ""),
    it("Layer", ""),
    SEP,
    it("Categories", "").radio(true),
    it("Simple", "").radio(false),
    it("Tree", "").radio(false),
];
const ADD_OTHER: &[Item] = &[
    head("Other"),
    it("Effect", "").sub(),
    it("Plugin picker", "F8").on(),
    it("Browse plugin database", ""),
    it("Browse all installed plugins", ""),
    it("Browse presets", ""),
    SEP,
    it("Refresh plugin list (fast scan)", "").on(),
    it("Manage plugins", ""),
    SEP,
    it("Automation for last tweaked parameter", ""),
    SEP,
    it("Pattern", "").on(),
];

/// The pattern list column is built from the project; this is its placeholder (never drawn).
const PAT_LIST: &[Item] = &[];
const PAT_CMDS: &[Item] = &[
    it("Find first empty...", "Shift+F4").on(),
    it("Find next empty...", "F4").on(),
    it("Find next empty (no naming)", "Ctrl+F4").on(),
    SEP,
    it("Select in playlist", "").on(),
    it("Rename and color...", "F2").on(),
    it("Change color...", "").on(),
    it("Random color", "").on(),
    it("Open in project browser", "Shift+Ctrl+P"),
    it("Set time signature...", ""),
    it("Transpose...", "").on(),
    SEP,
    it("Insert one", "Shift+Ctrl+Ins").on(),
    it("Clone", "Shift+Ctrl+C").on(),
    it("Delete", "Shift+Ctrl+Del").on(),
    SEP,
    it("Move up", "Shift+Ctrl+Up").on(),
    it("Move down", "Shift+Ctrl+Down").on(),
    SEP,
    it("Split by channel", ""),
    it("Quick render as audio clip", ""),
    it("Render as audio clip...", ""),
    it("Render and replace...", ""),
];

const VIEW_ITEMS: &[Item] = &[
    head("Windows"),
    it("Playlist", "F5").on().tick(),
    it("Piano roll", "F7").on().tick(),
    it("Channel rack", "F6").on().tick(),
    it("Mixer", "F9").on().tick(),
    it("Browser", "Alt+F8").on().tick(),
    it("Project picker", "Ctrl+F8"),
    it("Plugin picker", "F8").on(),
    it("Tempo tapper", "Ctrl+F5").tick(),
    it("Touch controller", "Alt+F7"),
    it("Script output", ""),
    it("Toolbars", "").sub(),
    it("Plugin performance monitor", ""),
    head("Layout"),
    it("Close all windows", "F12").on(),
    it("Close all plugin windows", "Alt+F12").on(),
    it("Close all unfocused windows", "Ctrl+F12").on(),
    it("Align all channel editors", "Shift+F12"),
    it("Arrange windows", "").sub(),
    it("Background", "").sub(),
    head("Browser"),
    it("Undo history", ""),
    it("Patterns", ""),
    it("Generators in use", ""),
    it("Effects in use", ""),
    it("Remote control", ""),
    it("Plugin database", "Shift+F8"),
];

const OPTIONS: &[Item] = &[
    head("System"),
    it("MIDI settings", "F10"),
    it("Audio settings", ""),
    it("General settings", ""),
    it("File settings", ""),
    it("Theme settings", ""),
    it("Manage plugins", ""),
    head("Project"),
    it("Project general settings", ""),
    it("Project info", "F11"),
    head("Info"),
    it("Debugging log", ""),
    head("MIDI"),
    it("Enable MIDI remote control", "").tick(),
    it("Enable MIDI output", "").tick(),
    it("Enable MIDI master sync", "").tick(),
    head("Switches"),
    it("Typing keyboard to piano", "Ctrl+T").on().tick(),
    it(METRONOME_LABEL, "Ctrl+M").on().tick(),
    it("Recording precount", "Ctrl+P").tick(),
    it("Start on input", "Ctrl+I").tick(),
    it("Blend recorded notes", "Ctrl+B").tick(),
    it("Step edit", "Ctrl+E").tick(),
    it("Loop record", "").tick(),
    it(AUTOSCROLL_LABEL, "").on().tick(),
    it(GROUPS_LABEL, "Shift+Alt+G").on().tick(),
    it("Multilink to controllers", "Ctrl+J").tick(),
];

const TOOLS_A: &[Item] = &[
    it("Browser find...", "Ctrl+F"),
    it("One-click audio recording", "").sub(),
    it("Chord progression tool...", ""),
    it("Control creator", ""),
    head("Clipboard"),
    it("Save audio clipboard to browser", ""),
    it("Save MIDI clipboard to browser", ""),
    head("Score logger"),
    it("Dump score log to selected pattern", "").sub(),
    it("Clear log", ""),
    head("Audio logger"),
    it("Enabled", "").tick(),
    it("Settings", "").sub(),
    it("Dump to audio editor", ""),
    head("Last tweaked parameters"),
    it("Last tweaked", "").sub(),
    it("Next to last tweaked", "").sub(),
];
const TOOLS_B: &[Item] = &[
    head("Macros"),
    it("Stop sound", "Ctrl+H").on(),
    it("Cancel recording", ""),
    SEP,
    it("Prepare for performance mode", ""),
    it("Prepare for MIDI export", ""),
    it("Select unused channels", "").on(),
    it("Purge unused audio clips", "").on(),
    it("Switch smart disable for all plugins", ""),
    it("Switch all audio clip channels to", "").sub(),
    it("Switch selected audio clip channels to", "").sub(),
    it("Lock all playlist tracks to content", ""),
    it("Unlock all playlist tracks from content", ""),
    it("Reset empty playlist tracks", "").on(),
    it("Delete unused files and save project", ""),
    it("Switch all plugin editors to", "").sub(),
    it("Set keyboard focus for all plugins to", "").sub(),
    it("Change declicking for selected channels to", "").sub(),
];

const HELP: &[Item] = &[
    it("Help index", ""),
    it("Tutorials", "").sub(),
    head("Support"),
    it("Diagnostics", ""),
    it("What's new", ""),
    SEP,
    it("About...", "").on(),
];

pub const MENUS: &[Menu] = &[
    Menu { title: "FILE", cols: &[(216.0, FILE)] },
    Menu { title: "EDIT", cols: &[(121.0, EDIT)] },
    Menu { title: "ADD", cols: &[(176.0, ADD_PLUGINS), (239.0, ADD_OTHER)] },
    Menu { title: "PATTERNS", cols: &[(80.0, PAT_LIST), (243.0, PAT_CMDS)] },
    Menu { title: "VIEW", cols: &[(244.0, VIEW_ITEMS)] },
    Menu { title: "OPTIONS", cols: &[(217.0, OPTIONS)] },
    Menu { title: "TOOLS", cols: &[(212.0, TOOLS_A), (247.0, TOOLS_B)] },
    Menu { title: "HELP", cols: &[(207.0, HELP)] },
];

/// Flat item `i` of `menu` (columns in order; the pattern list column has no items).
pub fn item_at(menu: usize, i: usize) -> Option<&'static Item> {
    MENUS.get(menu)?.cols.iter().flat_map(|(_, items)| items.iter()).nth(i)
}

/// Menu bar font: Cuprum 18 px condensed (spec §3).
pub const BAR_FONT: Font = Font::condensed(18, 6);

/// Menu bar item layout: (x0, x1, average character width of the font). Left edges are the measured word
/// starts. The words are as wide as the font makes them; where a text engine sets them wider than the slots
/// were measured for, all eight are condensed one step further so that no word runs into the next.
pub fn bar_layout(c: &mut os_ui::Canvas) -> [(f32, f32, f32); 8] {
    const X: [f32; 8] = [8.0, 35.0, 61.0, 88.0, 143.0, 176.0, 226.0, 266.0];
    const END: f32 = 300.0;
    let mut out = [(0.0, 0.0, 6.0); 8];
    for width in [BAR_FONT.width, 5, 4] {
        let f = Font::condensed(BAR_FONT.size, width);
        let mut fits = true;
        for (i, m) in MENUS.iter().enumerate() {
            let x1 = X[i] + c.text_width(f, m.title);
            fits &= x1 + 4.0 <= X.get(i + 1).copied().unwrap_or(END);
            out[i] = (X[i], x1, f32::from(width));
        }
        if fits {
            break;
        }
    }
    out
}

/// VIEW's window entries toggle their window.
fn win_of(label: &str) -> Option<Win> {
    Some(match label {
        "Playlist" => Win::Playlist,
        "Piano roll" => Win::PianoRoll,
        "Channel rack" => Win::ChannelRack,
        "Mixer" => Win::Mixer,
        "Browser" => Win::Browser,
        "Plugin performance monitor" => Win::Perf,
        // spec: ui/tempo-tapper.md §1
        "Tempo tapper" => Win::Tapper,
        _ => return None,
    })
}

/// Whether an entry does something in this build: its static flag, or one of the label-dispatched commands
/// `App::menu_item` handles (settings tabs, pattern commands, window toggles, plugin picker, undo).
pub fn works(menu: usize, item: &Item) -> bool {
    item.works
        || ((menu == OPTIONS_MENU || menu == ADD_MENU) && crate::views::settings::tab_for_menu(item.label).is_some())
        || (menu == PATTERNS && crate::patterns::Cmd::from_label(item.label).is_some())
        || (menu == VIEW && (win_of(item.label).is_some() || item.label == "Plugin picker"))
        || (menu == EDIT_MENU && item.label == "Undo")
        || crate::params::menu_handles(item.label)
}

fn item_action(menu: usize, i: usize, item: &Item) -> Action {
    if menu == VIEW {
        if let Some(w) = win_of(item.label) {
            return Action::ToggleWin(w);
        }
    }
    Action::MenuItem(menu, i)
}

/// An entry's tick or radio state now: window entries follow their window (the browser its panel), the typing keyboard
/// switch its toggle; other ticks are drawn unticked.
fn mark_of(s: &State, menu: usize, item: &Item) -> Mark {
    match (item.mark, menu == VIEW) {
        // The browser is a panel of its own, shown by its own switch.
        (Mark::Check(_), true) if item.label == "Browser" => Mark::Check(s.show_browser),
        (Mark::Check(_), true) => Mark::Check(win_of(item.label).is_some_and(|w| s.shown(w))),
        (Mark::Check(_), _) if item.label == "Typing keyboard to piano" => Mark::Check(s.toggles.contains(&"typing")),
        (Mark::Check(_), _) if item.label == METRONOME_LABEL => Mark::Check(crate::metronome::on()),
        (Mark::Check(_), _) if item.label == AUTOSCROLL_LABEL => Mark::Check(crate::autoscroll::on()),
        (Mark::Check(_), _) if item.label == GROUPS_LABEL => Mark::Check(crate::enable_groups::on()),
        (mk, _) => mk,
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The menus as a native menu bar shows them (macOS): the same entries and what they do, built from `MENUS` and the
// project as the dropdown is, so the two never differ.

/// One row of a menu in the native menu bar.
#[derive(Clone, PartialEq)]
pub struct BarRow {
    pub kind: Kind,
    pub label: String,
    /// As written in `MENUS` ("Shift+Ctrl+S").
    pub shortcut: &'static str,
    pub enabled: bool,
    pub mark: Mark,
    /// What choosing it does (`pick`): menu and flat item index (`PATTERN_PICK + n` for pattern `n`).
    pub pick: Option<(usize, usize)>,
}

/// A menu of the native menu bar: its title ("File") and rows; columns follow one another, a separator between them.
#[derive(Clone, PartialEq)]
pub struct BarMenu {
    pub title: String,
    pub rows: Vec<BarRow>,
}

/// The menu bar for the project and windows as they are now.
pub fn bar_model(s: &State) -> Vec<BarMenu> {
    MENUS
        .iter()
        .enumerate()
        .map(|(mi, m)| {
            let mut rows = Vec::new();
            let mut flat = 0usize;
            for (ci, (_, items)) in m.cols.iter().enumerate() {
                if ci > 0 && !rows.is_empty() {
                    rows.push(BarRow { kind: Kind::Sep, label: String::new(), shortcut: "", enabled: false, mark: Mark::None, pick: None });
                }
                if mi == PATTERNS && ci == 0 {
                    for n in crate::patterns::listed(s) {
                        let mark = Mark::Radio(n == s.pattern);
                        rows.push(BarRow { kind: Kind::Item, label: crate::patterns::name_of(s, n), shortcut: "", enabled: true, mark, pick: Some((mi, PATTERN_PICK + n as usize)) });
                    }
                }
                for item in items.iter() {
                    let enabled = item.kind == Kind::Item && works(mi, item);
                    let pick = enabled.then_some((mi, flat));
                    rows.push(BarRow { kind: item.kind, label: item.label.to_string(), shortcut: item.shortcut, enabled, mark: mark_of(s, mi, item), pick });
                    flat += 1;
                }
            }
            BarMenu { title: bar_title(m.title), rows }
        })
        .collect()
}

/// A menu's title as a native menu bar writes it: "FILE" is "File".
pub fn bar_title(title: &str) -> String {
    title.chars().enumerate().map(|(i, c)| if i == 0 { c } else { c.to_ascii_lowercase() }).collect()
}

/// What choosing the menu bar row `pick` (`BarRow::pick`) does: what the same row of the dropdown does.
pub fn pick(menu: usize, i: usize) -> Action {
    match item_at(menu, i) {
        Some(item) if i < PATTERN_PICK => item_action(menu, i, item),
        _ => Action::MenuItem(menu, i),
    }
}

// Row heights follow the display scale like FL's popup menus (spec: ui/ui-scaling.md §2).
fn row_h(m: &os_ui::kit::menu::Metrics, it: &Item) -> f32 {
    match it.kind {
        Kind::Item => m.row,
        Kind::Head => m.head,
        Kind::Sep => m.sep,
    }
}

pub fn paint_dropdown(ui: &mut Ui, s: &State, menu: usize) {
    let Some(m) = MENUS.get(menu) else { return };
    let x0 = bar_layout(ui.c)[menu].0;
    let t = ui.t;
    let mt = os_ui::kit::menu::metrics(ui.c.scale);
    let (row, border) = (mt.row, mt.border);

    // Pattern list column: one row per project pattern (spec: ui/menus.md, PATTERNS).
    // spec: ui/pattern-menu.md §2: only used patterns, the column as wide as the longest name.
    let pats: Vec<(u16, String)> = if menu == PATTERNS {
        crate::patterns::listed(s).into_iter().map(|n| (n, crate::patterns::name_of(s, n))).collect()
    } else {
        Vec::new()
    };
    let list_w = pats.iter().map(|(_, n)| ui.c.text_width(Font::new(14), n) + 28.0).fold(80.0f32, f32::max).ceil();
    let col_w = |ci: usize, w: f32| if menu == PATTERNS && ci == 0 { list_w } else { w };

    let width: f32 = m.cols.iter().enumerate().map(|(ci, (w, _))| col_w(ci, *w)).sum::<f32>() + border * 2.0;
    let col_h = |items: &[Item]| items.iter().map(|it| row_h(&mt, it)).sum::<f32>();
    let body_h = m
        .cols
        .iter()
        .enumerate()
        .map(|(ci, (_, items))| if menu == PATTERNS && ci == 0 { pats.len() as f32 * row } else { col_h(items) })
        .fold(0.0, f32::max);
    let (ax, ay) = match (menu == PATTERNS, s.pat_ui.menu_at) {
        (true, Some(at)) => at,
        _ => (x0 - 4.0, 78.0),
    };
    let r = Rect::new(ax.min(s.screen.0 - width).max(0.0), ay, width, body_h + border * 2.0);
    let away = !r.contains(ui.mouse.0, ui.mouse.1);
    // Lemon (spec: ui/lemon-design.md §9): a rounded panel, rounded row highlights.
    let lemon = super::lemon(ui);
    if lemon {
        os_ui::kit::popup(ui.c, t, r);
    } else {
        ui.c.fill_rect(r, t.menu_pop);
        ui.c.stroke_rrect(Rect::new(r.x + mt.line, r.y + mt.line, r.w - 2.0 * mt.line, r.h - 2.0 * mt.line), 0.0, border, t.menu_pop_border);
    }
    ui.hit(r, Action::Swallow, "");

    let font = Font::new(14);
    // Text sits where it does at 100 %, moved by half of what the row lost.
    let dy = (row - os_ui::kit::menu::ROW) / 2.0;
    let mut cx = r.x + border;
    let mut flat = 0usize;
    for (ci, (cw, items)) in m.cols.iter().enumerate() {
        let cw = &col_w(ci, *cw);
        let col = Rect::new(cx, r.y + border, *cw, body_h);
        if ci > 0 {
            ui.c.fill_rect(Rect::new(cx - 1.0, col.y, 1.0, body_h), t.menu_pop_head);
        }
        let mut y = col.y;
        if menu == PATTERNS && ci == 0 {
            for (n, name) in &pats {
                let rr = Rect::new(col.x, y, col.w, row);
                let hov = ui.hit(rr, Action::MenuItem(menu, PATTERN_PICK + *n as usize), "Select pattern");
                let lit = hov || (*n == s.pattern && away);
                if lit && lemon {
                    os_ui::kit::row_highlight(ui.c, t, rr);
                } else if lit {
                    ui.c.fill_rect(rr, ui.t.s(0x03080c));
                }
                let col = if *n == s.pattern { t.glyph } else { t.menu_pop_text };
                let dx = if lit { 22.0 } else { 20.0 };
                ui.c.text_clip(font, rr.x + dx, rr.y + 2.0 + dy, rr.w - dx - 4.0, col, name);
                y += row;
            }
        }
        for item in items.iter() {
            let h = row_h(&mt, item);
            let rr = Rect::new(col.x, y, col.w, h);
            match item.kind {
                Kind::Sep => ui.c.fill_rect(Rect::new(col.x, y + mt.sep_line, col.w, mt.line), t.menu_pop_head),
                Kind::Head => {
                    if lemon {
                        ui.c.fill_rrect(Rect::new(rr.x + 3.0, rr.y + 1.0, rr.w - 6.0, rr.h - 2.0), 4.0f32.min(rr.h / 2.0 - 1.0).max(0.0), t.menu_pop_head);
                    } else {
                        ui.c.fill_rect(rr, t.menu_pop_head);
                    }
                    ui.c.text(Font::new(12), rr.x + 4.0, rr.y + 1.0 + (h - os_ui::kit::menu::HEAD) / 2.0, t.menu_pop_head_text, item.label);
                }
                Kind::Item => {
                    let ok = works(menu, item);
                    // Greyed-out entries never light up; they only say they are not available yet.
                    if ok {
                        if ui.hit(rr, item_action(menu, flat, item), item.label) {
                            if lemon {
                                os_ui::kit::row_highlight(ui.c, t, rr);
                            } else {
                                ui.c.fill_rect(rr, t.menu_pop_hi);
                            }
                        }
                    } else {
                        ui.off(rr, item.label);
                    }
                    let color = if ok { t.menu_pop_text } else { t.menu_pop_dim };
                    let mark = mark_of(s, menu, item);
                    let (mx, my) = (rr.x + 10.0, rr.y + row / 2.0);
                    match mark {
                        Mark::Check(true) => {
                            ui.c.line(mx - 4.0, my, mx - 1.0, my + 3.0, 1.5, t.menu_pop_text);
                            ui.c.line(mx - 1.0, my + 3.0, mx + 4.0, my - 3.0, 1.5, t.menu_pop_text);
                        }
                        Mark::Radio(on) => {
                            if on {
                                ui.c.fill_circle(mx, my, 3.0, t.menu_pop_text);
                            } else {
                                ui.c.stroke_circle(mx, my, 3.5, 1.0, t.menu_pop_dim);
                            }
                        }
                        _ => {}
                    }
                    ui.c.text_clip(font, rr.x + 20.0, rr.y + 2.0 + dy, rr.w - 24.0, color, item.label);
                    let right = rr.right() - if item.sub { 16.0 } else { 8.0 };
                    if !item.shortcut.is_empty() {
                        let sc = if ok { t.menu_pop_shortcut } else { t.menu_pop_dim.mix(t.menu_pop, 0.35) };
                        ui.c.text_right(font.italic(), right, rr.y + 3.0 + dy, sc, item.shortcut);
                    }
                    if item.sub {
                        let ax = rr.right() - 11.0;
                        ui.c.line(ax, my - 3.5, ax + 3.5, my, 1.2, t.menu_pop_dim);
                        ui.c.line(ax + 3.5, my, ax, my + 3.5, 1.2, t.menu_pop_dim);
                    }
                }
            }
            flat += 1;
            y += h;
        }
        cx += cw;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indices() {
        let l = |i| item_at(0, i).map(|it| it.label);
        assert_eq!(l(FILE_NEW), Some("New"));
        assert_eq!(l(FILE_OPEN), Some("Open..."));
        assert_eq!(l(FILE_SAVE), Some("Save"));
        assert_eq!(l(FILE_SAVE_AS), Some("Save as..."));
        assert_eq!(l(FILE_EXPORT), Some("Export as WAV..."));
        assert_eq!(l(FILE_SAVE_VERSION), Some("Save new version"));
        assert_eq!(item_at(OPTIONS_MENU, OPT_INFO).map(|it| it.label), Some("Project info"));
        assert_eq!(item_at(HELP_MENU, HELP_ABOUT).map(|it| it.label), Some("About..."));
        assert_eq!(MENUS[ADD_MENU].title, "ADD");
        assert_eq!(MENUS[TOOLS_MENU].title, "TOOLS");
        assert_eq!(l(FILE_EXIT), Some("Exit"));
        assert_eq!(MENUS[PATTERNS].title, "PATTERNS");
        assert_eq!(MENUS[VIEW].title, "VIEW");
        assert_eq!(MENUS[OPTIONS_MENU].title, "OPTIONS");
        assert_eq!(item_at(OPTIONS_MENU, OPT_THEME).map(|it| it.label), Some("Theme settings"));
    }

    /// The native menu bar (macOS) is the dropdowns: every entry in order, greyed out and ticked alike, choosing one
    /// doing what the dropdown's does, and the project's patterns in PATTERNS.
    #[test]
    fn bar_model_follows_the_dropdowns() {
        let mut app = crate::app::App::new(None);
        let bar = bar_model(&app.state);
        assert_eq!(bar.iter().map(|m| m.title.as_str()).collect::<Vec<_>>(), ["File", "Edit", "Add", "Patterns", "View", "Options", "Tools", "Help"]);
        for (mi, m) in MENUS.iter().enumerate() {
            let items: Vec<&Item> = m.cols.iter().flat_map(|(_, items)| items.iter()).collect();
            let rows: Vec<&BarRow> = bar[mi].rows.iter().filter(|r| r.pick.is_none_or(|(_, i)| i < PATTERN_PICK)).collect();
            // The columns' separator rows aside, the rows are the items.
            let rows: Vec<&BarRow> = rows.into_iter().filter(|r| r.kind != Kind::Sep || items.iter().any(|it| it.kind == Kind::Sep)).collect();
            let named: Vec<(&str, bool)> = rows.iter().filter(|r| r.kind == Kind::Item).map(|r| (r.label.as_str(), r.enabled)).collect();
            let want: Vec<(&str, bool)> = items.iter().filter(|it| it.kind == Kind::Item).map(|it| (it.label, works(mi, it))).collect();
            assert_eq!(named, want, "{}", m.title);
        }
        let save = bar[0].rows.iter().find(|r| r.label == "Save").unwrap();
        assert_eq!((save.shortcut, save.pick), ("Ctrl+S", Some((0, FILE_SAVE))));
        let playlist = bar[VIEW].rows.iter().find(|r| r.label == "Playlist").unwrap();
        assert_eq!(pick(VIEW, playlist.pick.unwrap().1), Action::ToggleWin(Win::Playlist));
        assert_eq!(playlist.mark, Mark::Check(app.state.shown(Win::Playlist)));
        // Patterns in use head PATTERNS, the current one marked.
        assert_eq!(bar[PATTERNS].rows[0].label, "Find first empty...", "a new project uses no pattern yet");
        app.state.project.patterns.iter_mut().find(|p| p.number == 1).expect("pattern 1").name = Some("Intro".into());
        let first = bar_model(&app.state)[PATTERNS].rows[0].clone();
        assert_eq!((first.label.as_str(), first.mark, first.pick), ("Intro", Mark::Radio(true), Some((PATTERNS, PATTERN_PICK + 1))));
        // Live state: hiding the playlist unticks it.
        app.state.hide(Win::Playlist);
        let playlist = bar_model(&app.state)[VIEW].rows.iter().find(|r| r.label == "Playlist").unwrap().clone();
        assert_eq!(playlist.mark, Mark::Check(false));
    }
}
