// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Cutting-edge UI hooks on [`App`]: tiling window keys, tile/veil/float-layer motion, floating-layer capture, and
//! in-app plugin cards & tabs (`EmbedFrame`, `plugin_tabs`). Kept out of `app.rs` so merges from `main` stay clean.

use os_ui::{Canvas, Rect};

use crate::app::{Action, App, Drag, Keep, Ui, Win};
use crate::views;

impl App {
    /// The windows of one plugin as tabs of one window: each has its row of tabs and the group's place.
    #[cfg(all(lemon_cards, plugin_windows))]
    pub(crate) fn sync_plugin_tabs(&mut self, frames: &std::collections::BTreeMap<usize, crate::plugin_window::EmbedFrame>) {
        let open: Vec<(usize, String)> = frames.keys().map(|&r| (r, self.state.session.plugins.descriptor(r).map_or(String::new(), |d| d.name.clone()))).collect();
        let tabs = &mut self.state.plugin_tabs;
        let joined = tabs.sync(&open, crate::prefs::check(&crate::prefs::PLUGIN_TABS));
        let place = |r: usize| frames.get(&r).map(|f| { let a = f.rect(); (a.x, a.y) });
        for j in joined {
            if let (Some(f), Some(p)) = (frames.get(&j.row), place(j.from)) {
                f.set_pos(p);
            }
        }
        let previewing = self.state.tab_preview.as_ref().map(|p| p.row);
        for (r, f) in frames {
            f.set_tabs(tabs.group_of(*r).is_some() || previewing == Some(*r));
        }
        for (r, active) in tabs.followers() {
            if let (Some(f), Some(p)) = (frames.get(&r), place(active)) {
                f.set_pos(p);
            }
        }
    }

    /// The plugin windows drawn in the app's (`plugin_window::EmbedFrame`) go into `State::embeds` to be painted,
    /// follow the canvas and the workspace, and are repainted where they or their editors changed.
    #[cfg(all(lemon_cards, plugin_windows))]
    pub(crate) fn sync_embeds(&mut self, frames: std::collections::BTreeMap<usize, crate::plugin_window::EmbedFrame>) {
        let k = self.state.ui_scale.max(0.1);
        let canvas = ((self.state.screen.0 * k).round() as u32, (self.state.screen.1 * k).round() as u32);
        let work = self.state.work_rect();
        let mut seen = std::mem::take(&mut self.embed_seen);
        for (row, (_, r, o)) in &seen {
            if !frames.contains_key(row) {
                self.damage_rect(*r);
                if let Some(o) = o {
                    self.damage_rect(*o);
                }
            }
        }
        for (&row, f) in &frames {
            // A tab behind another and a window slid away do not paint.
            f.set_shown(!self.state.plugin_tabs.hidden(row) && !self.state.float_layer.keeps(row));
            // Where it paints, with its row of tabs: the row coming or going repaints what lies under it.
            let now = f.sync(k, canvas, self.canvas_off, work, &self.theme.colors);
            if seen.get(&row) != Some(&now) {
                if let Some((_, r, o)) = seen.get(&row) {
                    self.damage_rect(*r);
                    if let Some(o) = o {
                        self.damage_rect(*o);
                    }
                }
                self.damage_rect(now.1);
                if let Some(o) = now.2 {
                    self.damage_rect(o);
                }
            }
            seen.insert(row, now);
        }
        seen.retain(|row, _| frames.contains_key(row));
        self.embed_seen = seen;
        self.state.embeds = frames;
    }

    /// A dragged plugin window is waiting out its hold over another (`plugin_tabs::Held`).
    #[cfg(all(lemon_cards, plugin_windows))]
    pub(crate) fn tab_holding(&self) -> bool {
        self.tab_held.is_some_and(|h| !h.ripe)
    }

    #[cfg(not(all(lemon_cards, plugin_windows)))]
    pub(crate) fn tab_holding(&self) -> bool {
        false
    }

