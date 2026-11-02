// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The editor's window glue: Lemon Studio's plugin editor window (`os_plugkit::gui`, a child of the host's window on
//! its own GUI thread) painting `zest_ui` with the os-ui canvas; the parameters as `zest_ui::Model` (`os_plugkit`'s
//! values with the editor's pending edits, heard at once through the params' `LiveEdits`); preset files loaded and
//! saved; the host link (the three pages as tabs in Lemon Studio's plugin strip); host-driven resizing.
//!
//! Everything else of the editor (pages, painting, input, the library) is `zest_ui`, which does not know zest-core.

use std::any::Any;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use nih_plug::prelude::*;
use os_plugkit::vals::Vals;
use serde::{Deserialize, Serialize};
use zest_core::params::{COUNT, PARAMS};
use zest_ui::view;
use zest_ui::{ParamInfo, ParamTable};

use crate::params_gen::ZestParams;
use crate::PendingSlop;

/// What the editor keeps in the plugin's state: its size (logical px, the standalone editor's), page and preset.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Memo {
    pub w: u32,
    pub h: u32,
    pub page: usize,
    pub preset: Option<String>,
    /// The analog spread of the preset loaded (`spread_text`), so a reopened project sounds the same.
    #[serde(default)]
    pub spread: Option<String>,
}

impl Default for Memo {
    fn default() -> Self {
        Memo { w: view::BASE_W as u32, h: view::BASE_H as u32, page: 0, preset: None, spread: None }
    }
}

/// The largest editor the host may make.
const MAX_W: u32 = 2560;
const MAX_H: u32 = 1600;

/// The editor's parameter metadata, from zest-core's table.
pub fn table() -> ParamTable {
    ParamTable::new(
        PARAMS
            .iter()
            .map(|d| ParamInfo {
                key: d.key.into(),
                name: d.name.into(),
                group: d.group.into(),
                min: d.min,
                max: d.max,
                default: d.default,
                stepped: d.stepped,
                labels: d.labels.iter().map(|s| s.to_string()).collect(),
                hidden: d.hidden,
            })
            .collect(),
    )
}

// -------------------------------------------------------------------------------------------------------------------
// Presets

/// The current values (what the editor shows), in `PARAMS` order.
fn current_values(m: &dyn zest_ui::Model) -> [f32; COUNT] {
    std::array::from_fn(|i| m.get(PARAMS[i].key))
}

/// Sets every parameter that differs from `vals` (one gesture each).
fn apply(m: &mut dyn zest_ui::Model, vals: &[f32; COUNT]) {
    let cur = current_values(m);
    for ((d, v), c) in PARAMS.iter().zip(vals).zip(cur) {
        let v = v.clamp(d.min, d.max);
        if (v - c).abs() > 1e-7 {
            m.begin(d.key);
            m.set(d.key, v);
            m.end(d.key);
        }
    }
}

/// The values as a user preset: those that differ from their default, and the spread loaded with the last preset.
pub fn preset_text(vals: &[f32; COUNT], spread: zest_preset::Pots) -> String {
    let params = PARAMS.iter().zip(vals.iter()).filter(|(d, v)| **v != d.default).map(|(d, v)| (d.key.to_owned(), *v)).collect();
    zest_preset::write(&zest_preset::PresetFile { head: Default::default(), spread, params })
}

/// The spread alone, as the plugin's state keeps it.
pub fn spread_text(spread: zest_preset::Pots) -> String {
    zest_preset::write(&zest_preset::PresetFile { spread, ..Default::default() })
}

/// The author named in a Diva preset's metadata (`Author:` then `'name'` in the header comment).
pub fn h2p_author(text: &str) -> Option<String> {
    let head = text.split("*/").next()?;
    let mut lines = head.lines();
    while let Some(l) = lines.next() {
        if l.trim() == "Author:" {
            let a = lines.next()?.trim().trim_matches('\'').trim().to_string();
            return (!a.is_empty()).then_some(a);
        }
    }
    None
}

/// What loading a preset file gives besides the values.
#[derive(Debug, PartialEq)]
pub struct Loaded {
    pub author: Option<String>,
    pub spread: zest_preset::Pots,
}

