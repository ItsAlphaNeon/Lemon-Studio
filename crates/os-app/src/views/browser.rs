//! Docked browser (left): five tabs, a tree of the project / plugin database / folders, drag sources.
// spec: ui/main-window.md §4, ui/browser.md (§4 options menu, §5 search)

use os_ui::{Color, Rect};

use crate::app::{Action, State, Ui};
use crate::icons::{self, Icon};

/// Row pitch and indent per depth level (spec: ui/browser.md §2).
pub const ROW_H: f32 = 23.0;
const INDENT: f32 = 13.0;
/// Top of the list area.
pub const LIST_Y: f32 = 142.0;
/// Footer height below the list.
pub const FOOTER_H: f32 = 28.0;

/// Category colour indices (`cat_color`); 255 = folder.
const FOLDER: u8 = 255;

/// (label, icon, category colour index)
const ROOTS: &[(&str, Icon, u8)] = &[
    ("Current project", Icon::Page, 0),
    ("Recent files", Icon::Back, 1),
    ("Plugin database", Icon::Speaker, 2),
    ("Plugin presets", Icon::Speaker, 3),
    ("Channel presets", Icon::Box, 4),
    ("Mixer presets", Icon::Mixer, 5),
    ("Scores", Icon::Note, 6),
    ("Audio", Icon::Person, 7),
    ("Backup", Icon::Back, 8),
    ("Channel envelopes", Icon::Folder, FOLDER),
    ("Clipboard files", Icon::Folder, FOLDER),
    ("Demo projects", Icon::Folder, FOLDER),
    ("Envelopes", Icon::Folder, FOLDER),
    ("Cloud", Icon::Cloud, 9),
    ("Impulses", Icon::Folder, FOLDER),
    ("Misc", Icon::Folder, FOLDER),
    ("My projects", Icon::Folder, FOLDER),
    ("Packs", Icon::Box, 2),
    ("Project bones", Icon::Folder, FOLDER),
    ("Recorded", Icon::Wave, 2),
    ("Rendered", Icon::Wave, 2),
    ("Sliced audio", Icon::Wave, 2),
    ("Soundfonts", Icon::Folder, FOLDER),
    ("Speech", Icon::Folder, FOLDER),
    ("Templates", Icon::Folder, FOLDER),
    ("User data", Icon::Person, 7),
];

/// The "Current project" tree's sections, in screenshot order (spec §2).
const PROJECT: &[(&str, Icon)] = &[
    ("History", Icon::Undo),
    ("Patterns", Icon::Note),
    ("Effects", Icon::Speaker),
    ("Generators", Icon::Plug),
    ("Remote control", Icon::MidiControl),
    ("Initialized controls", Icon::Slide),
    ("Samples", Icon::Wave),
];

/// What a row stands for; everything but `None` can be dragged (spec §3).
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    None,
    /// 1-based pattern number.
    Pattern(u16),
    /// Rack row.
    Channel(usize),
    /// Sample file path.
    Sample(String),
    /// Plugin picker generator index (built-ins, then hosted).
    Generator(usize),
    /// Plugin picker effect index.
    Effect(usize),
}

#[derive(Debug, Clone)]
pub struct Row {
    /// Open/closed state key (tab-qualified path).
    pub key: String,
    pub depth: u8,
    pub label: String,
    pub icon: Icon,
    pub cat: u8,
    /// `Some(open)` for an expandable row.
    pub open: Option<bool>,
    /// Drawn brighter (current pattern, opened roots).
    pub bright: bool,
    pub item: Item,
}

/// Keys of the rows that start opened (the project tab shows Patterns, like the screenshot).
pub fn default_open() -> std::collections::HashSet<String> {
    ["1:Current project", "1:Current project/Patterns", "2:Plugin database"].iter().map(|k| k.to_string()).collect()
}

