// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The editor's logic, platform independent: input in logical px (the layout's coordinates), parameter edits through
//! [`Model`], and actions for the window glue ([`Act`]: menus, type-in boxes, preset files, the pointer lock).

use std::path::{Path, PathBuf};

use os_ui::kit::menu::{Mark, MenuItem};
use os_ui::Canvas;

use crate::library::{self, Chip, Favorites, Library, Source};
use crate::macros::{Macro, MacroDrag};
use crate::pages;
use crate::params::{Model, ParamTable};
use crate::view::{self, Current, Hit, Target, ViewIn, ViewOut};

/// Pixels of drag for a knob's whole range.
pub const KNOB_PX: f32 = 200.0;
/// Ctrl: this much finer.
pub const FINE: f32 = 0.1;
/// Part of the range a wheel notch moves (at least one step).
const WHEEL_PART: f32 = 1.0 / 50.0;
/// Pixels the Advanced page scrolls per wheel notch.
const ADV_WHEEL: f32 = 48.0;
/// List rows per wheel notch.
const LIST_WHEEL: f32 = 3.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Btn {
    Left,
    Right,
    Middle,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

/// What a type-in box sets.
#[derive(Clone, Debug, PartialEq)]
pub enum TypeTarget {
    Param(String),
    Macro(Macro),
    /// The name to save the current sound under.
    PresetName,
}

/// A menu to show: its rows and which is current. Picking row `i` of a parameter's menu sets it to `min + i`.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuReq {
    pub key: String,
    pub items: Vec<MenuItem>,
    pub current: Option<usize>,
}

/// What the window glue does for the editor.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Repaint,
    /// Hide and hold the pointer while a button is down (knob drags).
    Lock,
    /// Open a menu at (x, y) (logical px).
    Menu(MenuReq, f32, f32),
    TypeIn(TypeTarget, String, f32, f32),
    /// Load this preset file (the plugin reads it; `loaded` / `load_failed` report back).
    Load(PathBuf),
    /// Save the current sound as a user preset at this path (then `saved`).
    Save(PathBuf),
    /// The page changed (the glue tells the host).
    Page(usize),
}

#[derive(Clone, Debug, PartialEq)]
enum Drag {
    /// A parameter: its key, the drag position (0..1), pixels for the range, across (both axes) or up only.
    Param { key: String, v: f32, px: f32, across: bool },
    Macro { mc: Macro, v: f32, g: MacroDrag },
    /// A scroll bar: where the drag started (pointer y, scroll position).
    ListBar { y0: f32, s0: f32 },
    AdvBar { y0: f32, s0: f32 },
}

/// The editor's state and logic.
pub struct Ui {
    pub table: ParamTable,
    pub lib: Library,
    pub favs: Favorites,
    sources: Vec<Source>,
    pub page: usize,
    /// When a turned parameter's hint goes (`reveal`).
    revealed: Option<std::time::Instant>,
    pub hosted: bool,
    /// Layout size (logical px, at least the base size).
    pub size: (f32, f32),
    pub mouse: Option<(f32, f32)>,
    pub pressed: Option<Target>,
    pub hint: String,
    /// A message for the hint well while nothing is hovered (loaded, saved, errors).
    pub status: String,
    pub query: String,
    pub search_focus: bool,
    pub chip: Chip,
    pub list_scroll: f32,
    pub adv_scroll: f32,
    pub current: Current,
    /// The presets listed now (library indices).
    pub shown: Vec<usize>,
    pub caret: bool,
    /// The visualizer, moved on by the glue from the sound's band levels (`Viz::step`).
    pub viz: crate::viz::Viz,
    /// The chips the ⋯ menu offers, in its order (`more_menu`).
    more: Vec<(Chip, String)>,
    out: ViewOut,
    drag: Option<Drag>,
}

/// `MenuReq::key` of the ⋯ menu (no parameter has it).
const MORE_MENU: &str = "\u{1}more";

impl Ui {
    /// An editor over `table`, its library scanned from `sources`, favourites from `favs_file` (and groups from the
    /// `groups.tsv` beside it).
    pub fn new(table: ParamTable, sources: Vec<Source>, favs_file: PathBuf) -> Ui {
        let mut lib = Library::scan_cached(&sources);
        let favs = Favorites::load(favs_file);
        lib.load_groups(&favs.file.with_file_name("groups.tsv"));
        // the first bank is the tab the browser opens on
        let chip = lib.banks.first().map(|b| Chip::Group(b.0.clone())).unwrap_or_default();
        let mut ui = Ui {
            table,
            lib,
            favs,
            sources,
            page: pages::MAIN,
            revealed: None,
            hosted: false,
            size: (view::BASE_W, view::BASE_H),
            mouse: None,
            pressed: None,
            hint: String::new(),
            status: String::new(),
            query: String::new(),
            search_focus: false,
            chip,
            list_scroll: 0.0,
            adv_scroll: 0.0,
            current: Current::default(),
            shown: Vec::new(),
            caret: true,
            viz: Default::default(),
            more: Vec::new(),
            out: ViewOut::default(),
            drag: None,
        };
        ui.refilter();
        ui
    }

    /// The default sources and favourites file (Zest's user folder, the Diva presets folder).
    pub fn with_defaults(table: ParamTable) -> Ui {
        Ui::new(table, library::default_sources(), Favorites::default_file())
    }