/// Loads a preset file into the parameters: a Diva preset the way Diva loads it (over the startup state, keeping
/// Accuracy, OfflineAcc and MultiCore; the pots and Rnd flags wait in `pending` for the audio thread), or a Zest preset
/// (built in or from disk) the same way.
pub fn load(path: &Path, m: &mut dyn zest_ui::Model, pending: &PendingSlop) -> Result<Loaded, String> {
    let text = zest_preset::read(path)?;
    let cur = current_values(m);
    if path.extension().is_some_and(|x| x.eq_ignore_ascii_case("h2p")) {
        let vals = zest_core::h2p::load(&text, &cur)?;
        apply(m, &vals);
        let slop = zest_core::slop::parse_preset(&text);
        *pending.lock().unwrap() = Some(slop);
        Ok(Loaded { author: h2p_author(&text), spread: slop.into() })
    } else {
        let f = zest_preset::parse(&text)?;
        let vals = zest_core::h2p::zest_values(&f, &cur);
        apply(m, &vals);
        *pending.lock().unwrap() = Some(f.spread.into());
        Ok(Loaded { author: f.head.author, spread: f.spread })
    }
}

pub fn save(path: &Path, m: &dyn zest_ui::Model, spread: zest_preset::Pots) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, preset_text(&current_values(m), spread)).map_err(|e| e.to_string())
}

// -------------------------------------------------------------------------------------------------------------------
// The parameters as the editor's model

/// `zest_ui::Model` over the host's parameters: keys to nih-plug ids, plain values through `Vals`.
pub struct PluginModel {
    pub vals: Vals,
    ids: HashMap<&'static str, &'static str>,
}

impl PluginModel {
    pub fn new(params: &ZestParams, ctx: Option<Arc<dyn GuiContext>>) -> Self {
        PluginModel { vals: Vals::new(params, params.live.clone(), ctx), ids: PARAMS.iter().map(|d| (d.key, d.id)).collect() }
    }

    fn id(&self, key: &str) -> Option<&'static str> {
        self.ids.get(key).copied()
    }
}

impl zest_ui::Model for PluginModel {
    fn get(&self, key: &str) -> f32 {
        self.id(key).map_or(0.0, |id| self.vals.plain(id))
    }
    fn set(&mut self, key: &str, plain: f32) {
        if let Some(id) = self.id(key) {
            let n = self.vals.plain_to_norm(id, plain);
            self.vals.set(id, n);
        }
    }
    fn begin(&mut self, key: &str) {
        if let Some(id) = self.id(key) {
            self.vals.begin(id);
        }
    }
    fn end(&mut self, key: &str) {
        if let Some(id) = self.id(key) {
            self.vals.end(id);
        }
    }
    fn text(&self, key: &str, plain: f32) -> Option<String> {
        let id = self.id(key)?;
        Some(self.vals.text_of(id, self.vals.plain_to_norm(id, plain)))
    }
}

// -------------------------------------------------------------------------------------------------------------------
// nih-plug's editor

pub struct ZestEditor {
    params: Arc<ZestParams>,
    pending: PendingSlop,
    scope: Arc<crate::scope::Scope>,
    /// DPI scale as f32 bits.
    scale: AtomicU32,
    #[cfg(target_os = "linux")]
    slot: os_plugkit::gui::Slot,
}

impl ZestEditor {
    pub fn new(params: Arc<ZestParams>, pending: PendingSlop, scope: Arc<crate::scope::Scope>) -> Self {
        ZestEditor {
            params,
            pending,
            scope,
            scale: AtomicU32::new(1f32.to_bits()),
            #[cfg(target_os = "linux")]
            slot: Default::default(),
        }
    }

    fn notify(&self) {
        #[cfg(target_os = "linux")]
        os_plugkit::gui::notify(&self.slot);
    }

    fn memo(&self) -> Memo {
        self.params.editor.read().map(|m| m.clone()).unwrap_or_default()
    }
}

/// A size the editor takes (logical px).
fn clamp_size(w: u32, h: u32) -> (u32, u32) {
    (w.clamp(view::MIN_W as u32, MAX_W), h.clamp(view::MIN_H as u32, MAX_H))
}

impl Editor for ZestEditor {
    fn spawn(&self, parent: ParentWindowHandle, context: Arc<dyn GuiContext>) -> Box<dyn Any + Send> {
        let scale = f32::from_bits(self.scale.load(Ordering::Relaxed));
        match parent {
            #[cfg(target_os = "linux")]
            ParentWindowHandle::X11Window(window) => Box::new(gui::open(window, scale, self.params.clone(), self.pending.clone(), self.scope.clone(), context, &self.slot)),
            _ => {
                let _ = (context, scale);
                Box::new(())
            }
        }
    }

    fn size(&self) -> (u32, u32) {
        let m = self.memo();
        clamp_size(m.w, m.h)
    }

