// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The plugin palette (Lemon Studio's design): a floating window with a search field listing the installed plugins,
//! opened where a plugin menu used to open on its own (F8, the toolbar's plugin button, the channel rack's "+", an
//! empty mixer slot, a container's Add). It lists what that menu listed and a pick does what its pick did; the window
//! keeps its size however many plugins there are.
// spec: ui/plugin-menu.md §6

use std::collections::BTreeMap;

use winit::keyboard::{Key, NamedKey};

use crate::app::{App, State, Win};
use crate::plugin_db::{self as db, Entry, MenuView};
use crate::plugin_menu::Target;
use crate::playlist::Cmd;

/// The window's size (§6.1); shorter when the workspace is.
pub const W: f32 = 380.0;
pub const H: f32 = 500.0;
/// Room kept under the window in a short workspace.
const MARGIN: f32 = 16.0;
/// Row heights: a plugin, a heading, a kind's heading.
pub const ROW_H: f32 = 24.0;
pub const HEAD_H: f32 = 22.0;
pub const MAJOR_H: f32 = 28.0;
/// Rows a wheel notch scrolls (§6.4).
const WHEEL_ROWS: f32 = 3.0;

/// Which kinds the palette lists when its target takes both (§6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    #[default]
    All,
    Instruments,
    Effects,
}

pub const KINDS: [(Kind, &str); 3] = [(Kind::All, "All"), (Kind::Instruments, "Instruments"), (Kind::Effects, "Effects")];
/// The display modes as the palette's pills name them (§6.2).
pub const VIEWS: [(MenuView, &str); 3] = [(MenuView::Categories, "Type"), (MenuView::Simple, "A-Z"), (MenuView::Vendor, "Vendor")];

/// The palette's controls (`Action::Palette`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PalAct {
    /// The search field (a click gives the window the keyboard).
    Search,
    /// The search field's clear button.
    Clear,
    Kind(Kind),
    View(MenuView),
    /// The list's empty space (the wheel scrolls it).
    List,
    /// Row `.0` of `rows`: picks its plugin.
    Row(usize),
    Scan,
    Manage,
    /// The browser's Plugin library.
    Library,
}

/// The palette's state.
#[derive(Debug, Clone, PartialEq)]
pub struct PaletteUi {
    /// Top-left.
    pub pos: (f32, f32),
    /// What it lists and where a pick goes.
    pub target: Target,
    /// The mixer track a container's pick goes to (the container window's), else the selected one.
    pub track: Option<u16>,
    pub query: String,
    pub kind: Kind,
    /// The highlighted row (an index into `rows`).
    pub sel: usize,
    /// The list's scroll, in px.
    pub scroll: f32,
}

impl Default for PaletteUi {
    fn default() -> Self {
        PaletteUi { pos: (0.0, 0.0), target: Target::All, track: None, query: String::new(), kind: Kind::All, sel: 0, scroll: 0.0 }
    }
}

/// A row of the list.
#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    /// A heading; `major` for a kind's (with both kinds listed).
    Head { label: String, major: bool },
    /// Plugin `at` of the generator list (`effect` false) or the effect list (`plugdb.lists`).
    Plugin { effect: bool, at: usize },
    /// A line of faint text: an empty kind, no match.
    Note(&'static str),
}

impl Row {
    pub fn height(&self) -> f32 {
        match self {
            Row::Head { major: true, .. } => MAJOR_H,
            Row::Head { .. } => HEAD_H,
            Row::Plugin { .. } | Row::Note(_) => ROW_H,
        }
    }
}

/// Whether the palette's target lists generators and effects (`.0`, `.1`).
fn kinds_of(target: Target) -> (bool, bool) {
    match target {
        Target::All | Target::Container(crate::container::CHANNEL) => (true, true),
        Target::Gen(_) | Target::Replace(_) | Target::MemberInst(..) | Target::Track(_) => (true, false),
        Target::Fx(_) | Target::Container(_) | Target::Member(..) => (false, true),
    }
}

/// Whether the palette's target takes both kinds (it shows the kind pills).
pub fn mixed(p: &PaletteUi) -> bool {
    kinds_of(p.target) == (true, true)
}

