//! Menu bar contents and the dropdown.
//!
//! Structure, sizes and colours follow the captured menus (spec: ui/menus.md). Product names and
//! cloud / shop entries are not reproduced; entries with no behaviour yet are dim and answer
//! "not available yet".
// spec: ui/menus.md, ui/main-window.md §3

use os_ui::{Font, Rect};

use crate::app::{Action, State, Ui, Win};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Item,
    /// Section header row (not clickable).
    Head,
    Sep,
}

#[derive(Clone, Copy, PartialEq, Eq)]
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
pub const FILE_EXIT: usize = 16;
/// `MenuItem(PATTERNS, PATTERN_PICK + n)` selects pattern `n` (the pattern list column).
pub const PATTERNS: usize = 3;
pub const PATTERN_PICK: usize = 1000;
const VIEW: usize = 4;

const FILE: &[Item] = &[
    it("New", "").on(),
    it("New from template", "").sub(),
    it("Open...", "Ctrl+O").on(),
    SEP,
    it("Save", "Ctrl+S").on(),
    it("Save as...", "Shift+Ctrl+S").on(),
    it("Save new version", "Ctrl+N"),
    it("Save to new project folder...", ""),
    it("Save as template...", ""),
    it("Delete unused files and save project", ""),
    SEP,
    it("Import", "").sub(),
    it("Export", "").sub(),
    SEP,
    it("Revert to last backup", ""),
    SEP,
    it("Exit", "").on(),
];

const EDIT: &[Item] = &[it("Undo", "Ctrl+Z")];

const ADD_PLUGINS: &[Item] = &[
    it("More plugins...", ""),
    SEP,
    head("Channel"),
    it("Sampler", ""),
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
    it("Plugin picker", "F8"),
    it("Browse plugin database", ""),
    it("Browse all installed plugins", ""),
    it("Browse presets", ""),
    SEP,
    it("Refresh plugin list (fast scan)", ""),
    it("Manage plugins", ""),
    SEP,
    it("Automation for last tweaked parameter", ""),
    SEP,
    it("Pattern", ""),
];

/// The pattern list column is built from the project; this is its placeholder (never drawn).
const PAT_LIST: &[Item] = &[];
const PAT_CMDS: &[Item] = &[
    it("Find first empty...", "Shift+F4"),
    it("Find next empty...", "F4"),
    it("Find next empty (no naming)", "Ctrl+F4"),
    SEP,
    it("Select in playlist", ""),
    it("Rename and color...", "F2"),
    it("Change color...", ""),
    it("Random color", ""),
    it("Open in project browser", "Shift+Ctrl+P"),
    it("Set time signature...", ""),
    it("Transpose...", ""),
    SEP,
    it("Insert one", "Shift+Ctrl+Ins"),
    it("Clone", "Shift+Ctrl+C"),
    it("Delete", "Shift+Ctrl+Del"),
    SEP,
    it("Move up", "Shift+Ctrl+Up"),
    it("Move down", "Shift+Ctrl+Down"),
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
    it("Plugin picker", "F8"),
    it("Tempo tapper", "Ctrl+F5"),
    it("Touch controller", "Alt+F7"),
    it("Script output", ""),
    it("Toolbars", "").sub(),
    it("Plugin performance monitor", ""),
    head("Layout"),
    it("Close all windows", "F12"),
    it("Close all plugin windows", "Alt+F12"),
    it("Close all unfocused windows", "Ctrl+F12"),
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
    it("Typing keyboard to piano", "Ctrl+T").tick(),
    it("Metronome", "Ctrl+M").tick(),
    it("Recording precount", "Ctrl+P").tick(),
    it("Start on input", "Ctrl+I").tick(),
    it("Blend recorded notes", "Ctrl+B").tick(),
    it("Step edit", "Ctrl+E").tick(),
    it("Loop record", "").tick(),
    it("Auto scrolling", "").tick(),
    it("Enable groups", "Shift+Alt+G").tick(),
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
    it("Stop sound", "Ctrl+H"),
    it("Cancel recording", ""),
    SEP,
    it("Prepare for performance mode", ""),
    it("Prepare for MIDI export", ""),
    it("Select unused channels", ""),
    it("Purge unused audio clips", ""),
    it("Switch smart disable for all plugins", ""),
    it("Switch all audio clip channels to", "").sub(),
    it("Switch selected audio clip channels to", "").sub(),
    it("Lock all playlist tracks to content", ""),
    it("Unlock all playlist tracks from content", ""),
    it("Reset empty playlist tracks", ""),
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
    it("About...", ""),
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

/// Menu bar item layout: (x0, x1, font size). Left edges are the measured word starts; widths
/// come from the font, so the words always fit.
pub fn bar_layout(c: &mut os_ui::Canvas) -> [(f32, f32, f32); 8] {
    const X: [f32; 8] = [8.0, 35.0, 61.0, 88.0, 143.0, 176.0, 226.0, 266.0];
    let mut out = [(0.0, 0.0, 18.0); 8];
    for (i, m) in MENUS.iter().enumerate() {
        out[i] = (X[i], X[i] + c.text_width(BAR_FONT, m.title), 18.0);
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
        _ => return None,
    })
}