    /// The tiling layout's windows changed places (shown, hidden, swapped, restored, another preset): the frame repaints
    /// whole, and from "Polished" up (General ▸ Display ▸ Animations) they move there as Hyprland's windows do
    /// (`tiles::Anim`). A split dragged, the card resized, the first frame and a window maximized jump straight
    /// there.
    pub(crate) fn tiles_motion(&mut self) {
        let now = std::time::Instant::now();
        // A window alone is never maximized (`State::alone`).
        if self.state.alone() {
            self.state.wins.iter_mut().for_each(|w| w.maxed = false);
        }
        let targets = crate::tiles::targets(&self.state);
        let work = self.state.work_rect();
        // Which windows are open shows on the toolbar's window buttons: a change repaints all.
        let open = (self.state.z.clone(), self.state.show_browser);
        if open != self.tile_open {
            self.tile_open = open;
            if self.state.tiled() {
                self.invalidate();
            }
        }
        if targets != self.tile_targets {
            // Instant too: maximizing a window over the card.
            let full = self.state.docked().is_some();
            let direct = full || matches!(self.drag, Some(Drag::TileSplit { .. })) || self.tile_work != Some(work);
            let motion = !cfg!(test) && !direct && crate::anim::level() >= crate::anim::Level::Polished;
            let from = self.state.tile_anim.as_ref().map_or_else(|| self.tile_targets.clone(), |a| a.shown(&self.tile_targets));
            self.state.tile_anim = motion.then(|| crate::tiles::Anim::new(&from, &targets)).flatten();
            self.tile_anim_at = Some(now);
            self.tile_targets = targets;
            self.invalidate();
        }
        self.tile_work = Some(work);
        if let (Some(a), Some(t0)) = (&mut self.state.tile_anim, self.tile_anim_at) {
            a.elapsed = now.duration_since(t0).as_secs_f32();
            if a.done() {
                self.state.tile_anim = None;
            }
            self.invalidate();
        }
        self.veils_tick(now);
        self.float_layer_tick(now);
        #[cfg(all(lemon_cards, plugin_windows))]
        self.tab_hold_tick(now);
    }

    /// Shift+F or the button pressed, a floating window opened while they are down, and the slide on its way
    /// (`crate::float_layer`). Plugin windows of their own move with it, on their own screen's height.
    fn float_layer_tick(&mut self, now: std::time::Instant) {
        use crate::float_layer as layer;
        #[cfg(all(target_os = "linux", plugin_windows))]
        let native = self.state.editor_host.native_rows();
        #[cfg(not(all(target_os = "linux", plugin_windows)))]
        let native: Vec<usize> = Vec::new();
        let motion = !cfg!(test) && crate::anim::level() >= crate::anim::Level::Polished;
        let pressed = std::mem::take(&mut self.state.float_layer.pressed);
        let down = self.state.float_layer.is_down();
        let moved = if pressed && down || layer::wanted_back(&self.state, &native) {
            layer::go(&mut self.state, false, Vec::new(), motion, now);
            true
        } else if pressed && layer::has_windows(&self.state, &native) {
            layer::go(&mut self.state, true, native, motion, now);
            true
        } else {
            false
        };
        if layer::step(&mut self.state, now) || moved {
            #[cfg(all(target_os = "linux", plugin_windows))]
            self.state.editor_host.slide_native(self.state.float_layer.at());
            self.invalidate();
        }
    }

    /// The split windows without the keyboard mute (`tiles::veil_target`): fading, from "Subtle" up, as Hyprland's
    /// borders do (`tiles::VEIL_FADE`).
    fn veils_tick(&mut self, now: std::time::Instant) {
        let want = crate::tiles::veil_target(&self.state);
        let to = self.veil_fade.map_or(self.state.veil, |f| f.2);
        if want != to {
            let fade = !cfg!(test) && crate::anim::level() > crate::anim::Level::Off;
            if fade {
                self.veil_fade = Some((now, self.state.veil, want));
            } else {
                self.veil_fade = None;
                self.state.veil = want;
            }
            self.damage_veils(&want, &to);
        }
        if let Some((t0, from, to)) = self.veil_fade {
            let x = (now.duration_since(t0).as_secs_f32() / crate::tiles::VEIL_FADE).min(1.0);
            let e = crate::tiles::ease(x);
            for i in 0..3 {
                self.state.veil[i] = from[i] + (to[i] - from[i]) * e;
            }
            if x >= 1.0 {
                self.veil_fade = None;
            }
            self.damage_veils(&from, &to);
        }
    }

