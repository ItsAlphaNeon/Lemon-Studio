// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The toolbar's layout: which panels its rows hold and in which order, the editor that changes it and the presets
//! that set it (spec: ui/toolbar-menu.md §8). The panels paint in `views::toolbar`. Each was measured where the classic
//! layout has it (its home, `Panel::home`) and paints wherever its row and place put it, moved by whole band px (`At`).
//!
//! App-wide, kept in `settings.txt` (`toolbar_layout`: the layout's text, lines joined by `; `). A user preset is a
//! `.lstoolbar` text file in the user folder's `Toolbar presets`: `row=` lines of panel keys, top row first, and
//! `logo=1` or `logo=0`.

use std::path::{Path, PathBuf};

use os_ui::Rect;

/// A row's height in band px (row 1's top edge to row 2's).
pub const ROW_H: f32 = 41.0;
/// Rows a layout has at most.
pub const MAX_ROWS: usize = 2;
/// The fixed space's width.
pub const SPACE_W: f32 = 20.0;
/// What the logo keeps free at the band's right end when flexible spaces share the rest.
pub const LOGO_ROOM: f32 = 92.0;
/// While editing, each row starts with a grip this wide.
pub const GRIP_W: f32 = 10.0;
/// While editing, a flexible space is at least this wide, to be taken hold of.
pub const FLEX_EDIT_W: f32 = 16.0;
/// The preset file's extension.
pub const EXT: &str = "lstoolbar";
/// The settings key of the layout.
const KEY: &str = "toolbar_layout";

/// What the toolbar can hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Panel {
    Caption,
    Hint,
    Transport,
    Recording,
    Time,
    Monitor,
    Cpu,
    Utilities,
    Menu,
    Volume,
    Pitch,
    SongPos,
    Modes,
    Snap,
    Pattern,
    Windows,
    Tools,
    /// A fixed space (`SPACE_W`).
    Space,
    /// A space that grows: the flexible spaces of a row share what the window leaves over.
    Flex,
}

/// Every panel, in the palette's order.
pub const PANELS: [Panel; 19] = [
    Panel::Menu,
    Panel::Hint,
    Panel::Caption,
    Panel::Transport,
    Panel::Recording,
    Panel::Time,
    Panel::Monitor,
    Panel::Cpu,
    Panel::Utilities,
    Panel::Volume,
    Panel::Pitch,
    Panel::SongPos,
    Panel::Modes,
    Panel::Snap,
    Panel::Pattern,
    Panel::Windows,
    Panel::Tools,
    Panel::Space,
    Panel::Flex,
];