    pub fn hits(&self) -> &[Hit] {
        &self.out.hits
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Paints into `c` (whose logical size is `self.size`).
    pub fn paint(&mut self, c: &mut Canvas, m: &dyn Model) {
        let hint = self.line().to_string();
        let v = ViewIn {
            table: &self.table,
            lib: &self.lib,
            shown: &self.shown,
            favs: &self.favs,
            current: &self.current,
            page: self.page,
            hosted: self.hosted,
            size: self.size,
            mouse: self.mouse,
            pressed: self.pressed.as_ref(),
            query: &self.query,
            search_focus: self.search_focus,
            chip: &self.chip,
            list_scroll: self.list_scroll,
            adv_scroll: self.adv_scroll,
            hint: &hint,
            caret: self.caret,
            viz: &self.viz,
        };
        self.out = view::paint(c, &v, m);
        self.clamp_scrolls();
    }

    fn clamp_scrolls(&mut self) {
        let max = (self.shown.len() as f32 - self.out.list_rows).max(0.0);
        self.list_scroll = self.list_scroll.clamp(0.0, max);
        let amax = (self.out.adv_height - self.out.adv.h).max(0.0);
        self.adv_scroll = self.adv_scroll.clamp(0.0, amax);
    }

    fn hit(&self, x: f32, y: f32) -> Option<Target> {
        self.out.hits.iter().rev().find(|h| h.r.contains(x, y)).map(|h| h.t.clone())
    }

    fn hit_rect(&self, t: &Target) -> Option<os_ui::Rect> {
        self.out.hits.iter().rev().find(|h| &h.t == t).map(|h| h.r)
    }

    // ---------------------------------------------------------------------------------------------------------------
    // Presets

    /// Applies the filter (search words, chip) to the library.
    pub fn refilter(&mut self) {
        self.shown = self.lib.filter(&self.query, &self.chip, &self.favs);
    }

    /// Rescans the sources (after a save).
    pub fn rescan(&mut self) {
        self.lib = Library::scan(&self.sources);
        self.lib.load_groups(&self.favs.file.with_file_name("groups.tsv"));
        self.refilter();
        if let Some(p) = self.current.path.clone() {
            self.set_current(Some(&p), None);
        }
    }

    /// Shows `path` as the current preset (also one the library does not hold), with the author from the file's
    /// metadata when given.
    pub fn set_current(&mut self, path: Option<&Path>, meta_author: Option<String>) {
        let Some(path) = path else {
            self.current = Current::default();
            return;
        };
        self.current = match self.lib.find(path) {
            Some(i) => {
                let p = &self.lib.presets[i];
                let author = p.author_name.clone().or_else(|| p.author.clone());
                Current { path: Some(path.into()), name: p.name.clone(), place: p.place(), author, favourite: false, modified: false }
            }
            None => {
                let file = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let (a, n) = library::split_initials(&file);
                Current { path: Some(path.into()), name: n.into(), place: String::new(), author: a.map(str::to_string), favourite: false, modified: false }
            }
        };
        if let Some(a) = meta_author.filter(|a| !a.trim().is_empty()) {
            self.current.author = Some(a);
        }
        self.current.favourite = self.favs.contains(path);
        if let Some(i) = self.lib.find(path) {
            self.reveal(i);
        }
    }

    pub fn current_path(&self) -> Option<&Path> {
        self.current.path.as_deref()
    }

    /// Starts loading library preset `i`.
    fn load(&mut self, i: usize) -> Vec<Act> {
        let path = self.lib.presets[i].path.clone();
        self.set_current(Some(&path), None);
        self.status = format!("Loaded {}", self.lib.presets[i].file);
        self.reveal(i);
        vec![Act::Load(path), Act::Repaint]
    }

    /// The plugin loaded `path`; `author` from the file's metadata if it has one.
    pub fn loaded(&mut self, path: &Path, author: Option<String>) {
        self.set_current(Some(path), author);
    }

    pub fn load_failed(&mut self, path: &Path, err: &str) {
        self.status = format!("Could not load {}: {err}", path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
    }

    /// The plugin saved the current sound at `path`.
    pub fn saved(&mut self, path: &Path) {
        self.rescan();
        self.set_current(Some(path), None);
        if let Some(i) = self.lib.find(path) {
            self.reveal(i);
        }
        self.status = format!("Saved {}", path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
    }

    pub fn save_failed(&mut self, err: &str) {
        self.status = format!("Could not save: {err}");
    }

    /// Scrolls the list so library preset `i` shows.
    fn reveal(&mut self, i: usize) {
        if let Some(k) = self.shown.iter().position(|&j| j == i) {
            let rows = self.out.list_rows.max(1.0);
            let k = k as f32;
            if k < self.list_scroll {
                self.list_scroll = k;
            } else if k >= self.list_scroll + rows {
                self.list_scroll = k - rows + 1.0;
            }
        }
    }

    /// Shows chip `c`'s presets.
    fn select_chip(&mut self, c: Chip) {
        self.chip = c;
        self.list_scroll = 0.0;
        self.refilter();
        if let Some(i) = self.current.path.as_deref().and_then(|p| self.lib.find(p)) {
            self.reveal(i);
        }
    }

    /// The chips of the row (`view::ROW_KINDS` kinds after the collections), and those the ⋯ menu offers: the other
    /// kinds, the user's own presets, the explore groups.
    pub fn more_chips(lib: &Library) -> Vec<(Chip, String)> {
        let mut v: Vec<(Chip, String)> = lib.kinds().into_iter().skip(view::ROW_KINDS).map(|k| (Chip::Kind(k), k.label().to_string())).collect();
        if lib.presets.iter().any(|p| p.user) {
            v.push((Chip::Category(library::USER.to_string()), "Your presets".into()));
        }
        v.extend(lib.groups.iter().map(|g| (Chip::Group(g.0.clone()), g.0.clone())));
        v
    }

    /// The ⋯ menu, when it has anything.
    fn more_menu(&mut self) -> Option<MenuReq> {
        self.more = Ui::more_chips(&self.lib);
        if self.more.is_empty() {
            return None;
        }
        let current = self.more.iter().position(|(c, _)| *c == self.chip);
        let items = self.more.iter().enumerate().map(|(i, (_, l))| MenuItem::item(l).mark(Mark::Radio(Some(i) == current))).collect();
        Some(MenuReq { key: MORE_MENU.into(), items, current })
    }

    /// The previous / next preset of the list (wrapping).
    pub fn step(&mut self, d: i32) -> Vec<Act> {
        if self.shown.is_empty() {
            return vec![];
        }
        let n = self.shown.len() as i32;
        let at = self.current.path.as_deref().and_then(|p| self.lib.find(p)).and_then(|i| self.shown.iter().position(|&j| j == i));
        let k = match at {
            Some(k) => (k as i32 + d).rem_euclid(n),
            None if d > 0 => 0,
            None => n - 1,
        };
        self.load(self.shown[k as usize])
    }

    fn toggle_favourite(&mut self) {
        let Some(p) = self.current.path.clone() else { return };
        match self.favs.toggle(&p) {
            Ok(on) => {
                self.current.favourite = on;
                self.status = if on { "Added to favourites".into() } else { "Removed from favourites".into() };
            }
            Err(e) => self.status = format!("Could not write favourites: {e}"),
        }
        if self.chip == Chip::Favorites {
            self.refilter();
        }
    }

    // ---------------------------------------------------------------------------------------------------------------
    // Edits

    fn edited(&mut self) {
        self.current.modified = true;
    }

    fn edit(&mut self, m: &mut dyn Model, key: &str, plain: f32) {
        m.edit(key, plain);
        self.edited();
    }

    fn reset(&mut self, m: &mut dyn Model, t: &Target) {
        match t {
            Target::Macro(mc) => {
                let g = mc.begin(m, &self.table);
                g.set(m, mc.value_of(mc.default_norm(&self.table)));
                mc.end(m);
                self.edited();
            }
            t => {
                if let Some(d) = t.key().and_then(|k| self.table.get(k)).map(|p| p.default) {
                    let k = t.key().unwrap().to_string();
                    self.edit(m, &k, d);
                }
            }
        }
    }

    /// What the hint line says: the hint, or the last message while nothing is hovered.
    pub fn line(&self) -> &str {
        if self.hint.is_empty() { &self.status } else { &self.hint }
    }

    /// The hint for a target: name and value.
    pub fn hint_for(&self, t: Option<&Target>, m: &dyn Model) -> String {
        let Some(t) = t else { return String::new() };
        match t {
            Target::Macro(mc) => format!("{}: {}", mc.name(), mc.text(m)),
            Target::Preset(i) => {
                let p = &self.lib.presets[*i];
                let about: Vec<&str> = p.kind.map(|k| k.label()).into_iter().chain(p.author_name.as_deref()).collect();
                if about.is_empty() { p.name.clone() } else { format!("{}  ({})", p.name, about.join(", ")) }
            }
            Target::Star => if self.current.favourite { "Remove from favourites" } else { "Add to favourites" }.into(),
            Target::Prev => "Previous preset (also the up arrow, or the wheel over the screen)".into(),
            Target::Next => "Next preset (also the down arrow, or the wheel over the screen)".into(),
            Target::Save => "Save the sound as a preset of your own".into(),
            Target::Search => "Search presets by name, kind or author".into(),
            Target::Chip(Chip::All) => "Every preset".into(),
            Target::Chip(Chip::Favorites) => "Favourites".into(),
            Target::Chip(Chip::Category(c)) => c.clone(),
            Target::Chip(Chip::Group(g)) if self.lib.banks.iter().any(|b| b.0 == *g) => format!("{g}: Zest's own selection"),
            Target::Chip(Chip::Group(g)) => format!("{g} (explore group)"),
            Target::Chip(Chip::Kind(k)) => format!("{} sounds", k.label()),
            Target::More => "More: other kinds, your own presets, explore groups".into(),
            Target::Tab(i) => pages::PAGES[*i].to_string(),
            Target::ListBar | Target::AdvBar => String::new(),
            t => {
                let key = t.key().unwrap_or("");
                let v = m.get(key);
                let text = m.text(key, v).unwrap_or_else(|| self.table.get(key).map(|p| p.format(v)).unwrap_or_default());
                format!("{}: {}", self.hint_name(key), text)
            }
        }
    }

    /// A parameter's name for the hint line, its section first where the name alone is vague.
    pub fn hint_name(&self, key: &str) -> String {
        let Some(p) = self.table.get(key) else { return key.into() };
        let section = match p.group.as_str() {
            "Oscillators" => "Osc",
            "Envelope 1" => "Env 1",
            "Envelope 2" => "Env 2",
            "Quality and spread" | "Voice" | "Main" | "Matrix" => "",
            "Effect 1" => "Effect 1",
            "Effect 2" => "Effect 2",
            g => g,
        };
        if section.is_empty() || p.name.starts_with(section) {
            p.name.clone()
        } else {
            format!("{section} {}", p.name)
        }
    }

    // ---------------------------------------------------------------------------------------------------------------
    // Input

    fn begin_param(&mut self, m: &mut dyn Model, key: &str, across: bool, t: Target) -> Vec<Act> {
        let Some(p) = self.table.get(key) else { return vec![] };
        let v = p.norm(m.get(key));
        m.begin(key);
        self.drag = Some(Drag::Param { key: key.into(), v, px: KNOB_PX, across });
        self.pressed = Some(t);
        vec![Act::Repaint, Act::Lock]
    }

    pub fn down(&mut self, x: f32, y: f32, btn: Btn, mods: Mods, dbl: bool, m: &mut dyn Model) -> Vec<Act> {
        self.mouse = Some((x, y));
        let t = self.hit(x, y);
        if t != Some(Target::Search) {
            self.search_focus = false;
        }
        let Some(t) = t else { return vec![Act::Repaint] };
        let mut acts = vec![Act::Repaint];
        let editable = matches!(t, Target::Knob(_) | Target::Value(_) | Target::Macro(_));
        if editable && (dbl || btn == Btn::Middle || (btn == Btn::Left && mods.alt)) {
            self.reset(m, &t);
        } else if editable && btn == Btn::Right {
            let (r, text) = match &t {
                Target::Macro(mc) => (self.hit_rect(&t), mc.text(m).trim_end_matches(" ct").to_string()),
                t => {
                    let k = t.key().unwrap_or("");
                    (self.hit_rect(t), self.table.get(k).map(|p| p.format(m.get(k))).unwrap_or_default())
                }
            };
            let r = r.unwrap_or(os_ui::Rect::new(x, y, 0.0, 0.0));
            let tt = match &t {
                Target::Macro(mc) => TypeTarget::Macro(*mc),
                t => TypeTarget::Param(t.key().unwrap_or("").into()),
            };
            acts.push(Act::TypeIn(tt, text, r.x, r.bottom()));
        } else if btn == Btn::Left {
            match &t {
                Target::Knob(k) => acts = self.begin_param(m, &k.clone(), false, t.clone()),
                Target::Value(k) => acts = self.begin_param(m, &k.clone(), true, t.clone()),
                Target::Macro(mc) => {
                    let g = mc.begin(m, &self.table);
                    self.drag = Some(Drag::Macro { mc: *mc, v: mc.norm(m), g });
                    self.pressed = Some(t.clone());
                    acts = vec![Act::Repaint, Act::Lock];
                }
                Target::Toggle(k) => {
                    let on = m.on(k);
                    let k = k.clone();
                    self.edit(m, &k, if on { 0.0 } else { 1.0 });
                }
                Target::Choice(k, i) => {
                    let min = self.table.get(k).map_or(0.0, |p| p.min);
                    let k = k.clone();
                    self.edit(m, &k, min + *i as f32);
                }
                Target::Menu(k) => {
                    if let Some(req) = self.menu_for(k, m) {
                        let r = self.hit_rect(&t).unwrap_or(os_ui::Rect::new(x, y, 0.0, 0.0));
                        acts.push(Act::Menu(req, r.x, r.bottom()));
                    }
                }
                Target::Tab(i) => {
                    self.page = *i;
                    acts.push(Act::Page(*i));
                }
                Target::Chip(c) => self.select_chip(c.clone()),
                Target::More => {
                    let r = self.hit_rect(&t).unwrap_or(os_ui::Rect::new(x, y, 0.0, 0.0));
                    if let Some(req) = self.more_menu() {
                        acts.push(Act::Menu(req, r.x, r.bottom()));
                    }
                }
                Target::Preset(i) => acts.extend(self.load(*i)),
                Target::Star => self.toggle_favourite(),
                Target::Prev => acts.extend(self.step(-1)),
                Target::Next => acts.extend(self.step(1)),
                Target::Save => {
                    let r = self.hit_rect(&t).unwrap_or(os_ui::Rect::new(x, y, 0.0, 0.0));
                    let name = if self.current.path.as_deref().is_some_and(|p| self.lib.find(p).is_some_and(|i| self.lib.presets[i].user)) { self.current.name.clone() } else { String::new() };
                    acts.push(Act::TypeIn(TypeTarget::PresetName, name, r.x - 120.0, r.bottom()));
                }
                Target::Search => {
                    self.search_focus = true;
                    self.caret = true;
                }
                Target::ListBar => {
                    self.drag = Some(Drag::ListBar { y0: y, s0: self.list_scroll });
                    self.pressed = Some(t.clone());
                }
                Target::AdvBar => {
                    self.drag = Some(Drag::AdvBar { y0: y, s0: self.adv_scroll });
                    self.pressed = Some(t.clone());
                }
            }
        }
        self.hint = self.hint_for(Some(&t), m);
        acts
    }

    /// The pointer moved by (dx, dy) during a locked drag (logical px).
    pub fn delta(&mut self, dx: f32, dy: f32, mods: Mods, m: &mut dyn Model) -> Vec<Act> {
        let f = if mods.ctrl { FINE } else { 1.0 };
        match self.drag.clone() {
            Some(Drag::Param { key, v, px, across }) => {
                let moved = if across { dx - dy } else { -dy };
                let v = (v + moved * f / px).clamp(0.0, 1.0);
                if let Some(p) = self.table.get(&key) {
                    m.set(&key, p.plain(v));
                }
                self.drag = Some(Drag::Param { key, v, px, across });
                self.edited();
            }
            Some(Drag::Macro { mc, v, g }) => {
                let v = (v - dy * f / KNOB_PX).clamp(0.0, 1.0);
                g.set(m, mc.value_of(v));
                self.drag = Some(Drag::Macro { mc, v, g });
                self.edited();
            }
            _ => return vec![],
        }
        self.hint = self.hint_for(self.pressed.as_ref(), m);
        vec![Act::Repaint]
    }

    pub fn up(&mut self, m: &mut dyn Model) -> Vec<Act> {
        match self.drag.take() {
            Some(Drag::Param { key, .. }) => m.end(&key),
            Some(Drag::Macro { mc, .. }) => mc.end(m),
            _ => {}
        }
        self.pressed = None;
        vec![Act::Repaint]
    }

    pub fn moved(&mut self, x: f32, y: f32, m: &dyn Model) -> Vec<Act> {
        self.mouse = Some((x, y));
        match self.drag.clone() {
            Some(Drag::ListBar { y0, s0 }) => {
                let n = self.shown.len() as f32;
                let track = (self.out.list.h - 8.0).max(1.0);
                self.list_scroll = s0 + (y - y0) / track * n;
                self.clamp_scrolls();
                return vec![Act::Repaint];
            }
            Some(Drag::AdvBar { y0, s0 }) => {
                let k = self.out.adv_height / self.out.adv.h.max(1.0);
                self.adv_scroll = s0 + (y - y0) * k;
                self.clamp_scrolls();
                return vec![Act::Repaint];
            }
            Some(_) => return vec![],
            None => {}
        }
        let t = self.hit(x, y);
        let hint = self.hint_for(t.as_ref(), m);
        if self.revealed.is_some() && t.is_none() {
            return vec![];
        }
        self.revealed = None;
        if hint != self.hint {
            self.hint = hint;
        }
        vec![Act::Repaint]
    }

    pub fn leave(&mut self) -> Vec<Act> {
        self.mouse = None;
        self.hint.clear();
        vec![Act::Repaint]
    }

    pub fn wheel(&mut self, x: f32, y: f32, notches: f32, mods: Mods, m: &mut dyn Model) -> Vec<Act> {
        // Over the screen: the previous / next preset, one per notch.
        if self.page == pages::MAIN && self.out.screen.contains(x, y) && notches != 0.0 {
            return self.step(if notches > 0.0 { -1 } else { 1 });
        }
        if self.page == pages::MAIN && self.out.list.contains(x, y) {
            // the row under the pointer changes: its hint comes with the next move
            self.hint.clear();
            self.list_scroll -= notches * LIST_WHEEL;
            self.clamp_scrolls();
            return vec![Act::Repaint];
        }
        if self.page == pages::ADVANCED {
            self.adv_scroll -= notches * ADV_WHEEL;
            self.clamp_scrolls();
            self.mouse = Some((x, y));
            return vec![Act::Repaint];
        }
        let f = if mods.ctrl { FINE } else { 1.0 };
        match self.hit(x, y) {
            Some(Target::Knob(k)) => {
                let Some(p) = self.table.get(&k).cloned() else { return vec![] };
                let cur = m.get(&k);
                let v = if p.stepped {
                    (cur + notches.signum()).clamp(p.min, p.max)
                } else {
                    p.plain(p.norm(cur) + notches * WHEEL_PART * f)
                };
                self.edit(m, &k, v);
                self.hint = self.hint_for(Some(&Target::Knob(k)), m);
            }
            Some(Target::Macro(mc)) => {
                let target = mc.value_of(mc.norm(m) + notches * WHEEL_PART * f);
                let g = mc.begin(m, &self.table);
                g.set(m, target);
                mc.end(m);
                self.edited();
                self.hint = self.hint_for(Some(&Target::Macro(mc)), m);
            }
            _ => return vec![],
        }
        vec![Act::Repaint]
    }

    /// A key typed: the search box takes text while it has the focus; otherwise the arrows step through the presets.
    /// `sym` is an X11 key symbol (see os_plugkit's keysyms), `ch` the character it types.
    pub fn key(&mut self, sym: u32, ch: Option<char>) -> Vec<Act> {
        const BACKSPACE: u32 = 0xff08;
        const RETURN: u32 = 0xff0d;
        const KP_ENTER: u32 = 0xff8d;
        const ESCAPE: u32 = 0xff1b;
        const UP: u32 = 0xff52;
        const DOWN: u32 = 0xff54;
        if self.search_focus {
            match sym {
                BACKSPACE => {
                    self.query.pop();
                }
                ESCAPE => {
                    if self.query.is_empty() {
                        self.search_focus = false;
                    }
                    self.query.clear();
                }
                RETURN | KP_ENTER => {
                    self.search_focus = false;
                    if let Some(&i) = self.shown.first() {
                        return self.load(i);
                    }
                }
                UP => return self.step(-1),
                DOWN => return self.step(1),
                _ => match ch {
                    Some(c) if !c.is_control() => self.query.push(c),
                    _ => return vec![],
                },
            }
            self.list_scroll = 0.0;
            self.refilter();
            return vec![Act::Repaint];
        }
        match sym {
            UP if self.page == pages::MAIN => self.step(-1),
            DOWN if self.page == pages::MAIN => self.step(1),
            _ => vec![],
        }
    }

    /// The menu of a choice parameter.
    pub fn menu_for(&self, key: &str, m: &dyn Model) -> Option<MenuReq> {
        let p = self.table.get(key)?;
        let n = p.steps();
        if n == 0 {
            return None;
        }
        let cur = (m.get(key) - p.min).round() as usize;
        let items = (0..n).map(|i| MenuItem::item(&p.format(p.min + i as f32)).mark(Mark::Radio(i == cur))).collect();
        Some(MenuReq { key: key.into(), items, current: Some(cur) })
    }

    /// A menu row was picked.
    pub fn pick(&mut self, req: &MenuReq, row: usize, m: &mut dyn Model) -> Vec<Act> {
        if req.key == MORE_MENU {
            if let Some((c, _)) = self.more.get(row).cloned() {
                self.select_chip(c);
            }
            return vec![Act::Repaint];
        }
        if let Some(p) = self.table.get(&req.key) {
            let v = (p.min + row as f32).min(p.max);
            let k = req.key.clone();
            self.edit(m, &k, v);
            let h = self.hint_for(Some(&Target::Menu(k)), m);
            self.hint = h;
        }
        vec![Act::Repaint]
    }

    /// Text typed into a type-in box.
    pub fn typed(&mut self, t: &TypeTarget, text: &str, m: &mut dyn Model) -> Vec<Act> {
        match t {
            TypeTarget::Param(k) => {
                if let Some(v) = self.table.get(k).and_then(|p| p.parse(text)) {
                    let k = k.clone();
                    self.edit(m, &k, v);
                }
            }
            TypeTarget::Macro(mc) => {
                if let Some(v) = mc.parse(text) {
                    let g = mc.begin(m, &self.table);
                    g.set(m, v);
                    mc.end(m);
                    self.edited();
                }
            }
            TypeTarget::PresetName => {
                return match library::preset_file_name(text) {
                    Some(name) => vec![Act::Save(library::user_presets_dir().join(format!("{name}.zest"))), Act::Repaint],
                    None => {
                        self.status = "Type a name to save the preset under".into();
                        vec![Act::Repaint]
                    }
                };
            }
        }
        vec![Act::Repaint]
    }

    /// The current value as text for a type-in box.
    pub fn value_text(&self, t: &TypeTarget, m: &dyn Model) -> String {
        match t {
            TypeTarget::Param(k) => self.table.get(k).map(|p| p.format(m.get(k))).unwrap_or_default(),
            TypeTarget::Macro(mc) => mc.text(m),
            TypeTarget::PresetName => self.current.name.clone(),
        }
    }

    /// Switches pages (the host's strip).
    pub fn set_page(&mut self, i: usize) {
        self.page = i.min(pages::PAGES.len() - 1);
    }

    /// The host turned parameter `key`: its page shows, and the hint line says what changes. The line says it until
    /// `until` (`reveal_over`) or until the mouse has something to say.
    pub fn reveal_param(&mut self, key: &str, m: &dyn Model, until: std::time::Instant) {
        self.set_page(pages::page_of(&self.table, m, key));
        self.hint = self.hint_for(Some(&Target::Knob(key.to_string())), m);
        self.revealed = Some(until);
    }

    /// The hint line's time for a turned parameter is up. Whether it changed.
    pub fn reveal_over(&mut self, now: std::time::Instant) -> bool {
        if self.revealed.is_some_and(|u| now >= u) {
            self.revealed = None;
            self.hint.clear();
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::testing::table;
    use crate::params::MemModel;

    #[test]
    fn a_turned_parameter_shows_its_page_and_says_what_changes() {
        let mut f = fixture("reveal");
        let t0 = std::time::Instant::now();
        f.ui.set_page(pages::ADVANCED);
        let key = "VCF1/FreqModDepth";
        f.ui.reveal_param(key, &f.m, t0 + std::time::Duration::from_secs(2));
        assert_eq!(f.ui.page, pages::MAIN);
        assert!(f.ui.hint.contains(':'), "{}", f.ui.hint);
        assert!(!f.ui.reveal_over(t0), "still says it");
        assert!(f.ui.reveal_over(t0 + std::time::Duration::from_secs(2)));
        assert!(f.ui.hint.is_empty());
    }

    struct Fx {
        ui: Ui,
        m: MemModel,
        root: PathBuf,
    }

    fn fixture(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("zest-ui-ui-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let tagged = |c: &str| format!("/*@Meta\n\nCategories:\n'{c}'\n\n*/\n");
        for (f, text) in [
            ("diva/1 BASS/HS Deep Bass.h2p", tagged("Bass:Analogue")),
            ("diva/1 BASS/MK Low.h2p", tagged("Bass:Synth")),
            ("diva/2 LEAD/SG Saw Lead.h2p", tagged("Leads:Synth")),
            ("user/My Pad.zest", "x".into()),
        ] {
            let p = root.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, text).unwrap();
        }
        let sources = vec![
            Source { root: root.join("user"), exts: vec!["zest".into()], name: "User".into(), single: true, user: true, ..Default::default() },
            Source { root: root.join("diva"), exts: vec!["h2p".into()], name: "Diva".into(), ..Default::default() },
        ];
        let t = table();
        let m = MemModel::new(&t);
        let mut ui = Ui::new(t, sources, root.join("favorites.txt"));
        paint(&mut ui, &m);
        Fx { ui, m, root }
    }

    fn paint(ui: &mut Ui, m: &MemModel) {
        let (w, h) = (ui.size.0.ceil() as u32, ui.size.1.ceil() as u32);
        let mut c = Canvas::new(w, h, 1.0).unwrap();
        ui.paint(&mut c, m);
    }

    /// How long the editor's opening takes on the real preset folders: the editor's state, a first and a second paint
    /// (`cargo test -p zest-ui --release open_time -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn open_time() {
        for _ in 0..3 {
            let t0 = std::time::Instant::now();
            let mut ui = Ui::new(table(), library::default_sources(), Favorites::default_file());
            let built = t0.elapsed();
            let m = MemModel::new(&table());
            let (w, h) = (ui.size.0.ceil() as u32, ui.size.1.ceil() as u32);
            let t1 = std::time::Instant::now();
            let mut c = Canvas::new(w, h, 1.0).unwrap();
            let canvas = t1.elapsed();
            let t2 = std::time::Instant::now();
            ui.paint(&mut c, &m);
            let first = t2.elapsed();
            let t3 = std::time::Instant::now();
            ui.paint(&mut c, &m);
            eprintln!("new {built:?} canvas {canvas:?} first paint {first:?} second paint {:?} presets {}", t3.elapsed(), ui.lib.presets.len());
        }
        let t = std::time::Instant::now();
        let _ = table();
        eprintln!("table {:?}", t.elapsed());
    }

    fn centre(ui: &Ui, t: &Target) -> (f32, f32) {
        let h = ui.hits().iter().find(|h| &h.t == t).unwrap_or_else(|| panic!("no hit {t:?} on page {}", ui.page));
        (h.r.cx(), h.r.cy())
    }

    fn click(f: &mut Fx, t: &Target) -> Vec<Act> {
        let (x, y) = centre(&f.ui, t);
        let acts = f.ui.down(x, y, Btn::Left, Mods::default(), false, &mut f.m);
        f.ui.up(&mut f.m);
        paint(&mut f.ui, &f.m);
        acts
    }

    fn knob(k: &str) -> Target {
        Target::Knob(k.into())
    }

    #[test]
    fn every_page_has_its_controls() {
        let mut f = fixture("pages");
        for t in [
            knob("VCF1/Frequency"),
            knob("OSC/Tune2"),
            knob("ENV2/Attack"),
            Target::Macro(Macro::Drift),
            Target::Macro(Macro::Detune),
            Target::Choice("OSC/Model".into(), 4),
            Target::Choice("VCC/Mode".into(), 2),
            Target::Choice("VCC/GlideMode".into(), 1),
            Target::Menu("VCC/PitchBend Up".into()),
            Target::Menu("LFO1/Waveform".into()),
            Target::Tab(2),
            Target::Save,
            Target::Search,
            Target::Next,
            Target::Chip(Chip::Kind(crate::library::Kind::Bass)),
            Target::More,
            Target::Preset(1),
        ] {
            centre(&f.ui, &t);
        }
        assert!(!f.ui.hits().iter().any(|h| h.t == Target::Star), "no star without a preset");
        click(&mut f, &Target::Tab(pages::EFFECTS));
        assert_eq!(f.ui.page, pages::EFFECTS);
        for t in [Target::Toggle("main/Active #FX1".into()), Target::Choice("FX1/Module".into(), 3), knob("Chrs1/Rate"), Target::Choice("Chrs1/Type".into(), 2), knob("Plate2/Decay")] {
            centre(&f.ui, &t);
        }
        click(&mut f, &Target::Tab(pages::ADVANCED));
        for t in [Target::Value("VCF1/Frequency".into()), Target::Menu("VCF1/FreqModSrc".into()), Target::Value("OPT/TuneSlop".into())] {
            centre(&f.ui, &t);
        }
        assert!(!f.ui.hits().iter().any(|h| h.t == Target::Value("Chrs1/Rate".into())), "effects stay on their page");
        assert!(!f.ui.hits().iter().any(|h| h.t.key() == Some("VCC/Vc1")), "hidden parameters never show");
        let _ = std::fs::remove_dir_all(&f.root);
    }

    #[test]
    fn controls_edit_the_parameters() {
        let mut f = fixture("edit");
        click(&mut f, &Target::Choice("OSC/Model".into(), 1));
        assert_eq!(f.m.get("OSC/Model"), 1.0);
        assert!(f.ui.current.modified);
        // A knob drag: 100 px up moves half the range.
        let (x, y) = centre(&f.ui, &knob("VCF1/Frequency"));
        let acts = f.ui.down(x, y, Btn::Left, Mods::default(), false, &mut f.m);
        assert!(acts.contains(&Act::Lock));
        f.ui.delta(0.0, -100.0, Mods::default(), &mut f.m);
        f.ui.up(&mut f.m);
        assert!((f.m.get("VCF1/Frequency") - (41.0 + 60.0)).abs() < 0.01);
        assert_eq!(f.m.gestures.last(), Some(&("VCF1/Frequency".to_string(), false)));
        // Double click: the default.
        f.ui.down(x, y, Btn::Left, Mods::default(), true, &mut f.m);
        assert_eq!(f.m.get("VCF1/Frequency"), 41.0);
        // Right click: type a value in.
        let acts = f.ui.down(x, y, Btn::Right, Mods::default(), false, &mut f.m);
        let Some(Act::TypeIn(tt, text, ..)) = acts.iter().find(|a| matches!(a, Act::TypeIn(..))).cloned() else { panic!("{acts:?}") };
        assert_eq!(text, "41.0");
        f.ui.typed(&tt, "120", &mut f.m);
        assert_eq!(f.m.get("VCF1/Frequency"), 120.0);
        // Drift moves the slops together.
        paint(&mut f.ui, &f.m);
        let (x, y) = centre(&f.ui, &Target::Macro(Macro::Drift));
        f.ui.down(x, y, Btn::Left, Mods::default(), false, &mut f.m);
        f.ui.delta(0.0, -66.0, Mods::default(), &mut f.m);
        f.ui.up(&mut f.m);
        assert!((f.m.get("OPT/PWSlop") - 66.0).abs() < 0.5, "{}", f.m.get("OPT/PWSlop"));
        assert!((f.m.get("OPT/TuneSlop") - 50.0).abs() < 0.5);
        // A menu.
        let acts = click(&mut f, &Target::Menu("VCC/Voices".into()));
        let Some(Act::Menu(req, ..)) = acts.into_iter().find(|a| matches!(a, Act::Menu(..))) else { panic!() };
        assert_eq!(req.items.len(), 8);
        assert_eq!(req.current, Some(4));
        f.ui.pick(&req, 7, &mut f.m);
        assert_eq!(f.m.get("VCC/Voices"), 7.0);
        // A switch on the Effects page.
        click(&mut f, &Target::Tab(pages::EFFECTS));
        click(&mut f, &Target::Toggle("main/Active #FX2".into()));
        assert_eq!(f.m.get("main/Active #FX2"), 1.0);
        click(&mut f, &Target::Choice("FX1/Module".into(), 1));
        assert_eq!(f.m.get("FX1/Module"), 1.0);
        centre(&f.ui, &knob("Phase1/Rate"));
        let _ = std::fs::remove_dir_all(&f.root);
    }

    #[test]
    fn the_preset_browser() {
        let mut f = fixture("browser");
        // Library order: user presets, then Diva's folders.
        let files: Vec<&str> = f.ui.shown.iter().map(|&i| f.ui.lib.presets[i].file.as_str()).collect();
        assert_eq!(files, ["My Pad", "HS Deep Bass", "MK Low", "SG Saw Lead"]);
        let acts = click(&mut f, &Target::Preset(1));
        let deep = f.root.join("diva/1 BASS/HS Deep Bass.h2p");
        assert!(acts.contains(&Act::Load(deep.clone())));
        assert_eq!((f.ui.current.name.as_str(), f.ui.current.place.as_str(), f.ui.current.author.as_deref()), ("Deep Bass", "Bass", Some("HS")));
        f.ui.loaded(&deep, Some("Howard Scarr".into()));
        assert_eq!(f.ui.current.author.as_deref(), Some("Howard Scarr"), "the file's author wins");
        // Next / previous follow the list.
        let acts = click(&mut f, &Target::Next);
        assert!(acts.contains(&Act::Load(f.root.join("diva/1 BASS/MK Low.h2p"))));
        let acts = f.ui.step(-1);
        assert!(acts.contains(&Act::Load(deep.clone())));
        // The star: favourites.txt.
        paint(&mut f.ui, &f.m);
        click(&mut f, &Target::Star);
        assert!(f.ui.current.favourite);
        assert_eq!(std::fs::read_to_string(f.root.join("favorites.txt")).unwrap().trim(), deep.to_string_lossy());
        click(&mut f, &Target::Chip(Chip::Favorites));
        assert_eq!(f.ui.shown.len(), 1);
        click(&mut f, &Target::Chip(Chip::Kind(crate::library::Kind::Lead)));
        assert_eq!(f.ui.shown.len(), 1);
        click(&mut f, &Target::Chip(Chip::Kind(crate::library::Kind::Bass)));
        assert_eq!(f.ui.shown.len(), 2);
        // ⋯: the chips without a place in the row (here the user's own presets).
        let acts = click(&mut f, &Target::More);
        let Some(Act::Menu(req, ..)) = acts.into_iter().find(|a| matches!(a, Act::Menu(..))) else { panic!("no menu") };
        f.ui.pick(&req, 0, &mut f.m);
        assert_eq!(f.ui.chip, Chip::Category(crate::library::USER.into()));
        assert_eq!(f.ui.shown.len(), 1);
        click(&mut f, &Target::Chip(Chip::All));
        // The wheel over the screen steps through the presets.
        paint(&mut f.ui, &f.m);
        let screen = f.ui.out.screen;
        let acts = f.ui.wheel(screen.cx(), screen.cy(), -1.0, Mods::default(), &mut f.m);
        assert!(acts.contains(&Act::Load(f.root.join("diva/1 BASS/MK Low.h2p"))), "down: the next preset");
        // Search: typed while the box has the focus.
        click(&mut f, &Target::Search);
        assert!(f.ui.search_focus);
        for c in "low".chars() {
            f.ui.key(c as u32, Some(c));
        }
        assert_eq!(f.ui.shown.len(), 1);
        f.ui.key(0xff08, None);
        assert_eq!(f.ui.query, "lo");
        let acts = f.ui.key(0xff0d, None);
        assert!(acts.contains(&Act::Load(f.root.join("diva/1 BASS/MK Low.h2p"))), "Return loads the first match");
        // Save: a name typed in.
        paint(&mut f.ui, &f.m);
        let acts = click(&mut f, &Target::Save);
        let Some(Act::TypeIn(tt, ..)) = acts.into_iter().find(|a| matches!(a, Act::TypeIn(..))) else { panic!() };
        assert_eq!(tt, TypeTarget::PresetName);
        let acts = f.ui.typed(&tt, " Big/Pad ", &mut f.m);
        let Some(Act::Save(p)) = acts.into_iter().find(|a| matches!(a, Act::Save(_))) else { panic!() };
        assert_eq!(p.file_name().unwrap(), "BigPad.zest");
        assert!(f.ui.typed(&tt, "  ", &mut f.m).iter().all(|a| !matches!(a, Act::Save(_))));
        let _ = std::fs::remove_dir_all(&f.root);
    }

    /// Every control lies inside the window and no two dials, boxes or pills overlap, at the default size, a small
    /// window (zoomed out) and hosted.
    #[test]
    fn layout_fits() {
        let mut f = fixture("fit");
        for (w, h, hosted) in [(view::BASE_W, view::BASE_H, false), (760.0, 460.0, false), (view::BASE_W, view::BASE_H - view::STRIP, true), (1400.0, 900.0, false)] {
            f.ui.hosted = hosted;
            let (_, lw, lh) = view::fit(w, h, hosted);
            assert!(lw >= view::BASE_W - 0.01 && lh >= view::base_h(hosted) - 0.01);
            f.ui.size = (lw, lh);
            for page in 0..pages::PAGES.len() {
                f.ui.set_page(page);
                for model in 0..5 {
                    f.m.values.insert("OSC/Model".into(), model as f32);
                    f.m.values.insert("VCF1/Model".into(), model as f32);
                    paint(&mut f.ui, &f.m);
                    let hits: Vec<&Hit> = f.ui.hits().iter().filter(|h| !matches!(h.t, Target::ListBar | Target::AdvBar | Target::Preset(_))).collect();
                    for hit in &hits {
                        let r = hit.r;
                        assert!(r.x >= 0.0 && r.y >= 0.0 && r.right() <= lw + 0.5 && r.bottom() <= lh + 0.5, "{w}x{h} page {page}: {:?} out of the window", hit.t);
                    }
                    let solid = |t: &Target| matches!(t, Target::Knob(_) | Target::Macro(_) | Target::Menu(_) | Target::Choice(..) | Target::Value(_));
                    for (i, a) in hits.iter().enumerate().filter(|(_, a)| solid(&a.t)) {
                        for b in hits[i + 1..].iter().filter(|b| solid(&b.t)) {
                            let o = a.r.intersect(&b.r);
                            assert!(o.is_empty() || o.w * o.h < 4.0, "{w}x{h} page {page} model {model}: {:?} overlaps {:?}", a.t, b.t);
                        }
                    }
                }
            }
        }
        let _ = std::fs::remove_dir_all(&f.root);
    }
}