    /// Repaint the split windows whose veil changes between `a` and `b`.
    fn damage_veils(&mut self, a: &[f32; 3], b: &[f32; 3]) {
        for (w, r) in crate::tiles::side_by_side(&self.state) {
            if w.slot().is_some_and(|i| a[i] != b[i]) {
                self.damage_rect(r);
                // The mixer's cables are muted with it: the gap they hang in and where they hang over the window below.
                if w == Win::Mixer {
                    let s = &self.state;
                    let extra: Vec<Rect> = s.mixer_hang().into_iter().chain(s.cable_reach().zip(s.cables.borrow().front_bounds()).map(|(r, c)| r.intersect(&c.outset(3.0)))).collect();
                    for e in extra.into_iter().filter(|e| e.w > 0.0 && e.h > 0.0) {
                        self.damage_rect(e);
                    }
                }
            }
        }
    }

    /// The slide's picture: the layer's windows painted once on a clear canvas, with the app's text engine, and cut down
    /// to their part (`crate::float_layer`).
    pub(crate) fn capture_float_layer(&mut self, canvas: &mut Canvas) {
        let k = canvas.scale;
        let (cw, ch) = (canvas.pixmap.width(), canvas.pixmap.height());
        let areas: Vec<Rect> = self.state.z.iter().filter(|&&w| crate::float_layer::member(&self.state, w)).filter_map(|&w| crate::floating::paint_area(&self.state, w)).collect();
        let bounds = areas.iter().copied().reduce(|a, b| a.union(&b));
        let image = bounds.and_then(|b| {
            let x0 = ((b.x * k).floor().max(0.0) as u32).min(cw);
            let y0 = ((b.y * k).floor().max(0.0) as u32).min(ch);
            let x1 = ((b.right() * k).ceil().max(0.0) as u32).min(cw);
            let y1 = ((b.bottom() * k).ceil().max(0.0) as u32).min(ch);
            let mut out = tiny_skia::Pixmap::new(x1.checked_sub(x0).filter(|&w| w > 0)?, y1.checked_sub(y0).filter(|&h| h > 0)?)?;
            let clear = tiny_skia::Pixmap::new(cw, ch)?;
            let main = std::mem::replace(&mut canvas.pixmap, clear);
            canvas.begin_frame(None);
            let info = self.frame_info();
            let (mut hits, mut bars, mut live) = (Vec::new(), Vec::new(), Vec::new());
            let mut keep = Keep::full();
            let mut ui = Ui {
                c: canvas,
                t: &self.theme.colors,
                hits: &mut hits,
                bars: &mut bars,
                hover: &self.hover,
                pressed: &self.pressed,
                mouse: self.mouse,
                win: None,
                keep: &mut keep,
                live: &mut live,
                flat: self.state.toolbar.flat,
                tiled: self.state.tiled(),
                alone: self.state.alone(),
                maxed: self.state.wins.map(|w| w.maxed),
                motion: &self.state.motion,
                k: 1.0,
            };
            views::paint_float_layer(&mut ui, &self.state, &info);
            let whole = std::mem::replace(&mut canvas.pixmap, main);
            let w = out.width() as usize;
            for (y, row) in out.pixels_mut().chunks_exact_mut(w).enumerate() {
                let from = (y0 as usize + y) * cw as usize + x0 as usize;
                row.copy_from_slice(&whole.pixels()[from..from + w]);
            }
            Some(crate::float_layer::Image { pixels: out, at: (x0 as i32, y0 as i32) })
        });
        self.state.float_layer.set_image(image, std::time::Instant::now());
    }