impl Panel {
    /// Its name in files.
    pub fn key(self) -> &'static str {
        match self {
            Panel::Caption => "caption",
            Panel::Hint => "hint",
            Panel::Transport => "transport",
            Panel::Recording => "recording",
            Panel::Time => "time",
            Panel::Monitor => "monitor",
            Panel::Cpu => "cpu",
            Panel::Utilities => "utilities",
            Panel::Menu => "menu",
            Panel::Volume => "volume",
            Panel::Pitch => "pitch",
            Panel::SongPos => "song-position",
            Panel::Modes => "modes",
            Panel::Snap => "snap",
            Panel::Pattern => "pattern",
            Panel::Windows => "windows",
            Panel::Tools => "tools",
            Panel::Space => "space",
            Panel::Flex => "flex",
        }
    }

    pub fn from_key(k: &str) -> Option<Panel> {
        PANELS.into_iter().find(|p| p.key() == k)
    }

    /// Its name in the editor.
    pub fn name(self) -> &'static str {
        match self {
            Panel::Caption => "Caption buttons",
            Panel::Hint => "Hint panel",
            Panel::Transport => "Transport",
            Panel::Recording => "Recording options",
            Panel::Time => "Time display",
            Panel::Monitor => "Output monitor",
            Panel::Cpu => "CPU and memory",
            Panel::Utilities => "Utility buttons",
            Panel::Menu => "Menu bar",
            Panel::Volume => "Main volume",
            Panel::Pitch => "Main pitch",
            Panel::SongPos => "Song position",
            Panel::Modes => "Mode buttons",
            Panel::Snap => "Global snap",
            Panel::Pattern => "Pattern picker",
            Panel::Windows => "Window buttons",
            Panel::Tools => "Pickers and tools",
            Panel::Space => "Space",
            Panel::Flex => "Flexible space",
        }
    }

    /// The spaces can be on the toolbar any number of times, the rest once.
    pub fn repeats(self) -> bool {
        matches!(self, Panel::Space | Panel::Flex)
    }

    /// Where the classic layout has it: its row and its left and right edges (band px). Each separator line sits at the
    /// left edge of the panel it comes with (`separator`). The spaces have no home: row 1 from x 0.
    pub fn home(self) -> (usize, f32, f32) {
        match self {
            Panel::Caption => (0, 0.0, 68.0),
            Panel::Hint => (0, 68.0, 379.0),
            Panel::Transport => (0, 379.0, 575.0),
            Panel::Recording => (0, 575.0, 743.0),
            Panel::Time => (0, 743.0, 876.0),
            Panel::Monitor => (0, 876.0, 1048.0),
            Panel::Cpu => (0, 1048.0, 1204.0),
            Panel::Utilities => (0, 1204.0, 1440.0),
            Panel::Menu => (1, 0.0, 298.0),
            Panel::Volume => (1, 298.0, 339.0),
            Panel::Pitch => (1, 339.0, 378.0),
            Panel::SongPos => (1, 378.0, 576.0),
            Panel::Modes => (1, 576.0, 743.0),
            Panel::Snap => (1, 743.0, 834.0),
            Panel::Pattern => (1, 834.0, 967.0),
            Panel::Windows => (1, 967.0, 1136.0),
            Panel::Tools => (1, 1136.0, 1270.0),
            Panel::Space => (0, 0.0, SPACE_W),
            Panel::Flex => (0, 0.0, 0.0),
        }
    }

    /// Its width when it does not grow.
    pub fn width(self) -> f32 {
        let (_, x0, x1) = self.home();
        x1 - x0
    }

    /// Where (home x) its first part ends: the panel, or its first button, shows when that fits (`views::toolbar::room`).
    pub fn first_end(self) -> f32 {
        match self {
            Panel::Caption => 63.0,
            Panel::Hint => 376.0,
            Panel::Transport => 570.0,
            Panel::Recording => 610.0,
            Panel::Time => 873.0,
            Panel::Monitor => 1045.0,
            Panel::Cpu => 1202.0,
            Panel::Utilities => 1239.0,
            Panel::Menu => 296.0,
            Panel::Volume => 337.0,
            Panel::Pitch => 376.0,
            Panel::SongPos => 575.0,
            Panel::Modes => 610.0,
            Panel::Snap => 830.0,
            Panel::Pattern => 962.0,
            Panel::Windows => 1003.0,
            Panel::Tools => 1168.0,
            Panel::Space | Panel::Flex => 0.0,
        }
    }

    /// Whether a group separator line comes before it (at its left edge), as the classic layout has them (spec:
    /// ui/toolbar-menu.md §2).
    pub fn separator(self) -> bool {
        matches!(self, Panel::Transport | Panel::Recording | Panel::Monitor | Panel::Cpu | Panel::Utilities | Panel::Volume | Panel::SongPos | Panel::Modes | Panel::Windows)
    }

    /// The home panel whose slot holds band point (x, y) of the classic layout, for what is laid out in home px (the
    /// constants of `app`, `song_pos`, `patterns`).
    pub fn at_home(x: f32, y: f32) -> Option<Panel> {
        let row = usize::from(y >= ROW_H);
        PANELS.into_iter().filter(|p| !p.repeats()).find(|p| {
            let (r, x0, x1) = p.home();
            r == row && x >= x0 && x < x1
        })
    }
}

/// How far a panel is drawn from its home, in whole band px: what it paints and registers at home x / y goes to `x(..)`
/// / `y(..)`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct At {
    pub dx: f32,
    pub dy: f32,
}

impl At {
    pub fn x(self, x: f32) -> f32 {
        x + self.dx
    }
    pub fn y(self, y: f32) -> f32 {
        y + self.dy
    }
    pub fn r(self, r: Rect) -> Rect {
        Rect::new(r.x + self.dx, r.y + self.dy, r.w, r.h)
    }
    pub fn span(self, x0: f32, x1: f32, y0: f32, y1: f32) -> Rect {
        self.r(Rect::span(x0, x1, y0, y1))
    }
}

