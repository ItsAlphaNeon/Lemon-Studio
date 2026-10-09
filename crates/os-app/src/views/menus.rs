//! Menu bar contents and the dropdown.
//!
//! Top-level menus and their positions come from the spec; the item lists follow the public
//! user manual's menu chapters and are provisional until captured (spec open question).
// spec: ui/main-window.md §3

use os_ui::{Face, Rect};

use crate::app::{Action, Ui};

pub struct Item {
    pub label: &'static str,
    pub shortcut: &'static str,
    /// Implemented in this build.
    pub works: bool,
    /// Draw a separator above.
    pub sep: bool,
}

const fn it(label: &'static str, shortcut: &'static str, works: bool, sep: bool) -> Item {
    Item { label, shortcut, works, sep }
}

pub const FILE_NEW: usize = 0;
pub const FILE_OPEN: usize = 2;
pub const FILE_SAVE: usize = 4;
pub const FILE_SAVE_AS: usize = 5;
pub const FILE_EXIT: usize = 12;

/// (title, x0, x1, items) — title spans measured at 1920×1080.
pub const MENUS: &[(&str, &[Item])] = &[
    (
        "FILE",
        &[
            it("New", "Ctrl+N", true, false),
            it("New from template", "", false, false),
            it("Open...", "Ctrl+O", true, true),
            it("Open recent", "", false, false),
            it("Save", "Ctrl+S", true, true),
            it("Save as...", "Ctrl+Shift+S", true, false),
            it("Save new version", "Ctrl+N", false, false),
            it("Import", "", false, true),
            it("Export", "", false, false),
            it("Project info...", "", false, true),
            it("Project bones", "", false, false),
            it("Revert to last backup", "", false, false),
            it("Exit", "Alt+F4", true, true),
        ],
    ),
    (
        "EDIT",
        &[
            it("Undo", "Ctrl+Z", false, false),
            it("Undo history", "", false, false),
            it("Cut", "Ctrl+X", false, true),
            it("Copy", "Ctrl+C", false, false),
            it("Paste", "Ctrl+V", false, false),
            it("Delete", "Del", false, false),
            it("Insert", "Ins", false, true),
            it("Edit events", "", false, false),
        ],
    ),
    (
        "ADD",
        &[
            it("Generator", "", false, false),
            it("Effect", "", false, false),
            it("Pattern", "F4", false, true),
            it("Automation clip", "", false, false),
            it("Audio clip", "", false, false),
            it("More plugins...", "", false, true),
        ],
    ),
    (
        "PATTERNS",
        &[
            it("Find first empty", "F4", false, false),
            it("Rename, color and icon", "F2", false, false),
            it("Clone", "Ctrl+Shift+C", false, false),
            it("Delete", "", false, false),
            it("Split by channel", "", false, true),
            it("Move up", "", false, true),
            it("Move down", "", false, false),
        ],
    ),
    (
        "VIEW",
        &[
            it("Playlist", "F5", true, false),
            it("Channel rack", "F6", true, false),
            it("Piano roll", "F7", true, false),
            it("Mixer", "F9", true, false),
            it("Browser", "Alt+F8", true, false),
            it("Plugin picker", "F8", false, true),
            it("Event editor", "", false, false),
            it("Close all windows", "F12", false, true),
            it("Layouts", "", false, false),
        ],
    ),
    (
        "OPTIONS",
        &[
            it("MIDI settings", "F10", false, false),
            it("Audio settings", "", false, false),
            it("General settings", "", false, false),
            it("File settings", "", false, false),
            it("Theme settings", "", false, false),
            it("Project general settings", "", false, true),
            it("Metronome", "Ctrl+M", false, true),
            it("Typing keyboard to piano", "Ctrl+T", false, false),
            it("Multilink to controllers", "Ctrl+J", false, false),
        ],
    ),
    (
        "TOOLS",
        &[
            it("Macros", "", false, false),
            it("Quick quantize", "Ctrl+Q", false, true),
            it("Tap tempo", "", false, false),
            it("Dump score log to pattern", "", false, true),
            it("One-click audio recording", "", false, false),
        ],
    ),
    (
        "HELP",
        &[
            it("Help index", "F1", false, false),
            it("Keyboard shortcuts", "", false, false),
            it("Check for updates", "", false, true),
            it("About", "", false, true),
        ],
    ),
];

/// Menu bar item layout: (x0, x1, font size). The titles are spread over the measured panel
/// (x 8–292) by their text widths, so they never overlap whatever font is in use.
pub fn bar_layout(c: &mut os_ui::Canvas) -> [(f32, f32, f32); 8] {
    const GAP: f32 = 9.0;
    const SPAN: f32 = 284.0;
    let base = 16.0;
    let widths: Vec<f32> = MENUS.iter().map(|(t, _)| c.text_width(Face::Ui, base, t)).collect();
    let total: f32 = widths.iter().sum::<f32>() + GAP * 7.0;
    let k = ((SPAN - GAP * 7.0) / (total - GAP * 7.0)).min(1.15);
    let mut out = [(0.0, 0.0, base); 8];
    let mut x = 8.0;
    for (i, w) in widths.iter().enumerate() {
        let w = w * k;
        out[i] = (x, x + w, base * k);
        x += w + GAP;
    }
    out
}

/// Map VIEW menu items to their window actions; other menus use `MenuItem`.
fn item_action(menu: usize, i: usize) -> Action {
    use crate::app::Win;
    if menu == 4 {
        let win = [Some(Win::Playlist), Some(Win::ChannelRack), Some(Win::PianoRoll), Some(Win::Mixer), Some(Win::Browser)];
        if let Some(Some(w)) = win.get(i) {
            return Action::ToggleWin(*w);
        }
    }
    Action::MenuItem(menu, i)
}

pub fn paint_dropdown(ui: &mut Ui, menu: usize) {
    let Some((_, items)) = MENUS.get(menu) else { return };
    let x0 = bar_layout(ui.c)[menu].0;
    let t = ui.t;
    let row = 22.0;
    let seps = items.iter().filter(|i| i.sep).count() as f32;
    let width = 260.0;
    let r = Rect::new(x0 - 4.0, 78.0, width, items.len() as f32 * row + seps * 7.0 + 8.0);
    ui.c.fill_rect(Rect::new(r.x + 3.0, r.y + 3.0, r.w, r.h), t.toolbar_line.with_alpha(160));
    ui.c.fill_rect(r, t.menu_drop);
    ui.c.stroke_rrect(r, 0.0, 1.0, t.toolbar_line);
    ui.hit(r, Action::Swallow, "");
    let mut y = r.y + 4.0;
    for (i, item) in items.iter().enumerate() {
        if item.sep {
            ui.c.line(r.x + 8.0, y + 3.5, r.right() - 8.0, y + 3.5, 1.0, t.toolbar_line);
            y += 7.0;
        }
        let rr = Rect::new(r.x + 2.0, y, r.w - 4.0, row);
        let hint = if item.works { item.label } else { "Not available yet" };
        let hov = ui.hit(rr, item_action(menu, i), hint);
        if hov {
            ui.c.fill_rect(rr, t.menu_drop_hi);
        }
        let c = if item.works { t.menu_drop_text } else { t.menu_drop_disabled };
        ui.c.text(Face::Ui, 13.0, rr.x + 22.0, rr.y + 5.0, c, item.label);
        if !item.shortcut.is_empty() {
            ui.c.text_right(Face::Ui, 12.0, rr.right() - 10.0, rr.y + 5.5, t.menu_drop_disabled, item.shortcut);
        }
        y += row;
    }
}