/// Options-menu toggles (spec §4); `DEFAULT_OPTS` are on in a fresh browser.
pub const OPT_PARENTS: &str = "parents";
pub const OPT_EXPAND: &str = "expand";
pub const OPT_FOLDERS: &str = "folders";
pub const OPT_TOP: &str = "top";
pub const OPT_TYPE: &str = "type";
pub const OPT_TAGS: &str = "tags";
pub const OPT_INDEX: &str = "index";
pub const OPT_REINDEX: &str = "reindex";
pub const OPT_AUTOHIDE: &str = "autohide";
pub const OPT_HIDEDRAG: &str = "hidedrag";
pub const OPT_CONTRAST: &str = "contrast";
pub const OPT_PREVIEW: &str = "preview";
pub const DEFAULT_OPTS: &[&str] = &[OPT_FOLDERS, OPT_TAGS, OPT_INDEX, OPT_REINDEX, OPT_HIDEDRAG];

pub fn opt(s: &State, k: &str) -> bool {
    s.browser_opts.contains(k)
}

/// Whether `row` matches the search text `q` (lower case), spec §5.
pub fn matches(s: &State, row: &Row, q: &str) -> bool {
    !q.is_empty() && (opt(s, OPT_FOLDERS) || row.open.is_none()) && row.label.to_lowercase().contains(q)
}

/// A node is open when the user opened it, or always while searching (search covers the whole tree).
fn is_open(s: &State, key: &str) -> bool {
    !s.browser_search.is_empty() || s.browser_open.contains(key)
}

/// The visible rows of the active tab, top to bottom (filtered while searching, spec §5).
pub fn rows(s: &State) -> Vec<Row> {
    let all = tree(s);
    if s.browser_search.is_empty() {
        return all;
    }
    let q = s.browser_search.to_lowercase();
    let hit: Vec<bool> = all.iter().map(|r| matches(s, r, &q)).collect();
    if !opt(s, OPT_PARENTS) {
        return all.into_iter().zip(hit).filter(|(_, h)| *h).map(|(mut r, _)| {
            r.depth = 0;
            r
        }).collect();
    }
    // Keep matches and their ancestors.
    let mut keep = hit.clone();
    let mut stack: Vec<usize> = Vec::new();
    for (i, r) in all.iter().enumerate() {
        while stack.last().is_some_and(|&p| all[p].depth >= r.depth) {
            stack.pop();
        }
        if hit[i] {
            for &p in &stack {
                keep[p] = true;
            }
        }
        stack.push(i);
    }
    all.into_iter().zip(keep).filter(|(_, k)| *k).map(|(r, _)| r).collect()
}

fn tree(s: &State) -> Vec<Row> {
    let mut out = Vec::new();
    let tab = s.browser_tab;
    let roots: Vec<usize> = match tab {
        0 => (0..ROOTS.len()).collect(),
        1 => vec![0],
        2 => vec![2],
        _ => Vec::new(),
    };
    for r in roots {
        let (label, icon, cat) = ROOTS[r];
        let key = format!("{tab}:{label}");
        let open = is_open(s, &key);
        out.push(Row { key: key.clone(), depth: 0, label: label.into(), icon, cat, open: Some(open), bright: open, item: Item::None });
        if !open {
            continue;
        }
        match r {
            0 => project_rows(s, &key, cat, &mut out),
            2 => plugin_rows(s, &key, cat, &mut out),
            // Disk folders: no content ships yet (spec §2, owner decision).
            _ => {}
        }
    }
    out
}

fn section(s: &State, parent: &str, label: &str, icon: Icon, cat: u8, out: &mut Vec<Row>) -> bool {
    let key = format!("{parent}/{label}");
    let open = is_open(s, &key);
    out.push(Row { key, depth: 1, label: label.into(), icon, cat, open: Some(open), bright: open, item: Item::None });
    open
}

fn leaf(out: &mut Vec<Row>, parent: &str, label: String, icon: Icon, cat: u8, bright: bool, item: Item) {
    out.push(Row { key: format!("{parent}/{label}"), depth: 2, label, icon, cat, open: None, bright, item });
}