    fn set_scale_factor(&self, factor: f32) -> bool {
        self.scale.store(factor.to_bits(), Ordering::Relaxed);
        true
    }

    fn param_value_changed(&self, _id: &str, _normalized_value: f32) {
        self.notify();
    }
    fn param_modulation_changed(&self, _id: &str, _modulation_offset: f32) {
        self.notify();
    }
    fn param_values_changed(&self) {
        self.notify();
    }
}

#[cfg(target_os = "linux")]
mod gui {
    use std::path::PathBuf;
    use std::sync::Arc;

    use nih_plug::prelude::GuiContext;
    use os_hostlink::{Cmd, PageIcon, Peer};
    use os_plugkit::gui::{self as px, Button, Input, Window};
    use os_plugkit::MenuDef;
    use os_theme::Palette;
    use os_ui::kit::menu::MenuStyle;
    use os_ui::Canvas;
    use zest_ui::view;
    use zest_ui::{Act, Btn, Mods, Ui};

    use super::{PluginModel, ZestParams};
    use crate::PendingSlop;

    /// The page tabs the host shows: Main (plug), Effects (waveform), Advanced (node graph).
    const PAGE_ICONS: [PageIcon; 3] = [PageIcon::Plugin, PageIcon::Sample, PageIcon::Envelope];

    struct Ed {
        win: Window,
        canvas: Canvas,
        buf: Vec<u32>,
        scale: f32,
        /// Window size in device px.
        dev: (u32, u32),
        /// The size asked for (logical, standalone), last applied.
        asked: (u32, u32),
        params: Arc<ZestParams>,
        pending: PendingSlop,
        scope: Arc<crate::scope::Scope>,
        bands: crate::scope::Bands,
        model: PluginModel,
        ui: Ui,
        palette: Palette,
        last_sig: u64,
        last_hint: String,
        last_page: usize,
        dirty: bool,
        link: Arc<Peer>,
        /// Parameter keys by the id the host knows them by.
        host_ids: std::collections::HashMap<u32, &'static str>,
    }

    /// How long the hint line says what a turn of the host's changed.
    const REVEAL: std::time::Duration = std::time::Duration::from_millis(2500);

    impl Drop for Ed {
        fn drop(&mut self) {
            self.link.unregister();
        }
    }