/// The toolbar's rows and whether the logo shows at the band's right end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub rows: Vec<Vec<Panel>>,
    pub logo: bool,
}

impl Default for Layout {
    fn default() -> Self {
        Layout::classic()
    }
}

impl Layout {
    /// The layout of spec ui/main-window.md §1-3.
    pub fn classic() -> Layout {
        PRESETS[0].layout()
    }

    /// The panels' band height, its bottom edge excepted: 81 px for two rows, 40 for one.
    pub fn height(&self) -> f32 {
        ROW_H * self.rows.len().max(1) as f32 - 1.0
    }

    /// The band's height with its 2 px bottom edge (`views::band_h` at Main): 83 px for two rows.
    pub fn band_h(&self) -> f32 {
        self.height() + 2.0
    }

    /// Top and bottom of row `r` (band px): rows meet at a 1 px line.
    pub fn row_span(&self, r: usize) -> (f32, f32) {
        let top = if r == 0 { 0.0 } else { ROW_H * r as f32 + 1.0 };
        let bottom = if r + 1 >= self.rows.len() { self.height() } else { ROW_H * (r + 1) as f32 };
        (top, bottom)
    }

    pub fn has(&self, p: Panel) -> bool {
        self.rows.iter().flatten().any(|&q| q == p)
    }

    /// The text kept in files: a `row=` line per row, top first, with its panels' keys, and the logo switch.
    pub fn text(&self) -> String {
        let mut s = String::new();
        for row in &self.rows {
            let keys: Vec<&str> = row.iter().map(|p| p.key()).collect();
            s.push_str(&format!("row={}\n", keys.join(" ")));
        }
        s.push_str(if self.logo { "logo=1\n" } else { "logo=0\n" });
        s
    }

    /// A layout from its text (lines, or `;`-separated as in `settings.txt`). Unknown keys and panels are skipped, rows
    /// past `MAX_ROWS` and second copies of a panel (but the spaces) dropped; `None` without a row.
    pub fn parse(text: &str) -> Option<Layout> {
        let mut rows: Vec<Vec<Panel>> = Vec::new();
        let mut logo = false;
        for line in text.split(['\n', ';']) {
            let Some((k, v)) = line.split_once('=') else { continue };
            match k.trim() {
                "row" if rows.len() < MAX_ROWS => rows.push(v.split_whitespace().filter_map(Panel::from_key).collect()),
                "logo" => logo = v.trim() == "1",
                _ => {}
            }
        }
        if rows.is_empty() {
            return None;
        }
        let mut seen = Vec::new();
        for row in &mut rows {
            row.retain(|&p| {
                let first = p.repeats() || !seen.contains(&p);
                seen.push(p);
                first
            });
        }
        Some(Layout { rows, logo })
    }

    /// The layout kept in `settings.txt`; the classic one when there is none or it cannot be read.
    pub fn load() -> Layout {
        crate::settings_file::get(KEY).and_then(|v| Layout::parse(&v)).unwrap_or_else(Layout::classic)
    }

    /// Keeps the layout for the next start.
    pub fn save(&self) {
        crate::settings_file::write(KEY, self.text().trim_end().replace('\n', "; ").as_str());
    }

    /// One or two rows. Panels of rows taken away go to the end of the last row left; a new row is empty.
    pub fn set_rows(&mut self, n: usize) {
        let n = n.clamp(1, MAX_ROWS);
        while self.rows.len() > n {
            let gone = self.rows.pop().unwrap_or_default();
            if let Some(last) = self.rows.last_mut() {
                last.extend(gone);
            }
        }
        while self.rows.len() < n {
            self.rows.push(Vec::new());
        }
    }

    /// The factory preset with this layout, if any.
    pub fn factory(&self) -> Option<usize> {
        PRESETS.iter().position(|p| p.layout() == *self)
    }
}

/// A panel as laid out: its row, its index in the row, its left edge and width (band px).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    pub panel: Panel,
    pub row: usize,
    pub idx: usize,
    pub x: f32,
    pub w: f32,
}

impl Placed {
    /// How far it is from its home.
    pub fn at(&self) -> At {
        let (row, x0, _) = self.panel.home();
        At { dx: self.x - x0, dy: ROW_H * (self.row as f32 - row as f32) }
    }