    /// The tiling layout's keys for its windows, wherever the keyboard is:
    /// - Ctrl+Alt+arrows: the keyboard to the nearest split window that way (from a floating one too; floating windows
    ///   are no stops). While one is maximized, a split window that way is maximized in its place.
    /// - Ctrl+Alt+Shift+arrows: the split window with the keyboard changes places with the one that way.
    /// - F: maximizes the split window under the mouse, or brings the split back (the one with the keyboard when the
    ///   mouse is over none).
    /// - Ctrl+Shift+F: closes the window under the mouse, split or floating.
    ///
    /// Returns whether the key was one of them.
    pub(crate) fn window_key(&mut self, key: &crate::keys::Key, ctrl: bool, alt: bool) -> bool {
        use crate::keys::{Key, NamedKey};
        use crate::tiles::{self, Dir};
        let dir = match key {
            Key::Named(NamedKey::ArrowLeft) => Some(Dir::Left),
            Key::Named(NamedKey::ArrowRight) => Some(Dir::Right),
            Key::Named(NamedKey::ArrowUp) => Some(Dir::Up),
            Key::Named(NamedKey::ArrowDown) => Some(Dir::Down),
            _ => None,
        };
        if let Some(d) = dir.filter(|_| ctrl && alt) {
            let s = &mut self.state;
            let Some(from) = s.focused() else { return true };
            let docked = s.docked();
            // Where each window is: the split's as if none were maximized, so the arrows go the same ways either way.
            let split = if docked.is_some() { tiles::plan_all(s) } else { tiles::plan(s) };
            // Only split windows are stops: the floating ones are a layer of their own (from one, the arrows go to the
            // split window that way).
            let rects: Vec<(Win, Rect)> = split.tiles.clone();
            let at = rects.iter().find(|r| r.0 == from).map_or_else(|| views::frame_rect(s, from), |r| r.1);
            let others: Vec<(Win, Rect)> = rects.into_iter().filter(|r| r.0 != from).collect();
            if self.shift {
                // Only split windows change places, and not while one is maximized.
                if docked.is_none() && from.slot().is_some() && let Some(to) = tiles::neighbour(&others, at, d) {
                    tiles::swap(s, from, to);
                }
                return true;
            }
            let Some(to) = tiles::neighbour(&others, at, d) else { return true };
            if let (Some(old), Some(i)) = (docked, to.slot()) {
                // A maximized window gives the workspace to the one that way.
                if let Some(j) = old.slot() {
                    s.wins[j].maxed = false;
                }
                s.wins[i].maxed = true;
            }
            s.raise(to);
            return true;
        }
        let f = matches!(key, Key::Character(c) if c.eq_ignore_ascii_case("f"));
        if f && !ctrl && !alt && !self.shift {
            let (x, y) = self.mouse;
            let under = self.top_hit(x, y).and_then(|h| h.win).filter(|w| w.slot().is_some());
            let s = &self.state;
            if let Some(w) = under.or(s.docked()).or(s.focused().filter(|w| w.slot().is_some())) {
                self.act(Action::MaxWin(w));
            }
            return true;
        }
        // Ctrl+Shift+F: closes the window under the mouse, as its close button does: a split window, or a floating
        // one (Ctrl+F is FL's browser search).
        if f && ctrl && !alt && self.shift {
            let (x, y) = self.mouse;
            if let Some(w) = self.top_hit(x, y).and_then(|h| h.win) {
                if self.closes_first(w) {
                    self.close_floating(w);
                } else if w.slot().is_some() {
                    self.act(Action::CloseWin(w));
                    self.invalidate();
                }
            }
            return true;
        }
        false
    }

    /// Ctrl, Shift and Alt for a plugin editor drawn in the app's window.
    #[cfg(lemon_cards)]
    pub(crate) fn embed_mods(&self) -> (bool, bool, bool) {
        (self.ctrl, self.shift, self.alt)
    }

    /// The plugin window drawn in the app's whose editor has a menu or box open: every input goes to it.
    #[cfg(lemon_cards)]
    pub(crate) fn embed_modal(&self) -> Option<usize> {
        self.state.embeds.iter().find(|(_, e)| e.modal()).map(|(&r, _)| r)
    }

    /// The plugin window drawn in the app's that is on top at (`x`, `y`).
    #[cfg(lemon_cards)]
    pub(crate) fn embed_at(&self, x: f32, y: f32) -> Option<usize> {
        match self.top_hit(x, y)?.action {
            Action::Embed(r) => Some(r),
            _ => None,
        }
    }