fn item_action(menu: usize, i: usize, item: &Item) -> Action {
    if menu == VIEW {
        if let Some(w) = win_of(item.label) {
            return Action::ToggleWin(w);
        }
    }
    Action::MenuItem(menu, i)
}

const ROW: f32 = 19.0;
const HEAD_H: f32 = 16.0;
const SEP_H: f32 = 7.0;
const BORDER: f32 = 2.0;

fn row_h(it: &Item) -> f32 {
    match it.kind {
        Kind::Item => ROW,
        Kind::Head => HEAD_H,
        Kind::Sep => SEP_H,
    }
}

pub fn paint_dropdown(ui: &mut Ui, s: &State, menu: usize) {
    let Some(m) = MENUS.get(menu) else { return };
    let x0 = bar_layout(ui.c)[menu].0;
    let t = ui.t;

    // Pattern list column: one row per project pattern (spec: ui/menus.md, PATTERNS).
    let pats: Vec<(u16, String)> = if menu == PATTERNS {
        let mut v: Vec<(u16, String)> = s.project.patterns.iter().map(|p| (p.number, p.name.clone().unwrap_or_else(|| format!("Pattern {}", p.number)))).collect();
        if !v.iter().any(|(n, _)| *n == s.pattern) {
            v.push((s.pattern, format!("Pattern {}", s.pattern)));
        }
        v.sort_by_key(|(n, _)| *n);
        v
    } else {
        Vec::new()
    };

    let width: f32 = m.cols.iter().map(|(w, _)| w).sum::<f32>() + BORDER * 2.0;
    let col_h = |items: &[Item]| items.iter().map(row_h).sum::<f32>();
    let body_h = m
        .cols
        .iter()
        .enumerate()
        .map(|(ci, (_, items))| if menu == PATTERNS && ci == 0 { pats.len() as f32 * ROW } else { col_h(items) })
        .fold(0.0, f32::max);
    let r = Rect::new(x0 - 4.0, 78.0, width, body_h + BORDER * 2.0);
    ui.c.fill_rect(r, t.menu_pop);
    ui.c.stroke_rrect(Rect::new(r.x + 1.0, r.y + 1.0, r.w - 2.0, r.h - 2.0), 0.0, 2.0, t.menu_pop_border);
    ui.hit(r, Action::Swallow, "");

    let font = Font::new(14);
    let mut cx = r.x + BORDER;
    let mut flat = 0usize;
    for (ci, (cw, items)) in m.cols.iter().enumerate() {
        let col = Rect::new(cx, r.y + BORDER, *cw, body_h);
        if ci > 0 {
            ui.c.fill_rect(Rect::new(cx - 1.0, col.y, 1.0, body_h), t.menu_pop_head);
        }
        let mut y = col.y;
        if menu == PATTERNS && ci == 0 {
            for (n, name) in &pats {
                let rr = Rect::new(col.x, y, col.w, ROW);
                let hov = ui.hit(rr, Action::MenuItem(menu, PATTERN_PICK + *n as usize), "Select pattern");
                if hov || *n == s.pattern {
                    ui.c.fill_rect(rr, t.menu_pop_hi);
                }
                ui.c.text_clip(font, rr.x + 6.0, rr.y + 2.0, rr.w - 8.0, t.menu_pop_text, name);
                y += ROW;
            }
        }
        for item in items.iter() {
            let h = row_h(item);
            let rr = Rect::new(col.x, y, col.w, h);
            match item.kind {
                Kind::Sep => ui.c.fill_rect(Rect::new(col.x, y + 3.0, col.w, 1.0), t.menu_pop_head),
                Kind::Head => {
                    ui.c.fill_rect(rr, t.menu_pop_head);
                    ui.c.text(Font::new(12), rr.x + 4.0, rr.y + 1.0, t.menu_pop_head_text, item.label);
                }
                Kind::Item => {
                    let action = item_action(menu, flat, item);
                    let hint = if item.works { item.label } else { "Not available yet" };
                    if ui.hit(rr, action, hint) {
                        ui.c.fill_rect(rr, t.menu_pop_hi);
                    }
                    let color = if item.works { t.menu_pop_text } else { t.menu_pop_dim };
                    // Tick state: window entries follow the window, others are drawn unticked.
                    let mark = match (item.mark, menu == VIEW) {
                        (Mark::Check(_), true) => Mark::Check(win_of(item.label).is_some_and(|w| s.shown(w))),
                        (mk, _) => mk,
                    };
                    let (mx, my) = (rr.x + 10.0, rr.y + ROW / 2.0);
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
                    ui.c.text_clip(font, rr.x + 20.0, rr.y + 2.0, rr.w - 24.0, color, item.label);
                    let right = rr.right() - if item.sub { 16.0 } else { 8.0 };
                    if !item.shortcut.is_empty() {
                        ui.c.text_right(font.italic(), right, rr.y + 3.0, t.menu_pop_shortcut, item.shortcut);
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
        assert_eq!(l(FILE_EXIT), Some("Exit"));
        assert_eq!(MENUS[PATTERNS].title, "PATTERNS");
        assert_eq!(MENUS[VIEW].title, "VIEW");
    }
}