/// The kinds listed now: the target's, narrowed by the kind pills.
fn shown_kinds(p: &PaletteUi) -> (bool, bool) {
    let (g, f) = kinds_of(p.target);
    match p.kind {
        Kind::Instruments if g && f => (true, false),
        Kind::Effects if g && f => (false, true),
        _ => (g, f),
    }
}

/// The title (§6.1).
pub fn title(p: &PaletteUi) -> &'static str {
    match kinds_of(p.target) {
        (true, true) => "Add plugin",
        (true, false) => "Add instrument",
        _ => "Add effect",
    }
}

pub fn entry(s: &State, effect: bool, at: usize) -> Option<&Entry> {
    if effect { s.plugdb.lists.fx.get(at) } else { s.plugdb.lists.gens.get(at) }
}

/// A category as a row shows it: "Other" says which kind (§6.2).
pub fn category_text(e: &Entry, effect: bool) -> &'static str {
    match e.category {
        db::OTHER if effect => "Other effect",
        db::OTHER => "Other instrument",
        c => c,
    }
}

/// The dim text at a row's right (§6.2): the vendor in Categories, else the category.
pub fn detail(s: &State, e: &Entry, effect: bool) -> String {
    if s.plugdb.prefs.menu_view == MenuView::Categories && s.palette.query.trim().is_empty() {
        e.vendor.clone()
    } else {
        category_text(e, effect).to_string()
    }
}

/// The format a search finds a plugin by (§6.3).
fn format_text(e: &Entry) -> &'static str {
    if e.builtin {
        return "built-in";
    }
    e.format.as_str()
}

/// How well `e` matches the search (§6.3): `None` when a word is missing; else 0 (the label starts with the text) to 3
/// (only another field holds it).
fn rank(e: &Entry, effect: bool, q: &str, words: &[&str]) -> Option<u8> {
    let label = e.label.to_lowercase();
    let fields = [label.as_str(), &e.vendor.to_lowercase(), &category_text(e, effect).to_lowercase(), format_text(e)];
    if !words.iter().all(|w| fields.iter().any(|f| f.contains(w))) {
        return None;
    }
    Some(if label.starts_with(q) {
        0
    } else if label.split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(q)) {
        1
    } else if label.contains(q) {
        2
    } else {
        3
    })
}

/// The list's rows (§6.2, §6.3).
pub fn rows(s: &State) -> Vec<Row> {
    let p = &s.palette;
    let (gens, fx) = shown_kinds(p);
    let lists = &s.plugdb.lists;
    let q = p.query.trim().to_lowercase();
    if !q.is_empty() {
        let words: Vec<&str> = q.split_whitespace().collect();
        let mut hits: Vec<(u8, bool, String, Row)> = Vec::new();
        for (effect, list, on) in [(false, &lists.gens, gens), (true, &lists.fx, fx)] {
            if !on {
                continue;
            }
            for (at, e) in list.iter().enumerate() {
                if let Some(r) = rank(e, effect, &q, &words) {
                    hits.push((r, !e.favorite, e.label.to_lowercase(), Row::Plugin { effect, at }));
                }
            }
        }
        hits.sort_by(|a, b| (a.0, a.1, &a.2).cmp(&(b.0, b.1, &b.2)));
        if hits.is_empty() {
            return vec![Row::Note("No plugin matches")];
        }
        return hits.into_iter().map(|h| h.3).collect();
    }
    let mut v = Vec::new();
    for (effect, list, on) in [(false, &lists.gens, gens), (true, &lists.fx, fx)] {
        if !on {
            continue;
        }
        if gens && fx {
            v.push(Row::Head { label: if effect { "Effects" } else { "Instruments" }.into(), major: true });
        }
        if list.is_empty() {
            v.push(Row::Note(if effect { "(no effect plugins found)" } else { "(no instruments found)" }));
            continue;
        }
        let row = |at: usize| Row::Plugin { effect, at };
        let favs: Vec<usize> = (0..list.len()).filter(|&i| list[i].favorite).collect();
        let rest: Vec<usize> = (0..list.len()).filter(|&i| !list[i].favorite).collect();
        let head = |label: &str| Row::Head { label: label.into(), major: false };
        if !favs.is_empty() {
            v.push(head("Favourites"));
            v.extend(favs.iter().map(|&i| row(i)));
        }
        match s.plugdb.prefs.menu_view {
            MenuView::Simple => {
                if !favs.is_empty() && !rest.is_empty() {
                    v.push(head("Others"));
                }
                v.extend(rest.iter().map(|&i| row(i)));
            }
            MenuView::Categories => {
                for cat in db::categories(effect) {
                    let in_cat: Vec<usize> = rest.iter().copied().filter(|&i| list[i].category == cat).collect();
                    if !in_cat.is_empty() {
                        let e = &list[in_cat[0]];
                        v.push(head(category_text(e, effect)));
                        v.extend(in_cat.into_iter().map(row));
                    }
                }
            }
            MenuView::Vendor => {
                let mut by: BTreeMap<String, (String, Vec<usize>)> = BTreeMap::new();
                for &i in &rest {
                    let e = &list[i];
                    by.entry(e.vendor.to_lowercase()).or_insert_with(|| (e.vendor.clone(), Vec::new())).1.push(i);
                }
                for (_, (vendor, is)) in by {
                    v.push(head(&vendor));
                    v.extend(is.into_iter().map(row));
                }
            }
        }
    }
    v
}