    /// A press on a plugin window drawn in the app's (or anywhere while its editor's menu is open) goes to it,
    /// and brings it to the front. Whether it took the press.
    #[cfg(all(lemon_cards, plugin_windows))]
    pub(crate) fn embed_down(&mut self, btn: os_plugkit::input::Button) -> bool {
        if self.modal_open() || self.state.menu_open.is_some() {
            return false;
        }
        let (x, y) = self.mouse;
        if let Some(&Action::PluginTabClose(r)) = self.top_hit(x, y).map(|h| &h.action) {
            if btn == os_plugkit::input::Button::Left {
                self.tab_close_press = Some(r);
            }
            return true;
        }
        if let Some(&Action::PluginTab(r)) = self.top_hit(x, y).map(|h| &h.action) {
            if btn == os_plugkit::input::Button::Left {
                self.state.plugin_tabs.activate(r);
                self.state.raise(Win::Hosted(r));
                self.state.no_focus = false;
                self.tab_press = Some((r, (x, y), None));
                self.invalidate();
            }
            return true;
        }
        let Some(row) = self.embed_modal().or_else(|| self.embed_at(x, y)) else { return false };
        let Some(e) = self.state.embeds.get(&row).cloned() else { return false };
        // Its title-row buttons act on the release: the close button never brings the window forward (it showed in
        // front of the window the keyboard was in until the release closed it), maximize and minimize at the release.
        let button = e.button_at(x, y).filter(|_| btn == os_plugkit::input::Button::Left);
        self.embed_raise = button.filter(|&close| !close).map(|_| row);
        if button.is_none() && self.state.z.last() != Some(&Win::Hosted(row)) {
            self.state.raise(Win::Hosted(row));
            self.invalidate();
        }
        self.state.no_focus = false;
        e.press(x, y, btn, self.embed_mods());
        self.embed_press = Some((row, btn));
        self.embed_virt = None;
        true
    }

    /// The mouse moved: to the plugin window drawn in the app's that was pressed on (as the distance moved while
    /// its editor holds the cursor), whose menu is open, or that it is over. Whether that window took it alone.
    #[cfg(all(lemon_cards, plugin_windows))]
    pub(crate) fn embed_move(&mut self, x: f32, y: f32) -> bool {
        if let Some((row, from, apart)) = self.tab_press {
            let Some(e) = self.state.embeds.get(&row).cloned() else {
                self.tab_press = None;
                return true;
            };
            // Out of the row, it stands alone and follows the mouse.
            let strip = e.tabs_rect();
            let offset = apart.or_else(|| {
                let out = !strip.contains(x, y) && (x - from.0).hypot(y - from.1) > 12.0;
                out.then(|| {
                    self.state.plugin_tabs.pull_out(row);
                    let r = e.rect();
                    (from.0 - r.x, from.1 - r.y)
                })
            });
            if let Some(d) = offset {
                e.set_pos((x - d.0, y - d.1));
                self.tab_press = Some((row, from, Some(d)));
                self.state.raise(Win::Hosted(row));
                self.invalidate();
                self.tab_hold_track(row, x, y);
            }
            return true;
        }
        let mods = self.embed_mods();
        if let Some((row, _)) = self.embed_press {
            if let Some(e) = self.state.embeds.get(&row).cloned() {
                if e.locked() {
                    let v = self.embed_virt.unwrap_or((x, y));
                    if (x, y) != v {
                        e.delta(x - v.0, y - v.1, mods);
                    }
                    self.embed_virt = Some((x, y));
                } else {
                    self.embed_virt = None;
                    e.motion(x, y, mods);
                    if e.moving() {
                        self.tab_hold_track(row, x, y);
                    }
                }
            }
            return true;
        }
        if let Some(row) = self.embed_modal() {
            if let Some(e) = self.state.embeds.get(&row) {
                e.motion(x, y, mods);
            }
            return true;
        }
        let under = self.embed_at(x, y);
        if self.embed_hover != under {
            if let Some(e) = self.embed_hover.and_then(|r| self.state.embeds.get(&r)) {
                e.leave();
            }
            self.embed_hover = under;
        }
        if let Some(e) = under.and_then(|r| self.state.embeds.get(&r)) {
            e.motion(x, y, mods);
        }
        false
    }