fn project_rows(s: &State, root: &str, cat: u8, out: &mut Vec<Row>) {
    let p = &s.project;
    for &(name, icon) in PROJECT {
        if !section(s, root, name, icon, cat, out) {
            continue;
        }
        let parent = format!("{root}/{name}");
        match name {
            "Patterns" => {
                let mut pats: Vec<_> = p.patterns.iter().collect();
                pats.sort_by_key(|p| p.number);
                for pat in pats {
                    let label = pat.name.clone().unwrap_or_else(|| format!("Pattern {}", pat.number));
                    leaf(out, &parent, label, Icon::Note, cat, pat.number == s.pattern, Item::Pattern(pat.number));
                }
            }
            "Generators" => {
                for (i, c) in p.channels.iter().enumerate() {
                    let label = c.name.clone().or_else(|| c.plugin.clone()).unwrap_or_else(|| format!("Channel {}", i + 1));
                    leaf(out, &parent, label, Icon::Plug, cat, i == s.selected_channel, Item::Channel(i));
                }
            }
            "Effects" => {
                for m in &p.mixer {
                    for slot in m.slots.iter().flatten() {
                        let track = if m.index == 0 { "Master".to_string() } else { format!("Insert {}", m.index) };
                        leaf(out, &parent, format!("{track} - {}", slot.name), Icon::Speaker, cat, false, Item::None);
                    }
                }
            }
            "Samples" => {
                for c in &p.channels {
                    if let Some(path) = c.sample_path.as_ref().filter(|p| !p.is_empty()) {
                        let file = path.rsplit(['/', '\\']).next().unwrap_or(path).to_string();
                        leaf(out, &parent, file, Icon::Wave, cat, false, Item::Sample(path.clone()));
                    }
                }
            }
            // History, Remote control, Initialized controls: nothing tracked yet (spec §2).
            _ => {}
        }
    }
}

fn plugin_rows(s: &State, root: &str, cat: u8, out: &mut Vec<Row>) {
    use crate::views::picker::{EFFECTS, GENERATORS};
    if section(s, root, "Generators", Icon::Plug, cat, out) {
        let parent = format!("{root}/Generators");
        let names = GENERATORS.iter().map(|g| g.name).chain(s.hosted_generators.iter().copied());
        for (i, n) in names.enumerate() {
            leaf(out, &parent, n.to_string(), Icon::Plug, cat, false, Item::Generator(i));
        }
    }
    if section(s, root, "Effects", Icon::Speaker, cat, out) {
        let parent = format!("{root}/Effects");
        for (i, e) in EFFECTS.iter().enumerate() {
            leaf(out, &parent, e.name.to_string(), Icon::Speaker, cat, false, Item::Effect(i));
        }
    }
}

/// Height of the list viewport for a browser of height `h` (from `WORK_Y`).
pub fn list_h(s: &State) -> f32 {
    (s.screen.1 - list_top(s) - FOOTER_H).max(0.0)
}

/// Top of the rows: below the search box when "Search box on top" is on (spec §4).
pub fn list_top(s: &State) -> f32 {
    if opt(s, OPT_TOP) { LIST_Y + SEARCH_ROW } else { LIST_Y }
}

/// Height of the search row above the list ("Search box on top").
const SEARCH_ROW: f32 = 27.0;

/// Largest useful scroll offset.
pub fn max_scroll(s: &State) -> f32 {
    (rows(s).len() as f32 * ROW_H - list_h(s)).max(0.0)
}