/// The plugins among `rows` (what a count counts).
fn plugins(rows: &[Row]) -> usize {
    rows.iter().filter(|r| matches!(r, Row::Plugin { .. })).count()
}

/// The count at the footer's left (§6.2): "12 plugins", while searching "3 of 12".
pub fn count_text(s: &State, rows: &[Row]) -> String {
    let p = &s.palette;
    let (gens, fx) = shown_kinds(p);
    let all = if gens { s.plugdb.lists.gens.len() } else { 0 } + if fx { s.plugdb.lists.fx.len() } else { 0 };
    let unit = if all == 1 { "plugin" } else { "plugins" };
    if p.query.trim().is_empty() { format!("{all} {unit}") } else { format!("{} of {all}", plugins(rows)) }
}

/// The window's rect: its place, its height at most the workspace's less a margin.
pub fn rect(s: &State) -> os_ui::Rect {
    let work = s.work_rect();
    os_ui::Rect::new(s.palette.pos.0, s.palette.pos.1, W, height(work.h))
}

fn height(work_h: f32) -> f32 {
    H.min((work_h - MARGIN).max(crate::views::plugin_palette::MIN_H))
}

/// Whether the palette has the keyboard: it is the window in front (§6.1).
pub fn has_keys(s: &State) -> bool {
    s.z.last() == Some(&Win::PluginPalette) && !s.no_focus && s.menu_open.is_none() && s.playlist.popup.is_none() && !s.settings.open
}

/// Whether the palette is shown.
#[cfg(test)]
pub fn is_open(s: &State) -> bool {
    s.shown(Win::PluginPalette)
}

/// What picking the listed plugin `label` runs.
#[cfg(test)]
pub fn cmd_of(s: &State, label: &str) -> Option<Cmd> {
    rows(s).iter().find(|r| matches!(**r, Row::Plugin { effect, at } if entry(s, effect, at).is_some_and(|e| e.label == label))).and_then(|r| pick_cmd(s, r))
}

/// The first plugin row at or after `from` going `dir` (1 or -1), else `from` itself.
fn step(rows: &[Row], from: usize, dir: isize) -> usize {
    let mut i = from as isize;
    loop {
        i += dir;
        if i < 0 || i as usize >= rows.len() {
            return from;
        }
        if matches!(rows[i as usize], Row::Plugin { .. }) {
            return i as usize;
        }
    }
}

fn first(rows: &[Row]) -> usize {
    rows.iter().position(|r| matches!(r, Row::Plugin { .. })).unwrap_or(0)
}