    /// The button went up: to the plugin window drawn in the app's that had the press (or the open menu).
    #[cfg(all(lemon_cards, plugin_windows))]
    pub(crate) fn embed_up(&mut self) -> bool {
        if self.tab_press.take().is_some() {
            self.tab_hold_finish();
            return true;
        }
        let (x, y) = self.mouse;
        // A tab's little x acts on the release, over it still.
        if let Some(r) = self.tab_close_press.take() {
            if self.top_hit(x, y).is_some_and(|h| h.action == Action::PluginTabClose(r)) {
                self.state.plugin_tabs.closing(r);
                self.state.editor_host.close_window(&mut self.state.session.plugins, r);
                self.invalidate();
            }
            return true;
        }
        let mods = self.embed_mods();
        let (row, btn) = match self.embed_press.take() {
            Some(p) => p,
            None => match self.embed_modal() {
                Some(r) => (r, os_plugkit::input::Button::Left),
                None => return false,
            },
        };
        self.embed_virt = None;
        if self.embed_raise.take() == Some(row) && self.state.z.last() != Some(&Win::Hosted(row)) {
            self.state.raise(Win::Hosted(row));
            self.invalidate();
        }
        if let Some(e) = self.state.embeds.get(&row) {
            e.release(x, y, btn, mods);
        }
        self.tab_hold_finish();
        true
    }

    /// The window of another of `row`'s plugin whose title the mouse is near (`plugin_tabs::join_zone`), with
    /// nothing lying over it there.
    #[cfg(all(lemon_cards, plugin_windows))]
    fn tab_target(&self, row: usize, x: f32, y: f32) -> Option<usize> {
        if !crate::prefs::check(&crate::prefs::PLUGIN_TABS) {
            return None;
        }
        let kind = self.state.session.plugins.descriptor(row)?.name;
        let own = self.state.plugin_tabs.merged(row, row);
        for w in self.state.paint_order().into_iter().rev() {
            if w == Win::Hosted(row) {
                continue;
            }
            if let Win::Hosted(r) = w {
                let Some(e) = self.state.embeds.get(&r) else { continue };
                let keep = self.tab_held.is_some_and(|h| h.ripe && h.into == r);
                if !crate::plugin_tabs::join_zone(e.head_rect(), keep).contains(x, y) {
                    if e.rect().contains(x, y) {
                        return None;
                    }
                    continue;
                }
                let same = self.state.session.plugins.descriptor(r).is_some_and(|d| d.name == kind);
                return (same && !own.contains(&r)).then_some(r);
            }
            if crate::floating::floats(w) && crate::floating::paint_area(&self.state, w).is_some_and(|a| a.contains(x, y)) {
                return None;
            }
        }
        None
    }

    /// A window dragged to (`x`, `y`): the hold over another window starts, goes on or ends.
    #[cfg(all(lemon_cards, plugin_windows))]
    fn tab_hold_track(&mut self, row: usize, x: f32, y: f32) {
        let now = std::time::Instant::now();
        match (self.tab_target(row, x, y), &mut self.tab_held) {
            (Some(t), Some(h)) if h.into == t => {}
            (Some(t), _) => {
                self.tab_preview_off();
                self.tab_held = Some(crate::plugin_tabs::Held::new(row, t, now));
            }
            (None, _) => {
                self.tab_preview_off();
                self.tab_held = None;
            }
        }
    }

    #[cfg(all(lemon_cards, plugin_windows))]
    fn tab_preview_off(&mut self) {
        if self.state.tab_preview.take().is_some() {
            self.invalidate();
        }
    }