    /// Its slot: its width, its row's height.
    pub fn rect(&self, l: &Layout) -> Rect {
        let (top, bottom) = l.row_span(self.row);
        Rect::new(self.x, top, self.w, bottom - top)
    }
}

/// Where rows start and how narrow a flexible space gets: the editor's grips and spaces to take hold of.
fn edit_sizes(editing: bool) -> (f32, f32) {
    if editing { (GRIP_W, FLEX_EDIT_W) } else { (0.0, 0.0) }
}

/// Row `row`'s width with its flexible spaces as narrow as they get.
fn row_width(row: &[Panel], editing: bool) -> f32 {
    let (lead, flex) = edit_sizes(editing);
    lead + row.iter().map(|&p| if p == Panel::Flex { flex } else { p.width() }).sum::<f32>()
}

/// The panels of `l` laid out in a band `width` px wide, each row after its grip while editing. The flexible spaces of
/// a row share what the band leaves over (whole px), the logo's room excepted.
pub fn place(l: &Layout, width: f32, editing: bool) -> Vec<Placed> {
    let (lead, min_flex) = edit_sizes(editing);
    let avail = width - if l.logo { LOGO_ROOM } else { 0.0 };
    let mut out = Vec::new();
    for (r, row) in l.rows.iter().enumerate() {
        let flexes = row.iter().filter(|&&p| p == Panel::Flex).count();
        let share = if flexes == 0 { 0.0 } else { ((avail - row_width(row, editing)).max(0.0) / flexes as f32).floor() };
        let mut x = lead;
        for (idx, &panel) in row.iter().enumerate() {
            let w = if panel == Panel::Flex { min_flex + share } else { panel.width() };
            out.push(Placed { panel, row: r, idx, x, w });
            x += w;
        }
    }
    out
}

/// The band width from which every panel shows: the longest row, its flexible spaces as narrow as they get.
pub fn natural_width(l: &Layout, editing: bool) -> f32 {
    l.rows.iter().map(|row| row_width(row, editing)).fold(edit_sizes(editing).0, f32::max)
}

/// A factory preset (spec: ui/toolbar-menu.md §8).
pub struct Preset {
    pub name: &'static str,
    pub rows: &'static [&'static [Panel]],
    pub logo: bool,
}

impl Preset {
    pub fn layout(&self) -> Layout {
        Layout { rows: self.rows.iter().map(|r| r.to_vec()).collect(), logo: self.logo }
    }
}

use Panel::*;

/// The factory presets, in the Toolbars menu's order.
pub const PRESETS: [Preset; 8] = [
    Preset {
        name: "Classic",
        rows: &[&[Caption, Hint, Transport, Recording, Time, Monitor, Cpu, Utilities], &[Menu, Volume, Pitch, SongPos, Modes, Snap, Pattern, Windows, Tools]],
        logo: true,
    },
    Preset {
        name: "Hint first",
        rows: &[&[Hint, Volume, Transport, Recording, Time, Monitor, Cpu, Utilities, Flex, Caption], &[Menu, Pitch, SongPos, Modes, Snap, Pattern, Windows, Tools]],
        logo: false,
    },
    Preset {
        name: "Menu first",
        rows: &[&[Menu, Volume, Transport, Recording, Time, Monitor, Cpu, Utilities, Flex, Caption], &[Hint, Pitch, SongPos, Modes, Snap, Pattern, Windows, Tools]],
        logo: false,
    },
    Preset {
        name: "Menu first, caption left",
        rows: &[&[Caption, Menu, Volume, Transport, Recording, Time, Monitor, Cpu, Utilities], &[Hint, Pitch, SongPos, Modes, Snap, Pattern, Windows, Tools]],
        logo: true,
    },
    Preset {
        name: "Streamlined",
        rows: &[&[Menu, Transport, Time, Recording, Monitor, Cpu, Flex, Caption], &[Hint, Windows, Tools, Snap, Pattern]],
        logo: false,
    },
    Preset { name: "Compact", rows: &[&[Menu, Transport, Time, Pattern, Flex, Caption], &[Hint, SongPos, Windows, Snap]], logo: false },
    Preset { name: "One row", rows: &[&[Menu, Hint, Transport, Time, Monitor, Pattern, Snap, Windows, Cpu, Flex, Caption]], logo: false },
    Preset { name: "One row, centered", rows: &[&[Menu, Flex, Transport, Time, Monitor, Pattern, Windows, Flex, Hint, Caption]], logo: false },
];