/// Each row's top in the list's content (px), and the content's height.
pub fn tops(rows: &[Row]) -> (Vec<f32>, f32) {
    let mut y = 0.0;
    let tops = rows
        .iter()
        .map(|r| {
            let t = y;
            y += r.height();
            t
        })
        .collect();
    (tops, y)
}

/// Back to the first plugin, the list at its top (a new search, another kind or arrangement).
pub fn reset(s: &mut State) {
    let rows = rows(s);
    s.palette.sel = first(&rows);
    s.palette.scroll = 0.0;
}

/// The list scrolled to keep the highlighted row in view (a heading right above it comes into view with it).
fn reveal(s: &mut State, rows: &[Row]) {
    let (tops, _) = tops(rows);
    let view = crate::views::plugin_palette::list_h(s);
    let sel = s.palette.sel;
    let Some(&top) = tops.get(sel) else { return };
    let from = match sel.checked_sub(1).map(|i| &rows[i]) {
        Some(Row::Head { .. }) => tops[sel - 1],
        _ => top,
    };
    let bottom = top + rows[sel].height();
    let p = &mut s.palette;
    if from < p.scroll {
        p.scroll = from;
    } else if bottom > p.scroll + view {
        p.scroll = bottom - view;
    }
}

/// What picking plugin row `row` runs: the plugin menu's pick for the target, on the container's track for a container.
pub fn pick_cmd(s: &State, row: &Row) -> Option<Cmd> {
    let Row::Plugin { effect, at } = *row else { return None };
    let e = entry(s, effect, at)?;
    let cmd = crate::plugin_menu::pick(s.palette.target, effect, e.index);
    Some(match (s.palette.track, cmd) {
        (Some(t), c @ Cmd::Mixer(_)) => Cmd::SlotWin(t, Box::new(c)),
        (_, c) => c,
    })
}

/// Whether a Shift pick keeps the palette open for target `t`, and where the next pick goes then (§6.4).
fn after_pick(t: Target) -> Option<Target> {
    match t {
        Target::All | Target::Gen(None) => Some(t),
        // The slot is taken now: the next one goes into the first empty slot.
        Target::Fx(_) => Some(Target::Fx(None)),
        _ => None,
    }
}

impl App {
    /// Opens the plugin palette for `target` with its top-left at `at` (kept in the workspace); for a container,
    /// `track` is the container window's mixer track. A palette already open moves there and comes to the front.
    pub fn open_plugin_palette(&mut self, target: Target, at: (f32, f32), track: Option<u16>) {
        let s = &mut self.state;
        s.menu_open = None;
        s.playlist.popup = None;
        let work = s.work_rect();
        let h = height(work.h);
        let x = at.0.min(work.right() - W).max(work.x);
        let y = at.1.min(work.bottom() - h).max(work.y);
        let p = &mut s.palette;
        p.pos = (x.round(), y.round());
        p.target = target;
        p.track = track;
        p.query.clear();
        reset(s);
        s.raise(Win::PluginPalette);
        self.invalidate();
    }

    /// F8 (§6.1): closes the palette in front, else opens the all-plugins palette at the mouse.
    pub fn toggle_plugin_palette(&mut self) {
        if has_keys(&self.state) {
            self.close_plugin_palette();
        } else {
            self.open_plugin_palette(Target::All, self.mouse, None);
        }
    }

    pub fn close_plugin_palette(&mut self) {
        self.state.hide(Win::PluginPalette);
        self.invalidate();
    }

    /// The palette's controls.
    pub(crate) fn palette_act(&mut self, a: PalAct) {
        let s = &mut self.state;
        match a {
            PalAct::Search | PalAct::List => {}
            PalAct::Clear => {
                s.palette.query.clear();
                reset(s);
            }
            PalAct::Kind(k) => {
                s.palette.kind = k;
                reset(s);
            }
            PalAct::View(v) => {
                s.plugdb.prefs.menu_view = v;
                reset(s);
                self.prefs_changed();
            }
            PalAct::Row(i) => return self.palette_pick(i, self.shift),
            PalAct::Scan => {
                if s.plugdb.progress.is_none() {
                    self.start_scan(db::Mode::Changed);
                }
            }
            PalAct::Manage => self.open_settings(crate::views::settings::PLUGINS),
            PalAct::Library => {
                let fx_only = kinds_of(s.palette.target) == (false, true);
                let jump = if fx_only { crate::browser::Jump::DbEffects } else { crate::browser::Jump::PluginDb };
                if let Some(m) = self.browser_cmd(crate::browser::BCmd::Jump(jump)) {
                    self.state.message = Some(m);
                }
            }
        }
        self.damage_win(Win::PluginPalette);
        self.invalidate();
    }