    /// The hold's time is up: the held window shows the group it would make.
    #[cfg(all(lemon_cards, plugin_windows))]
    pub(crate) fn tab_hold_tick(&mut self, now: std::time::Instant) {
        let Some(mut h) = self.tab_held else { return };
        if !self.state.embeds.contains_key(&h.row) || !self.state.embeds.contains_key(&h.into) {
            self.tab_held = None;
            self.tab_preview_off();
            return;
        }
        if h.ripen(now) {
            let rows = self.state.plugin_tabs.merged(h.row, h.into);
            self.state.tab_preview = Some(crate::plugin_tabs::Preview { row: h.row, rows });
            self.invalidate();
        }
        self.tab_held = Some(h);
    }

    /// The button came up: a held window that was showing the group makes it, where it was let go (the other
    /// window comes to it).
    #[cfg(all(lemon_cards, plugin_windows))]
    fn tab_hold_finish(&mut self) {
        let held = self.tab_held.take();
        self.tab_preview_off();
        let Some(h) = held.filter(|h| h.ripe) else { return };
        let (true, true, Some(kind)) = (self.state.embeds.contains_key(&h.row), self.state.embeds.contains_key(&h.into), self.state.session.plugins.descriptor(h.row).map(|d| d.name)) else { return };
        self.state.plugin_tabs.merge(h.row, h.into, &kind);
        self.state.raise(Win::Hosted(h.row));
        self.invalidate();
    }

    /// The wheel over a plugin window drawn in the app's (or with its editor's menu open) turns there.
    #[cfg(lemon_cards)]
    pub(crate) fn embed_wheel(&mut self, dy: f32) -> bool {
        let (x, y) = self.mouse;
        let Some(row) = self.embed_modal().or_else(|| self.embed_at(x, y)) else { return false };
        if let Some(e) = self.state.embeds.get(&row) {
            e.wheel(x, y, dy, self.embed_mods());
        }
        true
    }

    /// The plugin editor drawn in the app's window that takes the keys: one with its menu or box open, or the one in
    /// front while a text field of it has the keyboard.
    #[cfg(lemon_cards)]
    pub(crate) fn embed_keys_to(&self) -> Option<usize> {
        let typing = match self.state.z.last() {
            Some(&Win::Hosted(r)) if !self.state.no_focus && self.state.embeds.get(&r).is_some_and(|e| e.view().wants_keys) => Some(r),
            _ => None,
        };
        self.embed_modal().or(typing)
    }

    /// Keys go to a plugin editor drawn in the app's window while its menu or box is open, or while a text
    /// field of it has the keyboard and its window is in front. Whether it took the key.
    #[cfg(lemon_cards)]
    pub(crate) fn embed_key(&mut self, key: &crate::keys::Key, ctrl: bool, alt: bool) -> bool {
        let Some(row) = self.embed_keys_to() else { return false };
        let Some((sym, text)) = keysym_of(key) else { return true };
        if let Some(e) = self.state.embeds.get(&row) {
            e.key(sym, text, (ctrl, self.shift, alt));
        }
        true
    }
}

/// A key as an X11 key symbol and the character it types, for a plugin editor drawn in the app's window.
#[cfg(lemon_cards)]
fn keysym_of(key: &crate::keys::Key) -> Option<(u32, Option<char>)> {
    use crate::keys::{Key, NamedKey};
    use os_plugkit::input::keysym;
    Some(match key {
        Key::Named(n) => (
            match n {
                NamedKey::Escape => keysym::ESCAPE,
                NamedKey::Enter => keysym::RETURN,
                NamedKey::Backspace => keysym::BACKSPACE,
                NamedKey::Delete => keysym::DELETE,
                NamedKey::ArrowLeft => keysym::LEFT,
                NamedKey::ArrowRight => keysym::RIGHT,
                NamedKey::ArrowUp => keysym::UP,
                NamedKey::ArrowDown => keysym::DOWN,
                NamedKey::Home => keysym::HOME,
                NamedKey::End => keysym::END,
                NamedKey::Space => 0x20,
                _ => return None,
            },
            (*n == NamedKey::Space).then_some(' '),
        ),
        Key::Character(c) => {
            let ch = c.chars().next()?;
            let sym = if (ch as u32) < 0x100 { ch as u32 } else { 0x0100_0000 + ch as u32 };
            (sym, Some(ch))
        }
        _ => return None,
    })
}