    pub struct Handle(#[allow(dead_code)] Option<px::EditorThread>);

    pub fn open(parent: px::Parent, scale: f32, params: Arc<ZestParams>, pending: PendingSlop, scope: Arc<crate::scope::Scope>, ctx: Arc<dyn GuiContext>, slot: &px::Slot) -> Handle {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let memo = params.editor.read().map(|m| m.clone()).unwrap_or_default();
        let (w, h) = super::clamp_size(memo.w, memo.h);
        let dev = ((w as f32 * scale).round() as u32, (h as f32 * scale).round() as u32);
        let wake = slot.clone();
        // Joined to Lemon Studio's strip, the editor is what lies below its own tab row.
        let link = Peer::register(parent as u64, PAGE_ICONS.to_vec(), (w, h - view::STRIP as u32), move || px::notify(&wake));
        let peer = link.clone();
        let thread = px::spawn(parent, dev, slot, move |win| {
            let Ok(canvas) = Canvas::new(dev.0, dev.1, scale) else { return };
            let model = PluginModel::new(&params, Some(ctx));
            let mut ui = Ui::with_defaults(super::table());
            ui.set_page(memo.page);
            // Screenshots on a hidden display: open on another page.
            if let Some(p) = std::env::var("ZEST_EDITOR_PAGE").ok().and_then(|v| v.parse().ok()) {
                ui.set_page(p);
            }
            if let Some(p) = &memo.preset {
                ui.set_current(Some(std::path::Path::new(p)), None);
            }
            peer.set_page(ui.page);
            let mut ed = Ed {
                win,
                canvas,
                buf: Vec::new(),
                scale,
                dev,
                asked: (w, h),
                params,
                pending,
                scope,
                bands: Default::default(),
                model,
                ui,
                palette: os_plugkit::palette(),
                last_sig: 0,
                last_hint: String::new(),
                last_page: memo.page,
                host_ids: zest_core::params::PARAMS.iter().map(|d| (os_hostlink::host_param_id(d.id), d.key)).collect(),
                dirty: true,
                link: peer,
            };
            ed.relayout();
            ed.run();
        });
        if thread.is_none() {
            link.unregister();
        }
        Handle(thread)
    }

    fn btn(b: Button) -> Btn {
        match b {
            Button::Left => Btn::Left,
            Button::Right => Btn::Right,
            Button::Middle => Btn::Middle,
        }
    }

    impl Ed {
        /// The canvas's scale: device px per layout px (the zoom of small windows included).
        fn k(&self) -> f32 {
            let (z, _, _) = view::fit(self.dev.0 as f32 / self.scale, self.dev.1 as f32 / self.scale, self.ui.hosted);
            self.scale * z
        }

        /// Lays out for the window's size now.
        fn relayout(&mut self) {
            let (z, lw, lh) = view::fit(self.dev.0 as f32 / self.scale, self.dev.1 as f32 / self.scale, self.ui.hosted);
            self.ui.size = (lw, lh);
            self.canvas.resize(self.dev.0, self.dev.1, self.scale * z);
            self.dirty = true;
        }

        /// Resizes the window to `w` x `h` logical px.
        fn resize(&mut self, w: u32, h: u32) {
            let dev = ((w as f32 * self.scale).round() as u32, (h as f32 * self.scale).round() as u32);
            if dev != self.dev {
                self.dev = dev;
                self.win.resize(dev.0, dev.1);
            }
            self.relayout();
        }

        fn run(mut self) {
            while let Some(input) = self.win.next() {
                let k = self.k();
                match input {
                    Input::Expose => self.dirty = true,
                    Input::Tick => self.poll(),
                    Input::Down { x, y, btn: b, dbl } => {
                        let acts = self.ui.down(x / k, y / k, btn(b), self.mods(), dbl && b == Button::Left, &mut self.model);
                        self.act(acts);
                    }
                    Input::Move { x, y } => {
                        let acts = self.ui.moved(x / k, y / k, &self.model);
                        self.act(acts);
                    }
                    Input::Delta { dx, dy } => {
                        let acts = self.ui.delta(dx / k, dy / k, self.mods(), &mut self.model);
                        self.act(acts);
                    }
                    Input::Up { .. } => {
                        self.win.unlock();
                        let acts = self.ui.up(&mut self.model);
                        self.act(acts);
                    }
                    Input::Leave => {
                        if !self.win.locked() && !self.ui.dragging() {
                            let acts = self.ui.leave();
                            self.act(acts);
                        }
                    }
                    Input::Wheel { x, y, notches } => {
                        let acts = self.ui.wheel(x / k, y / k, notches, self.mods(), &mut self.model);
                        self.act(acts);
                    }
                    Input::Key { sym, text } => {
                        let acts = self.ui.key(sym, text);
                        self.act(acts);
                    }
                    Input::Drop { .. } => {}
                }
                if self.dirty && self.win.idle() {
                    self.paint();
                }
            }
            self.remember();
        }

        fn mods(&self) -> Mods {
            let (ctrl, shift, alt) = self.win.mods();
            Mods { ctrl, shift, alt }
        }

        fn screen(&self, x: f32, y: f32) -> (i32, i32) {
            let k = self.k();
            self.win.to_root((x * k) as i32, (y * k) as i32)
        }

        /// Keeps the page and the preset in the plugin's state.
        fn remember(&self) {
            if let Ok(mut m) = self.params.editor.write() {
                m.page = self.ui.page;
                m.preset = self.ui.current_path().map(|p| p.to_string_lossy().into_owned());
            }
        }

        /// Timer / wake: values changed elsewhere, the host link, a size the host asked for.
        fn poll(&mut self) {
            // The visualizer: a repaint only while its columns move (none while silent).
            let (levels, pitch, dt) = self.bands.next(&self.scope);
            if self.ui.viz.step(&levels, pitch, dt) {
                self.dirty = true;
            }
            if self.model.vals.expire() {
                self.dirty = true;
            }
            // The host turned a parameter: its page, and what changes in the hint line.
            if let Some(key) = self.link.take_reveal().and_then(|id| self.host_ids.get(&id).copied()) {
                self.ui.reveal_param(key, &self.model, std::time::Instant::now() + REVEAL);
                self.dirty = true;
            }
            if self.ui.reveal_over(std::time::Instant::now()) {
                self.dirty = true;
            }
            let sig = self.model.vals.signature();
            if sig != self.last_sig {
                self.last_sig = sig;
                self.dirty = true;
            }
            if self.ui.line() != self.last_hint {
                self.last_hint = self.ui.line().to_string();
                self.link.set_hint(&self.last_hint);
                self.link.to_host(Cmd::Changed, 0);
            }
            // Joined Lemon Studio's strip: the editor leaves its tab row and foot to the host and shrinks by them.
            if self.link.hosted() && !self.ui.hosted {
                self.ui.hosted = true;
                let (w, h) = self.asked;
                self.resize(w, h - (view::BASE_H - view::base_h(true)) as u32);
            }
            let page = self.link.page();
            if page != self.last_page {
                self.ui.set_page(page);
                self.last_page = page;
                self.remember();
                self.dirty = true;
            } else if self.ui.page != page {
                self.last_page = self.ui.page;
                self.link.set_page(self.last_page);
                self.link.to_host(Cmd::Changed, 0);
                self.remember();
            }
            // The host resized the window (set_size).
            let memo = self.params.editor.read().map(|m| (m.w, m.h)).unwrap_or(self.asked);
            if memo != self.asked && !self.ui.hosted {
                self.asked = memo;
                self.resize(memo.0, memo.1);
            }
        }

        fn act(&mut self, acts: Vec<Act>) {
            for a in acts {
                match a {
                    Act::Repaint => self.dirty = true,
                    Act::Lock => self.win.lock(),
                    Act::Page(_) => {
                        self.remember();
                        self.dirty = true;
                    }
                    Act::Menu(req, x, y) => {
                        let mut def = MenuDef::new(MenuStyle::List);
                        for (i, it) in req.items.iter().enumerate() {
                            def.add(it.clone(), i);
                        }
                        def.current = req.current;
                        let (sx, sy) = self.screen(x, y);
                        if let Some(row) = self.win.popup(sx, sy, &def, &self.palette, self.scale) {
                            let acts = self.ui.pick(&req, row, &mut self.model);
                            self.act(acts);
                        }
                        self.dirty = true;
                    }
                    Act::TypeIn(t, text, x, y) => {
                        let (sx, sy) = self.screen(x, y);
                        if let Some(s) = self.win.type_in(sx, sy, &text, self.scale) {
                            let acts = self.ui.typed(&t, &s, &mut self.model);
                            self.act(acts);
                        }
                        self.dirty = true;
                    }
                    Act::Load(path) => {
                        match super::load(&path, &mut self.model, &self.pending) {
                            Ok(got) => {
                                if let Ok(mut m) = self.params.editor.write() {
                                    m.spread = Some(super::spread_text(got.spread));
                                }
                                self.ui.loaded(&path, got.author)
                            }
                            Err(e) => self.ui.load_failed(&path, &e),
                        }
                        self.remember();
                        self.dirty = true;
                    }
                    Act::Save(path) => {
                        let path: PathBuf = path;
                        let spread = self.params.editor.read().ok().and_then(|m| m.spread.as_deref().and_then(|t| zest_preset::parse(t).ok())).map(|f| f.spread).unwrap_or_default();
                        match super::save(&path, &self.model, spread) {
                            Ok(()) => self.ui.saved(&path),
                            Err(e) => self.ui.save_failed(&e),
                        }
                        self.remember();
                        self.dirty = true;
                    }
                }
            }
        }

        fn paint(&mut self) {
            self.dirty = false;
            self.ui.paint(&mut self.canvas, &self.model);
            self.buf.resize((self.dev.0 * self.dev.1) as usize, 0);
            self.canvas.copy_to_xrgb(&mut self.buf);
            self.win.blit(&self.buf, self.dev.0, self.dev.1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zest_ui::{MemModel, Model, Ui};

    /// The editor painted as a PNG (`scale` = device px per layout px).
    fn save_png(ui: &mut Ui, m: &dyn Model, path: &Path, w: f32, h: f32) {
        let (z, lw, lh) = view::fit(w, h, ui.hosted);
        ui.size = (lw, lh);
        let (dw, dh) = (w.round() as u32, h.round() as u32);
        let mut c = os_ui::Canvas::new(dw, dh, z).unwrap();
        ui.paint(&mut c, m);
        // paint twice: the first paint finds the scroll ranges
        ui.paint(&mut c, m);
        let mut px = vec![0u32; (dw * dh) as usize];
        c.copy_to_xrgb(&mut px);
        let mut pm = tiny_skia::Pixmap::new(dw, dh).unwrap();
        for (o, p) in pm.data_mut().chunks_mut(4).zip(&px) {
            o.copy_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255]);
        }
        pm.save_png(path).unwrap();
    }

    /// `ZEST_PNG=<dir> [ZEST_PRESET=<.h2p>] cargo test -p zest-plugin -- --ignored render_pages`: every page with a
    /// Diva preset loaded, at the default size, a small size and a large one.
    #[test]
    #[ignore]
    fn render_pages() {
        let Some(dir) = std::env::var_os("ZEST_PNG").map(std::path::PathBuf::from) else { return };
        std::fs::create_dir_all(&dir).unwrap();
        let favs = dir.join("favorites.txt");
        let mut ui = Ui::new(table(), zest_ui::library::default_sources(), favs);
        // the visualizer as a sound would light it
        // (ZEST_VIZ_PITCH: the pitch to show; ZEST_VIZ_FRAMES: stop that many 30 ms frames into the zoom from 2 x 2)
        let shape: [f32; zest_ui::viz::BANDS] = std::array::from_fn(|i| (1.0 - ((i as f32 - 4.0) / 11.0).powi(2)).clamp(0.1, 1.0));
        let var = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<f32>().ok());
        let frames = var("ZEST_VIZ_FRAMES").map_or(60, |f| f as usize);
        if var("ZEST_VIZ_FRAMES").is_some() {
            (0..60).for_each(|_| _ = ui.viz.step(&shape, Some(0.0), 0.03));
        }
        (0..frames).for_each(|_| _ = ui.viz.step(&shape, var("ZEST_VIZ_PITCH").or(Some(0.5)), 0.03));
        let mut mm = MemModel::new(&ui.table);
        let pending: PendingSlop = Default::default();
        let presets: Vec<String> = std::env::var("ZEST_PRESET").map(|s| s.split(',').map(str::to_string).collect()).unwrap_or_else(|_| {
            let d = zest_ui::library::diva_presets_dir().unwrap();
            ["HS Albert Hall Mini.h2p", "BS Deep Space Diva.h2p", "SG Chica Go.h2p", "HS Kromosaur.h2p", "1 BASS/HS Pazong.h2p"].iter().map(|f| d.join(f).to_string_lossy().into_owned()).collect()
        });
        for (n, p) in presets.iter().enumerate() {
            let path = Path::new(p);
            if !path.exists() {
                eprintln!("missing {p}");
                continue;
            }
            let got = load(path, &mut mm, &pending).unwrap();
            ui.loaded(path, got.author);
            if let Some(i) = ui.lib.find(path) {
                let _ = i;
            }
            for (page, name) in zest_ui::pages::PAGES.iter().enumerate() {
                ui.set_page(page);
                save_png(&mut ui, &mm, &dir.join(format!("zest_{n}_{}.png", name.to_lowercase())), view::BASE_W, view::BASE_H);
            }
            ui.set_page(0);
            save_png(&mut ui, &mm, &dir.join(format!("zest_{n}_main_small.png")), 760.0, 460.0);
            save_png(&mut ui, &mm, &dir.join(format!("zest_{n}_main_large.png")), 1280.0, 720.0);
            ui.hosted = true;
            save_png(&mut ui, &mm, &dir.join(format!("zest_{n}_main_hosted.png")), view::BASE_W, view::base_h(true));
            ui.hosted = false;
        }
    }

    #[test]
    fn presets_round_trip_through_the_model() {
        let t = table();
        let mut mm = MemModel::new(&t);
        mm.values.insert("VCF1/Frequency".into(), 99.0);
        mm.values.insert("OSC/Model".into(), 2.0);
        let dir = std::env::temp_dir().join(format!("zest-plugin-test-{}", std::process::id()));
        let p = dir.join("My Sound.zest");
        let mut spread = zest_preset::Pots::default();
        spread.pots[2][3] = -12.5;
        spread.have[2][3] = true;
        save(&p, &mm, spread).unwrap();
        let mut back = MemModel::new(&t);
        let pending: PendingSlop = Default::default();
        let got = load(&p, &mut back, &pending).unwrap();
        assert_eq!(got, Loaded { author: None, spread });
        assert_eq!(back.get("VCF1/Frequency"), 99.0);
        assert_eq!(back.get("OSC/Model"), 2.0);
        assert_eq!(back.get("VCF1/Resonance"), mm.get("VCF1/Resonance"));
        assert_eq!(pending.lock().unwrap().unwrap().pots[2][3], -12.5, "the pots wait for the audio thread");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(h2p_author("/*@Meta\n\nAuthor:\n'Howard Scarr'\n\n*/\n#AM=Diva"), Some("Howard Scarr".into()));
        assert_eq!(h2p_author("#AM=Diva"), None);
    }
}