pub fn paint(ui: &mut Ui, s: &State, r: Rect) {
    let t = ui.t;
    ui.hit(r, Action::Swallow, "");
    // Title bar.
    ui.c.fill_rect(Rect::span(0.0, 200.0, 83.0, 105.0), t.browser_title);
    let menu_open = s.playlist.popup.as_ref().is_some_and(|p| p.x == MENU_AT.0 && p.y == MENU_AT.1);
    let mhov = ui.hit(Rect::new(2.0, 85.0, 16.0, 18.0), Action::BrowserMenu, "Browser options");
    if mhov || menu_open {
        ui.c.stroke_rrect(Rect::new(3.0, 86.0, 15.0, 16.0), 2.0, 1.2, t.browser_tab_outline);
    }
    icons::draw(ui.c, Icon::ArrowRight, Rect::new(5.0, 87.0, 12.0, 14.0), if mhov || menu_open { t.glyph } else { t.text_dim });
    icons::draw(ui.c, Icon::ArrowUp, Rect::new(19.0, 87.0, 12.0, 14.0), t.text_dim);
    icons::draw(ui.c, Icon::Back, Rect::new(34.0, 87.0, 12.0, 14.0), t.text_dim);
    ui.hit(Rect::new(17.0, 85.0, 16.0, 18.0), Action::BrowserCollapse, "Collapse all");
    ui.c.text(os_ui::Font::new(14), 52.0, 85.0, t.text_dim, "Browser");
    // Tabs (spec ui/browser.md §1).
    ui.c.fill_rect(Rect::span(0.0, 200.0, 106.0, 141.0), t.browser_tabs);
    const TIPS: [&str; 5] = ["All", "Current project", "Plugin database", "Online content", "Favorites"];
    for (i, icon) in [Icon::Wave, Icon::Page, Icon::Speaker, Icon::Globe, Icon::Star].into_iter().enumerate() {
        let cx = 20.0 + 40.0 * i as f32;
        let rr = Rect::new(cx - 18.0, 108.0, 36.0, 30.0);
        let hov = ui.hit(rr, Action::BrowserTab(i), TIPS[i]);
        if i == s.browser_tab {
            ui.c.stroke_rrect(rr, 4.0, 1.5, t.browser_tab_outline);
        }
        let c = if hov || i == s.browser_tab { t.glyph } else { t.glyph.shade(-0.1) };
        icons::draw(ui.c, icon, Rect::new(cx - 9.0, 114.0, 18.0, 18.0), c);
    }
    // List.
    let top = list_top(s);
    if top > LIST_Y {
        ui.c.fill_rect(Rect::new(0.0, LIST_Y, 201.0, SEARCH_ROW), t.browser_footer);
        search_box(ui, s, Rect::new(13.0, LIST_Y + 4.0, 163.0, 20.0));
    }
    let list = Rect::span(13.0, 200.0, top, r.bottom() - FOOTER_H);
    ui.c.fill_rect(Rect::new(0.0, top, 13.0, list.h), t.browser_tabs.shade(0.02));
    ui.c.fill_rect(list, t.browser_list);
    ui.hit(Rect::new(0.0, top, 201.0, list.h), Action::BrowserList, "");
    ui.c.push_clip(Rect::new(0.0, top, 201.0, list.h));
    let cats: [Color; 10] = [
        t.cat_project,
        t.cat_recent,
        t.cat_plugin_db,
        t.cat_plugin_presets,
        t.cat_channel_presets,
        t.cat_mixer_presets,
        t.cat_plugin_db.mix(t.glyph, 0.4),
        t.cat_recent,
        t.cat_recent,
        t.cat_plugin_db,
    ];
    let rows = rows(s);
    let scroll = s.browser_scroll.min((rows.len() as f32 * ROW_H - list.h).max(0.0)).max(0.0);
    let sel = s.browser_sel.as_deref();
    for (i, row) in rows.iter().enumerate() {
        let y = top + ROW_H * i as f32 - scroll;
        if y + ROW_H < list.y || y > list.bottom() {
            continue;
        }
        let x = 13.0 + INDENT * f32::from(row.depth);
        let rect = Rect::new(13.0, y, 188.0, ROW_H);
        let hov = ui.hit(rect, Action::BrowserRow(i), row.label.clone());
        if sel == Some(row.key.as_str()) || hov {
            ui.c.fill_rect(rect, t.browser_row_hi);
        }
        let base = if row.cat == FOLDER { t.browser_folder } else { cats[usize::from(row.cat).min(9)] };
        let c = if row.bright { base.shade(0.12) } else if row.depth > 0 { base.shade(-0.06) } else { base };
        // Expander: only an opened node shows its ▾ in the gutter (FL screenshots, spec §2).
        if row.open == Some(true) && s.browser_search.is_empty() {
            let (ax, cy) = (x - 9.0, y + ROW_H / 2.0);
            for k in 0..4 {
                let k = k as f32;
                ui.c.fill_rect(Rect::new(ax + k, cy - 2.0 + k, 7.0 - 2.0 * k, 1.0), t.text_dim);
            }
        }
        icons::draw(ui.c, row.icon, Rect::new(x + 6.0, y + 3.0, 17.0, 17.0), c);
        ui.c.text_clip(os_ui::Font::new(17), x + 25.0, y, 200.0 - x - 27.0, c, &row.label);
    }
    let empty = if !s.browser_search.is_empty() { "No matches" } else if s.browser_tab == 3 { "No online content" } else { "No favorites yet" };
    if rows.is_empty() && (s.browser_tab >= 3 || !s.browser_search.is_empty()) {
        ui.c.text(os_ui::Font::new(14).italic(), 22.0, top + 6.0, t.text_faint, empty);
    }
    ui.c.pop_clip();
    // Footer.
    let fy = r.bottom() - 27.0;
    ui.c.fill_rect(Rect::new(0.0, fy, 201.0, 27.0), t.browser_footer);
    ui.c.fill_rrect(Rect::new(13.0, fy + 4.0, 41.0, 20.0), 3.0, t.sunken);
    ui.c.text_center(os_ui::Font::condensed(11, 6).bold(), Rect::new(13.0, fy + 4.0, 41.0, 20.0), ui.t.tx(0xc0c8cc), "TAGS");
    if top == LIST_Y {
        search_box(ui, s, Rect::new(57.0, fy + 4.0, 119.0, 20.0));
    } else {
        ui.c.fill_rrect(Rect::new(57.0, fy + 4.0, 119.0, 20.0), 3.0, t.sunken);
    }
    icons::draw(ui.c, Icon::Folder, Rect::new(158.0, fy + 7.0, 14.0, 14.0), t.text_faint);
    icons::draw(ui.c, Icon::Search, Rect::new(180.0, fy + 5.0, 17.0, 17.0), t.browser_search);
    // Splitter.
    ui.c.fill_rect(Rect::new(201.0, 83.0, 6.0, r.h), t.splitter);
    ui.c.fill_rect(Rect::new(207.0, 83.0, 2.0, r.h), t.splitter_dark);
}