/// Where the user's presets are: the user folder's `Toolbar presets` (in tests the thread's folder, `app::theme_dir`).
pub fn user_dir() -> Option<PathBuf> {
    crate::app::theme_dir().map(|d| d.join("Toolbar presets"))
}

/// A preset as the menus name it: a factory one by its place in `PRESETS`, the user's by its file.
#[derive(Debug, Clone, PartialEq)]
pub enum PresetId {
    Factory(usize),
    User(PathBuf),
}

/// A preset the user saved.
#[derive(Debug, Clone, PartialEq)]
pub struct UserPreset {
    pub name: String,
    pub path: PathBuf,
    pub layout: Layout,
}

/// The user's presets, by name: every `.lstoolbar` in `user_dir` that holds a layout.
pub fn user_presets() -> Vec<UserPreset> {
    let Some(dir) = user_dir() else { return Vec::new() };
    let Ok(rd) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut v: Vec<UserPreset> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case(EXT)))
        .filter_map(|path| {
            let layout = Layout::parse(&std::fs::read_to_string(&path).ok()?)?;
            Some(UserPreset { name: preset_name(&path), path, layout })
        })
        .collect();
    v.sort_by_key(|p| p.name.to_lowercase());
    v
}

/// A preset file's name as listed: its file name without the extension.
pub fn preset_name(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Writes `l` to `path` (its extension added when missing).
pub fn write_preset(path: &Path, l: &Layout) -> std::io::Result<PathBuf> {
    let path = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case(EXT)) { path.to_path_buf() } else { path.with_extension(EXT) };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, l.text())?;
    Ok(path)
}

/// The editor's state, never kept: whether it is open and the user's presets as last read.
#[derive(Debug, Clone, Default)]
pub struct Bar {
    pub layout: Layout,
    pub editing: bool,
    pub user: Vec<UserPreset>,
}

impl Bar {
    pub fn load() -> Bar {
        Bar { layout: Layout::load(), editing: false, user: user_presets() }
    }

    /// The user preset with the current layout, if any.
    pub fn user_current(&self) -> Option<&UserPreset> {
        self.user.iter().find(|p| p.layout == self.layout)
    }

    /// The name the editor shows for the layout: its preset's, else "Custom".
    pub fn name(&self) -> String {
        match (self.layout.factory(), self.user_current()) {
            (Some(i), _) => PRESETS[i].name.to_string(),
            (None, Some(p)) => p.name.clone(),
            (None, None) => "Custom".to_string(),
        }
    }
}

/// What the editor's controls are (`app::Action::Bar`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarHit {
    /// A panel on the toolbar: row, index.
    Item(usize, usize),
    /// A row's grip.
    Grip(usize),
    /// The logo on the toolbar.
    Logo,
    /// A palette tile.
    Tile(Tile),
    /// The palette's preset name: its menu.
    Presets,
    /// The rows switch.
    Rows(usize),
    /// Ends editing.
    Done,
    /// The palette elsewhere (a tile already on the toolbar, the background): nothing.
    Palette,
}

/// A palette tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tile {
    Panel(Panel),
    Logo,
}

/// What an editor drag carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moving {
    /// A panel on the toolbar: row, index.
    Item(usize, usize),
    /// A palette tile, onto the toolbar.
    Tile(Tile),
    /// The logo, off the toolbar.
    Logo,
    /// A whole row, by its grip: it swaps places with the row it is dropped on.
    Row(usize),
}

/// An editor drag: what it carries and where the mouse holds it (band px from its top-left).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BarDrag {
    pub moving: Moving,
    pub grab: (f32, f32),
}

/// Where a drag would land.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Before item `idx` of row `row` (the dragged item left out of the count).
    Insert { row: usize, idx: usize },
    /// Off the toolbar (onto the palette).
    Remove,
    /// Nowhere: the drag is let go.
    Nothing,
}