    /// Picks row `i` (§6.4): adds its plugin and closes the palette; with `keep` (Shift) it stays open where the target
    /// allows another pick.
    fn palette_pick(&mut self, i: usize, keep: bool) {
        let rows = rows(&self.state);
        let Some(cmd) = rows.get(i).and_then(|r| pick_cmd(&self.state, r)) else { return };
        let next = after_pick(self.state.palette.target).filter(|_| keep && self.state.palette.track.is_none());
        if let Some(m) = crate::playlist::run(self, cmd) {
            self.say(m);
        }
        match next {
            Some(t) => {
                let p = &mut self.state.palette;
                p.target = t;
                p.sel = i;
                self.state.raise(Win::PluginPalette);
            }
            None => self.state.hide(Win::PluginPalette),
        }
        self.invalidate();
    }

    /// The wheel over the list: three rows a notch (§6.4).
    pub(crate) fn palette_wheel(&mut self, notches: f32) {
        let s = &mut self.state;
        let (_, total) = tops(&rows(s));
        let max = (total - crate::views::plugin_palette::list_h(s)).max(0.0);
        s.palette.scroll = (s.palette.scroll - notches * WHEEL_ROWS * ROW_H).clamp(0.0, max);
    }

    /// The palette's keys while it has the keyboard (§6.1, §6.4); returns whether the key was taken. Typed text goes
    /// into the search; shortcuts with Ctrl or Alt (but AltGr's characters) and the function keys are left to the app.
    pub(crate) fn palette_key(&mut self, key: &Key, ctrl: bool, alt: bool) -> bool {
        if !has_keys(&self.state) {
            return false;
        }
        let shift = self.shift;
        let rows = rows(&self.state);
        let page = ((crate::views::plugin_palette::list_h(&self.state) / ROW_H).floor() as usize).max(1);
        let s = &mut self.state;
        let p = &mut s.palette;
        let typing = !(ctrl ^ alt);
        match key {
            Key::Named(NamedKey::Escape) if p.query.is_empty() => {
                self.close_plugin_palette();
                return true;
            }
            Key::Named(NamedKey::Escape) => {
                p.query.clear();
                reset(s);
            }
            Key::Named(NamedKey::Enter) => {
                let sel = p.sel;
                self.palette_pick(sel, shift);
                return true;
            }
            Key::Named(NamedKey::ArrowDown) => p.sel = step(&rows, p.sel, 1),
            Key::Named(NamedKey::ArrowUp) => p.sel = step(&rows, p.sel, -1),
            Key::Named(NamedKey::PageDown) => p.sel = (0..page).fold(p.sel, |i, _| step(&rows, i, 1)),
            Key::Named(NamedKey::PageUp) => p.sel = (0..page).fold(p.sel, |i, _| step(&rows, i, -1)),
            Key::Named(NamedKey::Home) if !ctrl => p.sel = first(&rows),
            Key::Named(NamedKey::End) if !ctrl => p.sel = rows.iter().rposition(|r| matches!(r, Row::Plugin { .. })).unwrap_or(p.sel),
            // Tab: the next kind, with both kinds listed.
            Key::Named(NamedKey::Tab) if mixed(p) => {
                let at = KINDS.iter().position(|k| k.0 == p.kind).unwrap_or(0);
                let n = KINDS.len();
                p.kind = KINDS[if shift { (at + n - 1) % n } else { (at + 1) % n }].0;
                reset(s);
            }
            Key::Named(NamedKey::Backspace) if ctrl => {
                p.query.clear();
                reset(s);
            }
            Key::Named(NamedKey::Backspace) => {
                p.query.pop();
                reset(s);
            }
            Key::Named(NamedKey::Space) if typing => {
                p.query.push(' ');
                reset(s);
            }
            Key::Character(c) if typing && !c.chars().any(char::is_control) => {
                p.query.push_str(c);
                reset(s);
            }
            _ => return false,
        }
        let rows = self::rows(&self.state);
        reveal(&mut self.state, &rows);
        self.damage_win(Win::PluginPalette);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;

    fn app() -> App {
        let mut app = App::new(None);
        app.plugins = crate::plugins::Plugins::builtins();
        for name in ["Test Synth", "Bass Drum"] {
            app.plugins.available.push(db::fake(name, os_host::Kind::Instrument));
        }
        for name in ["Space Reverb", "Delay Lama"] {
            app.plugins.effects.push(db::fake(name, os_host::Kind::Effect));
        }
        app.state.plugdb.rebuild(&app.plugins);
        app
    }

    fn type_text(app: &mut App, t: &str) {
        for c in t.chars() {
            let k = if c == ' ' { Key::Named(NamedKey::Space) } else { Key::Character(c.to_string().into()) };
            app.key(&k, false, false);
        }
    }

    fn labels(app: &App) -> Vec<String> {
        rows(&app.state).iter().filter_map(|r| match *r {
            Row::Plugin { effect, at } => entry(&app.state, effect, at).map(|e| e.label.clone()),
            _ => None,
        }).collect()
    }

    /// F8 opens the palette with the keyboard in its search; typing filters both kinds, ranked by where the text is
    /// found; Esc clears, Esc again closes; F8 closes it too (spec §6.1, §6.3).
    #[test]
    fn typing_searches_and_escape_clears_then_closes() {
        let mut app = app();
        app.key(&Key::Named(NamedKey::F8), false, false);
        assert!(is_open(&app.state) && has_keys(&app.state));
        assert!(rows(&app.state).iter().any(|r| *r == Row::Head { label: "Instruments".into(), major: true }));
        type_text(&mut app, "re");
        assert_eq!(app.state.palette.query, "re");
        let found = labels(&app);
        // "Reverb" starts a word of the label; "Delay Lama" holds no "re" (not in its label, vendor or category).
        assert!(found.iter().any(|l| l == "Space Reverb"));
        assert!(!found.iter().any(|l| l == "Delay Lama"));
        assert!(!rows(&app.state).iter().any(|r| matches!(r, Row::Head { .. })), "results have no headings");
        app.key(&Key::Named(NamedKey::Escape), false, false);
        assert!(app.state.palette.query.is_empty() && is_open(&app.state));
        app.key(&Key::Named(NamedKey::Escape), false, false);
        assert!(!is_open(&app.state));
        app.key(&Key::Named(NamedKey::F8), false, false);
        app.key(&Key::Named(NamedKey::F8), false, false);
        assert!(!is_open(&app.state));
    }

    /// Every word must be found somewhere; the label's start ranks first (§6.3).
    #[test]
    fn search_words_and_ranking() {
        let mut app = app();
        app.open_plugin_palette(Target::All, (0.0, 0.0), None);
        type_text(&mut app, "delay");
        assert_eq!(labels(&app).first().map(String::as_str), Some("Delay Lama"));
        type_text(&mut app, " xyz");
        assert_eq!(rows(&app.state), vec![Row::Note("No plugin matches")]);
    }

    /// The letters typed while the palette has the keyboard never reach the app's shortcuts (L flips PAT / SONG) or
    /// the typing keyboard.
    #[test]
    fn letters_go_to_the_search_not_the_shortcuts() {
        let mut app = app();
        let song = app.state.song_mode;
        app.open_plugin_palette(Target::All, (0.0, 0.0), None);
        type_text(&mut app, "l");
        assert_eq!(app.state.song_mode, song);
        assert_eq!(app.state.palette.query, "l");
        assert!(!app.typing_key(winit::keyboard::KeyCode::KeyZ, true, false, false), "the typing keyboard leaves it alone");
    }

    /// Down / Enter picks the highlighted row: a generator goes to a new channel and the palette closes; Shift+Enter on
    /// an empty slot's palette adds and keeps it open, the next pick going into the first empty slot (§6.4).
    #[test]
    fn enter_picks_and_shift_keeps_it_open() {
        let mut app = app();
        app.open_plugin_palette(Target::Gen(None), (0.0, 0.0), None);
        type_text(&mut app, "test synth");
        let n = app.state.project.channels.len();
        app.key(&Key::Named(NamedKey::Enter), false, false);
        assert_eq!(app.state.project.channels.len(), n + 1);
        assert!(!is_open(&app.state));

        crate::mixer::select(&mut app.state, 1, false);
        app.open_plugin_palette(Target::Fx(Some(3)), (0.0, 0.0), None);
        type_text(&mut app, "space reverb");
        app.shift = true;
        app.key(&Key::Named(NamedKey::Enter), false, false);
        app.shift = false;
        assert!(is_open(&app.state), "Shift keeps it open");
        assert_eq!(crate::mixer::slot_name(&app.state, 1, 3).as_deref(), Some("Space Reverb"));
        assert_eq!(app.state.palette.target, Target::Fx(None));
        app.key(&Key::Named(NamedKey::Enter), false, false);
        assert_eq!(crate::mixer::slot_name(&app.state, 1, 0).as_deref(), Some("Space Reverb"), "the first empty slot");
        assert!(!is_open(&app.state));
    }

    /// The kind pills narrow a mixed list; Tab steps through them; an effects-only target has none (§6.2).
    #[test]
    fn kinds_narrow_the_list() {
        let mut app = app();
        app.open_plugin_palette(Target::All, (0.0, 0.0), None);
        app.palette_act(PalAct::Kind(Kind::Effects));
        assert!(rows(&app.state).iter().all(|r| !matches!(r, Row::Plugin { effect: false, .. })));
        app.key(&Key::Named(NamedKey::Tab), false, false);
        assert_eq!(app.state.palette.kind, Kind::All);
        app.palette_act(PalAct::Kind(Kind::Instruments));
        app.open_plugin_palette(Target::Fx(None), (0.0, 0.0), None);
        assert!(!mixed(&app.state.palette));
        assert!(rows(&app.state).iter().all(|r| !matches!(r, Row::Plugin { effect: false, .. })), "the kind pills do not apply");
        assert_eq!(title(&app.state.palette), "Add effect");
    }

    /// The highlight skips headings and stays in view; the window stays its own size with hundreds of plugins and
    /// keeps inside the workspace (§6.1, §6.4).
    #[test]
    fn many_plugins_scroll_inside_a_fixed_window() {
        let mut app = app();
        for i in 0..400 {
            app.plugins.available.push(db::fake(&format!("Many Synth {i:03}"), os_host::Kind::Instrument));
        }
        app.state.plugdb.rebuild(&app.plugins);
        app.state.screen = (1366.0, 768.0);
        app.open_plugin_palette(Target::Gen(None), (1300.0, 700.0), None);
        let r = rect(&app.state);
        let work = app.state.work_rect();
        assert!(r.right() <= work.right() && r.bottom() <= work.bottom() && r.h <= H, "{r:?}");
        let rows0 = rows(&app.state);
        assert!(matches!(rows0[app.state.palette.sel], Row::Plugin { .. }));
        for _ in 0..60 {
            app.key(&Key::Named(NamedKey::ArrowDown), false, false);
        }
        let rows1 = rows(&app.state);
        assert!(matches!(rows1[app.state.palette.sel], Row::Plugin { .. }));
        let (tops, _) = tops(&rows1);
        let top = tops[app.state.palette.sel];
        let view = crate::views::plugin_palette::list_h(&app.state);
        assert!(top >= app.state.palette.scroll && top + ROW_H <= app.state.palette.scroll + view, "in view");
        app.key(&Key::Named(NamedKey::End), false, false);
        app.palette_wheel(-1000.0);
        assert!(app.state.palette.scroll > 0.0);
    }
}