/// Where the options menu opens (just below the title bar's ▸, spec §4).
pub const MENU_AT: (f32, f32) = (1.0, 105.0);

/// The search field (spec §5): typed text, a caret while focused; click focuses.
fn search_box(ui: &mut Ui, s: &State, r: Rect) {
    let t = ui.t;
    ui.hit(r, Action::BrowserSearch, "Search the browser (Enter = find next, Esc = clear)");
    ui.c.fill_rrect(r, 3.0, t.sunken);
    if s.browser_search_focus {
        ui.c.stroke_rrect(r, 3.0, 1.0, t.browser_search);
    }
    let f = os_ui::Font::new(14);
    let text_w = r.w - 26.0;
    // Show the end of a long query.
    let mut shown: &str = &s.browser_search;
    while ui.c.text_width(f, shown) > text_w && !shown.is_empty() {
        let cut = shown.char_indices().nth(1).map_or(shown.len(), |(i, _)| i);
        shown = &shown[cut..];
    }
    ui.c.text_clip(f, r.x + 5.0, r.y + 1.0, text_w, t.glyph, shown);
    if s.browser_search_focus {
        let cx = r.x + 6.0 + ui.c.text_width(f, shown);
        ui.c.fill_rect(Rect::new(cx, r.y + 4.0, 1.0, r.h - 8.0), t.glyph);
    }
}

/// Rect of the drag label at mouse position (x, y).
pub fn ghost_rect(x: f32, y: f32) -> Rect {
    Rect::new(x + 12.0, y + 6.0, 190.0, 22.0)
}

/// The label that follows the cursor during a browser drag (spec §3).
pub fn paint_ghost(ui: &mut Ui, s: &State) {
    let Some((label, x, y)) = &s.browser_drag else { return };
    let t = ui.t;
    let r = ghost_rect(*x, *y);
    let f = os_ui::Font::new(15);
    let w = (ui.c.text_width(f, label) + 14.0).min(r.w);
    let r = Rect::new(r.x, r.y, w, r.h);
    ui.c.fill_rrect(r, 3.0, t.browser_row_hi.shade(0.08));
    ui.c.stroke_rrect(r, 3.0, 1.0, t.browser_tab_outline);
    ui.c.text_clip(f, r.x + 7.0, r.y + 2.0, r.w - 10.0, t.glyph, label);
}