/// Where a drag released at band point (x, y) lands, the palette from `palette_top` down to `palette_bottom`.
pub fn drop_at(b: &Bar, width: f32, d: &BarDrag, (x, y): (f32, f32), palette: (f32, f32)) -> Target {
    let l = &b.layout;
    if (0.0..l.band_h()).contains(&y) {
        let row = ((y / ROW_H) as usize).min(l.rows.len().saturating_sub(1));
        if let Moving::Row(_) = d.moving {
            return Target::Insert { row, idx: 0 };
        }
        let skip = match d.moving {
            Moving::Item(r, i) => Some((r, i)),
            _ => None,
        };
        let idx = place(l, width, b.editing).iter().filter(|p| p.row == row && Some((p.row, p.idx)) != skip && p.x + p.w / 2.0 < x).count();
        return Target::Insert { row, idx };
    }
    if (palette.0..palette.1).contains(&y) {
        return Target::Remove;
    }
    Target::Nothing
}

/// Applies drag `d` dropped at `to`; whether the layout changed.
pub fn apply(l: &mut Layout, d: &BarDrag, to: Target) -> bool {
    let before = l.clone();
    match (d.moving, to) {
        (Moving::Item(r, i), Target::Insert { row, idx }) if i < l.rows.get(r).map_or(0, Vec::len) => {
            let p = l.rows[r].remove(i);
            let idx = idx.min(l.rows[row].len());
            l.rows[row].insert(idx, p);
        }
        (Moving::Item(r, i), Target::Remove) if i < l.rows.get(r).map_or(0, Vec::len) => {
            l.rows[r].remove(i);
        }
        (Moving::Tile(Tile::Panel(p)), Target::Insert { row, idx }) if p.repeats() || !l.has(p) => {
            let idx = idx.min(l.rows[row].len());
            l.rows[row].insert(idx, p);
        }
        (Moving::Tile(Tile::Logo), Target::Insert { .. }) => l.logo = true,
        (Moving::Logo, Target::Remove) => l.logo = false,
        (Moving::Row(r), Target::Insert { row, .. }) if r < l.rows.len() => l.rows.swap(r, row),
        _ => {}
    }
    *l != before
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The classic layout puts every panel at its home and is 1440 px wide: the toolbar of spec ui/main-window.md.
    #[test]
    fn classic_is_at_home() {
        let l = Layout::classic();
        assert_eq!((natural_width(&l, false), l.band_h(), l.logo), (1440.0, 83.0, true));
        for p in place(&l, 1920.0, false) {
            assert_eq!(p.at(), At::default(), "{:?}", p.panel);
        }
        // The slots meet: each row's panels follow one another without gaps.
        for row in &l.rows {
            for w in row.windows(2) {
                assert_eq!(w[0].home().2, w[1].home().1, "{w:?}");
            }
        }
        assert_eq!((Panel::at_home(100.0, 20.0), Panel::at_home(837.0, 75.0), Panel::at_home(4.0, 78.0)), (Some(Panel::Hint), Some(Panel::Pattern), Some(Panel::Menu)));
    }

    #[test]
    fn text_round_trip() {
        for p in &PRESETS {
            let l = p.layout();
            assert_eq!(Layout::parse(&l.text()), Some(l.clone()), "{}", p.name);
            assert_eq!(Layout::parse(&l.text().replace('\n', "; ")), Some(l), "{}", p.name);
        }
        // Unknown panels and keys are skipped, second copies and a third row dropped; spaces repeat.
        let l = Layout::parse("name=x\nrow=menu wobble hint space space menu\nrow=time\nrow=cpu\nlogo=0").expect("a layout");
        assert_eq!(l, Layout { rows: vec![vec![Panel::Menu, Panel::Hint, Panel::Space, Panel::Space], vec![Panel::Time]], logo: false });
        assert_eq!(Layout::parse("logo=1"), None);
    }

    /// Every factory preset holds each panel once at most and fits its rows; the names are Lemon Studio's own.
    #[test]
    fn presets_are_valid() {
        for p in &PRESETS {
            let l = p.layout();
            assert!((1..=MAX_ROWS).contains(&l.rows.len()), "{}", p.name);
            assert_eq!(Layout::parse(&l.text()).as_ref(), Some(&l), "{}: no copies", p.name);
            assert!(!p.name.contains("FL") && !p.name.contains("Studio"), "{}", p.name);
        }
        assert_eq!(PRESETS.iter().filter(|p| p.rows.len() == 1).count(), 2);
    }

    /// Flexible spaces share the band left over (the logo's room excepted when it shows), in whole px.
    #[test]
    fn flexible_spaces_share_the_rest() {
        let l = PRESETS[7].layout();
        let p = place(&l, 1920.0, false);
        let flex: Vec<&Placed> = p.iter().filter(|p| p.panel == Panel::Flex).collect();
        let natural = natural_width(&l, false);
        assert_eq!(flex.len(), 2);
        assert_eq!(flex[0].w, ((1920.0 - natural) / 2.0).floor());
        assert_eq!(flex[0].w, flex[1].w);
        let last = p.last().expect("caption");
        assert!(last.x + last.w <= 1920.0 && last.x + last.w > 1918.0, "{last:?}");
        // Too narrow: nothing to share.
        assert!(place(&l, 900.0, false).iter().filter(|p| p.panel == Panel::Flex).all(|p| p.w == 0.0));
    }

    /// Dragging moves a panel within and between rows, takes it off onto the palette and puts a tile back.
    #[test]
    fn drops_move_remove_and_add() {
        let mut b = Bar { layout: Layout::classic(), editing: true, user: Vec::new() };
        let palette = (b.layout.band_h(), 400.0);
        // The hint panel (row 1, second) dropped right of the transport: after it.
        let d = BarDrag { moving: Moving::Item(0, 1), grab: (5.0, 5.0) };
        let to = drop_at(&b, 1920.0, &d, (600.0, 20.0), palette);
        assert_eq!(to, Target::Insert { row: 0, idx: 2 });
        assert!(apply(&mut b.layout, &d, to));
        assert_eq!(&b.layout.rows[0][..3], &[Panel::Caption, Panel::Transport, Panel::Hint]);
        // To row 2's start.
        let d = BarDrag { moving: Moving::Item(0, 2), grab: (5.0, 5.0) };
        let to = drop_at(&b, 1920.0, &d, (12.0, 60.0), palette);
        assert_eq!(to, Target::Insert { row: 1, idx: 0 });
        apply(&mut b.layout, &d, to);
        assert_eq!((b.layout.rows[1][0], b.layout.rows[0].len()), (Panel::Hint, 7));
        // Onto the palette: off the toolbar.
        let d = BarDrag { moving: Moving::Item(1, 0), grab: (5.0, 5.0) };
        assert_eq!(drop_at(&b, 1920.0, &d, (300.0, 200.0), palette), Target::Remove);
        apply(&mut b.layout, &d, Target::Remove);
        assert!(!b.layout.has(Panel::Hint));
        // Its tile back at row 1's end; a tile of a panel already there changes nothing.
        let d = BarDrag { moving: Moving::Tile(Tile::Panel(Panel::Hint)), grab: (0.0, 0.0) };
        assert!(apply(&mut b.layout, &d, Target::Insert { row: 0, idx: 99 }));
        assert_eq!(b.layout.rows[0].last(), Some(&Panel::Hint));
        assert!(!apply(&mut b.layout, &d, Target::Insert { row: 1, idx: 0 }));
        // The logo off and on; the rows swapped by a grip; below the palette: nothing.
        assert!(apply(&mut b.layout, &BarDrag { moving: Moving::Logo, grab: (0.0, 0.0) }, Target::Remove) && !b.layout.logo);
        assert!(apply(&mut b.layout, &BarDrag { moving: Moving::Tile(Tile::Logo), grab: (0.0, 0.0) }, Target::Insert { row: 0, idx: 0 }) && b.layout.logo);
        let top = b.layout.rows[0].clone();
        assert!(apply(&mut b.layout, &BarDrag { moving: Moving::Row(0), grab: (0.0, 0.0) }, Target::Insert { row: 1, idx: 0 }));
        assert_eq!(b.layout.rows[1], top);
        assert_eq!(drop_at(&b, 1920.0, &d, (300.0, 500.0), palette), Target::Nothing);
    }

    #[test]
    fn rows_come_and_go() {
        let mut l = Layout::classic();
        l.set_rows(1);
        assert_eq!((l.rows.len(), l.rows[0].len(), l.band_h(), l.row_span(0)), (1, 17, 42.0, (0.0, 40.0)));
        l.set_rows(2);
        assert_eq!((l.rows[1].len(), l.band_h(), l.row_span(1)), (0, 83.0, (42.0, 81.0)));
    }
}
